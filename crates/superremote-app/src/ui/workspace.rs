//! 右侧工作区：标签条 + 当前页面（详情 / 终端 / 内嵌远程桌面 / 设置）。
//!
//! 标签条进标题栏（终稿 ②）：单行「徽标 + 标题 + ✕」，活动标签是浅灰底 +
//! 2px 强调色下划线。会话页不再有页头——画面直接铺满标题栏以下的整块区域，
//! 终端在底部留一条 26px 状态条。

use iced::widget::{
    button, canvas, column, container, mouse_area, row, space, stack, text, Image, Row,
};
use iced::{alignment, Color, ContentFit, Element, Fill, Padding};
use superremote_core::session::SessionKind;
use superremote_core::tab::Tab;

use crate::app::{App, Message, PaneFit, TabAction};
use crate::terminal::Status as ConnStatus;
use crate::ui::context_menu::ContextMenu;
use crate::ui::terminal::TerminalCanvas;
use crate::{i18n, theme, ui};

/// 标签在标题栏里吃满 38 高（比原来悬在内容区顶部的 32 更高，点击区更大）。
const TAB_HEIGHT: f32 = ui::TITLEBAR_H - 1.0;
/// 标签的最大宽度。稿子写的是 220，超长标题直接截断。
const TAB_MAX: f32 = 220.0;
/// 标签内边距与关闭按钮的固定开销：左右各 12、徽标前后各 8、关闭 18。
const TAB_CHROME: f32 = 12.0 + 8.0 + 8.0 + 18.0 + 12.0;
/// 主页标签固定宽度：两个字 + 左右 12。
const HOME_TAB_WIDTH: f32 = 48.0;

pub fn view(app: &App, available: f32) -> Element<'_, Message> {
    if app.screen == crate::app::Screen::Settings {
        return ui::settings::view(app, available);
    }

    // 页面不再自己描边：标题栏下方已有 1px 分隔线、左侧已有竖分隔线、
    // 最外沿已有窗口描边。再套一圈 1px 页面描边，每条边上都会和它们并排
    // 成一道双线（设置页早就没有这圈描边，两个页面也因此对不齐）。
    let page = container(content(app, available))
        .width(Fill)
        .height(Fill)
        .style(theme::canvas());

    // 主窗口会话页的画面区底色与拖边冻结帧统一由页面级覆盖层绘制。
    // 它按窗口原点的偏移定位，所以页面改用单层容器后矩形仍与原生子窗口同源同算。
    match rdp_overlay(app) {
        Some(layer) => stack![page, layer].width(Fill).height(Fill).into(),
        None => page.into(),
    }
}

/// 主窗口会话页的 RDP 覆盖层：画面区底色 +（拖边时的）冻结帧。
///
/// 页面元素的原点就是窗口原点（标题栏由 `main_view` 排在它上面），所以抓帧记下的
/// `offset`（画面区相对窗口左上角、已扣掉标题栏高度）可以直接当内边距用。
fn rdp_overlay(app: &App) -> Option<Element<'_, Message>> {
    if app.screen != crate::app::Screen::Remote || app.show_home {
        return None;
    }
    let tab = app.active_tab()?;
    if tab.session.kind != SessionKind::Rdp || !app.rdp.contains_key(&tab.instance) {
        return None;
    }

    // 连接中/失败/断开时 `rdp_surface` 自己画状态卡，这层底色会把它压掉。
    let live = matches!(
        app.terminals
            .get(&tab.instance)
            .map(|terminal| &terminal.status),
        Some(ConnStatus::Live)
    );
    if !live {
        return None;
    }

    let hwnd = app.main_window.and_then(|id| app.hwnds.get(&id).copied())?;
    let frame = superremote_win::rdp_host::frozen_frame_for(hwnd);
    // 拖边时用抓帧记下的偏移（与子窗口同一个 GetWindowRect），平时按
    // `logical_rect` 现算：同一原点、同一取整，两条路径不会差 1px。
    let offset = match frame.as_ref() {
        Some(frame) => frame.offset,
        None => superremote_win::rdp_host::pane_offset(
            hwnd,
            superremote_win::rdp_host::PaneLayout::Main,
        )?,
    };

    Some(pane_layer(offset, frame))
}

