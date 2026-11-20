//! 视图层：每个模块只负责一块区域，消息统一收敛到 `app::Message`。
//!
//! 共享控件集中在这里，是为了让三个页面用同一套尺寸：按钮 `[6, 12]` + 12px 字、
//! 字段标签 11px 弱色、内容区内边距 `[16, 18]`。散落各地的手写内边距是上一版
//! 排版看起来「不齐」的直接原因。
//!
//! 颜色一律来自 [`theme::palette`]：这里出现的每个名字（`p.text_dim`）都在亮暗
//! 两套色板里各有一个值，视图不需要知道当前是哪一套。

pub mod button;
pub mod chrome;
pub mod context_menu;
pub mod draggable;
pub mod editor;
pub mod fields;
pub mod icons;
pub mod list;
pub mod settings;
pub mod sidebar;
pub mod terminal;
pub mod unlock;
pub mod workspace;

use iced::border::Radius;
use iced::widget::canvas::{self, Frame as CanvasFrame, Geometry, Path};
use iced::widget::{
    canvas as canvas_widget, column, container, mouse_area, row, space, text, text_input,
};
use iced::{
    alignment, mouse, Border, Color, Element, Fill, Point, Radians, Rectangle, Renderer, Size,
    Theme,
};
use superremote_core::session::SessionKind;

use crate::app::{Message, Status};
use crate::{i18n, theme};

/// 详情页的内边距（终稿 E1+）：顶部 14、左右 28。
///
/// 这是详情页所有元素的**左基准线**：分组头、字段标签、勾选框、底部操作条
/// 都落在这一条线上。任何一层额外的容器内边距都会把这根线推歪。
/// 设置页用自己的一套（侧边距 16，见 `ui::page_header` 与 `ui::settings`）。
pub const PANE_PAD: [f32; 2] = [14.0, 28.0];

/// 详情页表单的总宽度：标签列 88 + 间距 12 + 控件 400（终稿 E1+ 的基准）。
///
/// 控件不再吃满内容列：864 宽的详情栏里右侧刻意留白，标签与控件才连成一条
/// 稳定的竖线；宽窗口下留白变多，表单本身不变形。
pub const FORM_W: f32 = 500.0;
/// 单个控件的宽度，与 [`FORM_W`] 一起决定表单版的节奏。
pub const FIELD_W: f32 = 400.0;

/// 设置页两列布局的断点：判据是「窗口宽 − 图标栏 − 侧栏 − 分隔线」，
/// 不是窗口宽。侧栏固定 316，用户拖到 1500 宽时内容区只有 1183，
/// 按窗口宽判断会早一步放宽版式。
pub const WIDE_MIN: f32 = 1200.0;

/// 自绘标题栏高度。会话画面从这条线下方整窗铺开。
///
/// 与 RDP 宿主共享同一个值：`superremote_win::layout` 定义，这里只再导出。
pub use superremote_win::layout::{FULLSCREEN_BAR_H, RESIZE_EDGE, TITLEBAR_H};

/// 终端画面底部状态条高度（地址 · 状态 · 网格 · 已接收）。
/// 「画面设为 …」要把它连同分隔线一起从窗口里扣掉，所以不能只写在视图里。
pub const STATUS_BAR_H: f32 = 26.0;

/// 通栏分隔线厚度。
pub const DIVIDER_H: f32 = 1.0;

/// 四角缩放热区厚度，比三条边更宽一点，方便抓住角落。
pub const RESIZE_CORNER: f32 = 8.0;

/// 侧栏宽度。树行的文字预算与 RDP 画面的左边界都由它推算。
pub const SIDEBAR_W: f32 = 260.0;
/// 最左侧图标导航栏宽度（E 方案）：主页 / 设置。
pub const RAIL_W: f32 = 56.0;

/// 会话树的协议徽标、标签条共用。中性描边小签（稿子里的 `.kind`），
/// 颜色不再按协议走——同一屏十几个彩色块会盖过连接名。
pub fn kind_badge(kind: SessionKind) -> Element<'static, Message> {
    let p = theme::palette();

    container(text(kind.label()).size(9).color(p.text_dim))
        .padding([1.0, 5.0])
        .style(theme::kind_badge(kind))
        .into()
}

