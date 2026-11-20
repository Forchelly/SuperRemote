//! 无边框窗口的自绘标题栏与缩放热区。
//!
//! 系统标题栏在亮色主题下会突然出现一块 Win32 灰白，与内容区割裂；这里
//! 改成和画布同色的 38px 条：品牌、当前页面标题、窗口按钮。拖动、双击最大化
//! 与四边/四角缩放都走 Iced 的窗口命令，不依赖原生非客户区。

use iced::widget::{container, mouse_area, row, space, stack, text};
use iced::window::Direction;
use iced::{alignment, mouse, Element, Fill, Length};

use crate::app::{App, Message};
use crate::theme;
use crate::ui;
use crate::ui::context_menu::ContextMenu;

/// 标题栏高度与内容区之间的 1px 分隔线共享；按钮区域保持 38 高。
const CONTROL_W: f32 = 44.0;
/// 缩放热区厚度：与原生画面让开的边距同源（`ui::RESIZE_EDGE`），
/// 太薄不好抓，太厚会盖住标题栏与侧栏边缘的点击。
const EDGE: f32 = ui::RESIZE_EDGE;
const CORNER: f32 = ui::RESIZE_CORNER;

pub fn title_bar(app: &App) -> Element<'_, Message> {
    // 标签页搬进标题栏（对齐旧版）：品牌只留 20px 方块，省下的宽度全给标签。
    // 页面标题由标签本身表达，不再重复一行。
    let brand = ui::logo(20.0);

    // 拖拽区只覆盖「标签之后的空白」：把标签包进 mouse_area 的话，
    // 点标签会被当成拖窗口（mouse_area 在按下时就捕获事件）。
    let window = app.main_window;
    let drag_area = mouse_area(
        container(space().width(Fill).height(Fill))
            .width(Fill)
            .height(Fill),
    )
    .interaction(mouse::Interaction::Grab);
    let drag = match window {
        Some(window) => drag_area
            .on_press(Message::WindowDrag(window))
            .on_double_click(Message::WindowToggleMaximize(window)),
        None => drag_area,
    };
    let maximized = window
        .map(|window| app.window_maximized(window))
        .unwrap_or(false);

    let bar = row![
        // 稿子：左边距 14、品牌块 20、再空 10，标签从 44 开始。
        container(brand).padding(iced::Padding {
            top: 0.0,
            right: 10.0,
            bottom: 0.0,
            left: 14.0,
        }),
        ui::workspace::tab_strip(app),
        drag,
        window_button(
            ui::icons::WindowGlyph::Minimize,
            ui::TITLEBAR_H,
            window.map(Message::WindowMinimize),
            false,
            crate::i18n::t().tooltip_minimize,
        ),
        window_button(
            if maximized {
                ui::icons::WindowGlyph::Restore
            } else {
                ui::icons::WindowGlyph::Maximize
            },
            ui::TITLEBAR_H,
            window.map(Message::WindowToggleMaximize),
            false,
            if maximized {
                crate::i18n::t().tooltip_restore
            } else {
                crate::i18n::t().tooltip_maximize
            },
        ),
        window_button(
            ui::icons::WindowGlyph::Close,
            ui::TITLEBAR_H,
            window.map(Message::WindowClose),
            true,
            crate::i18n::t().tooltip_close_window,
        ),
    ]
    .height(ui::TITLEBAR_H - 1.0)
    .width(Fill)
    // 行默认是顶对齐：品牌块只有 20px 高，不居中会贴在标题栏上沿
    // （窗口按钮与标签条都是满高，只有 logo 看起来「飞上去」了）。
    .align_y(alignment::Vertical::Center);

    iced::widget::column![
        container(bar)
            .width(Fill)
            .height(ui::TITLEBAR_H - 1.0)
            .style(theme::title_bar()),
        ui::divider(),
    ]
    .width(Fill)
    .height(ui::TITLEBAR_H)
    .into()
}

/// 一个窗口按钮。居中交给 `ui::button::icon`（`button` 是 `layout::padded`，
/// 内容不会自己居中）；这里只管把图形做出来。`bar_h` 是所在标题条的高度：
/// 主标题栏 38、独立窗口标题条 30 共用同一套图形，格子都吃满它减 1px。
fn window_button(
    glyph: ui::icons::WindowGlyph,
    bar_h: f32,
    message: Option<Message>,
    close: bool,
    label: &'static str,
) -> Element<'static, Message> {
    // 图标颜色沿用主题：常态 `text_dim`，悬停/按下 `text`（关闭按钮用 `on_accent`
    // 压在危险色底上），与底色一起由 `title_button` 负责，不引入新色。
    let p = theme::palette();
    let (color, hot) = if close {
        (p.text_dim, p.on_accent)
    } else {
        (p.text_dim, p.text)
    };
    ui::tooltip(
        ui::button::icon(
            ui::icons::window_control(glyph, color, hot),
            CONTROL_W,
            bar_h - 1.0,
            message,
            theme::title_button(close),
        ),
        label,
        ui::TITLEBAR_TOOLTIP,
    )
}