/// 画面区图层：底色铺满 `logical_rect` 的矩形，冻结帧按 1:1 物理像素贴在其左上角。
fn pane_layer<'a>(
    offset: (f32, f32),
    frame: Option<superremote_win::rdp_host::FrozenFrame>,
) -> Element<'a, Message> {
    let content: Element<'_, Message> = match frame {
        Some(frame) => {
            let scale = frame.scale;
            Image::new(frame.handle)
                .content_fit(ContentFit::None)
                // 图片按逻辑坐标绘制时会被 DPI 放大，这里反向抵消，
                // 保证 1 个图像像素正好落在 1 个物理像素上。
                .scale(1.0 / scale)
                .width(frame.width as f32 / scale)
                .height(frame.height as f32 / scale)
                .into()
        }
        None => space().width(Fill).height(Fill).into(),
    };

    // 内层裁剪容器吃满扣掉偏移后的区域，宽高正好是画面区：
    // 窗口缩小时超出的旧画面被裁掉，不会压进缩放热区。
    let pane = container(content)
        .width(Fill)
        .height(Fill)
        .clip(true)
        .style(theme::rdp_backdrop());

    container(pane)
        .width(Fill)
        .height(Fill)
        .padding(Padding {
            top: offset.1,
            right: offset.0,
            bottom: offset.0,
            left: offset.0,
        })
        .into()
}

/// 全屏时只画远程画面占位；标题栏、标签条、页头都由上层让位。
pub fn fullscreen_view(app: &App) -> Element<'_, Message> {
    match app.active_tab() {
        // 全屏页的原点就是窗口原点（只有顶部一条退出栏），冻结帧就地画。
        Some(tab) if tab.session.kind == SessionKind::Rdp => rdp_surface(app, tab, false),
        // 提示页底色跟界面主题走：之前固定传深色色板，亮色主题下白底白字。
        _ => notice(
            i18n::t().rdp_title,
            i18n::t().session_missing_hint,
            theme::palette(),
        ),
    }
}

/// 标签条：由标题栏调用（`ui::chrome::title_bar`），自己不画底色与分隔线。
pub fn tab_strip(app: &App) -> Element<'_, Message> {
    let mut tabs: Row<'_, Message> = row![].spacing(2).height(TAB_HEIGHT);

    tabs = tabs.push(home_tab(app.show_home));
    if app.new_tab.is_some() {
        tabs = tabs.push(new_tab_cell(app));
    }
    for (index, tab) in app.tabs.tabs().iter().enumerate() {
        tabs = tabs.push(tab_cell(app, index, tab));
    }
    // 「+」开一个新建连接标签页，而不是回到已经在前台的主页。
    tabs = tabs.push(ui::tooltip(
        ui::button::icon(
            text("+").size(18),
            40.0,
            TAB_HEIGHT,
            Some(Message::NewTabOpen),
            theme::icon_button(),
        ),
        i18n::t().tooltip_new_tab,
        ui::TITLEBAR_TOOLTIP,
    ));

    tabs.height(TAB_HEIGHT).into()
}

/// 第一个固定标签：选中条目的详情/编辑器。
fn home_tab(active: bool) -> Element<'static, Message> {
    let p = theme::palette();

    let body = column![
        container(
            text(i18n::t().tab_details)
                .size(12)
                .color(if active { p.text } else { p.text_dim })
                .font(if active {
                    theme::ui_font_weight(iced::font::Weight::Semibold)
                } else {
                    theme::ui_font()
                }),
        )
        .width(Fill)
        .height(TAB_HEIGHT - 2.0)
        .padding([0.0, 12.0])
        .align_x(alignment::Horizontal::Center)
        .align_y(alignment::Vertical::Center),
        container(space().width(Fill).height(2.0)).style(theme::tab_underline(active)),
    ]
    .spacing(0)
    .width(Fill)
    .height(TAB_HEIGHT);

    button(body)
        .on_press(Message::ShowHome)
        .width(HOME_TAB_WIDTH)
        .height(TAB_HEIGHT)
        .padding(0.0)
        .style(theme::tab_label(active, false))
        .into()
}

