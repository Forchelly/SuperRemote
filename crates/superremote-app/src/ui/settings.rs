//! 设置页：外观、语言、连接默认、数据目录与凭据加密。
//!
//! 每张卡片都是「标题 / 说明 / 分隔线 / 内容」四段，横向吃满内容列。
//! 选择类控件（主题、语言）用分段按钮而不是下拉框：选项只有两三个，
//! 摊开比藏进菜单少一次点击。
//!
//! 「连接默认」两张卡与编辑器里的设置卡共用 `ui::fields` 的控件，只有「不设」
//! 那一档的说法不同：那边是「继承默认」，这里是「内置默认」。

use iced::widget::{button, column, container, row, scrollable, space, text};
use iced::{alignment, Element, Fill, Length};
use superremote_core::session::{
    RdpAudio, RdpColorDepth, RdpKeyboard, RdpPerformance, TerminalBackspace, TerminalCharset,
    TerminalColorScheme, TerminalCursor, TerminalHomeEnd, TerminalKeypad, TerminalMouseMode,
};
use superremote_store::{DataPaths, KeyMode};

use crate::app::{App, Message, RdpEdit, RdpRedirect, TerminalEdit};
use crate::i18n::{self, Lang};
use crate::theme::{self, Mode};
use crate::ui;
use crate::ui::fields;

/// 设置页的内容列比其它页宽：两列并排后每列 ~430，再窄就轮到下拉里的
/// 「内置默认（……）」被裁了。1180 的窗口扣掉侧栏后正好放得下。
const SETTINGS_MAX: f32 = 900.0;
/// 宽屏下的上限：左右两列各宽一档，长取值不再裁字。
const SETTINGS_WIDE_MAX: f32 = 1280.0;

pub fn view(app: &App, available: f32) -> Element<'_, Message> {
    let t = i18n::t();
    let wide = available >= ui::WIDE_MIN;

    // 宽屏用两列并**左对齐**：内容从侧栏右侧 16px 起排，右列吃满剩余宽度。
    // 窄屏（默认 1180 窗口）单列铺开——两列时右列的「Built-in · …」会被裁，
    // 英文标签也会折行，这正是切语言后整页高度跳动的原因。
    let max = if wide {
        (available - 32.0).clamp(320.0, SETTINGS_WIDE_MAX)
    } else {
        (available - 32.0).clamp(320.0, SETTINGS_MAX)
    };

    let left_cards = column![
        appearance_card(),
        language_card(),
        open_mode_defaults_card(app),
        data_dir_card(app),
        security_card(app),
    ]
    .spacing(12)
    .width(Fill);

    let right_cards = column![rdp_defaults_card(app), terminal_defaults_card(app)]
        .spacing(12)
        .width(Fill);

    let cards: Element<'_, Message> = if wide {
        row![
            container(left_cards).width(340.0),
            container(right_cards).width(Fill),
        ]
        .spacing(16)
        .width(Fill)
        .into()
    } else {
        column![left_cards, right_cards]
            .spacing(12)
            .width(Fill)
            .into()
    };

    container(
        column![
            // 页头右侧留空：连接数与凭据数的统计已经由侧栏页脚常驻，
            // 两处同时出现在同一屏里只是在重复自己。
            ui::page_header(
                t.settings_title,
                t.settings_subtitle,
                space().width(0.0),
                max
            ),
            ui::divider(),
            // 卡片的外边缘与页头标题都落在 16 的侧边距上（`page_header` 用 16）。
            //
            // `max_width` 与 `padding` 必须分在两层：同一层时内边距会吃掉
            // 上限的一部分，内容列实际只有 536 宽，右边缘就和页头差 24。
            // 两列：左边是「应用级」设置（外观/语言/打开方式/数据目录/凭据），
            // 右边是两张「连接默认」。设置页要一屏放下，靠的是把内容并排而不是
            // 把控件压扁——单列即便去掉全部卡片留白也还差三分之一屏。
            scrollable(
                container(container(cards).width(Fill).max_width(max),)
                    .width(Fill)
                    // 宽屏上内容列有上限：容器默认居中，不显式左对齐时整块会
                    // 往右缩，和页头的 16px 基线错开。
                    .align_x(alignment::Horizontal::Left)
                    .padding([10.0, 16.0]),
            )
            .width(Fill)
            .height(Fill),
        ]
        .spacing(0)
        .width(Fill)
        .height(Fill),
    )
    .width(Fill)
    .height(Fill)
    .into()
}

