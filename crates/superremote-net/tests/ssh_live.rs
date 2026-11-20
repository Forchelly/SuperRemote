//! 对着真实 SSH 服务端的集成测试。
//!
//! 默认跳过：凭据不进仓库，只有显式给了环境变量才跑。
//!
//! ```text
//! SUPERREMOTE_SSH_HOST=127.0.0.2 SUPERREMOTE_SSH_PORT=22 SUPERREMOTE_SSH_USER=user SUPERREMOTE_SSH_PASS=*** \
//!     cargo test -p superremote-net --test ssh_live -- --nocapture --test-threads=1
//! ```

use std::time::{Duration, Instant};

use superremote_net::{ConnId, Hub, NetEvent, Target};

struct Live {
    host: String,
    port: u16,
    user: String,
    password: String,
}

fn live_target() -> Option<Live> {
    let host = std::env::var("SUPERREMOTE_SSH_HOST").ok()?;
    let user = std::env::var("SUPERREMOTE_SSH_USER").ok()?;
    let password = std::env::var("SUPERREMOTE_SSH_PASS").ok()?;
    let port = std::env::var("SUPERREMOTE_SSH_PORT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(22);
    Some(Live {
        host,
        port,
        user,
        password,
    })
}

const TEXT: &str = "$ ";

/// 连接会话的测试包装：把「累积正文 + 抽干事件」收成一处。
struct Session {
    hub: Hub,
    id: ConnId,
    seen: Vec<u8>,
}

impl Session {
    fn open(live: &Live, cols: u16, rows: u16) -> Self {
        let mut hub = Hub::new();
        let id = hub
            .open_ssh(
                &Target {
                    host: live.host.clone(),
                    port: live.port,
                    user: Some(live.user.clone()),
                    password: Some(live.password.clone()),
                    private_key: None,
                },
                cols,
                rows,
            )
            .expect("应能启动 ssh 客户端");
        Self {
            hub,
            id,
            seen: Vec::new(),
        }
    }

    /// 抽干事件队列并累积正文，直到条件成立或超时。
    fn wait(&mut self, timeout: Duration, done: impl Fn(&str) -> bool) -> (String, bool) {
        let deadline = Instant::now() + timeout;
        loop {
            for (from, event) in self.hub.drain() {
                if from != self.id {
                    continue;
                }
                match event {
                    NetEvent::Data(chunk) => self.seen.extend_from_slice(&chunk),
                    NetEvent::Ready => {}
                    NetEvent::Error(message) => panic!("连接层报错：{message}"),
                    NetEvent::Exited(code) => {
                        panic!("会话提前退出（{code:?}）：{}", self.text())
                    }
                }
            }
            let text = self.text();
            if done(&text) {
                return (text, true);
            }
            if Instant::now() >= deadline {
                return (text, false);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.seen).into_owned()
    }

    /// 让远端报一次 `stty size`。返回 `(rows, cols)`。
    ///
    /// 标记写成 `SZ=$(...)`，命令回显里不会出现解析得出的字面量，避免把回显当成结果。
    fn remote_size(&mut self) -> Option<(u16, u16)> {
        self.seen.clear();
        self.hub
            .send_text(self.id, "echo \"SZ=$(stty size | tr ' ' '-')\"\r");

        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            let _ = self.wait(Duration::from_millis(50), |_| false);
            // 回显里也有 "SZ=" 字样，从后往前找第一个能解析的。
            for rest in self.text().rsplit("SZ=") {
                let token: String = rest
                    .chars()
                    .take_while(|c| c.is_ascii_digit() || *c == '-')
                    .collect();
                if let Some((rows, cols)) = token.split_once('-') {
                    if let (Ok(rows), Ok(cols)) = (rows.parse(), cols.parse()) {
                        return Some((rows, cols));
                    }
                }
            }
            if Instant::now() >= deadline {
                return None;
            }
        }
    }
}

#[test]
fn logs_in_runs_a_command_and_syncs_the_window_size() {
    let Some(live) = live_target() else {
        eprintln!(
            "跳过：未设置 SUPERREMOTE_SSH_HOST / SUPERREMOTE_SSH_USER / SUPERREMOTE_SSH_PASS"
        );
        return;
    };

    let mut session = Session::open(&live, 80, 24);

    // 1) 登录：口令由连接层代填，等到 shell 提示符出现。
    let (login, ok) = session.wait(Duration::from_secs(30), |text| text.contains(TEXT));
    assert!(ok, "登录超时，收到的内容是：{login:?}");

    // 2) 真实执行命令。`SR-<user>-END` 由远端 shell 求值产生，回显里不会出现同样的字面量。
    session
        .hub
        .send_text(session.id, "printf 'SR-%s-END\\n' \"$(id -un)\"\r");
    let marker = format!("SR-{}-END", live.user);
    let (output, ok) = session.wait(Duration::from_secs(20), |text| text.contains(&marker));
    assert!(ok, "没等到命令输出 {marker:?}，收到：{output:?}");

    // 3) 窗口尺寸。
    //    ConPTY 一侧一定生效；但 Win32-OpenSSH 会合并、延迟窗口变化事件，
    //    单次采样经常读到上一次的尺寸，所以这里按「最终收敛」断言。
    let mut observed = None;
    for _ in 0..5 {
        session.hub.resize(session.id, 100, 30);
        std::thread::sleep(Duration::from_millis(700));
        observed = session.remote_size();
        if observed == Some((30, 100)) {
            break;
        }
    }
    assert_eq!(
        observed,
        Some((30, 100)),
        "PTY 尺寸最终应同步为 30 行 100 列"
    );

    session.hub.close(session.id);
    session.hub.forget(session.id);
    assert!(session.hub.is_empty());
}

#[test]
fn a_bad_password_ends_the_session_instead_of_hanging() {
    let Some(live) = live_target() else {
        eprintln!(
            "跳过：未设置 SUPERREMOTE_SSH_HOST / SUPERREMOTE_SSH_USER / SUPERREMOTE_SSH_PASS"
        );
        return;
    };

    let mut hub = Hub::new();
    let id = hub
        .open_ssh(
            &Target {
                host: live.host,
                port: live.port,
                user: Some(live.user),
                password: Some("definitely-not-the-password".to_owned()),
                private_key: None,
            },
            80,
            24,
        )
        .expect("应能启动 ssh 客户端");

    let deadline = Instant::now() + Duration::from_secs(30);
    let mut exited = false;
    let mut seen = String::new();
    while Instant::now() < deadline && !exited {
        for (from, event) in hub.drain() {
            if from != id {
                continue;
            }
            match event {
                NetEvent::Data(chunk) => seen.push_str(&String::from_utf8_lossy(&chunk)),
                NetEvent::Exited(_) => exited = true,
                NetEvent::Ready | NetEvent::Error(_) => {}
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }

    assert!(exited, "口令错误时应结束会话，收到：{seen:?}");
    hub.forget(id);
}
