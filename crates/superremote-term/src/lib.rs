//! SuperRemote 终端内核：ANSI 网格状态机，不依赖任何 UI 框架。
//!
//! 底层网格与转义序列解析直接复用 alacritty 的引擎（`alacritty_terminal`），
//! 这一层只做两件事：把字节流喂进网格，以及把可见区域物化成 [`Frame`]。
//! Iced 的 `view(&self)` 只能借用状态，所以渲染需要的东西必须在数据到达时物化，
//! 不能在绘制时临时计算——这也是 [`Terminal::frame`] 存在的原因。

use alacritty_terminal::event::VoidListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::Column;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{point_to_viewport, Config, Term, TermMode};
use alacritty_terminal::vte::ansi::{
    Color, CursorShape as AnsiCursorShape, NamedColor, Processor, Rgb as AlacrittyRgb,
};

/// 终端可见区域尺寸。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Size {
    pub cols: usize,
    pub rows: usize,
}

impl Size {
    pub const fn new(cols: usize, rows: usize) -> Self {
        Self { cols, rows }
    }
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.rows
    }

    fn screen_lines(&self) -> usize {
        self.rows
    }

    fn columns(&self) -> usize {
        self.cols
    }
}

/// 终端默认背景色，画布用它打底。
pub const fn backdrop() -> Rgb {
    Rgb {
        r: 0x14,
        g: 0x18,
        b: 0x1F,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// 向 `other` 混合，`amount` 为 0–256。
    fn mix(self, other: Self, amount: u16) -> Self {
        let blend = |a: u8, b: u8| ((a as u16 * (256 - amount) + b as u16 * amount) / 256) as u8;
        Self {
            r: blend(self.r, other.r),
            g: blend(self.g, other.g),
            b: blend(self.b, other.b),
        }
    }
}

impl From<AlacrittyRgb> for Rgb {
    fn from(value: AlacrittyRgb) -> Self {
        Self::new(value.r, value.g, value.b)
    }
}

/// 单元格属性。渲染器按这些开关决定字重、下划线与反色。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CellAttrs {
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub inverse: bool,
    pub hidden: bool,
    /// 宽字符（CJK）占位：该格占两列，渲染时要双倍宽。
    pub wide: bool,
    /// 宽字符的第二列占位格，渲染时跳过，只占位置。
    pub spacer: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    pub c: char,
    pub fg: Rgb,
    pub bg: Rgb,
    pub attrs: CellAttrs,
}

impl Cell {
    /// 空单元格，供行末补齐与占位。
    pub fn blank() -> Self {
        Self {
            c: ' ',
            fg: PALETTE.foreground,
            bg: backdrop(),
            attrs: CellAttrs::default(),
        }
    }
}

pub type Row = Vec<Cell>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorShape {
    Block,
    Underline,
    Beam,
    HollowBlock,
    Hidden,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cursor {
    pub row: usize,
    pub col: usize,
    pub shape: CursorShape,
}

/// 一帧可见内容：屏幕行、光标位置、配色，以及滚动回溯信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub rows: Vec<Row>,
    pub cursor: Option<Cursor>,
    /// 当前向上滚动了多少行（0 = 贴着底部）。
    pub offset: usize,
    /// 回滚缓冲区里的行数，用于判断还能往上翻多少。
    pub history: usize,
    /// 当前配色方案的背景色。画布打底用它，而不是全局深色常量。
    pub background: Rgb,
    /// 当前配色方案的光标色。
    pub cursor_color: Rgb,
}

impl Frame {
    pub fn empty(size: Size) -> Self {
        let palette = Palette::default_palette();
        Self {
            rows: vec![vec![Cell::blank(); size.cols]; size.rows],
            cursor: None,
            offset: 0,
            history: 0,
            background: palette.background,
            cursor_color: palette.cursor,
        }
    }
}

/// 终端状态：网格 + 解析器 + 配色。
pub struct Terminal {
    term: Term<VoidListener>,
    parser: Processor,
    size: Size,
    palette: Palette,
}

impl Terminal {
    pub fn new(size: Size) -> Self {
        Self::with_palette(size, Palette::default_palette())
    }

