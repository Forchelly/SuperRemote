//! 终端会话运行时：连接、网格、键盘编码。
//!
//! 这里刻意不碰 UI：`superremote-app` 的 `view` 只借用状态，所以网格帧必须在数据到达时
//! 物化到 [`Terminal::frame`]，绘制阶段只做投影，不再解析字节流。

use std::collections::HashSet;
use std::sync::{LazyLock, Mutex};

use iced::keyboard::{self, key, Key};
use superremote_core::session::{
    ResolvedTerminalSettings, TerminalBackspace, TerminalCharset, TerminalColorScheme,
    TerminalHomeEnd,
};
use superremote_net::ConnId;
use superremote_term::{Frame, Size, Terminal as Grid};

/// 量不出字体时的兜底步进（Consolas 的实际值是 0.55 em）。
const FALLBACK_ADVANCE: f32 = 0.55;
const FALLBACK_LINE: f32 = 1.35;

/// 判断提示符时保留的尾部文本长度。够放下一整行提示符，又不至于越攒越多。
const TAIL_LIMIT: usize = 256;

/// 单元格度量。终端所有几何都由这两个数推出来。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Metrics {
    pub cell_width: f32,
    pub cell_height: f32,
    /// 字形基线相对单元格顶部的偏移，让文字在格子里垂直居中。
    pub baseline: f32,
}

impl Metrics {
    /// 按字体族与字号量一次。每个终端创建时量一次，不跟着绘制每帧跑。
    pub fn for_font(family: &str, font_size: f32) -> Self {
        measure(family, font_size)
    }

    /// 给定像素框，算出能塞下多少列、多少行。
    pub fn grid_size(&self, width: f32, height: f32) -> (u16, u16) {
        let cols = (width / self.cell_width).floor().max(1.0);
        let rows = (height / self.cell_height).floor().max(1.0);
        (
            cols.min(u16::MAX as f32) as u16,
            rows.min(u16::MAX as f32) as u16,
        )
    }
}

fn measure(font_family: &str, font_size: f32) -> Metrics {
    use cosmic_text::{Attrs, Buffer, Family, FontSystem, Metrics as TextMetrics, Shaping};

    const SAMPLES: f32 = 16.0;
    let font_size = font_size.clamp(6.0, 48.0);
    let cell_height = font_size * FALLBACK_LINE;
    let fallback = Metrics {
        cell_width: font_size * FALLBACK_ADVANCE,
        cell_height,
        baseline: font_size,
    };

    let mut font_system = FontSystem::new();
    let mut buffer = Buffer::new(&mut font_system, TextMetrics::new(font_size, cell_height));
    buffer.set_size(&mut font_system, Some(font_size * SAMPLES), None);
    buffer.set_text(
        &mut font_system,
        &"M".repeat(SAMPLES as usize),
        &Attrs::new().family(Family::Name(font_family)),
        Shaping::Advanced,
        None,
    );
    buffer.shape_until_scroll(&mut font_system, false);

    let Some(run) = buffer.layout_runs().next() else {
        return fallback;
    };
    if run.line_w <= 0.0 {
        return fallback;
    }

    // 用取整到 0.01 的步进，避免浮点尾巴让列位置每帧抖动。
    let advance = (run.line_w / SAMPLES * 100.0).round() / 100.0;
    Metrics {
        cell_width: advance.max(1.0),
        cell_height: run.line_height.max(1.0),
        baseline: run.line_y,
    }
}

/// 终端里的一个格子坐标（可见区域，不含回滚历史）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellPos {
    pub row: usize,
    pub col: usize,
}

/// 鼠标拖选出来的线性选择区。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    pub anchor: CellPos,
    pub cursor: CellPos,
}

impl Selection {
    pub fn is_empty(self) -> bool {
        self.anchor == self.cursor
    }

