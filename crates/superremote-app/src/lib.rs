//! SuperRemote 的应用层：状态机、视图与运行时装配。
//!
//! 主程序只是 [`app::App`] 的一层薄壳（`main.rs`），把这里拆成库是为了让集成
//! 测试能直接驱动 `update`，从而在无窗口的情况下验证「真实连接 → 网格 → 输入」
//! 这整条链路。

pub mod app;
pub mod i18n;
pub mod rdp;
pub mod shortcut;
pub mod terminal;
pub mod theme;
pub mod ui;

pub use app::App;
