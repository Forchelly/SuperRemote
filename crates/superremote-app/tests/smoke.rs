//! 端到端冒烟：真实 SSH 连接 → 终端网格 → 键盘输入 → 远端回显。
//!
//! 不打开窗口，直接驱动 `App::update`，所以能在 CI/命令行走完整条链路。
//! 默认跳过；给了环境变量才跑：
//!
//! ```text
//! SUPERREMOTE_SSH_HOST=127.0.0.2 SUPERREMOTE_SSH_PORT=22 SUPERREMOTE_SSH_USER=user SUPERREMOTE_SSH_PASS=*** \
//!     cargo test -p superremote-app --test smoke -- --nocapture
//! ```
//!
//! 设 `SUPERREMOTE_SMOKE_DIR` 可以把造出来的数据目录留在磁盘上，给 GUI 截图复用。

use std::path::PathBuf;
use std::time::{Duration, Instant};

use iced::keyboard::{key, Key, Location, Modifiers};
use superremote_app::app::{DraftField, Message};
use superremote_app::App;
use superremote_core::session::SessionKind;
use superremote_store::DataPaths;

const TIMEOUT: Duration = Duration::from_secs(40);

struct Live {
    host: String,
    port: u16,
    user: String,
    password: String,
}

fn live() -> Option<Live> {
    Some(Live {
        host: std::env::var("SUPERREMOTE_SSH_HOST").ok()?,
        port: std::env::var("SUPERREMOTE_SSH_PORT")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(22),
        user: std::env::var("SUPERREMOTE_SSH_USER").ok()?,
        password: std::env::var("SUPERREMOTE_SSH_PASS").ok()?,
    })
}

/// 数据目录：默认用一次性临时目录，`SUPERREMOTE_SMOKE_DIR` 指定时保留下来。
///
/// 每个用例必须拿到**互不相同**的目录：`cargo test` 默认并行跑，
/// 共用一个目录会互相 `remove_dir_all`，表现成随机超时。
fn data_dir(tag: &str) -> (DataPaths, bool) {
    let (dir, temporary) = match std::env::var("SUPERREMOTE_SMOKE_DIR") {
        Ok(dir) => (PathBuf::from(dir).join(tag), false),
        Err(_) => (
            std::env::temp_dir().join(format!("sr-smoke-{}-{tag}", std::process::id())),
            true,
        ),
    };

    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    (DataPaths::from_dir(&dir), temporary)
}

fn send(app: &mut App, message: Message) {
    let _ = app.update(message);
}

