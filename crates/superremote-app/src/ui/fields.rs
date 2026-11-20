//! 设置表单的共享控件：继承式下拉、三段式开关、分辨率选项。
//!
//! 连接编辑页（单条覆盖）与设置页（全局默认）用的是同一批控件，只是「空值」
//! 那一档的说法不同：前者叫「继承默认」，后者叫「内置默认」。控件本身不关心
//! 这个差别，标签由调用方传进来。

use iced::widget::{button, column, container, pick_list, row, text};
use iced::{alignment, Element, Fill};
use superremote_core::session::{
    OpenMode, RdpAudio, RdpColorDepth, RdpDesktopSize, RdpGraphics, RdpHostMode, RdpKeyboard,
    RdpPerformance, TerminalBackspace, TerminalCharset, TerminalColorScheme, TerminalCursor,
    TerminalHomeEnd, TerminalKeypad, TerminalMouseMode,
};

use crate::app::Message;
use crate::{i18n, theme};
use superremote_win::display;

/// 下拉选项。`value = None` 是「不覆盖 / 不设默认」，标签里带上当前回退到的值。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice<T> {
    pub value: Option<T>,
    label: String,
}

impl<T> std::fmt::Display for Choice<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.label)
    }
}

/// 把「不覆盖」这一档放在最前面的选项表。
///
/// 不限 `Copy`：字体族名是 `String`，下拉选项只需要把值搬进 `Choice`。
pub fn choices<T>(unset: String, values: impl IntoIterator<Item = (T, String)>) -> Vec<Choice<T>> {
    let mut options = vec![Choice {
        value: None,
        label: unset,
    }];
    options.extend(values.into_iter().map(|(value, label)| Choice {
        value: Some(value),
        label,
    }));
    options
}

/// 标签在左、下拉在右的字段。宽度吃满父容器，同一行的两个字段底边对齐。
pub fn pick_field<'a, T>(
    label: &'static str,
    options: Vec<Choice<T>>,
    selected: Option<T>,
    on_change: impl Fn(Option<T>) -> Message + 'a,
) -> Element<'a, Message>
where
    T: Clone + PartialEq + Eq + 'a,
{
    pick_field_sized(crate::ui::LABEL_W, label, options, selected, on_change)
}

/// 设置页专用的宽标签列版本。
pub fn pick_field_wide<'a, T>(
    label: &'static str,
    options: Vec<Choice<T>>,
    selected: Option<T>,
    on_change: impl Fn(Option<T>) -> Message + 'a,
) -> Element<'a, Message>
where
    T: Clone + PartialEq + Eq + 'a,
{
    pick_field_sized(
        crate::ui::SETTINGS_LABEL_W,
        label,
        options,
        selected,
        on_change,
    )
}

fn pick_field_sized<'a, T>(
    label_width: f32,
    label: &'static str,
    options: Vec<Choice<T>>,
    selected: Option<T>,
    on_change: impl Fn(Option<T>) -> Message + 'a,
) -> Element<'a, Message>
where
    T: Clone + PartialEq + Eq + 'a,
{
    let p = theme::palette();
    // 选中值可能不在选项表里（比如手改配置写了个本版本不认的分辨率），
    // 退回第一项而不是让下拉空着——空下拉在点开之前看不出问题，点开后是空的。
    let selected = options
        .iter()
        .find(|choice| choice.value == selected)
        .cloned()
        .or_else(|| options.first().cloned());

    let Some(selected) = selected else {
        return column![].into();
    };

    row![
        container(
            text(label)
                .size(theme::T_FIELD_LABEL)
                .color(p.text_dim)
                .wrapping(iced::widget::text::Wrapping::None),
        )
        .width(label_width),
        pick_list(options, Some(selected), move |choice| {
            on_change(choice.value)
        })
        .width(Fill)
        // 与输入框同一条 6px 纵向内边距：两者都是 13px 控件，并排时高度一致。
        .padding([6.0, 9.0])
        .text_size(theme::T_CONTROL)
        .style(theme::pick_list_style())
        .menu_style(theme::menu_style()),
    ]
    .spacing(crate::ui::LABEL_GAP)
    .align_y(alignment::Vertical::Center)
    .width(Fill)
    .into()
}

/// 三态复选框：用旧版的勾选框形态表达「继承 / 开 / 关」，点一下循环
/// 继承 → 开 → 关 → 继承。三段横排按钮在表单里既占宽又吵，五个重定向并排时
/// 一屏全是色块；勾选框一行一个，能按旧版那样 3 个一行平铺。
pub fn tri_checkbox<'a>(
    label: &'static str,
    value: Option<bool>,
    edit: impl Fn(Option<bool>) -> Message + 'a,
) -> Element<'a, Message> {
    let p = theme::palette();

    let (glyph, tone) = match value {
        None => ("–", p.text_dim),
        Some(true) => ("✓", p.accent),
        Some(false) => ("", p.text_dim),
    };

    // 点一下按 继承 → 开 → 关 循环；和旧版 IsThreeState 的行为一致。
    let next = match value {
        None => Some(true),
        Some(true) => Some(false),
        Some(false) => None,
    };

    let boxed = container(
        text(glyph)
            .size(10)
            .color(tone)
            .width(Fill)
            .align_x(alignment::Horizontal::Center),
    )
    .width(14.0)
    .height(14.0)
    .align_y(alignment::Vertical::Center)
    .style(theme::checkbox_box(value.is_some()));

    let t = crate::i18n::t();
    let hint = match value {
        None => t.check_inherit,
        Some(true) => t.check_on,
        Some(false) => t.check_off,
    };

    let row = button(
        row![boxed, text(label).size(theme::T_LABEL).color(p.text)]
            .spacing(6)
            .align_y(alignment::Vertical::Center),
    )
    .on_press(edit(next))
    .padding([2.0, 4.0])
    .style(theme::checkbox_row());

    crate::ui::tooltip(row, hint, iced::widget::tooltip::Position::Top)
}

