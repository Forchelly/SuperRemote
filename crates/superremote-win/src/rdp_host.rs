//! 把 `mstsc` 的 RDP 会话窗口托管进 Iced 主窗口。
//!
//! 旧版用 MSTSCLib ActiveX 控件，Rust 侧没有等价的进程内宿主；但 Iced 0.14
//! 暴露了 `window::run` 与 `raw-window-handle`，我们能拿到主窗口的 `HWND`。
//! 因此这里用最轻的 Win32 方案：等 `mstsc` 建出 `TscShellContainerClass`
//! 顶级窗口后，`SetParent` 成主窗口的子窗口，再去掉标题栏、贴到标签内容区。
//!
//! 这些调用全部在主窗口线程（`update`）里发生，避免跨线程 `SetParent`
//! 与消息循环互相等待。

use crate::layout as ui;

/// mstsc 真正承载远程画面的窗口类名。进程的顶层窗口里还有一个
/// `TSC_POPUP_PARENT_WNDCLASS` 与各类对话框，不能拿它们做宿主。
const SESSION_CLASS: &str = "TscShellContainerClass";

/// 内容区在物理像素下的矩形。宽高按 `GetClientRect` 计算，不做缩放假设。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// RDP 画面贴边时要扣掉的窗口装饰。三种宿主的装饰不同，几何必须分开算。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneLayout {
    /// 主窗口标签内容区：侧栏右侧、标签条与会话页头之下。
    Main,
    /// 独立窗口：只有顶部标题条（`detached_bar`）。
    Detached,
    /// 全屏：只剩顶部退出栏。
    Fullscreen,
}

/// 窗口里留给 RDP 画面的区域。
///
/// 布局常量的来源见 [`logical_rect`]；全屏与独立窗口只有顶部一条栏的差别。
/// 让主线程以 mixed-DPI 承载模式创建窗口。
///
/// 父窗口是 Iced/winit 的 Per-Monitor V2，而 mstsc 的会话窗口是另一套
/// `DPI_AWARENESS_CONTEXT`。不声明 mixed hosting 时，`SetParent` 会把子窗口
/// 做 DPI 虚拟化，两边的系统光标按不同缩放比绘制，鼠标划过 RDP 区域会一大一小
/// 地跳。这个属性只对**之后新建**的窗口生效，所以必须在 `window::open` 之前
/// 调用（见 `App::boot` 的第一行），不能等到 `embed` 时才设。
pub fn enable_mixed_dpi_hosting() {
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::HiDpi::{
            SetThreadDpiHostingBehavior, DPI_HOSTING_BEHAVIOR_MIXED,
        };

        // SAFETY: 无参数内存；只改当前线程的 DPI 承载属性。
        unsafe {
            SetThreadDpiHostingBehavior(DPI_HOSTING_BEHAVIOR_MIXED);
        }
    }
}

pub fn pane_rect(iced: isize, layout: PaneLayout) -> Rect {
    let (client_w, client_h, scale) = client_metrics(iced);
    logical_rect(client_w, client_h, scale, layout)
}

/// 窗口左上角（**逻辑**像素，桌面坐标）。句柄失效时返回 `None`。
///
/// 弹层菜单要落在鼠标点击处，而控件上报的锚点是窗口内坐标，必须先知道窗口在哪。
/// 无边框窗口没有非客户区，外框左上角就是内容区左上角，所以这个值可以直接用。
pub fn window_origin(hwnd: isize) -> Option<(f32, f32)> {
    platform::window_origin(hwnd)
}

/// 登记「这个父窗口要同步带着走哪些子窗口」，并给父窗口装上贴边的窗口过程。
///
/// 手动拖边缩放走 `begin_resize`（自己跟鼠标，`update` 每步都跑），这段由
/// `reposition` 主动跳过——拖动期间不碰子窗口，见 `is_dragging`。但最大化时
/// 仍会退回 `window::drag_resize`：它是 `PostMessageW(WM_NCLBUTTONDOWN)` →
/// `DefWindowProc` 进**系统模态缩放循环**，循环期间 Iced 的 `update` 一行都不跑，
/// 尺寸事件全部攒到松手才送达。窗口过程是那条循环里唯一还会被调用的代码，
/// 所以贴边在这里保留，子窗口不会停在旧矩形上露出底色。
///
/// 每次布局同步都重给一份完整名单，因此子窗口关掉后不需要单独注销。
pub fn set_sizing_children(parent: isize, children: Vec<(isize, PaneLayout)>) {
    sizing::set_children(parent, children);
}

/// 窗口要没了：注销名单并还原窗口过程。句柄值可能被系统复用给新窗口，
/// 留着旧表项就会对新窗口做错误的事情。
pub fn clear_sizing_window(parent: isize) {
    sizing::clear(parent);
}

/// 开始一次手动拖边缩放（见 `sizing` 模块的说明）。返回 `false` 表示这个窗口
/// 不能手动缩放（最大化、句柄失效或非 Windows），调用方应退回系统模态缩放。
pub fn begin_resize(
    window: isize,
    direction: iced::window::Direction,
    min_size: (f32, f32),
) -> bool {
    sizing::begin_resize(window, direction, min_size)
}

/// 拖动期替代原生画面的静态帧。
///
/// 按 1:1 物理像素、锚定在画面原来的位置上呈现：窗口变大时多出来的地方
/// 留底色，窗口变小时超出画面区的部分被裁掉，任何时候都不缩放。
#[derive(Clone)]
pub struct FrozenFrame {
    pub handle: iced_core::image::Handle,
    /// 抓帧时的物理像素尺寸。
    pub width: u32,
    pub height: u32,
    /// 抓帧时的窗口缩放因子（DPI / 96）。
    pub scale: f32,
    /// 画面区左上角相对「画面占位区」左上角的逻辑偏移。
    pub offset: (f32, f32),
}

/// 拖动期间替代原生画面的冻结帧：拖动开始前抓一张屏幕图，Iced 按原始像素
/// 贴在原位置；松手后子窗口被摆回原位并强制重绘，这张图即失效。
///
/// 只有正在拖边时才返回 `Some`，常规刷新不会去碰它。
pub fn frozen_frame() -> Option<FrozenFrame> {
    sizing::frozen_frame()
}

/// 只给「正在拖的那个窗口」返回冻结帧。
///
/// 同一时刻最多只有一个拖边，帧是进程级单例；主窗口页面按它画覆盖层时
/// 必须确认帧属于自己，否则拖独立窗口也会把那张图糊到主窗口上。
pub fn frozen_frame_for(parent: isize) -> Option<FrozenFrame> {
    sizing::frozen_frame_for(parent)
}

/// 只做几何换算，方便在没有窗口的单元测试里验证基准线。
///
/// 画面区按 `ui::RESIZE_EDGE` 让开左/右/下三条边：原生子窗口永远画在 Iced
/// 之上，压住边缘的话，那几条缩放热区就再也收不到按下了（右下角拖不动的真实
/// 成因）。顶部不用让——标题栏本来就是 Iced 的。全屏没有可拖的边，铺满。
pub fn logical_rect(client_w: i32, client_h: i32, scale: f32, layout: PaneLayout) -> Rect {
    let bar = bar_height(layout);
    let inset = match layout {
        PaneLayout::Fullscreen => 0,
        PaneLayout::Main | PaneLayout::Detached => (ui::RESIZE_EDGE * scale).round() as i32,
    };
    let y = (bar * scale).round() as i32;

    Rect {
        x: inset,
        y,
        width: (client_w - 2 * inset).max(1),
        height: (client_h - y - inset).max(1),
    }
}

/// 三种宿主顶部让出的栏高。`logical_rect` 与覆盖层定位必须用同一个值。
fn bar_height(layout: PaneLayout) -> f32 {
    match layout {
        PaneLayout::Main => ui::TITLEBAR_H,
        PaneLayout::Detached | PaneLayout::Fullscreen => ui::FULLSCREEN_BAR_H,
    }
}

/// 画面区左上角相对**窗口原点**的逻辑偏移，与 [`logical_rect`] 同源同算。
///
/// 主窗口会话页的覆盖层按它定位：页面元素的原点就是窗口原点（标题栏由
/// `main_view` 排在它上面），所以别再叠加页面内部的描边内边距。
pub fn pane_offset(hwnd: isize, layout: PaneLayout) -> Option<(f32, f32)> {
    if hwnd == 0 {
        return None;
    }
    let (client_w, client_h, scale) = client_metrics(hwnd);
    let rect = logical_rect(client_w, client_h, scale, layout);

    Some((
        rect.x as f32 / scale,
        rect.y as f32 / scale - bar_height(layout),
    ))
}

pub fn pane_size(iced: isize, layout: PaneLayout) -> (i32, i32) {
    let rect = pane_rect(iced, layout);
    (rect.width, rect.height)
}

pub fn find_session_window(pid: u32) -> Option<isize> {
    platform::find_session_window(pid)
}

/// 内置 ActiveX 宿主里属于 `session` 的顶级窗口。
///
/// 标题是 `SUPERREMOTE|<session>|<state>|<message>`：同一宿主进程里的多个窗口
/// 靠会话段区分，状态仍从标题读，不需要额外的响应通道。旧宿主的标题没有
/// 会话段（一个进程只有一个窗口），仍然接受，升级期间不会连不上。
pub fn find_activex_window(pid: u32, session: &str) -> Option<isize> {
    platform::find_activex_window(pid, session)
}