/// 品牌标：按 0..1 归一化坐标矢量绘制，任意 DPI 都直接按物理像素光栅化。
///
/// 上一版把 128×128 的 RGBA 缩到 20 逻辑像素（150% 下只有 30 物理像素）：
/// 4.3 倍双线性下采样时每个目标像素只采到 2×2 个源像素，显示器的细线和两个
/// 圆环糊成灰块。画布不经过位图缩放，标题栏 30 物理像素、解锁页 96 物理像素
/// 都是同一套几何；线宽兜底 1 逻辑像素，保证小尺寸下笔画仍能分辨。
struct BrandMark;

impl canvas::Program<Message> for BrandMark {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        // 母版底色是近黑深蓝；20px 下看不出那层细微噪点，用纯色即可。
        const PLATE: Color = Color::from_rgb(0.031, 0.035, 0.118);

        let mut frame = CanvasFrame::new(renderer, bounds.size());
        let s = bounds.width.min(bounds.height);
        let at = |x: f32, y: f32| Point::new(x * s, y * s);

        frame.fill(
            &Path::rounded_rectangle(
                Point::new(0.0, 0.0),
                Size::new(s, s),
                Radius::from(s * 0.24),
            ),
            PLATE,
        );

        // 母版线宽约为边长的 3.8%；20px 时不足 1 物理像素，兜底 1 逻辑像素。
        let stroke_w = (s * 0.038).max(1.0);
        let stroke = canvas::Stroke::default()
            .with_color(Color::WHITE)
            .with_width(stroke_w);

        // 显示器外框 (0.164,0.273)-(0.707,0.633)。描边以路径为中线，四边各内缩半个线宽。
        frame.stroke(
            &Path::rounded_rectangle(
                Point::new(0.164 * s + stroke_w / 2.0, 0.273 * s + stroke_w / 2.0),
                Size::new(0.543 * s - stroke_w, 0.360 * s - stroke_w),
                Radius::from(s * 0.012),
            ),
            stroke,
        );

        // 支架：颈 + 底座，母版里就是两块实心白。
        frame.fill(
            &Path::rectangle(
                Point::new(0.391 * s, 0.633 * s),
                Size::new(0.035 * s, 0.053 * s),
            ),
            Color::WHITE,
        );
        frame.fill(
            &Path::rectangle(
                Point::new(0.309 * s, 0.686 * s),
                Size::new(0.199 * s, 0.041 * s),
            ),
            Color::WHITE,
        );

        let big_c = at(0.580, 0.549);
        let small_c = at(0.723, 0.586);
        let big_outer = s * 0.127;
        let small_outer = s * 0.117;
        let big_inner = big_outer - stroke_w;
        let small_inner = small_outer - stroke_w;

        // 大环的圆盘先把显示器右/下线让开（母版里显示器右沿正好与大环外沿相切）。
        frame.fill(&Path::circle(big_c, big_outer), PLATE);
        frame.stroke(&Path::circle(big_c, big_outer - stroke_w / 2.0), stroke);

        // 小环内圈挖空：大环在小环内部让位，右下交点由小环压住。
        frame.fill(&Path::circle(small_c, small_inner), PLATE);
        frame.stroke(&Path::circle(small_c, small_outer - stroke_w / 2.0), stroke);

        // 大环内圈的上半部挖空：小环穿越大环内部的左上段被大环压住，形成链环效果。
        let upper_half = Path::new(|path| {
            path.move_to(Point::new(big_c.x - big_inner, big_c.y));
            path.arc(canvas::path::Arc {
                center: big_c,
                radius: big_inner,
                start_angle: Radians(std::f32::consts::PI),
                end_angle: Radians(std::f32::consts::TAU),
            });
            path.close();
        });
        frame.fill(&upper_half, PLATE);

        vec![frame.into_geometry()]
    }
}

/// 品牌标。标题栏 20px、解锁页 64px、致命错误页 48px 都走同一张矢量图。
pub fn logo(size: f32) -> Element<'static, Message> {
    canvas_widget(BrandMark).width(size).height(size).into()
}

