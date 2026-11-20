//! 左侧栏：搜索与会话目录树（终稿 E1+ 的中间一栏）。
//!
//! 搜索行是一条 40px 的通栏输入（无外框、只有一条底边线），下面是铺满的树；
//! 页脚统计与设置入口都拿掉了——稿子里这一栏只有「搜索 + 列表」两段，
//! 统计信息不再和连接抢版面。

use iced::widget::{button, column, container, row, scrollable, space, text, text_input, Column};
use iced::{alignment, Element, Fill};
use superremote_core::session::{SessionKind, TreeRow};

use crate::app::{App, Message, Screen, TreeAction};
use crate::ui::context_menu::ContextMenu;
use crate::ui::draggable::Draggable;
use crate::{i18n, theme, ui};

/// 树区两侧的内边距（稿子 `.items { padding: 6px }`）。
const LIST_PAD: f32 = 6.0;
/// 树行的左右内边距。选中块的宽度 = 侧栏宽 - 2×LIST_PAD - 2×ROW_PAD。
const ROW_PAD: f32 = 6.0;
/// 每层缩进。10 是「能看出层级、又不至于五层就到头」的折中。
const INDENT: f32 = 10.0;
/// 行内各元素之间的间距。
const GAP: f32 = 6.0;

/// 地址列的固定宽度：固定下来，地址才会在最右侧排成一条竖线。
const ADDR_W: f32 = 92.0;
/// 目录行箭头的占位宽度。
const ARROW_W: f32 = 10.0;

const NAME_FONT: f32 = 12.0;
const ADDR_FONT: f32 = 11.0;

/// 一行的可用宽度，已扣掉列表与行内两层内边距。
const ROW_INNER: f32 = ui::SIDEBAR_W - 2.0 * LIST_PAD - 2.0 * ROW_PAD;

/// 最左侧图标导航（56px）：主页 / 连接列表 / 设置。
///
/// 三项是三个不同目的地，选中态各自独立；会话画面本身会整窗铺开
/// （稿子 ②），这一栏在那时隐藏。
///
/// 图标必须自己吃满 34×34 再居中：`button` 的布局是 `layout::padded`，
/// 子元素贴在左上角，固定尺寸按钮里的字形不居中会偏到左上角去。
/// 图标由 `ui::icons` 用几何路径绘制，**不借用符号字体**：否则 ⚙ 会被
/// 彩色 emoji 字体接管，活动态的白字也会被 emoji 自带的灰色盖掉。
pub fn rail(app: &App) -> Element<'_, Message> {
    let t = i18n::t();
    let p = theme::palette();
    let item =
        |glyph: ui::icons::RailGlyph, active: bool, message: Message, label: &'static str| {
            let color = if active { p.on_accent } else { p.text_dim };
            ui::tooltip(
                ui::button::icon(
                    ui::icons::rail(glyph, color),
                    34.0,
                    34.0,
                    Some(message),
                    theme::rail_item(active),
                ),
                label,
                iced::widget::tooltip::Position::Right,
            )
        };

    let home = app.screen == Screen::Remote && app.show_home;
    let list = app.screen == Screen::List;
    let settings = app.screen == Screen::Settings;

    container(
        column![
            item(
                ui::icons::RailGlyph::Home,
                home,
                Message::ShowHome,
                t.tab_details
            ),
            item(
                ui::icons::RailGlyph::List,
                list,
                Message::ShowList,
                t.nav_list
            ),
            item(
                ui::icons::RailGlyph::Settings,
                settings,
                Message::ShowSettings,
                t.nav_settings
            ),
        ]
        .spacing(6)
        .align_x(alignment::Horizontal::Center)
        .padding([10.0, 0.0])
        .width(Fill),
    )
    .width(ui::RAIL_W)
    .height(Fill)
    .style(theme::rail())
    .into()
}

pub fn view(app: &App) -> Element<'_, Message> {
    container(
        column![search_row(app), ui::divider(), tree(app)]
            .spacing(0)
            .height(Fill),
    )
    .width(ui::SIDEBAR_W)
    .height(Fill)
    .style(theme::sidebar())
    .into()
}

