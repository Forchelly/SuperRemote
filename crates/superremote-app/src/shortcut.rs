//! 界面级快捷键：把按键事件翻译成动作。
//!
//! 只认**不会与终端抢输入**的组合。终端的键盘表在 [`crate::terminal::encode_key`]，
//! 那里 `Ctrl+W` 是「删掉前一个词」、`Ctrl+A` 是行首，属于 shell 的日常操作，
//! 界面不能截走。所以这里挑的都是终端不消费、或本来就没意义的组合：
//!
//! | 组合 | 动作 |
//! |------|------|
//! | `Ctrl+Tab` / `Ctrl+Shift+Tab` | 下一个 / 上一个标签（环绕） |
//! | `Ctrl+1` … `Ctrl+8` | 跳到第 N 个标签 |
//! | `Ctrl+9` | 跳到最后一个标签 |
//! | `Ctrl+Shift+W` | 关闭当前标签 |
//!
//! 认领不到的一律交回给终端，所以「这里没有的动作」等于「终端照常收到这个键」。

use iced::keyboard::{key, Event, Key, Modifiers};

/// `Ctrl+9` 的语义：最后一个标签。与浏览器的老规矩一致，标签多于 9 个时也能
/// 一步跳到末尾。
pub const LAST_TAB_KEY: usize = 9;

/// 界面动作。只描述「要做什么」，不碰任何状态，方便单测。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shortcut {
    /// `Ctrl+Tab`
    NextTab,
    /// `Ctrl+Shift+Tab`
    PrevTab,
    /// `Ctrl+1`…`Ctrl+9`，取值是**按键上的数字**（1 起），不是下标。
    ActivateTab(usize),
    /// `Ctrl+Shift+W`
    CloseTab,
}

/// 树 / 连接列表页的导航键。
///
/// 与 `Shortcut` 同一层只翻译不碰状态。认领范围由调用方决定：只有画面里
/// **没有终端**的页面（主页、连接列表）才轮到它，终端页里的裸方向键属于远端。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Nav {
    /// `↑`
    Up,
    /// `↓`
    Down,
    /// `Enter`：树上的连接 = 打开、目录 = 展开折叠；列表页 = 打开
    Open,
    /// `→`：展开目录
    Expand,
    /// `←`：折叠目录
    Collapse,
    /// `F2`
    Rename,
    /// `Delete`
    Delete,
}

/// 解析一次按键。返回 `None` 表示「界面不认领」，调用方交给终端。
pub fn resolve(event: &Event) -> Option<Shortcut> {
    let Event::KeyPressed { key, modifiers, .. } = event else {
        return None;
    };

    if !modifiers.control() {
        return None;
    }

    match key {
        Key::Named(key::Named::Tab) => Some(if modifiers.shift() {
            Shortcut::PrevTab
        } else {
            Shortcut::NextTab
        }),
        Key::Character(value) => resolve_character(value, *modifiers),
        _ => None,
    }
}

/// 解析一次**不带修饰键**的按键是否属于树/列表导航。
///
/// 带修饰键的组合一律不认领：`Ctrl` 系已经归 [`resolve`]，`Shift`/`Alt` 系
/// （终端里的转义用法、未来的自定义快捷键）不该被导航抢走。
pub fn resolve_nav(event: &Event) -> Option<Nav> {
    let Event::KeyPressed {
        key: Key::Named(named),
        modifiers,
        ..
    } = event
    else {
        return None;
    };

    if !modifiers.is_empty() {
        return None;
    }

    match named {
        key::Named::ArrowUp => Some(Nav::Up),
        key::Named::ArrowDown => Some(Nav::Down),
        key::Named::Enter => Some(Nav::Open),
        key::Named::ArrowRight => Some(Nav::Expand),
        key::Named::ArrowLeft => Some(Nav::Collapse),
        key::Named::F2 => Some(Nav::Rename),
        key::Named::Delete => Some(Nav::Delete),
        _ => None,
    }
}

