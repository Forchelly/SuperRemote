//! SuperRemote 连接层：SSH（ConPTY + 系统 OpenSSH）与 Telnet（原生 TCP）。
//!
//! 每个连接跑在自己的线程里，通过两条单向通道与界面通信：
//! 界面 → 连接是 [`Command`]，连接 → 界面是 [`NetEvent`]。
//! 界面侧不需要任何异步运行时，只要定期 [`Hub::drain`] 即可，
//! 这样 Iced 的 `update` 保持同步、可单测。

mod error;
#[cfg(windows)]
mod ssh;
mod telnet;

use std::collections::HashMap;
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};

pub use crate::error::NetError;
#[cfg(windows)]
pub use crate::ssh::ssh_client;

pub type ConnId = u64;

/// 丢弃一个含敏感内容的临时文件：先按原长度覆写一遍再删除。
///
/// 这只能算「尽力而为」——NTFS 日志与 SSD 的块重映射都可能留住旧扇区内容，
/// 真正不可恢复需要整盘加密。这么做的意义是不把明文留在「已删除但目录项仍可读」
/// 的文件里，而成本近乎为零。
///
/// `superremote-app` 也用它在启动时清掉上次异常退出遗留的临时文件：release 档是
/// `panic = "abort"`，任何 panic 都不会跑 `Drop`，清理只能靠下一次启动补上。
pub fn discard_secret_file(path: &std::path::Path) {
    use std::io::Write;

    if let Ok(metadata) = std::fs::metadata(path) {
        let len = metadata.len() as usize;
        if let Ok(mut file) = std::fs::OpenOptions::new().write(true).open(path) {
            let zeros = [0u8; 4096];
            let mut remaining = len;
            while remaining > 0 {
                let chunk = remaining.min(zeros.len());
                if file.write_all(&zeros[..chunk]).is_err() {
                    break;
                }
                remaining -= chunk;
            }
        }
    }

    // 删不掉就留着，下一次启动的清理会再来一遍。
    let _ = std::fs::remove_file(path);
}

/// 连接的目标地址与登录信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub host: String,
    pub port: u16,
    pub user: Option<String>,
    /// 只有 SSH 用得到；Telnet 通常是无认证的。
    pub password: Option<String>,
    /// OpenSSH 私钥内容。`spawn` 会写入临时文件，进程结束后删除。
    pub private_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetEvent {
    /// 远端送来的原始字节流，交给终端网格解析。
    Data(Vec<u8>),
    /// 连接层自己确认「可以输入了」。目前只有 Telnet 会发——TCP 握手成功即成立。
    ///
    /// SSH 不发：它跑在 `ssh.exe` 里，认证是否完成是子进程内部状态，进程外拿不到
    /// 可靠的信号（`ssh` 也不提供机器可读的进度输出）。所以界面侧对 SSH 仍然要
    /// 靠提示符启发式（`superremote-app` 的 `terminal::Status::is_prompt`）判断，
    /// 并用「已收到字节就允许输入」兜住口令提示这种没有提示符的情况。
    Ready,
    /// 子进程或 socket 已断开。
    Exited(Option<i32>),
    Error(String),
}

#[derive(Debug)]
pub(crate) enum Command {
    Input(Vec<u8>),
    Resize(u16, u16),
    Close,
}

/// 连接集合。界面持有一个 `Hub`，所有会话共享同一个事件队列。
#[derive(Default)]
pub struct Hub {
    next: ConnId,
    senders: HashMap<ConnId, Sender<Command>>,
    events: Arc<Mutex<Vec<(ConnId, NetEvent)>>>,
}

impl Hub {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn open_ssh(&mut self, target: &Target, cols: u16, rows: u16) -> Result<ConnId, NetError> {
        #[cfg(windows)]
        {
            let id = self.next_id();
            let (sender, receiver) = channel();
            ssh::spawn(id, target, cols, rows, Arc::clone(&self.events), receiver)?;
            self.senders.insert(id, sender);
            Ok(id)
        }
        #[cfg(not(windows))]
        {
            let _ = (target, cols, rows);
            Err(NetError::Unsupported("SSH 依赖 Windows 伪控制台"))
        }
    }

    pub fn open_telnet(&mut self, target: &Target) -> Result<ConnId, NetError> {
        let id = self.next_id();
        let (sender, receiver) = channel();
        // 建连在工作线程里完成，这里不等它：目标不可达时错误会以
        // `NetEvent::Error` 回来，界面从「连接中」直接转「失败」。
        telnet::spawn(id, target, Arc::clone(&self.events), receiver);
        self.senders.insert(id, sender);
        Ok(id)
    }

    /// 把用户敲的内容发过去。
    pub fn send(&self, id: ConnId, bytes: impl Into<Vec<u8>>) {
        self.dispatch(id, Command::Input(bytes.into()));
    }

    pub fn send_text(&self, id: ConnId, text: &str) {
        self.send(id, text.as_bytes().to_vec());
    }

    /// 终端尺寸变化时通知对端，让 `vim`、`top` 这类程序重排。
    pub fn resize(&self, id: ConnId, cols: u16, rows: u16) {
        self.dispatch(id, Command::Resize(cols, rows));
    }

