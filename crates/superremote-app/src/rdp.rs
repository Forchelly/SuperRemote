//! RDP：生成 `.rdp` 文件、把凭据交给系统凭据管理器，再拉起 `mstsc.exe`。
//!
//! 画面不交给独立窗口：`mstsc` 的 `TscShellContainerClass` 会被
//! `rdp_host` 用 `SetParent` 挂到 Iced 主窗口里，RDP 标签内就是远程桌面。
//! 这样既保留系统 RDP 的剪贴板、多显示器、音频与证书处理，又不需要在 Rust
//! 里重新实现 ActiveX 宿主或 RDP 协议栈。

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use process::HostProcess;

use superremote_core::session::{RdpDesktopSize, RdpPerformance, ResolvedRdpSettings, Session};
use superremote_core::tab::TabId;

use crate::theme;
use superremote_win::contract;
use superremote_win::rdp_host;

/// 一个宿主进程，可承载多路会话。最后一个会话释放时进程才结束。
pub struct Host {
    process: RefCell<HostProcess>,
    pub pid: u32,
    /// 命令通道窗口；按需查找并缓存，句柄失效后重新找。
    control: Cell<Option<isize>>,
    /// 当前挂在这个宿主上的会话数（不是 `Rc` 强引用数：`shared_host` 也持有
    /// 一份引用，不能拿来判断「最后一路」）。
    sessions: Cell<usize>,
    /// true = 只承载一路会话的独立宿主。
    pub isolated: bool,
}

pub type HostRef = Rc<Host>;

impl Host {
    fn new(process: HostProcess, isolated: bool) -> HostRef {
        Rc::new(Self {
            pid: process.id(),
            process: RefCell::new(process),
            control: Cell::new(None),
            sessions: Cell::new(1),
            isolated,
        })
    }

    /// 宿主进程是否已经退出，以及退出码。
    pub fn try_wait(&self) -> Option<u32> {
        self.process.borrow().try_wait()
    }

    pub fn sessions(&self) -> usize {
        self.sessions.get()
    }

    fn add_session(&self) {
        self.sessions.set(self.sessions.get() + 1);
    }

    fn control(&self) -> Option<isize> {
        if let Some(control) = self.control.get() {
            if rdp_host::is_window(control) {
                return Some(control);
            }
            self.control.set(None);
        }
        let control = rdp_host::find_activex_control(self.pid)?;
        self.control.set(Some(control));
        Some(control)
    }

    /// 释放一路会话：先让宿主自己关掉对应窗口；最后一路才结束进程。
    fn release(&self, session: &str, window: Option<isize>) {
        let remaining = self.sessions.get().saturating_sub(1);
        self.sessions.set(remaining);

        if let Some(control) = self.control() {
            let payload = close_payload(session);
            // `WM_COPYDATA` 必须同步等宿主把数据拷走，宿主 GUI 线程卡住（UAC
            // 蓝底那类场景）时这条调用会把 Iced 主线程一起拖住。关闭命令不需要
            // 回执：丢到后台线程发；发不出去就当宿主已经死了，由下面的进程
            // 收尾兜底。`add` 命令仍走同步——它的返回值决定要不要回退起新宿主。
            let _ = std::thread::spawn(move || {
                rdp_host::send_command(control, &payload);
            });
        } else if let Some(window) = window {
            rdp_host::request_close(window);
        }

        if remaining == 0 {
            let mut process = self.process.borrow_mut();
            process.kill();
            process.wait();
        }
    }
}

fn close_payload(session: &str) -> String {
    format!("{}\n{}", contract::COMMAND_CLOSE, session)
}

fn add_payload(session: &str, args: &[String]) -> String {
    format!(
        "{}\n{}\n{}",
        contract::COMMAND_ADD,
        session,
        args.join("\n")
    )
}

/// 一路已经拉起的 RDP 会话，以及它被托管后的窗口句柄。
pub struct RdpProcess {
    pub host: HostRef,
    /// 会话 id，与宿主窗口标题里的那一段一致。
    pub session: String,
    /// 会话窗口。窗口出现前是 `None`，由 `App::poll_rdp` 异步发现。
    pub window: Option<isize>,
    /// 会话窗口是否已经显示过。只有显示过才敢在我们这边再次 `ShowWindow`，
    /// 否则会在 mstsc 还在准备凭据框时抢先亮出一块黑屏。
    pub live: bool,
    /// 上一次贴边用的矩形。`SetWindowPos` 是跨进程调用，轮询时没必要每 150ms
    /// 重推一次同样的坐标。
    pub rect: Option<rdp_host::Rect>,
    /// 当前是否由我们显示着。隐藏/显示只在状态变化时调用。
    pub shown: bool,
    /// 要不要把 mstsc 的会话窗口挂进我们的窗口。「在新窗口打开」的 RDP
    /// 走 `false`：画面是 mstsc 自己的原生窗口，我们只观察它的存活。
    pub embed: bool,
    /// true = 内置 C# AxHost 宿主直连（不走 mstsc、不写 `.rdp`）。
    pub activex: bool,
    profile: PathBuf,
    /// `close` 可能从 `Drop` 与显式回收两条路进来，只释放一次。
    released: bool,
}

impl RdpProcess {
    pub fn is_live(&self) -> bool {
        self.live
    }

    /// 关闭这一路会话并清掉临时 `.rdp`。共享宿主里只关自己的窗口，
    /// 最后一个会话释放时才结束宿主进程。重复调用是安全的。
    pub fn close(&mut self) {
        if self.released {
            return;
        }
        self.released = true;

        let window = self.window.take();
        self.host.release(&self.session, window);

        // 收尾是「尽力而为」：`.rdp` 可能被 mstsc 打开着（删不掉）。失败
        // 没有补救动作，也不要盖掉真正的连接错误。
        if !self.profile.as_os_str().is_empty() {
            let _ = std::fs::remove_file(&self.profile);
        }
    }
}

impl Drop for RdpProcess {
    fn drop(&mut self) {
        self.close();
    }
}

/// 临时文件目录：`.rdp` 与 SSH 私钥都放这里。
///
/// 目录名**不跟着产品改名**：升级后旧目录里还可能有上次异常退出留下的私钥与
/// 宿主副本，换名字会让它们永远留在盘上、也不再有清理逻辑扫到。
fn temp_dir() -> PathBuf {
    std::env::temp_dir().join("SimpleRemote2")
}