/// 宿主进程的命令窗口（标题固定为 [`crate::contract::CONTROL_TITLE`]）。
///
/// 一个进程只有一个，供 Rust 发送 `add` / `close`；找不到说明宿主还没建好窗口
/// 或者已经退出，调用方应退回「新起一个宿主」。
pub fn find_activex_control(pid: u32) -> Option<isize> {
    platform::find_activex_control(pid)
}

/// 给宿主发一条命令。`payload` 以 UTF-16LE 送出（见 `command_bytes`），
/// 与宿主里的 `WM_COPYDATA` 解析一致。返回 `false` 表示窗口已失效。
pub fn send_command(control: isize, payload: &str) -> bool {
    platform::send_command(control, payload)
}

/// 命令载荷的线上编码：UTF-16LE，无反引号、无 BOM。
pub fn command_bytes(payload: &str) -> Vec<u8> {
    payload.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

#[cfg(windows)]
fn command_version() -> usize {
    crate::contract::COMMAND_VERSION
}

pub fn window_title(window: isize) -> String {
    platform::window_title(window)
}

/// 跨进程探活宿主窗口：`WM_GETTEXTLENGTH` 返回窗口标题长度（>0 表示宿主 GUI
/// 线程在响应）。超时/挂起返回 0，同时给出耗时。连续 0 只能说明宿主 GUI
/// 线程没有及时响应；既不能据此判定 Iced 主线程停摆，也不能推断图形通道
/// 或远程服务端状态。
pub fn probe(window: isize) -> (u128, isize) {
    platform::probe(window)
}

pub fn embed(window: isize, parent: isize, rect: Rect) -> Result<(), String> {
    platform::embed(window, parent, rect)
}

pub fn is_window(window: isize) -> bool {
    platform::is_window(window)
}

pub fn is_window_visible(window: isize) -> bool {
    platform::is_window_visible(window)
}

pub fn is_zoomed(window: isize) -> bool {
    platform::is_zoomed(window)
}

/// 显示但不抢焦点；抢焦点只发生在标签被激活的瞬间（`focus`）。
pub fn show(window: isize) {
    platform::show(window);
}

pub fn hide(window: isize) {
    platform::hide(window);
}

pub fn reposition(window: isize, rect: Rect) {
    // 手动拖边期间子窗口保持按下时的几何，由父客户区裁剪；
    // 常规同步（`sync_rdp_layout`）在这段时间必须让路，
    // 否则每帧都会把子窗口拖到新尺寸，缝隙又回来了。
    if sizing::is_dragging() {
        return;
    }
    platform::reposition(window, rect);
}

pub fn focus(window: isize) {
    platform::focus(window);
}

pub fn request_close(window: isize) {
    platform::request_close(window);
}

/// 从 Iced 的窗口 trait 对象里取 Win32 句柄。
pub fn hwnd_of(window: &dyn iced::window::Window) -> Option<isize> {
    use iced::window::raw_window_handle::RawWindowHandle;

    match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(handle) => Some(handle.hwnd.get()),
        _ => None,
    }
}

#[cfg(windows)]
fn client_metrics(iced: isize) -> (i32, i32, f32) {
    use windows_sys::Win32::Foundation::{HWND, RECT};
    use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
    use windows_sys::Win32::UI::WindowsAndMessaging::GetClientRect;

    unsafe {
        // SAFETY: `iced` 是本进程主窗口的 `HWND`（由 `App::hwnd` 从 Iced 拿到），
        // `GetClientRect`/`GetDpiForWindow` 都只读该窗口的状态，不需要释放任何资源。
        // 句柄失效时两者返回 0 / 空矩形，下面已按返回值分流到兜底尺寸与 1.0 缩放。
        let hwnd = iced as HWND;
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };

        let (width, height) = if GetClientRect(hwnd, &mut rect) != 0 {
            (rect.right - rect.left, rect.bottom - rect.top)
        } else {
            (1180, 760)
        };

        let dpi = GetDpiForWindow(hwnd);
        let scale = if dpi == 0 { 1.0 } else { dpi as f32 / 96.0 };

        (width.max(1), height.max(1), scale)
    }
}

#[cfg(not(windows))]
fn client_metrics(_iced: isize) -> (i32, i32, f32) {
    (1180, 760, 1.0)
}

#[cfg(windows)]
mod platform {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;