/// 「+」开的临时新建连接标签：`[标题 | ×]`，关闭即放弃草稿。
fn new_tab_cell<'a>(app: &'a App) -> Element<'a, Message> {
    let p = theme::palette();
    let t = i18n::t();
    let active = app.screen == crate::app::Screen::NewTab;
    let title = app
        .new_tab_session()
        .map(|session| session.name.as_str())
        .unwrap_or(t.new_tab_title);
    let width = (ui::estimated_width(title, 12.0) + TAB_CHROME).min(TAB_MAX);

    let label = button(
        text(title)
            .size(12)
            .color(if active { p.text } else { p.text_dim })
            .font(if active {
                theme::ui_font_weight(iced::font::Weight::Semibold)
            } else {
                theme::ui_font()
            })
            .wrapping(iced::widget::text::Wrapping::None),
    )
    .on_press(Message::NewTabActivate)
    .padding(0.0)
    .style(theme::tab_label(active, false));

    let close = ui::tooltip(
        ui::button::icon(
            text("×").size(16),
            18.0,
            18.0,
            Some(Message::NewTabClose),
            theme::tab_close(false),
        ),
        t.close_tab,
        ui::TITLEBAR_TOOLTIP,
    );

    let body = column![
        container(
            row![label, close]
                .spacing(8)
                .align_y(alignment::Vertical::Center)
                .height(TAB_HEIGHT - 2.0),
        )
        .padding([0.0, 12.0])
        .width(Fill)
        .height(TAB_HEIGHT - 2.0)
        .align_y(alignment::Vertical::Center),
        container(space().width(Fill).height(2.0)).style(theme::tab_underline(active)),
    ]
    .spacing(0)
    .width(Fill)
    .height(TAB_HEIGHT);

    container(body)
        .width(width)
        .height(TAB_HEIGHT)
        .style(theme::tab_cell(active, false))
        .into()
}

/// 一个会话标签：`[徽标 | 标题 | ✕]`，宽度按标题估算（上限 220）。
/// 一条会话的右键动作与分组线。**标签条与独立窗口标题条共用这一份**，
/// 不要另写一套。差别只有两条：独立窗口没有「别的标签」可关，也不该在自己
/// 外面再开一个自己，所以 `detached` 时去掉「在新窗口打开」与「关闭其他标签」。
pub fn session_actions(tab: &Tab, detached: bool) -> (Vec<TabAction>, Vec<usize>) {
    let mut actions = Vec::new();
    let mut groups = Vec::new();

    if tab.session.kind == SessionKind::Rdp {
        actions.push(TabAction::Fullscreen(tab.instance));
    }
    actions.push(TabAction::Reconnect(tab.instance));
    if tab.session.kind == SessionKind::Rdp {
        actions.push(TabAction::Disconnect(tab.instance));
    }

    // 画面尺寸自成一组：四档是同一件事的四个取值，混进动作里会看不出层级。
    groups.push(actions.len());
    for fit in PaneFit::ALL {
        actions.push(TabAction::FitSize(tab.instance, fit));
    }

    groups.push(actions.len());
    if !detached {
        actions.push(TabAction::Detach(tab.instance));
        groups.push(actions.len());
    }
    actions.push(TabAction::Close(tab.instance));
    if !detached {
        actions.push(TabAction::CloseOthers(tab.instance));
    }

    (actions, groups)
}

/// 会话菜单那个独立窗口的内容。面板与 overlay 用的是同一份 `menu_body`，
/// 两种宿主长得不可能不一样。
pub fn session_menu_view(app: &App) -> Element<'_, Message> {
    let Some(menu) = app.menu.as_ref() else {
        // 窗口比状态早一帧到位时画全尺寸透明占位：控件类型两路一致。
        return space().width(Fill).height(Fill).into();
    };

    container(ui::context_menu::menu_body(
        &menu.items,
        &menu.groups,
        None,
        &|action| Message::MenuChosen(action),
    ))
    .width(Fill)
    .height(Fill)
    .into()
}