pub fn divider() -> Element<'static, Message> {
    container(space().width(Fill).height(DIVIDER_H))
        .style(theme::divider())
        .into()
}

/// 表单字段：标签在上、输入框在下，统一宽度与字号。
pub fn field<'a>(
    label: &'a str,
    value: &'a str,
    placeholder: &'a str,
    on_change: impl Fn(String) -> Message + 'a,
) -> Element<'a, Message> {
    labelled(
        label,
        text_input(placeholder, value)
            .on_input(on_change)
            .padding([6.0, 9.0])
            .size(theme::T_CONTROL)
            .style(theme::text_input_style()),
    )
}

/// 「按住看口令」：按下显示、松开隐藏。
///
/// 用 `mouse_area` 的按下/松开各发一条**显式**消息，而不是一个 toggle——
/// 长按期间若因为失焦/拖出丢了松开事件，toggle 会停在「一直显示」，
/// 显式赋值则最多停在「显示」，下次按下/松开就纠正回来。
/// 「按住显示口令」的眼睛。用几何路径画：`◎` 在多数字体里是一个靶心圆环，
/// 看不出「显示」的意思；眼睛的两段弧线 + 瞳孔在 16×11 里也能读出来。
struct Eye {
    color: iced::Color,
}

impl canvas::Program<Message> for Eye {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &iced::Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = CanvasFrame::new(renderer, bounds.size());
        let (w, h) = (bounds.width, bounds.height);

        let outline = Path::new(|p| {
            p.move_to(Point::new(1.0, h / 2.0));
            p.quadratic_curve_to(Point::new(w / 2.0, -h * 0.15), Point::new(w - 1.0, h / 2.0));
            p.quadratic_curve_to(Point::new(w / 2.0, h * 1.15), Point::new(1.0, h / 2.0));
            p.close();
        });
        frame.stroke(
            &outline,
            canvas::Stroke::default()
                .with_color(self.color)
                .with_width(1.2),
        );
        frame.fill(&Path::circle(Point::new(w / 2.0, h / 2.0), 1.7), self.color);

        vec![frame.into_geometry()]
    }
}

pub fn hold_reveal(visible: bool) -> Element<'static, Message> {
    let p = theme::palette();
    let color = if visible { p.accent } else { p.text_dim };

    tooltip(
        mouse_area(
            container(canvas_widget(Eye { color }).width(16.0).height(11.0))
                .width(30.0)
                .height(30.0)
                .align_x(alignment::Horizontal::Center)
                .align_y(alignment::Vertical::Center),
        )
        .on_press(Message::SetPasswordVisible(true))
        .on_release(Message::SetPasswordVisible(false))
        .interaction(mouse::Interaction::Pointer),
        i18n::t().tooltip_hold_password,
        iced::widget::tooltip::Position::Bottom,
    )
}

/// 标题栏里的图标按钮统一用这个悬停位置。
///
/// 气泡必须留在标题栏那条带内（主窗口 38、独立窗口 30）：`Bottom` 会落到下面
/// 原生 RDP 画面的区域被整个盖掉，`Top` 直接跑到窗外。`Left` 是垂直居中于图标的，
/// 21px 高的小气泡放在 37px 的格子里刚好全在带内。
pub const TITLEBAR_TOOLTIP: iced::widget::tooltip::Position = iced::widget::tooltip::Position::Left;

/// 悬停提示。统一 350ms 延迟、同一套气泡样式；图标按钮都走这里。
pub fn tooltip<'a>(
    content: impl Into<Element<'a, Message>>,
    label: impl Into<String>,
    position: iced::widget::tooltip::Position,
) -> Element<'a, Message> {
    iced::widget::tooltip(
        content,
        container(text(label.into()).size(11).color(theme::palette().text)).padding([4.0, 8.0]),
        position,
    )
    .gap(6.0)
    .padding(0.0)
    .delay(std::time::Duration::from_millis(350))
    .style(theme::tooltip_style())
    .into()
}

