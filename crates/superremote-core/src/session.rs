//! 会话树模型：目录分组与连接条目，以及为列表渲染准备的扁平化行。

use std::collections::HashSet;

/// 目录与会话的主键。阶段二的持久化沿用字符串 Id。
pub type SessionId = String;

/// 连接协议类型。
///
/// 判别值对齐旧版 LiteDB 中 `RemoteLink.Type` 的 `RemoteType`
/// （`dir = 0, rdp = 1, ssh = 2, telnet = 3`），便于阶段二导入历史数据库。
/// 目录在 Rust 侧由 [`SessionNode::Directory`] 表达，所以不占用判别值。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum SessionKind {
    Rdp = 1,
    Ssh = 2,
    Telnet = 3,
}

impl SessionKind {
    pub fn label(self) -> &'static str {
        match self {
            SessionKind::Rdp => "RDP",
            SessionKind::Ssh => "SSH",
            SessionKind::Telnet => "TELNET",
        }
    }

    pub fn default_port(self) -> u16 {
        match self {
            SessionKind::Rdp => 3389,
            SessionKind::Ssh => 22,
            SessionKind::Telnet => 23,
        }
    }

    /// 旧版语义：同一条 RDP 只允许一个运行实例，SSH/Telnet 可以重复打开。
    pub fn allows_multiple_instances(self) -> bool {
        !matches!(self, SessionKind::Rdp)
    }
}

/// RDP 连接类型（对应 `.rdp` 的 `connection type:i:`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RdpPerformance {
    #[default]
    Auto,
    Modem,
    BroadbandLow,
    Satellite,
    BroadbandHigh,
    Wan,
    Lan,
}

impl RdpPerformance {
    pub const ALL: [Self; 7] = [
        Self::Auto,
        Self::Modem,
        Self::BroadbandLow,
        Self::Satellite,
        Self::BroadbandHigh,
        Self::Wan,
        Self::Lan,
    ];

    pub fn code(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Modem => "modem",
            Self::BroadbandLow => "broadband-low",
            Self::Satellite => "satellite",
            Self::BroadbandHigh => "broadband-high",
            Self::Wan => "wan",
            Self::Lan => "lan",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|value| value.code() == code)
    }

    /// `.rdp` 的 `connection type:i:` 取值：1–6 是固定档位，7 是自动。
    pub fn rdp_value(self) -> u8 {
        match self {
            Self::Modem => 1,
            Self::BroadbandLow => 2,
            Self::Satellite => 3,
            Self::BroadbandHigh => 4,
            Self::Wan => 5,
            Self::Lan => 6,
            Self::Auto => 7,
        }
    }
}

/// RDP 颜色深度。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RdpColorDepth {
    Bpp15,
    Bpp16,
    Bpp24,
    #[default]
    Bpp32,
}

impl RdpColorDepth {
    pub const ALL: [Self; 4] = [Self::Bpp15, Self::Bpp16, Self::Bpp24, Self::Bpp32];

    pub fn code(self) -> &'static str {
        match self {
            Self::Bpp15 => "bpp15",
            Self::Bpp16 => "bpp16",
            Self::Bpp24 => "bpp24",
            Self::Bpp32 => "bpp32",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|value| value.code() == code)
    }

    pub fn bits(self) -> u8 {
        match self {
            Self::Bpp15 => 15,
            Self::Bpp16 => 16,
            Self::Bpp24 => 24,
            Self::Bpp32 => 32,
        }
    }
}

/// RDP 音频重定向方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RdpAudio {
    #[default]
    Local,
    Remote,
    None,
}

impl RdpAudio {
    pub const ALL: [Self; 3] = [Self::Local, Self::Remote, Self::None];

    pub fn code(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Remote => "remote",
            Self::None => "none",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|value| value.code() == code)
    }

    /// `.rdp` 的 `audiomode:i:`：0 本地、1 远端、2 不播放。
    pub fn rdp_value(self) -> u8 {
        match self {
            Self::Local => 0,
            Self::Remote => 1,
            Self::None => 2,
        }
    }
}

/// RDP 组合键归属。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RdpKeyboard {
    Local,
    Remote,
    #[default]
    Fullscreen,
}

impl RdpKeyboard {
    pub const ALL: [Self; 3] = [Self::Local, Self::Remote, Self::Fullscreen];

    pub fn code(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Remote => "remote",
            Self::Fullscreen => "fullscreen",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|value| value.code() == code)
    }

    /// `.rdp` 的 `keyboardhook:i:`：0 本地、1 远端、2 仅全屏。
    pub fn rdp_value(self) -> u8 {
        match self {
            Self::Local => 0,
            Self::Remote => 1,
            Self::Fullscreen => 2,
        }
    }
}

/// RDP 远端桌面尺寸。
///
/// 三态而不是「预置档位的闭集枚举」：显示模式由显卡决定，写死几档会漏掉
/// 1366×768、1600×900、3440×1440 这类真实尺寸。旧版用 `EnumDisplaySettings`
/// 枚举是同一个道理，只是把可选值挪到了运行时。
///
/// `Custom` 的宽高在构造/解析时就夹到 mstsc 接受的范围里，所以这个类型的
/// 任何值都能直接写进 `.rdp`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RdpDesktopSize {
    /// 跟随标签内容区的物理像素（开启动态分辨率）。
    #[default]
    FollowWindow,
    /// 用主显示器当前分辨率，并在连接建立后铺满整个窗口。
    Fullscreen,
    /// 固定分辨率。宽高已夹到 [`MIN_DESKTOP_WIDTH`]–[`MAX_DESKTOP_WIDTH`] 区间。
    Custom { width: u32, height: u32 },
}

/// mstsc 接受的桌面宽高范围。写进 `.rdp` 之前必须夹一次，超出会让 mstsc 静默
/// 回退到 1024×768，用户改了设置却看不到任何变化。
pub const MIN_DESKTOP_WIDTH: u32 = 640;
pub const MIN_DESKTOP_HEIGHT: u32 = 480;
pub const MAX_DESKTOP_WIDTH: u32 = 7680;
pub const MAX_DESKTOP_HEIGHT: u32 = 4320;

/// 下拉里始终出现的预置档位。系统枚举不到（非 Windows、或显卡只报一种模式）时，
/// 至少还有这几档可选。
pub const DESKTOP_PRESETS: [(u32, u32); 8] = [
    (1280, 720),
    (1366, 768),
    (1440, 900),
    (1600, 900),
    (1680, 1050),
    (1920, 1080),
    (2560, 1440),
    (3840, 2160),
];

impl RdpDesktopSize {
    /// 构造固定分辨率，宽高自动夹到合法区间。
    pub fn custom(width: u32, height: u32) -> Self {
        Self::Custom {
            width: width.clamp(MIN_DESKTOP_WIDTH, MAX_DESKTOP_WIDTH),
            height: height.clamp(MIN_DESKTOP_HEIGHT, MAX_DESKTOP_HEIGHT),
        }
    }

    pub fn code(self) -> String {
        match self {
            Self::FollowWindow => "follow-window".to_owned(),
            Self::Fullscreen => "fullscreen".to_owned(),
            Self::Custom { width, height } => format!("{width}x{height}"),
        }
    }

    /// 解析落盘的取值。不认识的字符串返回 `None`，由调用方回退到内置默认——
    /// 这样以后新增档位，旧程序读到新值只是忽略这一项，不会整份配置解析失败。
    pub fn from_code(code: &str) -> Option<Self> {
        match code {
            "follow-window" => Some(Self::FollowWindow),
            "fullscreen" => Some(Self::Fullscreen),
            // 旧版本的四档预置码是 `1280x720` 这类字面量，格式与自定义尺寸一致，
            // 因此不需要为它们留专门的分支。
            other => {
                let (width, height) = other.split_once('x')?;
                let width: u32 = width.parse().ok()?;
                let height: u32 = height.parse().ok()?;
                if !(MIN_DESKTOP_WIDTH..=MAX_DESKTOP_WIDTH).contains(&width)
                    || !(MIN_DESKTOP_HEIGHT..=MAX_DESKTOP_HEIGHT).contains(&height)
                {
                    return None;
                }
                Some(Self::Custom { width, height })
            }
        }
    }