/// 反复拉取连接事件，直到条件成立。
fn pump_until(app: &mut App, what: &str, done: impl Fn(&App) -> bool) {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        send(app, Message::NetPoll);
        if done(app) {
            return;
        }
        if Instant::now() >= deadline {
            let screen = app
                .active_terminal()
                .map(|terminal| terminal.screen_text())
                .unwrap_or_else(|| "(没有活动终端)".to_owned());
            panic!("{what} 超时。当前屏幕：\n{screen}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn type_text(app: &mut App, text: &str) {
    let event = iced::keyboard::Event::KeyPressed {
        key: Key::Character(text.into()),
        modified_key: Key::Character(text.into()),
        physical_key: key::Physical::Code(key::Code::KeyA),
        location: Location::Standard,
        modifiers: Modifiers::NONE,
        text: None,
        repeat: false,
    };
    send(app, Message::KeyInput(Box::new(event)));
}

#[test]
fn a_real_ssh_session_shows_output_and_accepts_typed_commands() {
    let Some(live) = live() else {
        eprintln!(
            "跳过：未设置 SUPERREMOTE_SSH_HOST / SUPERREMOTE_SSH_USER / SUPERREMOTE_SSH_PASS"
        );
        return;
    };

    let (paths, temporary) = data_dir("typed-commands");
    let mut app = App::with_paths(paths.clone());

    // 1) 像用户那样把连接建出来。
    send(&mut app, Message::NewSession(SessionKind::Ssh));
    send(
        &mut app,
        Message::DraftChanged(DraftField::Name, "冒烟用例".to_owned()),
    );
    send(
        &mut app,
        Message::DraftChanged(DraftField::Host, live.host.clone()),
    );
    send(
        &mut app,
        Message::DraftChanged(DraftField::Port, live.port.to_string()),
    );
    send(
        &mut app,
        Message::DraftChanged(DraftField::Username, live.user.clone()),
    );
    send(
        &mut app,
        Message::DraftChanged(DraftField::Password, live.password.clone()),
    );
    send(&mut app, Message::ApplyDraft);

    // 2) 打开连接，等远端提示符出现。
    send(&mut app, Message::OpenSelected);
    pump_until(&mut app, "登录", |app| {
        app.active_terminal()
            .is_some_and(|terminal| terminal.status.is_live())
    });

    let screen = app.active_terminal().unwrap().screen_text();
    assert!(
        screen.contains('$') || screen.contains('#') || screen.contains('~'),
        "屏幕上应出现 shell 提示符，实际内容：\n{screen}"
    );

    // 3) 键盘输入要真的送到远端：标记由远端求值产生，回显里不会出现同样的字面量。
    type_text(&mut app, "printf 'SMOKE-%s\\n' ok\r");
    pump_until(&mut app, "命令回显", |app| {
        app.active_terminal()
            .is_some_and(|terminal| terminal.screen_text().contains("SMOKE-ok"))
    });

    // 4) 视口变化要重排网格并同步给远端。
    let instance = app.active_terminal_instance().expect("应有活动终端");
    send(&mut app, Message::TerminalResized(instance, 100, 30));
    let terminal = app.active_terminal().unwrap();
    assert_eq!((terminal.cols, terminal.rows), (100, 30));
    assert_eq!(terminal.frame.rows.len(), 30);
    assert_eq!(terminal.frame.rows[0].len(), 100);

    // 5) 关闭标签要把连接一起收掉，不能留下悬空线程。
    send(&mut app, Message::CloseTab(0));
    assert!(app.active_terminal().is_none());

    if temporary {
        let _ = std::fs::remove_dir_all(paths.dir());
    } else {
        eprintln!("数据目录保留在 {}", paths.dir().display());
    }
}

/// 没有保存口令的会话，必须能对着 `password:` 提示手输口令登录。
///
/// 这条链路很容易做错：口令提示出来时会话还不算「已连接」，
/// 若此时不接管键盘，用户既看不到提示也没地方输入，只能永远停在「正在连接」。
#[test]
fn an_unsaved_password_can_still_be_typed_at_the_prompt() {
    let Some(live) = live() else {
        eprintln!(
            "跳过：未设置 SUPERREMOTE_SSH_HOST / SUPERREMOTE_SSH_USER / SUPERREMOTE_SSH_PASS"
        );
        return;
    };

    let (paths, temporary) = data_dir("typed-password");
    let mut app = App::with_paths(paths.clone());

    send(&mut app, Message::NewSession(SessionKind::Ssh));
    send(
        &mut app,
        Message::DraftChanged(DraftField::Name, "手输口令".to_owned()),
    );
    send(
        &mut app,
        Message::DraftChanged(DraftField::Host, live.host.clone()),
    );
    send(
        &mut app,
        Message::DraftChanged(DraftField::Port, live.port.to_string()),
    );
    send(
        &mut app,
        Message::DraftChanged(DraftField::Username, live.user.clone()),
    );
    // 关键：故意不填口令。
    send(&mut app, Message::ApplyDraft);
    send(&mut app, Message::OpenSelected);

    pump_until(&mut app, "口令提示", |app| {
        app.active_terminal()
            .is_some_and(|terminal| terminal.screen_text().contains("assword:"))
    });

    let terminal = app.active_terminal().expect("应已有终端");
    assert!(!terminal.status.is_live(), "口令还没输入完，不能算已连接");
    assert!(
        terminal.accepts_input(),
        "停在口令提示上必须接管键盘，否则用户没法输入"
    );

    for ch in format!("{}\r", live.password).chars() {
        type_text(&mut app, &ch.to_string());
    }

    pump_until(&mut app, "手输口令后登录", |app| {
        app.active_terminal()
            .is_some_and(|terminal| terminal.status.is_live())
    });

    let screen = app.active_terminal().unwrap().screen_text();
    assert!(
        screen.contains('$') || screen.contains('#'),
        "登录后应出现 shell 提示符，实际内容：\n{screen}"
    );

    send(&mut app, Message::CloseTab(0));

    if temporary {
        let _ = std::fs::remove_dir_all(paths.dir());
    }
}
