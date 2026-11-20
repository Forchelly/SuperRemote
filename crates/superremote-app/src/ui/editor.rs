//! 主页：选中条目的详情编辑器。
//!
//! 版式对齐终稿 E1+：页面上只有两张分组（身份认证 / 远程连接），分组头是
//! 「圆圈箭头 + 标题 + 通栏底线」，控件统一 400 宽、标签列 88，分组级按钮
//! 缩进到控件列，页面级按钮固定在底部 52px 操作条里。

use iced::widget::canvas::{self, Frame as CanvasFrame, Geometry, Path};
use iced::widget::{
    button, canvas as canvas_widget, column, container, row, scrollable, space, text, text_editor,
    text_input,
};
use iced::{alignment, mouse, Element, Fill, Point, Rectangle, Theme};
use superremote_core::session::{
    RdpAudio, RdpColorDepth, RdpGraphics, RdpHostMode, RdpKeyboard, RdpPerformance, SessionKind,
    TerminalBackspace, TerminalCharset, TerminalColorScheme, TerminalCursor, TerminalHomeEnd,
    TerminalKeypad, TerminalMouseMode,
};

use crate::app::{App, DraftField, Message, RdpEdit, RdpRedirect, TerminalEdit};
use crate::i18n;
use crate::ui::fields;
use crate::{theme, ui};

/// 同一分组内相邻字段的间距（稿子 10）。
const ROW_GAP: f32 = 8.0;
/// 两张分组之间的距离（稿子 18）。
const GROUP_GAP: f32 = 15.0;
/// 勾选框与分组级按钮缩进到控件列：标签列 88 + 间距 12。
const INDENT: f32 = ui::LABEL_W + ui::LABEL_GAP;

/// 分组头里的小三角。用几何路径画而不是字体字形：`▾` 的墨迹在行盒里
/// 偏上，固定 15px 圆环里看起来会浮在顶部（文字基线不完全等于视觉中心）。
struct Chevron {
    open: bool,
}

impl canvas::Program<Message> for Chevron {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &iced::Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = CanvasFrame::new(renderer, bounds.size());
        let color = theme::palette().text_dim;
        let cx = bounds.width / 2.0;
        let cy = bounds.height / 2.0;
        let half = 2.4;
        let depth = 1.8;

        let path = Path::new(|builder| {
            if self.open {
                builder.move_to(Point::new(cx - half, cy - depth));
                builder.line_to(Point::new(cx + half, cy - depth));
                builder.line_to(Point::new(cx, cy + depth));
            } else {
                builder.move_to(Point::new(cx - depth, cy - half));
                builder.line_to(Point::new(cx - depth, cy + half));
                builder.line_to(Point::new(cx + depth, cy));
            }
            builder.close();
        });
        frame.fill(&path, color);

        vec![frame.into_geometry()]
    }
}

/// 分组头：15px 细环 + 展开箭头 + 标题，整行可点。
fn section_header<'a>(open: bool, title: &'a str, toggle: Message) -> Element<'a, Message> {
    let p = theme::palette();

    button(
        row![
            container(canvas_widget(Chevron { open }).width(15.0).height(15.0),)
                .width(15.0)
                .height(15.0)
                .style(theme::section_dot()),
            // 分区标题用 14px + Bold：微软雅黑 UI 只有 Regular(400)/Bold(700)，
            // 请求 Semibold 也会落到 Bold。13px 在 150% 下只有 19.5 物理像素，
            // 笔画会在 2/3 像素之间来回取整；14px 是 21 物理像素，粗体笔画稳定
            // 落在 3 像素上，不再出现忽粗忽细的台阶。
            // 标题与圆环的视觉中心对齐：14px 文字的墨迹落在行盒中心下方约
            // 0.8px；给标题留 1.5px 底边，行高增大后圆环居中时同步下移。
            container(
                text(title.to_owned())
                    .size(14.0)
                    .color(p.text)
                    .font(theme::ui_font_weight(iced::font::Weight::Bold)),
            )
            .padding(iced::Padding {
                top: 0.0,
                right: 0.0,
                bottom: 1.5,
                left: 0.0,
            }),
            space().width(Fill),
        ]
        .spacing(7)
        .align_y(alignment::Vertical::Center),
    )
    .on_press(toggle)
    .width(Fill)
    // 稿子：标题上方 4、下方 8 再画底线。文字与圆环的对齐靠标题容器那 1.5px，
    // 不能把 8px 塞进标题容器里，否则文字会被推得比圆环高 3px。
    .padding(iced::Padding {
        top: 4.0,
        right: 0.0,
        bottom: 8.0,
        left: 0.0,
    })
    .style(theme::checkbox_row())
    .into()
}

