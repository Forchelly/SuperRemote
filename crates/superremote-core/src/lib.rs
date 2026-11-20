//! SuperRemote 的领域层。
//!
//! 这一层刻意不依赖 iced：会话树、搜索、标签页规则都可以脱离 UI 单测，
//! 阶段二的持久化和阶段三的协议控件同样只对接这里的类型。

pub mod session;
pub mod tab;
