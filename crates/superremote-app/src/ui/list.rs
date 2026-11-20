//! 连接列表页（图标栏第二项）。
//!
//! 主页是「树 + 选中项详情」；这一页把树里的所有 RDP / SSH / Telnet 连接
//! 平铺成一张表，方便在连接多、目录深的时候直接找人。点一行选中并回到
//! 主页看详情，右侧「打开」直接开会话标签。

use iced::widget::{button, column, container, row, scrollable, space, text, text_input};
use iced::{alignment, Element, Fill};
use superremote_core::session::{Session, SessionId, SessionNode};

use crate::app::{App, Message, TreeAction};
use crate::ui::context_menu::ContextMenu;
use crate::{i18n, theme, ui};

pub fn view(app: &App, available: f32) -> Element<'_, Message> {
    let t = i18n::t();
    let p = theme::palette();

    let header = row![
        text(t.nav_sessions).size(16).color(p.text),
        container(
            text(i18n::lang().n_sessions(total(app)))
                .size(11)
                .color(p.text_dim),
        )
        .padding([3.0, 8.0])
        .style(theme::count_chip()),
        space().width(Fill),
        text_input(t.search_placeholder, &app.search)
            .on_input(Message::SearchChanged)
            .padding([6.0, 9.0])
            .size(theme::T_CONTROL)
            .style(theme::text_input_style())
            .width(280.0),
    ]
    .spacing(10)
    .align_y(alignment::Vertical::Center);

    let body: Element<'_, Message> = if app.tree().is_none() {
        hint(t.tree_locked, p.text_dim)
    } else {
        let sessions = filtered(app);
        if sessions.is_empty() {
            let message = if app.search.trim().is_empty() {
                t.tree_empty
            } else {
                t.tree_no_match
            };
            hint(message, p.text_dim)
        } else {
            let mut list = column![].width(Fill);
            for session in sessions {
                list = list.push(row_view(app, session, available));
                list = list.push(ui::divider());
            }
            scrollable(list.width(Fill)).height(Fill).into()
        }
    };

    // 空白处右键：新建到根层 / 粘贴到根层（与树侧栏一致）。
    let menu = crate::ui::sidebar::root_actions(app);
    let body = ContextMenu::new(body, menu.items, Message::TreeAction).groups(menu.groups);

    container(
        column![header, ui::divider(), body]
            .spacing(0)
            .width(Fill)
            .height(Fill),
    )
    .width(Fill)
    .height(Fill)
    .padding([ui::PANE_PAD[0], ui::PANE_PAD[1]])
    .style(theme::canvas())
    .into()
}

fn hint(message: &str, color: iced::Color) -> Element<'static, Message> {
    container(text(message.to_owned()).size(12).color(color))
        .width(Fill)
        .height(Fill)
        .center_x(Fill)
        .center_y(Fill)
        .into()
}

/// 树里的全部连接，按树的先序排列；搜索时忽略目录展开状态。
fn filtered(app: &App) -> Vec<&Session> {
    let Some(tree) = app.tree() else {
        return Vec::new();
    };
    let needle = app.search.trim().to_lowercase();
    let mut out = Vec::new();
    collect(&tree.roots, &needle, &mut out);
    out
}

fn total(app: &App) -> usize {
    app.tree().map_or(0, |tree| tree.session_count())
}

fn collect<'a>(nodes: &'a [SessionNode], needle: &str, out: &mut Vec<&'a Session>) {
    for node in nodes {
        match node {
            SessionNode::Directory(directory) => {
                collect(&directory.children, needle, out);
            }
            SessionNode::Session(session) => {
                if needle.is_empty() || session.matches(needle) {
                    out.push(session);
                }
            }
        }
    }
}

fn row_view<'a>(app: &App, session: &'a Session, available: f32) -> Element<'a, Message> {
    let p = theme::palette();
    let t = i18n::t();
    let name = ui::fit(&session.name, (available - 540.0).max(120.0), 13.0);
    let id: SessionId = session.id.clone();

    let main = button(
        row![
            ui::kind_badge(session.kind),
            text(name)
                .size(13)
                .color(p.text)
                .width(Fill)
                .wrapping(iced::widget::text::Wrapping::None),
            text(session.address())
                .size(11)
                .color(p.text_dim)
                .width(180.0)
                .align_x(alignment::Horizontal::Right)
                .wrapping(iced::widget::text::Wrapping::None),
        ]
        .spacing(10)
        .align_y(alignment::Vertical::Center),
    )
    .on_press(Message::SelectNode(id.clone()))
    .width(Fill)
    .padding([9.0, 10.0])
    // 键盘 ↑↓ 会移动 selected；不给选中态，列表页的导航就是隐形的。
    .style(theme::tree_row(app.is_selected(&session.id), false));

    let open = ui::button::secondary(
        t.menu_open,
        Some(Message::TreeAction(TreeAction::Open(id.clone()))),
    );

    let row = row![main, open]
        .spacing(8)
        .align_y(alignment::Vertical::Center);

    let menu = crate::ui::sidebar::node_actions(app, &id, false);
    ContextMenu::new(row, menu.items, Message::TreeAction)
        .groups(menu.groups)
        .on_open(Message::HighlightNode(id))
        .into()
}