/// 分辨率候选：跟随窗口 / 全屏 / 系统上报的显示模式并入预置档。
///
/// 顺序固定（先两档特殊值，再按宽度降序的尺寸表）：Iced 按位置 diff 下拉项，
/// 选项每帧重排会让选中态错位。
pub fn desktop_choices(unset: String) -> Vec<Choice<RdpDesktopSize>> {
    let t = i18n::t();
    let mut options = vec![Choice {
        value: None,
        label: unset,
    }];
    options.push(Choice {
        value: Some(RdpDesktopSize::FollowWindow),
        label: t.follow_window.to_owned(),
    });
    options.push(Choice {
        value: Some(RdpDesktopSize::Fullscreen),
        label: t.desktop_fullscreen.to_owned(),
    });
    options.extend(
        display::resolutions()
            .iter()
            .map(|&(width, height)| Choice {
                value: Some(RdpDesktopSize::Custom { width, height }),
                label: format!("{width} × {height}"),
            }),
    );
    options
}

/// 打开方式的候选：选项卡 / 后台选项卡 / 独立窗口。
pub fn open_mode_choices(unset: String) -> Vec<Choice<OpenMode>> {
    let t = i18n::t();
    choices(
        unset,
        OpenMode::ALL
            .into_iter()
            .map(|value| (value, open_mode_label(value, t).to_owned())),
    )
}

pub fn open_mode_label(value: OpenMode, t: &i18n::T) -> &'static str {
    match value {
        OpenMode::Tab => t.open_mode_tab,
        OpenMode::Background => t.open_mode_background,
        OpenMode::Detached => t.open_mode_window,
    }
}

/// 终端字体候选：系统枚举到的等宽字体族（[`superremote_win::fonts::monospace_families`]）。
///
/// `saved` 是已经写进配置、但不在列表里的字体名（手改过 `config.toml`，或那个
/// 字体不是等宽）。它必须作为末尾一个独立选项出现，否则下拉会显示成列表第一项，
/// 而实际生效的仍是保存的那个值——界面在说谎。
///
/// 比对用精确相等而不是忽略大小写：下拉的选中态本身就是按字符串相等找的，
/// `consolas` 与 `Consolas` 在这种口径下都得当成「列表里没有」。
pub fn font_choices(unset: String, saved: Option<&str>) -> Vec<Choice<String>> {
    let lang = i18n::lang();
    let mut options = choices(
        unset,
        superremote_win::fonts::monospace_families()
            .iter()
            .map(|family| (family.clone(), family.clone())),
    );

    if let Some(name) = saved.filter(|name| {
        !options
            .iter()
            .any(|choice| choice.value.as_deref() == Some(*name))
    }) {
        options.push(Choice {
            value: Some(name.to_owned()),
            label: lang.font_unlisted(name),
        });
    }

    options
}

pub fn desktop_label(value: RdpDesktopSize, t: &i18n::T) -> String {
    match value {
        RdpDesktopSize::FollowWindow => t.follow_window.to_owned(),
        RdpDesktopSize::Fullscreen => t.desktop_fullscreen.to_owned(),
        RdpDesktopSize::Custom { width, height } => format!("{width} × {height}"),
    }
}

pub fn host_label(value: RdpHostMode, t: &i18n::T) -> &'static str {
    match value {
        RdpHostMode::Shared => t.rdp_host_shared,
        RdpHostMode::Isolated => t.rdp_host_isolated,
    }
}

pub fn graphics_label(value: RdpGraphics, t: &i18n::T) -> &'static str {
    match value {
        RdpGraphics::Compat => t.rdp_graphics_compat,
        RdpGraphics::Modern => t.rdp_graphics_modern,
    }
}

pub fn performance_label(value: RdpPerformance, t: &i18n::T) -> &'static str {
    match value {
        RdpPerformance::Auto => t.perf_auto,
        RdpPerformance::Modem => t.perf_modem,
        RdpPerformance::BroadbandLow => t.perf_broadband_low,
        RdpPerformance::Satellite => t.perf_satellite,
        RdpPerformance::BroadbandHigh => t.perf_broadband_high,
        RdpPerformance::Wan => t.perf_wan,
        RdpPerformance::Lan => t.perf_lan,
    }
}