/// 排查用时序日志：与 C# 宿主的 `rdp-host-<pid>.log` 放同一目录，复现一次即可
/// 按时间戳对齐。默认开启、2MB 封顶、写失败一律忽略，不做逐帧打点。
///
/// 句柄只开一次并常驻：宿主日志按 2 秒心跳与事件打点，旧行为每行 open/close
/// 一次文件纯属浪费；封顶检查退化为持句柄的一次元数据查询。
pub(crate) fn trace(line: &str) {
    use std::io::Write;
    use std::sync::{Mutex, OnceLock};
    use std::time::{SystemTime, UNIX_EPOCH};

    const MAX_BYTES: u64 = 2 * 1024 * 1024;
    static SINK: OnceLock<Option<Mutex<std::fs::File>>> = OnceLock::new();

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    // 与 C# 侧 DateTime.Now 同一时间轴（中国时区 UTC+8）。
    let local = (now.as_secs() + 8 * 3600) % 86_400;
    let stamp = format!(
        "{:02}:{:02}:{:02}.{:03}",
        local / 3600,
        (local % 3600) / 60,
        local % 60,
        now.subsec_millis()
    );

    let Some(sink) = SINK.get_or_init(|| {
        let path = temp_dir().join("rdp-app.log");
        // 首次调用可能早于任何 `.rdp`/私钥落盘：目录自己建，别依赖别人的创建顺序。
        let _ = std::fs::create_dir_all(temp_dir());
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .ok()
            .map(Mutex::new)
    }) else {
        return;
    };
    let Ok(mut file) = sink.lock() else {
        return;
    };
    if file.metadata().is_ok_and(|meta| meta.len() > MAX_BYTES) {
        return;
    }
    let _ = writeln!(file, "{stamp} {line}");
}

/// 2 秒一次：证明 Iced 主线程还在跳，并跨进程探活每个 RDP 宿主窗口。
/// 调用方必须传真实 `process.window`，不能拿 Iced 父窗口代替。蓝色卡顿期间
/// 仍在打点 = 主线程没被挂住；`title-len=0` 连续出现 = 宿主 GUI 线程没响应。
pub(crate) fn trace_tick(windows: &[isize]) {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static LAST: AtomicU64 = AtomicU64::new(0);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    if now.saturating_sub(LAST.load(Ordering::Relaxed)) < 2000 {
        return;
    }
    LAST.store(now, Ordering::Relaxed);
    for (index, window) in windows.iter().enumerate() {
        let (elapsed, len) = rdp_host::probe(*window);
        trace(&format!(
            "tick window[{index}]={window} probe-ms={elapsed} title-len={len}"
        ));
    }
    trace(&format!("tick windows={}", windows.len()));
}

/// 清掉上次异常退出留下的临时文件。
///
/// release 档是 `panic = "abort"`：任何 panic 都不会跑 `Drop`，于是
/// `RdpProcess` 来不及删自己的 `.rdp`、SSH 线程也来不及擦私钥。这种情况只能
/// 在下一次启动时补上。只动比 `max_age` 更旧的文件——另开一个实例正在用的
/// 文件不能被误删。
pub fn clean_stale_temp_files(max_age: Duration) {
    sweep_temp_dir(&temp_dir(), max_age);
}

fn sweep_temp_dir(dir: &Path, max_age: Duration) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let Ok(modified) = entry.metadata().and_then(|metadata| metadata.modified()) else {
            continue;
        };
        let Ok(age) = std::time::SystemTime::now().duration_since(modified) else {
            continue;
        };

        if age >= max_age {
            superremote_net::discard_secret_file(&entry.path());
        }
    }
}

/// 生成 RDP 文件并启动系统远程桌面。
///
/// `viewport` 是内容区的物理像素尺寸，会写进 `.rdp` 的 `desktopwidth/height`，
/// 让远端分辨率与标签页可用区域一致，而不是固定 1280×800 再缩放。
pub fn launch(
    session: &Session,
    password: Option<&str>,
    viewport: Option<(i32, i32)>,
    settings: &ResolvedRdpSettings,
    embed: bool,
) -> Result<RdpProcess, String> {
    let path = write_profile(session, viewport, settings)?;

    if let (Some(user), Some(password)) = (session.username.as_deref(), password) {
        // 凭据交给系统凭据管理器，mstsc 才能免提示登录。
        // 这里失败不阻断启动：`prompt for credentials` 已关，但 mstsc 仍会弹自己的
        // 登录框，用户手输一次照样能进。所以只吞掉错误，不改变主流程。
        if let Err(err) = store_credential(&session.host, user, password) {
            eprintln!("写入系统凭据失败（将退回手输）：{err}");
        }
    }

    // 嵌入模式必须**隐藏启动**：mstsc 会先建一个独立窗口，等我们 SetParent
    // 挂进标签后再由 `App::poll_rdp` 显示。直接可见启动会先闪出一个独立窗口，
    // 旧版 ActiveX 没有这一步，用户看到的就是「连接时弹了个窗」。
    let child = HostProcess::spawn(&path, embed).inspect_err(|_| {
        let _ = std::fs::remove_file(&path);
    })?;

    // 旧 mstsc 路径不再进入共享宿主，按独立宿主回收。
    let host = Host::new(child, true);

    Ok(RdpProcess {
        host,
        session: String::new(),
        window: None,
        live: false,
        rect: None,
        shown: false,
        embed,
        activex: false,
        profile: path,
        released: false,
    })
}

/// 内置 C# AxHost 宿主（`host/Program.cs`，由 build.rs 编译并嵌入）。
/// 它与旧项目走同一条 ActiveX 直连路径：不写 `.rdp`，因此不会触发 2026-04
/// 的「未签名 .rdp」确认框；安全确认与证书级别在宿主里显式关闭。
#[cfg(windows)]
static HELPER_EXE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/SuperRemote.Host.exe"));
#[cfg(not(windows))]
static HELPER_EXE: &[u8] = &[];

/// 新起一个内置 ActiveX 宿主，并承载第一路会话。
///
/// 凭据仍然先写系统凭据管理器（宿主用 `CredReadW("TERMSRV/<host>")` 取），
/// 不经过命令行/临时文件。宿主窗口先出现在屏幕外，Rust 侧的 `poll_rdp`
/// 发现后 `SetParent` 进标签内容区；窗口标题里的 `SUPERREMOTE|...` 是状态通道。
///
/// `isolated` 为 `true` 时这个宿主只服务这一路会话；为 `false` 时它成为
/// 共用宿主，后续标签走 [`add_activex_session`]。
pub fn launch_activex(
    instance: TabId,
    session: &Session,
    password: Option<&str>,
    viewport: Option<(i32, i32)>,
    settings: &ResolvedRdpSettings,
    isolated: bool,
) -> Result<RdpProcess, String> {
    let helper = ensure_helper()?;

    if let (Some(user), Some(password)) = (session.username.as_deref(), password) {
        if let Err(err) = store_credential(&session.host, user, password) {
            eprintln!("写入系统凭据失败（宿主将退回提示登录）：{err}");
        }
    }

    let (width, height) = activex_size(settings, viewport);
    let key = session_key(instance);
    let args = activex_args(&key, session, settings, width, height);
    let child = HostProcess::spawn_program(&helper, &args, false)?;
    let host = Host::new(child, isolated);

    Ok(RdpProcess {
        host,
        session: key,
        window: None,
        live: false,
        rect: None,
        shown: false,
        embed: true,
        activex: true,
        profile: PathBuf::new(),
        released: false,
    })
}

