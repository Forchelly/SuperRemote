//! SuperRemote 的 Windows 平台层。
//!
//! 这里只放「直接跟操作系统打交道」的东西：Win32 宿主窗口、系统剪贴板、字体与
//! 显示模式枚举、原生目录选择器、运行时窗口图标的像素数据。业务状态机与界面
//! 留在 `superremote-app`，依赖方向只有 `superremote-app → superremote-win`，本 crate 不反向引用它。
//!
//! 与 C# RDP 宿主的字符串契约（窗口标题前缀、命令行参数名）也集中在这里的
//! [`contract`]，Rust 侧只从这里取字面量。

pub mod clipboard;
pub mod contract;
pub mod dir_picker;
pub mod display;
pub mod fonts;
pub mod icon_rgba;
pub mod layout;
pub mod rdp_host;
pub mod sys_tuning;
pub mod system;
