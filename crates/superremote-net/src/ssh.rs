//! SSH 连接：把 OpenSSH 客户端放进 Windows 伪控制台（ConPTY）。
//!
//! 为什么不用纯 Rust 的 SSH 实现（`russh`）：它的两种加密后端 `aws-lc-rs`（要 cmake + NASM）
//! 和 `ring`（要 NASM）在这台机器上都没有可用工具链，而本项目把「单文件、极小体积」当成硬指标。
//! 走系统 `ssh.exe` 换来的额外好处是 known_hosts、代理、密钥链这些安全特性由 OpenSSH 自己维护。

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use alacritty_terminal::event::{OnResize, WindowSize};
use alacritty_terminal::tty::{self, ChildEvent, EventedPty, EventedReadWrite, Options, Shell};

use crate::error::NetError;
use crate::{Command, ConnId, NetEvent, Target};

/// 数据到达时的读取缓冲。
const READ_BUFFER: usize = 16 * 1024;
/// 空闲轮询间隔：没有数据时慢一点，有数据时立刻再来一轮。
const IDLE_POLL: Duration = Duration::from_millis(12);
const BUSY_POLL: Duration = Duration::from_millis(1);
/// 交互认证最多代填几次。
const MAX_PASSWORD_ATTEMPTS: usize = 3;
/// 单次写用户输入的时限：超过说明输入管道真的堵死了，报错比无限等待更安全。
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);

/// 在 PATH 与 OpenSSH 默认目录里找 `ssh.exe`。
pub fn ssh_client() -> Option<PathBuf> {
    let candidates = [
        PathBuf::from("ssh.exe"),
        PathBuf::from(r"C:\Windows\System32\OpenSSH\ssh.exe"),
    ];

    candidates.into_iter().find(|path| {
        if path.is_absolute() {
            path.exists()
        } else {
            std::env::var_os("PATH")
                .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(path).exists()))
                .unwrap_or(false)
        }
    })
}

pub fn spawn(
    id: ConnId,
    target: &Target,
    cols: u16,
    rows: u16,
    events: Arc<Mutex<Vec<(ConnId, NetEvent)>>>,
    commands: Receiver<Command>,
) -> Result<thread::JoinHandle<()>, NetError> {
    if ssh_client().is_none() {
        return Err(NetError::NoSshClient);
    }

    let key_path = match target.private_key.as_deref() {
        Some(key) if !key.trim().is_empty() => Some(write_private_key(id, key)?),
        _ => None,
    };
    let args = arguments(target, key_path.as_deref());
    let options = Options {
        shell: Some(Shell::new("ssh.exe".to_owned(), args)),
        // 远端拿到 TERM 才会输出 256 色与全屏程序需要的转义序列。
        env: [("TERM".to_owned(), "xterm-256color".to_owned())]
            .into_iter()
            .collect(),
        ..Options::default()
    };

    let size = WindowSize {
        num_lines: rows.max(1),
        num_cols: cols.max(1),
        cell_width: 0,
        cell_height: 0,
    };

    let pty = match tty::new(&options, size, 0) {
        Ok(pty) => pty,
        Err(err) => {
            if let Some(path) = &key_path {
                discard_private_key(path);
            }
            return Err(NetError::Io(err.to_string()));
        }
    };
    let password = target.password.clone();

    Ok(thread::spawn(move || {
        run(id, pty, password, events, commands, key_path);
    }))
}

/// 私钥落地到临时目录，供系统 `ssh.exe` 用 `-i` 读取；会话结束由 `run` 删除。
///
/// 文件名带本次调用的随机后缀：`ConnId` 在进程重启后从 1 重新开始，只用 id
/// 会覆盖上一次异常退出遗留的文件，且同名文件可被预判（同一用户下的其他进程
/// 可以抢在写入前占位）。隐私边界仍由 `%TEMP%` 的 ACL 保证——它只授予当前
/// 用户、SYSTEM 与 Administrators。
fn write_private_key(id: ConnId, key: &str) -> Result<PathBuf, NetError> {
    // 目录名沿用旧产品名：里面可能还有上次异常退出留下的私钥，
    // 换名字会让清理逻辑扫不到它们（见 `rdp::clean_stale_temp_files`）。
    let dir = std::env::temp_dir().join("SimpleRemote2");
    std::fs::create_dir_all(&dir)
        .map_err(|err| NetError::Io(format!("无法创建私钥临时目录：{err}")))?;

    let path = dir.join(format!("ssh-key-{id}-{}", unique_suffix()));
    std::fs::write(&path, key).map_err(|err| NetError::Io(format!("无法写入临时私钥：{err}")))?;

    Ok(path)
}