/// 外观：三选一，当前策略高亮。
///
/// 三个按钮的位置固定、只在样式上区分选中，不做「选中项换个控件重画」——
/// Iced 按子元素位置 diff，形状一变整行的状态就会串位。
fn appearance_card() -> Element<'static, Message> {
    let t = i18n::t();
    let current = theme::current_mode();

    card(
        t.appearance_title,
        t.appearance_hint,
        container(
            row![
                segment(
                    t.theme_system,
                    Mode::System == current,
                    Message::SetTheme(Mode::System)
                ),
                segment(
                    t.theme_light,
                    Mode::Light == current,
                    Message::SetTheme(Mode::Light)
                ),
                segment(
                    t.theme_dark,
                    Mode::Dark == current,
                    Message::SetTheme(Mode::Dark)
                ),
            ]
            .spacing(4)
            .width(Fill),
        )
        .width(Fill)
        .padding(3.0)
        .style(theme::segmented_track()),
    )
}

fn language_card() -> Element<'static, Message> {
    let t = i18n::t();
    let current = i18n::lang();

    card(
        t.language_title,
        t.language_hint,
        container(
            row![
                segment(
                    Lang::Zh.label(),
                    Lang::Zh == current,
                    Message::SetLang(Lang::Zh)
                ),
                segment(
                    Lang::En.label(),
                    Lang::En == current,
                    Message::SetLang(Lang::En)
                ),
            ]
            .spacing(4)
            .width(Fill),
        )
        .width(Fill)
        .padding(3.0)
        .style(theme::segmented_track()),
    )
}

/// 分段控件里的一段：吃满剩余宽度，等宽排布。
fn segment(label: &'static str, active: bool, on_press: Message) -> Element<'static, Message> {
    button(
        text(label)
            .size(theme::T_CONTROL)
            .width(Fill)
            .align_x(alignment::Horizontal::Center),
    )
    .on_press(on_press)
    .width(Fill)
    .padding([5.0, 0.0])
    .style(theme::nav_segment(active))
    .into()
}

fn data_dir_card(app: &App) -> Element<'_, Message> {
    let p = theme::palette();
    let t = i18n::t();
    let path = app.paths.dir().display().to_string();

    let env_locked = DataPaths::env_override().is_some();
    let is_default =
        DataPaths::portable_dir().is_some_and(|default| default.dir() == app.paths.dir());

    let change = ui::button::secondary(
        t.change_dir,
        (!env_locked).then_some(Message::ChooseDataDir),
    );
    let change = if env_locked {
        ui::tooltip(
            change,
            t.data_dir_env_locked,
            iced::widget::tooltip::Position::Top,
        )
    } else {
        change
    };

    let reset = ui::button::secondary(
        t.use_default_dir,
        (!env_locked && !is_default).then_some(Message::ResetDataDir),
    );
    let reset = if env_locked {
        ui::tooltip(
            reset,
            t.data_dir_env_locked,
            iced::widget::tooltip::Position::Top,
        )
    } else {
        reset
    };

    card(
        t.data_dir_title,
        t.data_dir_hint,
        column![
            container(text(path).size(11).color(p.text_dim),)
                .width(Fill)
                .padding([7.0, 9.0])
                .style(theme::info_block()),
            row![
                space().width(Fill),
                ui::button::secondary(t.reveal_dir, Some(Message::RevealDataDir)),
                change,
                reset,
            ]
            .spacing(8)
            .width(Fill)
            .align_y(alignment::Vertical::Center),
        ]
        .spacing(8)
        .width(Fill),
    )
}