    /// 起点/终点按行列排序，反向拖选也能得到同一段文本。
    pub fn normalized(self) -> (CellPos, CellPos) {
        let a = (self.anchor.row, self.anchor.col);
        let b = (self.cursor.row, self.cursor.col);
        let (start, end) = if a <= b { (a, b) } else { (b, a) };
        (
            CellPos {
                row: start.0,
                col: start.1,
            },
            CellPos {
                row: end.0,
                col: end.1,
            },
        )
    }

    pub fn contains(self, row: usize, col: usize) -> bool {
        let (start, end) = self.normalized();
        if row < start.row || row > end.row {
            return false;
        }
        if start.row == end.row {
            return col >= start.col && col <= end.col;
        }
        if row == start.row {
            return col >= start.col;
        }
        if row == end.row {
            return col <= end.col;
        }
        true
    }
}

/// 连接状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// 正在建立连接（包括等口令）。
    Connecting,
    /// 已进入远端 shell，可以输入。
    Live,
    /// 会话正常结束。
    Closed(Option<i32>),
    /// 连接失败。
    Failed(String),
}

impl Status {
    pub fn is_live(&self) -> bool {
        matches!(self, Self::Live)
    }

    pub fn is_settled(&self) -> bool {
        matches!(self, Self::Closed(_) | Self::Failed(_))
    }

    /// SSH 侧靠提示符判断：连接层从不发 `Ready`（认证完成与否是 `ssh.exe` 的
    /// 内部状态，进程外拿不到），所以这是 SSH 从「连接中」转「已连接」的唯一信号。
    ///
    /// 只看**最后一行**、且只认「行尾正好是提示符字符」的形状。早先用
    /// `text.contains("$ ")` 做过全串查找，`ps aux` 或一段引用的邮件正文里出现
    /// `> ` 就会把状态提前判成已登录。
    fn is_prompt(text: &str) -> bool {
        let Some(last) = text.lines().rev().find(|line| !line.trim().is_empty()) else {
            return false;
        };

        last.trim_end()
            .chars()
            .next_back()
            .is_some_and(|tail| matches!(tail, '$' | '#' | '>'))
    }
}

/// 一个标签背后的终端会话。
pub struct Terminal {
    pub conn: Option<ConnId>,
    pub grid: Grid,
    /// 物化后的可见帧。只在数据到达时更新，`view` 直接读。
    pub frame: Frame,
    pub status: Status,
    pub cols: u16,
    pub rows: u16,
    /// 连接时固化的终端设置。字体/配色改变后需要重新连接才会生效。
    pub settings: ResolvedTerminalSettings,
    /// Iced 0.14 的 `Font::with_name` 只接受 `&'static str`，所以字体名按进程内缓存一次。
    pub font_family: &'static str,
    pub metrics: Metrics,
    /// 一次连接里收到的字节数，用于在状态栏显示流量。
    pub received: u64,
    /// 最近一小段输出文本，用来判断提示符——提示符经常被拆在两次读里。
    tail: String,
    /// 非 UTF-8 字符集的**增量**解码器。UTF-8 是 `None`，走 `from_utf8_lossy` 原路。
    transcoder: Option<encoding_rs::Decoder>,
}

impl Terminal {
    pub fn new(cols: u16, rows: u16, settings: ResolvedTerminalSettings) -> Self {
        let metrics = Metrics::for_font(&settings.font_family, settings.font_size as f32);
        let font_family = intern_font(&settings.font_family);
        let palette = grid_palette(settings.color_scheme);
        let transcoder = transcoder_for(settings.charset);
        let grid = Grid::with_palette(Size::new(cols as usize, rows as usize), palette);
        let frame = grid.frame();
        Self {
            conn: None,
            grid,
            frame,
            status: Status::Connecting,
            cols,
            rows,
            settings,
            font_family,
            metrics,
            received: 0,
            tail: String::new(),
            transcoder,
        }
    }