pub fn color_label(value: RdpColorDepth, t: &i18n::T) -> &'static str {
    match value {
        RdpColorDepth::Bpp15 => t.color_15,
        RdpColorDepth::Bpp16 => t.color_16,
        RdpColorDepth::Bpp24 => t.color_24,
        RdpColorDepth::Bpp32 => t.color_32,
    }
}

pub fn audio_label(value: RdpAudio, t: &i18n::T) -> &'static str {
    match value {
        RdpAudio::Local => t.audio_local,
        RdpAudio::Remote => t.audio_remote,
        RdpAudio::None => t.audio_none,
    }
}

pub fn keyboard_label(value: RdpKeyboard, t: &i18n::T) -> &'static str {
    match value {
        RdpKeyboard::Local => t.keyboard_local,
        RdpKeyboard::Remote => t.keyboard_remote,
        RdpKeyboard::Fullscreen => t.keyboard_fullscreen,
    }
}

pub fn cursor_label(value: TerminalCursor, t: &i18n::T) -> &'static str {
    match value {
        TerminalCursor::Block => t.terminal_cursor_block,
        TerminalCursor::Underline => t.terminal_cursor_underline,
        TerminalCursor::Beam => t.terminal_cursor_beam,
    }
}

pub fn backspace_label(value: TerminalBackspace, t: &i18n::T) -> &'static str {
    match value {
        TerminalBackspace::Delete => t.terminal_backspace_del,
        TerminalBackspace::Backspace => t.terminal_backspace_bs,
    }
}

pub fn home_end_label(value: TerminalHomeEnd, t: &i18n::T) -> &'static str {
    match value {
        TerminalHomeEnd::Standard => t.terminal_home_end_standard,
        TerminalHomeEnd::Rxvt => t.terminal_home_end_rxvt,
    }
}

pub fn scheme_label(value: TerminalColorScheme, t: &i18n::T) -> &'static str {
    match value {
        TerminalColorScheme::Default => t.terminal_scheme_default,
        TerminalColorScheme::SolarizedDark => t.terminal_scheme_solarized_dark,
        TerminalColorScheme::SolarizedLight => t.terminal_scheme_solarized_light,
        TerminalColorScheme::LinuxConsole => t.terminal_scheme_linux,
        TerminalColorScheme::Xterm => t.terminal_scheme_xterm,
    }
}

pub fn charset_label(value: TerminalCharset, t: &i18n::T) -> &'static str {
    match value {
        TerminalCharset::Utf8 => t.charset_utf8,
        TerminalCharset::Gbk => t.charset_gbk,
        TerminalCharset::Big5 => t.charset_big5,
        TerminalCharset::Latin1 => t.charset_latin1,
    }
}

pub fn mouse_mode_label(value: TerminalMouseMode, t: &i18n::T) -> &'static str {
    match value {
        TerminalMouseMode::Hybrid => t.mouse_hybrid,
        TerminalMouseMode::Xterm => t.mouse_xterm,
        TerminalMouseMode::Windows => t.mouse_windows,
    }
}

pub fn keypad_label(value: TerminalKeypad, t: &i18n::T) -> &'static str {
    match value {
        TerminalKeypad::Standard => t.keypad_standard,
        TerminalKeypad::EscN => t.keypad_escn,
        TerminalKeypad::Linux => t.keypad_linux,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 「不覆盖」必须排在第一位，后面才是系统枚举出来的族名。
    #[test]
    fn font_choices_puts_unset_first() {
        let options = font_choices("inherit".to_owned(), None);

        assert!(
            options.first().is_some_and(|choice| choice.value.is_none()),
            "第一项必须是「不覆盖」"
        );
        assert_eq!(
            options.len() - 1,
            superremote_win::fonts::monospace_families().len(),
            "其余项应与枚举结果一一对应"
        );
    }

    /// 配置里存着、但系统列表没有的字体必须仍然出现在末尾。
    ///
    /// 少了这一条，下拉会把「实际生效的字体」显示成列表第一项——用户在
    /// `config.toml` 里写的名字被界面悄悄替换掉。
    #[test]
    fn an_unlisted_saved_font_stays_visible() {
        let options = font_choices("inherit".to_owned(), Some("Not Installed Mono"));

        let last = options.last().expect("字体列表不该为空");
        assert_eq!(last.value.as_deref(), Some("Not Installed Mono"));
        // 只断言标签里带着这个名字：两种语言的措辞不同，且并行用例可能切换语言。
        assert!(
            last.label.contains("Not Installed Mono"),
            "标签要能看出这是哪个字体，实际标签：{}",
            last.label
        );
    }

    /// 已经在列表里的值不能再生成一份，否则选中态指到第一份，末尾多出一份同名项。
    #[test]
    fn a_listed_saved_font_is_not_duplicated() {
        let Some(present) = superremote_win::fonts::monospace_families()
            .first()
            .cloned()
        else {
            return;
        };

        let options = font_choices("inherit".to_owned(), Some(&present));
        let hits = options
            .iter()
            .filter(|choice| choice.value.as_deref() == Some(present.as_str()))
            .count();
        assert_eq!(hits, 1, "{present} 只该出现一次，实际 {hits} 次");
    }
}