/// 折叠体：收起时高度归零并裁切。**不删子项**——Iced 按位置 diff，
/// 增删子树会让整行的控件状态串位。
fn collapsible<'a>(open: bool, body: Element<'a, Message>) -> Element<'a, Message> {
    container(body)
        .width(Fill)
        .height(if open {
            iced::Length::Shrink
        } else {
            iced::Length::Fixed(0.0)
        })
        .clip(true)
        .into()
}

/// 一张分组：分组头连同底线 + 可折叠的内容体。
fn group<'a>(
    open: bool,
    title: &'a str,
    toggle: Message,
    body: Element<'a, Message>,
) -> Element<'a, Message> {
    column![
        column![section_header(open, title, toggle), ui::divider()],
        collapsible(open, body),
    ]
    .spacing(12)
    .width(Fill)
    .into()
}

pub fn view(app: &App, _available: f32) -> Element<'_, Message> {
    let Some(id) = app.selected.clone() else {
        return empty_state();
    };

    let Some(tree) = app.tree() else {
        return empty_state();
    };

    let t = i18n::t();
    let is_directory = tree.is_directory(&id);
    let protocol = tree
        .find_session(&id)
        .map(|session| session.kind)
        .unwrap_or(SessionKind::Ssh);

    let auth_title = if is_directory {
        t.section_directory
    } else {
        t.section_connection
    };

    let mut groups = column![group(
        app.connection_section_open,
        auth_title,
        Message::ToggleConnectionSection,
        auth_body(app, &id, is_directory),
    )]
    .spacing(GROUP_GAP)
    .width(Fill);

    if !is_directory {
        let (title, body) = if protocol == SessionKind::Rdp {
            (t.rdp_settings_title, rdp_body(app, &id))
        } else {
            (t.terminal_settings_title, terminal_body(app, &id))
        };

        groups = groups.push(group(
            app.protocol_section_open,
            title,
            Message::ToggleProtocolSection,
            body,
        ));
    }

    // 内容从 `PANE_PAD` 的左上角开始铺，不居中：终稿里 864 宽的详情栏
    // 刻意让 500 宽的表单贴着左基准线，右侧留白。
    container(
        column![
            scrollable(
                container(groups)
                    .width(Fill)
                    .padding([ui::PANE_PAD[0], ui::PANE_PAD[1]]),
            )
            .width(Fill)
            .height(Fill),
            ui::divider(),
            action_bar(is_directory),
        ]
        .spacing(0)
        .width(Fill)
        .height(Fill),
    )
    .width(Fill)
    .height(Fill)
    .into()
}

/// 身份认证分组：名称 / 地址 / 用户名 / 密码 / 描述（SSH 还有私钥）。
fn auth_body<'a>(app: &'a App, id: &str, is_directory: bool) -> Element<'a, Message> {
    let t = i18n::t();
    let draft = &app.draft;

    if is_directory {
        return column![ui::field_id(
            t.field_name,
            &draft.name,
            t.placeholder_directory_name,
            ui::name_field_id(),
            |value| Message::DraftChanged(DraftField::Name, value),
        )]
        .spacing(ROW_GAP)
        .width(ui::FORM_W)
        .into();
    }

    let mut fields = column![
        ui::field_id(
            t.field_name,
            &draft.name,
            t.placeholder_session_name,
            ui::name_field_id(),
            |value| Message::DraftChanged(DraftField::Name, value),
        ),
        // 地址与端口在同一行：稿子里只有一个「地址」字段，端口用 62px 的
        // 后缀框跟在后面，不额外占一行。
        ui::form_row(
            t.field_host,
            row![
                text_input(t.placeholder_host, &draft.host)
                    .on_input(|value| Message::DraftChanged(DraftField::Host, value))
                    .padding([6.0, 9.0])
                    .size(theme::T_CONTROL)
                    .style(theme::text_input_style())
                    .width(Fill),
                text(":").size(13).color(theme::palette().text_dim),
                text_input("3389", &draft.port)
                    .on_input(|value| Message::DraftChanged(DraftField::Port, value))
                    .padding([6.0, 9.0])
                    .size(theme::T_CONTROL)
                    .style(theme::text_input_style())
                    .width(62.0),
            ]
            .spacing(6)
            .align_y(alignment::Vertical::Center),
        ),
        ui::field(
            t.field_username,
            &draft.username,
            t.placeholder_username,
            |value| Message::DraftChanged(DraftField::Username, value),
        ),
        password_row(app),
        note_row(app),
    ]
    .spacing(ROW_GAP)
    .width(ui::FORM_W);

    let protocol = app
        .tree()
        .and_then(|tree| tree.find_session(id))
        .map(|session| session.kind);

    if protocol == Some(SessionKind::Ssh) {
        fields = fields.push(private_key_row(app, id));
    }

    fields.into()
}

