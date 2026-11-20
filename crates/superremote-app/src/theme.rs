//! 配色、圆角、字阶与全部控件样式。
//!
//! 视图模块只引用语义名称（`p.text`、`p.line`），不出现任何字面色值——
//! 这是亮/暗两套主题能成立的前提：多一处 `Color::from_rgb8` 就多一处暗色专有色。
//!
//! 设计方向是「精密仪器」：中性表面、单一强调色、发丝级分隔线，
//! 层次靠表面明度差与极轻的投影，而不是粗描边和大面积渐变。
//! 运维工具长时间盯着看，克制的对比比视觉冲击重要。

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::LazyLock;

use iced::widget::{button, container, overlay::menu, pick_list, text_editor, text_input};
use iced::{Background, Border, Color, Shadow, Theme};
use superremote_core::session::SessionKind;

/// 圆角标度。控件 6、卡片 8、胶囊用全圆角：三种就够，多一种是噪音。
pub const RADIUS_SM: f32 = 6.0;
pub const RADIUS_MD: f32 = 8.0;
pub const RADIUS_PILL: f32 = 999.0;

/// 字阶。所有字号从这里取，避免出现 12 / 12.5 / 13 三种「差不多大」的正文。
pub const T_TITLE: f32 = 15.0;
pub const T_SUBTITLE: f32 = 12.5;
pub const T_BODY: f32 = 12.5;
pub const T_LABEL: f32 = 11.0;
/// 表单左侧的字段名。**与控件正文同号（13）**：11.5 的小字居中到 28px 的行高时，
/// 基线会比 13px 的输入文字高 1 个物理像素（150% DPI 下实测），整页看起来
/// 像「文字高低不齐」。同号同字体的两行文字，基线与视觉中心都会自然对齐。
pub const T_FIELD_LABEL: f32 = 13.0;
pub const T_MICRO: f32 = 10.0;
/// 表单控件正文：输入框、下拉、右键菜单共用同一个字号。
/// 三处各写一个数会出现「输入框 13、下拉 12」这种并排却不齐的版面。
pub const T_CONTROL: f32 = 13.0;

/// Windows 中文界面的标准 UI 字体：微软雅黑 UI。
///
/// 界面文字**只有这一款**：地址、路径、会话状态都不再单独用等宽字体；
/// 图标由 `ui::icons` 几何绘制，不借用符号字体。唯一例外是终端画布——
/// 它使用连接自己选的等宽字体，那是远端内容，不是界面字体。
pub fn ui_font() -> iced::Font {
    iced::Font::with_name("Microsoft YaHei UI")
}

/// 与全局 UI 字体同族的指定字重。
///
/// **不要写 `Font { weight, ..Font::DEFAULT }`**：`Font::DEFAULT` 的 family 是
/// 通用 `SansSerif`，会绕开 `daemon.default_font` 设置的微软雅黑 UI，中文落到
/// 另一款回退字体上——分组标题、标签、Logo 的字形和粗细会跟正文对不上。
pub fn ui_font_weight(weight: iced::font::Weight) -> iced::Font {
    iced::Font {
        weight,
        ..ui_font()
    }
}

/// 一套完整的语义色板。`Copy` 是为了让样式闭包能直接按值捕获。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    /// 内容区底色。
    pub canvas: Color,
    /// 侧栏底色，比画布低一层。
    pub sidebar: Color,
    /// 输入框 / 卡片 / 活动标签。
    pub surface: Color,
    /// 悬停与轻微抬升。
    pub hover: Color,
    /// 选中行。
    pub selected: Color,
    /// 发丝分隔线。
    pub line: Color,
    /// 需要被看见的描边（输入框、卡片）。
    pub line_strong: Color,
    pub text: Color,
    pub text_dim: Color,
    pub accent: Color,
    pub accent_hover: Color,
    pub accent_pressed: Color,
    /// 强调色上的文字。
    pub on_accent: Color,
    pub danger: Color,
    /// 危险操作悬停时的底。
    pub danger_weak: Color,
    pub ssh: Color,
    pub rdp: Color,
    pub telnet: Color,
    /// 卡片投影。暗色下为全透明：靠表面明度差分层，再叠阴影会显脏。
    pub shadow: Color,
}