    pub fn with_palette(size: Size, palette: Palette) -> Self {
        let config = Config {
            scrolling_history: 10_000,
            ..Config::default()
        };

        Self {
            term: Term::new(config, &size, VoidListener),
            parser: Processor::new(),
            size,
            palette,
        }
    }

    pub fn palette(&self) -> Palette {
        self.palette
    }

    pub fn size(&self) -> Size {
        self.size
    }

    /// 把远端（或本地 PTY）送来的字节流喂进网格。
    pub fn feed(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.term, bytes);
    }

    pub fn resize(&mut self, size: Size) {
        if size.cols == 0 || size.rows == 0 || size == self.size {
            return;
        }
        self.term.resize(size);
        self.size = size;
    }

    /// 回滚：`delta` 为正表示向上翻。
    pub fn scroll(&mut self, delta: i32) {
        self.term
            .scroll_display(alacritty_terminal::grid::Scroll::Delta(delta));
    }

    /// 物化当前可见区域。
    ///
    /// 关键点是**按列号落位**：宽字符（CJK、emoji）在网格里占两列，第二列是
    /// `WIDE_CHAR_SPACER` 占位格。如果把它丢掉，之后的每一列都会左移一格，
    /// 表格类输出会整列错位，所以这里保留占位格并标记 [`CellAttrs::spacer`]，
    /// 由渲染器决定跳过绘制。
    pub fn frame(&self) -> Frame {
        let content = self.term.renderable_content();
        let palette = self.palette;
        let cols = self.size.cols;
        let blank_row = || {
            vec![
                Cell {
                    c: ' ',
                    fg: palette.foreground,
                    bg: palette.background,
                    attrs: CellAttrs::default(),
                };
                cols
            ]
        };
        let mut rows: Vec<Row> = Vec::with_capacity(self.size.rows);
        let mut row: Row = blank_row();
        let mut row_open = false;

        for indexed in content.display_iter {
            if indexed.point.column == Column(0) {
                if row_open {
                    rows.push(std::mem::replace(&mut row, blank_row()));
                }
                row_open = true;
            }

            let column = indexed.point.column.0;
            if column >= cols {
                continue;
            }

            let cell = indexed.cell;
            let mut fg = resolve(&cell.fg, &palette);
            let mut bg = resolve(&cell.bg, &palette);

            let attrs = CellAttrs {
                bold: cell.flags.contains(Flags::BOLD),
                dim: cell.flags.contains(Flags::DIM),
                italic: cell.flags.contains(Flags::ITALIC),
                underline: cell.flags.intersects(Flags::ALL_UNDERLINES),
                strike: cell.flags.contains(Flags::STRIKEOUT),
                inverse: cell.flags.contains(Flags::INVERSE),
                hidden: cell.flags.contains(Flags::HIDDEN),
                wide: cell.flags.contains(Flags::WIDE_CHAR),
                spacer: cell.flags.contains(Flags::WIDE_CHAR_SPACER),
            };

            if attrs.inverse {
                std::mem::swap(&mut fg, &mut bg);
            }
            if attrs.dim {
                fg = fg.mix(bg, 96);
            }

            row[column] = Cell {
                c: cell.c,
                fg,
                bg,
                attrs,
            };
        }

        if row_open {
            rows.push(row);
        }

        while rows.len() < self.size.rows {
            rows.push(blank_row());
        }
        rows.truncate(self.size.rows);

        let shape = match content.cursor.shape {
            AnsiCursorShape::Block => CursorShape::Block,
            AnsiCursorShape::Underline => CursorShape::Underline,
            AnsiCursorShape::Beam => CursorShape::Beam,
            AnsiCursorShape::HollowBlock => CursorShape::HollowBlock,
            AnsiCursorShape::Hidden => CursorShape::Hidden,
        };

        let cursor =
            point_to_viewport(content.display_offset, content.cursor.point).map(|point| Cursor {
                row: point.line,
                col: point.column.0,
                shape,
            });

        // 应用主动隐藏光标时（`vim` 退出前会这么做）不要画出闪烁方块。
        let cursor = if self.term.mode().contains(TermMode::SHOW_CURSOR) {
            cursor
        } else {
            None
        };

        Frame {
            rows,
            cursor,
            offset: content.display_offset,
            history: self.term.grid().history_size(),
            background: palette.background,
            cursor_color: palette.cursor,
        }
    }
}