    /// 固定分辨率的像素尺寸。`FollowWindow` 用内容区、`Fullscreen` 用主屏，
    /// 两者都要等窗口就绪才知道，所以这里给不出数值。
    pub fn pixels(self) -> Option<(u32, u32)> {
        match self {
            Self::Custom { width, height } => Some((width, height)),
            Self::FollowWindow | Self::Fullscreen => None,
        }
    }

    /// 是否要在连接后自动铺满窗口。只有显式选了「全屏」才成立——
    /// 跟随窗口时全屏是用户临时点的运行时状态，不该被设置值顶掉。
    pub fn starts_fullscreen(self) -> bool {
        self == Self::Fullscreen
    }
}

/// 多路 RDP 会话是否复用同一个宿主进程。
///
/// 共用宿主省一套 CLR + WinForms + mstscax 基线；代价是一个会话崩溃会带走
/// 同宿主里的全部会话。给高风险目标可以单独改成独立宿主。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RdpHostMode {
    #[default]
    Shared,
    Isolated,
}

/// RDP 图形管线，决定宿主创建哪个版本的 ActiveX 控件。
///
/// 「version 10」是旧版 C# 使用的 RDP 8.1 控件；「version 11」支持 RDP 10
/// 和 AVC。该选项只表达能力与兼容性差异，不作为 UAC 或安全桌面卡顿的开关。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RdpGraphics {
    /// 旧版 C# 同款控件，协商 RDP 8.1
    #[default]
    Compat,
    /// 新版控件，支持 RDP 10 与 AVC
    Modern,
}

impl RdpGraphics {
    pub const ALL: [Self; 2] = [Self::Compat, Self::Modern];

    pub fn code(self) -> &'static str {
        match self {
            Self::Compat => "compat",
            Self::Modern => "modern",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        match code {
            "compat" => Some(Self::Compat),
            "modern" => Some(Self::Modern),
            _ => None,
        }
    }

    /// 传给宿主的 `--rdp-control`：注册名里的 version 号（比 .NET 类名大 1）。
    pub fn control(self) -> &'static str {
        match self {
            Self::Compat => "10",
            Self::Modern => "11",
        }
    }
}

impl RdpHostMode {
    pub const ALL: [Self; 2] = [Self::Shared, Self::Isolated];

    pub fn code(self) -> &'static str {
        match self {
            Self::Shared => "shared",
            Self::Isolated => "isolated",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|value| value.code() == code)
    }
}

/// 单条 RDP 连接的可覆盖设置。`None` 表示继承全局默认值。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RdpSettings {
    pub host_mode: Option<RdpHostMode>,
    pub performance: Option<RdpPerformance>,
    pub color_depth: Option<RdpColorDepth>,
    pub audio: Option<RdpAudio>,
    pub audio_capture: Option<bool>,
    pub keyboard: Option<RdpKeyboard>,
    pub redirect_printers: Option<bool>,
    pub redirect_clipboard: Option<bool>,
    pub redirect_smartcards: Option<bool>,
    pub redirect_ports: Option<bool>,
    pub redirect_drives: Option<bool>,
    pub desktop_size: Option<RdpDesktopSize>,
    pub graphics: Option<RdpGraphics>,
}

impl RdpSettings {
    pub fn is_empty(&self) -> bool {
        // 备注也算内容：只有备注的连接不该被当成「没有任何设置」。
        *self == Self::default()
    }

    /// 单条覆盖 → 全局默认 → 程序内置默认，逐项回退。
    pub fn resolve(&self, defaults: &Self) -> ResolvedRdpSettings {
        ResolvedRdpSettings {
            host_mode: self.host_mode.or(defaults.host_mode).unwrap_or_default(),
            performance: self
                .performance
                .or(defaults.performance)
                .unwrap_or_default(),
            color_depth: self
                .color_depth
                .or(defaults.color_depth)
                .unwrap_or_default(),
            audio: self.audio.or(defaults.audio).unwrap_or_default(),
            audio_capture: self
                .audio_capture
                .or(defaults.audio_capture)
                .unwrap_or(false),
            keyboard: self.keyboard.or(defaults.keyboard).unwrap_or_default(),
            redirect_printers: self
                .redirect_printers
                .or(defaults.redirect_printers)
                .unwrap_or(false),
            redirect_clipboard: self
                .redirect_clipboard
                .or(defaults.redirect_clipboard)
                .unwrap_or(true),
            redirect_smartcards: self
                .redirect_smartcards
                .or(defaults.redirect_smartcards)
                .unwrap_or(false),
            redirect_ports: self
                .redirect_ports
                .or(defaults.redirect_ports)
                .unwrap_or(false),
            redirect_drives: self
                .redirect_drives
                .or(defaults.redirect_drives)
                .unwrap_or(false),
            desktop_size: self
                .desktop_size
                .or(defaults.desktop_size)
                .unwrap_or_default(),
            graphics: self.graphics.or(defaults.graphics).unwrap_or_default(),
        }
    }
}

/// 合并后的 RDP 设置，连接时只使用这个快照。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedRdpSettings {
    pub host_mode: RdpHostMode,
    pub performance: RdpPerformance,
    pub color_depth: RdpColorDepth,
    pub audio: RdpAudio,
    pub audio_capture: bool,
    pub keyboard: RdpKeyboard,
    pub redirect_printers: bool,
    pub redirect_clipboard: bool,
    pub redirect_smartcards: bool,
    pub redirect_ports: bool,
    pub redirect_drives: bool,
    pub desktop_size: RdpDesktopSize,
    pub graphics: RdpGraphics,
}

impl Default for ResolvedRdpSettings {
    fn default() -> Self {
        Self {
            host_mode: RdpHostMode::Shared,
            performance: RdpPerformance::Auto,
            color_depth: RdpColorDepth::Bpp32,
            audio: RdpAudio::Local,
            audio_capture: false,
            keyboard: RdpKeyboard::Fullscreen,
            redirect_printers: false,
            redirect_clipboard: true,
            redirect_smartcards: false,
            redirect_ports: false,
            redirect_drives: false,
            desktop_size: RdpDesktopSize::FollowWindow,
            graphics: RdpGraphics::Compat,
        }
    }
}

impl ResolvedRdpSettings {
    /// 把当前生效值固化成一组覆盖，用于「设为默认」。
    pub fn into_settings(self) -> RdpSettings {
        RdpSettings {
            host_mode: Some(self.host_mode),
            performance: Some(self.performance),
            color_depth: Some(self.color_depth),
            audio: Some(self.audio),
            audio_capture: Some(self.audio_capture),
            keyboard: Some(self.keyboard),
            redirect_printers: Some(self.redirect_printers),
            redirect_clipboard: Some(self.redirect_clipboard),
            redirect_smartcards: Some(self.redirect_smartcards),
            redirect_ports: Some(self.redirect_ports),
            redirect_drives: Some(self.redirect_drives),
            desktop_size: Some(self.desktop_size),
            graphics: Some(self.graphics),
        }
    }
}

/// 终端光标形状（对应旧版 Putty 的 Cursor 选项）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TerminalCursor {
    #[default]
    Block,
    Underline,
    Beam,
}

impl TerminalCursor {
    pub const ALL: [Self; 3] = [Self::Block, Self::Underline, Self::Beam];

    pub fn code(self) -> &'static str {
        match self {
            Self::Block => "block",
            Self::Underline => "underline",
            Self::Beam => "beam",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|value| value.code() == code)
    }
}

/// 回退键发送的字节。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TerminalBackspace {
    /// `0x7f`（DEL），现代 Unix 终端的默认值。
    #[default]
    Delete,
    /// `0x08`（BS），老式交换机和少数 Unix 配置需要。
    Backspace,
}

impl TerminalBackspace {
    pub const ALL: [Self; 2] = [Self::Delete, Self::Backspace];