    /// 收到远端字节：按字符集解码、喂网格并刷新帧。
    pub fn feed(&mut self, bytes: &[u8]) {
        self.received += bytes.len() as u64;
        let text = self.decode_chunk(bytes);
        // 解码结果统一按 UTF-8 再喂给网格：`alacritty_terminal` 的解析器只认 UTF-8，
        // 直接把 GBK 字节丢进去会整片变成替换字符。
        self.grid.feed(text.as_bytes());
        self.frame = self.grid.frame();

        // 连接层只知道「ssh 进程还活着」，认证是否完成只能从提示符看出来。
        // 提示符经常被拆在两次读里，所以要拿累积的尾部一起判断。
        self.tail.push_str(&text);
        if self.tail.len() > TAIL_LIMIT {
            let cut = self.tail.len() - TAIL_LIMIT;
            let cut = (cut..self.tail.len())
                .find(|index| self.tail.is_char_boundary(*index))
                .unwrap_or(0);
            self.tail.drain(..cut);
        }

        if self.status == Status::Connecting && Status::is_prompt(&self.tail) {
            self.status = Status::Live;
        }
    }

    /// 解一块远端字节。`last` 传 `false`：连接还开着，没收完的半个字留给下一块。
    ///
    /// 输出缓冲按 `max_utf8_buffer_length` 的上界预留，所以 `OutputFull` 理论上不
    /// 会出现；万一出现（编码表上界算保守了），循环只推进真正吃掉的字节，
    /// 不存在「把没解的那截丢掉」这条路。
    fn decode_chunk(&mut self, bytes: &[u8]) -> String {
        let Some(decoder) = &mut self.transcoder else {
            return String::from_utf8_lossy(bytes).into_owned();
        };

        let mut out = String::new();
        let mut offset = 0usize;

        loop {
            let rest = &bytes[offset..];
            let capacity = decoder
                .max_utf8_buffer_length(rest.len())
                .unwrap_or_else(|| rest.len().saturating_mul(3) + 8);
            out.reserve(capacity);

            let (result, read, _replaced) = decoder.decode_to_string(rest, &mut out, false);
            if matches!(result, encoding_rs::CoderResult::InputEmpty) {
                break;
            }
            // 没有任何进展就停下：宁可少解一段，也不要把调用方挂在循环里。
            if read == 0 {
                break;
            }
            offset += read;
        }

        out
    }

    /// 能不能把键盘输入送给远端。
    ///
    /// `Connecting` 必须放行：远端可能停在 `password:` 提示上——这时既没有提示符，
    /// 也没有已保存的口令可代填，只能让用户对着画面手输。未保存口令的会话
    /// 若不放行就会永久停在「正在连接」，界面上没有任何补救入口。
    pub fn accepts_input(&self) -> bool {
        match self.status {
            Status::Live => true,
            Status::Connecting => self.received > 0,
            Status::Closed(_) | Status::Failed(_) => false,
        }
    }

    /// 连接层确认可以输入了。
    pub fn mark_ready(&mut self) {
        if !self.status.is_settled() {
            self.status = Status::Live;
        }
    }

    /// 视口尺寸变化：先重排网格，再刷新帧。
    pub fn resize(&mut self, cols: u16, rows: u16) {
        if cols == self.cols && rows == self.rows {
            return;
        }
        self.cols = cols.max(1);
        self.rows = rows.max(1);
        self.grid
            .resize(Size::new(self.cols as usize, self.rows as usize));
        self.frame = self.grid.frame();
    }

    /// 取选择区的文本。宽字符占位格跳过，逐行去掉行尾空白。
    pub fn selection_text(&self, selection: Selection) -> String {
        let (start, end) = selection.normalized();
        let mut lines = Vec::new();

        for row in start.row..=end.row {
            let Some(cells) = self.frame.rows.get(row) else {
                break;
            };
            let first = if row == start.row { start.col } else { 0 };
            let last = if row == end.row {
                end.col.min(cells.len().saturating_sub(1))
            } else {
                cells.len().saturating_sub(1)
            };
            if first > last {
                continue;
            }

            let line: String = cells[first..=last]
                .iter()
                .filter(|cell| !cell.attrs.spacer)
                .map(|cell| cell.c)
                .collect();
            lines.push(line.trim_end().to_owned());
        }

        lines.join("\n")
    }

