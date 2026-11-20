//! 全应用唯一的按钮组件。
//!
//! 所有**带文字**的按钮都从这里出：同一套内边距、字号、圆角与 1px 描边，
//! 只有颜色随层级变化（主 / 次 / 危险）。视图层不要再直接写
//! `button(text(...))`——那样每处各调一次 padding 和样式，最后必然出现
//! 「有的描边、有的没描边、两个按钮高度差 1px」。
//!
//! 图标按钮（标题栏窗口控件、标签的 ✕、`＋`、导航图标）是另一类：
//! 它们没有文字标签，由 [`icon`] 统一负责「吃满格子 + 双向居中」，
//! 因为 `button` 的布局是 `layout::padded`，内容默认贴在左上角。

use iced::widget::{button, container, text};
use iced::{alignment, Element, Theme};

use crate::app::Message;
use crate::theme::{self, ButtonKind};

/// 动作按钮的统一内边距。三种层级共用，换字号时只改这里。
const PADDING: [f32; 2] = [6.0, 14.0];

/// 主操作：实心强调色。
pub fn primary<'a>(
    label: impl Into<String>,
    on_press: impl Into<Option<Message>>,
) -> Element<'a, Message> {
    action(label, ButtonKind::Primary, on_press)
}

/// 次要操作：表面底 + 发丝描边。`on_press = None` 时自动进入禁用态，
/// 控件类型与位置不变（不要用 `space()` 顶替）。
pub fn secondary<'a>(
    label: impl Into<String>,
    on_press: impl Into<Option<Message>>,
) -> Element<'a, Message> {
    action(label, ButtonKind::Secondary, on_press)
}

/// 危险操作：危险色文字 + 淡红描边。
pub fn danger<'a>(
    label: impl Into<String>,
    on_press: impl Into<Option<Message>>,
) -> Element<'a, Message> {
    action(label, ButtonKind::Danger, on_press)
}

fn action<'a>(
    label: impl Into<String>,
    kind: ButtonKind,
    on_press: impl Into<Option<Message>>,
) -> Element<'a, Message> {
    // 与输入框共用 13px：并排的「口令框 + 清除」不再一边高一边低。
    button(text(label.into()).size(theme::T_CONTROL))
        .on_press_maybe(on_press.into())
        .padding(PADDING)
        .style(theme::action_button(kind))
        .into()
}

/// 图标按钮：内容吃满 `width × height` 的格子并双向居中。
///
/// `style` 决定悬停/选中的颜色（标题栏、标签、导航各有各的语义色），
/// 但几何与居中逻辑只在这里写一次。
pub fn icon<'a>(
    content: impl Into<Element<'a, Message>>,
    width: f32,
    height: f32,
    on_press: Option<Message>,
    style: impl Fn(&Theme, button::Status) -> button::Style + 'a,
) -> Element<'a, Message> {
    button(
        container(content)
            .width(width)
            .height(height)
            .align_x(alignment::Horizontal::Center)
            .align_y(alignment::Vertical::Center),
    )
    .on_press_maybe(on_press)
    .width(width)
    .height(height)
    .padding(0.0)
    .style(style)
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 三种文字按钮必须共用同一条描边与圆角；只有颜色随层级变化。
    /// 之前每页各拼一个 `button(...)`，就是在这里走样的。
    #[test]
    fn every_action_button_shares_the_same_geometry() {
        let theme = theme::app_theme();
        for kind in [
            ButtonKind::Primary,
            ButtonKind::Secondary,
            ButtonKind::Danger,
        ] {
            for status in [
                button::Status::Active,
                button::Status::Hovered,
                button::Status::Pressed,
                button::Status::Disabled,
            ] {
                let style = theme::action_button(kind)(&theme, status);
                assert_eq!(style.border.width, 1.0, "{kind:?} 少了 1px 描边");
                assert_eq!(
                    style.border.radius,
                    theme::RADIUS_SM.into(),
                    "{kind:?} 的圆角与其它按钮不一致"
                );
            }
        }
    }
}
