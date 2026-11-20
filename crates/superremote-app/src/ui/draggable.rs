//! 拖拽源/落点包装控件：把「按下 → 移动 → 松开」翻译成拖拽消息。
//!
//! Iced 0.14 没有原生拖放，只能自己跟鼠标事件。这里**只发消息、不改任何状态**，
//! 真正的移动语义（含环检查）在 [`superremote_core::session::SessionTree::move_node`]。
//!
//! 能直接包在 `button` 外面的原因：`button` 在**松开**时才发 `on_press`，而且要求
//! 光标仍在自己身上。拖拽结束时手是松在别的行上的——源行拿不到 click，目标行也没有
//! 按下状态，所以不会误触点击。反过来，一次普通点击（按下松开都在同一行）不受影响。
//!
//! 松手落在行上时本控件会 `capture_event`：这样 `iced::event::listen` 那条全局订阅
//! （只收 `Status::Ignored` 的事件）就不会把同一次松手当成「取消」，一次松手只有一条消息。

use iced::mouse;
use iced::{Element, Event, Length, Point, Rectangle, Renderer, Size, Theme};
use iced_core::layout::{Limits, Node};
use iced_core::widget::tree;
use iced_core::widget::{Operation, Tree, Widget};
use iced_core::{Clipboard, Layout, Shell};

/// 超过这个位移才算拖拽，否则只是一次普通点击。
const DRAG_THRESHOLD: f32 = 5.0;

#[derive(Debug, Default)]
struct DragState {
    /// 这一行上有没有未松开的左键按下。
    pressed: bool,
    /// 按下时的鼠标位置，用来算位移是否超过阈值。
    press_at: Option<Point>,
    /// 已经超过阈值、确认是拖拽而不是点击。
    dragging: bool,
    /// 鼠标当前是否在这一行上，用来只在进/出时报一次。
    hovered: bool,
}

/// 一行可拖拽的内容。
pub struct Draggable<'a, Message> {
    content: Element<'a, Message>,
    /// 左键按下：让上层记下「谁是拖拽源」。
    on_press: Message,
    /// 拖拽过程中的鼠标位置（窗口坐标），让上层画跟手的反馈。
    on_move: Box<dyn Fn(Point) -> Message + 'a>,
    /// 鼠标进/出这一行：`true` 进、`false` 出，让上层维护落点候选。
    /// 用回调而不是固定消息，是因为「哪一行」只有调用方知道。
    on_hover: Box<dyn Fn(bool) -> Message + 'a>,
    /// 松手落在这条行上。
    on_drop: Message,
}

impl<'a, Message> Draggable<'a, Message>
where
    Message: Clone,
{
    pub fn new(
        content: impl Into<Element<'a, Message>>,
        on_press: Message,
        on_move: impl Fn(Point) -> Message + 'a,
        on_hover: impl Fn(bool) -> Message + 'a,
        on_drop: Message,
    ) -> Self {
        Self {
            content: content.into(),
            on_press,
            on_move: Box::new(on_move),
            on_hover: Box::new(on_hover),
            on_drop,
        }
    }
}

impl<'a, Message> Widget<Message, Theme, Renderer> for Draggable<'a, Message>
where
    Message: Clone + 'a,
{
    /// 同 [`ContextMenu`](crate::ui::context_menu::ContextMenu)：必须声明自带状态，
    /// 否则 `Tree` 建成无状态，第一次按下就 `Downcast on stateless state`。
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<DragState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(DragState::default())
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
        // 先让内容处理：点击、hover 这些都归它。
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

        let state = tree.state.downcast_mut::<DragState>();
        let inside = cursor.is_over(layout.bounds());

        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) if inside => {
                state.pressed = true;
                state.dragging = false;
                state.press_at = cursor.position();
                shell.publish(self.on_press.clone());
                // 刻意不 capture：`button` 也要看到按下，否则它的按压反馈不更新。
            }
            Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                // 只在进/出这一行时报一次：落点候选变了才值得打扰上层。
                if inside != state.hovered {
                    state.hovered = inside;
                    shell.publish((self.on_hover)(inside));
                }

                if state.pressed {
                    if let (Some(start), Some(now)) = (state.press_at, cursor.position()) {
                        // `Vector` 没有 `length()`，位移平方比较就够，顺便省一次开方。
                        let dx = now.x - start.x;
                        let dy = now.y - start.y;
                        if !state.dragging && dx * dx + dy * dy > DRAG_THRESHOLD * DRAG_THRESHOLD {
                            state.dragging = true;
                        }
                        if state.dragging {
                            shell.publish((self.on_move)(now));
                        }
                    }
                }
            }
            // 松手落在哪一行，就由哪一行汇报。**不要**要求「本行有过按下」：
            // 拖拽时按下发生在源行、松手发生在目标行，目标行的按下标记是空的。
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                state.pressed = false;
                state.dragging = false;
                state.press_at = None;

                if inside {
                    shell.publish(self.on_drop.clone());
                    // 只有行内松手才截住事件：全局订阅（只收 Ignored）据此区分
                    // 「落在这行」和「落在空白处」，一次松手只会产生一条消息。
                    shell.capture_event();
                }
            }
            _ => {}
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
        style: &iced_core::renderer::Style,
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
}

impl<'a, Message> From<Draggable<'a, Message>> for Element<'a, Message>
where
    Message: Clone + 'a,
{
    fn from(draggable: Draggable<'a, Message>) -> Self {
        Element::new(draggable)
    }
}