    pub fn code(self) -> &'static str {
        match self {
            Self::Delete => "delete",
            Self::Backspace => "backspace",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|value| value.code() == code)
    }
}

/// Home/End 的转义序列风格。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TerminalHomeEnd {
    /// `ESC [ H` / `ESC [ F`
    #[default]
    Standard,
    /// `ESC [ 1 ~` / `ESC [ 4 ~`，老式 rxvt 兼容。
    Rxvt,
}

impl TerminalHomeEnd {
    pub const ALL: [Self; 2] = [Self::Standard, Self::Rxvt];

    pub fn code(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Rxvt => "rxvt",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|value| value.code() == code)
    }
}

/// 终端配色方案。旧版是 Putty 色表；这里先提供几套内置方案。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TerminalColorScheme {
    #[default]
    Default,
    SolarizedDark,
    SolarizedLight,
    /// Linux 控制台：EGA/VGA 硬件调色板，黑底灰字。
    LinuxConsole,
    /// xterm 经典色板：白底黑字，ANSI 色用 X11 那套偏暗的原色。
    Xterm,
}

impl TerminalColorScheme {
    pub const ALL: [Self; 5] = [
        Self::Default,
        Self::SolarizedDark,
        Self::SolarizedLight,
        Self::LinuxConsole,
        Self::Xterm,
    ];

    pub fn code(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::SolarizedDark => "solarized-dark",
            Self::SolarizedLight => "solarized-light",
            Self::LinuxConsole => "linux",
            Self::Xterm => "xterm",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|value| value.code() == code)
    }
}

/// 远端字节的字符集。只影响**收到的数据**怎么解码，发送永远是 UTF-8。
///
/// 只列常用几种：旧版用 `Encoding.GetEncodings()` 拉出上百条，运维真正会遇到的
/// 就这几类（UTF-8 主机、GBK 的国产系统、Big5 的台港主机、Latin-1 的老 Unix）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TerminalCharset {
    #[default]
    Utf8,
    /// 简体中文（GBK / GB2312 / GB18030 的超集，Windows 代码页 936）。
    Gbk,
    /// 繁体中文。
    Big5,
    /// 西欧单字节。不会解码失败，所以老设备上的乱码能被它兜住。
    Latin1,
}

impl TerminalCharset {
    pub const ALL: [Self; 4] = [Self::Utf8, Self::Gbk, Self::Big5, Self::Latin1];

    pub fn code(self) -> &'static str {
        match self {
            Self::Utf8 => "utf-8",
            Self::Gbk => "gbk",
            Self::Big5 => "big5",
            Self::Latin1 => "latin1",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|value| value.code() == code)
    }

    /// `encoding_rs` 的标签名。解码在 `superremote-term`，这里只负责给出名字。
    pub fn encoding_label(self) -> &'static str {
        match self {
            Self::Utf8 => "utf-8",
            Self::Gbk => "gbk",
            Self::Big5 => "big5",
            Self::Latin1 => "windows-1252",
        }
    }

    /// UTF-8 不需要额外解码，走原路（`alacritty_terminal` 自己解 UTF-8）。
    pub fn needs_transcode(self) -> bool {
        self != Self::Utf8
    }
}

/// 终端里的鼠标动作（对应旧版 Putty 的 Mouse 选项）。
///
/// 「扩展选区」= 按一下把已有选区延伸到点击处；「粘贴」= 把系统剪贴板发到远端。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TerminalMouseMode {
    /// 混合模式：中键扩展选区，右键粘贴。
    #[default]
    Hybrid,
    /// xterm：右键扩展选区，中键粘贴。
    Xterm,
    /// Windows：中键扩展选区，右键弹出菜单。
    ///
    /// 菜单还没做，右键暂时退化为「粘贴」——与混合模式一致的那个动作，
    /// 免得选了这项之后右键变成没反应。
    Windows,
}

impl TerminalMouseMode {
    pub const ALL: [Self; 3] = [Self::Hybrid, Self::Xterm, Self::Windows];

    pub fn code(self) -> &'static str {
        match self {
            Self::Hybrid => "hybrid",
            Self::Xterm => "xterm",
            Self::Windows => "windows",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|value| value.code() == code)
    }

    /// 这个模式下右键是不是「粘贴」。
    pub fn right_click_pastes(self) -> bool {
        !matches!(self, Self::Xterm)
    }

    /// 这个模式下中键是不是「粘贴」。
    pub fn middle_click_pastes(self) -> bool {
        matches!(self, Self::Xterm)
    }
}

/// 功能键的转义序列风格（旧版 Putty 的「Fn 与小键盘」）。
///
/// 只管 F1–F4：小键盘的「应用模式」要能区分主键盘与小键盘的数字键，
/// Iced 的按键事件不携带这个来源信息，硬猜会把主键盘数字也带跑。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TerminalKeypad {
    /// `\eOP` … `\eOS`（VT100 风格）。
    #[default]
    Standard,
    /// `\e[11~` … `\e[14~`。
    EscN,
    /// Linux 控制台：`\e[[A` … `\e[[D`。
    Linux,
}

impl TerminalKeypad {
    pub const ALL: [Self; 3] = [Self::Standard, Self::EscN, Self::Linux];

    pub fn code(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::EscN => "escn",
            Self::Linux => "linux",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|value| value.code() == code)
    }

    /// F1–F4 的序列。F5 以上三种风格一致，不在这里。
    pub fn function_key(self, index: u8) -> &'static [u8] {
        match (self, index) {
            (Self::Standard, 1) => b"\x1bOP",
            (Self::Standard, 2) => b"\x1bOQ",
            (Self::Standard, 3) => b"\x1bOR",
            (Self::Standard, 4) => b"\x1bOS",
            (Self::EscN, 1) => b"\x1b[11~",
            (Self::EscN, 2) => b"\x1b[12~",
            (Self::EscN, 3) => b"\x1b[13~",
            (Self::EscN, 4) => b"\x1b[14~",
            (Self::Linux, 1) => b"\x1b[[A",
            (Self::Linux, 2) => b"\x1b[[B",
            (Self::Linux, 3) => b"\x1b[[C",
            (Self::Linux, 4) => b"\x1b[[D",
            _ => b"",
        }
    }
}

/// SSH/Telnet 终端的可覆盖设置。`None` 表示继承全局默认值。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TerminalSettings {
    pub font_family: Option<String>,
    pub font_size: Option<u8>,
    pub cursor: Option<TerminalCursor>,
    pub backspace: Option<TerminalBackspace>,
    pub home_end: Option<TerminalHomeEnd>,
    pub color_scheme: Option<TerminalColorScheme>,
    pub charset: Option<TerminalCharset>,
    pub mouse_mode: Option<TerminalMouseMode>,
    pub keypad: Option<TerminalKeypad>,
}

impl TerminalSettings {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    pub fn resolve(&self, defaults: &Self) -> ResolvedTerminalSettings {
        ResolvedTerminalSettings {
            font_family: self
                .font_family
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .map(str::to_owned)
                .or_else(|| {
                    defaults
                        .font_family
                        .as_deref()
                        .filter(|value| !value.trim().is_empty())
                        .map(str::to_owned)
                })
                .unwrap_or_else(|| "Consolas".to_owned()),
            font_size: self
                .font_size
                .or(defaults.font_size)
                .unwrap_or(13)
                .clamp(8, 32),
            cursor: self.cursor.or(defaults.cursor).unwrap_or_default(),
            backspace: self.backspace.or(defaults.backspace).unwrap_or_default(),
            home_end: self.home_end.or(defaults.home_end).unwrap_or_default(),
            color_scheme: self
                .color_scheme
                .or(defaults.color_scheme)
                .unwrap_or_default(),
            charset: self.charset.or(defaults.charset).unwrap_or_default(),
            mouse_mode: self.mouse_mode.or(defaults.mouse_mode).unwrap_or_default(),
            keypad: self.keypad.or(defaults.keypad).unwrap_or_default(),
        }
    }
}