/// 让一个已经在跑的共用宿主再承载一路会话（`add` 命令）。
///
/// 返回错误表示宿主已经退出或命令通道失效，调用方应改起一个新宿主。
pub fn add_activex_session(
    host: &HostRef,
    instance: TabId,
    session: &Session,
    password: Option<&str>,
    viewport: Option<(i32, i32)>,
    settings: &ResolvedRdpSettings,
) -> Result<RdpProcess, String> {
    if host.isolated || host.sessions() == 0 || host.try_wait().is_some() {
        return Err("共用宿主已退出".to_owned());
    }

    // 宿主刚起来时命令窗口可能还在创建：短暂重试，避免第二个标签因为这次
    // 时序竞争而误判共享宿主不可用、另起一个进程。
    let mut control = host.control();
    for _ in 0..10 {
        if control.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
        control = host.control();
    }
    let control = control.ok_or_else(|| "共用宿主尚未建立命令窗口".to_owned())?;

    if let (Some(user), Some(password)) = (session.username.as_deref(), password) {
        if let Err(err) = store_credential(&session.host, user, password) {
            eprintln!("写入系统凭据失败（宿主将退回提示登录）：{err}");
        }
    }

    let (width, height) = activex_size(settings, viewport);
    let key = session_key(instance);
    let args = activex_args(&key, session, settings, width, height);
    if !rdp_host::send_command(control, &add_payload(&key, &args)) {
        return Err("共用宿主命令发送失败".to_owned());
    }
    host.add_session();

    Ok(RdpProcess {
        host: Rc::clone(host),
        session: key,
        window: None,
        live: false,
        rect: None,
        shown: false,
        embed: true,
        activex: true,
        profile: PathBuf::new(),
        released: false,
    })
}

fn activex_size(settings: &ResolvedRdpSettings, viewport: Option<(i32, i32)>) -> (i32, i32) {
    let fixed = match settings.desktop_size {
        RdpDesktopSize::Fullscreen => superremote_win::display::primary_size(),
        other => other.pixels(),
    };
    let (width, height) = fixed
        .map(|(width, height)| (width as i32, height as i32))
        .or(viewport)
        .unwrap_or((1280, 800));
    (width.clamp(640, 7680), height.clamp(480, 4320))
}

/// 会话 id：会进入宿主窗口标题（`SUPERREMOTE|<session>|...`），也会在将来的
/// 共享宿主里用来认领窗口。只用数字加前缀，`|` 与空白都不可能出现。
pub fn session_key(instance: TabId) -> String {
    format!("r{}", instance.get())
}

fn activex_args(
    session_key: &str,
    session: &Session,
    settings: &ResolvedRdpSettings,
    width: i32,
    height: i32,
) -> Vec<String> {
    let args = vec![
        contract::ARG_SESSION.to_owned(),
        session_key.to_owned(),
        contract::ARG_SERVER.to_owned(),
        single_line(&session.host),
        contract::ARG_PORT.to_owned(),
        session.port.to_string(),
        contract::ARG_USER.to_owned(),
        single_line(session.username.as_deref().unwrap_or_default()),
        contract::ARG_WIDTH.to_owned(),
        width.to_string(),
        contract::ARG_HEIGHT.to_owned(),
        height.to_string(),
        // 原生窗体在画面小于客户区时露出的那圈底色。与 Iced 侧的
        // `theme::rdp_fill()` 同源（同一个主题变量），拖边换帧时才不会跳色。
        contract::ARG_BACKDROP.to_owned(),
        theme::css_hex(theme::rdp_fill()),
        // 关掉 SmartSizing：开着时 mstsc 既不缩放（实测画面仍按 1:1 绘制），
        // 也不画滚动条，只把画面居中裁进控件；关掉后控件小于桌面时才会出现
        // 滚动条，画面保持 1:1 左上对齐。控件尺寸由宿主夹在「客户区」与
        // 「远端分辨率」之间，见 `host/Program.cs` 的 `CoverClient()`。
        contract::ARG_SMART_SIZING.to_owned(),
        bool_arg(false),
        contract::ARG_COLOR.to_owned(),
        settings.color_depth.bits().to_string(),
        contract::ARG_AUDIO.to_owned(),
        settings.audio.rdp_value().to_string(),
        contract::ARG_AUDIO_CAPTURE.to_owned(),
        bool_arg(settings.audio_capture),
        contract::ARG_KEYBOARD.to_owned(),
        settings.keyboard.rdp_value().to_string(),
        contract::ARG_PERFORMANCE.to_owned(),
        settings.performance.rdp_value().to_string(),
        contract::ARG_REDIRECT_PRINTERS.to_owned(),
        bool_arg(settings.redirect_printers),
        contract::ARG_REDIRECT_CLIPBOARD.to_owned(),
        bool_arg(settings.redirect_clipboard),
        contract::ARG_REDIRECT_SMARTCARDS.to_owned(),
        bool_arg(settings.redirect_smartcards),
        contract::ARG_REDIRECT_PORTS.to_owned(),
        bool_arg(settings.redirect_ports),
        contract::ARG_REDIRECT_DRIVES.to_owned(),
        bool_arg(settings.redirect_drives),
        // 透传控件版本；该参数只选择协议与图形能力，见 `RdpGraphics`。
        contract::ARG_RDP_CONTROL.to_owned(),
        settings.graphics.control().to_owned(),
    ];
    args
}

fn bool_arg(value: bool) -> String {
    if value {
        "1".to_owned()
    } else {
        "0".to_owned()
    }
}

fn ensure_helper() -> Result<PathBuf, String> {
    ensure_helper_in(&temp_dir())
}

fn ensure_helper_in(dir: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|err| format!("无法创建临时目录：{err}"))?;
    let path = dir.join("SuperRemote.Host.exe");
    if std::fs::read(&path).ok().as_deref() == Some(HELPER_EXE) {
        return Ok(path);
    }

    match std::fs::write(&path, HELPER_EXE) {
        Ok(()) => Ok(path),
        Err(err) => {
            // 这个固定文件名可能正被另一个实例的旧宿主占着（运行中的 exe
            // 不允许覆盖）。**不能将就着用旧副本**：旧宿主的窗口标题前缀与
            // 当前版本不一致，界面会永远停在「正在建立会话」。改成按内容
            // 取名的副本，旧文件等占用它的进程退出后由清理逻辑回收。
            let fallback = dir.join(format!("SuperRemote.Host.{:08x}.exe", helper_tag()));
            if std::fs::read(&fallback).ok().as_deref() != Some(HELPER_EXE) {
                std::fs::write(&fallback, HELPER_EXE).map_err(|write| {
                    format!("无法释放 RDP 宿主（{err}；备用名同样失败：{write}）")
                })?;
            }
            Ok(fallback)
        }
    }
}