impl Palette {
    pub const fn dark() -> Self {
        Self {
            canvas: Color::from_rgb8(0x14, 0x17, 0x1C),
            sidebar: Color::from_rgb8(0x0F, 0x12, 0x16),
            surface: Color::from_rgb8(0x1A, 0x1F, 0x27),
            hover: Color::from_rgb8(0x23, 0x2A, 0x34),
            selected: Color::from_rgb8(0x1E, 0x2C, 0x40),
            line: Color::from_rgb8(0x25, 0x2C, 0x36),
            line_strong: Color::from_rgb8(0x33, 0x3C, 0x49),
            text: Color::from_rgb8(0xE4, 0xE9, 0xF1),
            text_dim: Color::from_rgb8(0x8A, 0x95, 0xA6),
            // 强调色要同时满足两条：在深底上看得见（≥3:1），压白字也读得清（≥4.5:1）。
            // 亮蓝（#4C8DF5）只过得了前一条，白字压上去只有 3.3:1，所以整体压暗一档。
            accent: Color::from_rgb8(0x34, 0x70, 0xCE),
            accent_hover: Color::from_rgb8(0x3D, 0x77, 0xD6),
            accent_pressed: Color::from_rgb8(0x2A, 0x5C, 0xAE),
            on_accent: Color::WHITE,
            danger: Color::from_rgb8(0xE4, 0x69, 0x6A),
            danger_weak: Color::from_rgb8(0x3A, 0x24, 0x29),
            ssh: Color::from_rgb8(0x4F, 0xB9, 0x64),
            rdp: Color::from_rgb8(0x34, 0x70, 0xCE),
            telnet: Color::from_rgb8(0xDD, 0xA4, 0x3C),
            shadow: Color::TRANSPARENT,
        }
    }

    pub const fn light() -> Self {
        Self {
            canvas: Color::from_rgb8(0xFB, 0xFB, 0xFC),
            sidebar: Color::from_rgb8(0xF3, 0xF4, 0xF6),
            surface: Color::from_rgb8(0xFF, 0xFF, 0xFF),
            hover: Color::from_rgb8(0xEC, 0xEE, 0xF3),
            selected: Color::from_rgb8(0xE3, 0xEC, 0xFD),
            line: Color::from_rgb8(0xE4, 0xE7, 0xEC),
            line_strong: Color::from_rgb8(0xD2, 0xD8, 0xE0),
            text: Color::from_rgb8(0x16, 0x19, 0x1F),
            text_dim: Color::from_rgb8(0x63, 0x6B, 0x79),
            accent: Color::from_rgb8(0x2B, 0x6C, 0xE0),
            accent_hover: Color::from_rgb8(0x1F, 0x5B, 0xC9),
            accent_pressed: Color::from_rgb8(0x1A, 0x4C, 0xA8),
            on_accent: Color::WHITE,
            danger: Color::from_rgb8(0xC4, 0x3A, 0x3A),
            danger_weak: Color::from_rgb8(0xFD, 0xEC, 0xEC),
            ssh: Color::from_rgb8(0x1B, 0x7F, 0x3F),
            rdp: Color::from_rgb8(0x2B, 0x6C, 0xE0),
            telnet: Color::from_rgb8(0x96, 0x62, 0x00),
            shadow: Color::from_rgba8(0x10, 0x18, 0x28, 0.07),
        }
    }
}

/// 用户可选的主题策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// 跟随系统（默认）。
    #[default]
    System,
    Light,
    Dark,
}

impl Mode {
    pub fn code(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        match code {
            "system" => Some(Self::System),
            "light" => Some(Self::Light),
            "dark" => Some(Self::Dark),
            _ => None,
        }
    }

    /// 解析成真正的亮/暗。`System` 在这里才去问操作系统。
    pub fn is_dark(self) -> bool {
        match self {
            Self::Light => false,
            Self::Dark => true,
            Self::System => system_prefers_dark(),
        }
    }

    pub fn palette(self) -> Palette {
        if self.is_dark() {
            Palette::dark()
        } else {
            Palette::light()
        }
    }
}

/// 读 Windows 的「应用模式」设置：`AppsUseLightTheme = 0` 表示深色。
///
/// 具体读取在 [`superremote_win::system`]；这里保留同名函数，让主题模块的调用点与
/// 测试不必直接拼平台 crate 的路径。
pub fn system_prefers_dark() -> bool {
    superremote_win::system::prefers_dark()
}

// ───────────────────────── 当前主题（进程级） ─────────────────────────
//
// `view(&self)` 只能借用状态，而样式闭包要按值捕获一份 `Palette`；把色板
// 顺着几十个视图函数往下传会让每个签名都多一个参数，读起来全是噪音。
// 这里改成进程级的「当前主题」：启动和设置页切换时写入，视图每帧读一次。
//
// 只缓存**解析结果**（`DARK`），不缓存 mode：`mode()` 要能原样还给设置页的
// 分段控件，而 `palette()` 每帧都可能被调用，不能每次都去读注册表。

static MODE: AtomicU8 = AtomicU8::new(0);
static DARK: AtomicBool = AtomicBool::new(false);

fn mode_code(mode: Mode) -> u8 {
    match mode {
        Mode::System => 0,
        Mode::Light => 1,
        Mode::Dark => 2,
    }
}

/// 切换主题。`System` 会在这里读一次操作系统设置，之后每帧只读缓存。
pub fn set_mode(mode: Mode) {
    MODE.store(mode_code(mode), Ordering::Relaxed);
    DARK.store(mode.is_dark(), Ordering::Relaxed);
}

pub fn current_mode() -> Mode {
    match MODE.load(Ordering::Relaxed) {
        1 => Mode::Light,
        2 => Mode::Dark,
        _ => Mode::System,
    }
}