/// 合并后的终端设置，连接时只使用这个快照。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTerminalSettings {
    pub font_family: String,
    pub font_size: u8,
    pub cursor: TerminalCursor,
    pub backspace: TerminalBackspace,
    pub home_end: TerminalHomeEnd,
    pub color_scheme: TerminalColorScheme,
    pub charset: TerminalCharset,
    pub mouse_mode: TerminalMouseMode,
    pub keypad: TerminalKeypad,
}

impl Default for ResolvedTerminalSettings {
    fn default() -> Self {
        Self {
            font_family: "Consolas".to_owned(),
            font_size: 13,
            cursor: TerminalCursor::Block,
            backspace: TerminalBackspace::Delete,
            home_end: TerminalHomeEnd::Standard,
            color_scheme: TerminalColorScheme::Default,
            charset: TerminalCharset::Utf8,
            mouse_mode: TerminalMouseMode::Hybrid,
            keypad: TerminalKeypad::Standard,
        }
    }
}

impl ResolvedTerminalSettings {
    pub fn into_settings(self) -> TerminalSettings {
        TerminalSettings {
            font_family: Some(self.font_family),
            font_size: Some(self.font_size),
            cursor: Some(self.cursor),
            backspace: Some(self.backspace),
            home_end: Some(self.home_end),
            color_scheme: Some(self.color_scheme),
            charset: Some(self.charset),
            mouse_mode: Some(self.mouse_mode),
            keypad: Some(self.keypad),
        }
    }
}

/// 一条连接的非敏感设置。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionSettings {
    pub rdp: RdpSettings,
    pub terminal: TerminalSettings,
    /// 首选打开方式。三种协议共用，所以放在协议无关的这一层。
    pub open_mode: Option<OpenMode>,
    /// 备注（旧版的「描述」）。纯文本、非敏感，跟着连接一起落盘。
    pub note: String,
}

impl SessionSettings {
    pub fn is_empty(&self) -> bool {
        self.rdp.is_empty()
            && self.terminal.is_empty()
            && self.open_mode.is_none()
            && self.note.is_empty()
    }
}

/// 一条连接被「打开」时落在哪里。
///
/// 旧版是 `DbItemSetting.OpenMode`（0 = 用全局默认、1 = 选项卡、2 = 后台选项卡、
/// 3 = 外部窗口）；这里把「用全局默认」表达成 `Option::None`，与其余设置一致。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OpenMode {
    /// 新建选项卡并切过去。
    #[default]
    Tab,
    /// 新建选项卡但留在当前页，之后用标签条切过去。
    Background,
    /// 单独开一个 OS 窗口。
    Detached,
}

impl OpenMode {
    pub const ALL: [Self; 3] = [Self::Tab, Self::Background, Self::Detached];

    pub fn code(self) -> &'static str {
        match self {
            Self::Tab => "tab",
            Self::Background => "background",
            Self::Detached => "window",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|value| value.code() == code)
    }
}

/// 一条连接。
///
/// 密码与私钥不在这里——它们属于阶段二的加密存储，模型只保留非敏感的登录账号。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub id: SessionId,
    pub name: String,
    pub kind: SessionKind,
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub settings: SessionSettings,
    /// 上次用独立窗口打开这条连接时的窗口尺寸（逻辑像素）。
    ///
    /// 这是使用习惯而不是设置项，所以放在连接本体上而不是 `SessionSettings` 里；
    /// 关窗时写回，下次「在新窗口打开」直接复用。
    pub window: Option<WindowSize>,
}

/// 独立窗口的尺寸。存整数：会话树要整体参与相等比较，`f32` 不是 `Eq`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowSize {
    pub width: u32,
    pub height: u32,
}

impl Session {
    pub fn address(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }

    /// 搜索命中：连接名或主机名包含关键字（大小写不敏感）。
    pub fn matches(&self, needle: &str) -> bool {
        self.name.to_lowercase().contains(needle) || self.host.to_lowercase().contains(needle)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Directory {
    pub id: SessionId,
    pub name: String,
    pub children: Vec<SessionNode>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionNode {
    Directory(Directory),
    Session(Session),
}

impl SessionNode {
    pub fn id(&self) -> &SessionId {
        match self {
            Self::Directory(dir) => &dir.id,
            Self::Session(session) => &session.id,
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Self::Directory(dir) => &dir.name,
            Self::Session(session) => &session.name,
        }
    }

    pub fn set_name(&mut self, name: &str) {
        match self {
            Self::Directory(dir) => dir.name = name.to_owned(),
            Self::Session(session) => session.name = name.to_owned(),
        }
    }
}

/// 会话树渲染用的一行，由 [`SessionTree::rows`] 按当前展开状态或搜索条件产出。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeRow {
    Directory {
        id: SessionId,
        name: String,
        depth: usize,
        expanded: bool,
    },
    Session {
        id: SessionId,
        name: String,
        kind: SessionKind,
        address: String,
        depth: usize,
    },
}

/// 节点移动的目的地。拖拽落到不同的行上，语义不一样。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MoveTarget {
    /// 落在目录上：成为它的最后一个子节点。
    IntoDirectory(SessionId),
    /// 落在连接上：与它同级，插到它后面。
    NextTo(SessionId),
    /// 落在空白处：回到根层末尾。
    ToRoot,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SessionTree {
    pub roots: Vec<SessionNode>,
    /// 展开的目录。旧版把该状态存放在 `UserSettings.xml` 的 `ExpandItem` 中，
    /// 阶段二同样从配置文件恢复。
    pub expanded: HashSet<SessionId>,
}

impl SessionTree {
    pub fn new(roots: Vec<SessionNode>) -> Self {
        Self {
            roots,
            expanded: HashSet::new(),
        }
    }

    /// 切换目录展开状态，返回切换后的值。
    pub fn toggle(&mut self, id: &str) -> bool {
        if self.expanded.remove(id) {
            false
        } else {
            self.expanded.insert(id.to_owned());
            true
        }
    }

    pub fn expand_all(&mut self) {
        fn walk(nodes: &[SessionNode], out: &mut HashSet<SessionId>) {
            for node in nodes {
                if let SessionNode::Directory(dir) = node {
                    out.insert(dir.id.clone());
                    walk(&dir.children, out);
                }
            }
        }

        walk(&self.roots, &mut self.expanded);
    }

    pub fn find_session(&self, id: &str) -> Option<&Session> {
        fn walk<'a>(nodes: &'a [SessionNode], id: &str) -> Option<&'a Session> {
            for node in nodes {
                match node {
                    SessionNode::Directory(dir) => {
                        if let Some(found) = walk(&dir.children, id) {
                            return Some(found);
                        }
                    }
                    SessionNode::Session(session) if session.id == id => return Some(session),
                    SessionNode::Session(_) => {}
                }
            }
            None
        }

        walk(&self.roots, id)
    }

    pub fn session_count(&self) -> usize {
        fn walk(nodes: &[SessionNode]) -> usize {
            nodes
                .iter()
                .map(|node| match node {
                    SessionNode::Directory(dir) => walk(&dir.children),
                    SessionNode::Session(_) => 1,
                })
                .sum()
        }

        walk(&self.roots)
    }

    /// 在 `parent`（`None` 为根层）下追加节点。父目录不存在时不改动树，返回 `false`。
    pub fn push_node(&mut self, parent: Option<&str>, node: SessionNode) -> bool {
        match parent {
            None => {
                self.roots.push(node);
                true
            }
            Some(parent_id) => match self.find_directory_mut(parent_id) {
                Some(dir) => {
                    dir.children.push(node);
                    self.expanded.insert(parent_id.to_owned());
                    true
                }
                None => false,
            },
        }
    }