fn tab_cell<'a>(app: &App, index: usize, tab: &'a Tab) -> Element<'a, Message> {
    let p = theme::palette();
    let active = app.screen == crate::app::Screen::Remote
        && !app.show_home
        && app.tabs.active_index() == Some(index);
    let hovered = app.hovered_tab == Some(index);

    let badge = ui::kind_badge(tab.session.kind);
    let badge_w = ui::estimated_width(tab.session.kind.label(), 9.0) + 12.0;
    let title_budget = (TAB_MAX - TAB_CHROME - badge_w).max(24.0);
    let title = ui::fit(tab.title(), title_budget, 12.0);
    let width = (TAB_CHROME + badge_w + ui::estimated_width(&title, 12.0)).min(TAB_MAX);

    let label = button(
        row![
            badge,
            text(title)
                .size(12)
                .color(if active || hovered {
                    p.text
                } else {
                    p.text_dim
                })
                .font(if active {
                    theme::ui_font_weight(iced::font::Weight::Semibold)
                } else {
                    theme::ui_font()
                })
                .wrapping(iced::widget::text::Wrapping::None),
        ]
        .spacing(8)
        .align_y(alignment::Vertical::Center),
    )
    .on_press(Message::ActivateTab(index))
    .padding(0.0)
    .style(theme::tab_label(active, hovered));

    // 关闭按钮常显（稿子里就是一直看得见的 ✕），悬停转危险色。
    let close = ui::tooltip(
        ui::button::icon(
            text("×").size(16),
            18.0,
            18.0,
            Some(Message::CloseTab(index)),
            theme::tab_close(hovered),
        ),
        i18n::t().close_tab,
        ui::TITLEBAR_TOOLTIP,
    );

    let body = column![
        container(
            row![label, close]
                .spacing(8)
                .align_y(alignment::Vertical::Center)
                .height(TAB_HEIGHT - 2.0),
        )
        .padding([0.0, 12.0])
        .width(Fill)
        .height(TAB_HEIGHT - 2.0)
        .align_y(alignment::Vertical::Center),
        container(space().width(Fill).height(2.0)).style(theme::tab_underline(active)),
    ]
    .spacing(0)
    .width(Fill)
    .height(TAB_HEIGHT);

    let (actions, groups) = session_actions(tab, false);

    // 悬停点亮的范围必须盖住整个标签（含关闭按钮），否则鼠标移到按钮上时
    // `on_exit` 先触发，整格底色会当场消失。
    // 右键菜单挂在整格外面：内部的按钮只吃左键，右键能一路走到这里。
    ContextMenu::new(
        mouse_area(
            container(body)
                .width(width)
                .height(TAB_HEIGHT)
                .style(theme::tab_cell(active, hovered)),
        )
        .on_enter(Message::HoverTab(Some(index)))
        .on_exit(Message::HoverTab(None)),
        actions,
        Message::TabAction,
    )
    .groups(groups)
    // 菜单开成独立窗口：原生 RDP 画面是父窗口的子窗口，永远画在 Iced 之上，
    // overlay 形式的菜单会被整个盖掉（只剩标题栏里露出的第一条）。
    .popup(move |at| Message::SessionMenu(tab.instance, at))
    .into()
}

fn content(app: &App, available: f32) -> Element<'_, Message> {
    if app.screen == crate::app::Screen::NewTab {
        // 与右键「新建 SSH」共用同一页详情编辑器：两边看到的必须是同一套控件。
        return ui::editor::view(app, available);
    }
    if app.show_home {
        return ui::editor::view(app, available);
    }

    match app.active_tab() {
        Some(tab) => session_pane(app, tab),
        None => ui::editor::view(app, available),
    }
}

/// 独立窗口的会话画面：不带页头与状态条，只有画面本身。
///
/// 与 [`session_pane`] 共用同一套画布/占位，避免两处渲染出现差异；
/// 标题条在 `ui::chrome::detached_bar`，操作入口挪到了那里。
pub fn detached_body<'a>(app: &'a App, tab: &'a Tab) -> Element<'a, Message> {
    let status = app
        .terminals
        .get(&tab.instance)
        .map(|terminal| terminal.status.clone())
        .unwrap_or(ConnStatus::Connecting);

    match tab.session.kind {
        SessionKind::Rdp => rdp_pane(app, tab, &status, false),
        _ => terminal_pane(app, tab, &status),
    }
}