/// 当前生效的色板。视图层只该从这里取色，不要自己拼 `Color::from_rgb8`。
pub fn palette() -> Palette {
    if DARK.load(Ordering::Relaxed) {
        Palette::dark()
    } else {
        Palette::light()
    }
}

static LIGHT_THEME: LazyLock<Theme> =
    LazyLock::new(|| build_theme("SuperRemote Light", Palette::light()));
static DARK_THEME: LazyLock<Theme> =
    LazyLock::new(|| build_theme("SuperRemote Dark", Palette::dark()));

fn build_theme(name: &'static str, p: Palette) -> Theme {
    Theme::custom(
        name,
        iced::theme::Palette {
            background: p.canvas,
            text: p.text,
            primary: p.accent,
            success: p.ssh,
            warning: p.telnet,
            danger: p.danger,
        },
    )
}

/// 交给 Iced 的主题对象（只影响它自己绘制的少数部件，本项目几乎全部自绘）。
pub fn app_theme() -> Theme {
    if DARK.load(Ordering::Relaxed) {
        DARK_THEME.clone()
    } else {
        LIGHT_THEME.clone()
    }
}

// ───────────────────────────── 容器 ─────────────────────────────

/// 连接列表：稿子里是一块白底，与画布（#FBFBFC）和图标栏（#F2F4F7）都分开。
pub fn sidebar() -> impl Fn(&Theme) -> container::Style {
    let p = palette();
    move |_| container::Style::default().background(p.surface)
}

/// 自绘标题栏：白底，与左侧图标栏、下方画布各差一档。
pub fn title_bar() -> impl Fn(&Theme) -> container::Style {
    let p = palette();
    move |_| container::Style::default().background(p.surface)
}

pub fn canvas() -> impl Fn(&Theme) -> container::Style {
    let p = palette();
    move |_| container::Style::default().background(p.canvas)
}

/// RDP 画面四周的统一底色。
///
/// 取当前主题「比画布低一档」的侧栏色：亮色下接近标题栏白，深色下接近画布。
/// 之前这里写死 `#ABABAB`（mstsc 自绘留边的中灰），白标题栏直接贴上去是
/// 84 级灰阶的硬跳变；现在由主题派生，并作为 `--backdrop` 传给原生宿主，
/// 原生窗口与 Iced 冻结帧始终同色，拖边松手不会跳色。
pub fn rdp_fill() -> Color {
    palette().sidebar
}

pub fn rdp_backdrop() -> impl Fn(&Theme) -> container::Style {
    let fill = rdp_fill();
    move |_| container::Style::default().background(fill)
}

/// 交给原生宿主的颜色（`#RRGGBB`）。C# 侧解析成窗体底色，别处不要用。
pub fn css_hex(color: Color) -> String {
    let [r, g, b, _] = color.into_rgba8();
    format!("#{r:02X}{g:02X}{b:02X}")
}

/// 活动标签的 2px 下划线。非活动标签也占同样高度，切换时文字不上下跳。
pub fn tab_underline(active: bool) -> impl Fn(&Theme) -> container::Style {
    let p = palette();
    move |_| {
        container::Style::default().background(if active { p.accent } else { Color::TRANSPARENT })
    }
}

/// 侧栏与内容区之间的一根发丝线。
pub fn divider() -> impl Fn(&Theme) -> container::Style {
    let p = palette();
    move |_| container::Style::default().background(p.line)
}

pub fn section_rule() -> impl Fn(&Theme) -> container::Style {
    let p = palette();
    move |_| container::Style::default().background(p.line)
}

/// 无边框窗口四周的 1px 描边，**一条边一层**。
///
/// 不要退回「一块整窗大小的带边框 quad」：软件后端会连透明底一起铺满整帧，
/// 无窗口实测（2× 2360×1520）单这一层就要 +15~20ms，页面上看不见任何差别。
/// 四条细线只画 1px 宽的条带，效果相同、代价可以忽略。
/// `decorations: false` 之后窗口和浅色桌面糊成一片，描边走 `line_strong`。
pub fn window_edge() -> impl Fn(&Theme) -> container::Style {
    let p = palette();
    move |_| container::Style::default().background(p.line_strong)
}

/// 最左侧图标导航栏：稿子里固定 #F2F4F7，与列表的白底之间另有一条竖线。
///
/// 这里只画底色、不画边框：Iced 的 `Border` 四边一起画，竖向导航会平白多出
/// 顶边和底边，与窗口描边叠成双层。
pub fn rail() -> impl Fn(&Theme) -> container::Style {
    let p = palette();
    move |_| container::Style::default().background(p.sidebar)
}