/// 口令行：输入框 + 行内「清除」+ 长按可见。控件总宽仍是 400。
fn password_row(app: &App) -> Element<'_, Message> {
    let t = i18n::t();
    let draft = &app.draft;

    ui::form_row(
        t.field_password,
        row![
            text_input(
                if draft.reveal_password {
                    t.placeholder_password_plain
                } else {
                    t.placeholder_password
                },
                &draft.password,
            )
            .on_input(|value| Message::DraftChanged(DraftField::Password, value))
            .secure(!draft.reveal_password)
            .padding([6.0, 9.0])
            .size(theme::T_CONTROL)
            .style(theme::text_input_style())
            .width(Fill),
            ui::button::secondary(t.clear_password, Some(Message::ClearDraftPassword)),
            // 按住看明文，松开立刻回到密文：按下/松开各发一条显式消息，
            // 不用 toggle（toggle 在长按丢事件时会停在错误状态）。
            ui::hold_reveal(draft.reveal_password),
        ]
        .spacing(6)
        .align_y(alignment::Vertical::Center),
    )
}

/// 描述：三行文本框，高度与稿子里的 74 一致。
fn note_row(app: &App) -> Element<'_, Message> {
    let t = i18n::t();
    let p = theme::palette();

    row![
        container(
            text(t.field_note)
                .size(theme::T_FIELD_LABEL)
                .color(p.text_dim)
        )
        .width(ui::LABEL_W)
        .padding([6.0, 0.0]),
        text_editor(&app.note_editor)
            .placeholder(t.placeholder_note)
            .on_action(Message::NoteAction)
            .height(74.0)
            .padding(8.0)
            .size(12.5)
            .style(theme::text_editor_style()),
    ]
    .spacing(ui::LABEL_GAP)
    .align_y(alignment::Vertical::Top)
    .width(ui::FORM_W)
    .into()
}

/// SSH 私钥：多行框 + 清除 / 保存状态，同样走 400 宽的控件列。
fn private_key_row<'a>(app: &'a App, id: &str) -> Element<'a, Message> {
    let t = i18n::t();
    let p = theme::palette();
    let saved = app.has_private_key(id);

    row![
        container(
            text(t.private_key_title)
                .size(theme::T_FIELD_LABEL)
                .color(p.text_dim)
        )
        .width(ui::LABEL_W)
        .padding([6.0, 0.0]),
        column![
            text_editor(&app.private_key_editor)
                .placeholder(t.private_key_placeholder)
                .on_action(Message::PrivateKeyAction)
                .height(56.0)
                .padding(8.0)
                .size(12)
                .style(theme::text_editor_style()),
            row![
                ui::button::secondary(t.clear_private_key, Some(Message::ClearPrivateKey)),
                text(if saved {
                    t.private_key_saved
                } else {
                    t.private_key_unsaved
                })
                .size(10)
                .color(if saved { p.ssh } else { p.text_dim }),
            ]
            .spacing(8)
            .align_y(alignment::Vertical::Center),
        ]
        .spacing(4)
        .width(Fill),
    ]
    .spacing(ui::LABEL_GAP)
    .align_y(alignment::Vertical::Top)
    .width(ui::FORM_W)
    .into()
}

/// 打开方式：不覆盖时显示当前会继承到的方式（全局默认或内置的选项卡）。
fn open_mode_field<'a>(app: &'a App, id: &str) -> Element<'a, Message> {
    let t = i18n::t();
    let lang = i18n::lang();

    let Some(session) = app.tree().and_then(|tree| tree.find_session(id)) else {
        return column![].into();
    };

    fields::pick_field(
        t.open_mode,
        fields::open_mode_choices(
            lang.inherit_with(fields::open_mode_label(app.effective_open_mode(id), t)),
        ),
        session.settings.open_mode,
        Message::OpenModeEdit,
    )
}

