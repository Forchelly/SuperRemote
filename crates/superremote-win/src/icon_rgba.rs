//! 运行时窗口图标的像素数据（128×128，RGBA），取自 `assets/icon.ico` 的 128×128
//! 一档原始像素（`assets/icon-128.rgba`），不做缩放或重绘。
//!
//! 只供任务栏 / Alt-Tab 的系统窗口图标使用；界面里的品牌标（标题栏、解锁页、
//! 致命错误页）已改为 `ui::logo` 的画布矢量绘制，不读这份位图。默认渲染后端
//! 不带图片解码器，所以直接存解码后的像素，而不是让运行时读 PNG；二进制用
//! `include_bytes!` 嵌进来，不写成几万行数组字面量——那会让 rustc 在 debug
//! 构建里额外吃掉数百 MB 内存。

pub const ICON_SIZE: u32 = 128;

pub static ICON_RGBA: &[u8; 65536] = include_bytes!("../assets/icon-128.rgba");