/// 在整页最上层铺八条透明的缩放热区。
///
/// 无边框窗口没有原生命中区，`decorations: false` 后必须自己补回来，否则
/// 窗口只能最大化、不能拖动边缘改变大小。热区放在 `stack` 的最上层，
/// 但每块都只占边缘 4px（角 8px），不会影响内容区的点击。
///
/// 目标窗口由调用方给死：多窗口下 `window::latest()` 会按焦点猜，
/// 拖独立窗口的边有可能缩到主窗口上。
pub fn with_resize_edges<'a>(
    content: Element<'a, Message>,
    window: iced::window::Id,
) -> Element<'a, Message> {
    let mut layers = stack![content];

    layers = layers.push(edge(
        Fill,
        EDGE,
        alignment::Horizontal::Left,
        alignment::Vertical::Top,
        window,
        Direction::North,
        mouse::Interaction::ResizingVertically,
    ));
    layers = layers.push(edge(
        Fill,
        EDGE,
        alignment::Horizontal::Left,
        alignment::Vertical::Bottom,
        window,
        Direction::South,
        mouse::Interaction::ResizingVertically,
    ));
    layers = layers.push(edge(
        EDGE,
        Fill,
        alignment::Horizontal::Left,
        alignment::Vertical::Top,
        window,
        Direction::West,
        mouse::Interaction::ResizingHorizontally,
    ));
    layers = layers.push(edge(
        EDGE,
        Fill,
        alignment::Horizontal::Right,
        alignment::Vertical::Top,
        window,
        Direction::East,
        mouse::Interaction::ResizingHorizontally,
    ));
    layers = layers.push(edge(
        CORNER,
        CORNER,
        alignment::Horizontal::Left,
        alignment::Vertical::Top,
        window,
        Direction::NorthWest,
        mouse::Interaction::ResizingDiagonallyUp,
    ));
    layers = layers.push(edge(
        CORNER,
        CORNER,
        alignment::Horizontal::Right,
        alignment::Vertical::Top,
        window,
        Direction::NorthEast,
        mouse::Interaction::ResizingDiagonallyDown,
    ));
    layers = layers.push(edge(
        CORNER,
        CORNER,
        alignment::Horizontal::Left,
        alignment::Vertical::Bottom,
        window,
        Direction::SouthWest,
        mouse::Interaction::ResizingDiagonallyDown,
    ));
    layers = layers.push(edge(
        CORNER,
        CORNER,
        alignment::Horizontal::Right,
        alignment::Vertical::Bottom,
        window,
        Direction::SouthEast,
        mouse::Interaction::ResizingDiagonallyUp,
    ));

    // 窗口描边放在**最上层**：四条 1px 细线，而不是一块整窗大小的带边框
    // 容器——后者会让软件后端每帧多铺一遍全屏透明底（实测 +15ms @2x）。
    // 这四层没有 `mouse_area`，不吃事件，边缘热区照常工作。
    layers = layers.push(frame_line(
        Fill,
        1.0,
        alignment::Horizontal::Left,
        alignment::Vertical::Top,
    ));
    layers = layers.push(frame_line(
        Fill,
        1.0,
        alignment::Horizontal::Left,
        alignment::Vertical::Bottom,
    ));
    layers = layers.push(frame_line(
        1.0,
        Fill,
        alignment::Horizontal::Left,
        alignment::Vertical::Top,
    ));
    layers = layers.push(frame_line(
        1.0,
        Fill,
        alignment::Horizontal::Right,
        alignment::Vertical::Top,
    ));

    layers.width(Fill).height(Fill).into()
}

/// 一条窗口描边。外层容器吃满整窗，只负责把 1px 的线推到对应边缘。
fn frame_line<'a>(
    width: impl Into<Length>,
    height: impl Into<Length>,
    align_x: alignment::Horizontal,
    align_y: alignment::Vertical,
) -> Element<'a, Message> {
    container(container(space().width(width).height(height)).style(theme::window_edge()))
        .width(Fill)
        .height(Fill)
        .align_x(align_x)
        .align_y(align_y)
        .into()
}

fn edge<'a>(
    width: impl Into<iced::Length>,
    height: impl Into<iced::Length>,
    align_x: alignment::Horizontal,
    align_y: alignment::Vertical,
    window: iced::window::Id,
    direction: Direction,
    interaction: mouse::Interaction,
) -> Element<'a, Message> {
    let hit = mouse_area(space().width(width).height(height))
        .on_press(Message::WindowResize(window, direction))
        .interaction(interaction);

    container(hit)
        .width(Fill)
        .height(Fill)
        .align_x(align_x)
        .align_y(align_y)
        .into()
}

