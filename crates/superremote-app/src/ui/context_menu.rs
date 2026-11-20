//! 右键菜单：把菜单以 **overlay** 形式挂到任意内容上的包装控件。
//!
//! 为什么需要自己写一个 [`Widget`]：菜单必须以 overlay 的形式浮在最上层并独占
//! 点击，而 `App::view(&self)` 是不可变借用，拿不到菜单状态需要的 `&mut`。
//! `Widget::overlay` 恰好有 `&mut Tree`，所以菜单状态放在这里——与 `combo_box`
//! 同一套做法，不违反「`view` 必须纯计算」那条纪律。
//!
//! 菜单面板本身也是自绘的（不再是 `overlay::menu`）：
//! - `menu::Menu` 的 `width` 默认是 **0**，忘了设置就得到一条零宽的边，
//!   像素上一个点都不变（这就是「右键没反应」的真正原因）；
//! - 它不支持分组分隔线，12 个动作平铺很难扫读；
//! - 选中底色是整行直角，和本应用其余控件的 6px 圆角不是一套。
//!
//! 现在用普通 `button` 组成一列，面板负责底色/描边，分组位置插入发丝线。
//! 点击语义、点外关闭、Esc 关闭都在 overlay 的 `update` 里处理。

use iced::keyboard;
use iced::widget::{button, container, space, text, Column};
use iced::{mouse, Element, Event, Length, Point, Rectangle, Renderer, Size, Theme, Vector};
use iced_core::layout::{Limits, Node};
use iced_core::overlay::{Element as OverlayElement, Overlay};
use iced_core::renderer::Style as RendererStyle;
use iced_core::widget::{tree, Operation, Tree, Widget};
use iced_core::{Clipboard, Layout, Renderer as CoreRenderer, Shell};

use crate::theme::{self, T_CONTROL};
use crate::ui::estimated_width;

/// 菜单自身状态。放在控件的 `Tree` 里，`view` 完全看不到它。
struct MenuState {
    /// 是否正显示着菜单。只有右键按下会把它置真，选中或点空白处置假。
    open: bool,
    /// 菜单锚点（窗口坐标）。右键按下的那一刻记下来，之后不再跟着鼠标走。
    anchor: Point,
    /// 菜单面板自己的控件树。面板每帧重建，但控件状态要跨帧保留。
    menu_tree: Tree,
    /// 当前悬停的行。overlay 在 draw 阶段会重建控件，按钮自己的 `status`
    /// 会在那一刻被重置，所以悬停必须记在菜单状态里、绘制时按它着色。
    hovered: Option<usize>,
}

impl Default for MenuState {
    fn default() -> Self {
        Self {
            open: false,
            anchor: Point::ORIGIN,
            menu_tree: Tree::empty(),
            hovered: None,
        }
    }
}

/// 一个右键菜单：`content` 是被包装的内容，右键点它会弹出 `items`。
pub struct ContextMenu<'a, T, Message>
where
    T: ToString + Clone,
{
    content: Element<'a, Message>,
    /// 选项。按行构造，所以所有权留在这里，`overlay` 阶段再借出去。
    items: Vec<T>,
    /// 在这些下标前插一条分组线（0 与越界值会被忽略）。
    groups: Vec<usize>,
    on_select: Box<dyn Fn(T) -> Message + 'a>,
    /// 右键按下时同时发一条消息，用来「先选中这一行，再看菜单动作」。
    on_open: Option<Message>,
    /// 把菜单交给宿主开成一个**真正的窗口**：控件只上报锚点，自己不画 overlay。
    /// 原生 RDP 画面是父窗口的子窗口，永远画在 Iced 之上，overlay 菜单会被整个
    /// 盖掉，所以会话菜单走这条路。
    popup: Option<Box<dyn Fn(Point) -> Message + 'a>>,
}