/// 进程内单调、不依赖额外 crate 的后缀：纳秒时间戳 + 当前 PID。
fn unique_suffix() -> u128 {
    let since_epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    since_epoch ^ ((std::process::id() as u128) << 96)
}

/// 丢弃临时私钥文件。擦除逻辑在 `crate::discard_secret_file`，这里只是给
/// 「私钥」这个用途一个好读的调用点。
fn discard_private_key(path: &Path) {
    crate::discard_secret_file(path);
}

/// 组装 `ssh` 命令行。参数不带空格，直接按原文拼接。
fn arguments(target: &Target, key_path: Option<&Path>) -> Vec<String> {
    let mut args = vec![
        "-tt".to_owned(),
        "-o".to_owned(),
        "StrictHostKeyChecking=accept-new".to_owned(),
        "-o".to_owned(),
        "NumberOfPasswordPrompts=1".to_owned(),
        "-o".to_owned(),
        "ServerAliveInterval=30".to_owned(),
        "-p".to_owned(),
        target.port.to_string(),
    ];

    if let Some(key_path) = key_path {
        args.push("-i".to_owned());
        args.push(key_path.to_string_lossy().into_owned());
        args.push("-o".to_owned());
        args.push("IdentitiesOnly=yes".to_owned());
    }

    // 有私钥时先试公钥再退回口令；只有口令时把口令排在最前，避免先试一堆本机密钥。
    if key_path.is_some() && target.password.is_some() {
        args.push("-o".to_owned());
        args.push("PreferredAuthentications=publickey,password,keyboard-interactive".to_owned());
    } else if target.password.is_some() {
        args.push("-o".to_owned());
        args.push("PreferredAuthentications=password,keyboard-interactive,publickey".to_owned());
    }

    match target.user.as_deref() {
        Some(user) if !user.is_empty() => args.push(format!("{}@{}", user, target.host)),
        _ => args.push(target.host.clone()),
    }

    args
}

fn run(
    id: ConnId,
    mut pty: tty::Pty,
    password: Option<String>,
    events: Arc<Mutex<Vec<(ConnId, NetEvent)>>>,
    commands: Receiver<Command>,
    key_path: Option<PathBuf>,
) {
    let mut buffer = [0u8; READ_BUFFER];
    let mut recent: Vec<u8> = Vec::with_capacity(1024);
    let mut attempts = 0usize;
    let mut authenticated = password.is_none();

    'outer: loop {
        let mut had_data = false;

        while let Ok(command) = commands.try_recv() {
            match command {
                Command::Input(bytes) => {
                    if let Err(err) = write_session(&mut pty, &bytes) {
                        push(&events, id, NetEvent::Error(err));
                    }
                }
                Command::Resize(cols, rows) => pty.on_resize(WindowSize {
                    num_lines: rows.max(1),
                    num_cols: cols.max(1),
                    cell_width: 0,
                    cell_height: 0,
                }),
                Command::Close => break 'outer,
            }
        }

        let read = pty.reader().try_read(&mut buffer);
        if read > 0 {
            had_data = true;
            let chunk = buffer[..read].to_vec();
            push(&events, id, NetEvent::Data(chunk.clone()));

            if !authenticated {
                recent.extend_from_slice(&chunk);
                if recent.len() > 1024 {
                    let drain = recent.len() - 1024;
                    recent.drain(..drain);
                }

                let text = String::from_utf8_lossy(&recent).to_lowercase();
                if text.contains("assword:") || text.contains("assword：") {
                    if attempts < MAX_PASSWORD_ATTEMPTS {
                        attempts += 1;
                        if let Some(password) = password.as_deref() {
                            // 代填失败必须上报：否则界面停在远端提示上、用户以为
                            // 程序还在连，实际上是这一行口令根本没送出去。
                            if let Err(err) =
                                write_session(&mut pty, format!("{password}\r").as_bytes())
                            {
                                push(&events, id, NetEvent::Error(err));
                            }
                        }
                    }
                    recent.clear();
                } else if attempts > 0 {
                    // 已经代填过口令，而这次的数据里没有再次索要口令，说明已经进到 shell。
                    // 之后命令行里出现的 “password:” 属于用户自己的操作（例如 sudo），不再代填。
                    authenticated = true;
                } else if text.contains("(yes/no") {
                    // 老版本 ssh 或被已知主机冲突挡住时，代为确认一次。
                    if let Err(err) = write_session(&mut pty, b"yes\r") {
                        push(&events, id, NetEvent::Error(err));
                    }
                    recent.clear();
                }
            }
        }

        if let Some(ChildEvent::Exited(code)) = pty.next_child_event() {
            let status = code.and_then(|status| status.code());
            push(&events, id, NetEvent::Exited(status));
            break;
        }

        thread::sleep(if had_data { BUSY_POLL } else { IDLE_POLL });
    }

    if let Some(path) = key_path {
        discard_private_key(&path);
    }
}