/// 嵌入宿主内容的小哈希，只用于给被锁住的旧副本另取一个文件名。
fn helper_tag() -> u32 {
    let mut hash = 0x811c_9dc5u32;
    for byte in HELPER_EXE {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

/// 清掉由本程序写入的 RDP 凭据。
///
/// 返回 `Err` 时凭据**仍然留在系统里**，调用方不能报「已清除」。
pub fn forget_credentials(host: &str) -> Result<(), String> {
    platform::delete_credential(&credential_target(host))
}

fn store_credential(host: &str, user: &str, password: &str) -> Result<(), String> {
    platform::write_credential(&credential_target(host), user, password)
}

/// 系统凭据管理器里的目标名。mstsc 只认 `TERMSRV/<host>` 这个形状。
fn credential_target(host: &str) -> String {
    format!("TERMSRV/{}", single_line(host))
}

/// 把用户输入压成单行。
///
/// 两处需要：`.rdp` 是逐行解析的键值对，`host`/`username` 里混进换行就等于让
/// 用户凭空插一条指令进去；凭据目标名同理，多一个控制字符就查不到同一条记录。
fn single_line(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).collect()
}

#[cfg(windows)]
mod platform {
    use windows_sys::Win32::Foundation::{GetLastError, ERROR_NOT_FOUND};
    use windows_sys::Win32::Security::Credentials::{
        CredDeleteW, CredWriteW, CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC,
    };

    pub fn write_credential(target: &str, user: &str, password: &str) -> Result<(), String> {
        let mut target_w = wide(target);
        let mut user_w = wide(user);
        // 口令按 UTF-16 存，系统会把它加密进凭据库；这样命令行里不会留下明文。
        let mut blob: Vec<u16> = password.encode_utf16().collect();

        let credential = CREDENTIALW {
            Type: CRED_TYPE_GENERIC,
            TargetName: target_w.as_mut_ptr(),
            CredentialBlobSize: (blob.len() * std::mem::size_of::<u16>()) as u32,
            CredentialBlob: blob.as_mut_ptr().cast::<u8>(),
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            UserName: user_w.as_mut_ptr(),
            ..CREDENTIALW::default()
        };

        // SAFETY: `credential` 里的三个指针（`TargetName`、`UserName`、`CredentialBlob`）
        // 分别指向上面几个本地 `Vec` 的堆缓冲，它们在本次调用期间不会被移动或释放
        // （`target_w`/`user_w`/`blob` 都活到函数末尾）；`CredentialBlobSize` 与
        // `blob` 的实际字节数一致。`CredWriteW` 只读这块内存，不接管所有权。
        let written = unsafe { CredWriteW(&credential, 0) };

        if written == 0 {
            // SAFETY: `GetLastError` 无参数、无前置条件，读取调用线程的 last-error。
            return Err(format!("CredWrite 失败（Win32 错误 {}）", unsafe {
                GetLastError()
            }));
        }

        Ok(())
    }

    pub fn delete_credential(target: &str) -> Result<(), String> {
        let target_w = wide(target);

        // SAFETY: `target_w` 是以 NUL 结尾的 UTF-16 缓冲，活到调用结束；
        // `CredDeleteW` 只读它，不保留指针。
        let deleted = unsafe { CredDeleteW(target_w.as_ptr(), CRED_TYPE_GENERIC, 0) };

        if deleted != 0 {
            return Ok(());
        }

        // SAFETY: 同 `write_credential`——只是读取 last-error。
        let code = unsafe { GetLastError() };
        if code == ERROR_NOT_FOUND {
            // 本来就没有这条凭据，对调用方来说「已经清干净了」是同一个结果。
            return Ok(());
        }

        Err(format!("CredDelete 失败（Win32 错误 {code}）"))
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }
}

#[cfg(not(windows))]
mod platform {
    pub fn write_credential(_target: &str, _user: &str, _password: &str) -> Result<(), String> {
        Err("系统凭据管理器只在 Windows 上可用".to_owned())
    }

    pub fn delete_credential(_target: &str) -> Result<(), String> {
        Err("系统凭据管理器只在 Windows 上可用".to_owned())
    }
}

/// `.rdp` 里不随连接变化的固定项。
///
/// 分辨率、颜色、音频、组合键与重定向都由 [`write_profile`] 按生效设置写。
/// `authentication level` 刻意取 0：连接管理器里主机是用户显式保存的，
/// 取 2 时 mstsc 对每个自签证书主机都会弹「无法验证发布者」安全警告，
/// 卡住内嵌流程（对话框要手点才能进）。风险是不校验服务器身份，
/// 与旧版 ActiveX 直连的行为一致。
const PROFILE_FIXED: &str = concat!(
    "prompt for credentials:i:0\r\n",
    "promptcredentialonce:i:0\r\n",
    "authentication level:i:0\r\n",
    "screen mode id:i:1\r\n",
    "smart sizing:i:1\r\n",
    "connect to console:i:0\r\n",
    "autoreconnection enabled:i:1\r\n",
);

/// 生成 `.rdp`。对单元测试公开：分辨率这类「设置怎么变成文件」的规则，
/// 只有读到文件内容才能断言，靠启动 mstsc 是验不了的。
pub(crate) fn write_profile(
    session: &Session,
    viewport: Option<(i32, i32)>,
    settings: &ResolvedRdpSettings,
) -> Result<PathBuf, String> {
    let dir = temp_dir();
    std::fs::create_dir_all(&dir).map_err(|err| format!("无法创建临时目录：{err}"))?;

    // 先把用户输入压成单行再落盘：`.rdp` 逐行解析，一个换行就能插进一条指令。
    let host = single_line(&session.host);

    // 会话 id 也要进文件名：同一个主机允许建多条连接，并行启动时不能互相覆盖
    // 正在被 mstsc 读取的 `.rdp`。单元测试并行跑到同一个主机时同样靠它隔离。
    let path = dir.join(format!("{}-{}.rdp", sanitize(&host), sanitize(&session.id)));

    // 「全屏」要主显示器的当前分辨率：内容区几何算出来的是**窗口**尺寸，进全屏
    // 之前它还带着侧栏与标题条的扣减，直接拿来当远端分辨率会小一圈。
    let fixed = match settings.desktop_size {
        RdpDesktopSize::Fullscreen => superremote_win::display::primary_size(),
        other => other.pixels(),
    };
    let (width, height) = fixed
        .map(|(width, height)| (width as i32, height as i32))
        .or(viewport)
        .unwrap_or((1280, 800));
    let follow_window = settings.desktop_size == RdpDesktopSize::FollowWindow;
    let width = width.clamp(640, 7680);
    let height = height.clamp(480, 4320);
    let auto_network = settings.performance == RdpPerformance::Auto;

    // 预留够一条连接串加固定项的空间，省掉中途扩容。
    let mut profile = String::with_capacity(256 + PROFILE_FIXED.len());
    profile.push_str(&format!("full address:s:{}:{}\r\n", host, session.port));
    if let Some(user) = session.username.as_deref() {
        if !user.is_empty() {
            profile.push_str(&format!("username:s:{}\r\n", single_line(user)));
        }
    }
    profile.push_str(&format!("desktopwidth:i:{width}\r\n"));
    profile.push_str(&format!("desktopheight:i:{height}\r\n"));
    profile.push_str(&format!(
        "dynamic resolution:i:{}\r\n",
        if follow_window { 1 } else { 0 }
    ));
    profile.push_str(&format!(
        "connection type:i:{}\r\n",
        settings.performance.rdp_value()
    ));
    profile.push_str(&format!(
        "networkautodetect:i:{}\r\n",
        if auto_network { 1 } else { 0 }
    ));
    profile.push_str(&format!(
        "bandwidthautodetect:i:{}\r\n",
        if auto_network { 1 } else { 0 }
    ));
    profile.push_str(&format!(
        "session bpp:i:{}\r\n",
        settings.color_depth.bits()
    ));
    profile.push_str(&format!("audiomode:i:{}\r\n", settings.audio.rdp_value()));
    profile.push_str(&format!(
        "audiocapturemode:i:{}\r\n",
        if settings.audio_capture { 1 } else { 0 }
    ));
    profile.push_str(&format!(
        "keyboardhook:i:{}\r\n",
        settings.keyboard.rdp_value()
    ));
    profile.push_str(&format!(
        "redirectprinters:i:{}\r\n",
        if settings.redirect_printers { 1 } else { 0 }
    ));
    profile.push_str(&format!(
        "redirectclipboard:i:{}\r\n",
        if settings.redirect_clipboard { 1 } else { 0 }
    ));
    profile.push_str(&format!(
        "redirectsmartcards:i:{}\r\n",
        if settings.redirect_smartcards { 1 } else { 0 }
    ));
    profile.push_str(&format!(
        "redirectcomports:i:{}\r\n",
        if settings.redirect_ports { 1 } else { 0 }
    ));
    profile.push_str(&format!(
        "redirectdrives:i:{}\r\n",
        if settings.redirect_drives { 1 } else { 0 }
    ));
    if settings.redirect_drives {
        profile.push_str("drivestoredirect:s:*\r\n");
    }
    profile.push_str(PROFILE_FIXED);

    std::fs::write(&path, profile).map_err(|err| format!("无法写入 RDP 配置：{err}"))?;
    Ok(path)
}

/// 文件名里不能带路径分隔符、冒号与控制字符，其余字符原样保留便于排查。
fn sanitize(host: &str) -> String {
    let cleaned: String = single_line(host)
        .chars()
        .map(|c| match c {
            ':' | '/' | '\\' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            other => other,
        })
        .collect();

    if cleaned.is_empty() {
        "connection".to_owned()
    } else {
        cleaned
    }
}

/// mstsc 进程句柄。
///
/// 嵌入模式用 `CreateProcessW` + `STARTF_USESHOWWINDOW/SW_HIDE` 启动，
/// 让 mstsc 的会话窗口在挂进标签之前保持不可见；非嵌入模式（独立窗口）
/// 仍然正常显示。`std::process::Command` 无法指定 `STARTUPINFO` 的显示方式，
/// 所以这里直接走 Win32。
mod process {
    #[cfg(windows)]
    mod imp {
        use std::ffi::c_void;
        use std::path::Path;
        use std::sync::OnceLock;

        use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, STILL_ACTIVE};
        use windows_sys::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
            SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        };
        use windows_sys::Win32::System::Threading::{
            CreateProcessW, GetExitCodeProcess, TerminateProcess, WaitForSingleObject,
            PROCESS_INFORMATION, STARTF_USESHOWWINDOW, STARTUPINFOW,
        };
        use windows_sys::Win32::UI::WindowsAndMessaging::{SW_HIDE, SW_SHOWNORMAL};

        /// 所有宿主进程共用的作业对象（句柄以 `usize` 存，0 表示创建失败）。
        ///
        /// 句柄**故意不关**：它跟着进程一起消失，系统随即按
        /// `KILL_ON_JOB_CLOSE` 终止作业内的宿主。这样 release 档
        /// `panic = "abort"` 或进程被强杀时，也不会再留下没人认领的宿主。
        static HOST_JOB: OnceLock<usize> = OnceLock::new();

        fn host_job() -> Option<HANDLE> {
            let slot = HOST_JOB.get_or_init(|| unsafe { create_host_job() } as usize);
            (*slot != 0).then_some(*slot as HANDLE)
        }

        /// SAFETY: 只在 `HOST_JOB` 首次初始化时调用一次。
        unsafe fn create_host_job() -> HANDLE {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return job;
            }

            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let size = std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32;
            let ok = SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const c_void,
                size,
            );
            if ok == 0 {
                CloseHandle(job);
                return std::ptr::null_mut();
            }
            job
        }

        /// CommandLineToArgvW-compatible quoting for one argument.
        fn quote_arg(arg: &str) -> String {
            let needs_quotes = arg.is_empty() || arg.chars().any(|c| c.is_whitespace() || c == '"');
            if !needs_quotes {
                return arg.to_owned();
            }

            let mut out = String::with_capacity(arg.len() + 2);
            out.push('"');
            let mut backslashes = 0usize;
            for c in arg.chars() {
                match c {
                    '\\' => backslashes += 1,
                    '"' => {
                        out.push_str(&"\\".repeat(backslashes * 2 + 1));
                        out.push('"');
                        backslashes = 0;
                    }
                    _ => {
                        out.push_str(&"\\".repeat(backslashes));
                        out.push(c);
                        backslashes = 0;
                    }
                }
            }
            out.push_str(&"\\".repeat(backslashes * 2));
            out.push('"');
            out
        }

        pub struct HostProcess {
            handle: HANDLE,
            pid: u32,
        }

        impl HostProcess {
            pub fn spawn(profile: &Path, hidden: bool) -> Result<Self, String> {
                Self::spawn_program(
                    Path::new("mstsc.exe"),
                    &[profile.display().to_string()],
                    hidden,
                )
            }

            pub fn spawn_program(
                program: &Path,
                args: &[String],
                hidden: bool,
            ) -> Result<Self, String> {
                let mut command = format!("\"{}\"", program.display());
                for arg in args {
                    command.push(' ');
                    command.push_str(&quote_arg(arg));
                }
                let mut command: Vec<u16> = command.encode_utf16().collect();
                command.push(0);

                let startup = STARTUPINFOW {
                    cb: std::mem::size_of::<STARTUPINFOW>() as u32,
                    dwFlags: STARTF_USESHOWWINDOW,
                    wShowWindow: if hidden {
                        SW_HIDE as u16
                    } else {
                        SW_SHOWNORMAL as u16
                    },
                    ..Default::default()
                };
                let mut info = PROCESS_INFORMATION::default();

                // SAFETY: `command` 以 NUL 结尾，并在调用期间保持有效；其余指针都是
                // null（表示继承当前进程的环境与工作目录）。`startup`/`info` 是栈上的
                // 局部值，`CreateProcessW` 只读 `startup`、写 `info`。
                let created = unsafe {
                    CreateProcessW(
                        std::ptr::null(),
                        command.as_mut_ptr(),
                        std::ptr::null(),
                        std::ptr::null(),
                        0,
                        0,
                        std::ptr::null(),
                        std::ptr::null(),
                        &startup,
                        &mut info,
                    )
                };

                if created == 0 {
                    return Err(format!(
                        "无法启动系统远程桌面（Win32 错误 {}）",
                        std::io::Error::last_os_error()
                            .raw_os_error()
                            .unwrap_or_default()
                    ));
                }

                // 线程句柄用不到；进程句柄留到来收尾。
                // SAFETY: `info.hThread` 是 `CreateProcessW` 新建的、本函数拥有的句柄，
                // 关闭一次即可，不会与其它人重复关闭。
                unsafe {
                    CloseHandle(info.hThread);
                }

                // 绑定生命周期：主进程退出（含崩溃/被强杀）时宿主一起被系统终止。
                // 分配失败只意味着回到「宿主可能变孤儿」的旧行为，不影响会话本身。
                if let Some(job) = host_job() {
                    // SAFETY: 作业句柄与进程句柄均有效；`AssignProcessToJobObject`
                    // 只登记归属，不接管句柄所有权。
                    unsafe {
                        AssignProcessToJobObject(job, info.hProcess);
                    }
                }

                Ok(Self {
                    handle: info.hProcess,
                    pid: info.dwProcessId,
                })
            }

            pub fn id(&self) -> u32 {
                self.pid
            }

            /// `Some(code)` 表示进程已退出。没有子进程退出通知，只能轮询。
            pub fn try_wait(&self) -> Option<u32> {
                let mut code = 0u32;
                // SAFETY: `self.handle` 是本进程持有的有效进程句柄；
                // `GetExitCodeProcess` 只写入 `code`。
                let ok = unsafe { GetExitCodeProcess(self.handle, &mut code) };
                if ok != 0 && code != STILL_ACTIVE as u32 {
                    Some(code)
                } else {
                    None
                }
            }

            pub fn kill(&self) {
                // SAFETY: 句柄无效时 `TerminateProcess` 返回 0，不会崩溃。
                unsafe {
                    TerminateProcess(self.handle, 1);
                }
            }

            pub fn wait(&mut self) {
                if self.handle.is_null() {
                    return;
                }

                // SAFETY: 句柄有效；`WaitForSingleObject` 只等待，不接管所有权。
                // 等 2 秒就够收尾了——别让一个不响应的 mstsc 把界面线程挂住。
                // 超时也照样关句柄，并把字段置空，`Drop` 不会再关一次。
                unsafe {
                    WaitForSingleObject(self.handle, 2_000);
                    CloseHandle(self.handle);
                }
                self.handle = std::ptr::null_mut::<c_void>();
            }
        }

        impl Drop for HostProcess {
            fn drop(&mut self) {
                if !self.handle.is_null() {
                    // SAFETY: 只为尚未 `wait()` 的句柄兜底关闭一次。
                    unsafe {
                        CloseHandle(self.handle);
                    }
                }
            }
        }
    }

    #[cfg(not(windows))]
    mod imp {
        use std::path::Path;
        use std::process::Child;

        pub struct HostProcess {
            child: Child,
        }

        impl HostProcess {
            pub fn spawn(profile: &Path, _hidden: bool) -> Result<Self, String> {
                std::process::Command::new("mstsc.exe")
                    .arg(profile)
                    .spawn()
                    .map(|child| Self { child })
                    .map_err(|err| format!("无法启动系统远程桌面：{err}"))
            }

            pub fn spawn_program(
                program: &Path,
                args: &[String],
                _hidden: bool,
            ) -> Result<Self, String> {
                std::process::Command::new(program)
                    .args(args)
                    .spawn()
                    .map(|child| Self { child })
                    .map_err(|err| format!("无法启动 {}：{err}", program.display()))
            }

            pub fn id(&self) -> u32 {
                self.child.id()
            }

            pub fn try_wait(&self) -> Option<u32> {
                self.child
                    .try_wait()
                    .ok()
                    .flatten()
                    .map(|status| status.code().unwrap_or(1) as u32)
            }

            pub fn kill(&self) {
                // 非 Windows 上 RDP 不可用；这里只保证 API 形状一致。
            }

            pub fn wait(&mut self) {
                let _ = self.child.wait();
            }
        }
    }

    pub use imp::HostProcess;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(host: &str, user: Option<&str>) -> Session {
        Session {
            id: format!("id-{host}"),
            name: "跳板机".into(),
            kind: superremote_core::session::SessionKind::Rdp,
            host: host.into(),
            port: 3389,
            username: user.map(str::to_owned),
            settings: Default::default(),
            window: None,
        }
    }

    #[test]
    fn the_profile_carries_address_user_and_redirects() {
        let path = write_profile(
            &session("10.0.0.9", Some("admin")),
            Some((1600, 900)),
            &ResolvedRdpSettings::default(),
        )
        .expect("应能写文件");
        let text = std::fs::read_to_string(&path).unwrap();

        assert!(text.contains("full address:s:10.0.0.9:3389"));
        assert!(text.contains("username:s:admin"));
        // 免提示登录 + 剪贴板重定向是运维场景的基本要求。
        assert!(text.contains("prompt for credentials:i:0"));
        assert!(text.contains("redirectclipboard:i:1"));
        assert!(text.contains("screen mode id:i:1"));
        // 远端分辨率要跟随内嵌区域，而不是固定 1280×800。
        assert!(text.contains("desktopwidth:i:1600"));
        assert!(text.contains("desktopheight:i:900"));

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_host_with_separators_still_yields_a_plain_file_name() {
        let path = write_profile(
            &session("host:12/34", None),
            None,
            &ResolvedRdpSettings::default(),
        )
        .expect("应能写文件");
        assert_eq!(path.file_name().unwrap(), "host_12_34-id-host_12_34.rdp");
        let _ = std::fs::remove_file(path);
    }

    /// `.rdp` 是逐行解析的键值对，主机名/用户名里夹一个换行就等于能凭空插一条指令。
    #[test]
    fn control_characters_cannot_inject_extra_profile_directives() {
        let mut target = session("10.0.0.30", Some("ops\r\nalternate shell:s:cmd.exe"));
        target.host = "10.0.0.30\r\nredirectclipboard:i:1\r\ndrivestoredirect:s:C:\\".to_owned();

        let path =
            write_profile(&target, None, &ResolvedRdpSettings::default()).expect("应能写文件");
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.split("\r\n").collect();

        // 注入进去的内容必须留在原来的那一行里，不能变成独立指令。
        let count = |prefix: &str| lines.iter().filter(|line| line.starts_with(prefix)).count();
        assert_eq!(count("drivestoredirect:"), 0, "换行必须被剥掉：{lines:?}");
        assert_eq!(count("alternate shell:"), 0, "换行必须被剥掉：{lines:?}");
        // `redirectclipboard` 是本来就有的固定项，注入不该让它变成两条。
        assert_eq!(count("redirectclipboard:"), 1, "不能多出一条：{lines:?}");
        // 真实的固定项没有被顶掉，默认值仍然是默认值。
        assert!(lines.contains(&"redirectdrives:i:0"));
        assert!(lines.contains(&"prompt for credentials:i:0"));
        // 用户名只出现一次，且是单行。
        assert_eq!(count("username:s:"), 1);

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn the_credential_target_is_a_single_line() {
        assert_eq!(credential_target("10.0.0.9"), "TERMSRV/10.0.0.9");
        assert_eq!(credential_target("h\r\nost"), "TERMSRV/host");
    }

    #[test]
    fn stale_temp_files_are_swept_but_fresh_ones_are_left_alone() {
        let dir = std::env::temp_dir().join(format!("superremote-sweep-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let stale = dir.join("old.rdp");
        std::fs::write(&stale, b"full address:s:10.0.0.1:3389\r\n").unwrap();

        // `max_age = 0` 表示「全都是旧的」，等价于启动时看到的 24h 前那批文件。
        sweep_temp_dir(&dir, Duration::ZERO);

        assert!(!stale.exists(), "超时的临时文件应被清掉");

        // 反过来：时效内的文件必须留着，否则会误删另一个实例正在用的 `.rdp`。
        let fresh = dir.join("fresh.rdp");
        std::fs::write(&fresh, b"x").unwrap();
        sweep_temp_dir(&dir, Duration::from_secs(3600));
        assert!(fresh.exists(), "刚写出来的文件不能被删");

        let _ = std::fs::remove_file(fresh);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tiny_viewports_are_clamped_to_a_usable_remote_desktop() {
        let path = write_profile(
            &session("10.0.0.10", None),
            Some((10, 10)),
            &ResolvedRdpSettings::default(),
        )
        .expect("应能写文件");
        let text = std::fs::read_to_string(&path).unwrap();

        assert!(text.contains("desktopwidth:i:640"));
        assert!(text.contains("desktopheight:i:480"));

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn every_rdp_setting_reaches_the_profile() {
        let settings = ResolvedRdpSettings {
            host_mode: superremote_core::session::RdpHostMode::Shared,
            performance: RdpPerformance::Lan,
            color_depth: superremote_core::session::RdpColorDepth::Bpp16,
            audio: superremote_core::session::RdpAudio::None,
            audio_capture: true,
            keyboard: superremote_core::session::RdpKeyboard::Remote,
            redirect_printers: true,
            redirect_clipboard: false,
            redirect_smartcards: true,
            redirect_ports: true,
            redirect_drives: true,
            desktop_size: RdpDesktopSize::custom(1920, 1080),
            graphics: superremote_core::session::RdpGraphics::Compat,
        };

        let path = write_profile(
            &session("10.0.0.20", Some("ops")),
            Some((800, 600)),
            &settings,
        )
        .expect("应能写文件");
        let text = std::fs::read_to_string(&path).unwrap();

        // 固定分辨率优先于内容区尺寸，并关掉动态分辨率。
        assert!(text.contains("desktopwidth:i:1920"));
        assert!(text.contains("desktopheight:i:1080"));
        assert!(text.contains("dynamic resolution:i:0"));
        assert!(text.contains("connection type:i:6"));
        assert!(text.contains("networkautodetect:i:0"));
        assert!(text.contains("session bpp:i:16"));
        assert!(text.contains("audiomode:i:2"));
        assert!(text.contains("audiocapturemode:i:1"));
        assert!(text.contains("keyboardhook:i:1"));
        assert!(text.contains("redirectprinters:i:1"));
        assert!(text.contains("redirectclipboard:i:0"));
        assert!(text.contains("redirectsmartcards:i:1"));
        assert!(text.contains("redirectcomports:i:1"));
        assert!(text.contains("redirectdrives:i:1"));
        assert!(text.contains("drivestoredirect:s:*"));

        let _ = std::fs::remove_file(path);
    }

    /// 控件版本必须真的落到宿主命令行上；漏传会让宿主默认值覆盖连接设置。
    #[test]
    fn activex_args_carry_the_control_version() {
        let compat = ResolvedRdpSettings {
            desktop_size: RdpDesktopSize::custom(1920, 1080),
            ..ResolvedRdpSettings::default()
        };
        let joined = activex_args("r1", &session("10.0.0.9", None), &compat, 1920, 1080).join(" ");
        assert!(
            joined.contains("--rdp-control 10"),
            "默认（兼容档）应传旧版控件: {joined}"
        );

        let modern = ResolvedRdpSettings {
            graphics: superremote_core::session::RdpGraphics::Modern,
            ..compat
        };
        let joined = activex_args("r1", &session("10.0.0.9", None), &modern, 1920, 1080).join(" ");
        assert!(
            joined.contains("--rdp-control 11"),
            "新版应传 v11: {joined}"
        );
    }

    #[test]
    fn activex_args_carry_the_connection_and_desktop_size() {
        let args = activex_args(
            "r7",
            &session("10.0.0.9", Some("ops")),
            &ResolvedRdpSettings::default(),
            1600,
            900,
        );
        let joined = args.join(" ");
        // 会话 id 进标题，也决定宿主窗口归哪一路标签。
        assert!(joined.contains("--session r7"));
        assert!(joined.contains("--server 10.0.0.9"));
        assert!(joined.contains("--port 3389"));
        assert!(joined.contains("--user ops"));
        assert!(joined.contains("--width 1600"));
        assert!(joined.contains("--height 900"));
        assert!(joined.contains("--color 32"));
        // 关闭 SmartSizing，窗口小于桌面时才会出现滚动条。
        assert!(joined.contains("--smart-sizing 0"));
        // 留边底色由主题派生并交给宿主，两边同源才不会在拖边松手时跳色。
        assert!(joined.contains("--backdrop #"));
        // 绝不生成 .rdp 路径：ActiveX 直连不经过文件。
        assert!(!joined.contains(".rdp"));
    }

    /// 合成、主题与持久位图缓存保持旧版 C# 的默认体验，不能按网络位置裁剪。
    /// 这是视觉与兼容性约束，不是 UAC 卡顿的性能修复。
    #[test]
    fn rdp_experience_flags_keep_the_graphics_pipeline() {
        let host = include_str!("../host/Program.cs");
        assert!(
            !host.contains("EnableDesktopComposition = false"),
            "不能按网络位置关闭桌面合成"
        );
        assert!(
            !host.contains("DisableThemes = true"),
            "不能按网络位置禁用主题"
        );
        assert!(
            !host.contains("BitmapPersistence = _trimGraphics"),
            "持久位图缓存不能按目标 IP 关闭"
        );
        assert!(
            !host.contains("CachePersistenceActive = _trimGraphics"),
            "持久位图缓存不能按目标 IP 关闭"
        );
        assert!(
            host.contains("DesktopCompositionFlag = 0x100"),
            "PerformanceFlags 必须含 TS_PERF_ENABLE_DESKTOP_COMPOSITION(0x100)"
        );
    }

    /// 三种桌面尺寸都关 SmartSizing：开着时画面既不缩放也不出滚动条，
    /// 只被居中裁掉；关掉后「窗口小于桌面 → 滚动条、窗口大于桌面 → 补边」
    /// 这两种观感都由宿主的控件尺寸夹取保证。
    #[test]
    fn every_desktop_size_leaves_smart_sizing_off() {
        let fixed = ResolvedRdpSettings {
            desktop_size: RdpDesktopSize::custom(1920, 1080),
            ..ResolvedRdpSettings::default()
        };
        let joined = activex_args("r8", &session("10.0.0.9", None), &fixed, 1920, 1080).join(" ");
        assert!(
            joined.contains("--smart-sizing 0"),
            "固定分辨率也不能开 SmartSizing，否则缩小的窗口不出滚动条"
        );

        let follow = activex_args(
            "r9",
            &session("10.0.0.9", None),
            &ResolvedRdpSettings::default(),
            1600,
            900,
        )
        .join(" ");
        assert!(follow.contains("--smart-sizing 0"));

        let fullscreen = ResolvedRdpSettings {
            desktop_size: RdpDesktopSize::Fullscreen,
            ..ResolvedRdpSettings::default()
        };
        let joined =
            activex_args("r10", &session("10.0.0.9", None), &fullscreen, 1920, 1080).join(" ");
        assert!(joined.contains("--smart-sizing 0"));
    }

    #[test]
    fn activex_size_follows_viewport_or_explicit_desktop() {
        let defaults = ResolvedRdpSettings::default();
        assert_eq!(
            activex_size(&defaults, Some((1600, 900))),
            (1600, 900),
            "跟随窗口取内容区物理像素"
        );

        let fixed = ResolvedRdpSettings {
            desktop_size: RdpDesktopSize::custom(1920, 1080),
            ..ResolvedRdpSettings::default()
        };
        assert_eq!(
            activex_size(&fixed, Some((800, 600))),
            (1920, 1080),
            "固定分辨率优先于内容区尺寸"
        );
    }

    /// Rust 与 C# 宿主只有字符串约定，一端改名另一端不会报错，只会静默失效。
    /// 这里直接读宿主源码对账：Rust 发送的每个参数，宿主都必须真的读取。
    #[test]
    fn every_flag_rust_sends_is_read_by_the_host() {
        let read = host_reads(include_str!("../host/Program.cs"));
        for flag in contract::ARGS {
            let name = flag.trim_start_matches("--");
            assert!(
                read.iter().any(|read_name| read_name == name),
                "Program.cs 没有读取 Rust 发送的参数 {flag}"
            );
        }
    }

    #[test]
    fn the_host_does_not_read_flags_rust_never_sends() {
        for name in host_reads(include_str!("../host/Program.cs")) {
            let flag = format!("--{name}");
            assert!(
                contract::ARGS.contains(&flag.as_str()),
                "Program.cs 读取了 {flag}，但 Rust 侧的 ARGS 没列出它"
            );
        }
    }

    #[test]
    fn the_title_prefix_and_states_match_the_host_source() {
        let source = include_str!("../host/Program.cs");
        assert!(
            source.contains(&format!(
                "const string TitlePrefix = \"{}\";",
                contract::TITLE_PREFIX
            )),
            "Program.cs 的 TitlePrefix 与 TITLE_PREFIX 不一致"
        );
        for state in [
            contract::STATE_CONNECTING,
            contract::STATE_CONNECTED,
            contract::STATE_FAILED,
        ] {
            assert!(
                source.contains(&format!("\"{state}\"")),
                "Program.cs 没有产生状态 {state}"
            );
        }

        // 标题必须带会话段，否则共享宿主里的多个窗口无法分辨。
        assert!(
            source.contains(
                "internal static string Title(string session, string state, string message)"
            ),
            "Program.cs 的标题没有会话段"
        );
        assert!(
            source.contains(&format!(
                "internal const string ControlTitle = \"{}\";",
                contract::CONTROL_TITLE
            )),
            "Program.cs 的控制窗口标题与 CONTROL_TITLE 不一致"
        );
        assert!(
            source.contains(&format!(
                "internal const string CommandAdd = \"{}\";",
                contract::COMMAND_ADD
            )),
            "Program.cs 没有定义 add 命令"
        );
        assert!(
            source.contains(&format!(
                "internal const string CommandClose = \"{}\";",
                contract::COMMAND_CLOSE
            )),
            "Program.cs 没有定义 close 命令"
        );
    }

    /// 静态对账只能证明**源码**一致，抓不到「运行时用了旧宿主副本」：
    /// 2026-11-06 改名后临时目录里的旧 exe 被运行中的进程锁住，界面因此
    /// 一直停在「正在建立会话」。这条测试把本次构建嵌入的宿主真的跑起来，
    /// 要求它按契约标题发布会话与状态。
    #[test]
    #[cfg(windows)]
    fn the_running_host_publishes_a_parseable_session_title() {
        let helper = std::env::temp_dir().join(format!(
            "SuperRemote.contract-test.{}.exe",
            std::process::id()
        ));
        std::fs::write(&helper, HELPER_EXE).expect("释放测试宿主");

        let session = format!("t{}", std::process::id());
        let args = vec![
            contract::ARG_SESSION.to_owned(),
            session.clone(),
            contract::ARG_SERVER.to_owned(),
            "127.0.0.1".to_owned(),
            // 一个必然连不上的端口：只验证标题契约，不依赖真实会话。
            contract::ARG_PORT.to_owned(),
            "1".to_owned(),
            contract::ARG_WIDTH.to_owned(),
            "800".to_owned(),
            contract::ARG_HEIGHT.to_owned(),
            "600".to_owned(),
        ];
        let mut child = HostProcess::spawn_program(&helper, &args, false).expect("启动测试宿主");

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        let mut parsed = None;
        while std::time::Instant::now() < deadline {
            if let Some(window) = rdp_host::find_activex_window(child.id(), &session) {
                let title = rdp_host::window_title(window);
                if let Some(host) = contract::parse_title(&title) {
                    let found_session = host.session.map(str::to_owned);
                    let state = host.state.to_owned();
                    parsed = Some((title, found_session, state));
                    break;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }

        child.kill();
        // 必须先等进程对象消失：exe 文件在进程句柄释放前删不掉，
        // 否则每次跑测试都会在 %TEMP% 里留一份 18KB 的宿主副本。
        child.wait();
        let _ = std::fs::remove_file(&helper);

        let (title, found_session, state) =
            parsed.expect("宿主启动后 20 秒内必须发布可解析的会话标题");
        assert!(title.starts_with(contract::TITLE_PREFIX), "标题：{title}");
        assert_eq!(
            found_session.as_deref(),
            Some(session.as_str()),
            "标题：{title}"
        );
        assert!(
            [
                contract::STATE_CONNECTING,
                contract::STATE_CONNECTED,
                contract::STATE_FAILED,
            ]
            .contains(&state.as_str()),
            "标题：{title}"
        );
    }

    /// 固定名被别的实例占着（运行中的 exe 不可覆盖）时，必须另写一份
    /// 与本次构建内容一致的副本；绝不能继续复用旧副本。
    #[test]
    #[cfg(windows)]
    // Windows 上只读位就是「不可覆盖」，解除它没有 Unix 的权限放宽语义。
    #[allow(clippy::permissions_set_readonly_false)]
    fn a_locked_helper_is_replaced_by_a_content_named_copy() {
        let dir =
            std::env::temp_dir().join(format!("SuperRemote.helper-test.{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建测试目录");
        let fixed = dir.join("SuperRemote.Host.exe");
        std::fs::write(&fixed, b"stale helper").expect("写入旧副本");
        let mut permissions = std::fs::metadata(&fixed).expect("读权限").permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&fixed, permissions).expect("设为只读，模拟被占用");

        let helper = ensure_helper_in(&dir).expect("旧副本不可写时必须另写一份");
        assert_ne!(helper, fixed, "不能返回不可写的旧副本");
        assert_eq!(
            std::fs::read(&helper).expect("读新副本").as_slice(),
            HELPER_EXE,
            "新副本必须是本次构建的宿主"
        );
        assert!(
            helper.file_name().is_some_and(|name| name
                .to_string_lossy()
                .contains(&format!("{:08x}", helper_tag()))),
            "备用名要按内容区分：{}",
            helper.display()
        );

        let mut permissions = std::fs::metadata(&fixed).expect("读权限").permissions();
        permissions.set_readonly(false);
        let _ = std::fs::set_permissions(&fixed, permissions);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `Program.cs` 里通过 `Args.Get/Int/Bool` 读取的参数名。
    fn host_reads(source: &str) -> Vec<String> {
        let mut names = Vec::new();
        for needle in ["args.Get(\"", "args.Int(\"", "args.Bool(\""] {
            let mut rest = source;
            while let Some(start) = rest.find(needle) {
                let after = &rest[start + needle.len()..];
                let Some(end) = after.find('"') else {
                    break;
                };
                names.push(after[..end].to_owned());
                rest = &after[end..];
            }
        }
        names.sort();
        names.dedup();
        names
    }
}