/// 远程连接分组（RDP）：首选 / 分辨率 / 性能 / 颜色 / 音频 / 录音 / 组合键，
/// 下面一整行五态勾选框，最后是分组级按钮。
fn rdp_body<'a>(app: &'a App, id: &str) -> Element<'a, Message> {
    let t = i18n::t();
    let lang = i18n::lang();

    let Some(session) = app.tree().and_then(|tree| tree.find_session(id)) else {
        return column![].into();
    };

    let defaults = app.rdp_defaults();
    let overrides = &session.settings.rdp;
    let resolved = overrides.resolve(&defaults);

    let performance = fields::pick_field(
        t.rdp_performance,
        fields::choices(
            lang.inherit_with(fields::performance_label(resolved.performance, t)),
            RdpPerformance::ALL
                .into_iter()
                .map(|value| (value, fields::performance_label(value, t).to_owned())),
        ),
        overrides.performance,
        |value| Message::RdpEdit(RdpEdit::Performance(value)),
    );

    let color = fields::pick_field(
        t.rdp_color_depth,
        fields::choices(
            lang.inherit_with(fields::color_label(resolved.color_depth, t)),
            RdpColorDepth::ALL
                .into_iter()
                .map(|value| (value, fields::color_label(value, t).to_owned())),
        ),
        overrides.color_depth,
        |value| Message::RdpEdit(RdpEdit::ColorDepth(value)),
    );

    let audio = fields::pick_field(
        t.rdp_audio,
        fields::choices(
            lang.inherit_with(fields::audio_label(resolved.audio, t)),
            RdpAudio::ALL
                .into_iter()
                .map(|value| (value, fields::audio_label(value, t).to_owned())),
        ),
        overrides.audio,
        |value| Message::RdpEdit(RdpEdit::Audio(value)),
    );

    // 录音在稿子里也是下拉（继承 / 开 / 关），不是勾选框。
    let audio_capture = fields::pick_field(
        t.rdp_audio_capture,
        fields::choices(
            lang.inherit_with(if resolved.audio_capture {
                t.toggle_on
            } else {
                t.toggle_off
            }),
            [
                (true, t.toggle_on.to_owned()),
                (false, t.toggle_off.to_owned()),
            ],
        ),
        overrides.audio_capture,
        |value| Message::RdpEdit(RdpEdit::AudioCapture(value)),
    );

    let keyboard = fields::pick_field(
        t.rdp_keyboard,
        fields::choices(
            lang.inherit_with(fields::keyboard_label(resolved.keyboard, t)),
            RdpKeyboard::ALL
                .into_iter()
                .map(|value| (value, fields::keyboard_label(value, t).to_owned())),
        ),
        overrides.keyboard,
        |value| Message::RdpEdit(RdpEdit::Keyboard(value)),
    );

    let desktop = fields::pick_field(
        t.rdp_desktop_size,
        fields::desktop_choices(
            lang.inherit_with(&fields::desktop_label(resolved.desktop_size, t)),
        ),
        overrides.desktop_size,
        |value| Message::RdpEdit(RdpEdit::DesktopSize(value)),
    );

    // 控件版本只表达协议与兼容性差异，不承担 UAC 卡顿开关的语义
    let graphics = fields::pick_field(
        t.rdp_graphics,
        fields::choices(
            lang.inherit_with(fields::graphics_label(resolved.graphics, t)),
            RdpGraphics::ALL
                .into_iter()
                .map(|value| (value, fields::graphics_label(value, t).to_owned())),
        ),
        overrides.graphics,
        |value| Message::RdpEdit(RdpEdit::Graphics(value)),
    );

    let host = fields::pick_field(
        t.rdp_host_mode,
        fields::choices(
            lang.inherit_with(fields::host_label(resolved.host_mode, t)),
            RdpHostMode::ALL
                .into_iter()
                .map(|value| (value, fields::host_label(value, t).to_owned())),
        ),
        overrides.host_mode,
        |value| Message::RdpEdit(RdpEdit::HostMode(value)),
    );

    let redirect = |label, field, value| {
        fields::tri_checkbox(label, value, move |value| {
            Message::RdpEdit(RdpEdit::Redirect { field, value })
        })
    };

    let settings = column![
        open_mode_field(app, id),
        host,
        desktop,
        performance,
        color,
        audio,
        audio_capture,
        keyboard,
        graphics,
    ]
    .spacing(ROW_GAP)
    .width(ui::FORM_W);

    let redirects = container(
        row![
            redirect(
                t.rdp_redirect_printers,
                RdpRedirect::Printers,
                overrides.redirect_printers
            ),
            redirect(
                t.rdp_redirect_clipboard,
                RdpRedirect::Clipboard,
                overrides.redirect_clipboard
            ),
            redirect(
                t.rdp_redirect_smartcards,
                RdpRedirect::Smartcards,
                overrides.redirect_smartcards
            ),
            redirect(
                t.rdp_redirect_ports,
                RdpRedirect::Ports,
                overrides.redirect_ports
            ),
            redirect(
                t.rdp_redirect_drives,
                RdpRedirect::Drives,
                overrides.redirect_drives
            ),
        ]
        .spacing(26),
    )
    .width(Fill)
    .padding(iced::Padding {
        top: 12.0,
        right: 0.0,
        bottom: 12.0,
        left: INDENT,
    });

    column![
        settings,
        redirects,
        group_footer(Message::ResetRdpSettings, Message::MakeRdpDefaults,),
    ]
    .spacing(0)
    .width(Fill)
    .into()
}