/// 图标栏里的一个按钮：选中态是强调色实心块，其余透明。
pub fn rail_item(active: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    let p = palette();
    move |_theme, status| {
        let hot = matches!(status, button::Status::Hovered | button::Status::Pressed) && !active;

        let background = if active {
            // 选中项按下时压暗一档，和标题栏窗口按钮同一套反馈。
            if matches!(status, button::Status::Pressed) {
                p.accent_pressed
            } else {
                p.accent
            }
        } else if hot {
            p.selected
        } else {
            Color::TRANSPARENT
        };

        button::Style {
            background: Some(Background::Color(background)),
            text_color: if active {
                p.on_accent
            } else if hot {
                p.text
            } else {
                p.text_dim
            },
            // 与 `icon_button` 同圆角：两者都是 34px 的图标格子，属于控件层。
            border: Border::default().rounded(RADIUS_SM),
            ..button::Style::default()
        }
    }
}

/// 三态复选框的方框：勾选态用强调色描边，继承态用普通描边。
pub fn checkbox_box(active: bool) -> impl Fn(&Theme) -> container::Style {
    let p = palette();
    move |_| container::Style {
        border: Border::default()
            .color(if active { p.accent } else { p.line_strong })
            .width(1.0)
            .rounded(3.0),
        ..container::Style::default()
    }
}

/// 复选框整行：无底无边，只在悬停时浮出一层浅底（与行内文字按钮同源）。
pub fn checkbox_row() -> impl Fn(&Theme, button::Status) -> button::Style {
    let p = palette();
    move |_theme, status| button::Style {
        background: Some(Background::Color(
            if matches!(status, button::Status::Hovered | button::Status::Pressed) {
                p.hover
            } else {
                Color::TRANSPARENT
            },
        )),
        text_color: p.text,
        border: Border::default().rounded(RADIUS_SM),
        ..button::Style::default()
    }
}

/// 表单卡片：亮色下用一层极轻的投影把卡片「抬」离画布，暗色下纯靠描边。
pub fn card() -> impl Fn(&Theme) -> container::Style {
    let p = palette();
    move |_| container::Style {
        background: Some(Background::Color(p.surface)),
        border: Border::default()
            .color(p.line)
            .width(1.0)
            .rounded(RADIUS_MD),
        // 临时 A/B：卡片阴影是亮色主题里唯一的大面积半透明图层，先关掉它验证
        // 暗块/卡顿是否由它引起（卡片本身有 1px 描边，不至于糊在画布上）。
        shadow: Shadow::default(),
        ..container::Style::default()
    }
}

/// 页头旁边的小计数胶囊：比正文小一号、弱一档，不跟标题抢注意力。
pub fn count_chip() -> impl Fn(&Theme) -> container::Style {
    let p = palette();
    move |_| container::Style {
        background: Some(Background::Color(p.hover)),
        border: Border::default().rounded(RADIUS_PILL),
        ..container::Style::default()
    }
}

/// 面板底：内容区里比画布略亮一层的板块。
pub fn pane() -> impl Fn(&Theme) -> container::Style {
    let p = palette();
    move |_| container::Style {
        background: Some(Background::Color(p.surface)),
        border: Border::default()
            .color(p.line)
            .width(1.0)
            .rounded(RADIUS_MD),
        ..container::Style::default()
    }
}

/// 悬停提示的气泡。与输入框同一套表面色、1px 描边与圆角，
/// 不用阴影（阴影在软件后端上另有一圈脏区问题）。
pub fn tooltip_style() -> impl Fn(&Theme) -> container::Style {
    let p = palette();
    move |_| container::Style {
        background: Some(Background::Color(p.surface)),
        border: Border::default()
            .color(p.line_strong)
            .width(1.0)
            .rounded(RADIUS_SM),
        ..container::Style::default()
    }
}

/// 只读信息块（路径回显、协议说明）。
pub fn info_block() -> impl Fn(&Theme) -> container::Style {
    let p = palette();
    move |_| container::Style {
        background: Some(Background::Color(p.hover)),
        border: Border::default()
            .color(p.line)
            .width(1.0)
            .rounded(RADIUS_SM),
        ..container::Style::default()
    }
}

/// 内容区底部的状态条。稿子里是白底，上面一条发丝线与终端画面分开。
pub fn status_strip() -> impl Fn(&Theme) -> container::Style {
    let p = palette();
    move |_| container::Style::default().background(p.surface)
}

pub fn status_pill() -> impl Fn(&Theme) -> container::Style {
    let p = palette();
    move |_| container::Style {
        background: Some(Background::Color(p.surface)),
        border: Border::default()
            .color(p.line)
            .width(1.0)
            .rounded(RADIUS_PILL),
        ..container::Style::default()
    }
}

pub fn nav_separator() -> impl Fn(&Theme) -> container::Style {
    let p = palette();
    move |_| container::Style::default().background(p.line)
}

/// 协议徽标。稿子里是一个中性描边小签（9px 弱化字 + 1px 边框），
/// 不再按协议染色：列表与标签条里同时出现十几个彩色块会盖过连接名本身。
pub fn kind_badge(_kind: SessionKind) -> impl Fn(&Theme) -> container::Style {
    let p = palette();

    move |_| container::Style {
        background: None,
        border: Border::default()
            .color(p.line_strong)
            .width(1.0)
            .rounded(4.0),
        ..container::Style::default()
    }
}

