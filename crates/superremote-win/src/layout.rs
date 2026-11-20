//! 主窗口与 RDP 宿主共享的布局常量。
//!
//! 这些值同时被 `superremote-app` 的视图和这里的 `logical_rect` 使用，所以放在平台层，
//! 由 `superremote_app::ui` 原样再导出，避免两处各写一份后悄悄漂移。

/// 自绘标题栏高度。会话画面从这条线下方整窗铺开。
pub const TITLEBAR_H: f32 = 38.0;

/// RDP 全屏时顶部那条「退出全屏」栏的高度。
pub const FULLSCREEN_BAR_H: f32 = 30.0;

/// 缩放热区厚度。原生 RDP 子窗口永远画在 Iced 之上，所以画面区要按这个值
/// 让开左/右/下三条边（见 `rdp_host::logical_rect`）；让开的部分就是画面四周
/// 那圈填充条，因此它同时决定「黑边」的宽度。取 3 是手感（150% DPI 下约
/// 5 个物理像素）与观感（细到像一圈描边）的折中。
pub const RESIZE_EDGE: f32 = 3.0;