impl<'a, T, Message> ContextMenu<'a, T, Message>
where
    T: ToString + Clone,
{
    pub fn new(
        content: impl Into<Element<'a, Message>>,
        items: Vec<T>,
        on_select: impl Fn(T) -> Message + 'a,
    ) -> Self {
        Self {
            content: content.into(),
            items,
            groups: Vec::new(),
            on_select: Box::new(on_select),
            on_open: None,
            popup: None,
        }
    }

    /// 分组：给出每个分组**第一条**的下标，渲染时在这些下标前画分隔线。
    pub fn groups(mut self, groups: Vec<usize>) -> Self {
        self.groups = groups;
        self
    }

    /// 右键按下时先发这条消息（例如把这一行设为选中项）。
    pub fn on_open(mut self, message: Message) -> Self {
        self.on_open = Some(message);
        self
    }

    /// 改用独立窗口弹菜单：右键按下只发这条消息（参数是窗口内锚点），
    /// 面板由宿主画到那个窗口里。见 `popup` 字段说的原因。
    pub fn popup(mut self, f: impl Fn(Point) -> Message + 'a) -> Self {
        self.popup = Some(Box::new(f));
        self
    }

    /// 没有选项时不弹菜单，右键也不吃事件。
    fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

impl<'a, T, Message> Widget<Message, Theme, Renderer> for ContextMenu<'a, T, Message>
where
    T: ToString + Clone + 'a,
    Message: Clone + 'a,
{
    /// 标记这个控件自带状态。少了这两个方法，`Tree` 会按「无状态」建，
    /// `update` 里的 `downcast_mut` 一开菜单就崩（真实运行才炸，单测走不到）。
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<MenuState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(MenuState::default())
    }

    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(&[&self.content]);
    }

    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &Limits) -> Node {
        self.content
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits)
    }

    /// 控件操作（聚焦、滚动、测试选择器）必须继续往下传：包装层吞掉 `operate`
    /// 会让内容里的文本/输入框在 `iced_test` 里完全找不到。
    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        self.content
            .as_widget_mut()
            .operate(&mut tree.children[0], layout, renderer, operation);
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        // 先让内容处理：左键点击、文本输入这些都归它。它吃掉了就不开菜单。
        self.content.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        );

        if shell.is_event_captured() || self.is_empty() {
            return;
        }

        let state = tree.state.downcast_mut::<MenuState>();

        if matches!(
            event,
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right))
        ) && cursor.is_over(layout.bounds())
        {
            // 锚点取**按下那一刻**的鼠标位置，菜单随后钉在那里，
            // 不会因为鼠标移开就跟着飘。
            state.anchor = cursor.position().unwrap_or_default();
            shell.capture_event();

            // 弹窗模式：只把锚点交给宿主，菜单由宿主开成独立窗口。这里不置 `open`，
            // 否则 overlay 与弹窗会同时出现两份菜单。
            if let Some(popup) = &self.popup {
                shell.publish(popup(state.anchor));
                return;
            }

            state.open = true;
            state.hovered = None;
            if let Some(message) = self.on_open.clone() {
                shell.publish(message);
            }
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.content.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &RendererStyle,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.content.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<OverlayElement<'b, Message, Theme, iced::Renderer>> {
        // 菜单没开时必须把子控件的 overlay 转出去：滚动区里每行还挂着 ContextMenu，
        // 标签里还挂着 tooltip。直接返回 None 会把这些子 overlay 全部吞掉——
        // 右键状态打开了，画面里却没有菜单（这是一次真实回归）。
        let state = tree.state.downcast_mut::<MenuState>();
        if !state.open {
            return self.content.as_widget_mut().overlay(
                &mut tree.children[0],
                layout,
                renderer,
                viewport,
                translation,
            );
        }

        let position = state.anchor + translation;
        let hovered_now = state.hovered;

        let content: Element<'b, Message> =
            menu_body(&self.items, &self.groups, hovered_now, &self.on_select);

        let MenuState {
            open,
            menu_tree,
            hovered,
            ..
        } = state;
        menu_tree.diff(content.as_widget());

        Some(OverlayElement::new(Box::new(MenuOverlay {
            content,
            tree: menu_tree,
            position,
            open,
            hovered,
            item_count: self.items.len(),
            groups: &self.groups,
        })))
    }
}

/// 分组线：上下各留 4px，左右各缩进 8px，不画成贯穿整块面板的硬线。
fn group_separator<'a, Message: 'a>() -> Element<'a, Message> {
    container(container(space().width(Length::Fill).height(1.0)).style(theme::divider()))
        .width(Length::Fill)
        .padding([4.0, 8.0])
        .into()
}

/// 行盒高度与整行高度：文字 18px 行盒 + 上下各 6px 内边距。
const ROW_TEXT_H: f32 = 18.0;
const ROW_H: f32 = ROW_TEXT_H + 12.0;
/// 分组线：1px 线 + 上下各 4px。
const SEPARATOR_H: f32 = 9.0;
const PANEL_BORDER: f32 = 1.0;
const PANEL_PAD: f32 = 4.0;