/// 搜索行：40px 高，无外框，只有一条与列表之间的底边线。
fn search_row(app: &App) -> Element<'_, Message> {
    container(
        text_input(i18n::t().search_placeholder, &app.search)
            .on_input(Message::SearchChanged)
            .padding([0.0, 0.0])
            .size(theme::T_CONTROL)
            .style(theme::sidebar_search()),
    )
    .width(Fill)
    .height(40.0)
    .padding([0.0, 12.0])
    .align_y(alignment::Vertical::Center)
    .into()
}

fn tree(app: &App) -> Element<'_, Message> {
    let rows = app.rows();
    let mut list: Column<'_, Message> = column![].spacing(1);

    if rows.is_empty() {
        let t = i18n::t();
        let hint = if app.search.trim().is_empty() {
            if app.tree().is_some_and(|tree| tree.roots.is_empty()) {
                t.tree_empty
            } else {
                t.tree_locked
            }
        } else {
            t.tree_no_match
        };

        // 空态贴着树区的顶部，而不是垂直居中：列表随时会变成非空，
        // 居中会让提示文字在第一次新建时从屏幕中间「跳」到顶部。
        list = list.push(
            container(text(hint).size(11).color(theme::palette().text_dim))
                .padding([8.0, ROW_PAD])
                .width(Fill),
        );
    } else {
        for item in rows {
            list = list.push(row_widget(app, item));
        }
    }

    // 必须吃满剩余高度：否则空列表时整列比窗口矮，列表会悬在半空。
    // 外层菜单负责空白处右键（新建 / 粘贴到根层）；行上的菜单会先捕获事件。
    let list = scrollable(
        container(list.width(Fill))
            .padding([LIST_PAD, LIST_PAD])
            .width(Fill),
    )
    .height(Fill);

    let menu = root_actions(app);
    ContextMenu::new(list, menu.items, Message::TreeAction)
        .groups(menu.groups)
        .on_open(Message::ClearSelection)
        .into()
}

/// 一行树。目录与连接共用同一套行内骨架：
/// `[缩进 | 箭头或徽标 | 标题 ……… 地址]`，长度一致、左边缘对齐。
fn row_widget<'a>(app: &App, row: &'a TreeRow) -> Element<'a, Message> {
    let p = theme::palette();

    match row {
        TreeRow::Directory {
            id,
            name,
            depth,
            expanded,
        } => {
            let gutter_w = gutter_width(*depth);
            let name = ui::fit(name, ROW_INNER - gutter_w - ARROW_W - GAP, NAME_FONT);

            let label = row![
                gutter(*depth),
                ui::icons::caret(*expanded, p.text_dim),
                text(name).size(NAME_FONT).color(p.text).width(Fill),
            ]
            .spacing(GAP)
            .align_y(alignment::Vertical::Center);

            wrap_row(
                id,
                button(label)
                    .on_press(Message::ToggleDirectory(id.clone()))
                    .width(Fill)
                    .padding([5.0, ROW_PAD])
                    .style(theme::tree_row(app.is_selected(id), app.is_drop_target(id))),
                node_actions(app, id, true),
            )
        }
        TreeRow::Session {
            id,
            name,
            kind,
            address,
            depth,
        } => {
            // 地址单独占右侧而不是压到第二行：两行的树行高 44，右侧栏 720 高
            // 只放得下十来条；单行 28 能把一屏的容量翻倍，这才是连接列表该有的密度。
            let gutter_w = gutter_width(*depth);
            let badge_w = ui::estimated_width(kind.label(), 9.0) + 12.0;
            let name = ui::fit(
                name,
                ROW_INNER - gutter_w - badge_w - ADDR_W - GAP * 2.0,
                NAME_FONT,
            );
            let address = ui::fit(address, ADDR_W, ADDR_FONT);

            let label = row![
                gutter(*depth),
                ui::kind_badge(*kind),
                text(name)
                    .size(NAME_FONT)
                    .color(p.text)
                    .width(Fill)
                    .wrapping(iced::widget::text::Wrapping::None),
                text(address)
                    .size(ADDR_FONT)
                    .color(p.text_dim)
                    .width(ADDR_W)
                    // 右对齐：地址长度不一（`10.0.0.9:23` 与 `192.168.1.44:3389`），
                    // 左对齐时右边缘参差，整列看起来像散落的碎片。
                    .align_x(alignment::Horizontal::Right)
                    .wrapping(iced::widget::text::Wrapping::None),
            ]
            .spacing(GAP)
            .align_y(alignment::Vertical::Center);

            wrap_row(
                id,
                button(label)
                    .on_press(Message::SelectNode(id.clone()))
                    .width(Fill)
                    .padding([5.0, ROW_PAD])
                    .style(theme::tree_row(app.is_selected(id), app.is_drop_target(id))),
                node_actions(app, id, false),
            )
        }
    }
}

