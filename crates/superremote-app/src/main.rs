#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use superremote_app::App;

fn main() -> iced::Result {
    // 提权实例只做一件事：把系统设置写回默认，然后立刻退出。
    // 不建窗口，避免 UAC 确认后闪一下主界面。
    if std::env::args().any(|arg| arg == "--fix-rdp-tuning") {
        let _ = superremote_win::sys_tuning::apply_defaults();
        return Ok(());
    }
    // `--secure-desktop=on|off`：开关 UAC 安全桌面，同样只在提权实例里执行。
    if let Some(flag) = std::env::args().find(|arg| arg.starts_with("--secure-desktop=")) {
        let _ = superremote_win::sys_tuning::set_secure_desktop(flag.ends_with("=on"));
        return Ok(());
    }

    // 必须在创建任何窗口之前设置：mixed-DPI 承载只对之后新建的父窗口生效。
    // 否则 mstsc 子窗口会被 DPI 虚拟化，鼠标划过 RDP 区域会一大一小地跳。
    superremote_win::rdp_host::enable_mixed_dpi_hosting();

    // 字体族与显示模式都在首次打开设置页时才枚举；提前预热，避免第一次
    // 切页时在渲染线程上做几十毫秒的系统调用（GDI / EnumDisplaySettings）。
    superremote_win::fonts::monospace_families();
    superremote_win::display::resolutions();

    // 用 `daemon` 而不是 `application`：后者的 `view` 收不到窗口 id，
    // 而本应用要按窗口渲染（主窗口整套管理界面，独立窗口只装一个会话）。
    // daemon 不自动建窗口，主窗口由 `App::boot` 里的 `window::open` 开出来。
    iced::daemon(App::boot, App::update, App::view)
        .default_font(superremote_app::theme::ui_font())
        .title(App::title)
        .theme(App::theme)
        .subscription(App::subscription)
        .run()
}