fn security_card(app: &App) -> Element<'_, Message> {
    let p = theme::palette();
    let t = i18n::t();
    let protected = app.key_mode() == KeyMode::Password;

    let explanation = if protected {
        t.security_hint_password
    } else {
        t.security_hint_os
    };

    let mode_line = row![
        ui::pill(
            if protected {
                t.mode_master_password
            } else {
                t.mode_os_keychain
            },
            if protected { p.accent } else { p.text_dim },
        ),
        space().width(Fill),
        text(if protected {
            t.startup_needs_password
        } else {
            t.startup_no_password
        })
        .size(10)
        .color(p.text_dim),
    ]
    .spacing(8)
    .align_y(alignment::Vertical::Center);

    // 「取消主口令」只在已经启用时才有意义。这里必须保持子项数量固定
    // （Iced 按子元素位置做 diff），所以用同一枚按钮、只切 `on_press`：
    // 没有 on_press 的 button 自带 Disabled 态，视觉自动退灰，位置不动。
    let cancel = ui::button::secondary(
        t.disable_password,
        protected.then_some(Message::DisableMasterPassword),
    );

    card(
        t.security_title,
        explanation,
        column![
            mode_line,
            ui::field_secure_wide(
                t.field_new_password,
                &app.master_password_draft,
                t.placeholder_min_password,
                true,
                |value| { Message::MasterPasswordChanged(value) }
            ),
            row![
                ui::button::primary(
                    if protected {
                        t.change_password
                    } else {
                        t.enable_password
                    },
                    Some(Message::EnableMasterPassword),
                ),
                cancel,
                space().width(Fill),
            ]
            .spacing(8)
            .align_y(alignment::Vertical::Center),
        ]
        .spacing(10)
        .width(Fill),
    )
}

/// 系统级 RDP 优化检测。只读，普通权限即可。正常时只留一行暗色结论，
/// 让用户能确认检测确实跑过；命中非默认值（AVC 4:4:4 / 仅 UDP）才展开
/// 警告与修复按钮。点「修复系统设置」才请求提权，不点就什么都不发生。
fn tuning_section(app: &App) -> Element<'_, Message> {
    let t = i18n::t();
    let p = theme::palette();

    let detection: Element<'_, Message> = if app.rdp_tuning.needs_fix() {
        column![
            text(t.rdp_tuning_title).size(12).color(p.text),
            text(t.rdp_tuning_hint).size(11).color(p.text_dim),
            row![
                ui::button::secondary(t.rdp_tuning_fix, Some(Message::FixRdpTuning)),
                space().width(Fill),
            ]
            .spacing(8),
        ]
        .spacing(6)
        .into()
    } else {
        text(t.rdp_tuning_ok).size(10).color(p.text_dim).into()
    };

    // 这是系统安全策略，不作为 RDP 卡顿修复；只给显式按钮，默认保持原样
    let secure_on = app.rdp_tuning.secure_desktop();
    let (state, label, action) = if secure_on {
        (
            t.uac_desktop_on,
            t.uac_desktop_disable,
            Message::SetSecureDesktop(false),
        )
    } else {
        (
            t.uac_desktop_off,
            t.uac_desktop_enable,
            Message::SetSecureDesktop(true),
        )
    };

    let mut uac = column![
        text(t.uac_desktop_title).size(12).color(p.text),
        text(state).size(11).color(p.text_dim),
    ]
    .spacing(6);
    if secure_on {
        uac = uac.push(text(t.uac_desktop_warn).size(10).color(p.text_dim));
    }
    uac = uac.push(
        row![
            ui::button::secondary(label, Some(action)),
            space().width(Fill),
        ]
        .spacing(8),
    );

    column![detection, uac].spacing(12).into()
}