/// 16 色 + 前景/背景/光标。默认配色与界面深色主题同源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub ansi: [Rgb; 16],
    pub foreground: Rgb,
    pub background: Rgb,
    pub cursor: Rgb,
}

impl Palette {
    pub const fn default_palette() -> Self {
        PALETTE
    }

    /// Solarized Dark 的标准 ANSI 16 色。
    pub const fn solarized_dark() -> Self {
        Self {
            ansi: [
                Rgb::new(0x07, 0x36, 0x42),
                Rgb::new(0xDC, 0x32, 0x2F),
                Rgb::new(0x85, 0x99, 0x00),
                Rgb::new(0xB5, 0x89, 0x00),
                Rgb::new(0x26, 0x8B, 0xD2),
                Rgb::new(0xD3, 0x36, 0x82),
                Rgb::new(0x2A, 0xA1, 0x98),
                Rgb::new(0xEE, 0xE8, 0xD5),
                Rgb::new(0x00, 0x2B, 0x36),
                Rgb::new(0xCB, 0x4B, 0x16),
                Rgb::new(0x58, 0x6E, 0x75),
                Rgb::new(0x65, 0x7B, 0x83),
                Rgb::new(0x83, 0x94, 0x96),
                Rgb::new(0x6C, 0x71, 0xC4),
                Rgb::new(0x93, 0xA1, 0xA1),
                Rgb::new(0xFD, 0xF6, 0xE3),
            ],
            foreground: Rgb::new(0x83, 0x94, 0x96),
            background: Rgb::new(0x00, 0x2B, 0x36),
            cursor: Rgb::new(0x93, 0xA1, 0xA1),
        }
    }

    /// Solarized Light 用同一套色相，但背景与前景互换。
    pub const fn solarized_light() -> Self {
        Self {
            ansi: [
                Rgb::new(0x07, 0x36, 0x42),
                Rgb::new(0xDC, 0x32, 0x2F),
                Rgb::new(0x85, 0x99, 0x00),
                Rgb::new(0xB5, 0x89, 0x00),
                Rgb::new(0x26, 0x8B, 0xD2),
                Rgb::new(0xD3, 0x36, 0x82),
                Rgb::new(0x2A, 0xA1, 0x98),
                Rgb::new(0xEE, 0xE8, 0xD5),
                Rgb::new(0x00, 0x2B, 0x36),
                Rgb::new(0xCB, 0x4B, 0x16),
                Rgb::new(0x58, 0x6E, 0x75),
                Rgb::new(0x65, 0x7B, 0x83),
                Rgb::new(0x83, 0x94, 0x96),
                Rgb::new(0x6C, 0x71, 0xC4),
                Rgb::new(0x93, 0xA1, 0xA1),
                Rgb::new(0xFD, 0xF6, 0xE3),
            ],
            foreground: Rgb::new(0x65, 0x7B, 0x83),
            background: Rgb::new(0xFD, 0xF6, 0xE3),
            cursor: Rgb::new(0x58, 0x6E, 0x75),
        }
    }