/// 带 [`iced::widget::Id`] 的表单字段：只有需要被**聚焦**的输入框才用它
/// （右键菜单的「重命名」要把光标送进名称框）。`Id::new` 不是 `const fn`，
/// 所以 id 用函数返回而不是常量。
pub fn name_field_id() -> iced::widget::Id {
    iced::widget::Id::new("session-name")
}

pub fn field_id<'a>(
    label: &'a str,
    value: &'a str,
    placeholder: &'a str,
    id: iced::widget::Id,
    on_change: impl Fn(String) -> Message + 'a,
) -> Element<'a, Message> {
    labelled(
        label,
        text_input(placeholder, value)
            .id(id)
            .on_input(on_change)
            .padding([6.0, 9.0])
            .size(theme::T_CONTROL)
            .style(theme::text_input_style()),
    )
}

pub fn field_secure<'a>(
    label: &'a str,
    value: &'a str,
    placeholder: &'a str,
    secure: bool,
    on_change: impl Fn(String) -> Message + 'a,
) -> Element<'a, Message> {
    field_secure_sized(LABEL_W, false, label, value, placeholder, secure, on_change)
}

/// 设置页专用的宽标签版本（标签列 100，禁止折行）。
pub fn field_secure_wide<'a>(
    label: &'a str,
    value: &'a str,
    placeholder: &'a str,
    secure: bool,
    on_change: impl Fn(String) -> Message + 'a,
) -> Element<'a, Message> {
    field_secure_sized(
        SETTINGS_LABEL_W,
        true,
        label,
        value,
        placeholder,
        secure,
        on_change,
    )
}

fn field_secure_sized<'a>(
    label_width: f32,
    nowrap: bool,
    label: &'a str,
    value: &'a str,
    placeholder: &'a str,
    secure: bool,
    on_change: impl Fn(String) -> Message + 'a,
) -> Element<'a, Message> {
    form_row_sized(
        label_width,
        nowrap,
        label,
        text_input(placeholder, value)
            .on_input(on_change)
            .secure(secure)
            .padding([6.0, 9.0])
            .size(theme::T_CONTROL)
            .style(theme::text_input_style()),
    )
}

/// 标签列的固定宽度。**标签在左、控件在右**（对齐旧版的表单密度）：
/// 标签在上会多占一行高度，十几个字段叠下来整页就撑出滚动条。
pub const LABEL_W: f32 = 88.0;

/// 设置页的标签列宽一档：英文 "New password" 等标签在 88px 下会折行，
/// 折行会把同一行两个控件的高度拉开，切语言时整页高度随之变化。
pub const SETTINGS_LABEL_W: f32 = 100.0;

/// 标签列与控件之间的固定间距；勾选框、分组按钮的 100px 缩进 = 88 + 12。
pub const LABEL_GAP: f32 = 12.0;

/// 字段外壳：左边定宽标签 + 右边控件，行高由控件决定。
fn labelled<'a>(label: &'a str, content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    form_row(label, content)
}

/// 与 [`field`] 同一条网格，但允许调用方自己给控件：口令（带长按按钮）、
/// 多行描述、私钥这些行都要在 400 宽里再排子控件。
pub fn form_row<'a>(
    label: &'a str,
    content: impl Into<Element<'a, Message>>,
) -> Element<'a, Message> {
    form_row_sized(LABEL_W, false, label, content)
}

/// 宽标签列版本，供设置页使用：标签不折行，同一行的控件高度稳定。
pub fn form_row_wide<'a>(
    label: &'a str,
    content: impl Into<Element<'a, Message>>,
) -> Element<'a, Message> {
    form_row_sized(SETTINGS_LABEL_W, true, label, content)
}

fn form_row_sized<'a>(
    label_width: f32,
    nowrap: bool,
    label: &'a str,
    content: impl Into<Element<'a, Message>>,
) -> Element<'a, Message> {
    let p = theme::palette();

    let label = text(label).size(theme::T_FIELD_LABEL).color(p.text_dim);
    let label = if nowrap {
        label.wrapping(iced::widget::text::Wrapping::None)
    } else {
        label
    };

    row![
        container(label)
            .width(label_width)
            // 与控件文字同号后仍有半个像素的墨迹偏移；底部留 1px 让居中的
            // 标签整体上移半像素，基线与 13px 的输入文字对齐。
            .padding(iced::Padding {
                top: 0.0,
                right: 0.0,
                bottom: 1.0,
                left: 0.0,
            }),
        content.into(),
    ]
    .spacing(LABEL_GAP)
    .align_y(alignment::Vertical::Center)
    .width(Fill)
    .into()
}