/// 往伪控制台里写用户输入。
///
/// `Pty` 的 writer 底层是 `try_write`（内部管道满时返回 `Ok(0)`），直接用 `write_all`
/// 会把「管道暂时满」误判成错误——粘贴一大段文本或刚 resize 完就会撞上。这里改成
/// 带超时的重试，只有真的写不进去才报错。
fn write_session(pty: &mut tty::Pty, bytes: &[u8]) -> Result<(), String> {
    let mut offset = 0;
    let deadline = Instant::now() + WRITE_TIMEOUT;
    let mut spins = 0u32;

    while offset < bytes.len() {
        match pty.writer().write(&bytes[offset..]) {
            Ok(0) => {
                if Instant::now() >= deadline {
                    return Err("写入会话超时：伪控制台输入管道长时间不可写".to_owned());
                }
                // 管道背后有独立线程在搬运，让出时间片即可；连续空转几次再降级为小睡。
                spins += 1;
                if spins > 64 {
                    thread::sleep(Duration::from_millis(1));
                } else {
                    thread::yield_now();
                }
            }
            Ok(written) => offset += written,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(1));
            }
            Err(err) => return Err(format!("写入会话失败：{err}")),
        }
    }

    // flush 失败意味着最后一段可能没进管道，不能当成功返回。
    pty.writer()
        .flush()
        .map_err(|err| format!("刷新会话输出失败：{err}"))
}

fn push(events: &Arc<Mutex<Vec<(ConnId, NetEvent)>>>, id: ConnId, event: NetEvent) {
    // 锁中毒说明某个线程持锁时 panic 过；事件队列本身没有需要维持的不变量，
    // 取回内部值继续用，比静默丢事件（界面永远等不到回显）好得多。
    let mut guard = events
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    guard.push((id, event));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(password: bool) -> Target {
        Target {
            host: "10.0.0.5".to_owned(),
            port: 22,
            user: Some("ops".to_owned()),
            password: password.then(|| "pw".to_owned()),
            private_key: None,
        }
    }

    #[test]
    fn a_private_key_is_passed_as_identity_only() {
        let args = arguments(&target(false), Some(Path::new("C:\\Temp\\key")));

        assert!(args.windows(2).any(|pair| pair == ["-i", "C:\\Temp\\key"]));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["-o", "IdentitiesOnly=yes"]));
        assert!(!args
            .iter()
            .any(|arg| arg.starts_with("PreferredAuthentications")));
    }

    #[test]
    fn key_plus_password_uses_publickey_first_then_password() {
        let args = arguments(&target(true), Some(Path::new("C:\\Temp\\key")));

        assert!(args.iter().any(|arg| {
            arg == "PreferredAuthentications=publickey,password,keyboard-interactive"
        }));
    }

    #[test]
    fn password_only_prefers_password_auth() {
        let args = arguments(&target(true), None);

        assert!(args
            .iter()
            .any(|arg| arg == "PreferredAuthentications=password,keyboard-interactive,publickey"));
        assert!(!args.iter().any(|arg| arg == "-i"));
    }
}