    /// 摘掉节点；删除目录会连带它的整个子树。
    pub fn remove_node(&mut self, id: &str) -> Option<SessionNode> {
        fn walk(nodes: &mut Vec<SessionNode>, id: &str) -> Option<SessionNode> {
            if let Some(index) = nodes.iter().position(|node| node.id() == id) {
                return Some(nodes.remove(index));
            }
            for node in nodes.iter_mut() {
                if let SessionNode::Directory(dir) = node {
                    if let Some(removed) = walk(&mut dir.children, id) {
                        return Some(removed);
                    }
                }
            }
            None
        }

        let removed = walk(&mut self.roots, id);
        if let Some(node) = &removed {
            // 展开状态是按 id 存的，整棵子树的目录都要清掉，否则会在配置文件里越积越多。
            let mut stale = vec![node.id().clone()];
            collect_directory_ids(node, &mut stale);
            for key in stale {
                self.expanded.remove(&key);
            }
        }
        removed
    }

    pub fn rename(&mut self, id: &str, name: &str) -> bool {
        match self.find_node_mut(id) {
            Some(node) => {
                node.set_name(name);
                true
            }
            None => false,
        }
    }

    /// 把节点挪到新的位置。落点不同语义不同，见 [`MoveTarget`]。
    ///
    /// 三条硬约束：
    /// 1. 不能挪到自己下面（树会变成环），也不能挪到自己旁边；
    /// 2. 目的地必须还在树上，挪动前先验，摘下来就找不到了；
    /// 3. 展开状态跟着子树一起走——挪动不该把已经展开的目录折起来。
    ///
    /// 失败时节点**留在原地**（或退回根层），绝不会凭空消失。
    pub fn move_node(&mut self, id: &str, target: MoveTarget) -> bool {
        if matches!(&target, MoveTarget::NextTo(sibling) if sibling == id) {
            return false;
        }

        let target_exists = match &target {
            MoveTarget::ToRoot => true,
            MoveTarget::IntoDirectory(dir) => self.is_directory(dir),
            MoveTarget::NextTo(sibling) => self.find_node(sibling).is_some(),
        };
        if !target_exists {
            return false;
        }

        // 目的地落在被移动的子树里 = 把节点挂到自己下面。必须在摘之前判：
        // 摘下来之后子树就脱离了，再查会查不到，节点会被塞进自己的旧位置里。
        let inside_source = match &target {
            MoveTarget::ToRoot => false,
            MoveTarget::IntoDirectory(dir) | MoveTarget::NextTo(dir) => {
                self.is_inside_subtree(dir, id)
            }
        };
        if inside_source {
            return false;
        }

        let Some(node) = self.detach_node(id) else {
            return false;
        };

        match target {
            MoveTarget::ToRoot => {
                self.roots.push(node);
                true
            }
            MoveTarget::IntoDirectory(dir) => self.push_child(&dir, node),
            MoveTarget::NextTo(sibling) => match self.insert_after(&sibling, node) {
                Ok(()) => true,
                // 兄弟节点意外消失了：退回根层，节点不能丢。
                Err(node) => {
                    self.roots.push(*node);
                    false
                }
            },
        }
    }

    /// 把节点追加到某个目录末尾；目录不在了就退回根层并报告失败。
    fn push_child(&mut self, dir_id: &str, node: SessionNode) -> bool {
        match self.find_directory_mut(dir_id) {
            Some(dir) => {
                dir.children.push(node);
                self.expanded.insert(dir_id.to_owned());
                true
            }
            None => {
                self.roots.push(node);
                false
            }
        }
    }

    /// 插到某条连接/目录后面（同级）。返回 `Err` 时把节点原样交还给调用方。
    ///
    /// 节点装箱是为了别让 `Result` 的错误变体撑到 150+ 字节（`clippy::result_large_err`）。
    fn insert_after(
        &mut self,
        sibling_id: &str,
        node: SessionNode,
    ) -> Result<(), Box<SessionNode>> {
        match self.parent_id_of(sibling_id) {
            None => match self.roots.iter().position(|item| item.id() == sibling_id) {
                Some(index) => {
                    self.roots.insert(index + 1, node);
                    Ok(())
                }
                None => Err(Box::new(node)),
            },
            Some(parent_id) => match self.find_directory_mut(&parent_id) {
                Some(dir) => match dir.children.iter().position(|item| item.id() == sibling_id) {
                    Some(index) => {
                        dir.children.insert(index + 1, node);
                        Ok(())
                    }
                    None => Err(Box::new(node)),
                },
                None => Err(Box::new(node)),
            },
        }
    }

    /// 某个节点的父目录 id；在根层返回 `None`。
    pub fn parent_id_of(&self, id: &str) -> Option<SessionId> {
        fn walk<'a>(
            nodes: &'a [SessionNode],
            id: &str,
            parent: Option<&'a SessionId>,
        ) -> Option<&'a SessionId> {
            for node in nodes {
                if node.id() == id {
                    return parent;
                }
                if let SessionNode::Directory(dir) = node {
                    if let Some(found) = walk(&dir.children, id, Some(&dir.id)) {
                        return Some(found);
                    }
                }
            }
            None
        }

        walk(&self.roots, id, None).cloned()
    }

    /// `candidate` 是否就是 `root` 本身或它子树里的节点。
    fn is_inside_subtree(&self, candidate: &str, root: &str) -> bool {
        fn walk(node: &SessionNode, candidate: &str) -> bool {
            if node.id() == candidate {
                return true;
            }
            match node {
                SessionNode::Directory(dir) => {
                    dir.children.iter().any(|child| walk(child, candidate))
                }
                SessionNode::Session(_) => false,
            }
        }

        self.find_node(root)
            .is_some_and(|node| walk(node, candidate))
    }

    /// 把节点从树里摘下来，**不动**它的展开状态。
    ///
    /// [`remove_node`] 会顺手清掉子树的展开记录（删除要清，否则配置文件里越积越多），
    /// 移动则相反：子树展开到第几层要原样带走。
    fn detach_node(&mut self, id: &str) -> Option<SessionNode> {
        fn walk(nodes: &mut Vec<SessionNode>, id: &str) -> Option<SessionNode> {
            if let Some(index) = nodes.iter().position(|node| node.id() == id) {
                return Some(nodes.remove(index));
            }
            for node in nodes.iter_mut() {
                if let SessionNode::Directory(dir) = node {
                    if let Some(removed) = walk(&mut dir.children, id) {
                        return Some(removed);
                    }
                }
            }
            None
        }

        walk(&mut self.roots, id)
    }

    /// 就地修改一条连接；找不到该 id 时返回 `false`。
    pub fn with_session_mut(&mut self, id: &str, edit: impl FnOnce(&mut Session)) -> bool {
        fn walk<'a>(nodes: &'a mut [SessionNode], id: &str) -> Option<&'a mut Session> {
            for node in nodes.iter_mut() {
                match node {
                    SessionNode::Directory(dir) => {
                        if let Some(found) = walk(&mut dir.children, id) {
                            return Some(found);
                        }
                    }
                    SessionNode::Session(session) if session.id == id => return Some(session),
                    SessionNode::Session(_) => {}
                }
            }
            None
        }

        match walk(&mut self.roots, id) {
            Some(session) => {
                edit(session);
                true
            }
            None => false,
        }
    }

    pub fn node_name(&self, id: &str) -> Option<&str> {
        self.find_node(id).map(SessionNode::name)
    }

    pub fn is_directory(&self, id: &str) -> bool {
        matches!(self.find_node(id), Some(SessionNode::Directory(_)))
    }

    /// 该节点及其子树里的全部连接 id，用于删除时连带清理凭据与标签。
    pub fn subtree_session_ids(node: &SessionNode) -> Vec<SessionId> {
        let mut out = Vec::new();
        fn walk(node: &SessionNode, out: &mut Vec<SessionId>) {
            match node {
                SessionNode::Directory(dir) => {
                    for child in &dir.children {
                        walk(child, out);
                    }
                }
                SessionNode::Session(session) => out.push(session.id.clone()),
            }
        }
        walk(node, &mut out);
        out
    }

    /// 新节点应该挂到哪个目录下：选中的是目录就用它，否则用它的父层（根）。
    pub fn parent_for_new(&self, selected: Option<&str>) -> Option<SessionId> {
        match selected {
            Some(id) if self.is_directory(id) => Some(id.to_owned()),
            _ => None,
        }
    }

    /// 按 id 找任意节点（目录或连接）。右键菜单复制时需要拿到整棵子树。
    pub fn find_node(&self, id: &str) -> Option<&SessionNode> {
        fn walk<'a>(nodes: &'a [SessionNode], id: &str) -> Option<&'a SessionNode> {
            for node in nodes {
                if node.id() == id {
                    return Some(node);
                }
                if let SessionNode::Directory(dir) = node {
                    if let Some(found) = walk(&dir.children, id) {
                        return Some(found);
                    }
                }
            }
            None
        }

        walk(&self.roots, id)
    }

    fn find_node_mut(&mut self, id: &str) -> Option<&mut SessionNode> {
        fn walk<'a>(nodes: &'a mut [SessionNode], id: &str) -> Option<&'a mut SessionNode> {
            for node in nodes.iter_mut() {
                if node.id() == id {
                    return Some(node);
                }
                if let SessionNode::Directory(dir) = node {
                    if let Some(found) = walk(&mut dir.children, id) {
                        return Some(found);
                    }
                }
            }
            None
        }

        walk(&mut self.roots, id)
    }

    fn find_directory_mut(&mut self, id: &str) -> Option<&mut Directory> {
        fn walk<'a>(nodes: &'a mut [SessionNode], id: &str) -> Option<&'a mut Directory> {
            for node in nodes.iter_mut() {
                if let SessionNode::Directory(dir) = node {
                    if dir.id == id {
                        return Some(dir);
                    }
                    if let Some(found) = walk(&mut dir.children, id) {
                        return Some(found);
                    }
                }
            }
            None
        }

        walk(&mut self.roots, id)
    }

    /// 搜索模式（`query` 非空）下忽略展开状态，命中项连同父级目录一起平铺显示。
    pub fn rows(&self, query: &str) -> Vec<TreeRow> {
        let needle = query.trim().to_lowercase();
        let mut out = Vec::new();

        if needle.is_empty() {
            for node in &self.roots {
                collect_visible(node, 0, &self.expanded, &mut out);
            }
        } else {
            for node in &self.roots {
                collect_matched(node, 0, &needle, &mut out);
            }
        }

        out
    }
}