    /// Linux 控制台：EGA/VGA 硬件调色板（`AA` = 暗档，`FF` = 亮档）。
    ///
    /// 16 个色值取自 [Gentoo Wiki 的 VGA/xterm 对照表] 的 VGA 列，前 8 色是
    /// 0/AA 两档三原色组合，后 8 色加 55/FF 一档亮度。默认前景取 ANSI 7
    /// （`AAAAAA` 灰）——真实控制台的普通文字就是这个色，加粗才升到亮白；
    /// 光标用亮白，保证在灰字背景上仍然看得见。
    ///
    /// [Gentoo Wiki 的 VGA/xterm 对照表]: https://wiki.gentoo.org/wiki/Terminal_emulator/Colors
    pub const fn linux_console() -> Self {
        Self {
            ansi: [
                Rgb::new(0x00, 0x00, 0x00),
                Rgb::new(0xAA, 0x00, 0x00),
                Rgb::new(0x00, 0xAA, 0x00),
                Rgb::new(0xAA, 0x55, 0x00),
                Rgb::new(0x00, 0x00, 0xAA),
                Rgb::new(0xAA, 0x00, 0xAA),
                Rgb::new(0x00, 0xAA, 0xAA),
                Rgb::new(0xAA, 0xAA, 0xAA),
                Rgb::new(0x55, 0x55, 0x55),
                Rgb::new(0xFF, 0x55, 0x55),
                Rgb::new(0x55, 0xFF, 0x55),
                Rgb::new(0xFF, 0xFF, 0x55),
                Rgb::new(0x55, 0x55, 0xFF),
                Rgb::new(0xFF, 0x55, 0xFF),
                Rgb::new(0x55, 0xFF, 0xFF),
                Rgb::new(0xFF, 0xFF, 0xFF),
            ],
            foreground: Rgb::new(0xAA, 0xAA, 0xAA),
            background: Rgb::new(0x00, 0x00, 0x00),
            cursor: Rgb::new(0xFF, 0xFF, 0xFF),
        }
    }

    /// xterm 经典色板：白底黑字。
    ///
    /// 16 色同样取自上面那张对照表的 xterm 列（`CD`/`EE` 那一档原色）。默认前景
    /// 是黑、背景是白——这是 xterm 不接任何资源文件时的样子。代价与所有正统
    /// 白底方案一样：ANSI 7（`E5E5E5`）在白底上几乎看不见，那是这个方案的
    /// 既有行为，不是渲染 bug，所以我们没有偷偷把它调深。
    pub const fn xterm() -> Self {
        Self {
            ansi: [
                Rgb::new(0x00, 0x00, 0x00),
                Rgb::new(0xCD, 0x00, 0x00),
                Rgb::new(0x00, 0xCD, 0x00),
                Rgb::new(0xCD, 0xCD, 0x00),
                Rgb::new(0x00, 0x00, 0xEE),
                Rgb::new(0xCD, 0x00, 0xCD),
                Rgb::new(0x00, 0xCD, 0xCD),
                Rgb::new(0xE5, 0xE5, 0xE5),
                Rgb::new(0x7F, 0x7F, 0x7F),
                Rgb::new(0xFF, 0x00, 0x00),
                Rgb::new(0x00, 0xFF, 0x00),
                Rgb::new(0xFF, 0xFF, 0x00),
                Rgb::new(0x5C, 0x5C, 0xFF),
                Rgb::new(0xFF, 0x00, 0xFF),
                Rgb::new(0x00, 0xFF, 0xFF),
                Rgb::new(0xFF, 0xFF, 0xFF),
            ],
            foreground: Rgb::new(0x00, 0x00, 0x00),
            background: Rgb::new(0xFF, 0xFF, 0xFF),
            cursor: Rgb::new(0x00, 0x00, 0x00),
        }
    }
}

static PALETTE: Palette = Palette {
    ansi: [
        Rgb::new(0x18, 0x1C, 0x24), // black
        Rgb::new(0xE0, 0x6C, 0x75), // red
        Rgb::new(0x98, 0xC3, 0x79), // green
        Rgb::new(0xE5, 0xC0, 0x7B), // yellow
        Rgb::new(0x61, 0xAF, 0xEF), // blue
        Rgb::new(0xC6, 0x78, 0xDD), // magenta
        Rgb::new(0x56, 0xB6, 0xC2), // cyan
        Rgb::new(0xBB, 0xC2, 0xCF), // white
        Rgb::new(0x5C, 0x63, 0x70), // bright black
        Rgb::new(0xFF, 0x7B, 0x88), // bright red
        Rgb::new(0xB5, 0xE8, 0x90), // bright green
        Rgb::new(0xF0, 0xC6, 0x74), // bright yellow
        Rgb::new(0x74, 0xB7, 0xF5), // bright blue
        Rgb::new(0xD9, 0x9B, 0xEF), // bright magenta
        Rgb::new(0x6F, 0xD6, 0xDE), // bright cyan
        Rgb::new(0xEC, 0xF0, 0xF7), // bright white
    ],
    foreground: Rgb::new(0xDC, 0xE3, 0xEC),
    background: Rgb::new(0x14, 0x18, 0x1F),
    cursor: Rgb::new(0x4C, 0x8D, 0xF5),
};

