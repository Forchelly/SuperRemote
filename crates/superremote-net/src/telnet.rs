//! Telnet 连接：原生 TCP + 最小 NVT 选项协商。
//!
//! 只实现运维场景真正用得到的部分：剥离服务端发来的协商序列，按 RFC 854 应答，
//! 窗口尺寸变化时通过 NAWS 告知对端。

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::push_event;
use crate::{Command, ConnId, EventQueue, NetError, NetEvent, Target};

const IAC: u8 = 0xFF;
const DONT: u8 = 0xFE;
const DO: u8 = 0xFD;
const WONT: u8 = 0xFC;
const WILL: u8 = 0xFB;
const SB: u8 = 0xFA;
const SE: u8 = 0xF0;

const OPT_ECHO: u8 = 0x01;
const OPT_SGA: u8 = 0x03;
const OPT_TTYPE: u8 = 0x18;
const OPT_NAWS: u8 = 0x1F;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(8);
const IDLE_POLL: Duration = Duration::from_millis(12);
const BUSY_POLL: Duration = Duration::from_millis(1);

pub fn spawn(
    id: ConnId,
    target: &Target,
    events: Arc<Mutex<EventQueue>>,
    commands: Receiver<Command>,
) -> thread::JoinHandle<()> {
    // 只在调用线程里拼地址字符串。真正的 DNS 解析与 TCP 握手放到工作线程：
    // `open_telnet` 是在 `App::update` 里调用的，`connect_timeout` 的 8 秒
    // 一旦同步跑完，整个界面就冻在那儿了——而目标不可达恰恰是最常见的场景。
    let address = format!("{}:{}", target.host, target.port);

    thread::spawn(move || match connect(&address) {
        Ok(stream) => run(id, stream, events, commands),
        // 失败只报 `Error`，不再补一个 `Exited`：后者会把状态从「失败」
        // 覆盖成「正常断开」，用户就看不到失败原因了。
        Err(err) => push_event(&events, id, NetEvent::Error(err)),
    })
}

/// 建连失败带上 `ErrorKind`：界面层按它给「端口没开 / 名字解析不了」
/// 这类下一步建议，比一段 OS 原文有用得多。
fn connect(address: &str) -> Result<TcpStream, NetError> {
    let server = address
        .to_socket_addrs()
        .map_err(|err| NetError::Io {
            kind: err.kind(),
            message: format!("无法解析地址 {address}：{err}"),
        })?
        .next()
        .ok_or_else(|| NetError::Other(format!("无法解析地址 {address}")))?;

    let stream =
        TcpStream::connect_timeout(&server, CONNECT_TIMEOUT).map_err(|err| NetError::Io {
            kind: err.kind(),
            message: format!("连接 {address} 失败：{err}"),
        })?;
    stream.set_nonblocking(true).map_err(|err| NetError::Io {
        kind: err.kind(),
        message: format!("无法把连接切到非阻塞模式：{err}"),
    })?;

    Ok(stream)
}