/// 会话页：标题栏以下整块都是远程画面，不再有页头与操作按钮。
fn session_pane<'a>(app: &'a App, tab: &'a Tab) -> Element<'a, Message> {
    let status = app
        .terminals
        .get(&tab.instance)
        .map(|terminal| terminal.status.clone())
        .unwrap_or(ConnStatus::Connecting);

    match tab.session.kind {
        SessionKind::Rdp => rdp_pane(app, tab, &status, true),
        _ => terminal_pane(app, tab, &status),
    }
}

fn terminal_pane<'a>(app: &'a App, tab: &'a Tab, status: &ConnStatus) -> Element<'a, Message> {
    let t = i18n::t();
    let lang = i18n::lang();

    let Some(terminal) = app.terminals.get(&tab.instance) else {
        return notice(t.session_missing, t.session_missing_hint, theme::palette());
    };

    // 只要远端吐过东西，就把画面交出来：
    // - 连接中：远端可能停在 `password:` 提示上，盖住画面等于让用户没法输口令；
    // - 失败/断开：ssh 的报错和退出前的输出就在终端里，盖掉等于藏起唯一的线索。
    if terminal.received == 0 {
        match status {
            ConnStatus::Connecting => {
                return notice(
                    t.connecting,
                    &lang.connecting_to(&tab.session.address()),
                    theme::palette(),
                )
            }
            ConnStatus::Failed(reason) => {
                return notice(t.connect_failed, reason, theme::palette())
            }
            ConnStatus::Closed(code) => {
                let detail = match code {
                    Some(code) => lang.exited_with(*code),
                    None => t.session_ended.to_owned(),
                };
                return notice(t.session_closed, &detail, theme::palette());
            }
            ConnStatus::Live => {}
        }
    }

    let canvas = canvas(TerminalCanvas {
        frame: &terminal.frame,
        instance: tab.instance,
        blink_on: app.blink_on,
        interactive: terminal.accepts_input(),
        metrics: terminal.metrics,
        font_family: terminal.font_family,
        font_size: terminal.settings.font_size as f32,
        cursor: terminal.settings.cursor,
        selection: app.selection,
        mouse_mode: terminal.settings.mouse_mode,
    })
    .width(Fill)
    .height(Fill);

    // 画布四周留 10/12 的文字内边距，但这圈用终端自己的背景色填：
    // 稿子里的终端是整窗一块深色，而不是被画布底色镶了一圈亮边。
    let backdrop = crate::ui::terminal::to_color(terminal.frame.background);

    column![
        container(canvas)
            .width(Fill)
            .height(Fill)
            .padding([10.0, 12.0])
            .style(move |_| iced::widget::container::Style::default().background(backdrop)),
        ui::divider(),
        terminal_status_bar(tab, terminal, status),
    ]
    .spacing(0)
    .width(Fill)
    .height(Fill)
    .into()
}

/// 终端底部状态条（稿子 ②）：地址 · 状态 · 网格尺寸 …… 已接收字节数。
fn terminal_status_bar<'a>(
    tab: &'a Tab,
    terminal: &'a crate::terminal::Terminal,
    status: &ConnStatus,
) -> Element<'a, Message> {
    let p = theme::palette();
    let (label, tone) = status_text(status);

    container(
        row![
            text(format!(
                "{} · {}",
                tab.session.kind.label(),
                tab.session.address()
            ))
            .size(11)
            .color(p.text_dim),
            text(label).size(11).color(tone),
            text(format!("{}×{}", terminal.cols, terminal.rows))
                .size(11)
                .color(p.text_dim),
            space().width(Fill),
            text(i18n::lang().received(&human_bytes(terminal.received)))
                .size(11)
                .color(p.text_dim),
        ]
        .spacing(14)
        .align_y(alignment::Vertical::Center)
        .padding([0.0, 12.0]),
    )
    .width(Fill)
    .height(ui::STATUS_BAR_H)
    .align_y(alignment::Vertical::Center)
    .style(theme::status_strip())
    .into()
}