/// 独立窗口的标题条：左边是会话信息，中间空白可拖拽与双击最大化，右边是状态徽标
/// 加三个窗口按钮。无边框窗口没有原生标题栏，这三个按钮就是 Alt+F4 之外唯一的出口。
pub fn detached_bar<'a>(app: &'a App, tab: &'a superremote_core::tab::Tab) -> Element<'a, Message> {
    let p = theme::palette();
    let t = crate::i18n::t();
    let window = app.windows.iter().find_map(|(id, slot)| match slot {
        crate::app::WindowSlot::Detached(owned) if owned.instance == tab.instance => Some(*id),
        _ => None,
    });

    // 拖拽区只占「信息之后的那段空白」：整条包进 mouse_area 的话，按下窗口按钮会
    // 同时开始拖窗口——mouse_area 在按下那一刻就吃事件，而按钮要等松开才发消息。
    let drag_area = mouse_area(
        container(space().width(Fill).height(Fill))
            .width(Fill)
            .height(Fill),
    )
    .interaction(mouse::Interaction::Grab);
    let drag = match window {
        Some(window) => drag_area
            .on_press(Message::WindowDrag(window))
            .on_double_click(Message::WindowToggleMaximize(window)),
        None => drag_area,
    };

    // 右键这条标题条 = 这条会话的动作，与标签条共用同一份（`session_actions`）。
    // 独立窗口没有标签条，这里就是「画面设为 1920×1080」唯一的入口。
    let (actions, groups) = ui::workspace::session_actions(tab, true);
    let drag = ContextMenu::new(drag, actions, Message::TabAction)
        .groups(groups)
        // 菜单开成独立窗口：原生画面永远画在 Iced 之上，overlay 会被整个盖掉。
        .popup(move |at| Message::SessionMenu(tab.instance, at));

    let maximized = window
        .map(|window| app.window_maximized(window))
        .unwrap_or(false);

    let controls = row![
        window_button(
            ui::icons::WindowGlyph::Minimize,
            ui::FULLSCREEN_BAR_H,
            window.map(Message::WindowMinimize),
            false,
            t.tooltip_minimize,
        ),
        window_button(
            if maximized {
                ui::icons::WindowGlyph::Restore
            } else {
                ui::icons::WindowGlyph::Maximize
            },
            ui::FULLSCREEN_BAR_H,
            window.map(Message::WindowToggleMaximize),
            false,
            if maximized {
                t.tooltip_restore
            } else {
                t.tooltip_maximize
            },
        ),
        window_button(
            ui::icons::WindowGlyph::Close,
            ui::FULLSCREEN_BAR_H,
            window.map(Message::WindowClose),
            true,
            t.tooltip_close_window,
        ),
    ]
    .spacing(0);

    let bar = row![
        ui::kind_badge(tab.session.kind),
        text(tab.title()).size(12).color(p.text),
        text(tab.session.address()).size(10).color(p.text_dim),
        drag,
        ui::workspace::status_pill_for(app, tab),
        controls,
    ]
    .spacing(8)
    .align_y(alignment::Vertical::Center)
    // 只留左边 12px：按钮格子必须贴到窗口右沿，否则右边会像缺了一条。
    .padding(iced::Padding {
        top: 0.0,
        right: 0.0,
        bottom: 0.0,
        left: 12.0,
    });

    // `on_press` / `on_double_click` 按有没有窗口 id 走两种：控件类型两路一致，
    // 不会在「有 id / 没 id」之间改变视图树形状。
    iced::widget::column![
        container(bar)
            .width(Fill)
            .height(ui::FULLSCREEN_BAR_H - 1.0)
            .style(theme::title_bar()),
        ui::divider(),
    ]
    .width(Fill)
    .height(ui::FULLSCREEN_BAR_H)
    .into()
}

/// 全屏时顶部只留一条极窄的返回栏；RDP 原生画面从它下方铺满。
pub fn fullscreen_bar(app: &App) -> Element<'_, Message> {
    let p = theme::palette();
    let t = crate::i18n::t();

    iced::widget::column![
        container(
            row![
                text(app.context_title()).size(11).color(p.text_dim),
                space().width(Fill),
                // 全屏退出用四角内收图形：与标题栏那组同一外接框与线宽，
                // 又和最大化的方块区分开；文字说明留在 tooltip 里。
                ui::tooltip(
                    ui::button::icon(
                        ui::icons::window_control(
                            ui::icons::WindowGlyph::Collapse,
                            p.text_dim,
                            p.text,
                        ),
                        CONTROL_W,
                        ui::FULLSCREEN_BAR_H - 1.0,
                        Some(Message::ExitRdpFullscreen),
                        theme::title_button(false),
                    ),
                    t.exit_fullscreen,
                    ui::TITLEBAR_TOOLTIP,
                ),
            ]
            .spacing(8)
            .align_y(alignment::Vertical::Center)
            .padding([0.0, 12.0]),
        )
        .width(Fill)
        // 栏内容 29 + 1px 分隔线 = 30。与主标题栏同一条边界语言，
        // 总高不变，原生画面仍从 30 开始。
        .height(ui::FULLSCREEN_BAR_H - 1.0)
        .style(theme::title_bar()),
        ui::divider(),
    ]
    .width(Fill)
    .height(ui::FULLSCREEN_BAR_H)
    .into()
}