/// 把终端里的颜色引用解析成具体 RGB。
fn resolve(color: &Color, palette: &Palette) -> Rgb {
    match color {
        Color::Spec(rgb) => (*rgb).into(),
        Color::Named(named) => match named {
            NamedColor::Foreground | NamedColor::BrightForeground => palette.foreground,
            NamedColor::Background => palette.background,
            NamedColor::Cursor => palette.cursor,
            NamedColor::DimForeground => palette.foreground.mix(palette.background, 96),
            NamedColor::DimBlack => palette.ansi[0].mix(palette.background, 96),
            NamedColor::DimRed => palette.ansi[1].mix(palette.background, 96),
            NamedColor::DimGreen => palette.ansi[2].mix(palette.background, 96),
            NamedColor::DimYellow => palette.ansi[3].mix(palette.background, 96),
            NamedColor::DimBlue => palette.ansi[4].mix(palette.background, 96),
            NamedColor::DimMagenta => palette.ansi[5].mix(palette.background, 96),
            NamedColor::DimCyan => palette.ansi[6].mix(palette.background, 96),
            NamedColor::DimWhite => palette.ansi[7].mix(palette.background, 96),
            // 走到这里的只应该是 `Black..=BrightWhite`（vte 里显式赋值 0–15）。
            // 但 `NamedColor` 是上游枚举，`as usize` 一旦遇到新变体（`Foreground`
            // 就已经是 256）就是越界 panic，所以宁可回落前景色也不直接下标。
            other => palette
                .ansi
                .get(*other as usize)
                .copied()
                .unwrap_or(palette.foreground),
        },
        Color::Indexed(index) => indexed(*index, palette),
    }
}