    pub fn close(&self, id: ConnId) {
        self.dispatch(id, Command::Close);
        // sender 随连接一起失效，后续 send 会自然丢弃。
    }

    pub fn forget(&mut self, id: ConnId) {
        self.senders.remove(&id);
    }

    /// 取走这一轮累积的事件。
    pub fn drain(&self) -> Vec<(ConnId, NetEvent)> {
        // 锁中毒时取回内部值而不是返回空表：返回空表等于把这一轮所有回显都丢掉，
        // 界面会毫无理由地停在上一帧。
        let mut guard = self
            .events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        std::mem::take(&mut *guard)
    }

    pub fn len(&self) -> usize {
        self.senders.len()
    }

    pub fn is_empty(&self) -> bool {
        self.senders.is_empty()
    }

    fn next_id(&mut self) -> ConnId {
        self.next += 1;
        self.next
    }

    fn dispatch(&self, id: ConnId, command: Command) {
        if let Some(sender) = self.senders.get(&id) {
            // 对端线程已经结束时会失败，此时静默丢弃即可。
            let _ = sender.send(command);
        }
    }
}

impl Drop for Hub {
    fn drop(&mut self) {
        for sender in self.senders.values() {
            let _ = sender.send(Command::Close);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;
    use std::time::{Duration, Instant};

    /// 等到队列里出现满足条件的事件，返回全部已收到的正文。
    fn collect(
        hub: &Hub,
        id: ConnId,
        timeout: Duration,
        want: impl Fn(&NetEvent) -> bool,
    ) -> (Vec<u8>, bool) {
        let deadline = Instant::now() + timeout;
        let mut text = Vec::new();

        while Instant::now() < deadline {
            for (from, event) in hub.drain() {
                if from != id {
                    continue;
                }
                match event {
                    NetEvent::Data(chunk) => text.extend_from_slice(&chunk),
                    other if want(&other) => return (text, true),
                    _ => {}
                }
            }
            if Instant::now() >= deadline {
                return (text, false);
            }
            thread::sleep(Duration::from_millis(10));
        }

        (text, false)
    }

    /// 起一个最小的 telnet 服务端：先发一句欢迎，随后把收到的内容原样回显。
    fn fake_server() -> (u16, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        let handle = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket.set_nonblocking(true).unwrap();
            let _ = socket.write_all(b"\xff\xfb\x01hello from server\n");

            let deadline = Instant::now() + Duration::from_secs(6);
            let mut buffer = [0u8; 512];
            while Instant::now() < deadline {
                match socket.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(count) => {
                        let echoed: Vec<u8> = buffer[..count]
                            .iter()
                            .copied()
                            .filter(|byte| *byte != 0xFF && *byte != b'\r')
                            .collect();
                        if socket.write_all(&echoed).is_err() {
                            break;
                        }
                    }
                    Err(_) => thread::sleep(Duration::from_millis(5)),
                }
            }
        });

        (port, handle)
    }

    #[test]
    fn telnet_strips_negotiation_and_round_trips_typed_text() {
        let (port, server) = fake_server();

        let mut hub = Hub::new();
        let id = hub
            .open_telnet(&Target {
                host: "127.0.0.1".to_owned(),
                port,
                user: None,
                password: None,
                private_key: None,
            })
            .expect("应能连上本地测试服务端");

        let (greeting, _) = collect(&hub, id, Duration::from_secs(5), |_| false);
        assert!(
            String::from_utf8_lossy(&greeting).contains("hello from server"),
            "应收到服务端欢迎语，且协商序列已被剥离：{:?}",
            String::from_utf8_lossy(&greeting)
        );

        hub.send_text(id, "echo-ok");
        let mut echoed = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline && echoed.len() < "echo-ok".len() {
            for (from, event) in hub.drain() {
                if from == id {
                    if let NetEvent::Data(chunk) = event {
                        echoed.extend_from_slice(&chunk);
                    }
                }
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(String::from_utf8_lossy(&echoed), "echo-ok");

        hub.close(id);
        hub.forget(id);
        server.join().unwrap();
    }

    #[test]
    fn connecting_to_a_closed_port_reports_an_error_without_blocking_the_caller() {
        let mut hub = Hub::new();
        // 端口 1 在本机没有监听。断言两件事：`open_telnet` 立刻返回（DNS 与握手
        // 都在工作线程里，UI 线程不会被 8 秒超时冻住），失败以事件形式回来。
        let id = hub
            .open_telnet(&Target {
                host: "127.0.0.1".to_owned(),
                port: 1,
                user: None,
                password: None,
                private_key: None,
            })
            .expect("建连不应在调用线程里失败");

        let (_, got_error) = collect(&hub, id, Duration::from_secs(10), |event| {
            matches!(event, NetEvent::Error(_))
        });
        assert!(got_error, "连不上时应通过事件报错而不是让调用方等待");
    }

    #[test]
    fn commands_to_a_forgotten_connection_are_dropped() {
        let hub = Hub::new();
        hub.send_text(999, "no such connection");
        hub.resize(999, 80, 24);
        hub.close(999);
        assert!(hub.is_empty());
    }
}