/// RDP 页：运行中时整块内容区都是原生画面的占位；未运行时是一张居中的连接卡。
///
/// `in_page` 表示这块画面直接铺在主窗口会话页里：拖边时的冻结帧由页面级
/// 覆盖层负责，这里不再重复画一张会偏位的图。
fn rdp_pane<'a>(
    app: &'a App,
    tab: &'a Tab,
    status: &ConnStatus,
    in_page: bool,
) -> Element<'a, Message> {
    let t = i18n::t();
    let session = &tab.session;
    let running = app.rdp.contains_key(&tab.instance);

    if running {
        return rdp_surface(app, tab, in_page);
    }

    let p = theme::palette();
    let has_password = app
        .store
        .as_ref()
        .is_some_and(|store| store.has_password(&session.id));

    let detail = match status {
        ConnStatus::Failed(reason) => Some(reason.as_str()),
        _ => None,
    };

    let mut card = column![
        // 页级标题统一 16：与本页之外的其他页面标题同号，19 只此一处。
        text(t.rdp_title).size(16).color(p.text),
        text(session.address()).size(11).color(p.text_dim),
        ui::divider(),
        ui::info_row(
            t.field_account,
            text(
                session
                    .username
                    .as_deref()
                    .unwrap_or(t.account_unspecified)
                    .to_owned()
            )
            .size(12)
            .color(p.text),
        ),
        ui::info_row(
            t.field_credential,
            text(if has_password {
                t.credential_saved
            } else {
                t.credential_unsaved
            })
            .size(12)
            .color(p.text),
        ),
        ui::info_row(
            t.field_clipboard,
            text(t.clipboard_enabled).size(12).color(p.text),
        ),
    ]
    .spacing(12)
    .width(Fill);

    if let Some(reason) = detail {
        card = card.push(text(reason.to_owned()).size(11).color(p.danger));
    }

    card = card.push(
        row![
            ui::button::primary(t.rdp_open, Some(Message::LaunchRdp(tab.instance))),
            ui::button::secondary(
                t.rdp_forget_credentials,
                Some(Message::ForgetRdpCredentials(tab.instance)),
            ),
            space().width(Fill),
        ]
        .spacing(4)
        .align_y(alignment::Vertical::Center),
    );

    card = card.push(text(t.rdp_hint).size(theme::T_MICRO).color(p.text_dim));

    container(
        container(card)
            .width(440.0)
            .padding([22.0, 24.0])
            .style(theme::card()),
    )
    .width(Fill)
    .height(Fill)
    .center_x(Fill)
    .center_y(Fill)
    .style(theme::canvas())
    .into()
}