/// 收集节点子树里所有目录 id（不含节点自身）。
fn collect_directory_ids(node: &SessionNode, out: &mut Vec<SessionId>) {
    if let SessionNode::Directory(dir) = node {
        for child in &dir.children {
            if let SessionNode::Directory(sub) = child {
                out.push(sub.id.clone());
                collect_directory_ids(child, out);
            }
        }
    }
}

fn session_row(session: &Session, depth: usize) -> TreeRow {
    TreeRow::Session {
        id: session.id.clone(),
        name: session.name.clone(),
        kind: session.kind,
        address: session.address(),
        depth,
    }
}

fn directory_row(dir: &Directory, depth: usize, expanded: bool) -> TreeRow {
    TreeRow::Directory {
        id: dir.id.clone(),
        name: dir.name.clone(),
        depth,
        expanded,
    }
}

fn collect_visible(
    node: &SessionNode,
    depth: usize,
    expanded: &HashSet<SessionId>,
    out: &mut Vec<TreeRow>,
) {
    match node {
        SessionNode::Directory(dir) => {
            let is_open = expanded.contains(&dir.id);
            out.push(directory_row(dir, depth, is_open));

            if is_open {
                for child in &dir.children {
                    collect_visible(child, depth + 1, expanded, out);
                }
            }
        }
        SessionNode::Session(session) => out.push(session_row(session, depth)),
    }
}

/// 目录名整体命中时，用它自己的子树（全部展开）填充 `out`。
fn collect_all(node: &SessionNode, depth: usize, out: &mut Vec<TreeRow>) {
    match node {
        SessionNode::Directory(dir) => {
            out.push(directory_row(dir, depth, true));
            for child in &dir.children {
                collect_all(child, depth + 1, out);
            }
        }
        SessionNode::Session(session) => out.push(session_row(session, depth)),
    }
}