/// 远程桌面的全局默认。改这里等于改所有「继承默认」的连接，
/// 已经打开的连接不受影响（它们用的是打开那一刻的快照）。
fn rdp_defaults_card(app: &App) -> Element<'_, Message> {
    let t = i18n::t();
    let lang = i18n::lang();
    let p = theme::palette();

    // 存的是「用户设过的默认值」，可能整份为空；标签里显示的回退值要再解一次，
    // 否则每一项都写着「内置默认」而看不出内置到底是什么。
    let stored = app.rdp_defaults();
    let resolved = superremote_core::session::RdpSettings::default().resolve(&stored);

    let performance = fields::pick_field_wide(
        t.rdp_performance,
        fields::choices(
            lang.builtin_with(fields::performance_label(resolved.performance, t)),
            RdpPerformance::ALL
                .into_iter()
                .map(|value| (value, fields::performance_label(value, t).to_owned())),
        ),
        stored.performance,
        |value| Message::RdpDefaultEdit(RdpEdit::Performance(value)),
    );

    let color = fields::pick_field_wide(
        t.rdp_color_depth,
        fields::choices(
            lang.builtin_with(fields::color_label(resolved.color_depth, t)),
            RdpColorDepth::ALL
                .into_iter()
                .map(|value| (value, fields::color_label(value, t).to_owned())),
        ),
        stored.color_depth,
        |value| Message::RdpDefaultEdit(RdpEdit::ColorDepth(value)),
    );

    let audio = fields::pick_field_wide(
        t.rdp_audio,
        fields::choices(
            lang.builtin_with(fields::audio_label(resolved.audio, t)),
            RdpAudio::ALL
                .into_iter()
                .map(|value| (value, fields::audio_label(value, t).to_owned())),
        ),
        stored.audio,
        |value| Message::RdpDefaultEdit(RdpEdit::Audio(value)),
    );

    // 录音与详情页保持一致用下拉（内置 / 开 / 关）：一行两个控件宽度相同，
    // 勾选框塞在这一行末尾时会被长取值挤到卡片边缘。
    let audio_capture = fields::pick_field_wide(
        t.rdp_audio_capture,
        fields::choices(
            lang.builtin_with(if resolved.audio_capture {
                t.toggle_on
            } else {
                t.toggle_off
            }),
            [
                (true, t.toggle_on.to_owned()),
                (false, t.toggle_off.to_owned()),
            ],
        ),
        stored.audio_capture,
        |value| Message::RdpDefaultEdit(RdpEdit::AudioCapture(value)),
    );

    let keyboard = fields::pick_field_wide(
        t.rdp_keyboard,
        fields::choices(
            lang.builtin_with(fields::keyboard_label(resolved.keyboard, t)),
            RdpKeyboard::ALL
                .into_iter()
                .map(|value| (value, fields::keyboard_label(value, t).to_owned())),
        ),
        stored.keyboard,
        |value| Message::RdpDefaultEdit(RdpEdit::Keyboard(value)),
    );

    let desktop = fields::pick_field_wide(
        t.rdp_desktop_size,
        fields::desktop_choices(
            lang.builtin_with(&fields::desktop_label(resolved.desktop_size, t)),
        ),
        stored.desktop_size,
        |value| Message::RdpDefaultEdit(RdpEdit::DesktopSize(value)),
    );

    let redirect = |label, field, value| {
        fields::tri_checkbox(label, value, move |value| {
            Message::RdpDefaultEdit(RdpEdit::Redirect { field, value })
        })
    };

    card(
        t.defaults_rdp_title,
        t.defaults_rdp_hint,
        column![
            row![performance, color].spacing(12),
            row![audio, audio_capture].spacing(12),
            row![keyboard, desktop].spacing(12),
            ui::section(t.rdp_redirects),
            row![
                redirect(
                    t.rdp_redirect_printers,
                    RdpRedirect::Printers,
                    stored.redirect_printers
                ),
                redirect(
                    t.rdp_redirect_clipboard,
                    RdpRedirect::Clipboard,
                    stored.redirect_clipboard
                ),
            ]
            .spacing(12),
            row![
                redirect(
                    t.rdp_redirect_smartcards,
                    RdpRedirect::Smartcards,
                    stored.redirect_smartcards
                ),
                redirect(
                    t.rdp_redirect_ports,
                    RdpRedirect::Ports,
                    stored.redirect_ports
                ),
            ]
            .spacing(12),
            row![
                redirect(
                    t.rdp_redirect_drives,
                    RdpRedirect::Drives,
                    stored.redirect_drives
                ),
                container(space()).width(Fill),
            ]
            .spacing(12),
            ui::divider(),
            tuning_section(app),
            row![
                ui::button::secondary(t.defaults_restore, Some(Message::ResetRdpDefaults)),
                space().width(Fill),
                text(t.rdp_reconnect_hint).size(10).color(p.text_dim),
            ]
            .spacing(8)
            .align_y(alignment::Vertical::Center),
        ]
        .spacing(12)
        .width(Fill),
    )
}