/// 分组头的圆圈。稿子里是一个 15px 的细环，箭头在环内。
pub fn section_dot() -> impl Fn(&Theme) -> container::Style {
    let p = palette();
    move |_| container::Style {
        border: Border::default()
            .color(p.text_dim)
            .width(1.0)
            .rounded(RADIUS_PILL),
        ..container::Style::default()
    }
}

/// 状态灯。传 `Color::TRANSPARENT` 表示「这一行没有灯」，但仍然占位。
pub fn led(color: Color) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(Background::Color(color)),
        border: Border::default().rounded(RADIUS_PILL),
        ..container::Style::default()
    }
}

/// 提示条：左侧一条色条 + 极淡底纹，不整块染色。
pub fn toast(is_error: bool) -> impl Fn(&Theme) -> container::Style {
    let p = palette();
    let tone = if is_error { p.danger } else { p.accent };

    move |_| container::Style {
        background: Some(Background::Color(p.surface)),
        border: Border::default()
            .color(Color { a: 0.35, ..tone })
            .width(1.0)
            // 直角：提示条现在画在自己的窗口里，窗口没有圆角也没有透明底，
            // 留 6px 圆角只会在四角露出一圈主题底色。
            .rounded(0.0),
        text_color: Some(p.text),
        // 不加高斯阴影：toast 面积小但 blur 很贵，软件后端每帧都要重算一次；
        // 1px 描边 + 表面色已经能和内容分开。
        ..container::Style::default()
    }
}

// ───────────────────────────── 按钮 ─────────────────────────────

/// 标题栏的窗口按钮：平时透明，悬停浮出浅底；关闭按钮悬停时整块变危险色。
pub fn title_button(close: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    let p = palette();
    move |_theme, status| {
        let hot = matches!(status, button::Status::Hovered | button::Status::Pressed);

        let (background, text_color) = if close && hot {
            (p.danger, p.on_accent)
        } else if hot {
            (p.hover, p.text)
        } else {
            (Color::TRANSPARENT, p.text_dim)
        };

        button::Style {
            background: Some(Background::Color(background)),
            text_color,
            border: Border::default(),
            ..button::Style::default()
        }
    }
}

/// 带文字的动作按钮的三种层级。**几何完全一致**：同一套内边距（由
/// [`crate::ui::button`] 提供）、同样的 1px 描边与 6px 圆角，只有颜色不同。
///
/// 不要再为某一个页面单独拼 `button(...)`——「有的描边、有的没描边」
/// 就是这么来的；所有文字按钮都必须走 `ui::button::primary/secondary/danger`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonKind {
    Primary,
    Secondary,
    Danger,
}

/// 动作按钮样式。三种层级共用同一条描边（1px）与圆角，颜色随层级变化：
/// 主操作实心强调色，次要操作表面底 + 发丝边，危险操作危险色文字 + 淡红边。
pub fn action_button(kind: ButtonKind) -> impl Fn(&Theme, button::Status) -> button::Style {
    let p = palette();
    move |_theme, status| {
        let disabled = matches!(status, button::Status::Disabled);
        let hot = matches!(status, button::Status::Hovered);
        let pressed = matches!(status, button::Status::Pressed);

        let (background, border, text_color) = match kind {
            ButtonKind::Primary => {
                let background = if pressed {
                    p.accent_pressed
                } else if hot {
                    p.accent_hover
                } else if disabled {
                    p.hover
                } else {
                    p.accent
                };
                let border = if disabled { p.line } else { background };
                (
                    background,
                    border,
                    if disabled { p.text_dim } else { p.on_accent },
                )
            }
            ButtonKind::Secondary => {
                let background = if pressed {
                    p.selected
                } else if hot {
                    p.hover
                } else if disabled {
                    Color::TRANSPARENT
                } else {
                    p.surface
                };
                (
                    background,
                    if disabled { p.line } else { p.line_strong },
                    if disabled {
                        Color {
                            a: 0.45,
                            ..p.text_dim
                        }
                    } else {
                        p.text
                    },
                )
            }
            ButtonKind::Danger => {
                let background = if hot || pressed {
                    p.danger_weak
                } else if disabled {
                    Color::TRANSPARENT
                } else {
                    p.surface
                };
                (
                    background,
                    if disabled {
                        p.line
                    } else {
                        Color {
                            a: 0.45,
                            ..p.danger
                        }
                    },
                    if disabled {
                        Color {
                            a: 0.45,
                            ..p.danger
                        }
                    } else {
                        p.danger
                    },
                )
            }
        };

        button::Style {
            background: Some(Background::Color(background)),
            text_color,
            // 三种层级同一条描边：宽度 1、圆角 6。主按钮的描边与填充同色，
            // 看起来是实心，但几何与次要/危险按钮完全对齐。
            border: Border::default()
                .color(border)
                .width(1.0)
                .rounded(RADIUS_SM),
            ..button::Style::default()
        }
    }
}