    use windows_sys::Win32::Foundation::{GetLastError, HWND, LPARAM, RECT};
    use windows_sys::Win32::System::DataExchange::COPYDATASTRUCT;
    use windows_sys::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetClassNameW, GetParent, GetWindowLongPtrW, GetWindowRect, GetWindowTextW,
        GetWindowThreadProcessId, IsWindow, IsWindowVisible, IsZoomed, PostMessageW,
        SendMessageTimeoutW, SendMessageW, SetParent, SetWindowLongPtrW, SetWindowPos, ShowWindow,
        GWL_EXSTYLE, GWL_STYLE, SMTO_ABORTIFHUNG, SMTO_BLOCK, SWP_FRAMECHANGED, SWP_NOACTIVATE,
        SWP_NOOWNERZORDER, SWP_NOZORDER, SW_HIDE, SW_SHOWNA, WM_CLOSE, WM_COPYDATA, WM_GETTEXT,
        WS_CAPTION, WS_CHILD, WS_DLGFRAME, WS_EX_APPWINDOW, WS_EX_CLIENTEDGE, WS_EX_DLGMODALFRAME,
        WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_WINDOWEDGE, WS_MAXIMIZEBOX, WS_MINIMIZEBOX,
        WS_POPUP, WS_SYSMENU, WS_THICKFRAME,
    };

    use super::{Rect, SESSION_CLASS};

    struct Search {
        pid: u32,
        found: Option<HWND>,
    }

    struct ActiveXSearch {
        pid: u32,
        session: String,
        found: Option<HWND>,
    }

    struct ControlSearch {
        pid: u32,
        found: Option<HWND>,
    }

    pub fn find_session_window(pid: u32) -> Option<isize> {
        let mut search = Search { pid, found: None };

        // SAFETY: `search` 活到本次调用结束，回调只在这段同步调用期间被系统调用，
        // 不会在 `EnumWindows` 返回后继续持有该指针；回调本身只写 `search.found`，
        // 不保留任何指针，因此没有别名或悬垂风险。
        unsafe {
            EnumWindows(Some(enum_window), &mut search as *mut Search as LPARAM);
        }

        search.found.map(|hwnd| hwnd as isize)
    }

    pub fn find_activex_window(pid: u32, session: &str) -> Option<isize> {
        let mut search = ActiveXSearch {
            pid,
            session: session.to_owned(),
            found: None,
        };

        // SAFETY: 同 `find_session_window`：`search` 活到本次同步调用结束，
        // 回调只在枚举期间写 `search.found`。
        unsafe {
            EnumWindows(
                Some(enum_activex_window),
                &mut search as *mut ActiveXSearch as LPARAM,
            );
        }

        search.found.map(|hwnd| hwnd as isize)
    }

    /// # Safety
    ///
    /// `lparam` 必须是 [`find_activex_window`] 传进来的 `*mut ActiveXSearch`，
    /// 且在回调期间有效。
    unsafe extern "system" fn enum_activex_window(hwnd: HWND, lparam: LPARAM) -> i32 {
        // SAFETY: 依据本函数的 `# Safety` 段，`lparam` 就是上面那个 `&mut search`。
        let search = unsafe { &mut *(lparam as *mut ActiveXSearch) };
        let mut pid = 0u32;

        // SAFETY: 两个指针都指向本栈帧里的局部变量。
        unsafe {
            GetWindowThreadProcessId(hwnd, &mut pid);
        }
        if pid != search.pid {
            return 1;
        }

        let mut buffer = [0u16; 256];
        // SAFETY: `buffer` 可写，长度按 `buffer.len()` 传入；系统最多写这么多。
        let len = unsafe { GetWindowTextW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32) };
        if len > 0 {
            let title = OsString::from_wide(&buffer[..len as usize]);
            let title = title.to_string_lossy();
            if let Some(host) = crate::contract::parse_title(&title) {
                // 新宿主的每个窗口都必须匹配自己的会话；旧宿主没有会话段，
                // 一个进程只有一个窗口，直接认领。
                if host.session.is_none() || host.session == Some(search.session.as_str()) {
                    search.found = Some(hwnd);
                    return 0;
                }
            }
        }
        1
    }

    pub fn find_activex_control(pid: u32) -> Option<isize> {
        let mut search = ControlSearch { pid, found: None };

        // SAFETY: 同 `find_activex_window`：`search` 活到枚举结束，回调只写
        // `search.found`，不保留指针。
        unsafe {
            EnumWindows(
                Some(enum_control_window),
                &mut search as *mut ControlSearch as LPARAM,
            );
        }

        search.found.map(|hwnd| hwnd as isize)
    }

    /// # Safety
    ///
    /// `lparam` 必须是 [`find_activex_control`] 传进来的 `*mut ControlSearch`，
    /// 且在回调期间有效。
    unsafe extern "system" fn enum_control_window(hwnd: HWND, lparam: LPARAM) -> i32 {
        // SAFETY: 依据本函数的 `# Safety` 段，`lparam` 就是上面那个 `&mut search`。
        let search = unsafe { &mut *(lparam as *mut ControlSearch) };
        let mut pid = 0u32;

        // SAFETY: 两个指针都指向本栈帧里的局部变量。
        unsafe {
            GetWindowThreadProcessId(hwnd, &mut pid);
        }
        if pid != search.pid {
            return 1;
        }

        if window_title(hwnd as isize) == crate::contract::CONTROL_TITLE {
            search.found = Some(hwnd);
            return 0;
        }
        1
    }

    pub fn send_command(control: isize, payload: &str) -> bool {
        let mut bytes = super::command_bytes(payload);
        let data = COPYDATASTRUCT {
            dwData: super::command_version(),
            cbData: bytes.len() as u32,
            lpData: bytes.as_mut_ptr().cast(),
        };

        // SAFETY: `bytes` 活到 `SendMessageW` 返回；`WM_COPYDATA` 是同步消息，
        // 宿主在返回前就把数据拷走，之后不会再读这块内存。
        unsafe {
            SendMessageW(
                control as HWND,
                WM_COPYDATA,
                0,
                &data as *const COPYDATASTRUCT as LPARAM,
            ) != 0
        }
    }

    /// 标题是宿主 → Rust 的状态通道，但窗口属于另一个进程。同步窗口消息可能
    /// 等待宿主 GUI 线程，必须带超时，避免拖住 Iced 主线程；读不到就返回空，
    /// 状态通道允许丢一拍，下次 RdpTick 再读。
    pub fn probe(window: isize) -> (u128, isize) {
        use std::time::Instant;

        let mut result = 0usize;
        let started = Instant::now();
        // SAFETY: WM_GETTEXTLENGTH(0x000E) 是系统消息，跨进程由系统编组；
        // 超时或目标无响应时返回 0，不会解引用调用方缓冲区。
        let len = unsafe {
            SendMessageTimeoutW(
                window as HWND,
                0x000E,
                0,
                0,
                SMTO_ABORTIFHUNG | SMTO_BLOCK,
                100,
                &mut result,
            )
        };
        (started.elapsed().as_millis(), len)
    }

    pub fn window_title(window: isize) -> String {
        let mut buffer = [0u16; 512];
        let mut copied = 0usize;
        // SAFETY: WM_GETTEXT 是系统消息，跨进程时由系统编组缓冲区；wParam 是
        // 缓冲区的 u16 容量，lParam 指向本栈帧可写内存，超时返回 0 不使用缓冲区。
        let ok = unsafe {
            SendMessageTimeoutW(
                window as HWND,
                WM_GETTEXT,
                buffer.len(),
                buffer.as_mut_ptr() as LPARAM,
                SMTO_ABORTIFHUNG | SMTO_BLOCK,
                50,
                &mut copied,
            )
        };
        if ok == 0 || copied == 0 || copied > buffer.len() {
            return String::new();
        }
        OsString::from_wide(&buffer[..copied])
            .to_string_lossy()
            .into_owned()
    }

    /// # Safety
    ///
    /// `lparam` 必须是 [`find_session_window`] 传进来的 `*mut Search`，且在回调期间有效。
    /// 系统按约定原样回传，这由 `EnumWindows` 的契约保证。
    unsafe extern "system" fn enum_window(hwnd: HWND, lparam: LPARAM) -> i32 {
        // SAFETY: 依据本函数的 `# Safety` 段，`lparam` 就是上面那个 `&mut search`。
        // 回调是同步且串行的，同一时刻只有一个 `&mut` 存在。
        let search = unsafe { &mut *(lparam as *mut Search) };
        let mut pid = 0u32;

        // SAFETY: 两个指针都指向本栈帧里的局部变量，`hwnd` 由系统在枚举时给出。
        unsafe {
            GetWindowThreadProcessId(hwnd, &mut pid);
        }

        if pid != search.pid {
            return 1;
        }

        let mut buffer = [0u16; 128];
        // SAFETY: `buffer` 是可写的 128 个 u16，长度按 `buffer.len()` 传入，
        // 系统最多写这么多，返回值 `len` 不超过它，下面的切片因此不会越界。
        let len = unsafe { GetClassNameW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32) };

        if len > 0 {
            let class = OsString::from_wide(&buffer[..len as usize]);
            if class.to_string_lossy() == SESSION_CLASS {
                search.found = Some(hwnd);
                return 0;
            }
        }

        1
    }

    pub fn embed(window: isize, parent: isize, rect: Rect) -> Result<(), String> {
        let hwnd = window as HWND;
        let parent = parent as HWND;

        // SAFETY: 全部是「改窗口风格 / 换父窗口 / 摆位置」的 Win32 调用，参数都是
        // 本进程持有的句柄（`parent` 是我们的主窗口，`hwnd` 是 `mstsc` 的会话窗口），
        // 不涉及指针所有权，也不分配需要释放的资源。
        // - `SetWindowLongPtrW` 的返回值只表示旧值，失败时旧值为 0，随后用
        //   `GetParent` 复核真正的结果，所以不依赖它的返回值判断成败。
        // - `SetWindowPos` 的 `hwnd_insert_after` 传 null 配合 `SWP_NOZORDER`
        //   表示「不改 Z 序」，这是该组合的合法用法。
        unsafe {
            let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
            let style = (style
                & !(WS_POPUP
                    | WS_CAPTION
                    | WS_THICKFRAME
                    | WS_SYSMENU
                    | WS_MINIMIZEBOX
                    | WS_MAXIMIZEBOX
                    | WS_DLGFRAME))
                | WS_CHILD;
            SetWindowLongPtrW(hwnd, GWL_STYLE, style as isize);

            let ex_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
            let ex_style = (ex_style
                & !(WS_EX_APPWINDOW
                    | WS_EX_WINDOWEDGE
                    | WS_EX_CLIENTEDGE
                    | WS_EX_DLGMODALFRAME
                    | WS_EX_TOPMOST))
                | WS_EX_TOOLWINDOW;
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex_style as isize);

            SetParent(hwnd, parent);

            if GetParent(hwnd) != parent {
                let code = GetLastError();
                return Err(format!("无法把远程桌面窗口挂进主窗口（Win32 错误 {code}）"));
            }

            SetWindowPos(
                hwnd,
                std::ptr::null_mut(),
                rect.x,
                rect.y,
                rect.width,
                rect.height,
                SWP_NOZORDER | SWP_NOOWNERZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
            );
        }

        Ok(())
    }

    pub fn is_window(window: isize) -> bool {
        // SAFETY: `IsWindow` 对任意句柄值都只查询，不合法时返回 0，不会解引用。
        unsafe { IsWindow(window as HWND) != 0 }
    }

    pub fn is_window_visible(window: isize) -> bool {
        // SAFETY: 同 `is_window`——纯查询，非法句柄返回 0。
        unsafe { IsWindowVisible(window as HWND) != 0 }
    }

    /// 窗口当前是否真的处于最大化（`SW_MAXIMIZE` 状态）。
    ///
    /// 标题栏据此校正 `App::maximized` 记账：系统拖拽还原、系统菜单、全屏往返
    /// 都会绕过窗口按钮改窗口状态，只靠按钮翻转记账会记反。
    pub fn is_zoomed(window: isize) -> bool {
        // SAFETY: 纯查询；句柄失效时 `IsZoomed` 返回 0。
        unsafe { IsZoomed(window as HWND) != 0 }
    }

    pub fn show(window: isize) {
        // SAFETY: `ShowWindow` 只改窗口可见性；`SW_SHOWNA` 不激活窗口，
        // 因此不会抢走用户当前的输入焦点。句柄失效时调用无效果。
        unsafe {
            ShowWindow(window as HWND, SW_SHOWNA);
        }
    }

    pub fn hide(window: isize) {
        // SAFETY: 同 `show`，只改可见性，不释放资源。
        unsafe {
            ShowWindow(window as HWND, SW_HIDE);
        }
    }

    pub fn window_origin(window: isize) -> Option<(f32, f32)> {
        let hwnd = window as HWND;
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };

        // SAFETY: 两个调用都只读该窗口的状态；句柄失效时 `GetWindowRect`
        // 返回 0、`GetDpiForWindow` 返回 0，下面按兜底返回 `None`。
        unsafe {
            if GetWindowRect(hwnd, &mut rect) == 0 {
                return None;
            }
            let dpi = GetDpiForWindow(hwnd);
            let scale = if dpi == 0 { 1.0 } else { dpi as f32 / 96.0 };

            Some((rect.left as f32 / scale, rect.top as f32 / scale))
        }
    }

    pub fn reposition(window: isize, rect: Rect) {
        // SAFETY: 只设置位置与尺寸；`SWP_NOZORDER` 配合空 `hwnd_insert_after`
        // 表示不改 Z 序，`SWP_NOACTIVATE` 保证不抢焦点。
        unsafe {
            SetWindowPos(
                window as HWND,
                std::ptr::null_mut(),
                rect.x,
                rect.y,
                rect.width,
                rect.height,
                SWP_NOZORDER | SWP_NOOWNERZORDER | SWP_NOACTIVATE,
            );
        }
    }

    /// 跨进程窗口的焦点要先把两个线程的输入队列接上，否则 `SetFocus` 会被忽略。
    pub fn focus(window: isize) {
        let hwnd = window as HWND;

        // SAFETY: 三个调用都只影响焦点与输入队列的挂接状态，不涉及内存所有权。
        // `GetWindowThreadProcessId` 的第二个参数允许为 null（表示不关心 PID）。
        // `AttachThreadInput` 的返回值已用于判断是否真的挂上了，只有挂上才解绑，
        // 避免把本来无关的两个线程解绑掉。
        unsafe {
            let target_thread = GetWindowThreadProcessId(hwnd, std::ptr::null_mut());
            let current_thread = GetCurrentThreadId();
            let attached = target_thread != current_thread
                && AttachThreadInput(current_thread, target_thread, 1) != 0;

            SetFocus(hwnd);

            if attached {
                AttachThreadInput(current_thread, target_thread, 0);
            }
        }
    }

    pub fn request_close(window: isize) {
        // SAFETY: `PostMessageW` 只把一条消息放进目标窗口的消息队列，
        // 立刻返回且不等待处理；句柄失效时返回 0，不会崩溃。
        unsafe {
            PostMessageW(window as HWND, WM_CLOSE, 0, 0);
        }
    }
}

