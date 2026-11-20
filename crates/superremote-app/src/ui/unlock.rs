//! 启动解锁页与致命错误页。

use iced::widget::{column, container, text, text_input};
use iced::{alignment, Element, Fill};

use crate::app::{App, Message};
use crate::{i18n, theme};

pub fn view(app: &App) -> Element<'_, Message> {
    let p = theme::palette();
    let t = i18n::t();

    let mut form = column![
        super::logo(64.0),
        text("SuperRemote").size(22),
        text(t.unlock_hint).size(12).color(p.text_dim),
        text_input(t.unlock_password, &app.unlock.password)
            .on_input(Message::UnlockPasswordChanged)
            .on_submit(Message::SubmitUnlock)
            .secure(true)
            .padding([7.0, 10.0])
            .size(14)
            .style(theme::text_input_style())
            .width(280.0),
    ]
    .spacing(12)
    .align_x(alignment::Horizontal::Center);

    if let Some(error) = &app.unlock.error {
        form = form.push(text(error.clone()).size(12).color(p.danger));
    }

    form = form.push(super::button::primary(
        t.unlock_action,
        Some(Message::SubmitUnlock),
    ));

    container(form)
        .width(Fill)
        .height(Fill)
        .center_x(Fill)
        .center_y(Fill)
        .style(theme::canvas())
        .into()
}

pub fn fatal(message: &str) -> Element<'_, Message> {
    let p = theme::palette();
    let t = i18n::t();

    container(
        column![
            super::logo(48.0),
            text(t.fatal_title).size(16),
            text(message.to_owned()).size(12).color(p.text_dim),
            text(t.fatal_hint).size(11).color(p.text_dim),
        ]
        .spacing(10)
        .align_x(alignment::Horizontal::Center),
    )
    .width(Fill)
    .height(Fill)
    .center_x(Fill)
    .center_y(Fill)
    .style(theme::canvas())
    .into()
}