/// 打开方式的全局默认。三种协议共用，所以单独一张卡而不是塞进 RDP/终端卡里。
fn open_mode_defaults_card(app: &App) -> Element<'_, Message> {
    let t = i18n::t();
    let lang = i18n::lang();

    let stored = app.default_open_mode();
    let resolved = stored.unwrap_or_default();

    card(
        t.defaults_open_title,
        t.defaults_open_hint,
        row![
            container(fields::pick_field_wide(
                t.open_mode,
                fields::open_mode_choices(lang.builtin_with(fields::open_mode_label(resolved, t))),
                stored,
                Message::OpenModeDefaultEdit,
            ))
            .width(260.0),
            container(space()).width(Fill),
        ]
        .spacing(12)
        .align_y(alignment::Vertical::Bottom),
    )
}

/// 终端的全局默认。与远程桌面默认卡同一套控件，只是字段不同。
fn terminal_defaults_card(app: &App) -> Element<'_, Message> {
    let t = i18n::t();
    let lang = i18n::lang();
    let p = theme::palette();

    let stored = app.terminal_defaults();
    let resolved = superremote_core::session::TerminalSettings::default().resolve(&stored);

    let font_family = fields::pick_field_wide(
        t.terminal_font_family,
        fields::font_choices(
            lang.builtin_with(&resolved.font_family),
            stored.font_family.as_deref(),
        ),
        stored.font_family.clone(),
        |value| Message::TerminalDefaultEdit(TerminalEdit::FontFamily(value)),
    );

    let font_size = fields::pick_field_wide(
        t.terminal_font_size,
        fields::choices(
            lang.builtin_with(&format!("{} pt", resolved.font_size)),
            (9u8..=20).map(|size| (size, format!("{size} pt"))),
        ),
        stored.font_size,
        |value| Message::TerminalDefaultEdit(TerminalEdit::FontSize(value)),
    );

    let cursor = fields::pick_field_wide(
        t.terminal_cursor,
        fields::choices(
            lang.builtin_with(fields::cursor_label(resolved.cursor, t)),
            TerminalCursor::ALL
                .into_iter()
                .map(|value| (value, fields::cursor_label(value, t).to_owned())),
        ),
        stored.cursor,
        |value| Message::TerminalDefaultEdit(TerminalEdit::Cursor(value)),
    );

    let backspace = fields::pick_field_wide(
        t.terminal_backspace,
        fields::choices(
            lang.builtin_with(fields::backspace_label(resolved.backspace, t)),
            TerminalBackspace::ALL
                .into_iter()
                .map(|value| (value, fields::backspace_label(value, t).to_owned())),
        ),
        stored.backspace,
        |value| Message::TerminalDefaultEdit(TerminalEdit::Backspace(value)),
    );

    let home_end = fields::pick_field_wide(
        t.terminal_home_end,
        fields::choices(
            lang.builtin_with(fields::home_end_label(resolved.home_end, t)),
            TerminalHomeEnd::ALL
                .into_iter()
                .map(|value| (value, fields::home_end_label(value, t).to_owned())),
        ),
        stored.home_end,
        |value| Message::TerminalDefaultEdit(TerminalEdit::HomeEnd(value)),
    );

    let color_scheme = fields::pick_field_wide(
        t.terminal_color_scheme,
        fields::choices(
            lang.builtin_with(fields::scheme_label(resolved.color_scheme, t)),
            TerminalColorScheme::ALL
                .into_iter()
                .map(|value| (value, fields::scheme_label(value, t).to_owned())),
        ),
        stored.color_scheme,
        |value| Message::TerminalDefaultEdit(TerminalEdit::ColorScheme(value)),
    );

    let charset = fields::pick_field_wide(
        t.terminal_charset,
        fields::choices(
            lang.builtin_with(fields::charset_label(resolved.charset, t)),
            TerminalCharset::ALL
                .into_iter()
                .map(|value| (value, fields::charset_label(value, t).to_owned())),
        ),
        stored.charset,
        |value| Message::TerminalDefaultEdit(TerminalEdit::Charset(value)),
    );

    let mouse = fields::pick_field_wide(
        t.terminal_mouse,
        fields::choices(
            lang.builtin_with(fields::mouse_mode_label(resolved.mouse_mode, t)),
            TerminalMouseMode::ALL
                .into_iter()
                .map(|value| (value, fields::mouse_mode_label(value, t).to_owned())),
        ),
        stored.mouse_mode,
        |value| Message::TerminalDefaultEdit(TerminalEdit::MouseMode(value)),
    );

    let keypad = fields::pick_field_wide(
        t.terminal_keypad,
        fields::choices(
            lang.builtin_with(fields::keypad_label(resolved.keypad, t)),
            TerminalKeypad::ALL
                .into_iter()
                .map(|value| (value, fields::keypad_label(value, t).to_owned())),
        ),
        stored.keypad,
        |value| Message::TerminalDefaultEdit(TerminalEdit::Keypad(value)),
    );

    card(
        t.defaults_terminal_title,
        t.defaults_terminal_hint,
        column![
            row![font_family, container(font_size).width(210.0),]
                .spacing(12)
                .align_y(alignment::Vertical::Bottom),
            row![cursor, color_scheme].spacing(12),
            row![backspace, home_end].spacing(12),
            row![charset, mouse].spacing(12),
            row![keypad, container(space()).width(Fill)].spacing(12),
            ui::divider(),
            row![
                ui::button::secondary(t.defaults_restore, Some(Message::ResetTerminalDefaults)),
                space().width(Fill),
                text(t.rdp_reconnect_hint).size(10).color(p.text_dim),
            ]
            .spacing(8)
            .align_y(alignment::Vertical::Center),
        ]
        .spacing(12)
        .width(Fill),
    )
}