fn resolve_character(value: &str, modifiers: Modifiers) -> Option<Shortcut> {
    // `Ctrl+Shift+W` 交给系统时 `value` 是大写，`Ctrl+W` 是小写；
    // 小写化之后再区分有没有 Shift，才能把两者稳定分开。
    let lowered = value.to_lowercase();

    if lowered == "w" {
        return modifiers.shift().then_some(Shortcut::CloseTab);
    }

    // 带 Shift 的数字是符号（`!@#`），不参与跳转。
    if modifiers.shift() {
        return None;
    }

    lowered
        .parse::<usize>()
        .ok()
        .filter(|digit| (1..=LAST_TAB_KEY).contains(digit))
        .map(Shortcut::ActivateTab)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(key: Key, modifiers: Modifiers) -> Event {
        Event::KeyPressed {
            key: key.clone(),
            modified_key: key,
            physical_key: key::Physical::Code(key::Code::KeyA),
            location: iced::keyboard::Location::Standard,
            modifiers,
            text: None,
            repeat: false,
        }
    }

    #[test]
    fn tab_chords_cycle_in_both_directions() {
        let tab = Key::Named(key::Named::Tab);

        assert_eq!(
            resolve(&press(tab.clone(), Modifiers::CTRL)),
            Some(Shortcut::NextTab)
        );
        assert_eq!(
            resolve(&press(tab, Modifiers::CTRL | Modifiers::SHIFT)),
            Some(Shortcut::PrevTab)
        );
    }

    #[test]
    fn digits_jump_to_the_nth_tab() {
        for digit in 1..=LAST_TAB_KEY {
            assert_eq!(
                resolve(&press(
                    Key::Character(digit.to_string().into()),
                    Modifiers::CTRL
                )),
                Some(Shortcut::ActivateTab(digit)),
                "Ctrl+{digit} 应跳到第 {digit} 个标签"
            );
        }
    }

    #[test]
    fn close_needs_the_shift_variant_so_the_shell_keeps_ctrl_w() {
        assert_eq!(
            resolve(&press(
                Key::Character("W".into()),
                Modifiers::CTRL | Modifiers::SHIFT
            )),
            Some(Shortcut::CloseTab)
        );
        // 裸 Ctrl+W 是终端的「删掉前一个词」，界面一律不认领。
        assert_eq!(
            resolve(&press(Key::Character("w".into()), Modifiers::CTRL)),
            None
        );
    }

    #[test]
    fn shell_editing_keys_are_left_to_the_terminal() {
        for letter in ["a", "e", "k", "u", "c", "d", "z", "l"] {
            assert_eq!(
                resolve(&press(Key::Character(letter.into()), Modifiers::CTRL)),
                None,
                "Ctrl+{letter} 属于 shell，界面不能截走"
            );
        }
    }

    #[test]
    fn keys_without_control_or_out_of_range_are_ignored() {
        assert_eq!(
            resolve(&press(Key::Character("1".into()), Modifiers::NONE)),
            None
        );
        assert_eq!(
            resolve(&press(Key::Character("0".into()), Modifiers::CTRL)),
            None,
            "就绪的标签编号从 1 开始"
        );
        // 带 Shift 的数字是符号，不是标签编号。
        assert_eq!(
            resolve(&press(
                Key::Character("!".into()),
                Modifiers::CTRL | Modifiers::SHIFT
            )),
            None
        );
        assert_eq!(
            resolve(&Event::KeyReleased {
                key: Key::Character("1".into()),
                modified_key: Key::Character("1".into()),
                physical_key: key::Physical::Code(key::Code::Digit1),
                location: iced::keyboard::Location::Standard,
                modifiers: Modifiers::CTRL,
            }),
            None,
            "抬起事件不该再触发动作"
        );
    }

    #[test]
    fn bare_arrows_enter_f2_and_delete_navigate_the_tree() {
        for (named, nav) in [
            (key::Named::ArrowUp, Nav::Up),
            (key::Named::ArrowDown, Nav::Down),
            (key::Named::Enter, Nav::Open),
            (key::Named::ArrowRight, Nav::Expand),
            (key::Named::ArrowLeft, Nav::Collapse),
            (key::Named::F2, Nav::Rename),
            (key::Named::Delete, Nav::Delete),
        ] {
            assert_eq!(
                resolve_nav(&press(Key::Named(named), Modifiers::NONE)),
                Some(nav),
                "裸 {named:?} 应翻译为 {nav:?}"
            );
        }
    }

    #[test]
    fn modified_keys_and_releases_never_navigate() {
        for modifiers in [
            Modifiers::CTRL,
            Modifiers::SHIFT,
            Modifiers::ALT,
            Modifiers::CTRL | Modifiers::SHIFT,
        ] {
            assert_eq!(
                resolve_nav(&press(Key::Named(key::Named::ArrowUp), modifiers)),
                None,
                "带修饰键的方向键属于终端或全局快捷键，不是导航"
            );
        }

        // 普通字母、松开事件都不属于导航。
        assert_eq!(
            resolve_nav(&press(Key::Character("j".into()), Modifiers::NONE)),
            None
        );
        assert_eq!(
            resolve_nav(&Event::KeyReleased {
                key: Key::Named(key::Named::Enter),
                modified_key: Key::Named(key::Named::Enter),
                physical_key: key::Physical::Code(key::Code::Enter),
                location: iced::keyboard::Location::Standard,
                modifiers: Modifiers::NONE,
            }),
            None,
            "抬起事件不该再触发动作"
        );
    }
}
