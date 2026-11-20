//! 连接层的错误类型。

use std::fmt;

#[derive(Debug)]
pub enum NetError {
    /// 本机找不到 OpenSSH 客户端。
    NoSshClient,
    /// 连接层只实现了 Windows 的伪控制台。
    Unsupported(&'static str),
    Io(String),
}

impl fmt::Display for NetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoSshClient => f.write_str(
                "找不到 ssh.exe。请在 Windows「可选功能」里安装 OpenSSH 客户端，或改用 Telnet 连接。",
            ),
            Self::Unsupported(what) => write!(f, "当前平台不支持：{what}"),
            Self::Io(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for NetError {}

impl From<std::io::Error> for NetError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value.to_string())
    }
}