/// 返回该子树里是否存在命中，命中行按 `depth` 缩进写入 `out`。
fn collect_matched(node: &SessionNode, depth: usize, needle: &str, out: &mut Vec<TreeRow>) -> bool {
    match node {
        SessionNode::Directory(dir) => {
            if dir.name.to_lowercase().contains(needle) {
                collect_all(node, depth, out);
                return true;
            }

            let mut children = Vec::new();
            for child in &dir.children {
                collect_matched(child, depth + 1, needle, &mut children);
            }

            if children.is_empty() {
                return false;
            }

            out.push(directory_row(dir, depth, true));
            out.append(&mut children);
            true
        }
        SessionNode::Session(session) => {
            if session.matches(needle) {
                out.push(session_row(session, depth));
                true
            } else {
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(id: &str, name: &str, kind: SessionKind, host: &str) -> SessionNode {
        SessionNode::Session(Session {
            id: id.into(),
            name: name.into(),
            kind,
            host: host.into(),
            port: kind.default_port(),
            username: None,
            settings: Default::default(),
            window: None,
        })
    }

    fn directory(id: &str, name: &str, children: Vec<SessionNode>) -> SessionNode {
        SessionNode::Directory(Directory {
            id: id.into(),
            name: name.into(),
            children,
        })
    }

    fn tree() -> SessionTree {
        SessionTree::new(vec![
            directory(
                "prod",
                "生产环境",
                vec![
                    session("web-01", "Web-01", SessionKind::Ssh, "10.0.0.11"),
                    directory(
                        "prod-data",
                        "数据平台",
                        vec![session("db-01", "DB-Master", SessionKind::Ssh, "10.0.0.21")],
                    ),
                    session("jump", "Jump-Server", SessionKind::Rdp, "10.0.0.5"),
                ],
            ),
            directory(
                "qa",
                "测试环境",
                vec![session(
                    "legacy",
                    "Legacy-App",
                    SessionKind::Telnet,
                    "10.1.0.9",
                )],
            ),
        ])
    }

    fn dir_names(rows: &[TreeRow]) -> Vec<&str> {
        rows.iter()
            .map(|row| match row {
                TreeRow::Directory { name, .. } => name.as_str(),
                TreeRow::Session { name, .. } => name.as_str(),
            })
            .collect()
    }

    #[test]
    fn collapsed_tree_only_shows_roots() {
        let rows = tree().rows("");
        assert_eq!(dir_names(&rows), ["生产环境", "测试环境"]);
        assert!(rows.iter().all(|row| matches!(
            row,
            TreeRow::Directory {
                depth: 0,
                expanded: false,
                ..
            }
        )));
    }

    #[test]
    fn toggle_reveals_children_and_reports_new_state() {
        let mut tree = tree();
        assert!(tree.toggle("prod"));
        assert!(!tree.toggle("prod"));

        tree.toggle("prod");
        let rows = tree.rows("");
        assert_eq!(
            dir_names(&rows),
            ["生产环境", "Web-01", "数据平台", "Jump-Server", "测试环境"]
        );

        let nested = rows
            .iter()
            .find(|row| matches!(row, TreeRow::Session { name, .. } if name == "Web-01"))
            .unwrap();
        assert!(matches!(nested, TreeRow::Session { depth: 1, .. }));
    }

    #[test]
    fn expand_all_reveals_nested_sessions() {
        let mut tree = tree();
        tree.expand_all();
        let rows = tree.rows("");
        assert!(rows.iter().any(|row| matches!(
            row,
            TreeRow::Session { id, depth: 2, .. } if id == "db-01"
        )));
    }

    #[test]
    fn search_flattens_matching_paths_and_forces_ancestors_open() {
        let rows = tree().rows("db");
        assert_eq!(dir_names(&rows), ["生产环境", "数据平台", "DB-Master"]);
        assert!(rows.iter().all(|row| match row {
            TreeRow::Directory { expanded, .. } => *expanded,
            TreeRow::Session { .. } => true,
        }));
    }

    #[test]
    fn search_on_directory_name_yields_whole_branch() {
        let rows = tree().rows("数据");
        assert_eq!(dir_names(&rows), ["生产环境", "数据平台", "DB-Master"]);
    }

    #[test]
    fn search_matches_host_case_insensitively_and_misses_are_empty() {
        let rows = tree().rows("10.1.0.9");
        assert_eq!(dir_names(&rows), ["测试环境", "Legacy-App"]);
        assert!(tree().rows("不存在的名字").is_empty());
    }

    #[test]
    fn counts_and_lookup() {
        let tree = tree();
        assert_eq!(tree.session_count(), 4);
        assert_eq!(
            tree.find_session("db-01").map(|s| s.address()),
            Some("10.0.0.21:22".to_owned())
        );
        assert_eq!(tree.find_session("prod"), None);
    }

    #[test]
    fn rdp_is_single_instance_and_kind_numbering_matches_legacy_db() {
        assert!(!SessionKind::Rdp.allows_multiple_instances());
        assert!(SessionKind::Ssh.allows_multiple_instances());
        assert!(SessionKind::Telnet.allows_multiple_instances());
        assert_eq!(SessionKind::Rdp as u8, 1);
        assert_eq!(SessionKind::Ssh as u8, 2);
        assert_eq!(SessionKind::Telnet as u8, 3);
    }

    #[test]
    fn push_node_under_a_directory_expands_it_and_rejects_unknown_parents() {
        let mut tree = tree();
        assert!(tree.push_node(
            Some("prod"),
            session("extra", "Extra", SessionKind::Ssh, "10.0.0.9")
        ));
        assert!(tree.expanded.contains("prod"), "新建后父目录应自动展开");

        let rows = tree.rows("");
        assert!(rows
            .iter()
            .any(|row| matches!(row, TreeRow::Session { id, .. } if id == "extra")));

        assert!(!tree.push_node(
            Some("nope"),
            session("ghost", "Ghost", SessionKind::Ssh, "1.1.1.1")
        ));
        assert_eq!(tree.session_count(), 5);
    }

    #[test]
    fn push_node_at_root_when_parent_is_none() {
        let mut tree = tree();
        let before = tree.roots.len();
        assert!(tree.push_node(None, directory("new", "新目录", vec![])));
        assert_eq!(tree.roots.len(), before + 1);
    }

    #[test]
    fn removing_a_directory_takes_its_whole_subtree() {
        let mut tree = tree();
        tree.expand_all();
        let removed = tree.remove_node("prod").expect("应能删掉生产环境");

        assert_eq!(
            SessionTree::subtree_session_ids(&removed),
            ["web-01", "db-01", "jump"]
        );
        assert_eq!(tree.session_count(), 1);
        assert!(!tree.expanded.contains("prod"));
        assert!(!tree.expanded.contains("prod-data"));
        assert_eq!(tree.find_session("db-01"), None);
    }

    #[test]
    fn rename_and_field_edits_only_hit_the_target_node() {
        let mut tree = tree();

        assert!(tree.rename("prod", "生产环境-改"));
        assert_eq!(tree.node_name("prod"), Some("生产环境-改"));
        assert!(tree.is_directory("prod"));
        assert!(!tree.is_directory("web-01"));

        assert!(tree.with_session_mut("web-01", |session| {
            session.host = "192.168.0.2".to_owned();
            session.port = 2222;
        }));
        assert_eq!(
            tree.find_session("web-01").map(|s| s.address()),
            Some("192.168.0.2:2222".to_owned())
        );
        // 其它节点不受影响。
        assert_eq!(
            tree.find_session("db-01").map(|s| s.address()),
            Some("10.0.0.21:22".to_owned())
        );

        assert!(!tree.with_session_mut("missing", |_| {}));
        assert!(!tree.rename("missing", "x"));
        assert_eq!(tree.remove_node("missing"), None);
    }

    #[test]
    fn new_nodes_attach_to_a_selected_directory_only() {
        let tree = tree();
        assert_eq!(tree.parent_for_new(Some("prod")).as_deref(), Some("prod"));
        assert_eq!(tree.parent_for_new(Some("web-01")), None);
        assert_eq!(tree.parent_for_new(None), None);
    }

    /// `compat` 对应旧版 C# 的控件（注册名 version 10），`modern` 对应
    /// version 11。落盘码必须能无损往返，否则重启后会静默回到默认。
    #[test]
    fn every_graphics_code_round_trips_to_a_distinct_control() {
        assert_eq!(
            RdpSettings::default()
                .resolve(&RdpSettings::default())
                .graphics,
            RdpGraphics::Compat,
            "内置默认必须保持兼容档"
        );
        for value in RdpGraphics::ALL {
            assert_eq!(RdpGraphics::from_code(value.code()), Some(value));
            assert!(matches!(value.control(), "10" | "11"));
        }
        assert_eq!(RdpGraphics::Compat.control(), "10");
        assert_eq!(RdpGraphics::Modern.control(), "11");
        assert_ne!(RdpGraphics::Compat.control(), RdpGraphics::Modern.control());
    }

    #[test]
    fn rdp_settings_resolve_from_session_then_defaults_then_builtin() {
        let defaults = RdpSettings {
            performance: Some(RdpPerformance::Lan),
            color_depth: Some(RdpColorDepth::Bpp16),
            audio: Some(RdpAudio::Remote),
            ..RdpSettings::default()
        };

        let inherited = RdpSettings::default().resolve(&defaults);
        assert_eq!(inherited.performance, RdpPerformance::Lan);
        assert_eq!(inherited.color_depth, RdpColorDepth::Bpp16);
        assert_eq!(inherited.audio, RdpAudio::Remote);
        // 程序内置默认：32 位、剪贴板开、其余重定向关、跟随窗口。
        assert_eq!(inherited.keyboard, RdpKeyboard::Fullscreen);
        assert!(inherited.redirect_clipboard);
        assert!(!inherited.redirect_printers);
        assert_eq!(inherited.desktop_size, RdpDesktopSize::FollowWindow);

        let overrides = RdpSettings {
            performance: Some(RdpPerformance::Modem),
            audio: Some(RdpAudio::None),
            redirect_clipboard: Some(false),
            redirect_drives: Some(true),
            ..RdpSettings::default()
        };
        let resolved = overrides.resolve(&defaults);
        assert_eq!(resolved.performance, RdpPerformance::Modem);
        assert_eq!(resolved.color_depth, RdpColorDepth::Bpp16);
        assert_eq!(resolved.audio, RdpAudio::None);
        assert!(!resolved.redirect_clipboard);
        assert!(resolved.redirect_drives);
    }

    #[test]
    fn every_terminal_scheme_code_round_trips() {
        for scheme in TerminalColorScheme::ALL {
            assert_eq!(
                TerminalColorScheme::from_code(scheme.code()),
                Some(scheme),
                "{:?} 的落盘码解析不回来，重启后这个方案会静默消失",
                scheme
            );
        }
    }

    #[test]
    fn terminal_settings_resolve_and_clamp() {
        let defaults = TerminalSettings {
            font_family: Some("Cascadia Mono".to_owned()),
            font_size: Some(15),
            cursor: Some(TerminalCursor::Beam),
            backspace: Some(TerminalBackspace::Backspace),
            home_end: Some(TerminalHomeEnd::Rxvt),
            color_scheme: Some(TerminalColorScheme::SolarizedDark),
            charset: Some(TerminalCharset::Gbk),
            mouse_mode: Some(TerminalMouseMode::Xterm),
            keypad: Some(TerminalKeypad::Linux),
        };

        let inherited = TerminalSettings::default().resolve(&defaults);
        assert_eq!(inherited.font_family, "Cascadia Mono");
        assert_eq!(inherited.font_size, 15);
        assert_eq!(inherited.cursor, TerminalCursor::Beam);
        assert_eq!(inherited.backspace, TerminalBackspace::Backspace);
        assert_eq!(inherited.home_end, TerminalHomeEnd::Rxvt);
        assert_eq!(inherited.color_scheme, TerminalColorScheme::SolarizedDark);
        assert_eq!(inherited.charset, TerminalCharset::Gbk);
        assert_eq!(inherited.mouse_mode, TerminalMouseMode::Xterm);
        assert_eq!(inherited.keypad, TerminalKeypad::Linux);

        let overrides = TerminalSettings {
            font_family: Some("  ".to_owned()),
            font_size: Some(99),
            cursor: Some(TerminalCursor::Underline),
            ..TerminalSettings::default()
        };
        let resolved = overrides.resolve(&defaults);
        // 空白字体名回退到默认；字号被夹到 32。
        assert_eq!(resolved.font_family, "Cascadia Mono");
        assert_eq!(resolved.font_size, 32);
        assert_eq!(resolved.cursor, TerminalCursor::Underline);
        assert_eq!(resolved.backspace, TerminalBackspace::Backspace);

        let frozen = resolved.clone().into_settings();
        assert_eq!(frozen.resolve(&TerminalSettings::default()), resolved);
    }

    #[test]
    fn effective_rdp_settings_can_be_frozen_as_defaults() {
        let resolved = ResolvedRdpSettings {
            host_mode: RdpHostMode::Isolated,
            performance: RdpPerformance::Wan,
            color_depth: RdpColorDepth::Bpp24,
            audio: RdpAudio::None,
            audio_capture: true,
            keyboard: RdpKeyboard::Remote,
            redirect_printers: true,
            redirect_clipboard: false,
            redirect_smartcards: true,
            redirect_ports: true,
            redirect_drives: true,
            desktop_size: RdpDesktopSize::custom(1600, 900),
            graphics: RdpGraphics::Modern,
        };

        let frozen = resolved.into_settings();
        assert_eq!(frozen.host_mode, Some(RdpHostMode::Isolated));
        assert_eq!(frozen.performance, Some(RdpPerformance::Wan));
        assert_eq!(frozen.color_depth, Some(RdpColorDepth::Bpp24));
        assert_eq!(frozen.audio, Some(RdpAudio::None));
        assert_eq!(frozen.graphics, Some(RdpGraphics::Modern));
        assert_eq!(frozen.audio_capture, Some(true));
        assert_eq!(frozen.keyboard, Some(RdpKeyboard::Remote));
        assert_eq!(frozen.redirect_clipboard, Some(false));
        assert_eq!(frozen.redirect_drives, Some(true));
        assert_eq!(frozen.desktop_size, Some(RdpDesktopSize::custom(1600, 900)));
        assert_eq!(frozen.resolve(&RdpSettings::default()), resolved);
    }

    #[test]
    fn desktop_size_codes_round_trip_and_old_presets_still_parse() {
        for size in [
            RdpDesktopSize::FollowWindow,
            RdpDesktopSize::Fullscreen,
            RdpDesktopSize::custom(1366, 768),
        ] {
            assert_eq!(RdpDesktopSize::from_code(&size.code()), Some(size));
        }

        // 旧版本落盘的四个预置码与新格式同构，必须继续认。
        assert_eq!(
            RdpDesktopSize::from_code("1920x1080"),
            Some(RdpDesktopSize::custom(1920, 1080))
        );
        // 越界的尺寸不认，交给回退逻辑，避免写出 mstsc 不接受的 .rdp。
        assert_eq!(RdpDesktopSize::from_code("10x10"), None);
        assert_eq!(RdpDesktopSize::from_code("1920*1080"), None);
        assert_eq!(RdpDesktopSize::from_code("warp"), None);
    }

    #[test]
    fn custom_sizes_are_clamped_into_the_range_mstsc_accepts() {
        assert_eq!(
            RdpDesktopSize::custom(100, 100),
            RdpDesktopSize::Custom {
                width: MIN_DESKTOP_WIDTH,
                height: MIN_DESKTOP_HEIGHT
            }
        );
        assert_eq!(
            RdpDesktopSize::custom(99999, 99999).pixels(),
            Some((MAX_DESKTOP_WIDTH, MAX_DESKTOP_HEIGHT))
        );
    }

    #[test]
    fn only_the_fullscreen_choice_starts_fullscreen() {
        assert!(RdpDesktopSize::Fullscreen.starts_fullscreen());
        assert!(!RdpDesktopSize::FollowWindow.starts_fullscreen());
        assert!(!RdpDesktopSize::custom(1920, 1080).starts_fullscreen());
    }

    /// 某个目录的直接子节点 id（按顺序），用来断言移动后的落点。
    fn child_ids(tree: &SessionTree, dir: &str) -> Vec<String> {
        fn walk(nodes: &[SessionNode], dir: &str) -> Vec<String> {
            for node in nodes {
                if node.id() == dir {
                    return match node {
                        SessionNode::Directory(d) => {
                            d.children.iter().map(|c| c.id().clone()).collect()
                        }
                        SessionNode::Session(_) => Vec::new(),
                    };
                }
                if let SessionNode::Directory(d) = node {
                    let found = walk(&d.children, dir);
                    if !found.is_empty() {
                        return found;
                    }
                }
            }
            Vec::new()
        }

        walk(&tree.roots, dir)
    }

    fn root_ids(tree: &SessionTree) -> Vec<String> {
        tree.roots.iter().map(|node| node.id().clone()).collect()
    }

    #[test]
    fn moving_into_a_directory_appends_it_last() {
        let mut tree = tree();

        assert!(tree.move_node("web-01", MoveTarget::IntoDirectory("qa".into())));
        assert_eq!(tree.parent_id_of("web-01"), Some("qa".to_owned()));
        assert_eq!(child_ids(&tree, "qa").join(","), "legacy,web-01");
    }

    #[test]
    fn moving_next_to_a_sibling_keeps_the_level_and_lands_behind_it() {
        let mut tree = tree();

        assert!(tree.move_node("jump", MoveTarget::NextTo("web-01".into())));
        assert_eq!(tree.parent_id_of("jump"), Some("prod".to_owned()));
        assert_eq!(child_ids(&tree, "prod").join(","), "web-01,jump,prod-data");
    }

    #[test]
    fn moving_to_root_lifts_it_out_of_its_directory() {
        let mut tree = tree();

        assert!(tree.move_node("db-01", MoveTarget::ToRoot));
        assert_eq!(tree.parent_id_of("db-01"), None);
        assert_eq!(root_ids(&tree).join(","), "prod,qa,db-01");
    }

    /// 把目录挪进它自己的子树会让树变成环，之后遍历再也出不来。
    #[test]
    fn a_directory_cannot_be_moved_into_its_own_subtree() {
        let mut tree = tree();

        assert!(!tree.move_node("prod", MoveTarget::IntoDirectory("prod-data".into())));
        assert_eq!(tree.parent_id_of("prod"), None, "失败时必须留在原地");
        assert!(!tree.move_node("prod", MoveTarget::NextTo("db-01".into())));
        assert_eq!(root_ids(&tree).join(","), "prod,qa", "失败时树不能被改动");
    }

    #[test]
    fn a_node_cannot_be_moved_next_to_itself_or_into_a_missing_target() {
        let mut tree = tree();

        assert!(!tree.move_node("web-01", MoveTarget::NextTo("web-01".into())));
        assert!(!tree.move_node("web-01", MoveTarget::IntoDirectory("nope".into())));
        assert_eq!(tree.parent_id_of("web-01"), Some("prod".to_owned()));
    }

    /// 挪动不该把子树里已经展开的目录折起来——用户的展开状态要跟着走。
    #[test]
    fn moving_keeps_the_expanded_state_of_the_moved_subtree() {
        let mut tree = tree();
        tree.expanded.insert("prod-data".to_owned());

        assert!(tree.move_node("prod-data", MoveTarget::IntoDirectory("qa".into())));

        assert!(
            tree.expanded.contains("prod-data"),
            "展开状态要跟着子树一起走"
        );
    }
}