#[cfg(not(windows))]
mod platform {
    use super::Rect;

    pub fn find_session_window(_pid: u32) -> Option<isize> {
        None
    }

    pub fn find_activex_window(_pid: u32, _session: &str) -> Option<isize> {
        None
    }

    pub fn find_activex_control(_pid: u32) -> Option<isize> {
        None
    }

    pub fn send_command(_control: isize, _payload: &str) -> bool {
        false
    }

    pub fn window_title(_window: isize) -> String {
        String::new()
    }

    pub fn probe(_window: isize) -> (u128, isize) {
        (0, 0)
    }

    pub fn embed(_window: isize, _parent: isize, _rect: Rect) -> Result<(), String> {
        Err("RDP 内嵌只在 Windows 上可用".to_owned())
    }

    pub fn is_window(_window: isize) -> bool {
        false
    }

    pub fn is_window_visible(_window: isize) -> bool {
        false
    }

    pub fn is_zoomed(_window: isize) -> bool {
        false
    }

    pub fn show(_window: isize) {}
    pub fn hide(_window: isize) {}
    pub fn window_origin(_window: isize) -> Option<(f32, f32)> {
        None
    }
    pub fn reposition(_window: isize, _rect: Rect) {}
    pub fn focus(_window: isize) {}
    pub fn request_close(_window: isize) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_host_rect_starts_after_the_title_bar() {
        let rect = logical_rect(1180, 760, 1.0, PaneLayout::Main);

        // 左/右/下各让开一条缩放热区，顶部由 38px 标题栏让开。
        assert_eq!(rect.x, 3, "画面要留出左边的缩放热区");
        assert_eq!(rect.y, 38, "只剩 38px 标题栏");
        assert_eq!(rect.width, 1174);
        assert_eq!(rect.height, 719);
    }

    #[test]
    fn the_host_rect_scales_with_dpi() {
        let rect = logical_rect(1770, 1140, 1.5, PaneLayout::Main);

        assert_eq!(rect.x, 5, "3 logical × 1.5");
        assert_eq!(rect.y, 57, "38 logical × 1.5");
        assert_eq!(rect.width, 1760);
        assert_eq!(rect.height, 1078);
    }

    #[test]
    fn fullscreen_keeps_only_the_exit_bar() {
        let rect = logical_rect(1920, 1080, 1.0, PaneLayout::Fullscreen);

        // 全屏没有可拖的边，画面铺满、不留热区。
        assert_eq!(rect.x, 0);
        assert_eq!(rect.y, 30);
        assert_eq!(rect.width, 1920);
        assert_eq!(rect.height, 1050);
    }

    /// 独立窗口没有侧栏与标签条，只有顶部标题条（与全屏退出栏同高）。
    #[test]
    fn detached_window_only_drops_the_title_bar() {
        let rect = logical_rect(980, 660, 1.0, PaneLayout::Detached);

        assert_eq!(rect.x, 3);
        assert_eq!(rect.y, 30);
        assert_eq!(rect.width, 974);
        assert_eq!(rect.height, 627);
    }
}