/// 面板尺寸：宽度按最长一项估算（行内左右各 10、面板左右各 4、再加 1px 描边），
/// 高度按固定行高、分隔线与内边距累加。
///
/// overlay 与独立弹窗窗口共用这一份——弹窗得先按这个尺寸开窗口、再画同一套面板，
/// 两处各算各的话窗口会比面板大或小一截。
pub(crate) fn panel_size<T: ToString>(items: &[T], groups: &[usize]) -> Size {
    let width = items
        .iter()
        .map(|item| estimated_width(&item.to_string(), T_CONTROL))
        .fold(0.0_f32, f32::max)
        + 30.0;
    let separators = (1..items.len())
        .filter(|index| groups.contains(index))
        .count();

    Size::new(
        width.clamp(170.0, 340.0),
        items.len() as f32 * ROW_H
            + separators as f32 * SEPARATOR_H
            + 2.0 * PANEL_PAD
            + 2.0 * PANEL_BORDER,
    )
}

/// 菜单面板本体：一列按钮，分组线落在 `groups` 给的那些下标之前。
///
/// `hovered` 只有 overlay 路径需要：overlay 在绘制阶段重新构造控件，按钮自己的
/// `status` 是空的，悬停得从外面喂进来。独立弹窗窗口里按钮是正常控件，传 `None`
/// 用原生悬停态。
pub(crate) fn menu_body<'i, 'a, T, Message>(
    items: &[T],
    groups: &[usize],
    hovered: Option<usize>,
    on_select: &(dyn Fn(T) -> Message + 'i),
) -> Element<'a, Message>
where
    T: ToString + Clone,
    Message: Clone + 'a,
{
    let p = theme::palette();
    let width = panel_size(items, groups).width;
    let mut list = Column::new().width(Length::Fixed(width));

    for (index, item) in items.iter().enumerate() {
        // 分隔线只出现在两个分组之间，不能是第一项之前或最后一项之后。
        if index > 0 && index < items.len() && groups.contains(&index) {
            list = list.push(group_separator());
        }

        let message = on_select(item.clone());
        list = list.push(
            button(
                text(item.to_string())
                    .size(T_CONTROL)
                    .line_height(iced::widget::text::LineHeight::Absolute(iced::Pixels(
                        ROW_TEXT_H,
                    )))
                    .color(p.text),
            )
            .on_press(message)
            .width(Length::Fill)
            .padding([6.0, 10.0])
            .style(theme::menu_item(hovered == Some(index))),
        );
    }

    container(list)
        .padding([PANEL_PAD, PANEL_PAD])
        .width(Length::Fixed(width))
        .style(theme::menu_panel())
        .into()
}

/// 菜单面板的 overlay：布局、绘制、点外关闭都在这里。
///
/// 面板本身是普通控件组成的 `Element`，因此悬停/按压反馈用的是全局的按钮样式，
/// 不需要再手写一遍命中测试。
struct MenuOverlay<'a, Message> {
    content: Element<'a, Message>,
    tree: &'a mut Tree,
    /// 锚点（已含父级 translation）。
    position: Point,
    /// 菜单是否打开。选中、点外、Esc 都把它置假；下一帧 overlay 自然消失。
    open: &'a mut bool,
    hovered: &'a mut Option<usize>,
    item_count: usize,
    groups: &'a [usize],
}