/// 分节：与旧版 `Home_Setting` 同一套排版——标题 + 说明 + 细横线 + 内容，
/// 不用卡片底色/内边距。七张卡各自带 32px 纵向留白，叠起来就是半屏滚动。
/// 卡片说明固定两行高。中英文行数不同（英文常多一行）会把下面所有卡片
/// 往下推；固定高度后切语言只换文字，整页高度与滚动位置都不动。
const HINT_H: f32 = 27.0;

fn card<'a>(
    title: &'a str,
    hint: &'a str,
    body: impl Into<Element<'a, Message>>,
) -> Element<'a, Message> {
    let p = theme::palette();

    column![
        // 与编辑器的分组标题同一种「层级靠字重」的表达：字号收到正文级，
        // 用粗体把它从说明与内容里拎出来。
        text(title)
            .size(theme::T_CONTROL)
            .color(p.text)
            .font(theme::ui_font_weight(iced::font::Weight::Bold)),
        container(
            text(hint)
                .size(theme::T_MICRO)
                .color(p.text_dim)
                .width(Fill)
                .wrapping(iced::widget::text::Wrapping::Word),
        )
        .width(Fill)
        .height(Length::Fixed(HINT_H))
        .clip(true),
        container(space().width(Fill).height(1.0)).style(theme::section_rule()),
        body.into(),
    ]
    .spacing(6)
    .width(Fill)
    .into()
}