#[cfg(windows)]
mod sizing {
    use std::collections::HashMap;
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Mutex, OnceLock};

    use super::{logical_rect, PaneLayout};
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
    use windows_sys::Win32::Graphics::Gdi::{
        BitBlt, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, GetUpdateRect,
        RedrawWindow, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
        DIB_RGB_COLORS, RDW_ALLCHILDREN, RDW_ERASE, RDW_FRAME, RDW_INVALIDATE, RDW_UPDATENOW,
        SRCCOPY,
    };
    use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{SetFocus, VK_ESCAPE};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CallWindowProcW, DefWindowProcW, GetClientRect, GetCursorPos, GetPropW, GetWindowLongPtrW,
        GetWindowRect, IsZoomed, KillTimer, RemovePropW, SetPropW, SetTimer, SetWindowLongPtrW,
        SetWindowPos, GWLP_WNDPROC, SIZE_MINIMIZED, SWP_NOACTIVATE, SWP_NOCOPYBITS, SWP_NOMOVE,
        SWP_NOOWNERZORDER, SWP_NOZORDER, WM_CAPTURECHANGED, WM_KEYDOWN, WM_LBUTTONUP, WM_MOUSEMOVE,
        WM_SIZE, WM_TIMER,
    };

    /// `WM_MOUSEMOVE` 的 `wParam` 里的 `MK_LBUTTON` 位。windows-sys 把它放在需要
    /// 额外 feature 的 `SystemServices` 模块，这里只需要这一个位。
    const LEFT_BUTTON_DOWN: WPARAM = 0x0001;

    /// 拖动期的几何应用计时器。`WM_MOUSEMOVE` 只置脏标记，真正的
    /// `SetWindowPos` 由 `WM_TIMER` 每 `DRAG_FRAME_MS` 做一次：
    /// `WM_TIMER` 只在消息队列排空时才被派发，不会被连续的鼠标消息饿死。
    const DRAG_TIMER: usize = 0x5352;
    const DRAG_FRAME_MS: u32 = 15;

    /// 拖动中鼠标动过、还没有应用到窗口的几何。只置位不读光标，
    /// 让 `WM_MOUSEMOVE` 保持 O(1)。
    static GEOMETRY_DIRTY: AtomicBool = AtomicBool::new(false);

    // 临时诊断用的消息号（定位拖拽明暗交替闪烁后连同 `diag` 一起删除）。
    const WM_PAINT: u32 = 0x000F;
    const WM_ERASEBKGND: u32 = 0x0014;
    const WM_WINDOWPOSCHANGED: u32 = 0x0047;
    const WM_NCPAINT: u32 = 0x0085;
    /// 光标诊断用（见 `diag::cursor_swallow`），与上面的临时消息号同一批。
    const WM_SETCURSOR: u32 = 0x0020;
    /// `WM_SETCURSOR` 的 `lParam` 低字：光标落在客户区。
    const HTCLIENT: u32 = 1;

    type WndProc = unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT;
    type Children = Vec<(isize, PaneLayout)>;
    type Table = Mutex<HashMap<isize, Children>>;

    /// 一次进行中的手动拖边缩放。坐标全部是屏幕/窗口物理像素。
    #[derive(Clone, Copy)]
    struct Drag {
        window: isize,
        direction: iced::window::Direction,
        cursor: (i32, i32),
        rect: (i32, i32, i32, i32),
        min: (i32, i32),
    }

    /// 拖动过程中每一帧都要用它的起点算绝对矩形，不能累加增量——
    /// 鼠标与窗口跟随之间有抖动，累加会把误差放大。
    fn drag() -> &'static Mutex<Option<Drag>> {
        static DRAG: OnceLock<Mutex<Option<Drag>>> = OnceLock::new();
        DRAG.get_or_init(|| Mutex::new(None))
    }

    /// 拖动期间被借走键盘焦点的子窗口。`begin_resize` 往这里存，
    /// 松开（含 ESC 取消）时原样还回去。
    fn focus_return() -> &'static Mutex<Option<isize>> {
        static FOCUS: OnceLock<Mutex<Option<isize>>> = OnceLock::new();
        FOCUS.get_or_init(|| Mutex::new(None))
    }

    /// 原窗口过程存在**窗口属性**里，而不是我们自己的表里：转发必须无条件成功，
    /// 不能依赖任何锁。锁被占时如果查不到原过程，就只能把消息丢给
    /// `DefWindowProc`——那会把 winit 自己的状态打烂。属性还随窗口一起消失。
    fn key() -> &'static [u16] {
        static KEY: OnceLock<Vec<u16>> = OnceLock::new();
        KEY.get_or_init(|| {
            "SUPERREMOTE_SIZING_WNDPROC"
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect()
        })
    }

    /// 父窗口 → 它要同步带动的子窗口。只在主线程读写；窗口过程用 `try_lock`，
    /// 抢不到就这一次不贴边（下一个 `WM_SIZE` 或常规路径会补上），绝不阻塞消息循环。
    fn table() -> &'static Table {
        static TABLE: OnceLock<Table> = OnceLock::new();
        TABLE.get_or_init(|| Mutex::new(HashMap::new()))
    }

    /// 有没有正在进行的拖边。
    ///
    /// 拖动期间几何由 `WM_TIMER` 每 15ms 应用一次，子窗口跟着每一次
    /// `WM_SIZE` 同步摆位；常规同步（150ms 轮询）在这段时间让路，
    /// 免得两条路径在同一步里各改一次子窗口几何。
    pub fn is_dragging() -> bool {
        drag()
            .try_lock()
            .map(|guard| guard.is_some())
            .unwrap_or(false)
    }

    pub fn set_children(parent: isize, children: Children) {
        // 名单是「当前真的显示着的子窗口」。空名单要显式清掉旧表项：
        // 否则切到主页/设置后，窗口过程还在按过期的表给隐藏窗口摆位，
        // 拖边抓帧也会抓到已经不在前台的那一页。
        if children.is_empty() {
            if let Ok(mut guard) = table().lock() {
                guard.remove(&parent);
            }
            return;
        }

        if install(parent).is_err() {
            return;
        }

        if let Ok(mut guard) = table().lock() {
            guard.insert(parent, children);
        }
    }

    pub fn clear(parent: isize) {
        if let Ok(mut guard) = table().lock() {
            guard.remove(&parent);
        }
        end_drag(Some(parent));

        unsafe {
            let hwnd = parent as HWND;
            let original = GetPropW(hwnd, key().as_ptr()) as isize;
            if original != 0 {
                SetWindowLongPtrW(hwnd, GWLP_WNDPROC, original);
                RemovePropW(hwnd, key().as_ptr());
            }
        }
    }

    /// 开始手动拖边。系统模态缩放循环里 Iced 的 `update` 不跑，Iced 只能按
    /// 旧版面重画新尺寸，露出的底色就是闪烁的来源；改成自己处理鼠标移动后，
    /// 每一步都是一次普通的 `SetWindowPos`，`WM_SIZE` 照常进事件循环。
    ///
    /// 这里**不调用** `SetCapture`：winit 在 `WM_LBUTTONDOWN` 里已经捕获了鼠标。
    /// 再捕获一次时窗口本来就持有捕获，Windows 仍会同步发一条
    /// `WM_CAPTURECHANGED`，拖拽状态会被当场清掉——这正是「按下边缘后窗口
    /// 完全不动」的成因。本轮回归是同一个坑的另一种触发方式：隐藏持有焦点的
    /// 子窗口同样会改变捕获/激活，`WM_CAPTURECHANGED` 一到状态就没了。
    /// 拖拽期间**不碰**子窗口，就是为了让这条消息在拖动中根本不会出现。
    pub fn begin_resize(
        window: isize,
        direction: iced::window::Direction,
        min_size: (f32, f32),
    ) -> bool {
        unsafe {
            let hwnd = window as HWND;
            // 最大化时交给系统的还原 + 缩放，与原生边框行为一致。
            if window == 0 || IsZoomed(hwnd) != 0 {
                return false;
            }

            let mut rect = RECT::default();
            let mut point = POINT::default();
            if GetWindowRect(hwnd, &mut rect) == 0 || GetCursorPos(&mut point) == 0 {
                return false;
            }

            if install(window).is_err() {
                return false;
            }

            let dpi = GetDpiForWindow(hwnd);
            let scale = if dpi == 0 { 1.0 } else { dpi as f32 / 96.0 };
            let min = (
                (min_size.0 * scale).round() as i32,
                (min_size.1 * scale).round() as i32,
            );

            let mut stored = false;
            if let Ok(mut guard) = drag().lock() {
                *guard = Some(Drag {
                    window,
                    direction,
                    cursor: (point.x, point.y),
                    rect: (rect.left, rect.top, rect.right, rect.bottom),
                    min,
                });
                stored = true;
            }
            if !stored {
                return false;
            }

            // 键盘焦点通常握在远程画面那个跨进程子窗口里，ESC 会直接进远端
            // 桌面，父窗口的窗口过程根本收不到，取消就失效了。拖动期间先把
            // 焦点借回父窗口（跟系统自己的缩放循环一致），松开时再还回去。
            // 这是 ESC 取消**唯一**可行的实现方式：键盘消息只送给焦点窗口，
            // 鼠标捕获不会把键盘一并带过来。子窗口保持可见，借还焦点本身不会
            // 影响激活状态。
            let child = table()
                .try_lock()
                .ok()
                .and_then(|guard| guard.get(&window).and_then(|list| list.first().cloned()));
            if let Some((child, _)) = child {
                if let Ok(mut guard) = focus_return().lock() {
                    *guard = Some(child);
                }
            }
            SetFocus(hwnd);
            SetTimer(hwnd, DRAG_TIMER, DRAG_FRAME_MS, None);
            GEOMETRY_DIRTY.store(false, Ordering::Relaxed);
            // 先抓一张静态帧（必须在子窗口还停在原位时抓），再把子窗口挪出
            // 客户区：拖动全程只有 Iced 在画，子窗口不擦背景也不重画。
            let captured = child.and_then(|(child, layout)| capture_child(window, child, layout));
            let frozen_ready = captured.is_some();
            if let Ok(mut guard) = frozen().lock() {
                *guard = captured;
            }
            park_children(window);

            diag::line(format!(
                "t={} event=begin dir={direction:?} rect={},{},{},{} frozen={}",
                diag::stamp(),
                rect.left,
                rect.top,
                rect.right,
                rect.bottom,
                u8::from(frozen_ready),
            ));
        }

        true
    }

    /// 把拖动期间借走的键盘焦点还给远程画面。没有借过就什么都不做，
    /// 免得普通点击也把焦点塞给 RDP 子窗口。
    fn hand_focus_back() {
        let child = focus_return()
            .try_lock()
            .ok()
            .and_then(|mut guard| guard.take());
        if let Some(child) = child {
            super::platform::focus(child);
        }
    }

    /// 拖动期替代画面用的静态帧。只在主线程写、只在拖动期间读。
    fn frozen() -> &'static Mutex<Option<crate::rdp_host::FrozenFrame>> {
        static FROZEN: OnceLock<Mutex<Option<crate::rdp_host::FrozenFrame>>> = OnceLock::new();
        FROZEN.get_or_init(|| Mutex::new(None))
    }

    pub fn frozen_frame() -> Option<crate::rdp_host::FrozenFrame> {
        if !is_dragging() {
            return None;
        }
        frozen().try_lock().ok().and_then(|guard| guard.clone())
    }

    pub fn frozen_frame_for(parent: isize) -> Option<crate::rdp_host::FrozenFrame> {
        let owner = drag()
            .try_lock()
            .ok()
            .and_then(|guard| guard.map(|drag| drag.window))?;
        if owner != parent {
            return None;
        }
        frozen_frame()
    }

    /// 拖动开始前把子窗口在屏幕上的内容抓成一张 RGBA 图，并记下它在画面
    /// 占位区里的原始偏移。
    ///
    /// 走桌面 DC 的 `BitBlt`：mstsc 用 D3D 画远程桌面，从它自己的 DC 上
    /// 一个像素都读不到。抓不到（没有子窗口、DC 失败、尺寸非法）就返回
    /// `None`，调用方退回纯黑底——拖动本身不受影响。
    fn capture_child(
        parent: isize,
        child: isize,
        layout: PaneLayout,
    ) -> Option<crate::rdp_host::FrozenFrame> {
        // 隐藏的子窗口在桌面 DC 上那一块是别的窗口或桌面本身，抓下来就是一张
        // 错图。拖动隐藏标签时宁可不画冻结帧，也不能把别人的画面糊上来。
        if !super::is_window_visible(child) {
            return None;
        }

        let mut rect = RECT::default();
        let mut parent_rect = RECT::default();
        // SAFETY: 只读两个窗口的外框，不改变它们的状态。
        unsafe {
            if GetWindowRect(child as HWND, &mut rect) == 0
                || GetWindowRect(parent as HWND, &mut parent_rect) == 0
            {
                return None;
            }
        }

        let width = rect.right - rect.left;
        let height = rect.bottom - rect.top;
        if width <= 0 || height <= 0 || width > 8192 || height > 8192 {
            return None;
        }

        let dpi = unsafe { GetDpiForWindow(parent as HWND) };
        let scale = if dpi == 0 { 1.0 } else { dpi as f32 / 96.0 };

        // 锚点取子窗口相对画面占位区左上角的偏移（日志里 offset=5,57 那一类），
        // 减掉占位区自身的顶栏高度后换成逻辑坐标，交给 Iced 当 padding 留白。
        // 只做减法、不做任何按宽高比的居中：画面区左上原点恒为 (0, 0)。
        // 无边框窗口的窗口矩形就是客户区矩形，左上角可直接当原点。
        let bar = match layout {
            PaneLayout::Main => crate::layout::TITLEBAR_H,
            PaneLayout::Detached | PaneLayout::Fullscreen => crate::layout::FULLSCREEN_BAR_H,
        };
        let offset = (
            (rect.left - parent_rect.left) as f32 / scale,
            ((rect.top - parent_rect.top) as f32 - bar * scale) / scale,
        );

        // SAFETY: 屏幕 DC / 内存 DC / DIB 都在本函数内成对创建与释放；
        // bits 只在 DIB 未释放期间读取。
        let pixels = unsafe {
            let screen = GetDC(std::ptr::null_mut());
            if screen.is_null() {
                return None;
            }

            let memory = CreateCompatibleDC(screen);
            if memory.is_null() {
                ReleaseDC(std::ptr::null_mut(), screen);
                return None;
            }

            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width,
                    // 负高度 = 自上而下，拿到的一行就是屏幕上一行。
                    biHeight: -height,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB,
                    ..BITMAPINFOHEADER::default()
                },
                ..BITMAPINFO::default()
            };

            let mut bits: *mut c_void = std::ptr::null_mut();
            let bitmap = CreateDIBSection(
                memory,
                &info,
                DIB_RGB_COLORS,
                &mut bits,
                std::ptr::null_mut(),
                0,
            );
            if bitmap.is_null() || bits.is_null() {
                DeleteDC(memory);
                ReleaseDC(std::ptr::null_mut(), screen);
                return None;
            }

            let previous = SelectObject(memory, bitmap);
            let copied = BitBlt(
                memory, 0, 0, width, height, screen, rect.left, rect.top, SRCCOPY,
            );

            let pixels = if copied != 0 {
                let raw = std::slice::from_raw_parts(bits as *const u32, (width * height) as usize);
                let mut rgba = Vec::with_capacity(raw.len() * 4);
                for &pixel in raw {
                    // DIB 是 0xAARRGGBB：低字节是蓝，高字节是红。
                    rgba.push((pixel >> 16) as u8);
                    rgba.push((pixel >> 8) as u8);
                    rgba.push(pixel as u8);
                    rgba.push(255);
                }
                Some(rgba)
            } else {
                None
            };

            SelectObject(memory, previous);
            DeleteObject(bitmap);
            DeleteDC(memory);
            ReleaseDC(std::ptr::null_mut(), screen);

            pixels
        }?;

        Some(crate::rdp_host::FrozenFrame {
            handle: iced_core::image::Handle::from_rgba(width as u32, height as u32, pixels),
            width: width as u32,
            height: height as u32,
            scale,
            offset,
        })
    }

    /// 把子窗口挪到父客户区左侧之外。
    ///
    /// 保持 `WS_VISIBLE`、焦点与鼠标捕获都不动——隐藏窗口会改变激活状态，
    /// Windows 会发 `WM_CAPTURECHANGED`，拖拽状态当场被清空（上一轮的回归
    /// 就是这么来的）。尺寸保持不变，所以连 `WM_SIZE` 都不会触发，mstsc
    /// 没有机会擦背景或重画。
    fn park_child(child: isize) {
        let mut rect = RECT::default();
        // SAFETY: 只读子窗口外框，拿到它的当前尺寸。
        if unsafe { GetWindowRect(child as HWND, &mut rect) } == 0 {
            return;
        }
        let width = rect.right - rect.left;
        let height = rect.bottom - rect.top;
        if width <= 0 || height <= 0 {
            return;
        }

        // SAFETY: 坐标是父客户区坐标；x 取负数把它推出裁剪区。
        unsafe {
            SetWindowPos(
                child as HWND,
                std::ptr::null_mut(),
                -(width + 16),
                0,
                width,
                height,
                SWP_NOZORDER | SWP_NOOWNERZORDER | SWP_NOACTIVATE,
            );
        }
    }

    fn park_children(parent: isize) {
        let children = table()
            .try_lock()
            .ok()
            .and_then(|guard| guard.get(&parent).cloned());
        if let Some(children) = children {
            for (child, _) in children {
                park_child(child);
            }
        }
    }

    /// 松手/取消后把画面接回来：先让整棵子树失效并立即重画一次；
    /// 再补一次 1px 尺寸抖动——mstsc 的 D3D 画面不一定会响应纯失效重绘，
    /// 真实的 `WM_SIZE` 才是可靠的恢复信号。
    fn restore_child(parent: isize) {
        let children = table()
            .try_lock()
            .ok()
            .and_then(|guard| guard.get(&parent).cloned());
        let Some(children) = children else {
            return;
        };

        for (child, _) in children {
            let mut rect = RECT::default();
            // SAFETY: 只读外框尺寸。
            if unsafe { GetWindowRect(child as HWND, &mut rect) } == 0 {
                continue;
            }
            let width = rect.right - rect.left;
            let height = rect.bottom - rect.top;

            // SAFETY: 句柄来自登记表，只发一条标准重绘请求。
            unsafe {
                RedrawWindow(
                    child as HWND,
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    RDW_INVALIDATE | RDW_ERASE | RDW_FRAME | RDW_ALLCHILDREN | RDW_UPDATENOW,
                );
            }

            if width <= 1 || height <= 1 {
                continue;
            }

            let flags =
                SWP_NOMOVE | SWP_NOZORDER | SWP_NOOWNERZORDER | SWP_NOACTIVATE | SWP_NOCOPYBITS;
            // SAFETY: `SWP_NOMOVE` 让 x/y 被忽略；先 +1px 再改回来。
            unsafe {
                SetWindowPos(
                    child as HWND,
                    std::ptr::null_mut(),
                    0,
                    0,
                    width + 1,
                    height + 1,
                    flags,
                );
                SetWindowPos(
                    child as HWND,
                    std::ptr::null_mut(),
                    0,
                    0,
                    width,
                    height,
                    flags,
                );
            }
        }
    }

    /// 结束拖动：清状态，然后把子窗口一次性对齐到最终尺寸。
    /// 捕获归 winit 管（它在 `WM_LBUTTONUP` 里 `ReleaseCapture`），
    /// 这里再放一次会把别的窗口的捕获抢掉。
    fn end_drag(window: Option<isize>) {
        let Ok(mut guard) = drag().try_lock() else {
            return;
        };
        let ended = match guard.as_ref() {
            Some(state) if window.is_none_or(|window| state.window == window) => {
                let state = *state;
                *guard = None;
                Some(state)
            }
            _ => None,
        };
        drop(guard);

        if let Some(state) = ended {
            unsafe {
                KillTimer(state.window as HWND, DRAG_TIMER);
            }
            GEOMETRY_DIRTY.store(false, Ordering::Relaxed);
            if window.is_some() {
                diag::line(format!("t={} event=end", diag::stamp()));
                align(state.window);
                // 子窗口先摆回最终几何，再强制它重绘；冻结帧同时停止使用。
                restore_child(state.window);
            }
        }
    }

    /// ESC 取消：退回按下时的外框，和原生缩放循环一致。
    fn cancel_drag(window: isize) {
        let original = {
            let Ok(mut guard) = drag().try_lock() else {
                return;
            };
            match guard.as_ref() {
                Some(state) if state.window == window => {
                    let rect = state.rect;
                    *guard = None;
                    rect
                }
                _ => return,
            }
        };

        let (left, top, right, bottom) = original;
        GEOMETRY_DIRTY.store(false, Ordering::Relaxed);
        unsafe {
            KillTimer(window as HWND, DRAG_TIMER);
            SetWindowPos(
                window as HWND,
                std::ptr::null_mut(),
                left,
                top,
                right - left,
                bottom - top,
                SWP_NOZORDER | SWP_NOOWNERZORDER | SWP_NOACTIVATE | SWP_NOCOPYBITS,
            );
        }

        diag::line(format!("t={} event=cancel", diag::stamp()));

        // 取消也按最终（退回后的）客户区对齐一次，和正常松手同一路径。
        align(window);
        restore_child(window);
    }

    /// 用起点矩形 + 当前光标算新外框；只改用户按住的那几条边。
    fn follow_cursor(state: &Drag) {
        let mut point = POINT::default();
        unsafe {
            if GetCursorPos(&mut point) == 0 {
                return;
            }
        }

        let dx = point.x - state.cursor.0;
        let dy = point.y - state.cursor.1;
        let (left, top, right, bottom) = state.rect;
        let mut edges = [left, top, right, bottom];

        if touches_left(state.direction) {
            edges[0] = (left + dx).min(right - state.min.0);
        }
        if touches_right(state.direction) {
            edges[2] = (right + dx).max(left + state.min.0);
        }
        if touches_top(state.direction) {
            edges[1] = (top + dy).min(bottom - state.min.1);
        }
        if touches_bottom(state.direction) {
            edges[3] = (bottom + dy).max(top + state.min.1);
        }

        // 没有实际位移就不动窗口：这和「只点一下边缘」等价。
        if edges == [left, top, right, bottom] {
            return;
        }

        // 只改父窗口。原生子窗口停在原处，被父客户区裁剪：
        // 变大时多出来的是 Iced 自己画的黑底，缩小时画面被切掉。
        unsafe {
            SetWindowPos(
                state.window as HWND,
                std::ptr::null_mut(),
                edges[0],
                edges[1],
                edges[2] - edges[0],
                edges[3] - edges[1],
                SWP_NOZORDER | SWP_NOOWNERZORDER | SWP_NOACTIVATE | SWP_NOCOPYBITS,
            );
        }
    }

    /// 定时器到点：把鼠标累积的位移一次性落到窗口上。
    ///
    /// 只有置过脏标记才真的算光标、改几何，所以静止不动时这个 15ms 计时器
    /// 是空转；`follow_cursor` 里的零位移判断再兜一层（只点不拖不会动窗口）。
    fn apply_geometry(hwnd: HWND) {
        if !GEOMETRY_DIRTY.swap(false, Ordering::Relaxed) {
            return;
        }
        if let Some(state) = drag().try_lock().ok().and_then(|guard| *guard) {
            if state.window == hwnd as isize {
                follow_cursor(&state);
                log_frame(hwnd);
            }
        }
    }

    /// 装一次。已经装过（属性还在）就绝不重复装——重复包装会把同一个 proc 套两层，
    /// 还原时只能剥掉一层，另一层永远留在窗口上。
    fn install(parent: isize) -> Result<(), ()> {
        unsafe {
            let hwnd = parent as HWND;
            if GetPropW(hwnd, key().as_ptr()) as isize != 0 {
                return Ok(());
            }

            let original = GetWindowLongPtrW(hwnd, GWLP_WNDPROC);
            if original == 0 || SetPropW(hwnd, key().as_ptr(), original as *mut c_void) == 0 {
                return Err(());
            }

            if SetWindowLongPtrW(hwnd, GWLP_WNDPROC, on_size as *const c_void as isize) == 0 {
                RemovePropW(hwnd, key().as_ptr());
                return Err(());
            }

            Ok(())
        }
    }

    fn touches_left(direction: iced::window::Direction) -> bool {
        matches!(
            direction,
            iced::window::Direction::West
                | iced::window::Direction::NorthWest
                | iced::window::Direction::SouthWest
        )
    }

    fn touches_right(direction: iced::window::Direction) -> bool {
        matches!(
            direction,
            iced::window::Direction::East
                | iced::window::Direction::NorthEast
                | iced::window::Direction::SouthEast
        )
    }

    fn touches_top(direction: iced::window::Direction) -> bool {
        matches!(
            direction,
            iced::window::Direction::North
                | iced::window::Direction::NorthWest
                | iced::window::Direction::NorthEast
        )
    }

    fn touches_bottom(direction: iced::window::Direction) -> bool {
        matches!(
            direction,
            iced::window::Direction::South
                | iced::window::Direction::SouthWest
                | iced::window::Direction::SouthEast
        )
    }

    unsafe extern "system" fn on_size(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        // `WM_SIZE` 的 lParam 就是新的**客户区尺寸**（物理像素），不必再查一次。
        // 最小化不贴：画面本来就看不见，恢复时还会再来一次 `WM_SIZE`。
        if message == WM_SIZE && wparam as u32 != SIZE_MINIMIZED {
            reposition(hwnd, lparam);
        }

        // 光标归子窗口自己管。子窗口把 `WM_SETCURSOR` 转发上来时 `wParam` 仍是
        // 「光标所在的那个子窗口」；再交给 winit，它会 `LoadCursorW` + `SetCursor`
        // 重设自己记着的箭头，与子窗口刚设好的光标（远端光标或隐藏）每移动一次
        // 互相覆盖，看起来就是光标忽大忽小地闪。HTCLIENT 之外的命中区（边框、
        // 标题栏）照常往下传，拖边手柄光标不受影响。
        if message == WM_SETCURSOR
            && wparam as isize != hwnd as isize
            && (lparam as u32 & 0xFFFF) == HTCLIENT
        {
            diag::cursor_swallow(hwnd as isize, wparam as isize);
            return 1;
        }

        let original = GetPropW(hwnd, key().as_ptr()) as isize;
        if original == 0 {
            // 属性没了（理论上不该发生）：交给默认过程，至少不把消息吞掉。
            return DefWindowProcW(hwnd, message, wparam, lparam);
        }

        let original: WndProc = std::mem::transmute(original);
        let result = CallWindowProcW(Some(original), hwnd, message, wparam, lparam);

        // 鼠标消息先让 winit 处理完再改窗口：Iced 拿到的坐标仍是拖动前的窗口坐标，
        // 不会因为窗口已经移动而算错悬停区域。
        match message {
            WM_PAINT => {
                diag::PAINT.fetch_add(1, Ordering::Relaxed);
            }
            WM_ERASEBKGND => {
                diag::ERASE.fetch_add(1, Ordering::Relaxed);
            }
            WM_NCPAINT => {
                diag::NCPAINT.fetch_add(1, Ordering::Relaxed);
            }
            WM_WINDOWPOSCHANGED => {
                diag::POSCHANGED.fetch_add(1, Ordering::Relaxed);
            }
            WM_MOUSEMOVE => {
                // 左键还按着才跟手。`begin_resize` 只登记状态，捕获是 winit 在
                // `WM_LBUTTONDOWN` 里做的；快速点一下就在窗口外松开时，之后第一条
                // 移动消息已没有这个位，顺手收尾，免得悬停都把窗口搬走。
                //
                // 这里**只置脏标记**，不读光标也不 `SetWindowPos`：几何全部交给
                // `WM_TIMER`。在鼠标消息里同步改窗口会把队列塞满，是最低优先级的
                // `WM_PAINT` 一直排不上，实测 5.32s 里 99 次尺寸变化只重绘了 13 次。
                if wparam & LEFT_BUTTON_DOWN == 0 {
                    end_drag(Some(hwnd as isize));
                    hand_focus_back();
                } else if drag()
                    .try_lock()
                    .is_ok_and(|guard| guard.is_some_and(|state| state.window == hwnd as isize))
                {
                    GEOMETRY_DIRTY.store(true, Ordering::Relaxed);
                }
            }
            WM_TIMER if wparam == DRAG_TIMER => {
                apply_geometry(hwnd);
            }
            WM_LBUTTONUP => {
                end_drag(Some(hwnd as isize));
                hand_focus_back();
            }
            WM_CAPTURECHANGED => {
                // lParam 是拿到捕获的窗口。等于自己时只是重新确认捕获
                // （winit 处理 `WM_LBUTTONDOWN` 时也走这条），不能当成拖动结束。
                if lparam != hwnd as isize {
                    end_drag(Some(hwnd as isize));
                    hand_focus_back();
                }
            }
            WM_KEYDOWN if wparam == VK_ESCAPE as WPARAM => {
                cancel_drag(hwnd as isize);
                hand_focus_back();
            }
            _ => {}
        }

        result
    }

    /// 按给定客户区尺寸把子窗口摆到画面区。
    ///
    /// 与 `pane_rect` 共用 `logical_rect`：两条路径算出的矩形必须一致，
    /// 否则常规同步会把拖动末尾对齐好的位置改歪。
    fn place_children(parent: isize, width: i32, height: i32, scale: f32) {
        let Ok(table) = table().try_lock() else {
            return;
        };
        let Some(children) = table.get(&parent) else {
            return;
        };

        for (child, layout) in children {
            // 拖动期间子窗口一律停在客户区外（`begin_resize` 已挪走），
            // 这里再摆回去就等于让它跟着每一步重画，闪烁会回来。
            if is_dragging() {
                park_child(*child);
                continue;
            }

            let rect = logical_rect(width, height, scale, *layout);
            // SAFETY: `child` 是登记表里的子窗口句柄，只改它的位置与大小。
            unsafe {
                SetWindowPos(
                    *child as HWND,
                    std::ptr::null_mut(),
                    rect.x,
                    rect.y,
                    rect.width,
                    rect.height,
                    SWP_NOZORDER | SWP_NOOWNERZORDER | SWP_NOACTIVATE,
                );
            }
        }
    }

    fn reposition(hwnd: HWND, lparam: LPARAM) {
        // 拖动期间子窗口被挪到客户区外，`place_children` 不会再按新客户区
        // 摆它；松手后 `align` 才按最终尺寸把它接回来并强制重绘。
        let raw = lparam as usize;
        let width = ((raw & 0xFFFF) as u16) as i32;
        let height = (((raw >> 16) & 0xFFFF) as u16) as i32;
        if width <= 0 || height <= 0 {
            return;
        }

        let dpi = unsafe { GetDpiForWindow(hwnd) };
        let scale = if dpi == 0 { 1.0 } else { dpi as f32 / 96.0 };
        place_children(hwnd as isize, width, height, scale);
    }

    /// 拖动结束（松手 / ESC 取消）时，用当时的客户区把子窗口一次性对齐。
    fn align(parent: isize) {
        let hwnd = parent as HWND;
        let mut rect = RECT::default();
        // SAFETY: 只读客户区尺寸；窗口已销毁时返回 0，下面直接放弃。
        if unsafe { GetClientRect(hwnd, &mut rect) } == 0 {
            return;
        }
        let width = rect.right - rect.left;
        let height = rect.bottom - rect.top;
        if width <= 0 || height <= 0 {
            return;
        }

        let dpi = unsafe { GetDpiForWindow(hwnd) };
        let scale = if dpi == 0 { 1.0 } else { dpi as f32 / 96.0 };
        place_children(parent, width, height, scale);
    }

    /// 拖动期每一帧的现场：客户区、待重绘区域、窗口过程收到的绘制消息数，
    /// 以及父窗口缝隙带与子窗口的**屏幕**像素（不是软缓冲）。
    ///
    /// 判读方式：若屏幕像素在「有内容」与「全黑」之间来回跳，而合成器日志里
    /// 缓冲每次都是对的，就说明暗帧是「新露出的区域还没画」，属于重绘时序，
    /// 不是脏区/present 判据画错了。
    fn log_frame(hwnd: HWND) {
        if !diag::enabled() {
            return;
        }
        let paint = diag::PAINT.swap(0, Ordering::Relaxed);
        let erase = diag::ERASE.swap(0, Ordering::Relaxed);
        let ncpaint = diag::NCPAINT.swap(0, Ordering::Relaxed);
        let pos = diag::POSCHANGED.swap(0, Ordering::Relaxed);

        let mut client = RECT::default();
        let mut frame = RECT::default();
        let mut update = RECT::default();
        // SAFETY: 三个矩形都是本栈帧里的可写局部量，句柄是本进程的父窗口。
        let (width, height, pending) = unsafe {
            GetClientRect(hwnd, &mut client);
            GetWindowRect(hwnd, &mut frame);
            GetUpdateRect(hwnd, &mut update, 0);

            (
                client.right - client.left,
                client.bottom - client.top,
                update.right > update.left && update.bottom > update.top,
            )
        };

        let gap = format!(
            "{},{},{}",
            diag::pixel(hwnd, width.saturating_sub(3), height / 2),
            diag::pixel(hwnd, width / 2, height.saturating_sub(3)),
            diag::pixel(hwnd, width / 2, 8),
        );

        let child = table().try_lock().ok().and_then(|guard| {
            guard
                .get(&(hwnd as isize))
                .and_then(|list| list.first().map(|(child, _)| *child))
        });

        let child = match child {
            Some(child) => {
                let child = child as HWND;
                let mut rect = RECT::default();
                // SAFETY: 只读子窗口的矩形，不改变它的状态。
                unsafe {
                    GetWindowRect(child, &mut rect);
                }
                let cx = (rect.left + rect.right) / 2;
                let cy = (rect.top + rect.bottom) / 2;

                // mstsc 的会话窗口用 D3D 合成，它自己的 DC 上 `GetPixel` 永远
                // 返回 `CLR_INVALID`，只有读桌面合成后的像素才能看到实际底色。
                format!(
                    "offset={},{} size={}x{} screen=[{}, {}, {}]",
                    rect.left - frame.left,
                    rect.top - frame.top,
                    rect.right - rect.left,
                    rect.bottom - rect.top,
                    diag::screen_pixel(cx, cy),
                    diag::screen_pixel(rect.right - 4, cy),
                    diag::screen_pixel(cx, rect.bottom - 4),
                )
            }
            None => "none".to_owned(),
        };

        diag::line(format!(
            "t={} event=drag size={width}x{height} update={} paint={paint} erase={erase} ncpaint={ncpaint} poschanged={pos} gap=[{gap}] child=[{child}]",
            diag::stamp(),
            if pending { "yes" } else { "no" },
        ));
    }

    /// 临时诊断：把拖动期现场写进日志文件。定位完成后整块删除。
    mod diag {
        use std::fs::{File, OpenOptions};
        use std::io::Write;
        use std::path::PathBuf;
        use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
        use std::sync::{Mutex, OnceLock};
        use std::time::{SystemTime, UNIX_EPOCH};

        use windows_sys::Win32::Foundation::HWND;
        use windows_sys::Win32::Graphics::Gdi::{GetDC, GetPixel, ReleaseDC};

        pub static PAINT: AtomicU32 = AtomicU32::new(0);
        pub static ERASE: AtomicU32 = AtomicU32::new(0);
        pub static NCPAINT: AtomicU32 = AtomicU32::new(0);
        pub static POSCHANGED: AtomicU32 = AtomicU32::new(0);

        fn sink() -> Option<&'static Mutex<File>> {
            static SINK: OnceLock<Option<Mutex<File>>> = OnceLock::new();

            SINK.get_or_init(|| {
                // 本轮取证默认打开：写到 `%TEMP%\superremote-frame.log`，
                // `SUPERREMOTE_FRAME_LOG` 可以改路径。
                let path = std::env::var_os("SUPERREMOTE_FRAME_LOG")
                    .map(PathBuf::from)
                    .or_else(|| {
                        std::env::var_os("TEMP")
                            .map(|dir| PathBuf::from(dir).join("superremote-frame.log"))
                    })?;

                OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
                    .ok()
                    .map(Mutex::new)
            })
            .as_ref()
        }

        pub fn enabled() -> bool {
            sink().is_some()
        }

        pub fn line(text: String) {
            let Some(sink) = sink() else {
                return;
            };

            if let Ok(mut file) = sink.lock() {
                let _ = writeln!(file, "{text}");
            }
        }

        pub fn stamp() -> u128 {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|elapsed| elapsed.as_millis())
                .unwrap_or(0)
        }

        /// 光标诊断：父窗口替子窗口挡下 `WM_SETCURSOR` 的次数。
        ///
        /// 默认关闭，只有设了 `SUPERREMOTE_CURSOR_LOG` 才写：值为 `1` 时写
        /// `%TEMP%\superremote-cursor.log`，其他非空值当作文件路径。每秒汇总一行，
        /// 免得鼠标移动时把日志刷爆。判读：画面区内移动鼠标时若 `n` 持续
        /// 增长，说明 winit 原本会每移动一次重设一次系统光标；为 0 则说明
        /// mstsc 自己处理了光标，闪烁另有来源。
        pub fn cursor_swallow(parent: isize, child: isize) {
            static SINK: OnceLock<Option<Mutex<File>>> = OnceLock::new();
            static COUNT: AtomicU32 = AtomicU32::new(0);
            static LAST: AtomicU64 = AtomicU64::new(0);

            let Some(sink) = SINK
                .get_or_init(|| {
                    let value = std::env::var_os("SUPERREMOTE_CURSOR_LOG")?;
                    let value = value.to_string_lossy();
                    let path = if value.trim().is_empty() || value == "1" {
                        let temp = std::env::var_os("TEMP")?;
                        PathBuf::from(temp).join("superremote-cursor.log")
                    } else {
                        PathBuf::from(value.as_ref())
                    };
                    OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(path)
                        .ok()
                        .map(Mutex::new)
                })
                .as_ref()
            else {
                return;
            };

            COUNT.fetch_add(1, Ordering::Relaxed);
            let now = stamp() as u64;
            let last = LAST.load(Ordering::Relaxed);
            if now.saturating_sub(last) < 1000 {
                return;
            }
            LAST.store(now, Ordering::Relaxed);
            let count = COUNT.swap(0, Ordering::Relaxed);
            if let Ok(mut file) = sink.lock() {
                let _ = writeln!(
                    file,
                    "t={now} cursor=swallow parent={parent} child={child} n={count}"
                );
            }
        }

        /// 读一个**屏幕**像素（窗口 DC），不是软缓冲里的值。
        pub fn pixel(hwnd: HWND, x: i32, y: i32) -> String {
            if x < 0 || y < 0 {
                return "----".to_owned();
            }

            // SAFETY: `GetDC` 借出的 DC 立刻用 `ReleaseDC` 归还；句柄由调用方保证有效。
            unsafe {
                let dc = GetDC(hwnd);
                if dc.is_null() {
                    return "----".to_owned();
                }

                let value = GetPixel(dc, x, y) as u32;
                ReleaseDC(hwnd, dc);

                format_pixel(value)
            }
        }

        /// 读桌面合成后的**屏幕**像素（`x`/`y` 是桌面坐标）。
        ///
        /// 跨进程的子窗口读不到自己的像素（D3D 表面），只能从屏幕 DC 取。
        pub fn screen_pixel(x: i32, y: i32) -> String {
            // SAFETY: 空句柄表示整个屏幕；借出的 DC 立刻归还。
            unsafe {
                let dc = GetDC(std::ptr::null_mut());
                if dc.is_null() {
                    return "----".to_owned();
                }

                let value = GetPixel(dc, x, y) as u32;
                ReleaseDC(std::ptr::null_mut(), dc);

                format_pixel(value)
            }
        }

        fn format_pixel(value: u32) -> String {
            if value == u32::MAX {
                "----".to_owned()
            } else {
                format!("#{:06x}", value & 0x00FF_FFFF)
            }
        }
    }
}

#[cfg(not(windows))]
mod sizing {
    use super::PaneLayout;

    pub fn set_children(_parent: isize, _children: Vec<(isize, PaneLayout)>) {}
    pub fn clear(_parent: isize) {}
    pub fn is_dragging() -> bool {
        false
    }
    pub fn begin_resize(
        _window: isize,
        _direction: iced::window::Direction,
        _min_size: (f32, f32),
    ) -> bool {
        false
    }
    pub fn frozen_frame() -> Option<crate::rdp_host::FrozenFrame> {
        None
    }
    pub fn frozen_frame_for(_parent: isize) -> Option<crate::rdp_host::FrozenFrame> {
        None
    }
}