/// 远程桌面占位：原生窗口会盖在这块深色区域上。
///
/// 连接阶段上面还有一层提示文字；原生窗口一旦显示出来就会把它完全盖住，
/// 不需要额外的状态切换，也就不会出现「画面已出现但提示层还在闪」。
fn rdp_surface<'a>(app: &'a App, tab: &'a Tab, in_page: bool) -> Element<'a, Message> {
    let pal = theme::palette();
    let t = i18n::t();
    let status = app
        .terminals
        .get(&tab.instance)
        .map(|terminal| terminal.status.clone())
        .unwrap_or(ConnStatus::Connecting);

    // 状态点与说明文字用的是页面主题色，不再用 RDP 那套固定深色：
    // 卡片底色是主题的 surface，深色文字/深色点放上去才有对比。
    let layer = match status {
        ConnStatus::Live => None,
        ConnStatus::Connecting => Some((
            t.rdp_waiting.to_owned(),
            i18n::lang().connecting_to(&tab.session.address()),
            pal.accent,
            pal.text_dim,
        )),
        ConnStatus::Failed(reason) => {
            Some((t.connect_failed.to_owned(), reason, pal.danger, pal.danger))
        }
        ConnStatus::Closed(code) => Some((
            t.session_closed.to_owned(),
            code.map(|code| i18n::lang().exited_with(code))
                .unwrap_or_else(|| t.session_ended.to_owned()),
            pal.text_dim,
            pal.text_dim,
        )),
    };

    let surface = match superremote_win::rdp_host::frozen_frame() {
        // 拖边期间原生子窗口被挪出客户区：把拖动前抓的帧按 1:1 物理像素贴回
        // 原位置。窗口变大时多出来的区域留 `theme::rdp_fill()`，变小时超出画面区的部分
        // 被裁掉；全程不缩放、不改变画面尺寸、不做任何居中。软光栅下缩放一帧
        // 要 10ms 以上（离线微基准），拖动期不值得为消除松手跳变付这笔开销。
        //
        // 只给独立窗口 / 全屏用：这两种宿主的原点就是画面占位区原点，
        // 偏移可以直接当 padding。主窗口会话页的帧改由页面级覆盖层统一绘制，
        // 避免两处各画一张、位置差一像素。
        Some(frame) if !in_page => {
            let scale = frame.scale;
            container(
                Image::new(frame.handle)
                    .content_fit(ContentFit::None)
                    // 图片按逻辑坐标绘制时会被 DPI 放大，这里反向抵消，
                    // 保证 1 个图像像素正好落在 1 个物理像素上。
                    .scale(1.0 / scale)
                    .width(frame.width as f32 / scale)
                    .height(frame.height as f32 / scale),
            )
            .width(Fill)
            .height(Fill)
            .padding(Padding {
                top: frame.offset.1,
                left: frame.offset.0,
                ..Padding::ZERO
            })
            .clip(true)
            .style(theme::rdp_backdrop())
        }
        _ => container(space().width(Fill).height(Fill)).style(theme::rdp_backdrop()),
    };

    match layer {
        Some((heading, detail, dot, tone)) => {
            // 连接 / 失败 / 已结束共用一张状态卡：第一行是状态点 + 标题，
            // 第二行是说明。之前是一段小字直接贴在 #ABABAB 底色上，
            // 对比度低、也没有信息层级。卡片放在画面区正中，留白由内边距给。
            let card = container(
                column![
                    row![
                        container(space().width(6.0).height(6.0)).style(theme::led(dot)),
                        // 状态卡标题与编辑器分组标题同级：14 + 粗体。
                        text(heading)
                            .size(14)
                            .color(pal.text)
                            .font(theme::ui_font_weight(iced::font::Weight::Bold)),
                    ]
                    .spacing(8)
                    .align_y(alignment::Vertical::Center),
                    text(detail).size(11).color(tone).width(Fill),
                ]
                .spacing(10)
                .width(Fill),
            )
            .width(360.0)
            .padding([16.0, 18.0])
            .style(theme::card());

            stack![
                surface,
                container(card)
                    .width(Fill)
                    .height(Fill)
                    .center_x(Fill)
                    .center_y(Fill),
            ]
            .width(Fill)
            .height(Fill)
            .into()
        }
        None => surface.into(),
    }
}

/// 独立窗口标题条用的状态徽标：按运行实例取状态。
pub fn status_pill_for(app: &App, tab: &Tab) -> Element<'static, Message> {
    let status = app
        .terminals
        .get(&tab.instance)
        .map(|terminal| terminal.status.clone())
        .unwrap_or(ConnStatus::Connecting);
    status_pill(&status)
}

fn status_pill(status: &ConnStatus) -> Element<'static, Message> {
    let (label, color) = status_text(status);

    container(
        row![
            container(space().width(6.0).height(6.0)).style(theme::led(color)),
            text(label).size(11).color(color),
        ]
        .spacing(6)
        .align_y(alignment::Vertical::Center),
    )
    .padding([3.0, 9.0])
    .style(theme::status_pill())
    .into()
}

fn status_text(status: &ConnStatus) -> (&'static str, Color) {
    let p = theme::palette();
    let t = i18n::t();

    match status {
        ConnStatus::Live => (t.status_live, p.ssh),
        ConnStatus::Connecting => (t.status_connecting, p.accent),
        ConnStatus::Closed(_) => (t.status_closed, p.text_dim),
        ConnStatus::Failed(_) => (t.status_failed, p.danger),
    }
}

/// 居中提示页。标题与说明的最大宽度限制在 420，
/// 否则一行长中文会横跨整个内容区，读起来非常吃力。
fn notice(heading: &str, detail: &str, palette: theme::Palette) -> Element<'static, Message> {
    container(
        column![
            text(heading.to_owned()).size(16).color(palette.text),
            text(detail.to_owned())
                .size(12)
                .color(palette.text_dim)
                .align_x(alignment::Horizontal::Center),
        ]
        .spacing(8)
        .align_x(alignment::Horizontal::Center)
        .width(420.0),
    )
    .width(Fill)
    .height(Fill)
    .center_x(Fill)
    .center_y(Fill)
    .style(theme::canvas())
    .into()
}

fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;

    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }

    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}