/// 分节标题：一条短线 + 标题，用来在长表单里切出层次。
pub fn section<'a>(title: &'a str) -> Element<'a, Message> {
    let p = theme::palette();

    column![
        text(title).size(11).color(p.text_dim),
        container(space().width(Fill).height(1.0)).style(theme::section_rule()),
    ]
    .spacing(4)
    .width(Fill)
    .into()
}

/// 提示气泡的窗口尺寸：宽度 = 文字估算宽 + 圆点/间距/关闭按钮/内边距（封顶
/// [`TOAST_MAX_W`]，超长的消息换行），高度按行数留。
///
/// 窗口尺寸必须**不小于**实际布局，否则会裁掉文字：`estimated_width` 对 ASCII
/// 用 0.55em（微软雅黑 UI 实际约 0.52em）、对中日韩用 1.0em，都是偏宽的估算；
/// 行高按 20 留（12px 字号的行盒实测不到 20）。宁可宽一点、高一点，也不要裁字。
pub fn toast_size(message: &str) -> iced::Size {
    let text_w = (TOAST_MAX_W - 2.0 * TOAST_PAD_X - TOAST_GUTTER).max(80.0);
    let est = estimated_width(message, TOAST_TEXT);
    let lines = (est / text_w).ceil().max(1.0);

    iced::Size::new(
        est.min(text_w) + 2.0 * TOAST_PAD_X + TOAST_GUTTER,
        lines * TOAST_LINE_H + 2.0 * TOAST_PAD_Y,
    )
}

/// 全局提示条（toast）的气泡本体。
///
/// 它画在**自己的窗口**里，不是页面叠层：原生 RDP 画面是父窗口的子窗口，永远画在
/// Iced 之上，页面里的提示条会被整个盖掉（菜单那侧已经改成独立窗口，提示条同理）。
/// 所以窗口尺寸就是气泡尺寸，见 `toast_size`。
///
/// 旧实现是侧栏里的一条常驻横幅：带描边和 ✕，看起来像一个被填了字的输入框，还会把
/// 搜索行往下推；连接列表页没有侧栏，消息干脆看不见。
pub fn toast_bubble(status: &Status) -> Element<'_, Message> {
    let p = theme::palette();
    let tone = if status.is_error { p.danger } else { p.accent };

    let dot =
        container(space().width(7.0).height(7.0)).style(move |_| iced::widget::container::Style {
            background: Some(iced::Background::Color(tone)),
            border: Border::default().rounded(99.0),
            ..iced::widget::container::Style::default()
        });

    container(
        row![
            dot,
            // 不给文字定宽：窗口宽就是按这条消息的估算宽算的（封顶 `TOAST_MAX_W`
            // 的正文宽），定宽会让文字块比窗口宽，右边整段被裁掉。
            text(status.message.as_str()).size(TOAST_TEXT).color(p.text),
            tooltip(
                button::icon(
                    text("×").size(15),
                    18.0,
                    18.0,
                    Some(Message::DismissStatus),
                    theme::icon_button(),
                ),
                i18n::t().tooltip_dismiss,
                // 提示条窗口只有 38 高：气泡放左边才留在窗口内（Top/Bottom 都会跑到窗外）。
                iced::widget::tooltip::Position::Left,
            ),
        ]
        .spacing(8)
        .align_y(alignment::Vertical::Center),
    )
    .padding([TOAST_PAD_Y, TOAST_PAD_X])
    .width(Fill)
    .height(Fill)
    .style(theme::toast(status.is_error))
    .into()
}