/// 终端分组（SSH/Telnet）：9 个设置在稿子的控件尺寸下分两列排，
/// 一列到底会多出半屏；分组按钮同样是缩进到控件列。
fn terminal_body<'a>(app: &'a App, id: &str) -> Element<'a, Message> {
    let t = i18n::t();
    let lang = i18n::lang();

    let Some(session) = app.tree().and_then(|tree| tree.find_session(id)) else {
        return column![].into();
    };

    let defaults = app.terminal_defaults();
    let overrides = &session.settings.terminal;
    let resolved = overrides.resolve(&defaults);
    let default_font = defaults
        .font_family
        .clone()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "Consolas".to_owned());

    let font_family = fields::pick_field(
        t.terminal_font_family,
        fields::font_choices(
            lang.inherit_with(&default_font),
            overrides.font_family.as_deref(),
        ),
        overrides.font_family.clone(),
        |value| Message::TerminalEdit(TerminalEdit::FontFamily(value)),
    );

    let font_size = fields::pick_field(
        t.terminal_font_size,
        fields::choices(
            lang.inherit_with(&format!("{} pt", resolved.font_size)),
            (9u8..=20).map(|size| (size, format!("{size} pt"))),
        ),
        overrides.font_size,
        |value| Message::TerminalEdit(TerminalEdit::FontSize(value)),
    );

    let cursor = fields::pick_field(
        t.terminal_cursor,
        fields::choices(
            lang.inherit_with(fields::cursor_label(resolved.cursor, t)),
            TerminalCursor::ALL
                .into_iter()
                .map(|value| (value, fields::cursor_label(value, t).to_owned())),
        ),
        overrides.cursor,
        |value| Message::TerminalEdit(TerminalEdit::Cursor(value)),
    );

    let backspace = fields::pick_field(
        t.terminal_backspace,
        fields::choices(
            lang.inherit_with(fields::backspace_label(resolved.backspace, t)),
            TerminalBackspace::ALL
                .into_iter()
                .map(|value| (value, fields::backspace_label(value, t).to_owned())),
        ),
        overrides.backspace,
        |value| Message::TerminalEdit(TerminalEdit::Backspace(value)),
    );

    let home_end = fields::pick_field(
        t.terminal_home_end,
        fields::choices(
            lang.inherit_with(fields::home_end_label(resolved.home_end, t)),
            TerminalHomeEnd::ALL
                .into_iter()
                .map(|value| (value, fields::home_end_label(value, t).to_owned())),
        ),
        overrides.home_end,
        |value| Message::TerminalEdit(TerminalEdit::HomeEnd(value)),
    );

    let color_scheme = fields::pick_field(
        t.terminal_color_scheme,
        fields::choices(
            lang.inherit_with(fields::scheme_label(resolved.color_scheme, t)),
            TerminalColorScheme::ALL
                .into_iter()
                .map(|value| (value, fields::scheme_label(value, t).to_owned())),
        ),
        overrides.color_scheme,
        |value| Message::TerminalEdit(TerminalEdit::ColorScheme(value)),
    );

    let charset = fields::pick_field(
        t.terminal_charset,
        fields::choices(
            lang.inherit_with(fields::charset_label(resolved.charset, t)),
            TerminalCharset::ALL
                .into_iter()
                .map(|value| (value, fields::charset_label(value, t).to_owned())),
        ),
        overrides.charset,
        |value| Message::TerminalEdit(TerminalEdit::Charset(value)),
    );

    let mouse = fields::pick_field(
        t.terminal_mouse,
        fields::choices(
            lang.inherit_with(fields::mouse_mode_label(resolved.mouse_mode, t)),
            TerminalMouseMode::ALL
                .into_iter()
                .map(|value| (value, fields::mouse_mode_label(value, t).to_owned())),
        ),
        overrides.mouse_mode,
        |value| Message::TerminalEdit(TerminalEdit::MouseMode(value)),
    );

    let keypad = fields::pick_field(
        t.terminal_keypad,
        fields::choices(
            lang.inherit_with(fields::keypad_label(resolved.keypad, t)),
            TerminalKeypad::ALL
                .into_iter()
                .map(|value| (value, fields::keypad_label(value, t).to_owned())),
        ),
        overrides.keypad,
        |value| Message::TerminalEdit(TerminalEdit::Keypad(value)),
    );

    let left = column![font_family, cursor, backspace, charset, keypad]
        .spacing(ROW_GAP)
        .width(Fill);
    let right = column![font_size, color_scheme, home_end, mouse]
        .spacing(ROW_GAP)
        .width(Fill);

    column![
        row![left, right].spacing(12).width(Fill),
        group_footer(
            Message::ResetTerminalSettings,
            Message::MakeTerminalDefaults,
        ),
    ]
    .spacing(0)
    .width(Fill)
    .into()
}