/// 图标按钮（无文字标签）的悬停样式：无描边无底，悬停才浮起浅底。
/// 带文字的按钮一律走 [`action_button`] / `ui::button`，不要用这一条。
pub fn icon_button() -> impl Fn(&Theme, button::Status) -> button::Style {
    let p = palette();
    move |_theme, status| button::Style {
        background: Some(Background::Color(match status {
            button::Status::Hovered => p.hover,
            button::Status::Pressed => p.selected,
            _ => Color::TRANSPARENT,
        })),
        text_color: match status {
            button::Status::Hovered => p.text,
            button::Status::Disabled => Color {
                a: 0.45,
                ..p.text_dim
            },
            _ => p.text_dim,
        },
        border: Border::default().rounded(RADIUS_SM),
        ..button::Style::default()
    }
}

/// 分段控件的轨道：与动作按钮共用 1px 描边与 6px 圆角，
/// 让「外框」在所有控件上保持同一种语言；活动段才是白色滑块。
pub fn segmented_track() -> impl Fn(&Theme) -> container::Style {
    let p = palette();
    move |_| container::Style {
        background: Some(Background::Color(p.sidebar)),
        border: Border::default()
            .color(p.line_strong)
            .width(1.0)
            .rounded(RADIUS_SM),
        ..container::Style::default()
    }
}

/// 分段导航的一段。活动段是一块实心滑块，不是描边方框。
pub fn nav_segment(active: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    let p = palette();
    move |_theme, status| button::Style {
        background: Some(Background::Color(if active {
            p.surface
        } else if matches!(status, button::Status::Hovered | button::Status::Pressed) {
            p.hover
        } else {
            Color::TRANSPARENT
        })),
        text_color: if active { p.text } else { p.text_dim },
        // 滑块嵌在 3px 内边距的轨道里：圆角要比轨道小 3px 才是同心圆弧。
        // 两层都写 6 时，滑块四角会切进轨道描边。
        border: Border::default().rounded(RADIUS_SM - 3.0),
        // 阴影全应用统一为「描边表达层次」：卡片、菜单、分段控件都不加投影，
        // 否则只有这一处浮起来，和其余容器不是一套语言。
        shadow: Shadow::default(),
        ..button::Style::default()
    }
}

/// 会话树的一行：选中态优先于悬停态。
///
/// 底色画在按钮上，所以左右内边距由调用方给；缩进由行内 spacer 表达，
/// 否则选中块会比同级目录行宽出一截，树的左边缘看起来是锯齿状的。
/// 树行。`drop_target` 是拖拽的落点候选，要比「悬停」更醒目：拖拽时手是按着的，
/// 仅靠 hover 的浅底色分不出「指到哪儿」。
pub fn tree_row(
    selected: bool,
    drop_target: bool,
) -> impl Fn(&Theme, button::Status) -> button::Style {
    let p = palette();
    move |_theme, status| {
        let background = if selected {
            p.selected
        } else if matches!(status, button::Status::Pressed) {
            // 按下先用选中色垫底：按住不放时整行不会退回透明，松手才落定选中。
            p.selected
        } else if drop_target || matches!(status, button::Status::Hovered) {
            p.hover
        } else {
            Color::TRANSPARENT
        };

        button::Style {
            background: Some(Background::Color(background)),
            text_color: p.text,
            // 选中态只铺一层淡强调底（稿子里的 `.row.on`），不再额外描边；
            // 拖拽落点才需要一圈强调色边，把「指到哪儿」和「选中哪条」分开。
            border: Border::default()
                .color(if drop_target {
                    p.accent
                } else {
                    Color::TRANSPARENT
                })
                .width(if drop_target { 1.0 } else { 0.0 })
                .rounded(RADIUS_SM),
            ..button::Style::default()
        }
    }
}

/// 标签栏里可点击的标签主体。底色由外壳（[`tab_cell`]）统一承担，
/// 按钮自己保持透明，否则底板会在内容区宽度上断一截。
pub fn tab_label(active: bool, hovered: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    let p = palette();
    move |_theme, status| {
        let hot = matches!(status, button::Status::Hovered) || (hovered && !active);

        button::Style {
            background: None,
            text_color: if active || hot { p.text } else { p.text_dim },
            border: Border::default(),
            ..button::Style::default()
        }
    }
}

/// 整个标签的外壳。活动标签按稿子铺一层比标题栏略深的浅灰（画布色），
/// 关闭按钮一起盖住；真正的选中标记是下面那条 2px 强调色下划线。
pub fn tab_cell(active: bool, hovered: bool) -> impl Fn(&Theme) -> container::Style {
    let p = palette();
    move |_| container::Style {
        background: Some(Background::Color(if active {
            p.sidebar
        } else if hovered {
            p.hover
        } else {
            Color::TRANSPARENT
        })),
        border: Border::default(),
        ..container::Style::default()
    }
}