/// 提示条的排版基准：改任何一个都要跟着改 `toast_size`，两处是同一套几何。
const TOAST_TEXT: f32 = 12.0;
const TOAST_LINE_H: f32 = 20.0;
const TOAST_PAD_X: f32 = 12.0;
const TOAST_PAD_Y: f32 = 9.0;
/// 圆点 7 + 两处间距 8×2 + 关闭按钮 18。
const TOAST_GUTTER: f32 = 49.0;
/// 消息文本的换行宽度；再长就折行，`toast_size` 会按行数加高。
const TOAST_MAX_W: f32 = 560.0;

/// 设置页顶部的页头：标题 + 副标题 + 右侧插槽。
///
/// 详情页与会话页的页头已经拿掉（终稿 E1+），现在只有设置页用它；
/// 左右内边距必须与设置页卡片容器的 16 对齐，否则页头比卡片宽出一截。
pub fn page_header<'a>(
    title: impl Into<String>,
    subtitle: impl Into<String>,
    trailing: impl Into<Element<'a, Message>>,
    max_width: f32,
) -> Element<'a, Message> {
    let p = theme::palette();

    container(
        container(
            // 标题与副标题并排：上下叠会白白多吃一行，十几个卡片叠下来就是滚动条。
            row![
                text(title.into()).size(16).color(p.text),
                text(subtitle.into())
                    .size(11)
                    .color(p.text_dim)
                    .wrapping(iced::widget::text::Wrapping::None),
                space().width(Fill),
                // 插槽贴着内容列的右边缘，与表单/卡片的右边界重合。
                trailing.into(),
            ]
            .spacing(8)
            .align_y(alignment::Vertical::Center),
        )
        .width(Fill)
        .max_width(max_width),
    )
    .width(Fill)
    .padding([PANE_PAD[0], 16.0])
    // 与卡片左对齐：全屏时内容从侧栏右侧 16px 起排，而不是整块居中留下一大块
    // 左边距；右半边空出来的宽度由内容列自己的上限决定。
    .align_x(alignment::Horizontal::Left)
    .into()
}

/// 只读键值行：左标签定宽，右值吃满，用于远控面板与设置页。
pub fn info_row(
    label: &str,
    value: impl Into<Element<'static, Message>>,
) -> Element<'static, Message> {
    let p = theme::palette();

    row![
        container(text(label.to_owned()).size(11).color(p.text_dim)).width(76.0),
        value.into(),
    ]
    .spacing(10)
    .align_y(alignment::Vertical::Center)
    .into()
}

/// 按字符估算文本宽度：CJK 记一个字宽、ASCII 记 0.55 个字宽。
///
/// iced 0.14 没有文本测量接口，侧栏与标签条又都需要在**固定预算**里排版，
/// 所以两处共用这一条估算——两套算法会让标题与标签宽度的口径打架。
pub fn estimated_width(value: &str, font_size: f32) -> f32 {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii() {
                font_size * 0.55
            } else {
                font_size
            }
        })
        .sum()
}

/// 按估算宽度截断，超出时补「…」：`Wrapping::None` 只保证不换行，
/// 超宽部分会被外层直接裁掉，连省略号都没有。宁可早一点截断，
/// 也不要让地址或标签被推出可视区。
pub fn fit(value: &str, max_px: f32, font_size: f32) -> String {
    if estimated_width(value, font_size) <= max_px {
        return value.to_owned();
    }

    let budget = (max_px - font_size).max(0.0);
    let mut out = String::new();
    let mut used = 0.0;

    for ch in value.chars() {
        let width = if ch.is_ascii() {
            font_size * 0.55
        } else {
            font_size
        };
        if used + width > budget {
            break;
        }
        used += width;
        out.push(ch);
    }

    out.push('…');
    out
}

/// 侧栏与内容区之间的竖向分隔线。
pub fn vertical_rule() -> Element<'static, Message> {
    container(space().width(1.0).height(Fill))
        .style(theme::divider())
        .into()
}

/// 统一的圆角小标签（状态、模式、数量）。
pub fn pill(label: impl Into<String>, color: iced::Color) -> Element<'static, Message> {
    container(text(label.into()).size(11).color(color))
        .padding([2.0, 8.0])
        .style(move |_| {
            iced::widget::container::Style::default()
                .background(iced::Color { a: 0.12, ..color })
                .border(Border::default().color(color).width(1.0).rounded(9.0))
        })
        .into()
}
