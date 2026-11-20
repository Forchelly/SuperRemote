//! 连接层的错误类型。

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetError {
    /// 本机找不到 OpenSSH 客户端。
    NoSshClient,
    /// 连接层只实现了 Windows 的伪控制台。
    Unsupported(&'static str),
    /// IO 失败。`kind` 交给界面层翻译成「端口没开 / 名字解析不了」这类可操作
    /// 的建议；`message` 是带上下文的原文。
    Io {
        kind: std::io::ErrorKind,
        message: String,
    },
    /// 进程 / 宿主层的消息类失败，没有对应的 io 分类可带。
    Other(String),
}

impl NetError {
    /// IO 分类。非 IO 失败返回 `None`，调用方按普通错误展示即可。
    pub fn error_kind(&self) -> Option<std::io::ErrorKind> {
        match self {
            Self::Io { kind, .. } => Some(*kind),
            _ => None,
        }
    }
}

impl fmt::Display for NetError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::NoSshClient => f.write_str(
                "找不到 ssh.exe。请在 Windows「可选功能」里安装 OpenSSH 客户端，或改用 Telnet 连接。",
            ),
            Self::Unsupported(what) => write!(f, "当前平台不支持：{what}"),
            Self::Io { message, .. } => f.write_str(message),
            Self::Other(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for NetError {}