/// 标签上的关闭按钮：稿子里常显（弱化色），悬停才转危险色。
pub fn tab_close(hovered: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    let p = palette();
    move |_theme, status| {
        let hot = hovered || matches!(status, button::Status::Hovered);

        button::Style {
            background: Some(Background::Color(
                if matches!(status, button::Status::Hovered | button::Status::Pressed) {
                    p.danger_weak
                } else {
                    Color::TRANSPARENT
                },
            )),
            text_color: if hot { p.danger } else { p.text_dim },
            border: Border::default().rounded(4.0),
            ..button::Style::default()
        }
    }
}

// ───────────────────────────── 输入框 ─────────────────────────────

/// 表单输入框。聚焦时描边换成强调色，这是唯一的焦点提示。
pub fn text_input_style() -> impl Fn(&Theme, text_input::Status) -> text_input::Style {
    let p = palette();
    move |_theme, status| {
        let focused = matches!(status, text_input::Status::Focused { .. });

        text_input::Style {
            background: Background::Color(p.surface),
            border: Border::default()
                .color(if focused { p.accent } else { p.line_strong })
                .width(1.0)
                .rounded(RADIUS_SM),
            icon: p.text_dim,
            placeholder: Color {
                a: 0.75,
                ..p.text_dim
            },
            value: p.text,
            selection: Color {
                a: 0.35,
                ..p.accent
            },
        }
    }
}

/// 多行文本编辑器（私钥框）。与输入框共用表面色和描边。
pub fn text_editor_style() -> impl Fn(&Theme, text_editor::Status) -> text_editor::Style {
    let p = palette();
    move |_theme, status| {
        let focused = matches!(status, text_editor::Status::Focused { .. });

        text_editor::Style {
            background: Background::Color(p.surface),
            border: Border::default()
                .color(if focused { p.accent } else { p.line_strong })
                .width(1.0)
                .rounded(RADIUS_SM),
            placeholder: Color {
                a: 0.75,
                ..p.text_dim
            },
            value: p.text,
            selection: Color {
                a: 0.35,
                ..p.accent
            },
        }
    }
}

/// 下拉选择器。和输入框同一套描边/圆角，悬停与展开时描边提亮。
pub fn pick_list_style() -> impl Fn(&Theme, pick_list::Status) -> pick_list::Style {
    let p = palette();
    move |_theme, status| {
        let opened = matches!(status, pick_list::Status::Opened { .. });
        let hovered = matches!(status, pick_list::Status::Hovered)
            || matches!(status, pick_list::Status::Opened { is_hovered: true });

        pick_list::Style {
            text_color: p.text,
            placeholder_color: p.text_dim,
            handle_color: if opened { p.accent } else { p.text_dim },
            // 悬停只提一档底色；强调色描边留给「展开中」，与输入框的聚焦描边同一口径。
            // 之前悬停也画强调色，一屏七八个下拉，鼠标扫过去整页闪边。
            background: Background::Color(if hovered && !opened {
                p.hover
            } else {
                p.surface
            }),
            border: Border::default()
                .color(if opened { p.accent } else { p.line_strong })
                .width(1.0)
                .rounded(RADIUS_SM),
        }
    }
}

/// 下拉弹层与右键菜单共用的样式。
///
/// iced 的默认菜单样式取自 `extended_palette()`：灰底、直角、**亮蓝**选中项配白字，
/// 和本应用的语义色板不是一套（一展开就跳出一块不相干的蓝）。这里让它和输入框同源：
/// 表面色 + 同样的 1px 描边与圆角，当前项用 `selected` 的淡强调底色（与树行选中态同源）。
///
/// 暂不给弹层加阴影：卡片阴影在 150% DPI 下疑似被渲染成大面积暗块，定位清楚之前
/// 浮动层也用描边表达边界。
/// 右键菜单面板：白底、1px 描边、6px 圆角。阴影留空——overlay 的绘制层
/// 按面板矩形裁剪，阴影本来就画不出来，硬加只会得到一圈被切掉的灰边。
pub fn menu_panel() -> impl Fn(&Theme) -> container::Style {
    let p = palette();
    move |_| container::Style {
        background: Some(Background::Color(p.surface)),
        border: Border::default()
            .color(p.line_strong)
            .width(1.0)
            .rounded(RADIUS_SM),
        text_color: Some(p.text),
        ..container::Style::default()
    }
}

/// 菜单里的一行：与其余控件同一套圆角，悬停浮出浅底。
///
/// 悬停态由菜单状态显式传入，不读按钮的 `status`：overlay 在绘制阶段会重建
/// 控件，按钮内部的 `status` 在那一刻是空的（按需重建的控件都不该依赖它）。
pub fn menu_item(hovered: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    let p = palette();
    move |_theme, _status| button::Style {
        background: Some(Background::Color(if hovered {
            p.hover
        } else {
            Color::TRANSPARENT
        })),
        text_color: p.text,
        border: Border::default().rounded(4.0),
        ..button::Style::default()
    }
}