    /// 把可见帧拍平成纯文本。宽字符占位格直接跳过，行尾空白去掉。
    ///
    /// 测试用它断言「屏幕上真的出现了某段文字」；整屏复制也走这条路。
    pub fn screen_text(&self) -> String {
        self.frame
            .rows
            .iter()
            .map(|row| {
                let line: String = row
                    .iter()
                    .filter(|cell| !cell.attrs.spacer)
                    .map(|cell| cell.c)
                    .collect();
                line.trim_end().to_owned()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// 向上/向下翻历史。`delta` 为正向上。
    pub fn scroll(&mut self, delta: i32) {
        self.grid.scroll(delta);
        self.frame = self.grid.frame();
    }
}

/// Iced 的字体族名要求 `&'static str`；同一个名字只泄漏一次，后续复用。
///
/// 字体名来自用户输入，所以设一个上限：超过之后一律回落默认字体。宁可字号/
/// 字形与设置不符，也不给「反复改字体名」留一条无界泄漏的路径。
const MAX_INTERNED_FONTS: usize = 32;

fn intern_font(name: &str) -> &'static str {
    static INTERNED: LazyLock<Mutex<HashSet<&'static str>>> =
        LazyLock::new(|| Mutex::new(HashSet::new()));

    let mut interned = INTERNED.lock().unwrap_or_else(|poison| poison.into_inner());
    if let Some(existing) = interned.get(name) {
        return existing;
    }
    if interned.len() >= MAX_INTERNED_FONTS {
        return "Consolas";
    }

    let leaked: &'static str = Box::leak(name.to_owned().into_boxed_str());
    interned.insert(leaked);
    leaked
}

fn grid_palette(scheme: TerminalColorScheme) -> superremote_term::Palette {
    match scheme {
        TerminalColorScheme::Default => superremote_term::Palette::default_palette(),
        TerminalColorScheme::SolarizedDark => superremote_term::Palette::solarized_dark(),
        TerminalColorScheme::SolarizedLight => superremote_term::Palette::solarized_light(),
        TerminalColorScheme::LinuxConsole => superremote_term::Palette::linux_console(),
        TerminalColorScheme::Xterm => superremote_term::Palette::xterm(),
    }
}

/// 把一次按键翻译成要发给远端的字节。
///
/// 只处理终端真正需要的部分：可打印字符、控制键、方向键与功能键。
/// 修饰键本身不产生输入，返回 `None`。
pub fn encode_key(event: &keyboard::Event, settings: &ResolvedTerminalSettings) -> Option<Vec<u8>> {
    let keyboard::Event::KeyPressed {
        key,
        modifiers,
        text,
        ..
    } = event
    else {
        return None;
    };

    let ctrl = modifiers.control();
    let alt = modifiers.alt();

    // 优先用系统给出的文本，输入法、组合字符都能正确落到终端。
    if let Some(text) = text {
        if !ctrl {
            let mut bytes = Vec::with_capacity(text.len() + 1);
            if alt {
                bytes.push(0x1b);
            }
            bytes.extend_from_slice(text.as_bytes());
            return Some(bytes);
        }
    }

    match key {
        Key::Character(value) => {
            let chars: Vec<char> = value.chars().collect();
            if ctrl {
                let byte = control_byte(chars.first().copied()?)?;
                return Some(vec![byte]);
            }
            if chars.is_empty() {
                return None;
            }
            let mut bytes = Vec::with_capacity(value.len() + 1);
            if alt {
                bytes.push(0x1b);
            }
            bytes.extend_from_slice(value.as_bytes());
            Some(bytes)
        }
        Key::Named(named) => {
            let sequence: &[u8] = match named {
                key::Named::Enter => b"\r",
                key::Named::Tab => b"\t",
                key::Named::Backspace => match settings.backspace {
                    TerminalBackspace::Delete => b"\x7f",
                    TerminalBackspace::Backspace => b"\x08",
                },
                key::Named::Escape => b"\x1b",
                key::Named::Space => b" ",
                key::Named::ArrowUp => b"\x1b[A",
                key::Named::ArrowDown => b"\x1b[B",
                key::Named::ArrowRight => b"\x1b[C",
                key::Named::ArrowLeft => b"\x1b[D",
                key::Named::Home => match settings.home_end {
                    TerminalHomeEnd::Standard => b"\x1b[H",
                    TerminalHomeEnd::Rxvt => b"\x1b[1~",
                },
                key::Named::End => match settings.home_end {
                    TerminalHomeEnd::Standard => b"\x1b[F",
                    TerminalHomeEnd::Rxvt => b"\x1b[4~",
                },
                key::Named::Insert => b"\x1b[2~",
                key::Named::Delete => b"\x1b[3~",
                key::Named::PageUp => b"\x1b[5~",
                key::Named::PageDown => b"\x1b[6~",
                // F1–F4 的序列随「功能键与小键盘」设置变化（旧版 Putty 同名选项）。
                key::Named::F1 => settings.keypad.function_key(1),
                key::Named::F2 => settings.keypad.function_key(2),
                key::Named::F3 => settings.keypad.function_key(3),
                key::Named::F4 => settings.keypad.function_key(4),
                key::Named::F5 => b"\x1b[15~",
                key::Named::F6 => b"\x1b[17~",
                key::Named::F7 => b"\x1b[18~",
                key::Named::F8 => b"\x1b[19~",
                key::Named::F9 => b"\x1b[20~",
                key::Named::F10 => b"\x1b[21~",
                key::Named::F11 => b"\x1b[23~",
                key::Named::F12 => b"\x1b[24~",
                // 修饰键、锁定键、媒体键都不该把字节塞到 shell 里。
                _ => return None,
            };

            let mut bytes = Vec::with_capacity(sequence.len() + 1);
            if alt {
                bytes.push(0x1b);
            }
            bytes.extend_from_slice(sequence);
            Some(bytes)
        }
        Key::Unidentified => None,
    }
}

/// 按字符集建增量解码器；UTF-8 返回 `None`（走原路，不为最常见的路径多付一次拷贝）。
///
/// 两个刻意的选择：
/// 1. **增量**而不是整块一次性解。远端字节是按 16ms 轮询一块块到的，一个 GBK
///    汉字正好跨在两块之间时，一次性解码会把前半个字丢掉、后半个再解出替换字符，
///    屏幕上就是随机乱码。`Decoder` 把没收完的字节留在自己的状态里。
/// 2. `new_decoder_without_bom_handling` 而不是 `Encoding::decode`。后者会嗅 BOM：
///    GBK 流里恰好出现 `FF FE`（UTF-16LE BOM）时，它整块改按 UTF-16 解，内容凭空
///    消失。终端面对的是字节流不是文件，不该做 BOM 嗅探。
fn transcoder_for(charset: TerminalCharset) -> Option<encoding_rs::Decoder> {
    if !charset.needs_transcode() {
        return None;
    }

    let label = charset.encoding_label().as_bytes();
    let encoding = encoding_rs::Encoding::for_label(label).unwrap_or(encoding_rs::UTF_8);
    (encoding != encoding_rs::UTF_8).then(|| encoding.new_decoder_without_bom_handling())
}

/// `Ctrl` + 字符 的经典映射：`@ A–Z [ \ ] ^ _` 依次对应 0x00–0x1f。
fn control_byte(c: char) -> Option<u8> {
    match c {
        '@' => Some(0x00),
        'a'..='z' => Some(c as u8 - b'a' + 1),
        'A'..='Z' => Some(c as u8 - b'A' + 1),
        '[' => Some(0x1b),
        '\\' => Some(0x1c),
        ']' => Some(0x1d),
        '^' => Some(0x1e),
        '_' => Some(0x1f),
        ' ' => Some(0x00),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::keyboard::Modifiers;
    // 只有用例里用到，放顶层会让 lib 构建出现未使用告警。
    use superremote_core::session::TerminalKeypad;

    fn test_settings() -> ResolvedTerminalSettings {
        ResolvedTerminalSettings::default()
    }

    fn press(key: Key, modifiers: Modifiers) -> Option<Vec<u8>> {
        press_with(key, modifiers, &test_settings())
    }

    fn press_with(
        key: Key,
        modifiers: Modifiers,
        settings: &ResolvedTerminalSettings,
    ) -> Option<Vec<u8>> {
        let event = keyboard::Event::KeyPressed {
            key: key.clone(),
            modified_key: key,
            physical_key: key::Physical::Code(key::Code::KeyA),
            location: keyboard::Location::Standard,
            modifiers,
            text: None,
            repeat: false,
        };
        encode_key(&event, settings)
    }

    /// F1–F4 的序列跟着「功能键」设置走；F5 以上三种风格一致，不跟着变。
    #[test]
    fn function_keys_follow_the_keypad_setting() {
        for (keypad, expected) in [
            (TerminalKeypad::Standard, b"\x1bOP".as_slice()),
            (TerminalKeypad::EscN, b"\x1b[11~"),
            (TerminalKeypad::Linux, b"\x1b[[A"),
        ] {
            let mut settings = test_settings();
            settings.keypad = keypad;

            assert_eq!(
                press_with(Key::Named(key::Named::F1), Modifiers::NONE, &settings),
                Some(expected.to_vec()),
                "{keypad:?} 的 F1 序列不对"
            );
        }

        let mut settings = test_settings();
        settings.keypad = TerminalKeypad::Linux;
        assert_eq!(
            press_with(Key::Named(key::Named::F5), Modifiers::NONE, &settings),
            Some(b"\x1b[15~".to_vec()),
            "F5 不该受功能键设置影响"
        );
    }

    fn terminal_with_charset(charset: TerminalCharset) -> Terminal {
        let mut settings = test_settings();
        settings.charset = charset;
        Terminal::new(20, 4, settings)
    }

    /// 非 UTF-8 主机必须在**进网格之前**转码，否则解析器只看到非法 UTF-8。
    #[test]
    fn non_utf8_charsets_are_transcoded_before_the_grid() {
        // GBK 的「中文」= D6D0 CEC4；这串字节按 UTF-8 解只会得到替换字符。
        let mut gbk = terminal_with_charset(TerminalCharset::Gbk);
        assert_eq!(gbk.decode_chunk(&[0xD6, 0xD0, 0xCE, 0xC4]), "中文");

        let mut latin1 = terminal_with_charset(TerminalCharset::Latin1);
        assert_eq!(latin1.decode_chunk(&[0xE9]), "é");

        // UTF-8 走原路。
        let mut utf8 = terminal_with_charset(TerminalCharset::Utf8);
        assert_eq!(utf8.decode_chunk("会".as_bytes()), "会");
    }

    /// 一个汉字被拆在两次读里也必须解成同一个字。
    ///
    /// 一次性解码（`Encoding::decode`）在这种切法下会先吐一个替换字符、再把后半
    /// 解成另一个字，屏幕上就是随机乱码——所以解码器是按连接持有的。
    #[test]
    fn a_multibyte_character_split_across_reads_still_decodes() {
        let mut gbk = terminal_with_charset(TerminalCharset::Gbk);

        let first = gbk.decode_chunk(&[0xD6]);
        let second = gbk.decode_chunk(&[0xD0, 0xCE, 0xC4]);

        assert_eq!(first, "", "半个字不该提前吐东西");
        assert_eq!(first + &second, "中文");
    }

    /// GBK 流里出现 `FF FE` 不能被当成 UTF-16LE BOM 吞掉。
    ///
    /// 一次性 API（`Encoding::decode`）会嗅 BOM：`FF FE` 开头就整块改按 UTF-16LE
    /// 解，这两个字节在屏幕上什么都不留。按流解码时 `FF` 立刻换一个替换字符，
    /// `FE` 作为前导字节留在解码器里等下一个字节。
    #[test]
    fn a_utf16_bom_in_a_gbk_stream_is_not_sniffed() {
        let mut gbk = terminal_with_charset(TerminalCharset::Gbk);

        let text = gbk.decode_chunk(&[0xFF, 0xFE]);
        assert_eq!(
            text.chars().collect::<Vec<_>>(),
            vec!['\u{FFFD}'],
            "非法的 FF 必须立刻留下痕迹；FE 是前导字节，等后半"
        );
        assert!(
            !gbk.decode_chunk(&[0x41]).is_empty(),
            "留在解码器里的前导字节必须在下一块落地"
        );
    }

    #[test]
    fn printable_characters_go_out_as_utf8() {
        assert_eq!(
            press(Key::Character("a".into()), Modifiers::NONE),
            Some(b"a".to_vec())
        );
        // 中文按 UTF-8 原样送出，交给远端 locale 处理。
        assert_eq!(
            press(Key::Character("会".into()), Modifiers::NONE),
            Some("会".as_bytes().to_vec())
        );
    }

    #[test]
    fn only_a_trailing_prompt_character_counts_as_ready() {
        assert!(Status::is_prompt("ops@web-01:~$ "));
        assert!(Status::is_prompt("root@web-01:/# "));
        assert!(Status::is_prompt("[ops@web-01 ~]>"));

        // 提示符被拆在两次读里：等 `$` 到齐才认，半截的 `~` 不算。
        assert!(!Status::is_prompt("ops@web-01:~"));
        assert!(Status::is_prompt("ops@web-01:~$"));

        // 提示符字符出现在输出中间不算：早先的全串查找会在这里误判。
        assert!(!Status::is_prompt(
            "total 8\r\ndrwxr-xr-x 2 ops ops 4096 .\r\n"
        ));
        assert!(!Status::is_prompt(
            "> quoted reply from a mail\r\nnext line\r\n"
        ));
        assert!(!Status::is_prompt(""));
    }

    /// `Metrics` 的绝对值依赖实际安装的字体，钉死数字只会变成脆弱的快照；
    /// 这里只钉「量出来了、且量级正确」。iced 升级带来 cosmic-text 漂移时，
    /// 这条会先红，而不是等终端整屏错列才发现。
    #[test]
    fn measured_cell_metrics_stay_in_a_plausible_band() {
        let metrics = Metrics::for_font("Consolas", 13.0);
        let em = 13.0_f32;

        assert!(
            (0.4 * em..=0.95 * em).contains(&metrics.cell_width),
            "字符步进应落在 0.4–0.95 em：{metrics:?}"
        );
        assert!(
            (1.0 * em..=2.0 * em).contains(&metrics.cell_height),
            "行高应落在 1.0–2.0 em：{metrics:?}"
        );
        assert!(metrics.baseline > 0.0, "基线偏移应为正：{metrics:?}");
    }

    #[test]
    fn ctrl_letters_become_control_codes() {
        assert_eq!(
            press(Key::Character("c".into()), Modifiers::CTRL),
            Some(vec![0x03])
        );
        assert_eq!(
            press(Key::Character("D".into()), Modifiers::CTRL),
            Some(vec![0x04])
        );
        assert_eq!(
            press(Key::Character("[".into()), Modifiers::CTRL),
            Some(vec![0x1b])
        );
    }

    #[test]
    fn named_keys_map_to_the_sequences_readline_expects() {
        assert_eq!(
            press(Key::Named(key::Named::Enter), Modifiers::NONE),
            Some(b"\r".to_vec())
        );
        assert_eq!(
            press(Key::Named(key::Named::Backspace), Modifiers::NONE),
            Some(vec![0x7f])
        );
        assert_eq!(
            press(Key::Named(key::Named::ArrowUp), Modifiers::NONE),
            Some(b"\x1b[A".to_vec())
        );
    }

    #[test]
    fn modifier_only_keys_produce_no_input() {
        assert!(press(Key::Named(key::Named::Shift), Modifiers::SHIFT).is_none());
        assert!(press(Key::Named(key::Named::Control), Modifiers::CTRL).is_none());
    }

    #[test]
    fn alt_prefixes_escape_for_readline_word_motions() {
        assert_eq!(
            press(Key::Character("b".into()), Modifiers::ALT),
            Some(vec![0x1b, b'b'])
        );
    }

    #[test]
    fn metrics_produce_a_usable_grid() {
        let metrics = Metrics::for_font("Consolas", 13.0);
        assert!(
            metrics.cell_width > 3.0 && metrics.cell_width < 20.0,
            "步进应落在合理区间"
        );
        assert!(metrics.cell_height > 8.0 && metrics.cell_height < 40.0);

        let (cols, rows) = metrics.grid_size(800.0, 400.0);
        assert!(
            cols > 40 && cols < 200,
            "800px 宽应能放下几十列，实际 {cols}"
        );
        assert!(
            rows > 10 && rows < 60,
            "400px 高应能放下十几行，实际 {rows}"
        );
    }

    #[test]
    fn a_received_prompt_flips_the_session_to_live() {
        let mut terminal = Terminal::new(20, 5, test_settings());
        terminal.feed(b"user@host:/$ ");
        assert_eq!(terminal.status, Status::Live);
        assert_eq!(terminal.frame.rows[0][0].c, 'u');
    }

    #[test]
    fn a_password_prompt_does_not_count_as_being_connected() {
        let mut terminal = Terminal::new(60, 5, test_settings());
        terminal.feed(b"user@10.0.0.1's password:");
        assert_eq!(
            terminal.status,
            Status::Connecting,
            "还在等口令时不能报「已连接」"
        );
        terminal.mark_ready();
        assert_eq!(terminal.status, Status::Live);
    }

    /// 口令提示出来时必须允许输入：没有已保存凭据的会话全靠这一手。
    #[test]
    fn a_password_prompt_still_accepts_typed_input() {
        let mut terminal = Terminal::new(60, 5, test_settings());
        assert!(!terminal.accepts_input(), "还没收到任何输出时不接管键盘");

        terminal.feed(b"user@10.0.0.1's password:");
        assert_eq!(terminal.status, Status::Connecting);
        assert!(
            terminal.accepts_input(),
            "停在 password: 提示上必须能手输口令"
        );

        terminal.mark_ready();
        assert!(terminal.accepts_input());
    }

    /// 会话结束后键盘要交还给界面（搜索框、快捷键）。
    #[test]
    fn selection_text_covers_rows_and_skips_wide_spacers() {
        let mut terminal = Terminal::new(12, 3, test_settings());
        terminal.feed("hello\r\n会话 world".as_bytes());

        let selection = Selection {
            anchor: CellPos { row: 0, col: 0 },
            cursor: CellPos { row: 1, col: 6 },
        };
        assert_eq!(terminal.selection_text(selection), "hello\n会话 wo");

        // 反向拖选得到同一段文本。
        let reversed = Selection {
            anchor: CellPos { row: 1, col: 6 },
            cursor: CellPos { row: 0, col: 0 },
        };
        assert_eq!(terminal.selection_text(reversed), "hello\n会话 wo");
    }

    #[test]
    fn a_single_cell_selection_is_empty() {
        let cell = CellPos { row: 2, col: 3 };
        assert!(Selection {
            anchor: cell,
            cursor: cell
        }
        .is_empty());

        let selection = Selection {
            anchor: CellPos { row: 2, col: 3 },
            cursor: CellPos { row: 2, col: 5 },
        };
        assert!(selection.contains(2, 4));
        assert!(!selection.contains(1, 4));
        assert!(!selection.contains(2, 6));
    }

    #[test]
    fn a_finished_session_stops_accepting_input() {
        let mut terminal = Terminal::new(60, 5, test_settings());
        terminal.feed(b"$ ");
        terminal.mark_ready();
        assert!(terminal.accepts_input());

        terminal.status = Status::Closed(Some(0));
        assert!(!terminal.accepts_input());
    }
}