/// 一个节点的右键菜单。顺序与旧版一致：连接动作 → 新建 → 剪切/复制/粘贴 →
/// 重命名/删除；目录没有连接动作与复制地址。
pub(crate) struct MenuActions {
    pub items: Vec<TreeAction>,
    /// 每个分组第一条的下标；渲染时在这些下标前画分隔线。
    pub groups: Vec<usize>,
}

pub(crate) fn node_actions(app: &App, id: &str, is_directory: bool) -> MenuActions {
    let mut items = Vec::new();
    let mut groups = Vec::new();

    if !is_directory {
        items.push(TreeAction::Open(id.to_owned()));
        items.push(TreeAction::OpenInBackground(id.to_owned()));
        items.push(TreeAction::OpenInNewWindow(id.to_owned()));
        groups.push(items.len());
    }

    items.push(TreeAction::NewSession(id.to_owned(), SessionKind::Rdp));
    items.push(TreeAction::NewSession(id.to_owned(), SessionKind::Ssh));
    items.push(TreeAction::NewSession(id.to_owned(), SessionKind::Telnet));
    items.push(TreeAction::NewDirectory(id.to_owned()));
    groups.push(items.len());

    items.push(TreeAction::Cut(id.to_owned()));
    items.push(TreeAction::Copy(id.to_owned()));
    if app.node_clipboard.is_some() {
        items.push(TreeAction::Paste(id.to_owned()));
    }
    groups.push(items.len());

    items.push(TreeAction::Rename(id.to_owned()));
    if !is_directory {
        items.push(TreeAction::CopyAddress(id.to_owned()));
    }
    items.push(TreeAction::Delete(id.to_owned()));

    MenuActions { items, groups }
}

/// 树/列表空白处的右键菜单：新建到根层，剪贴板有内容时允许粘贴到根层。
pub(crate) fn root_actions(app: &App) -> MenuActions {
    let root = String::new();
    let mut items = vec![
        TreeAction::NewSession(root.clone(), SessionKind::Rdp),
        TreeAction::NewSession(root.clone(), SessionKind::Ssh),
        TreeAction::NewSession(root.clone(), SessionKind::Telnet),
        TreeAction::NewDirectory(root.clone()),
    ];
    let mut groups = Vec::new();
    if app.node_clipboard.is_some() {
        groups.push(items.len());
        items.push(TreeAction::Paste(root));
    }
    MenuActions { items, groups }
}

/// 把一行包成「可拖拽 + 可右键」。两种树行共用，避免两处写法走偏。
///
/// 事件归属：左键按下/移动/松开归 [`Draggable`]，右键归 [`ContextMenu`]，
/// 普通点击归内容里的 `button`——`button` 在松开时才发 `on_press` 且要求光标仍在
/// 自己身上，所以拖到别的行上不会误触点击。
fn wrap_row<'a>(
    id: &str,
    content: impl Into<Element<'a, Message>>,
    menu: MenuActions,
) -> Element<'a, Message> {
    let hover_id = id.to_owned();
    let draggable = Draggable::new(
        content,
        Message::DragStart(id.to_owned()),
        Message::DragMove,
        move |entered| Message::DragHover(entered.then(|| hover_id.clone())),
        Message::DragDrop(id.to_owned()),
    );

    ContextMenu::new(draggable, menu.items, Message::TreeAction)
        .groups(menu.groups)
        .on_open(Message::HighlightNode(id.to_owned()))
        .into()
}

/// 层级缩进的像素宽度。目录与连接的 `depth` 语义一致，直接用同一个倍数。
///
/// 行内还要减掉 `ROW_PAD`：`padding` 只把内容往里推、不缩小外框，
/// 不减这一项时每层实际的视觉缩进会变成 `INDENT + ROW_PAD`。
fn gutter_width(depth: usize) -> f32 {
    (INDENT * depth as f32 - ROW_PAD).max(0.0)
}

fn gutter(depth: usize) -> Element<'static, Message> {
    space().width(gutter_width(depth)).into()
}