/// 256 色表：0–15 直接取命名色，16–231 是 6×6×6 立方，232–255 是灰阶。
fn indexed(index: u8, palette: &Palette) -> Rgb {
    match index {
        0..=15 => palette.ansi[index as usize],
        16..=231 => {
            let n = index - 16;
            let b = n % 6;
            let g = (n / 6) % 6;
            let r = n / 36;
            let ramp = |level: u8| if level == 0 { 0 } else { 55 + level * 40 };
            Rgb::new(ramp(r), ramp(g), ramp(b))
        }
        232..=255 => {
            let level = 8 + (index - 232) * 10;
            Rgb::new(level, level, level)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 取一行的文本，去掉行尾空白，便于直接比对。
    fn line_of(frame: &Frame, row: usize) -> String {
        frame.rows[row]
            .iter()
            .map(|cell| cell.c)
            .collect::<String>()
            .trim_end()
            .to_owned()
    }

    #[test]
    fn plain_text_and_newline_land_on_the_grid() {
        let mut term = Terminal::new(Size::new(10, 3));
        term.feed(b"hello\r\nworld");

        let frame = term.frame();
        assert!(line_of(&frame, 0).starts_with("hello"));
        assert!(line_of(&frame, 1).starts_with("world"));
    }

    #[test]
    fn carriage_return_rewrites_the_same_line() {
        let mut term = Terminal::new(Size::new(10, 2));
        term.feed(b"abc\rXY");

        let frame = term.frame();
        assert!(line_of(&frame, 0).starts_with("XYc"));
    }

    #[test]
    fn cursor_movement_and_erase_are_honoured() {
        let mut term = Terminal::new(Size::new(8, 2));
        term.feed(b"abcdefgh");
        term.feed(b"\x1b[1;3H");
        term.feed(b"\x1b[K");

        let frame = term.frame();
        assert_eq!(&line_of(&frame, 0)[..2], "ab");
        assert_eq!(line_of(&frame, 0), "ab");
        assert_eq!(
            frame.cursor,
            Some(Cursor {
                row: 0,
                col: 2,
                shape: CursorShape::Block
            })
        );
    }

    #[test]
    fn sgr_colors_are_resolved_to_rgb() {
        let mut term = Terminal::new(Size::new(6, 1));
        term.feed(b"\x1b[31;44mRED");

        let frame = term.frame();
        assert_eq!(frame.rows[0][0].fg, PALETTE.ansi[1], "前景应为红色");
        assert_eq!(frame.rows[0][0].bg, PALETTE.ansi[4], "背景应为蓝色");
    }

    #[test]
    fn inverse_video_swaps_foreground_and_background() {
        let mut term = Terminal::new(Size::new(4, 1));
        term.feed(b"\x1b[7mAB");

        let frame = term.frame();
        assert_eq!(frame.rows[0][0].fg, backdrop());
        assert_eq!(frame.rows[0][0].bg, PALETTE.foreground);
        assert!(frame.rows[0][0].attrs.inverse);
    }

    #[test]
    fn clearing_the_screen_empties_every_row() {
        let mut term = Terminal::new(Size::new(5, 3));
        term.feed(b"abc\r\ndef\r\nghi");
        term.feed(b"\x1b[2J\x1b[H");

        let frame = term.frame();
        assert!(frame
            .rows
            .iter()
            .all(|row| row.iter().all(|cell| cell.c == ' ')));
    }

    #[test]
    fn scrolling_off_the_bottom_keeps_history_and_moves_the_view() {
        let mut term = Terminal::new(Size::new(4, 2));
        term.feed(b"L0\r\nL1\r\nL2\r\nL3");

        let frame = term.frame();
        assert_eq!(line_of(&frame, 0), "L2");
        assert_eq!(line_of(&frame, 1), "L3");
        assert!(frame.history >= 2, "滚出屏幕的行应进入回滚缓冲");

        term.scroll(2);
        let scrolled = term.frame();
        assert!(scrolled.offset >= 2);
        assert_eq!(line_of(&scrolled, 0), "L0");
    }

    #[test]
    fn resizing_reflows_the_grid_to_the_new_geometry() {
        let mut term = Terminal::new(Size::new(20, 4));
        term.feed(b"keep me");
        term.resize(Size::new(8, 3));

        let frame = term.frame();
        assert_eq!(frame.rows.len(), 3);
        assert!(frame.rows.iter().all(|row| row.len() == 8));
        assert!(line_of(&frame, 0).starts_with("keep me"));
    }

    #[test]
    fn a_utf8_sequence_is_not_split_across_cells() {
        let mut term = Terminal::new(Size::new(10, 1));
        term.feed("会话".as_bytes());

        let frame = term.frame();
        assert_eq!(frame.rows[0][0].c, '会');
        assert!(frame.rows[0][0].attrs.wide, "CJK 应标记为宽字符");
        // 宽字符占两列：第二列是占位格，第二个字落在第三列。
        assert!(frame.rows[0][1].attrs.spacer, "第二列应是宽字符占位格");
        assert_eq!(frame.rows[0][2].c, '话');
        assert!(frame.rows[0][2].attrs.wide);
    }

    #[test]
    fn alternate_screen_switches_and_restores() {
        let mut term = Terminal::new(Size::new(6, 2));
        term.feed(b"base");
        term.feed(b"\x1b[?1049h");
        term.feed(b"\x1b[2J\x1b[Halt");
        assert!(line_of(&term.frame(), 0).starts_with("alt"));

        term.feed(b"\x1b[?1049l");
        assert!(line_of(&term.frame(), 0).starts_with("base"));
    }

    #[test]
    fn indexed_colors_follow_the_xterm_table() {
        assert_eq!(indexed(1, &PALETTE), PALETTE.ansi[1]);
        assert_eq!(
            indexed(196, &PALETTE),
            Rgb::new(255, 0, 0),
            "256 色立方里的纯红"
        );
        assert_eq!(indexed(255, &PALETTE), Rgb::new(238, 238, 238), "灰阶末端");
    }

    /// `resolve` 的兜底分支靠「`Black..=BrightWhite` 的判别值正好是 0–15」这条上游契约。
    /// 这里把 16 个标准色逐个钉住：上游哪天改了赋值，这条会先红，而不是等运行时画错颜色。
    #[test]
    fn standard_named_colors_map_onto_the_sixteen_slots_in_order() {
        let standard = [
            NamedColor::Black,
            NamedColor::Red,
            NamedColor::Green,
            NamedColor::Yellow,
            NamedColor::Blue,
            NamedColor::Magenta,
            NamedColor::Cyan,
            NamedColor::White,
            NamedColor::BrightBlack,
            NamedColor::BrightRed,
            NamedColor::BrightGreen,
            NamedColor::BrightYellow,
            NamedColor::BrightBlue,
            NamedColor::BrightMagenta,
            NamedColor::BrightCyan,
            NamedColor::BrightWhite,
        ];

        for (slot, named) in standard.into_iter().enumerate() {
            assert_eq!(
                resolve(&Color::Named(named), &PALETTE),
                PALETTE.ansi[slot],
                "{named:?} 应落在 ansi[{slot}]"
            );
        }
    }

    #[test]
    fn a_palette_recolors_the_frame_and_named_cells() {
        let palette = Palette::solarized_dark();
        let mut term = Terminal::with_palette(Size::new(4, 1), palette);
        term.feed(b"\x1b[31mX");

        let frame = term.frame();
        assert_eq!(frame.background, palette.background);
        assert_eq!(frame.cursor_color, palette.cursor);
        assert_eq!(frame.rows[0][0].fg, palette.ansi[1]);
        assert_eq!(frame.rows[0][1].bg, palette.background);
    }

    /// 两套新方案的色值来自对照表，逐个写死：改错一档就是无声地换了一套配色。
    #[test]
    fn console_and_xterm_use_the_documented_palettes() {
        let linux = Palette::linux_console();
        assert_eq!(linux.ansi[0], Rgb::new(0x00, 0x00, 0x00));
        assert_eq!(linux.ansi[1], Rgb::new(0xAA, 0x00, 0x00), "VGA 红是 AA 档");
        assert_eq!(linux.ansi[3], Rgb::new(0xAA, 0x55, 0x00), "VGA 黄是棕");
        assert_eq!(linux.ansi[7], Rgb::new(0xAA, 0xAA, 0xAA));
        assert_eq!(linux.ansi[9], Rgb::new(0xFF, 0x55, 0x55), "亮红加 FF 档");
        assert_eq!(linux.ansi[15], Rgb::new(0xFF, 0xFF, 0xFF));
        assert_eq!(linux.background, Rgb::new(0x00, 0x00, 0x00));
        assert_eq!(linux.foreground, linux.ansi[7], "普通文字就是 ANSI 7 灰");

        let xterm = Palette::xterm();
        assert_eq!(
            xterm.ansi[1],
            Rgb::new(0xCD, 0x00, 0x00),
            "xterm 红是 CD 档"
        );
        assert_eq!(
            xterm.ansi[4],
            Rgb::new(0x00, 0x00, 0xEE),
            "xterm 蓝是 EE 而不是 FF"
        );
        assert_eq!(xterm.ansi[7], Rgb::new(0xE5, 0xE5, 0xE5));
        assert_eq!(xterm.ansi[8], Rgb::new(0x7F, 0x7F, 0x7F));
        assert_eq!(xterm.ansi[12], Rgb::new(0x5C, 0x5C, 0xFF));
        assert_eq!(xterm.background, Rgb::new(0xFF, 0xFF, 0xFF), "白底");
        assert_eq!(xterm.foreground, Rgb::new(0x00, 0x00, 0x00), "黑字");
    }

    /// 任何方案都必须让前景与背景可分辨——白底白字这种方案不该出现在下拉里。
    #[test]
    fn every_palette_keeps_foreground_and_background_apart() {
        for palette in [
            Palette::default_palette(),
            Palette::solarized_dark(),
            Palette::solarized_light(),
            Palette::linux_console(),
            Palette::xterm(),
        ] {
            let distance = (i16::from(palette.foreground.r) - i16::from(palette.background.r))
                .abs()
                + (i16::from(palette.foreground.g) - i16::from(palette.background.g)).abs()
                + (i16::from(palette.foreground.b) - i16::from(palette.background.b)).abs();
            assert!(
                distance > 128,
                "前景/背景太接近（距离 {distance}）：{:?}",
                (palette.foreground, palette.background)
            );
        }
    }
}