/// 分组级按钮：重置为继承 / 设为默认，缩进到控件列。
fn group_footer<'a>(reset: Message, set_default: Message) -> Element<'a, Message> {
    let t = i18n::t();

    container(
        row![
            ui::button::secondary(t.rdp_reset, Some(reset)),
            ui::button::secondary(t.rdp_set_default, Some(set_default)),
        ]
        .spacing(10),
    )
    .width(Fill)
    .padding(iced::Padding {
        top: 10.0,
        right: 0.0,
        bottom: 0.0,
        left: INDENT,
    })
    .into()
}

/// 底部操作条：高 52，白底，上边一条发丝线。保存贴左、删除靠右。
fn action_bar(is_directory: bool) -> Element<'static, Message> {
    let t = i18n::t();

    // 目录没有「打开连接」这个动作，但子项位置**和控件类型**都必须固定：
    // Iced 按子元素位置做 diff，用 `space()` 顶替按钮会让这个位置上出现另一种控件。
    let secondary: Element<'static, Message> = if is_directory {
        ui::button::secondary(t.action_open, None)
    } else {
        ui::button::secondary(t.action_open, Some(Message::OpenSelected))
    };

    container(
        row![
            ui::button::primary(t.action_save, Some(Message::ApplyDraft)),
            secondary,
            space().width(Fill),
            ui::button::danger(
                if is_directory {
                    t.action_delete_directory
                } else {
                    t.action_delete_session
                },
                Some(Message::DeleteSelected),
            ),
        ]
        .spacing(8)
        .align_y(alignment::Vertical::Center),
    )
    .width(Fill)
    .height(52.0)
    .padding([0.0, ui::PANE_PAD[1]])
    .align_y(alignment::Vertical::Center)
    .style(theme::status_strip())
    .into()
}

/// 空态：一段说明 + 三个新建立即入口。
fn empty_state() -> Element<'static, Message> {
    let p = theme::palette();
    let t = i18n::t();

    container(
        column![
            text(t.empty_title).size(16).color(p.text),
            text(t.empty_hint).size(12).color(p.text_dim),
            row![
                ui::button::secondary("＋ SSH", Some(Message::NewSession(SessionKind::Ssh))),
                ui::button::secondary("＋ RDP", Some(Message::NewSession(SessionKind::Rdp))),
                ui::button::secondary("＋ Telnet", Some(Message::NewSession(SessionKind::Telnet))),
            ]
            .spacing(8),
            text(t.empty_storage_note).size(10).color(p.text_dim),
        ]
        .spacing(8)
        .align_x(alignment::Horizontal::Center),
    )
    .width(Fill)
    .height(Fill)
    .center_x(Fill)
    .center_y(Fill)
    .into()
}