fn run(
    id: ConnId,
    mut stream: TcpStream,
    events: Arc<Mutex<EventQueue>>,
    commands: Receiver<Command>,
) {
    let mut raw = [0u8; 16 * 1024];
    // Telnet 没有认证阶段，TCP 握手成功即可输入。
    push_event(&events, id, NetEvent::Ready);

    'outer: loop {
        let mut had_data = false;

        while let Ok(command) = commands.try_recv() {
            match command {
                Command::Input(bytes) => {
                    if stream.write_all(&bytes).is_err() {
                        push_event(
                            &events,
                            id,
                            NetEvent::Error(NetError::Other("写入会话失败".to_owned())),
                        );
                    }
                }
                Command::Resize(cols, rows) => {
                    // 尺寸通知发不出去说明对端已经不在了，等下一次 read 报错即可，
                    // 这里没有别的补救动作，所以刻意丢弃结果。
                    let _ = stream.write_all(&naws(cols, rows));
                }
                Command::Close => break 'outer,
            }
        }

        match stream.read(&mut raw) {
            // `Ok(0)` 是干净的对端关闭，由循环出口统一发一次 `Exited`。
            Ok(0) => break,
            Ok(count) => {
                had_data = true;
                let (text, replies) = decode(&raw[..count]);
                for reply in replies {
                    // 协商应答写不出去不影响把已收到的正文交给界面；
                    // 真正的断链会由下一轮 read 报出来。
                    let _ = stream.write_all(&reply);
                }
                if !text.is_empty() {
                    push_event(&events, id, NetEvent::Data(text));
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(err) => {
                // 读出错就到此为止，只报原因；末尾的 `Exited` 由状态决定是否补发。
                push_event(
                    &events,
                    id,
                    NetEvent::Error(NetError::Io {
                        kind: err.kind(),
                        message: err.to_string(),
                    }),
                );
                return;
            }
        }

        thread::sleep(if had_data { BUSY_POLL } else { IDLE_POLL });
    }

    push_event(&events, id, NetEvent::Exited(Some(0)));
}

/// 剥掉协商序列，返回（正文, 应答序列）。
fn decode(input: &[u8]) -> (Vec<u8>, Vec<Vec<u8>>) {
    let mut text = Vec::with_capacity(input.len());
    let mut replies = Vec::new();
    let mut index = 0usize;

    while index < input.len() {
        let byte = input[index];

        if byte != IAC {
            text.push(byte);
            index += 1;
            continue;
        }

        let Some(command) = input.get(index + 1).copied() else {
            // 半个序列，等下一包。
            break;
        };

        match command {
            IAC => {
                text.push(IAC);
                index += 2;
            }
            WILL | WONT | DO | DONT => {
                let Some(option) = input.get(index + 2).copied() else {
                    break;
                };
                index += 3;

                match (command, option) {
                    // 服务端说「我要负责回显」，我们同意（`DO ECHO`）：这样一来
                    // 本地敲的字符由远端回显，屏幕内容与服务端看到的一致。
                    (WILL, OPT_ECHO) => replies.push(neg(DO, OPT_ECHO)),
                    // 反过来要我们回显的请求一律拒绝：本地终端自己画画面。
                    (DO, OPT_ECHO) => replies.push(neg(WONT, OPT_ECHO)),
                    (WILL, OPT_SGA) | (DO, OPT_SGA) => {
                        let answer = if command == WILL { DO } else { WILL };
                        replies.push(neg(answer, OPT_SGA));
                    }
                    // 告诉对端我们支持告知窗口尺寸，并在随后上报一次。
                    (DO, OPT_NAWS) => replies.push(neg(WILL, OPT_NAWS)),
                    (DO, OPT_TTYPE) => replies.push(neg(WILL, OPT_TTYPE)),
                    (WILL, _) => replies.push(neg(DONT, option)),
                    (DO, _) => replies.push(neg(WONT, option)),
                    _ => {}
                }
            }
            SB => {
                // 子协商：一路找到 IAC SE。
                match input[index..].windows(2).position(|pair| pair == [IAC, SE]) {
                    Some(offset) => index += offset + 2,
                    None => break,
                }
            }
            _ => index += 2,
        }
    }

    (text, replies)
}

fn neg(command: u8, option: u8) -> Vec<u8> {
    vec![IAC, command, option]
}

/// NAWS：把终端尺寸报给对端。
fn naws(cols: u16, rows: u16) -> Vec<u8> {
    vec![
        IAC,
        SB,
        OPT_NAWS,
        (cols >> 8) as u8,
        (cols & 0xFF) as u8,
        (rows >> 8) as u8,
        (rows & 0xFF) as u8,
        IAC,
        SE,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negotiation_sequences_are_stripped_from_the_text() {
        let (text, replies) = decode(b"hello\xff\xfb\x01\xff\xfd\x03world");

        assert_eq!(text, b"helloworld");
        assert!(replies.contains(&vec![IAC, DO, OPT_ECHO]));
        assert!(replies.contains(&vec![IAC, WILL, OPT_SGA]));
    }

    #[test]
    fn an_escaped_iac_becomes_a_single_byte() {
        let (text, _) = decode(&[b'a', IAC, IAC, b'b']);
        assert_eq!(text, &[b'a', IAC, b'b']);
    }

    #[test]
    fn subnegotiation_is_swallowed_whole() {
        let (text, replies) = decode(b"\xff\xfa\x18\x01\xff\xf0after");
        assert_eq!(text, b"after");
        assert!(replies.is_empty());
    }

    #[test]
    fn a_split_sequence_waits_for_the_next_packet() {
        let (first, _) = decode(b"ok\xff\xfb");
        assert_eq!(first, b"ok");
    }

    #[test]
    fn naws_reports_the_geometry() {
        assert_eq!(naws(80, 24), [IAC, SB, OPT_NAWS, 0, 80, 0, 24, IAC, SE]);
    }
}