impl<Message> Overlay<Message, Theme, iced::Renderer> for MenuOverlay<'_, Message>
where
    Message: Clone,
{
    fn layout(&mut self, renderer: &iced::Renderer, bounds: Size) -> Node {
        let limits = Limits::new(Size::ZERO, bounds);
        let node = self
            .content
            .as_widget_mut()
            .layout(self.tree, renderer, &limits);
        let size = node.size();

        // 靠近右边缘时向左收、靠近底边时向上翻：菜单永远完整留在窗口里。
        let x = self.position.x.min((bounds.width - size.width).max(0.0));
        let y = if self.position.y + size.height > bounds.height && self.position.y >= size.height {
            self.position.y - size.height
        } else {
            self.position.y
        };

        node.move_to(Point::new(x, y))
    }

    fn draw(
        &self,
        renderer: &mut iced::Renderer,
        theme: &Theme,
        style: &RendererStyle,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
    ) {
        let bounds = layout.bounds();
        CoreRenderer::with_layer(renderer, bounds, |renderer| {
            self.content
                .as_widget()
                .draw(self.tree, renderer, theme, style, layout, cursor, &bounds);
        });
    }

    fn operate(
        &mut self,
        layout: Layout<'_>,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        self.content
            .as_widget_mut()
            .operate(self.tree, layout, renderer, operation);
    }

    fn update(
        &mut self,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
    ) {
        let bounds = layout.bounds();

        let dismiss = match event {
            Event::Mouse(mouse::Event::ButtonPressed(
                mouse::Button::Left | mouse::Button::Right,
            )) => !cursor.is_over(bounds),
            Event::Keyboard(keyboard::Event::KeyPressed { key, .. }) => {
                matches!(key, keyboard::Key::Named(keyboard::key::Named::Escape))
            }
            _ => false,
        };

        if dismiss {
            *self.open = false;
            // 吃掉这次点击：Windows 的上下文菜单在点外关闭时也不把点击穿透给底层。
            shell.capture_event();
            shell.request_redraw();
            return;
        }

        // 悬停命中按固定行高算：行高 = 18px 行盒 + 上下 6px 内边距。
        if matches!(
            event,
            Event::Mouse(mouse::Event::CursorMoved { .. })
                | Event::Window(iced::window::Event::RedrawRequested(_))
        ) {
            let new_hovered = self.hit_test(cursor, layout);
            if *self.hovered != new_hovered {
                *self.hovered = new_hovered;
                shell.request_redraw();
            }
        }

        self.content.as_widget_mut().update(
            self.tree, event, layout, cursor, renderer, clipboard, shell, &bounds,
        );

        // 左键松开后菜单收起：面板里的按钮在松开时发消息，消息已经进了 shell。
        // 右键的松开不能算——那正是把菜单打开的那一次点击。
        if matches!(
            event,
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left))
        ) {
            *self.open = false;
            shell.request_redraw();
        }
    }

    fn mouse_interaction(
        &self,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        let bounds = layout.bounds();
        self.content
            .as_widget()
            .mouse_interaction(self.tree, layout, cursor, &bounds, renderer)
    }
}

impl<Message> MenuOverlay<'_, Message> {
    /// 光标落在第几项。行高固定（18px 行盒 + 12px 上下内边距），
    /// 分隔线占 9px；这样不必遍历面板内部的控件布局。
    fn hit_test(&self, cursor: mouse::Cursor, layout: Layout<'_>) -> Option<usize> {
        let cursor = cursor.position()?;
        let bounds = layout.bounds();

        if !bounds.contains(cursor) {
            return None;
        }

        let mut y = bounds.y + PANEL_BORDER + PANEL_PAD;
        for index in 0..self.item_count {
            if index > 0 && self.groups.contains(&index) {
                y += SEPARATOR_H;
            }
            if cursor.y >= y && cursor.y < y + ROW_H {
                return Some(index);
            }
            y += ROW_H;
        }

        None
    }
}

impl<'a, T, Message> From<ContextMenu<'a, T, Message>> for Element<'a, Message>
where
    T: ToString + Clone + 'a,
    Message: Clone + 'a,
{
    fn from(menu: ContextMenu<'a, T, Message>) -> Self {
        Element::new(menu)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::widget::container;
    use iced::{Fill, Settings};

    #[derive(Debug, Clone, PartialEq)]
    enum Probe {
        Selected(&'static str),
    }

    /// 点中菜单项必须发消息，并且菜单自己关掉。
    ///
    /// 这条用例守的是 overlay 的点击流程：右键打开 → 左键按下/松开落在第一项上。
    /// 只验证「菜单画出来了」不够——按钮的 `on_press` 在松开时触发，如果 overlay
    /// 在按下那一帧就把自己关掉，消息会静默丢失。
    #[test]
    fn clicking_a_menu_item_publishes_its_action() {
        let menu = ContextMenu::new(
            container(space().width(Fill).height(Fill)).style(|_| container::Style::default()),
            vec!["打开", "复制"],
            Probe::Selected,
        )
        .groups(vec![1]);

        let mut sim = iced_test::Simulator::with_size(
            Settings {
                default_font: crate::theme::ui_font(),
                ..Settings::default()
            },
            Size::new(400.0, 300.0),
            menu,
        );

        sim.point_at(Point::new(60.0, 60.0));
        let _ = sim.simulate([Event::Mouse(mouse::Event::ButtonPressed(
            mouse::Button::Right,
        ))]);

        // 面板 padding 4，第一行大约在 y=64..92；点在行中间。
        sim.point_at(Point::new(90.0, 75.0));
        let _ = sim.simulate([
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
        ]);

        let messages: Vec<_> = sim.into_messages().collect();
        assert_eq!(messages, vec![Probe::Selected("打开")]);
    }
}