pub fn menu_style() -> impl Fn(&Theme) -> menu::Style {
    let p = palette();
    move |_| menu::Style {
        background: Background::Color(p.surface),
        border: Border::default()
            .color(p.line_strong)
            .width(1.0)
            .rounded(RADIUS_SM),
        text_color: p.text,
        selected_text_color: p.text,
        selected_background: Background::Color(p.selected),
        shadow: Shadow::default(),
    }
}

/// 侧栏顶部的搜索行：稿子里没有输入框外框，只有一行占位文字，
/// 与列表之间用一条底边线分开。聚焦也不画边框——那会让整行突然缩进。
pub fn sidebar_search() -> impl Fn(&Theme, text_input::Status) -> text_input::Style {
    let p = palette();
    move |_theme, _status| text_input::Style {
        background: Background::Color(Color::TRANSPARENT),
        border: Border::default().width(0.0),
        icon: p.text_dim,
        placeholder: Color {
            a: 0.75,
            ..p.text_dim
        },
        value: p.text,
        selection: Color {
            a: 0.35,
            ..p.accent
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 下拉弹层不许落回 iced 的默认菜单样式：默认选中底色/文字色取自
    /// `extended_palette().primary.strong`（亮蓝 + 白字），和本应用的语义色板
    /// 不是一套，一展开就是一块不相干的蓝。这里只钉「不是默认蓝」这个不变量，
    /// 不去比对具体色值——色板是进程级的，和别的用例并发时比色值会间歇失败。
    #[test]
    fn the_dropdown_menu_does_not_fall_back_to_the_default_blue() {
        let theme = app_theme();
        let extended = theme.extended_palette();
        let style = menu_style()(&theme);

        assert_ne!(
            style.selected_background,
            Background::Color(extended.primary.strong.color),
            "选中项又回到了 iced 默认的亮蓝底"
        );
        assert_ne!(
            style.selected_text_color, extended.primary.strong.text,
            "选中项又回到了默认的强对比白字"
        );
        assert_eq!(style.border.width, 1.0, "弹层要有一致的 1px 描边");
        assert_eq!(style.border.radius, RADIUS_SM.into(), "圆角要与输入框一致");
        assert_eq!(style.shadow, Shadow::default(), "阴影未定位清楚前不加");
    }

    /// 两套色板的关键配对必须都过 4.5:1（正文）与 3:1（大字/图形）。
    /// 这些数字是硬指标，不能靠眼睛看着「差不多」。
    #[test]
    fn both_palettes_have_readable_contrast() {
        for (name, p) in [("dark", Palette::dark()), ("light", Palette::light())] {
            assert!(
                contrast(p.text, p.canvas) >= 7.0,
                "{name}: 正文对比度只有 {:.2}",
                contrast(p.text, p.canvas)
            );
            assert!(
                contrast(p.text_dim, p.canvas) >= 4.5,
                "{name}: 弱化文字对比度只有 {:.2}",
                contrast(p.text_dim, p.canvas)
            );
            assert!(
                contrast(p.accent, p.canvas) >= 3.0,
                "{name}: 强调色对比度只有 {:.2}",
                contrast(p.accent, p.canvas)
            );
            assert!(
                contrast(p.on_accent, p.accent) >= 4.5,
                "{name}: 主按钮文字对比度只有 {:.2}",
                contrast(p.on_accent, p.accent)
            );
            assert!(
                contrast(p.text_dim, p.sidebar) >= 4.5,
                "{name}: 侧栏弱化文字对比度只有 {:.2}",
                contrast(p.text_dim, p.sidebar)
            );
        }
    }

    #[test]
    fn modes_round_trip_through_their_codes() {
        for mode in [Mode::System, Mode::Light, Mode::Dark] {
            assert_eq!(Mode::from_code(mode.code()), Some(mode));
        }
        assert_eq!(Mode::from_code("sepia"), None);
    }

    #[test]
    fn explicit_modes_ignore_the_operating_system() {
        assert!(!Mode::Light.is_dark());
        assert!(Mode::Dark.is_dark());
    }

    /// 显式模式必须映射到对应色板。这条不碰进程级状态，所以不会被并行用例干扰。
    #[test]
    fn explicit_modes_map_to_their_palettes() {
        assert_eq!(Mode::Light.palette(), Palette::light());
        assert_eq!(Mode::Dark.palette(), Palette::dark());
    }

    /// WCAG 相对亮度 + 对比度。
    fn contrast(a: Color, b: Color) -> f32 {
        let (la, lb) = (luminance(a), luminance(b));
        let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
        (hi + 0.05) / (lo + 0.05)
    }

    fn luminance(c: Color) -> f32 {
        let channel = |v: f32| {
            if v <= 0.039_28 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(c.r) + 0.7152 * channel(c.g) + 0.0722 * channel(c.b)
    }
}
