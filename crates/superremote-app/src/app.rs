//! 应用状态：Iced 的 Elm 单向数据流入口。
//!
//! `App` 只持有状态，`Message` 是唯一的状态变更入口，`view` 是状态的纯函数投影。
//! 会话树与标签规则在 `superremote-core`，落盘与加解密在 `superremote-store`，这里只做编排。

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use iced::event::Status as EventStatus;
use iced::widget::text_editor;
use iced::{
    event::listen_with, keyboard, mouse, window, Element, Fill, Point, Size, Subscription, Task,
    Theme,
};
use superremote_core::session::{
    Directory, MoveTarget, OpenMode, RdpAudio, RdpColorDepth, RdpDesktopSize, RdpGraphics,
    RdpHostMode, RdpKeyboard, RdpPerformance, RdpSettings, ResolvedTerminalSettings, Session,
    SessionId, SessionKind, SessionNode, SessionTree, TerminalBackspace, TerminalCharset,
    TerminalColorScheme, TerminalCursor, TerminalHomeEnd, TerminalKeypad, TerminalMouseMode,
    TerminalSettings, TreeRow, WindowSize,
};
use superremote_core::tab::{Tab, TabId, TabManager};
use superremote_net::{Hub, NetEvent, Target};
use superremote_store::{DataPaths, KeyMode, OpenError, Opened, Store, StoreError};

use crate::i18n::{self, Lang};
use crate::rdp::{HostRef, RdpProcess};
use crate::shortcut::{self, Shortcut};
use crate::terminal::{self, Selection, Status as ConnStatus, Terminal};
use crate::theme::Mode;
use crate::{theme, ui};
use superremote_win::{clipboard, rdp_host};

mod rdp;
mod tabs;
mod tree;
mod windows;

/// 自动保存的节流窗口：编辑期间的连续按键合并成一次落盘。
const AUTOSAVE_AFTER: Duration = Duration::from_millis(1200);

/// 成功/普通提示的停留时间。3–5 秒是瞬态反馈的常规区间：
/// 太短读不完，太长会盖住底下的操作条。
const STATUS_LIFETIME_INFO: Duration = Duration::from_millis(4000);
/// 错误提示多留两秒：用户需要时间读完原因。
const STATUS_LIFETIME_ERROR: Duration = Duration::from_millis(6000);
/// 提示条到期检查的节拍。只有提示条存在时才订阅，空闲不唤醒。
const STATUS_TICK: Duration = Duration::from_millis(250);
/// 连接层是线程 + 通道，界面这边按帧拉取事件。
const NET_POLL: Duration = Duration::from_millis(16);
/// 光标闪烁周期。
const BLINK: Duration = Duration::from_millis(530);
/// 判定双击的时间窗。Windows 默认双击间隔是 500ms，但这里是「点一次看详情、
/// 点两次直接打开」，放太宽会让连点两次看详情的操作误开标签，收紧到 400ms。
const DOUBLE_CLICK: Duration = Duration::from_millis(400);
const MIN_MASTER_PASSWORD: usize = 6;
/// 临时文件（`.rdp`、SSH 私钥）超过这个年龄就视为上次异常退出留下的残留。
const STALE_TEMP_AGE: Duration = Duration::from_secs(24 * 60 * 60);
/// 还没拿到真实布局时用的初始网格，`TerminalResized` 会立刻纠正它。
const INITIAL_COLS: u16 = 100;
const INITIAL_ROWS: u16 = 30;
/// 主窗口的默认与最小尺寸（逻辑像素）。
const MAIN_W: f32 = 1180.0;
const MAIN_H: f32 = 760.0;
const MAIN_MIN_W: f32 = 900.0;
const MAIN_MIN_H: f32 = 560.0;
/// 独立窗口的默认与最小尺寸（逻辑像素）。记忆下来的尺寸也按最小值夹一次：
/// 用户把窗口缩到最小再关掉，下次打开不该弹一个被系统强制放大的窗口。
const DETACHED_W: f32 = 980.0;
const DETACHED_H: f32 = 660.0;
const DETACHED_MIN_W: f32 = 520.0;
const DETACHED_MIN_H: f32 = 360.0;

/// 松手落在树的空白处：行控件捕获不到这次事件，由这里收尾。
///
/// 只收 `Status::Ignored`：落在某一行上的松手会被 `Draggable` 捕获（它自己发
/// `DragDrop`），这里就不会再收到。于是「落点」与「取消」互斥，不存在谁先谁后。
fn drag_release(event: iced::Event, status: EventStatus, _window: window::Id) -> Option<Message> {
    match (event, status) {
        (
            iced::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
            EventStatus::Ignored,
        ) => Some(Message::DragCancel),
        _ => None,
    }
}

/// 把配置里的界面偏好应用到进程级的主题与语言。
///
/// 配置里没写（或写了不认识的值）时用默认值——默认值分别是「跟随系统」与中文，
/// 这两个都必须真的执行一次 `set_*`：主题的默认态要求读一次注册表，
/// 只在需要时才解析会让首次启动停在上一次（或亮色）的状态。
fn apply_ui_prefs(ui: &superremote_store::UiDoc) {
    let mode = ui
        .theme
        .as_deref()
        .and_then(Mode::from_code)
        .unwrap_or_default();
    theme::set_mode(mode);

    let lang = ui
        .lang
        .as_deref()
        .and_then(Lang::from_code)
        .unwrap_or_default();
    i18n::set_lang(lang);
}

pub struct App {
    pub(crate) screen: Screen,
    pub(crate) paths: DataPaths,
    pub(crate) store: Option<Store>,
    pub(crate) search: String,
    /// 树与搜索条件推导出的可见行。Iced 的 `view(&self)` 只能借用状态，
    /// 所以派生行必须在 `update` 里物化。
    pub(crate) rows: Vec<TreeRow>,
    pub(crate) tabs: TabManager,
    /// 工作区显示「会话」主页还是当前标签。
    pub(crate) show_home: bool,
    pub(crate) selected: Option<SessionId>,
    pub(crate) draft: Draft,
    /// 私钥是多行文本，输入状态由 `text_editor` 持有；每次动作后同步回 `draft`。
    pub(crate) private_key_editor: text_editor::Content,
    /// 备注是多行文本，输入状态同样由 `text_editor` 持有。
    pub(crate) note_editor: text_editor::Content,
    /// 详情页两个分区的折叠状态（默认展开）。折叠用「裁切 + 高度 0」实现，
    /// 子树始终存在——Iced 按子元素位置 diff，增删子项会让整行状态串位。
    pub(crate) connection_section_open: bool,
    pub(crate) protocol_section_open: bool,
    pub(crate) status: Option<Status>,
    pub(crate) unlock: UnlockForm,
    pub(crate) master_password_draft: String,
    /// 连接层：一个进程一个 Hub，所有会话共享事件队列。
    pub(crate) hub: Hub,
    /// 标签实例 → 终端运行时。
    pub(crate) terminals: HashMap<TabId, Terminal>,
    /// 光标闪烁相位。`view` 只读，所以相位留在状态里。
    pub(crate) blink_on: bool,
    /// 鼠标停在第几个标签上。只用来决定「关闭按钮显不显示」，
    /// 放在状态里是因为 Iced 的按钮样式回调拿不到父容器的悬停态。
    pub(crate) hovered_tab: Option<usize>,
    /// 当前终端里的鼠标选择区。切换标签、滚动或重连时清空。
    pub(crate) selection: Option<Selection>,
    /// 进行中的树节点拖拽。`None` = 没有拖拽（绝大多数时候）。
    pub(crate) drag: Option<Drag>,
    /// 树节点的内存剪贴板（旧版的自定义剪贴板，不是系统剪贴板）。
    pub(crate) node_clipboard: Option<NodeClipboard>,
    /// 标题栏「+」开的临时新建连接标签。
    pub(crate) new_tab: Option<SessionId>,
    /// 每个运行中的 RDP 标签对应一个 mstsc 进程与它被托管的会话窗口。
    pub(crate) rdp: HashMap<TabId, RdpProcess>,
    /// 共用宿主。第一个共享模式的 RDP 标签启动它，其余标签走 `add`；
    /// `None` 表示下一个共享会话要新起进程。
    pub(crate) shared_host: Option<HostRef>,
    /// 上一次共用宿主崩溃的时间，用来识别「30 秒内二次崩溃」。
    pub(crate) shared_crash_at: Option<Instant>,
    /// 正在全屏看哪一路 RDP。全屏时标题栏、侧栏、标签条全部让位。
    pub(crate) rdp_fullscreen: Option<TabId>,
    /// 系统 RDP 优化设置的检测结果（只读）。启动时读一次，用户点「恢复默认」
    /// 之后延迟回读一次，用来刷新设置页上的提示。
    pub(crate) rdp_tuning: superremote_win::sys_tuning::RdpTuning,
    /// 上一次左键点击的节点与时刻，用来识别双击。Iced 的 `button` 只有
    /// `on_press`，没有双击事件，只能靠比对连续两次点击自己判。
    pub(crate) last_click: Option<(SessionId, Instant)>,
    /// 在拿到主窗口句柄之前就请求了 RDP 的标签。句柄一到就补启动，
    /// 避免「程序刚起来就点了连接」变成一次假失败。
    pub(crate) pending_rdp: Vec<TabId>,
    /// 每个 OS 窗口装的是什么。daemon 模式下主窗口也是 `window::open` 开出来的，
    /// 所以它和独立窗口走同一条登记路径。
    pub(crate) windows: HashMap<window::Id, WindowSlot>,
    /// 主窗口。侧栏、标签条与整套会话管理只在它里面。
    pub(crate) main_window: Option<window::Id>,
    /// 每个窗口的原生句柄。RDP 画面要 `SetParent` 到**对应那一个**窗口上。
    pub(crate) hwnds: HashMap<window::Id, isize>,
    /// 每个窗口最近一次量到的尺寸（逻辑像素）。独立窗口关掉时把它写回连接，
    /// 下次「在新窗口打开」直接复用——查询尺寸是异步的，关窗那一刻再问就晚了。
    pub(crate) window_sizes: HashMap<window::Id, WindowSize>,
    /// 哪些窗口当前处于最大化。`window::is_maximized` 是异步查询，点完按钮
    /// 立刻问会拿到旧值，所以按「最后一次切换到的状态」记账；标题栏的图标与
    /// tooltip 都读这里。
    pub(crate) maximized: std::collections::HashSet<window::Id>,
    /// 正开着的会话菜单（一个独立窗口）。见 `Message::SessionMenu`。
    pub(crate) menu: Option<MenuPopup>,
    /// 正开着的状态提示窗口，以及它当时的尺寸（消息被替换时窗口要跟着改大小）。
    /// 提示条画在自己的窗口里：原生 RDP 画面永远盖在 Iced 之上，页面叠层会被整个
    /// 盖掉。见 `ui::toast_bubble`。
    toast: Option<(window::Id, Size)>,
}

/// 一个打开中的会话菜单。
///
/// 菜单必须是真正的窗口：原生 RDP 画面是父窗口的子窗口，永远画在父窗口的客户区
/// 之上，overlay 形式的菜单会被整个盖掉（用户只会看到标题栏里露出的第一条）。
/// 遮掉画面能让菜单露出来，但代价是右键时远端桌面凭空消失，所以换成独立窗口。
pub(crate) struct MenuPopup {
    pub(crate) window: window::Id,
    /// 这条菜单是为哪个运行实例开的：收起时要把键盘焦点还给它画面。
    pub(crate) target: TabId,
    pub(crate) items: Vec<TabAction>,
    pub(crate) groups: Vec<usize>,
}

/// 一个 OS 窗口装的是什么。
// `Detached` 装的是整条会话（Session + Tab），比 `Main` 大得多。装箱会把
// 所有 match 分支都改成解引用，收益只是几十字节的枚举栈占用，不值得。
#[allow(clippy::large_enum_variant)]
pub(crate) enum WindowSlot {
    /// 主窗口：侧栏 + 标签条 + 工作区。
    Main,
    /// 独立窗口：一个窗口只装一个会话实例。
    Detached(Tab),
}

/// 树节点剪贴板：复制存一份深拷贝，剪切只记来源节点的快照与 cut 标志。
#[derive(Debug, Clone)]
pub(crate) struct NodeClipboard {
    pub(crate) node: SessionNode,
    pub(crate) cut: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Screen {
    Remote,
    /// 连接列表页（图标栏第二项）：平铺所有 RDP / SSH / Telnet 条目。
    List,
    Settings,
    /// 标题栏「+」新建的连接标签页（尚未保存到会话树）。
    NewTab,
    Unlock,
    /// 数据目录不可用（版本过新、目录不可写等），只能退出。
    Fatal(String),
}

#[derive(Debug, Default, Clone)]
pub struct Draft {
    pub(crate) name: String,
    pub(crate) host: String,
    pub(crate) port: String,
    pub(crate) username: String,
    pub(crate) password: String,
    pub(crate) private_key: String,
    pub(crate) reveal_password: bool,
    /// 备注（旧版的「描述」）。
    pub(crate) note: String,
}

#[derive(Debug, Clone)]
pub struct Status {
    pub(crate) message: String,
    pub(crate) is_error: bool,
    /// 出现时刻。提示条是**短暂反馈**，不是常驻状态：到点自己消失，
    /// 否则「已保存」会一直挂在界面上，看起来像一个被填了字的输入框。
    pub(crate) shown_at: Instant,
}

impl Status {
    fn info(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            is_error: false,
            shown_at: Instant::now(),
        }
    }

    fn error(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            is_error: true,
            shown_at: Instant::now(),
        }
    }

    fn lifetime(&self) -> Duration {
        if self.is_error {
            STATUS_LIFETIME_ERROR
        } else {
            STATUS_LIFETIME_INFO
        }
    }

    fn is_stale(&self) -> bool {
        self.shown_at.elapsed() >= self.lifetime()
    }
}

#[derive(Debug, Default, Clone)]
pub struct UnlockForm {
    pub(crate) password: String,
    pub(crate) error: Option<String>,
}

/// 一次树节点拖拽的状态。只在按下之后、松开之前存在。
#[derive(Debug, Clone)]
pub struct Drag {
    /// 拖的是哪一条。
    pub source: SessionId,
    /// 位移是否已经超过阈值——没超过就只是一次点击，不该动树。
    pub active: bool,
    /// 当前悬在哪一行上，作为落点候选；移到空白处就变回 `None`。
    pub over: Option<SessionId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DraftField {
    Name,
    Host,
    Port,
    Username,
    Password,
    Note,
}

/// 提权实例写完之后的回读，用来判断这一次到底成功没有。两条设置互相独立，
/// 不能用 `needs_fix()` 一刀切，否则关掉安全桌面也会被报成「未恢复」。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TuningIntent {
    /// 恢复 AVC444 / 传输方式的默认值。
    Defaults,
    /// 开关 UAC 安全桌面，`true` 表示要开启。
    SecureDesktop(bool),
}

/// 单条 RDP 设置的一次修改。`None` 表示改回「继承默认」。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RdpEdit {
    Performance(Option<RdpPerformance>),
    ColorDepth(Option<RdpColorDepth>),
    Audio(Option<RdpAudio>),
    AudioCapture(Option<bool>),
    Keyboard(Option<RdpKeyboard>),
    Redirect {
        field: RdpRedirect,
        value: Option<bool>,
    },
    DesktopSize(Option<RdpDesktopSize>),
    HostMode(Option<RdpHostMode>),
    Graphics(Option<RdpGraphics>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RdpRedirect {
    Printers,
    Clipboard,
    Smartcards,
    Ports,
    Drives,
}

/// 单条 SSH/Telnet 终端设置的一次修改。`None` 表示改回「继承默认」。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalEdit {
    FontFamily(Option<String>),
    FontSize(Option<u8>),
    Cursor(Option<TerminalCursor>),
    Backspace(Option<TerminalBackspace>),
    HomeEnd(Option<TerminalHomeEnd>),
    ColorScheme(Option<TerminalColorScheme>),
    Charset(Option<TerminalCharset>),
    MouseMode(Option<TerminalMouseMode>),
    Keypad(Option<TerminalKeypad>),
}

impl TerminalEdit {
    pub(crate) fn apply(self, settings: &mut TerminalSettings) {
        match self {
            Self::FontFamily(value) => {
                settings.font_family = value.filter(|value| !value.trim().is_empty())
            }
            Self::FontSize(value) => settings.font_size = value,
            Self::Cursor(value) => settings.cursor = value,
            Self::Backspace(value) => settings.backspace = value,
            Self::HomeEnd(value) => settings.home_end = value,
            Self::ColorScheme(value) => settings.color_scheme = value,
            Self::Charset(value) => settings.charset = value,
            Self::MouseMode(value) => settings.mouse_mode = value,
            Self::Keypad(value) => settings.keypad = value,
        }
    }
}

impl RdpEdit {
    pub(crate) fn apply(self, settings: &mut RdpSettings) {
        match self {
            Self::Performance(value) => settings.performance = value,
            Self::ColorDepth(value) => settings.color_depth = value,
            Self::Audio(value) => settings.audio = value,
            Self::AudioCapture(value) => settings.audio_capture = value,
            Self::Keyboard(value) => settings.keyboard = value,
            Self::DesktopSize(value) => settings.desktop_size = value,
            Self::HostMode(value) => settings.host_mode = value,
            Self::Graphics(value) => settings.graphics = value,
            Self::Redirect { field, value } => match field {
                RdpRedirect::Printers => settings.redirect_printers = value,
                RdpRedirect::Clipboard => settings.redirect_clipboard = value,
                RdpRedirect::Smartcards => settings.redirect_smartcards = value,
                RdpRedirect::Ports => settings.redirect_ports = value,
                RdpRedirect::Drives => settings.redirect_drives = value,
            },
        }
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    ShowHome,
    /// 打开连接列表页（图标栏第二项）。
    ShowList,
    /// 标题栏「+」：打开/聚焦新建连接标签。
    NewTabOpen,
    NewTabActivate,
    NewTabClose,
    /// 树/列表空白处右键：先取消选中（旧版行为），菜单只留新建与粘贴。
    ClearSelection,
    ShowSettings,
    SearchChanged(String),
    ToggleDirectory(SessionId),
    SelectNode(SessionId),
    NewDirectory,
    NewSession(SessionKind),
    DraftChanged(DraftField, String),
    /// 口令按住显示 / 松开隐藏。显式给值而不是 toggle：按下=显示，松开=隐藏。
    SetPasswordVisible(bool),
    ToggleConnectionSection,
    ToggleProtocolSection,
    ApplyDraft,
    ClearDraftPassword,
    DeleteSelected,
    /// 会话树右键菜单选中的一条动作。id 随动作走，菜单是按行构造的。
    TreeAction(TreeAction),
    /// 右键一条树行：只标记选中并回填草稿，**不跳页**——右键不该把用户
    /// 从正在看的连接页里拽走（`SelectNode` 才会跳页）。
    HighlightNode(SessionId),
    /// 树行左键按下：记下拖拽源。
    DragStart(SessionId),
    /// 拖拽位移超过阈值后的跟手移动。
    DragMove(Point),
    /// 鼠标进/出某一行：`Some` 是落点候选，`None` 表示移到空白处了。
    DragHover(Option<SessionId>),
    /// 松手落在某一行上。
    DragDrop(SessionId),
    /// 松手落在空白处：取消本次拖拽。
    DragCancel,
    /// 标签条右键菜单选中的一条动作。
    TabAction(TabAction),
    /// 会话菜单：在父窗口内的 `anchor` 处开一个真正的弹层窗口。
    /// 菜单不能是 overlay——原生 RDP 画面是父窗口的子窗口，永远画在 Iced 之上，
    /// overlay 会被整个盖掉。
    SessionMenu(TabId, Point),
    /// 弹层菜单里选中的一条动作（关菜单 + 执行）。
    MenuChosen(TabAction),
    /// 某个 OS 窗口的生命周期事件；菜单靠它实现「点了别处就收起」。
    WindowEvent(window::Id, iced::window::Event),
    /// 提示条窗口的原生句柄到了：按 `SW_SHOWNA` 显示，不抢远端画面的键盘焦点。
    ToastHandle(window::Id, Option<isize>),
    OpenSelected,
    /// 在后台打开选中的连接：建标签、连上，但不切走当前视图。
    OpenInBackground(SessionId),
    ActivateTab(usize),
    CloseTab(usize),
    /// 鼠标进出某个标签，只影响关闭按钮的显隐。
    HoverTab(Option<usize>),
    MasterPasswordChanged(String),
    EnableMasterPassword,
    DisableMasterPassword,
    UnlockPasswordChanged(String),
    SubmitUnlock,
    RevealDataDir,
    ChooseDataDir,
    ResetDataDir,
    Autosave,
    DismissStatus,
    /// 提示条到期检查；只有 `status` 存在时才订阅。
    ExpireStatus,
    /// 拉取连接层事件。
    NetPoll,
    /// 光标闪烁相位翻转。
    Blink,
    /// 当前标签的键盘输入。
    KeyInput(Box<keyboard::Event>),
    /// 画布量出了新的行列数。
    TerminalResized(TabId, u16, u16),
    /// 终端里滚动历史。
    TerminalScrolled(TabId, i32),
    /// 更新/清空终端选择区。
    TerminalSelection(Option<Selection>),
    /// 把选择区复制到系统剪贴板。
    CopySelection,
    /// 把系统剪贴板粘贴到当前终端。
    PasteClipboard,
    /// 私钥编辑器的内部动作（光标、粘贴、删除）。
    PrivateKeyAction(text_editor::Action),
    NoteAction(text_editor::Action),
    /// 清除已保存的 SSH 私钥。
    ClearPrivateKey,
    /// 重新连接当前标签。
    Reconnect(TabId),
    /// RDP 走内嵌窗口，这里负责启动 mstsc 并把它挂进当前标签。
    LaunchRdp(TabId),
    /// 主动断开某一路 RDP。
    DisconnectRdp(TabId),
    /// 进入/退出 RDP 全屏。
    ToggleRdpFullscreen(TabId),
    ExitRdpFullscreen,
    /// 清除交给系统凭据管理器的 RDP 口令。
    ForgetRdpCredentials(TabId),
    /// 设置页里切换外观。
    SetTheme(Mode),
    /// 设置页里切换界面语言。
    SetLang(Lang),
    /// 修改当前 RDP 连接的一项设置。
    RdpEdit(RdpEdit),
    /// 清空当前 RDP 连接的全部覆盖，回到继承默认。
    ResetRdpSettings,
    /// 把当前 RDP 连接的生效设置固化为全局默认。
    MakeRdpDefaults,
    /// 修改当前终端连接的一项设置。
    TerminalEdit(TerminalEdit),
    /// 清空当前终端连接的全部覆盖，回到继承默认。
    ResetTerminalSettings,
    /// 把当前终端连接的生效设置固化为全局默认。
    MakeTerminalDefaults,
    /// 设置页里修改全局 RDP 默认的一项。
    RdpDefaultEdit(RdpEdit),
    /// 清空全部 RDP 全局默认，回到内置值。
    ResetRdpDefaults,
    /// 重新检测系统 RDP 优化设置（只读）。
    RefreshRdpTuning,
    /// 请求提权把两项系统 RDP 优化设置恢复默认。
    FixRdpTuning,
    /// 请求提权开关 UAC 安全桌面。`false` = 提权提示在当前桌面显示。
    SetSecureDesktop(bool),
    /// 提权请求发出后延迟回读的检测结果，连同「这次写的是哪一项」。
    RdpTuningProbed(superremote_win::sys_tuning::RdpTuning, TuningIntent),
    /// 设置页里修改全局终端默认的一项。
    TerminalDefaultEdit(TerminalEdit),
    /// 清空全部终端全局默认。
    ResetTerminalDefaults,
    /// 改当前连接的首选打开方式。`None` = 继承全局默认。
    OpenModeEdit(Option<OpenMode>),
    /// 改全局默认的打开方式。`None` = 用内置默认（选项卡）。
    OpenModeDefaultEdit(Option<OpenMode>),
    /// 某个窗口的原生句柄已取到（异步查询）。RDP 内嵌要用它当父窗口。
    WindowHandle(window::Id, Option<isize>),
    /// 一个 OS 窗口已经开出来（`window::open` 的回执）。
    WindowOpened(window::Id),
    /// 一个 OS 窗口被关掉了，收掉它身上的运行实例。
    WindowClosed(window::Id),
    /// 把某条连接开到独立窗口里（SSH/Telnet 进新窗口；RDP 让 mstsc 自己开窗）。
    OpenInNewWindow(SessionId),
    WindowDrag(window::Id),
    /// 窗口控件都带 id：独立窗口和主窗口各自的按钮不能靠 `latest()` 猜。
    WindowMinimize(window::Id),
    WindowToggleMaximize(window::Id),
    WindowClose(window::Id),
    /// 拖某条缩放热区开始改变**指定窗口**的大小（多窗口下不能靠 latest 猜）。
    WindowResize(window::Id, window::Direction),
    /// 某个窗口尺寸变了：RDP 原生画面要跟着重新贴合，顺手记下新尺寸。
    WindowResized(window::Id, Size<f32>),
    /// 轮询 mstsc 的会话窗口：发现、挂载、贴边、退出。
    RdpTick,
}

impl App {
    /// 返回状态与「开局就要跑」的任务。自动打开的那一路连接可能命中
    /// 「打开即全屏」，那是一个窗口任务，必须交回运行时，不能丢。
    pub fn new() -> (Self, Task<Message>) {
        let (paths, path_notice) = DataPaths::resolve_with_notice();
        let app = Self::with_paths(paths);

        #[cfg(debug_assertions)]
        let (mut app, opened) = app.auto_open_for_smoke_test();
        #[cfg(not(debug_assertions))]
        let (mut app, opened) = (app, Task::none());

        // 旧版 AppData 数据首次复制到程序目录后给一次提示；若已经有更重要的
        // 启动错误（配置恢复、密钥不匹配），不要把它盖掉。
        if app.status.is_none() {
            if let Some(superremote_store::PathNotice::Migrated { to }) = path_notice {
                app.status = Some(Status::info(
                    i18n::lang().data_moved(&to.display().to_string()),
                ));
            }
        }

        (app, opened)
    }

    /// 程序入口。daemon 模式不自动建窗口，主窗口也要自己开——这样它和独立窗口
    /// 走同一条登记路径，不会出现两套语义。
    ///
    /// 原生句柄只用于把 mstsc 的会话窗口 `SetParent` 进来；拿不到时
    /// RDP 会退回「只建立连接、不显示画面」的失败提示，而不是 panic。
    pub fn boot() -> (Self, Task<Message>) {
        let settings = main_window_settings();
        let initial = settings.size;
        let (id, opened) = window::open(settings);
        let (mut app, smoke) = Self::new();
        app.windows.insert(id, WindowSlot::Main);
        app.main_window = Some(id);
        // 启动时就登记主窗口尺寸：内容列宽度与分栏断点都按它算，
        // 等第一次 resize 才记的话，最大化启动会先按 1180 排一帧。
        app.window_sizes.insert(
            id,
            WindowSize {
                width: initial.width.round() as u32,
                height: initial.height.round() as u32,
            },
        );
        let auto = app.debug_auto_open();

        (
            app,
            Task::batch([Self::window_ready_task(opened), auto, smoke]),
        )
    }

    /// GUI 自动化验证钩子：`SUPERREMOTE_DEBUG_OPEN=<会话 id>` 启动即打开该会话并连线，
    /// `SUPERREMOTE_DEBUG_OPEN=win:<会话 id>` 开到独立窗口。环境变量不存在时零影响。
    ///
    /// 为什么需要它：控制台锁屏时 `TrackMouseEvent` 会为每一次进入立刻补发
    /// `WM_MOUSELEAVE`（真实光标被锁屏顶层窗口挡住），Iced 批处理用的光标快照
    /// 变成 `Unavailable`，合成点击走不进任何控件。截图验证只能从启动链路进。
    fn debug_auto_open(&mut self) -> Task<Message> {
        let Ok(spec) = std::env::var("SUPERREMOTE_DEBUG_OPEN") else {
            return Task::none();
        };

        match spec.strip_prefix("win:") {
            Some(id) => self.open_in_new_window(id),
            None => {
                self.selected = Some(spec.clone());
                self.draft = self.load_draft(&spec);
                self.open_session(&spec, true)
            }
        }
    }

    /// 用指定数据目录构造。集成测试靠它把 `config.toml`/`secrets.toml` 关进临时目录。
    pub fn with_paths(paths: DataPaths) -> Self {
        // release 档 `panic = "abort"`，上次异常退出不会跑 `Drop`，`.rdp` 与私钥
        // 临时文件会留在磁盘上；启动时补删一次。
        rdp::clean_stale_temp_files(STALE_TEMP_AGE);

        // 主口令模式下 `Store::open` 拿不到配置，但解锁页本身也要正确的主题与语言，
        // 所以先只读地窥一眼 `[ui]`，读不到就用默认值。
        apply_ui_prefs(&Store::peek_ui(&paths));

        let mut app = Self {
            screen: Screen::Remote,
            paths: paths.clone(),
            store: None,
            search: String::new(),
            rows: Vec::new(),
            tabs: TabManager::default(),
            show_home: true,
            selected: None,
            draft: Draft::default(),
            private_key_editor: text_editor::Content::with_text(""),
            note_editor: text_editor::Content::with_text(""),
            connection_section_open: true,
            protocol_section_open: true,
            status: None,
            unlock: UnlockForm::default(),
            master_password_draft: String::new(),
            hub: Hub::new(),
            terminals: HashMap::new(),
            blink_on: true,
            hovered_tab: None,
            selection: None,
            drag: None,
            node_clipboard: None,
            new_tab: None,
            rdp: HashMap::new(),
            shared_host: None,
            shared_crash_at: None,
            rdp_fullscreen: None,
            rdp_tuning: superremote_win::sys_tuning::probe(),
            last_click: None,
            pending_rdp: Vec::new(),
            windows: HashMap::new(),
            main_window: None,
            hwnds: HashMap::new(),
            window_sizes: HashMap::new(),
            maximized: std::collections::HashSet::new(),
            toast: None,
            menu: None,
        };

        match Store::open(paths, None) {
            Ok(opened) => app.adopt_store(opened),
            Err(OpenError::NeedsPassword) => app.screen = Screen::Unlock,
            Err(other) => app.screen = Screen::Fatal(other.to_string()),
        }

        app
    }

    /// 消息分发 + 提示条窗口的对账。
    ///
    /// `self.status = Some(..)` 散落在三十多处，逐个记得开窗口一定会漏；放在这里
    /// 统一比较「status 有没有」与「窗口开没开」，两条路径（出现/消失）都在一处。
    pub fn update(&mut self, message: Message) -> Task<Message> {
        let task = self.apply(message);
        Task::batch([task, self.sync_toast()])
    }

    fn apply(&mut self, message: Message) -> Task<Message> {
        match message {
            // `show_home` 只决定「主页标签 vs 连接标签」，光设它回不到会话页——
            // 还必须把 `screen` 从 `Settings` 摘出来。之前这两条都漏了这一步，
            // 结果在设置页里点侧栏「会话」或点树上的条目，界面纹丝不动。
            Message::ShowHome => {
                self.go_sessions();
                self.sync_rdp_layout();
            }
            Message::NewTabOpen | Message::NewTabActivate => {
                // 锁定/致命错误页没有可写入的 Store，不能从标题栏绕过解锁。
                if self.store.is_none() {
                    return Task::none();
                }
                if self.new_tab.is_none() {
                    // 与右键「新建 SSH」同一条创建路径：先落一个空节点，
                    // 页面直接复用详情编辑器，两处看到的才是同一套控件。
                    self.create_node(NewKind::Session(SessionKind::Ssh));
                    self.new_tab = self.selected.clone();
                }
                if let Some(id) = self.new_tab.clone() {
                    if !self.focus_new_tab(&id) {
                        return Task::none();
                    }
                }
                self.screen = Screen::NewTab;
                self.show_home = false;
                self.sync_rdp_layout();
            }
            Message::NewTabClose => {
                // 在设置页/列表页关闭这个后台标签时，不要把用户也拽回主页。
                let was_active = self.screen == Screen::NewTab;
                if let Some(id) = self.new_tab.take() {
                    self.discard_blank_new_tab(&id);
                }
                if was_active {
                    self.go_sessions();
                }
            }
            Message::ClearSelection => {
                self.selected = None;
                self.draft = Draft::default();
                self.private_key_editor = text_editor::Content::with_text("");
                self.note_editor = text_editor::Content::with_text("");
            }
            Message::ShowList => {
                self.screen = Screen::List;
                self.show_home = false;
                self.selection = None;
                self.sync_rdp_layout();
            }
            Message::ShowSettings => {
                self.screen = Screen::Settings;
                self.sync_rdp_layout();
            }
            Message::SearchChanged(query) => {
                self.search = query;
                self.sync_rows();
            }
            Message::ToggleDirectory(id) => {
                if let Some(store) = self.store.as_mut() {
                    store.tree_mut().toggle(&id);
                }
                self.sync_rows();
            }
            Message::SelectNode(id) => {
                self.load_draft_into_editor(&id);
                self.go_sessions();
                self.status = None;
                // 双击直接打开。目录不参与：双击目录的语义是展开/折叠，
                // 已经由 `ToggleDirectory` 负责，走到 `open_session` 只会
                // 弹出「目录不能打开」。
                if self.register_click(&id) {
                    return self.open_session(&id, true);
                }
            }
            Message::NewDirectory => self.create_node(NewKind::Directory),
            Message::NewSession(kind) => self.create_node(NewKind::Session(kind)),
            Message::DraftChanged(field, value) => match field {
                DraftField::Name => self.draft.name = value,
                DraftField::Host => self.draft.host = value,
                DraftField::Port => self.draft.port = value,
                DraftField::Username => self.draft.username = value,
                DraftField::Note => self.draft.note = value,
                DraftField::Password => self.draft.password = value,
            },
            Message::SetPasswordVisible(visible) => self.draft.reveal_password = visible,
            Message::ToggleConnectionSection => {
                self.connection_section_open = !self.connection_section_open
            }
            Message::ToggleProtocolSection => {
                self.protocol_section_open = !self.protocol_section_open
            }
            Message::ApplyDraft => self.apply_draft(),
            Message::ClearDraftPassword => self.clear_draft_password(),
            Message::DeleteSelected => self.delete_selected(),
            Message::TreeAction(action) => return self.run_tree_action(action),
            Message::HighlightNode(id) => self.load_draft_into_editor(&id),
            Message::DragStart(source) => {
                self.drag = Some(Drag {
                    source,
                    active: false,
                    over: None,
                });
            }
            Message::DragMove(_) => {
                if let Some(drag) = self.drag.as_mut() {
                    drag.active = true;
                }
            }
            Message::DragHover(over) => {
                if let Some(drag) = self.drag.as_mut() {
                    if drag.active {
                        drag.over = over;
                    }
                }
            }
            Message::DragDrop(target) => return self.finish_drag(Some(target)),
            Message::DragCancel => return self.finish_drag(None),
            Message::TabAction(action) => return self.run_tab_action(action),
            Message::SessionMenu(instance, anchor) => {
                return self.open_session_menu(instance, anchor)
            }
            Message::MenuChosen(action) => {
                let closed = self.close_session_menu();
                return Task::batch([closed, self.run_tab_action(action)]);
            }
            Message::WindowEvent(window, event) => return self.on_window_event(window, event),
            Message::ToastHandle(window, handle) => {
                // 只对「还开着的那一个」动手：提示条可能在这条回执到达前就过期收起了。
                if let (Some(hwnd), Some((id, _))) = (handle, self.toast) {
                    if id == window {
                        rdp_host::show(hwnd);
                    }
                }
            }
            Message::OpenSelected => return self.open_selected(),
            Message::OpenInBackground(id) => return self.open_session(&id, false),
            Message::ActivateTab(index) => {
                self.tabs.activate(index);
                // 与 `ShowHome` 同一类坑：只改 `show_home` 不动 `screen`，
                // 在设置页点标题栏标签会「标签高亮变了、画面还停在设置页」。
                self.screen = Screen::Remote;
                self.show_home = false;
                self.selection = None;
                // 它之前可能是后台标签，帧攒着没物化；切到面前先补上。
                self.refresh_active_frame();
                self.sync_rdp_layout();
                self.focus_active_rdp();
            }
            Message::CloseTab(index) => {
                let closed = self.tabs.tabs().get(index).map(|tab| tab.instance);
                self.tabs.close(index);
                self.hovered_tab = None;
                if let Some(instance) = closed {
                    self.forget_terminal(instance);
                }
                // 关掉最后一个标签后还停在"标签视图"上会显示空白，回到主页。
                if self.tabs.is_empty() {
                    self.go_sessions();
                }
                // 关掉的可能是活动标签，邻居被自动激活——它原来在后台，
                // 帧可能攒着没物化。
                self.refresh_active_frame();
                self.sync_rdp_layout();
                return self.leave_fullscreen_if_gone();
            }
            Message::HoverTab(index) => self.hovered_tab = index,
            Message::MasterPasswordChanged(value) => self.master_password_draft = value,
            Message::EnableMasterPassword => self.set_master_password(),
            Message::DisableMasterPassword => self.clear_master_password(),
            Message::UnlockPasswordChanged(value) => {
                self.unlock.password = value;
                self.unlock.error = None;
            }
            Message::SubmitUnlock => self.submit_unlock(),
            Message::RevealDataDir => self.reveal_data_dir(),
            Message::ChooseDataDir => self.choose_data_dir(),
            Message::ResetDataDir => self.reset_data_dir(),
            Message::Autosave => self.save_now(),
            Message::DismissStatus => self.status = None,
            Message::ExpireStatus => {
                if self.status.as_ref().is_some_and(Status::is_stale) {
                    self.status = None;
                }
            }
            Message::NetPoll => self.pump_network(),
            Message::Blink => self.blink_on = !self.blink_on,
            Message::KeyInput(event) => return self.handle_key(*event),
            Message::TerminalResized(instance, cols, rows) => {
                if self.active_terminal_instance() == Some(instance) {
                    self.selection = None;
                }
                self.resize_terminal(instance, cols, rows);
            }
            Message::TerminalScrolled(instance, delta) => {
                if self.active_terminal_instance() == Some(instance) {
                    self.selection = None;
                }
                if let Some(terminal) = self.terminals.get_mut(&instance) {
                    terminal.scroll(delta);
                }
            }
            Message::TerminalSelection(selection) => self.selection = selection,
            Message::CopySelection => self.copy_selection(),
            Message::PasteClipboard => self.paste_clipboard(),
            Message::NoteAction(action) => {
                self.note_editor.perform(action);
                self.draft.note = self.note_editor.text();
            }
            Message::PrivateKeyAction(action) => {
                self.private_key_editor.perform(action);
                self.draft.private_key = self.private_key_editor.text();
            }
            Message::ClearPrivateKey => self.clear_draft_private_key(),
            Message::Reconnect(instance) => {
                self.selection = None;
                let launched = self.reconnect(instance);
                self.sync_rdp_layout();
                return launched;
            }
            Message::LaunchRdp(instance) => return self.launch_rdp(instance),
            Message::DisconnectRdp(instance) => {
                self.disconnect_rdp(instance);
                return self.leave_fullscreen_if_gone();
            }
            Message::ToggleRdpFullscreen(instance) => {
                if self.rdp_fullscreen == Some(instance) {
                    self.rdp_fullscreen = None;
                    return Self::set_window_fullscreen(false);
                }

                return self.enter_rdp_fullscreen(instance);
            }
            Message::ExitRdpFullscreen => {
                if self.rdp_fullscreen.take().is_some() {
                    self.sync_rdp_layout();
                    return Self::set_window_fullscreen(false);
                }
            }
            Message::ForgetRdpCredentials(instance) => self.forget_rdp_credentials(instance),
            Message::SetTheme(mode) => self.set_theme(mode),
            Message::SetLang(lang) => self.set_lang(lang),
            Message::RdpEdit(edit) => self.edit_rdp_settings(edit),
            Message::ResetRdpSettings => self.reset_rdp_settings(),
            Message::MakeRdpDefaults => self.make_rdp_defaults(),
            Message::TerminalEdit(edit) => self.edit_terminal_settings(edit),
            Message::ResetTerminalSettings => self.reset_terminal_settings(),
            Message::MakeTerminalDefaults => self.make_terminal_defaults(),
            Message::RdpDefaultEdit(edit) => self.edit_rdp_defaults(edit),
            Message::ResetRdpDefaults => self.reset_rdp_defaults(),
            Message::RefreshRdpTuning => {
                self.rdp_tuning = superremote_win::sys_tuning::probe();
            }
            Message::FixRdpTuning => return self.fix_rdp_tuning(),
            Message::SetSecureDesktop(enabled) => return self.set_secure_desktop(enabled),
            Message::RdpTuningProbed(state, intent) => {
                self.rdp_tuning = state;
                let t = i18n::t();
                let (ok, ok_text, fail_text) = match intent {
                    TuningIntent::Defaults => {
                        (!state.needs_fix(), t.rdp_tuning_fixed, t.rdp_tuning_failed)
                    }
                    TuningIntent::SecureDesktop(want) => (
                        state.secure_desktop() == want,
                        t.uac_desktop_applied,
                        t.uac_desktop_failed,
                    ),
                };
                self.status = Some(if ok {
                    Status::info(ok_text.to_owned())
                } else {
                    Status::error(fail_text.to_owned())
                });
            }
            Message::TerminalDefaultEdit(edit) => self.edit_terminal_defaults(edit),
            Message::ResetTerminalDefaults => self.reset_terminal_defaults(),
            Message::OpenModeEdit(mode) => self.edit_open_mode(mode),
            Message::OpenModeDefaultEdit(mode) => self.edit_default_open_mode(mode),
            Message::WindowHandle(window, handle) => {
                if let Some(handle) = handle {
                    self.hwnds.insert(window, handle);
                }

                // 逐个补启动：`launch_rdp` 内部会再查一次句柄，查不到的重新入列，
                // 于是多个窗口的 RDP 可以在各自句柄就绪时分别启动。
                let pending: Vec<TabId> = std::mem::take(&mut self.pending_rdp);
                let mut launched = Vec::new();
                for instance in pending {
                    if self.hwnd_of_instance(instance).is_some() {
                        launched.push(self.launch_rdp(instance));
                    } else if handle.is_none() {
                        // 句柄明确取不到（不是「还没回来」）：报一次失败，不再重试。
                        let message = i18n::t().rdp_window_missing.to_owned();
                        self.terminal_slot(
                            instance,
                            INITIAL_COLS,
                            INITIAL_ROWS,
                            ResolvedTerminalSettings::default(),
                        )
                        .status = ConnStatus::Failed(message.clone());
                        self.status = Some(Status::error(message));
                    } else {
                        // 句柄属于别的窗口，等那一个的 WindowHandle。
                        self.pending_rdp.push(instance);
                    }
                }

                launched.push(self.poll_rdp());
                return Task::batch(launched);
            }
            Message::WindowOpened(_) => {}
            Message::WindowClosed(window) => {
                // 窗口已经没了，把它身上的运行实例一并收掉，免得连接还挂着。
                // 尺寸要在摘掉登记之前记下来：连接项里存的就是「上次关窗时多大」。
                let size = self.window_sizes.remove(&window);
                match self.windows.remove(&window) {
                    Some(WindowSlot::Detached(tab)) => {
                        self.remember_window_size(&tab.session.id, size);
                        self.forget_terminal(tab.instance);
                    }
                    Some(WindowSlot::Main) => self.main_window = None,
                    None => {}
                }
                self.maximized.remove(&window);
                self.hwnds.remove(&window);
                // 句柄值可能被系统复用给别的新窗口：必须趁现在把同步贴边的
                // 表项和子类化一起摘掉，否则新窗口会被我们错误地摆子窗口。
                if let Some(hwnd) = self.hwnds.remove(&window) {
                    rdp_host::clear_sizing_window(hwnd);
                }

                // iced 的 daemon **不会**在窗口关完后自己退出（0.14 文档原话：
                // "a Daemon will not stop running when all its windows are closed"）。
                // 少了这一句，关掉窗口只留下一个没有窗口的进程：它占着 exe 不放，
                // 用户以为程序已经关了，下次构建还会撞上「文件被占用」。
                if self.should_exit_after_close() {
                    return iced::exit();
                }
            }
            Message::OpenInNewWindow(id) => return self.open_in_new_window(&id),
            Message::WindowDrag(window) => return window::drag(window),
            Message::WindowMinimize(window) => {
                return window::minimize(window, true);
            }
            Message::WindowToggleMaximize(window) => {
                // 记账放在切换之前：查询窗口状态是异步的，切完再问会读到旧值。
                if self.maximized.contains(&window) {
                    self.maximized.remove(&window);
                } else {
                    self.maximized.insert(window);
                }
                return window::toggle_maximize(window);
            }
            Message::WindowClose(window) => return self.close_window(window),
            Message::WindowResize(window, direction) => {
                let hwnd = self.hwnds.get(&window).copied();
                let min = self.window_min_size(window);
                if let Some(hwnd) = hwnd {
                    // 自己跟着鼠标改尺寸：系统模态缩放循环里 Iced 不更新版面，
                    // 新露出的区域只能先画底色，拖动时就会闪。
                    if rdp_host::begin_resize(hwnd, direction, min) {
                        return Task::none();
                    }
                }
                return window::drag_resize(window, direction);
            }
            Message::WindowResized(window, size) => {
                self.window_sizes.insert(window, remembered_size(size));
                // 最大化、还原、拖边还原、全屏往返都会改窗口尺寸。其中只有按钮那一下
                // 会翻 `maximized` 记账，系统自己做的不经过这里，记账就会和真实状态
                // 相反——标题栏于是画出「还原」双框、点下去却在做最大化。尺寸变一次
                // 就按 Win32 实测值校正一次，字形、tooltip 始终跟着窗口走。
                if let Some(hwnd) = self.hwnds.get(&window).copied() {
                    if rdp_host::is_zoomed(hwnd) {
                        self.maximized.insert(window);
                    } else {
                        self.maximized.remove(&window);
                    }
                }
                self.sync_rdp_layout();
            }
            Message::RdpTick => return self.poll_rdp(),
        }

        Task::none()
    }

    /// 每个窗口各自渲染：主窗口是整套管理界面，独立窗口只有一个会话，
    /// 菜单与提示条窗口各只装一块面板。
    pub fn view(&self, window: window::Id) -> Element<'_, Message> {
        if self.toast.map(|(id, _)| id) == Some(window) {
            return match self.status.as_ref() {
                Some(status) => ui::toast_bubble(status),
                // 状态已经清了、窗口还在收的路上：画空占位，不要闪一下旧内容。
                None => iced::widget::space().width(Fill).height(Fill).into(),
            };
        }

        if self.menu.as_ref().is_some_and(|menu| menu.window == window) {
            return ui::workspace::session_menu_view(self);
        }

        match self.windows.get(&window) {
            Some(WindowSlot::Detached(tab)) => self.detached_view(window, tab),
            // 还没登记的窗口按主窗口渲染：`window::open` 的回执可能晚一帧到。
            _ => self.main_view(window),
        }
    }

    /// 标题栏里跟在品牌名后面的当前页面名。
    pub fn context_title(&self) -> String {
        let t = i18n::t();

        match &self.screen {
            Screen::Settings => t.nav_settings.to_owned(),
            Screen::List => t.nav_sessions.to_owned(),
            Screen::NewTab => self
                .new_tab_session()
                .map(|session| session.name.clone())
                .unwrap_or_else(|| t.new_tab_title.to_owned()),
            Screen::Unlock => t.unlock_action.to_owned(),
            Screen::Fatal(_) => t.fatal_title.to_owned(),
            Screen::Remote => match self.active_tab() {
                Some(tab) => tab.title().to_owned(),
                None => {
                    if self.show_home {
                        t.tab_details.to_owned()
                    } else {
                        t.nav_sessions.to_owned()
                    }
                }
            },
        }
    }

    /// 只在真的有活需要时挂定时器，空闲状态不产生任何唤醒。
    pub fn subscription(&self) -> Subscription<Message> {
        let mut subscriptions = Vec::new();

        if self.store.as_ref().is_some_and(Store::is_dirty) {
            subscriptions.push(iced::time::every(AUTOSAVE_AFTER).map(|_| Message::Autosave));
        }

        // 提示条到点自己消失。只在真的有提示时订阅，空闲时不多一次唤醒。
        if self.status.is_some() {
            subscriptions.push(iced::time::every(STATUS_TICK).map(|_| Message::ExpireStatus));
        }

        // 有连接还在跑就按帧拉事件；全部结束（或没有连接）时彻底停掉唤醒。
        if self
            .terminals
            .values()
            .any(|term| !term.status.is_settled())
        {
            subscriptions.push(iced::time::every(NET_POLL).map(|_| Message::NetPoll));
        }

        // 键盘监听常驻：快捷键要能在任意页面生效，不能只在终端前台时挂。
        // `keyboard::listen` 只送**未被控件捕获**的事件，所以文本框、下拉框里
        // 的按键天然不会漏进来——这正是快捷键能与输入框共存的前提。
        subscriptions
            .push(iced::keyboard::listen().map(|event| Message::KeyInput(Box::new(event))));

        // 光标闪烁只跟终端走，没有终端就不必空转。
        if self.active_terminal().is_some() {
            subscriptions.push(iced::time::every(BLINK).map(|_| Message::Blink));
        }

        // 窗口尺寸变化不是空闲唤醒：只有用户拖边、最大化或切 DPI 时才会来。
        // 带上窗口 id——独立窗口的尺寸要各自记住，不能记到最新那一个头上。
        subscriptions.push(
            iced::window::resize_events()
                .map(|(window, size)| Message::WindowResized(window, size)),
        );

        // 拖拽收尾：松手落在树的空白处时没有任何行控件捕获这次事件，
        // 由这条全局订阅发 `DragCancel`。它常驻但极廉价（纯过滤，不产生唤醒）。
        subscriptions.push(listen_with(drag_release));

        // 弹层菜单的收起判据是「那个窗口没了焦点」——用户点了别处（包括远端画面），
        // 菜单就该消失。没有这条，菜单会一直挂在屏幕上。
        subscriptions.push(
            iced::window::events().map(|(window, event)| Message::WindowEvent(window, event)),
        );

        // 独立窗口被关掉时收掉它身上的会话，不能让连接一直挂着。
        subscriptions.push(window::close_events().map(Message::WindowClosed));

        // 只在真的有 RDP 进程时轮询。连接阶段 150ms 一次，会话稳定后降到 500ms，
        // 用来等 mstsc 建出会话窗口、发现进程退出、以及把画面重新贴回内容区。
        if !self.rdp.is_empty() {
            let interval = if self.rdp.values().any(|process| !process.is_live()) {
                Duration::from_millis(150)
            } else {
                Duration::from_millis(500)
            };
            subscriptions.push(iced::time::every(interval).map(|_| Message::RdpTick));
        }

        Subscription::batch(subscriptions)
    }

    /// 窗口标题。独立窗口直接用会话名，主窗口跟着活动标签走。
    pub fn title(&self, window: window::Id) -> String {
        if let Some(WindowSlot::Detached(tab)) = self.windows.get(&window) {
            return format!("{} - SuperRemote", tab.title());
        }

        if self.screen == Screen::NewTab {
            let name = self
                .new_tab_session()
                .map(|session| session.name.as_str())
                .unwrap_or(i18n::t().new_tab_title);

            return format!("{name} - SuperRemote");
        }

        match self.tabs.active_tab() {
            Some(tab) if !self.show_home => format!("{} - SuperRemote", tab.title()),
            _ => i18n::lang().window_title(None),
        }
    }

    pub fn theme(&self, _window: window::Id) -> Theme {
        theme::app_theme()
    }

    pub(crate) fn rdp_defaults(&self) -> RdpSettings {
        self.store
            .as_ref()
            .map(Store::rdp_defaults)
            .unwrap_or_default()
    }

    pub(crate) fn has_private_key(&self, id: &str) -> bool {
        self.store
            .as_ref()
            .is_some_and(|store| store.has_private_key(id))
    }

    pub(crate) fn terminal_defaults(&self) -> TerminalSettings {
        self.store
            .as_ref()
            .map(Store::terminal_defaults)
            .unwrap_or_default()
    }

    /// 全局默认的打开方式。`None` 表示用户还没设过，界面上要显示内置值。
    pub(crate) fn default_open_mode(&self) -> Option<OpenMode> {
        self.store.as_ref().and_then(Store::open_mode)
    }

    /// 一条连接实际会按哪种方式打开：单条覆盖 → 全局默认 → 内置（选项卡）。
    pub(crate) fn effective_open_mode(&self, id: &str) -> OpenMode {
        self.tree()
            .and_then(|tree| tree.find_session(id))
            .and_then(|session| session.settings.open_mode)
            .or_else(|| self.default_open_mode())
            .unwrap_or_default()
    }

    pub(crate) fn key_mode(&self) -> KeyMode {
        self.store
            .as_ref()
            .map(Store::key_mode)
            .unwrap_or(KeyMode::Os)
    }

    /// 冒烟测试用的自动化入口：设了 `SUPERREMOTE_SMOKE_OPEN=1` 就自动打开第一条连接。
    ///
    /// 只在 debug 构建里编译，发布产物不含这段逻辑。
    #[cfg(debug_assertions)]
    fn auto_open_for_smoke_test(mut self) -> (Self, Task<Message>) {
        if std::env::var("SUPERREMOTE_SMOKE_OPEN").as_deref() != Ok("1") {
            return (self, Task::none());
        }

        let first = self.rows.iter().find_map(|row| match row {
            TreeRow::Session { id, .. } => Some(id.clone()),
            _ => None,
        });
        let opened = match first {
            Some(id) => {
                self.load_draft_into_editor(&id);
                self.go_sessions();
                self.open_selected()
            }
            None => Task::none(),
        };
        (self, opened)
    }

    /// 当前正在看的终端。主页、设置页与 RDP 标签都没有终端。
    pub fn active_terminal(&self) -> Option<&Terminal> {
        let tab = self.active_tab()?;
        if tab.session.kind == SessionKind::Rdp {
            return None;
        }
        self.terminals.get(&tab.instance)
    }

    pub fn active_terminal_instance(&self) -> Option<TabId> {
        let tab = self.active_tab()?;
        if tab.session.kind == SessionKind::Rdp {
            None
        } else {
            Some(tab.instance)
        }
    }

    /// 凭据条数。侧栏页脚拿掉之后只剩用例在用（`cfg(test)`），
    /// 留在这里是因为它断言的是「删除连接时凭据要一起清理」这条数据不变量。
    #[cfg(test)]
    pub(crate) fn secret_count(&self) -> usize {
        self.store.as_ref().map(Store::secret_count).unwrap_or(0)
    }

    fn adopt_store(&mut self, opened: Opened) {
        let Opened { store, notices } = opened;
        // 解锁之后才算真正拿到配置：以文件里的偏好为准再应用一次，
        // 覆盖 `with_paths` 里那次只读窥探的结果。
        apply_ui_prefs(store.ui());
        self.store = Some(store);
        self.screen = Screen::Remote;
        self.unlock = UnlockForm::default();

        if let Some(notice) = notices.first() {
            self.status = Some(Status::error(match notice {
                superremote_store::Notice::ConfigRestored { backup, reason }
                | superremote_store::Notice::SecretsRestored { backup, reason } => {
                    i18n::lang().config_restored(reason, &backup.display().to_string())
                }
            }));
        }

        self.sync_rows();
    }

    /// 切回「会话」页：既显示主页标签，也把 `screen` 从设置页摘出来。
    ///
    /// 两个状态位各管一半：`screen` 决定整页是会话还是设置，`show_home` 决定
    /// 会话页里显示主页标签还是某个连接的标签。只改一个就会出现「点了没反应」。
    /// 凡是「回到会话」的意图都必须走这里，不要再单独写 `show_home = true`。
    /// 记一次左键点击，并回答「这一次是否构成双击」。
    ///
    /// 只有同一节点、间隔在 [`DOUBLE_CLICK`] 内才算命中；命中后立刻清空记录，
    /// 这样三连击不会在第三下再开一次标签。目录不算命中——它没有「打开」这个
    /// 动作，双击应当只当作两次选中。
    fn register_click(&mut self, id: &str) -> bool {
        let now = Instant::now();
        let doubled = self.last_click.as_ref().is_some_and(|(last, at)| {
            last.as_str() == id && now.duration_since(*at) <= DOUBLE_CLICK
        });
        self.last_click = if doubled {
            None
        } else {
            Some((id.to_owned(), now))
        };
        doubled
            && self
                .tree()
                .is_some_and(|tree| tree.find_session(id).is_some())
    }

    fn go_sessions(&mut self) {
        self.screen = Screen::Remote;
        self.show_home = true;
        self.selection = None;
        // 回详情页/设置页时把 RDP 原生画面藏掉。它是不受 Iced 布局裁剪的
        // 子窗口，只改视图状态而不 hide，会继续盖在详情页上。
        self.sync_rdp_layout();
    }

    /// 按「首选打开方式」打开选中的连接。
    ///
    /// 右键菜单里的「在后台打开 / 在新窗口打开」是**显式**指定，不走这里；
    /// 这里服务的是双击、回车与详情页的「打开连接」——它们只说「打开」，
    /// 落在哪儿由连接自己的偏好决定。
    fn open_preferred(&mut self, id: &str) -> Task<Message> {
        match self.effective_open_mode(id) {
            OpenMode::Tab => self.open_session(id, true),
            OpenMode::Background => self.open_session(id, false),
            OpenMode::Detached => self.open_in_new_window(id),
        }
    }

    fn open_selected(&mut self) -> Task<Message> {
        let Some(id) = self.selected.clone() else {
            return Task::none();
        };

        self.open_preferred(&id)
    }

    /// 把一条连接开到独立窗口里：一个窗口只装这一个会话，没有侧栏与标签条。
    ///
    /// RDP 在开头分流给 [`Self::launch_rdp_detached`]：独立窗口里同样是**内嵌** mstsc
    /// 画面（对齐旧版 `RemoteWinControl`），不是放养 mstsc 自己的原生窗口。窗口尺寸取
    /// 该连接上次记录的 `Session::window`，没有记录时用默认值。
    fn open_in_new_window(&mut self, id: &str) -> Task<Message> {
        let Some(session) = self.tree().and_then(|tree| tree.find_session(id)).cloned() else {
            self.status = Some(Status::error(i18n::t().err_node_gone));
            return Task::none();
        };

        if self.new_tab.as_deref() == Some(id) {
            self.new_tab = None;
            self.go_sessions();
        }

        if session.kind == SessionKind::Rdp {
            return self.launch_rdp_detached(&session);
        }

        let defaults = self
            .store
            .as_ref()
            .map(Store::terminal_defaults)
            .unwrap_or_default();
        let settings = session.settings.terminal.resolve(&defaults);
        let target = self.target_of(&session);

        let opened = match session.kind {
            SessionKind::Ssh => self.hub.open_ssh(&target, INITIAL_COLS, INITIAL_ROWS),
            SessionKind::Telnet => self.hub.open_telnet(&target),
            // 上面已经分流给 `launch_rdp_detached`。
            SessionKind::Rdp => return Task::none(),
        };

        let instance = self.tabs.allocate();
        let terminal = self.terminal_slot(instance, INITIAL_COLS, INITIAL_ROWS, settings);
        match opened {
            Ok(conn) => {
                terminal.conn = Some(conn);
                terminal.status = ConnStatus::Connecting;
            }
            Err(err) => terminal.status = ConnStatus::Failed(net_error_text(&err)),
        }

        let (window, opened) = window::open(detached_window_settings(session.window));
        self.window_sizes
            .insert(window, remembered_size(detached_size(session.window)));
        self.windows
            .insert(window, WindowSlot::Detached(Tab { session, instance }));

        Self::window_ready_task(opened)
    }

    /// 把树上的会话快照 + 保存的凭据拼成连接层要的 [`Target`]。
    fn target_of(&self, session: &Session) -> Target {
        Target {
            host: session.host.clone(),
            port: session.port,
            user: session.username.clone(),
            password: self
                .store
                .as_ref()
                .and_then(|store| store.password(&session.id).ok().flatten()),
            private_key: self
                .store
                .as_ref()
                .and_then(|store| store.private_key(&session.id).ok().flatten()),
        }
    }

    /// 打开一条连接。`switch` 为假时在后台开：新标签入栈但**不抢焦点**，
    /// 用户继续看当前这一页，需要时用标签条或 `Ctrl+Tab` 切过去。
    fn open_session(&mut self, id: &str, switch: bool) -> Task<Message> {
        let Some(session) = self.tree().and_then(|tree| tree.find_session(id)).cloned() else {
            self.status = Some(Status::info(i18n::t().err_directory_not_openable));
            return Task::none();
        };

        if self.new_tab.as_deref() == Some(id) {
            self.new_tab = None;
            if !switch && self.screen == Screen::NewTab {
                self.go_sessions();
            }
        }

        let instance = if switch {
            let instance = self.tabs.open(session.clone());
            // 从「连接列表」页打开时也要把 screen 切回 Remote，
            // 否则标签建出来了、主区域还停在列表页。
            self.screen = Screen::Remote;
            self.show_home = false;
            instance
        } else {
            self.tabs.open_in_background(session.clone())
        };

        self.status = None;
        // 新开的可能是 SSH/Telnet；上一路 RDP 的原生窗口必须跟着让位。
        self.sync_rdp_layout();

        // 同一个标签重复点「打开」只切换焦点，不要重连。
        if self
            .terminals
            .get(&instance)
            .is_some_and(|term| !term.status.is_settled())
        {
            return Task::none();
        }

        self.connect(instance, &session)
    }

    /// 建立（或重建）某个标签的连接。
    fn connect(&mut self, instance: TabId, session: &Session) -> Task<Message> {
        // 复用已有网格的尺寸，避免重连后把远端的画面折回默认 80×24。
        let (cols, rows) = self
            .terminals
            .get(&instance)
            .map(|term| (term.cols, term.rows))
            .unwrap_or((INITIAL_COLS, INITIAL_ROWS));

        if session.kind == SessionKind::Rdp {
            // RDP 没有终端网格：状态要等 mstsc 的会话窗口真的出现才转为 Live。
            self.terminal_slot(instance, cols, rows, ResolvedTerminalSettings::default())
                .status = ConnStatus::Connecting;
            return self.launch_rdp(instance);
        }

        let terminal_defaults = self
            .store
            .as_ref()
            .map(Store::terminal_defaults)
            .unwrap_or_default();
        let terminal_settings = session.settings.terminal.resolve(&terminal_defaults);

        let target = self.target_of(session);

        // 先拿连接再写回网格：`open_*` 需要 `&mut self.hub`，不能和 `&mut self.terminals`
        // 同时持有。这样写还有个好处——不用为了取回终端再查一次表，
        // 也就没有「刚刚插入过所以肯定存在」的 `expect`。
        let opened = match session.kind {
            SessionKind::Ssh => self.hub.open_ssh(&target, cols, rows),
            SessionKind::Telnet => self.hub.open_telnet(&target),
            // 上面已经提前返回，这里只是为了让 match 穷尽；不 panic。
            SessionKind::Rdp => return Task::none(),
        };

        let terminal = self.terminal_slot(instance, cols, rows, terminal_settings);
        match opened {
            Ok(conn) => {
                terminal.conn = Some(conn);
                terminal.status = ConnStatus::Connecting;
            }
            Err(err) => terminal.status = ConnStatus::Failed(net_error_text(&err)),
        }

        Task::none()
    }

    /// 取（必要时创建）某个标签的终端网格，并把尺寸对齐到 `cols × rows`。
    ///
    /// `HashMap::entry` 不会失败，所以整个程序不需要为了「我相信它存在」再写一个
    /// `expect`——生产路径里一个 panic 都不能有。
    fn terminal_slot(
        &mut self,
        instance: TabId,
        cols: u16,
        rows: u16,
        settings: ResolvedTerminalSettings,
    ) -> &mut Terminal {
        let terminal = self
            .terminals
            .entry(instance)
            .or_insert_with(|| Terminal::new(cols, rows, settings));
        terminal.resize(cols, rows);
        terminal
    }

    /// 按运行实例找它对应的会话。标签被关掉之后会返回 `None`。
    /// 取某个运行实例的会话快照。标签与独立窗口都要找——
    /// 独立窗口的实例不在标签条里，漏掉它会让 `launch_rdp` 静默不启动。
    fn session_of(&self, instance: TabId) -> Option<Session> {
        if let Some(session) = self
            .tabs
            .tabs()
            .iter()
            .find(|tab| tab.instance == instance)
            .map(|tab| tab.session.clone())
        {
            return Some(session);
        }
        self.windows.values().find_map(|slot| match slot {
            WindowSlot::Detached(tab) if tab.instance == instance => Some(tab.session.clone()),
            _ => None,
        })
    }

    fn reconnect(&mut self, instance: TabId) -> Task<Message> {
        // 标签里存的是打开瞬间的快照；重连要取树上最新的地址与设置，
        // 否则在详情页改完 RDP 选项后点「重新连接」仍然用旧配置。
        let Some(session_id) = self
            .tabs
            .tabs()
            .iter()
            .find(|tab| tab.instance == instance)
            .map(|tab| tab.id().clone())
        else {
            return Task::none();
        };

        let Some(session) = self
            .tree()
            .and_then(|tree| tree.find_session(&session_id))
            .cloned()
        else {
            return Task::none();
        };

        self.tabs.replace_session(instance, session.clone());
        self.forget_terminal(instance);
        self.connect(instance, &session)
    }

    /// 断开并丢弃某个标签的运行时。
    fn forget_terminal(&mut self, instance: TabId) {
        if let Some(terminal) = self.terminals.remove(&instance) {
            if let Some(conn) = terminal.conn {
                self.hub.close(conn);
                self.hub.forget(conn);
            }
        }

        if let Some(mut process) = self.rdp.remove(&instance) {
            process.close();
        }

        if self.rdp_fullscreen == Some(instance) {
            self.rdp_fullscreen = None;
        }
    }

    /// 把连接层攒下的事件喂进各自的终端。
    ///
    /// 这里对每个事件在 `terminals` 里线性找一次归属，复杂度是 O(事件数 × 标签数)。
    /// 两边的量级都很小（16ms 一轮、事件通常个位数、标签数是个位数），所以刻意
    /// 不维护 `ConnId → TabId` 的反查表：那张表要在关标签、重连、RDP 启动失败
    /// 等多条路径上同步，漏一处就是「事件喂错终端」的幽灵 bug，收益远小于代价。
    fn pump_network(&mut self) {
        let events = self.hub.drain();
        if events.is_empty() {
            return;
        }

        for (conn, event) in events {
            let instance = self
                .terminals
                .iter()
                .find(|(_, terminal)| terminal.conn == Some(conn))
                .map(|(instance, _)| *instance);

            let Some(instance) = instance else { continue };
            // 可见性要在拿 `terminals` 的可变借用之前判定：它要读标签条与窗口表。
            let visible = self.is_instance_visible(instance);
            let Some(terminal) = self.terminals.get_mut(&instance) else {
                continue;
            };

            match event {
                NetEvent::Data(bytes) => {
                    // 后台标签只喂网格不物化 Frame：整屏物化只服务渲染，
                    // 四个标签跑 `tail -f` 时就是每 16ms 四次全屏分配。
                    if visible {
                        terminal.feed(&bytes);
                    } else {
                        terminal.feed_background(&bytes);
                    }
                    // 有新输出时把光标立刻点亮，闪烁相位从这一刻重新计时。
                    self.blink_on = true;
                }
                NetEvent::Ready => terminal.mark_ready(),
                NetEvent::Error(err) => {
                    terminal.status = ConnStatus::Failed(net_error_text(&err));
                }
                // 已经失败过的会话不要被随后的 `Exited` 覆盖成「正常断开」：
                // 连接层常在报错之后紧跟一个退出事件，覆盖掉就等于把失败原因藏起来。
                NetEvent::Exited(code) => {
                    if !matches!(terminal.status, ConnStatus::Failed(_)) {
                        terminal.status = ConnStatus::Closed(code);
                    }
                }
            }
        }

        // 队列顶到过上限：中间一段输出被丢了，给一条提示解释画面里的缺口。
        if self.hub.take_overflowed() {
            self.status = Some(Status::error(i18n::lang().net_overflow()));
        }
    }

    /// 键盘事件的唯一入口：界面快捷键 → 树/列表导航 → 终端，按这个顺序认领。
    ///
    /// 顺序不能反。`encode_key` 对 `Named` 键**不看** `ctrl`（`Tab` 一律编成 `\t`），
    /// 先发给终端的话，按 `Ctrl+Tab` 会在 shell 里凭空多出一个制表符。
    /// 导航键只在画面里没有终端的页面认领：主页（`show_home`）与连接列表页；
    /// 终端页里的裸方向键属于远端 shell。
    fn handle_key(&mut self, event: keyboard::Event) -> Task<Message> {
        if let Some(shortcut) = shortcut::resolve(&event) {
            return self.apply_shortcut(shortcut);
        }

        let on_navigator =
            (self.screen == Screen::Remote && self.show_home) || self.screen == Screen::List;
        if on_navigator {
            if let Some(nav) = shortcut::resolve_nav(&event) {
                return self.apply_nav(nav);
            }
        }

        self.send_key(event);
        Task::none()
    }

    /// 快捷键只翻译成既有的 `Message`，行为与点界面完全一致（含关标签后的收尾）。
    fn apply_shortcut(&mut self, shortcut: Shortcut) -> Task<Message> {
        match shortcut {
            Shortcut::NextTab => self.cycle_tab(1),
            Shortcut::PrevTab => self.cycle_tab(-1),
            Shortcut::ActivateTab(position) => {
                let len = self.tabs.len();
                if len == 0 {
                    return Task::none();
                }

                let index = if position == shortcut::LAST_TAB_KEY {
                    len - 1
                } else {
                    position - 1
                };

                if index >= len {
                    return Task::none();
                }
                self.update(Message::ActivateTab(index))
            }
            Shortcut::CloseTab => match self.tabs.active_index() {
                Some(index) => self.update(Message::CloseTab(index)),
                None => Task::none(),
            },
        }
    }

    fn send_key(&mut self, event: keyboard::Event) {
        let Some(instance) = self.active_terminal_instance() else {
            return;
        };

        // Ctrl+Shift+C / V 是终端自己的复制/粘贴，不能把 ^C 发给远端。
        if let keyboard::Event::KeyPressed {
            key: keyboard::Key::Character(value),
            modifiers,
            ..
        } = &event
        {
            if modifiers.control() && modifiers.shift() {
                match value.to_lowercase().as_str() {
                    "c" => {
                        self.copy_selection();
                        return;
                    }
                    "v" => {
                        self.paste_clipboard();
                        return;
                    }
                    _ => {}
                }
            }
        }

        let Some(terminal) = self.terminals.get(&instance) else {
            return;
        };
        if !terminal.accepts_input() {
            return;
        }
        let settings = terminal.settings.clone();
        let Some(conn) = terminal.conn else { return };
        let Some(bytes) = terminal::encode_key(&event, &settings) else {
            return;
        };

        self.hub.send(conn, bytes);
        self.blink_on = true;
    }

    fn copy_selection(&mut self) {
        let Some(instance) = self.active_terminal_instance() else {
            return;
        };
        let Some(selection) = self.selection else {
            return;
        };
        if selection.is_empty() {
            return;
        }
        let Some(terminal) = self.terminals.get(&instance) else {
            return;
        };

        let text = terminal.selection_text(selection);
        if text.is_empty() {
            return;
        }

        self.status = Some(match clipboard::write_text(&text) {
            Ok(()) => Status::info(i18n::t().info_copied),
            Err(err) => Status::error(err),
        });
    }

    fn paste_clipboard(&mut self) {
        let Some(instance) = self.active_terminal_instance() else {
            return;
        };
        let Some(terminal) = self.terminals.get(&instance) else {
            return;
        };
        if !terminal.accepts_input() {
            return;
        }
        let Some(conn) = terminal.conn else {
            return;
        };
        let Some(text) = clipboard::read_text() else {
            return;
        };
        if text.is_empty() {
            return;
        }

        // 粘贴进 shell 用 CR 表示换行；Windows 剪贴板常见的是 CRLF。
        let text = text.replace("\r\n", "\r").replace('\n', "\r");
        self.hub.send(conn, text.into_bytes());
        self.blink_on = true;
    }

    fn resize_terminal(&mut self, instance: TabId, cols: u16, rows: u16) {
        let Some(terminal) = self.terminals.get_mut(&instance) else {
            return;
        };
        if terminal.cols == cols && terminal.rows == rows {
            return;
        }

        terminal.resize(cols, rows);
        if let Some(conn) = terminal.conn {
            self.hub.resize(conn, cols, rows);
        }
    }

    fn set_master_password(&mut self) {
        let password = std::mem::take(&mut self.master_password_draft);

        if password.chars().count() < MIN_MASTER_PASSWORD {
            self.master_password_draft = password;
            self.status = Some(Status::error(
                i18n::lang().min_password(MIN_MASTER_PASSWORD),
            ));
            return;
        }

        let was_protected = self.key_mode() == KeyMode::Password;
        let result = self
            .store
            .as_mut()
            .map(|store| store.set_master_password(&password));

        match result {
            Some(Ok(())) => {
                self.status = Some(Status::info(if was_protected {
                    i18n::t().info_password_changed
                } else {
                    i18n::t().info_password_enabled
                }));
                self.save_now();
            }
            Some(Err(err)) => self.status = Some(Status::error(err.to_string())),
            None => {}
        }
    }

    fn edit_rdp_settings(&mut self, edit: RdpEdit) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        let Some(store) = self.store.as_mut() else {
            return;
        };

        let changed = store
            .tree_mut()
            .with_session_mut(&id, |session| edit.apply(&mut session.settings.rdp));

        if !changed {
            self.status = Some(Status::error(i18n::t().err_selection_gone));
        }
    }

    fn reset_rdp_settings(&mut self) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        let Some(store) = self.store.as_mut() else {
            return;
        };

        let changed = store.tree_mut().with_session_mut(&id, |session| {
            session.settings.rdp = RdpSettings::default();
        });

        if changed {
            self.status = Some(Status::info(i18n::t().info_rdp_settings_reset));
        } else {
            self.status = Some(Status::error(i18n::t().err_selection_gone));
        }
    }

    fn make_rdp_defaults(&mut self) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        let Some(session) = self.tree().and_then(|tree| tree.find_session(&id)).cloned() else {
            self.status = Some(Status::error(i18n::t().err_selection_gone));
            return;
        };
        let defaults = self
            .store
            .as_ref()
            .map(Store::rdp_defaults)
            .unwrap_or_default();
        let resolved = session.settings.rdp.resolve(&defaults);

        if let Some(store) = self.store.as_mut() {
            store.set_rdp_defaults(&resolved.into_settings());
            self.status = Some(Status::info(i18n::t().info_rdp_defaults_saved));
        }
    }

    fn edit_terminal_settings(&mut self, edit: TerminalEdit) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        let Some(store) = self.store.as_mut() else {
            return;
        };

        let changed = store
            .tree_mut()
            .with_session_mut(&id, |session| edit.apply(&mut session.settings.terminal));

        if !changed {
            self.status = Some(Status::error(i18n::t().err_selection_gone));
        }
    }

    fn reset_terminal_settings(&mut self) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        let Some(store) = self.store.as_mut() else {
            return;
        };

        let changed = store.tree_mut().with_session_mut(&id, |session| {
            session.settings.terminal = TerminalSettings::default();
        });

        if changed {
            self.status = Some(Status::info(i18n::t().info_terminal_settings_reset));
        } else {
            self.status = Some(Status::error(i18n::t().err_selection_gone));
        }
    }

    fn make_terminal_defaults(&mut self) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        let Some(session) = self.tree().and_then(|tree| tree.find_session(&id)).cloned() else {
            self.status = Some(Status::error(i18n::t().err_selection_gone));
            return;
        };
        let defaults = self
            .store
            .as_ref()
            .map(Store::terminal_defaults)
            .unwrap_or_default();
        let resolved = session.settings.terminal.resolve(&defaults);

        if let Some(store) = self.store.as_mut() {
            store.set_terminal_defaults(&resolved.into_settings());
            self.status = Some(Status::info(i18n::t().info_terminal_defaults_saved));
        }
    }

    /// 改全局默认的一项。读出来改完再整份写回：`RdpSettings` 是 `Option` 的集合，
    /// 逐字段写要为每个字段开一个入口，而这里只有一条链路需要它。
    fn edit_rdp_defaults(&mut self, edit: RdpEdit) {
        let Some(store) = self.store.as_mut() else {
            return;
        };

        let mut settings = store.rdp_defaults();
        edit.apply(&mut settings);
        store.set_rdp_defaults(&settings);
    }

    /// 清空全部 RDP 全局默认。清空之后每项都走程序内置值，
    /// 与「单条连接重置为继承」是两层不同的东西，别混用。
    fn reset_rdp_defaults(&mut self) {
        let Some(store) = self.store.as_mut() else {
            return;
        };

        store.set_rdp_defaults(&RdpSettings::default());
        self.status = Some(Status::info(i18n::t().info_rdp_defaults_reset));
    }

    /// 请求提权恢复系统 RDP 优化设置。提权实例只写两项就退出，主进程约 2 秒后
    /// 回读一次刷新提示；用户取消 UAC 时保持原状，不报假成功。
    fn fix_rdp_tuning(&mut self) -> Task<Message> {
        match superremote_win::sys_tuning::request_fix() {
            Ok(()) => {
                self.status = Some(Status::info(i18n::t().rdp_tuning_pending.to_owned()));
                self.reprobe_tuning(TuningIntent::Defaults)
            }
            Err(err) => {
                self.status = Some(Status::error(err));
                Task::none()
            }
        }
    }

    /// 请求提权开关 UAC 安全桌面。这是 `PromptOnSecureDesktop`，属于系统安全
    /// 策略，默认不动；只有用户在设置页显式点了才写。
    fn set_secure_desktop(&mut self, enabled: bool) -> Task<Message> {
        match superremote_win::sys_tuning::request_secure_desktop(enabled) {
            Ok(()) => {
                self.status = Some(Status::info(i18n::t().uac_desktop_pending.to_owned()));
                self.reprobe_tuning(TuningIntent::SecureDesktop(enabled))
            }
            Err(err) => {
                self.status = Some(Status::error(err));
                Task::none()
            }
        }
    }

    /// 提权实例要先过 UAC 再写注册表，这里等它落盘后回读一次。
    /// 阻塞的是 iced 执行器的一个 worker，2 秒不影响渲染线程。
    fn reprobe_tuning(&self, intent: TuningIntent) -> Task<Message> {
        Task::perform(
            async move {
                std::thread::sleep(Duration::from_millis(2000));
                superremote_win::sys_tuning::probe()
            },
            move |state| Message::RdpTuningProbed(state, intent),
        )
    }

    fn edit_terminal_defaults(&mut self, edit: TerminalEdit) {
        let Some(store) = self.store.as_mut() else {
            return;
        };

        let mut settings = store.terminal_defaults();
        edit.apply(&mut settings);
        store.set_terminal_defaults(&settings);
    }

    fn reset_terminal_defaults(&mut self) {
        let Some(store) = self.store.as_mut() else {
            return;
        };

        store.set_terminal_defaults(&TerminalSettings::default());
        self.status = Some(Status::info(i18n::t().info_terminal_defaults_reset));
    }

    fn edit_open_mode(&mut self, mode: Option<OpenMode>) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        let Some(store) = self.store.as_mut() else {
            return;
        };

        let changed = store.tree_mut().with_session_mut(&id, |session| {
            session.settings.open_mode = mode;
        });

        if !changed {
            self.status = Some(Status::error(i18n::t().err_selection_gone));
        }
    }

    fn edit_default_open_mode(&mut self, mode: Option<OpenMode>) {
        if let Some(store) = self.store.as_mut() {
            store.set_open_mode(mode);
        }
    }

    fn clear_master_password(&mut self) {
        let result = self.store.as_mut().map(Store::clear_master_password);
        match result {
            Some(Ok(())) => {
                self.status = Some(Status::info(i18n::t().info_password_disabled));
                self.save_now();
            }
            Some(Err(err)) => self.status = Some(Status::error(err.to_string())),
            None => {}
        }
    }

    fn submit_unlock(&mut self) {
        let password = std::mem::take(&mut self.unlock.password);

        match Store::open(self.paths.clone(), Some(&password)) {
            Ok(opened) => self.adopt_store(opened),
            Err(OpenError::WrongPassword) => {
                self.unlock = UnlockForm {
                    password,
                    error: Some(i18n::t().unlock_wrong.to_owned()),
                };
            }
            Err(other) => self.screen = Screen::Fatal(other.to_string()),
        }
    }

    /// 切换外观。立即生效，并写进配置（由脏标记触发的自动保存落盘）。
    fn set_theme(&mut self, mode: Mode) {
        theme::set_mode(mode);

        if let Some(store) = self.store.as_mut() {
            store.ui_mut().theme = Some(mode.code().to_owned());
        }
    }

    /// 切换界面语言。文案在 `view` 里现取，所以下一帧就换过来了。
    fn set_lang(&mut self, lang: Lang) {
        i18n::set_lang(lang);

        if let Some(store) = self.store.as_mut() {
            store.ui_mut().lang = Some(lang.code().to_owned());
        }
    }

    fn choose_data_dir(&mut self) {
        let title = i18n::t().choose_data_dir;
        let owner = self
            .main_window
            .and_then(|window| self.hwnds.get(&window).copied());

        if let Some(dir) = superremote_win::dir_picker::pick_folder(owner, title) {
            self.switch_data_dir_impl(dir, true);
        }
    }

    fn reset_data_dir(&mut self) {
        let Some(default) = DataPaths::portable_dir() else {
            self.status = Some(Status::error(i18n::t().data_dir_env_locked));
            return;
        };

        self.switch_data_dir_impl(default.dir().clone(), true);
    }

    /// 切换数据目录：先把当前数据复制过去，再把打开中的 Store 指过去。
    ///
    /// `persist_pointer` 只给单测关掉写指针用——测试进程不能把
    /// `data-location.txt` 写到真实程序目录/AppData。
    fn switch_data_dir_impl(&mut self, target: PathBuf, persist_pointer: bool) {
        if DataPaths::env_override().is_some() {
            self.status = Some(Status::error(i18n::t().data_dir_env_locked));
            return;
        }

        let target = DataPaths::from_dir(target);
        if target == self.paths {
            return;
        }

        // 复制前先落盘，避免把 1.2 秒内还没保存的编辑内容留在旧目录。
        if let Some(store) = self.store.as_mut() {
            if let Err(err) = store.save() {
                self.status = Some(Status::error(
                    i18n::lang().data_dir_change_failed(&err.to_string()),
                ));
                return;
            }
        }

        let is_default =
            DataPaths::portable_dir().is_some_and(|default| default.dir() == target.dir());

        if let Err(err) = self.paths.copy_data_to(&target, is_default) {
            self.status = Some(Status::error(
                i18n::lang().data_dir_change_failed(&err.to_string()),
            ));
            return;
        }

        let previous_pointer = DataPaths::read_location();
        if persist_pointer {
            let saved = if is_default {
                DataPaths::clear_location()
            } else {
                DataPaths::save_location(target.dir())
            };

            if let Err(err) = saved {
                self.status = Some(Status::error(
                    i18n::lang().data_dir_change_failed(&err.to_string()),
                ));
                return;
            }
        }

        if let Some(store) = self.store.as_mut() {
            if let Err(err) = store.relocate(target.clone()) {
                // 存储没搬成功，把指针退回原来的位置，避免下次启动指向一份
                // 还没被当前 Store 接管的数据目录。
                if persist_pointer {
                    match previous_pointer {
                        Some(path) => {
                            let _ = DataPaths::save_location(&path);
                        }
                        None => {
                            let _ = DataPaths::clear_location();
                        }
                    }
                }

                self.status = Some(Status::error(
                    i18n::lang().data_dir_change_failed(&err.to_string()),
                ));
                return;
            }
        }

        self.paths = target;
        self.status = Some(Status::info(i18n::t().data_dir_changed));
    }

    fn reveal_data_dir(&mut self) {
        let dir = self.paths.dir().clone();
        let spawned = std::process::Command::new("explorer")
            .arg(dir.as_os_str())
            .spawn();

        if let Err(err) = spawned {
            self.status = Some(Status::error(
                i18n::lang().open_dir_failed(&err.to_string()),
            ));
        }
    }

    fn save_now(&mut self) {
        let failure = match self.store.as_mut() {
            Some(store) => store.save().err(),
            None => None,
        };

        if let Some(err) = failure {
            // 外部修改冲突有自己的说法：不套「保存失败：io 原因」的模板，
            // 用户要的是「没保存 + 该怎么办」，不是技术细节。
            let message = if matches!(err, StoreError::ExternalModified(_)) {
                i18n::lang().data_file_changed().to_owned()
            } else {
                i18n::lang().save_failed(&err.to_string())
            };
            self.status = Some(Status::error(message));
        }
    }
}

/// 连接层错误 → 状态条文案：原文 + 按 `ErrorKind` 的可操作建议（建议在 i18n，
/// 原文在 net 层生成）。「os error 10061」对用户没有任何指向性，
/// 「目标拒绝连接：端口没开或服务未启动」才有。
fn net_error_text(err: &superremote_net::NetError) -> String {
    let hint = err
        .error_kind()
        .and_then(|kind| i18n::lang().net_error_hint(kind));
    match hint {
        Some(hint) => format!("{err}（{hint}）"),
        None => err.to_string(),
    }
}

/// 深拷贝一棵节点子树，所有节点换新 id；连接的口令与私钥一并复制。
fn copy_node(store: &mut Store, node: &SessionNode) -> SessionNode {
    match node {
        SessionNode::Directory(directory) => SessionNode::Directory(Directory {
            id: store.next_id(),
            name: directory.name.clone(),
            children: directory
                .children
                .iter()
                .map(|child| copy_node(store, child))
                .collect(),
        }),
        SessionNode::Session(session) => {
            let id = store.next_id();
            let password = store.password(&session.id).ok().flatten();
            let private_key = store.private_key(&session.id).ok().flatten();
            let _ = store.set_password(&id, password.as_deref());
            let _ = store.set_private_key(&id, private_key.as_deref());

            SessionNode::Session(Session {
                id,
                ..session.clone()
            })
        }
    }
}

/// 会话树右键菜单的一条动作。
///
/// id 随动作一起走：菜单是**按行**构造的，动作自己指名道姓，就不必再依赖
/// 「当前选中项」是哪一个——右键一条未选中的行也能直接操作它。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeAction {
    Open(SessionId),
    OpenInBackground(SessionId),
    OpenInNewWindow(SessionId),
    Rename(SessionId),
    CopyAddress(SessionId),
    Delete(SessionId),
    Cut(SessionId),
    Copy(SessionId),
    Paste(SessionId),
    /// 在某一行上「新建」：目录行表示建在它里面，连接行表示建在它旁边（根层）。
    NewDirectory(SessionId),
    NewSession(SessionId, SessionKind),
}

impl TreeAction {
    fn id(&self) -> &SessionId {
        match self {
            Self::Open(id)
            | Self::OpenInBackground(id)
            | Self::OpenInNewWindow(id)
            | Self::Rename(id)
            | Self::CopyAddress(id)
            | Self::Delete(id)
            | Self::Cut(id)
            | Self::Copy(id)
            | Self::Paste(id)
            | Self::NewDirectory(id)
            | Self::NewSession(id, _) => id,
        }
    }
}

/// 菜单项的文字。`overlay::menu` 只吃 `ToString` 的选项，标题就从这里出。
impl std::fmt::Display for TreeAction {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let t = i18n::t();
        let label = match self {
            Self::Open(_) => t.menu_open,
            Self::OpenInBackground(_) => t.menu_open_background,
            Self::OpenInNewWindow(_) => t.menu_open_new_window,
            Self::Rename(_) => t.menu_rename,
            Self::CopyAddress(_) => t.menu_copy_address,
            Self::Delete(_) => t.menu_delete,
            Self::Cut(_) => t.menu_cut,
            Self::Copy(_) => t.menu_copy,
            Self::Paste(_) => t.menu_paste,
            Self::NewDirectory(_) => t.menu_new_directory,
            Self::NewSession(_, SessionKind::Ssh) => t.menu_new_ssh,
            Self::NewSession(_, SessionKind::Rdp) => t.menu_new_rdp,
            Self::NewSession(_, SessionKind::Telnet) => t.menu_new_telnet,
        };
        formatter.write_str(label)
    }
}

/// 一条会话的动作。带的是**运行实例**而不是标签下标：下标只有主窗口的标签条
/// 认，独立窗口没有标签条，而两处要共用同一份菜单（`ui::workspace::session_actions`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabAction {
    Reconnect(TabId),
    /// RDP 会话的「全屏 / 退出全屏」。
    Fullscreen(TabId),
    /// 把这条会话的画面撑到某一档实际像素。
    FitSize(TabId, PaneFit),
    /// 结束 RDP 会话但保留标签/窗口。
    Disconnect(TabId),
    /// 把这个实例的会话再开到一个独立窗口里（新开一个运行实例）。
    Detach(TabId),
    Close(TabId),
    /// 关掉标签条里除它以外的全部标签（只对主窗口有意义）。
    CloseOthers(TabId),
}

/// 菜单里内置的画面尺寸档。单位是**实际像素**：150% 屏上窗口开不到 1920 逻辑宽，
/// 撑的只是标题栏以下那块画面（换算见 `window_size_for_pane`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneFit {
    Hd,
    FullHd,
    QuadHd,
    UltraHd,
}

impl PaneFit {
    /// 菜单里列出的档位，从小到大。
    pub const ALL: [Self; 4] = [Self::Hd, Self::FullHd, Self::QuadHd, Self::UltraHd];

    pub const fn size(self) -> (u32, u32) {
        match self {
            Self::Hd => (1280, 720),
            Self::FullHd => (1920, 1080),
            Self::QuadHd => (2560, 1440),
            Self::UltraHd => (3840, 2160),
        }
    }
}

impl std::fmt::Display for TabAction {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let t = i18n::t();
        let label = match self {
            Self::Reconnect(_) => t.reconnect.to_owned(),
            Self::Fullscreen(_) => t.rdp_fullscreen.to_owned(),
            Self::Disconnect(_) => t.rdp_disconnect.to_owned(),
            Self::Close(_) => t.menu_close_tab.to_owned(),
            Self::CloseOthers(_) => t.menu_close_others.to_owned(),
            Self::Detach(_) => t.menu_open_new_window.to_owned(),
            Self::FitSize(_, fit) => {
                let (width, height) = fit.size();
                i18n::lang().pane_size(width, height)
            }
        };
        formatter.write_str(&label)
    }
}

/// 运行时窗口图标（任务栏 / Alt-Tab 显示）。exe 资源图标只影响资源管理器，
/// 运行中窗口必须在这里显式设置，否则 Windows 会用默认图标。
fn app_window_icon() -> Option<window::Icon> {
    window::icon::from_rgba(
        superremote_win::icon_rgba::ICON_RGBA.to_vec(),
        superremote_win::icon_rgba::ICON_SIZE,
        superremote_win::icon_rgba::ICON_SIZE,
    )
    .ok()
}

/// 主窗口的窗口参数。daemon 模式下它也是 `window::open` 开出来的，
/// 所以和独立窗口共用一套参数来源。
pub(crate) fn main_window_settings() -> window::Settings {
    window::Settings {
        size: Size::new(MAIN_W, MAIN_H),
        min_size: Some(Size::new(MAIN_MIN_W, MAIN_MIN_H)),
        position: window::Position::Centered,
        // 标题栏由 `ui::chrome` 自绘，系统边框只保留窗口阴影与贴边行为。
        decorations: false,
        icon: app_window_icon(),
        ..window::Settings::default()
    }
}

/// 会话画面之外，窗口还要为装饰留多少**逻辑**像素，返回 `(横向, 纵向)`。
///
/// 纵向基准与 `rdp_host::logical_rect` 同源：主窗口只有自绘标题栏（含它内部那条
/// 1px 分隔线），独立窗口只有一条 30px 标题条；终端画面底下还压着 1px 分隔线 +
/// 状态条，RDP 画面直接铺到底。横向只有 RDP 需要：它是原生子窗口，画面区自己
/// 也要让开左右两条缩放热区，而终端画布是 Iced 画的，热区压在画面上照样收得到事件。
fn pane_chrome(layout: rdp_host::PaneLayout, kind: SessionKind) -> (f32, f32) {
    let bar = match layout {
        rdp_host::PaneLayout::Main => ui::TITLEBAR_H,
        rdp_host::PaneLayout::Detached | rdp_host::PaneLayout::Fullscreen => ui::FULLSCREEN_BAR_H,
    };

    match kind {
        SessionKind::Rdp => (2.0 * ui::RESIZE_EDGE, bar + ui::RESIZE_EDGE),
        SessionKind::Ssh | SessionKind::Telnet => (0.0, bar + ui::DIVIDER_H + ui::STATUS_BAR_H),
    }
}

/// 画面撑到 `pane` 实际像素时，窗口该开多大（逻辑像素）。
/// DPI 只除画面那一段：装饰按逻辑像素布局，本来就跟着窗口一起缩放。
fn window_size_for_pane(pane: (u32, u32), chrome: (f32, f32), scale: f32) -> Size<f32> {
    Size::new(
        pane.0 as f32 / scale + chrome.0,
        pane.1 as f32 / scale + chrome.1,
    )
}

/// 弹层窗口的公共参数：无边框、不可缩放、不进任务栏、盖在普通窗口之上。
/// 会话菜单要能接点击与 Esc（`visible: true`，正常激活），提示条只要看得见
/// （`visible: false`，句柄到手后用 `SW_SHOWNA` 显示，全程不抢焦点）。
fn popup_window_settings(size: Size, position: Point, visible: bool) -> window::Settings {
    window::Settings {
        size,
        position: window::Position::Specific(position),
        decorations: false,
        resizable: false,
        visible,
        level: window::Level::AlwaysOnTop,
        platform_specific: window::settings::PlatformSpecific {
            drag_and_drop: false,
            skip_taskbar: true,
            ..Default::default()
        },
        ..window::Settings::default()
    }
}

/// 提示条距父窗口底边的距离（逻辑像素）。
const TOAST_BOTTOM: f32 = 24.0;

/// 独立窗口：比主窗口小一圈，够放一个会话画面就行。
/// 尺寸用上一次关窗时记下来的值；没有记录时用默认。
/// 窗口标题由 `App::title(window)` 给，`window::Settings` 里没有 title 字段。
/// 逻辑像素尺寸 → 记忆用的整数尺寸，按独立窗口的最小尺寸夹一次。
fn remembered_size(size: Size<f32>) -> WindowSize {
    WindowSize {
        width: size.width.round().max(DETACHED_MIN_W) as u32,
        height: size.height.round().max(DETACHED_MIN_H) as u32,
    }
}

fn detached_size(remembered: Option<WindowSize>) -> Size {
    match remembered {
        Some(size) => Size::new(
            (size.width as f32).max(DETACHED_MIN_W),
            (size.height as f32).max(DETACHED_MIN_H),
        ),
        None => Size::new(DETACHED_W, DETACHED_H),
    }
}

fn detached_window_settings(remembered: Option<WindowSize>) -> window::Settings {
    window::Settings {
        size: detached_size(remembered),
        min_size: Some(Size::new(DETACHED_MIN_W, DETACHED_MIN_H)),
        position: window::Position::Centered,
        decorations: false,
        icon: app_window_icon(),
        ..window::Settings::default()
    }
}

enum NewKind {
    Directory,
    Session(SessionKind),
}

impl Default for App {
    /// 只取状态：开局任务（冒烟自动打开）在测试里没有运行时可交回，
    /// 而那条路径默认不启用，丢掉的是 `Task::none()`。
    fn default() -> Self {
        Self::new().0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("superremote-app-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn paths(&self) -> DataPaths {
            DataPaths::from_dir(&self.0)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// 骨架阶段 `update` 只返回 `Task::none()`，但 `Task` 是 `#[must_use]`。
    fn send(app: &mut App, message: Message) {
        let _ = app.update(message);
    }

    fn fill(app: &mut App, name: &str, host: &str, port: &str, user: &str) {
        app.draft.name = name.to_owned();
        app.draft.host = host.to_owned();
        app.draft.port = port.to_owned();
        app.draft.username = user.to_owned();
    }

    /// 数据目录切换必须**当场生效**：旧目录留备份，新目录可重新打开看到同一棵树。
    #[test]
    fn changing_the_data_dir_relocates_the_live_store() {
        let source = TempDir::new("data-source");
        let target = TempDir::new("data-target");
        let mut app = App::with_paths(source.paths());

        send(&mut app, Message::NewSession(SessionKind::Ssh));
        fill(&mut app, "Web-01", "10.20.1.11", "22", "ops");
        send(&mut app, Message::ApplyDraft);

        app.switch_data_dir_impl(target.paths().dir().clone(), false);

        assert_eq!(app.paths.dir(), target.paths().dir());
        assert!(target.paths().config().exists(), "新目录必须写入配置");
        assert!(source.paths().config().exists(), "旧目录必须保留备份");

        let reopened = App::with_paths(target.paths());
        assert_eq!(
            reopened.tree().unwrap().session_count(),
            1,
            "从新目录重新打开后应看到原会话"
        );
    }

    /// 标题栏「+」复用右键「新建 SSH」的创建路径，只是把它放在独立标签里。
    #[test]
    fn the_plus_opens_the_same_new_connection_page_in_a_tab() {
        let temp = TempDir::new("new-tab");
        let mut app = App::with_paths(temp.paths());

        send(&mut app, Message::NewTabOpen);
        assert_eq!(app.screen, Screen::NewTab);
        let new_tab = app.new_tab.clone().expect("应打开新建连接标签");
        assert_eq!(app.selected.as_deref(), Some(new_tab.as_str()));

        let session = app
            .tree()
            .unwrap()
            .find_session(&new_tab)
            .expect("与右键新建一样，先落一个空节点");
        assert_eq!(session.kind, SessionKind::Ssh);
        assert_eq!(session.host, "");
        assert_eq!(session.port, 22);

        // 详情编辑器就是右键新建那一页：填完保存，再点「打开连接」。
        fill(&mut app, "Web-01", "10.20.1.11", "22", "ops");
        send(&mut app, Message::ApplyDraft);
        send(&mut app, Message::OpenSelected);

        assert!(app.new_tab.is_none(), "打开后临时标签应被真实会话标签取代");
        assert_eq!(app.screen, Screen::Remote);
        assert!(!app.show_home);
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.tree().unwrap().session_count(), 1);
    }

    /// 关闭仍未保存的空草稿要撤掉节点；已经保存过的连接要留下。
    #[test]
    fn closing_a_new_tab_discards_only_a_blank_draft() {
        let temp = TempDir::new("new-tab-close");
        let mut app = App::with_paths(temp.paths());

        send(&mut app, Message::NewTabOpen);
        send(&mut app, Message::NewTabClose);
        assert!(app.new_tab.is_none());
        assert_eq!(app.screen, Screen::Remote);
        assert!(app.show_home);
        assert_eq!(app.tree().unwrap().session_count(), 0);

        send(&mut app, Message::NewTabOpen);
        fill(&mut app, "Web-01", "10.20.1.11", "22", "ops");
        send(&mut app, Message::ApplyDraft);
        send(&mut app, Message::NewTabClose);
        assert_eq!(
            app.tree().unwrap().session_count(),
            1,
            "保存过的连接不能在关标签时被删掉"
        );
    }

    /// 提示条是瞬态反馈：到点自己消失，但错误要多留一会儿给用户读完。
    #[test]
    fn status_messages_expire_unless_they_are_errors() {
        let temp = TempDir::new("status-expiry");
        let mut app = App::with_paths(temp.paths());

        app.status = Some(Status::info("saved"));
        send(&mut app, Message::ExpireStatus);
        assert!(app.status.is_some(), "刚出现的提示不该立刻消失");

        app.status.as_mut().unwrap().shown_at =
            Instant::now() - STATUS_LIFETIME_INFO - Duration::from_millis(1);
        send(&mut app, Message::ExpireStatus);
        assert!(app.status.is_none(), "普通提示应到点消失");

        app.status = Some(Status::error("boom"));
        app.status.as_mut().unwrap().shown_at =
            Instant::now() - STATUS_LIFETIME_INFO - Duration::from_millis(1);
        send(&mut app, Message::ExpireStatus);
        assert!(app.status.is_some(), "错误提示应比普通提示多停留一会儿");
    }

    /// 回归：在设置页里点侧栏「会话」或点树上的条目，必须真的切回会话页。
    ///
    /// 这两个动作过去只设了 `show_home`，忘了把 `screen` 从 `Settings` 摘出来，
    /// 于是界面「点了没反应」——只有实际跑起来点一下才会发现，光看状态位很像是通的。
    #[test]
    fn leaving_the_settings_page_by_sidebar_or_selection() {
        let temp = TempDir::new("leave-settings");
        let mut app = App::with_paths(temp.paths());

        send(&mut app, Message::NewSession(SessionKind::Ssh));
        fill(&mut app, "跳板机", "10.0.0.1", "22", "root");
        send(&mut app, Message::ApplyDraft);
        let id = app.selected.clone().expect("新建后应处于选中态");

        send(&mut app, Message::ShowSettings);
        assert_eq!(app.screen, Screen::Settings);

        send(&mut app, Message::ShowHome);
        assert_eq!(app.screen, Screen::Remote, "侧栏「会话」要能离开设置页");

        send(&mut app, Message::ShowSettings);
        send(&mut app, Message::SelectNode(id));
        assert_eq!(app.screen, Screen::Remote, "选中条目要能离开设置页");
        assert!(app.show_home, "选中条目后应停在主页标签");
    }

    /// 回归：设置页改全局默认必须落盘，并且只影响「继承默认」的连接。
    ///
    /// 数据层（`set_rdp_defaults`）早就有，缺的是界面入口；这个用例守的是
    /// 「改完重启还在」——只校验内存里的话，标脏漏了也照样能过。
    #[test]
    fn global_defaults_are_editable_and_survive_a_restart() {
        let temp = TempDir::new("global-defaults");

        {
            let mut app = App::with_paths(temp.paths());
            send(
                &mut app,
                Message::RdpDefaultEdit(RdpEdit::Performance(Some(RdpPerformance::Lan))),
            );
            send(
                &mut app,
                Message::RdpDefaultEdit(RdpEdit::Redirect {
                    field: RdpRedirect::Drives,
                    value: Some(true),
                }),
            );
            send(
                &mut app,
                Message::TerminalDefaultEdit(TerminalEdit::Cursor(Some(TerminalCursor::Beam))),
            );
            app.save_now();

            assert_eq!(app.rdp_defaults().performance, Some(RdpPerformance::Lan));
            assert_eq!(app.rdp_defaults().redirect_drives, Some(true));
            assert_eq!(app.terminal_defaults().cursor, Some(TerminalCursor::Beam));
        }

        let reopened = App::with_paths(temp.paths());
        assert_eq!(
            reopened.rdp_defaults().performance,
            Some(RdpPerformance::Lan)
        );
        assert_eq!(reopened.rdp_defaults().redirect_drives, Some(true));
        assert_eq!(
            reopened.terminal_defaults().cursor,
            Some(TerminalCursor::Beam)
        );
    }

    #[test]
    fn resetting_global_defaults_brings_back_the_builtin_values() {
        let temp = TempDir::new("reset-defaults");
        let mut app = App::with_paths(temp.paths());

        send(
            &mut app,
            Message::RdpDefaultEdit(RdpEdit::ColorDepth(Some(RdpColorDepth::Bpp16))),
        );
        send(
            &mut app,
            Message::TerminalDefaultEdit(TerminalEdit::FontSize(Some(18))),
        );
        assert!(app.rdp_defaults().color_depth.is_some());

        send(&mut app, Message::ResetRdpDefaults);
        send(&mut app, Message::ResetTerminalDefaults);

        assert!(app.rdp_defaults().is_empty());
        assert!(app.terminal_defaults().is_empty());
        // 内置默认不受影响：解析结果仍应是 32 位 / 14pt。
        let resolved = RdpSettings::default().resolve(&app.rdp_defaults());
        assert_eq!(resolved.color_depth, RdpColorDepth::Bpp32);
    }

    /// 「全屏」是分辨率的一个档位，写成 `.rdp` 时要取主显示器尺寸而不是内容区尺寸。
    #[test]
    fn the_fullscreen_desktop_size_is_written_as_the_primary_monitor() {
        let temp = TempDir::new("fullscreen-size");
        let mut app = App::with_paths(temp.paths());

        send(&mut app, Message::NewSession(SessionKind::Rdp));
        fill(&mut app, "全屏机", "10.0.0.30", "3389", "ops");
        send(&mut app, Message::ApplyDraft);
        let id = app.selected.clone().expect("新建后应选中 RDP");

        send(
            &mut app,
            Message::RdpEdit(RdpEdit::DesktopSize(Some(RdpDesktopSize::Fullscreen))),
        );

        let session = app.tree().unwrap().find_session(&id).unwrap();
        let defaults = app.rdp_defaults();
        let resolved = session.settings.rdp.resolve(&defaults);
        assert!(resolved.desktop_size.starts_fullscreen());

        let path = rdp::write_profile(session, Some((800, 600)), &resolved).expect("应能写文件");
        let text = std::fs::read_to_string(&path).unwrap();
        match superremote_win::display::primary_size() {
            Some((width, height)) => {
                assert!(text.contains(&format!("desktopwidth:i:{width}")));
                assert!(text.contains(&format!("desktopheight:i:{height}")));
            }
            // 拿不到主显示器尺寸（非 Windows 或驱动异常）时退回内容区，也不能写空。
            None => assert!(text.contains("desktopwidth:i:800")),
        }
        assert!(text.contains("dynamic resolution:i:0"));

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_new_session_written_through_the_ui_is_still_there_after_a_restart() {
        let temp = TempDir::new("persist");

        {
            let mut app = App::with_paths(temp.paths());
            send(&mut app, Message::NewDirectory);
            let dir_id = app.selected.clone().expect("新建目录后应被选中");

            send(&mut app, Message::NewSession(SessionKind::Ssh));
            fill(&mut app, "Web-01", "10.20.1.11", "22", "ops");
            send(
                &mut app,
                Message::DraftChanged(DraftField::Password, "s3cr3t".to_owned()),
            );
            send(&mut app, Message::ApplyDraft);

            let tree = app.tree().unwrap();
            assert_eq!(tree.session_count(), 1);
            assert!(tree.is_directory(&dir_id), "目录应当还在");
        }

        let reopened = App::with_paths(temp.paths());
        let tree = reopened.tree().expect("重启后应能打开数据");
        let session = tree
            .rows("")
            .iter()
            .find_map(|row| match row {
                TreeRow::Session { id, .. } => Some(id.clone()),
                _ => None,
            })
            .expect("重启后树里应有一条连接");

        let session = tree.find_session(&session).unwrap();
        assert_eq!(
            (session.name.as_str(), session.address().as_str()),
            ("Web-01", "10.20.1.11:22")
        );
        assert_eq!(session.username.as_deref(), Some("ops"));
        assert_eq!(reopened.secret_count(), 1);
        assert_eq!(
            reopened
                .store
                .as_ref()
                .unwrap()
                .password(&session.id)
                .unwrap()
                .as_deref(),
            Some("s3cr3t")
        );
    }

    #[test]
    fn an_invalid_port_is_reported_and_leaves_the_stored_record_untouched() {
        let temp = TempDir::new("bad-port");
        let mut app = App::with_paths(temp.paths());

        send(&mut app, Message::NewSession(SessionKind::Rdp));
        fill(&mut app, "Jump", "10.0.0.5", "70000", "admin");
        send(&mut app, Message::ApplyDraft);

        assert!(app.status.as_ref().unwrap().is_error, "端口非法应报错");
        let id = app.selected.clone().unwrap();
        let session = app.tree().unwrap().find_session(&id).unwrap();
        assert_eq!(session.port, SessionKind::Rdp.default_port());
        assert_eq!(session.host, "");
    }

    #[test]
    fn deleting_a_directory_closes_its_tabs_and_forgets_its_credentials() {
        let temp = TempDir::new("delete");
        let mut app = App::with_paths(temp.paths());

        send(&mut app, Message::NewDirectory);
        let dir_id = app.selected.clone().unwrap();
        send(&mut app, Message::NewSession(SessionKind::Ssh));
        fill(&mut app, "Web", "10.0.0.1", "22", "");
        send(
            &mut app,
            Message::DraftChanged(DraftField::Password, "pw".to_owned()),
        );
        send(&mut app, Message::ApplyDraft);
        let session_id = app.selected.clone().unwrap();

        send(&mut app, Message::OpenSelected);
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.tabs.active_tab().unwrap().id(), &session_id);
        assert_eq!(app.secret_count(), 1);

        send(&mut app, Message::SelectNode(dir_id));
        send(&mut app, Message::DeleteSelected);

        assert_eq!(app.tree().unwrap().session_count(), 0);
        assert_eq!(app.tabs.len(), 0, "子会话的标签要一起关掉");
        assert_eq!(app.secret_count(), 0, "凭据要一起删掉，不留孤儿记录");
        assert!(app.selected.is_none());

        let reopened = App::with_paths(temp.paths());
        assert_eq!(reopened.secret_count(), 0);
        assert!(reopened.tree().unwrap().rows("").is_empty());
    }

    #[test]
    fn search_and_toggle_keep_the_materialized_rows_in_sync() {
        let temp = TempDir::new("rows");
        let mut app = App::with_paths(temp.paths());

        send(&mut app, Message::NewDirectory);
        let dir_id = app.selected.clone().unwrap();
        send(&mut app, Message::NewSession(SessionKind::Ssh));
        assert_eq!(app.rows.len(), 2, "目录展开后应看到它和里面的连接");

        send(&mut app, Message::SearchChanged("nomatch".to_owned()));
        assert!(app.rows.is_empty());

        send(&mut app, Message::SearchChanged(String::new()));
        assert_eq!(app.rows.len(), 2);

        send(&mut app, Message::ToggleDirectory(dir_id));
        assert_eq!(app.rows.len(), 1, "折叠后只剩目录本身");
    }

    #[test]
    fn a_new_node_prefills_its_draft_so_saving_is_not_a_dead_end() {
        let temp = TempDir::new("prefill");
        let mut app = App::with_paths(temp.paths());

        send(&mut app, Message::NewDirectory);
        assert_eq!(app.draft.name, "新建目录");
        send(&mut app, Message::ApplyDraft);
        assert!(
            !app.status.as_ref().unwrap().is_error,
            "只改名直接保存不该报错"
        );

        send(&mut app, Message::NewSession(SessionKind::Telnet));
        assert_eq!(app.draft.name, "新建 TELNET");
        assert_eq!(app.draft.port, "23", "端口草稿应跟随协议默认值");
    }

    #[test]
    fn closing_the_last_tab_falls_back_to_the_home_page() {
        let temp = TempDir::new("last-tab");
        let mut app = App::with_paths(temp.paths());

        send(&mut app, Message::NewSession(SessionKind::Ssh));
        fill(&mut app, "Web", "10.0.0.1", "22", "");
        send(&mut app, Message::ApplyDraft);
        send(&mut app, Message::OpenSelected);
        assert!(!app.show_home, "打开连接后应切到标签视图");

        send(&mut app, Message::CloseTab(0));
        assert!(app.show_home, "最后一个标签关掉后要回主页");
    }

    #[test]
    fn master_password_gates_the_next_start_through_the_unlock_screen() {
        let temp = TempDir::new("unlock");

        {
            let mut app = App::with_paths(temp.paths());
            send(&mut app, Message::NewSession(SessionKind::Ssh));
            fill(&mut app, "Web", "10.0.0.1", "22", "");
            send(
                &mut app,
                Message::DraftChanged(DraftField::Password, "pw".to_owned()),
            );
            send(&mut app, Message::ApplyDraft);

            app.master_password_draft = "短".to_owned();
            send(&mut app, Message::EnableMasterPassword);
            assert!(app.status.as_ref().unwrap().is_error, "口令太短应被拒绝");

            app.master_password_draft = "口令至少六位".to_owned();
            send(&mut app, Message::EnableMasterPassword);
            assert!(!app.status.as_ref().unwrap().is_error);
        }

        let mut locked = App::with_paths(temp.paths());
        assert_eq!(locked.screen, Screen::Unlock);
        assert!(locked.store.is_none(), "未解锁前不该拿到密钥");

        locked.unlock.password = "不对".to_owned();
        send(&mut locked, Message::SubmitUnlock);
        assert_eq!(locked.screen, Screen::Unlock);
        assert_eq!(locked.unlock.error.as_deref(), Some("口令不正确"));

        locked.unlock.password = "口令至少六位".to_owned();
        send(&mut locked, Message::SubmitUnlock);
        assert_eq!(locked.screen, Screen::Remote);
        assert_eq!(locked.tree().unwrap().session_count(), 1);
        assert_eq!(locked.secret_count(), 1);
    }

    #[test]
    fn rdp_settings_edits_persist_and_reset_keeps_global_defaults() {
        let temp = TempDir::new("rdp-settings");
        let id;

        {
            let mut app = App::with_paths(temp.paths());
            send(&mut app, Message::NewSession(SessionKind::Rdp));
            fill(&mut app, "RDP", "10.0.0.5", "3389", "admin");
            send(&mut app, Message::ApplyDraft);
            id = app.selected.clone().expect("新建后应选中 RDP");

            send(
                &mut app,
                Message::RdpEdit(RdpEdit::Performance(Some(RdpPerformance::Lan))),
            );
            send(
                &mut app,
                Message::RdpEdit(RdpEdit::Redirect {
                    field: RdpRedirect::Clipboard,
                    value: Some(false),
                }),
            );
            send(&mut app, Message::MakeRdpDefaults);

            let session = app.tree().unwrap().find_session(&id).unwrap();
            assert_eq!(session.settings.rdp.performance, Some(RdpPerformance::Lan));
            assert_eq!(session.settings.rdp.redirect_clipboard, Some(false));
            assert_eq!(app.rdp_defaults().performance, Some(RdpPerformance::Lan));
            assert_eq!(app.rdp_defaults().redirect_clipboard, Some(false));

            send(&mut app, Message::ResetRdpSettings);
            assert!(app
                .tree()
                .unwrap()
                .find_session(&id)
                .unwrap()
                .settings
                .rdp
                .is_empty());

            send(&mut app, Message::Autosave);
        }

        let reopened = App::with_paths(temp.paths());
        assert_eq!(
            reopened
                .tree()
                .unwrap()
                .find_session(&id)
                .unwrap()
                .settings
                .rdp,
            RdpSettings::default()
        );
        assert_eq!(
            reopened.rdp_defaults().performance,
            Some(RdpPerformance::Lan)
        );
        assert_eq!(reopened.rdp_defaults().redirect_clipboard, Some(false));
    }

    #[test]
    fn terminal_settings_edits_persist_and_reset_keeps_global_defaults() {
        let temp = TempDir::new("terminal-settings");
        let id;

        {
            let mut app = App::with_paths(temp.paths());
            send(&mut app, Message::NewSession(SessionKind::Ssh));
            fill(&mut app, "Shell", "10.0.0.6", "22", "ops");
            send(&mut app, Message::ApplyDraft);
            id = app.selected.clone().expect("新建后应选中 SSH");

            send(
                &mut app,
                Message::TerminalEdit(TerminalEdit::FontSize(Some(18))),
            );
            send(
                &mut app,
                Message::TerminalEdit(TerminalEdit::Cursor(Some(TerminalCursor::Beam))),
            );
            send(&mut app, Message::MakeTerminalDefaults);

            let session = app.tree().unwrap().find_session(&id).unwrap();
            assert_eq!(session.settings.terminal.font_size, Some(18));
            assert_eq!(session.settings.terminal.cursor, Some(TerminalCursor::Beam));
            assert_eq!(app.terminal_defaults().font_size, Some(18));
            assert_eq!(app.terminal_defaults().cursor, Some(TerminalCursor::Beam));

            send(&mut app, Message::ResetTerminalSettings);
            assert!(app
                .tree()
                .unwrap()
                .find_session(&id)
                .unwrap()
                .settings
                .terminal
                .is_empty());

            send(&mut app, Message::Autosave);
        }

        let reopened = App::with_paths(temp.paths());
        assert_eq!(
            reopened
                .tree()
                .unwrap()
                .find_session(&id)
                .unwrap()
                .settings
                .terminal,
            TerminalSettings::default()
        );
        assert_eq!(reopened.terminal_defaults().font_size, Some(18));
        assert_eq!(
            reopened.terminal_defaults().cursor,
            Some(TerminalCursor::Beam)
        );
    }

    #[test]
    fn ssh_private_key_is_saved_and_can_be_cleared() {
        let temp = TempDir::new("private-key-ui");
        let mut app = App::with_paths(temp.paths());

        send(&mut app, Message::NewSession(SessionKind::Ssh));
        fill(&mut app, "Shell", "10.0.0.7", "22", "ops");
        app.draft.private_key = "-----BEGIN OPENSSH PRIVATE KEY-----\nkey\n".to_owned();
        send(&mut app, Message::ApplyDraft);
        let id = app.selected.clone().unwrap();

        assert_eq!(
            app.store
                .as_ref()
                .unwrap()
                .private_key(&id)
                .unwrap()
                .as_deref(),
            Some("-----BEGIN OPENSSH PRIVATE KEY-----\nkey\n")
        );
        assert!(app.has_private_key(&id));

        send(&mut app, Message::ClearPrivateKey);
        assert!(app
            .store
            .as_ref()
            .unwrap()
            .private_key(&id)
            .unwrap()
            .is_none());
        assert!(!app.has_private_key(&id));
        assert!(app.draft.private_key.is_empty());
    }

    /// 右键菜单的动作必须与界面按钮走同一条路，否则「右键删除」和「选中后点删除」
    /// 早晚会走岔。这里把四条路径都过一遍：展开/折叠、右键不跳页、重命名、删除子树。
    #[test]
    fn the_context_menu_actions_reach_the_same_state_as_the_buttons() {
        let temp = TempDir::new("menu-actions");
        let mut app = App::with_paths(temp.paths());

        send(&mut app, Message::NewDirectory);
        let dir_id = app.selected.clone().expect("新建目录后应被选中");
        send(&mut app, Message::NewSession(SessionKind::Ssh));
        fill(&mut app, "Web-01", "10.20.1.11", "22", "ops");
        send(&mut app, Message::ApplyDraft);
        let session_id = app.selected.clone().expect("新建后应处于选中态");

        // 右键只标记选中，不能把人从正在看的页面里拽走。
        send(&mut app, Message::ShowSettings);
        send(&mut app, Message::HighlightNode(session_id.clone()));
        assert_eq!(
            app.screen,
            Screen::Settings,
            "右键不该跳页，只有左键选中才会"
        );

        // 重命名会回到编辑页（名字在那里才看得见），改完名字要落到树上。
        send(
            &mut app,
            Message::TreeAction(TreeAction::Rename(session_id.clone())),
        );
        assert_eq!(app.screen, Screen::Remote);
        send(
            &mut app,
            Message::DraftChanged(DraftField::Name, "Web-02".to_owned()),
        );
        send(&mut app, Message::ApplyDraft);
        assert_eq!(
            app.tree()
                .unwrap()
                .find_session(&session_id)
                .map(|session| session.name.clone()),
            Some("Web-02".to_owned()),
            "改名必须真的写回会话树"
        );

        // 删除目录要连带清掉里面的连接。
        send(&mut app, Message::TreeAction(TreeAction::Delete(dir_id)));
        assert_eq!(app.tree().unwrap().session_count(), 0);
    }

    /// 后台打开：建标签、连上，但不把用户从当前页面拽走。
    #[test]
    fn opening_in_the_background_keeps_the_current_view() {
        let temp = TempDir::new("background");
        let mut app = App::with_paths(temp.paths());

        send(&mut app, Message::NewSession(SessionKind::Ssh));
        fill(&mut app, "Web-01", "10.20.1.11", "22", "ops");
        send(&mut app, Message::ApplyDraft);
        let first = app.selected.clone().expect("新建后应处于选中态");

        send(&mut app, Message::NewSession(SessionKind::Ssh));
        fill(&mut app, "Web-02", "10.20.1.12", "22", "ops");
        send(&mut app, Message::ApplyDraft);
        let second = app.selected.clone().expect("新建后应处于选中态");

        // 先前台打开第一条，画面落在它的标签上。
        send(&mut app, Message::SelectNode(first));
        send(&mut app, Message::OpenSelected);
        assert!(!app.show_home, "前台打开应切到标签视图");
        let active_before = app.tabs.active_index();

        // 后台打开第二条：标签要多一个，但活动标签与当前页面都不该变。
        send(&mut app, Message::OpenInBackground(second));
        assert_eq!(app.tabs.len(), 2, "后台打开也要建标签并连上");
        assert_eq!(app.tabs.active_index(), active_before, "后台打开不该抢焦点");
        assert!(!app.show_home, "不该被拽回主页标签");
    }

    /// 在设置页点标题栏的会话标签，必须真的切回会话画面：`screen` 也要从
    /// `Settings` 摘出来，否则标签高亮变了、主区域还停在设置页。
    #[test]
    fn activating_a_tab_from_settings_returns_to_the_session() {
        let temp = TempDir::new("tab-from-settings");
        let mut app = App::with_paths(temp.paths());

        send(&mut app, Message::NewSession(SessionKind::Ssh));
        fill(&mut app, "Web-01", "10.20.1.11", "22", "ops");
        send(&mut app, Message::ApplyDraft);
        send(&mut app, Message::OpenSelected);
        assert!(!app.show_home, "前台打开后应停在会话标签");

        send(&mut app, Message::ShowSettings);
        assert_eq!(app.screen, Screen::Settings);

        send(&mut app, Message::ActivateTab(0));
        assert_eq!(
            app.screen,
            Screen::Remote,
            "点会话标签要把 screen 从设置页摘出来"
        );
        assert!(!app.show_home, "标签激活后应停在会话页");
    }

    /// 图标栏第二项是独立的连接列表页，不是主页的另一个入口：
    /// 从列表选中一条要回到主页详情，从列表直接打开要切到会话标签。
    #[test]
    fn the_connection_list_is_a_separate_screen() {
        let temp = TempDir::new("list-screen");
        let mut app = App::with_paths(temp.paths());

        send(&mut app, Message::NewSession(SessionKind::Ssh));
        fill(&mut app, "Web-01", "10.20.1.11", "22", "ops");
        send(&mut app, Message::ApplyDraft);
        let id = app.selected.clone().expect("新建后应处于选中态");

        send(&mut app, Message::ShowList);
        assert_eq!(app.screen, Screen::List);
        assert!(!app.show_home, "列表页不是主页标签");

        send(&mut app, Message::SelectNode(id.clone()));
        assert_eq!(app.screen, Screen::Remote, "选中条目应回到主页详情");
        assert!(app.show_home);

        send(&mut app, Message::ShowList);
        send(&mut app, Message::TreeAction(TreeAction::Open(id)));
        assert_eq!(app.screen, Screen::Remote, "从列表打开要把 screen 摘出来");
        assert_eq!(app.tabs.len(), 1);
    }

    /// 构造一次「裸按键」走完整的 `handle_key` 管线，导航认领与否都在里面判定。
    fn nav_key(named: iced::keyboard::Key) -> Message {
        Message::KeyInput(Box::new(iced::keyboard::Event::KeyPressed {
            key: named.clone(),
            modified_key: named,
            physical_key: iced::keyboard::key::Physical::Code(iced::keyboard::key::Code::ArrowDown),
            location: iced::keyboard::Location::Standard,
            modifiers: iced::keyboard::Modifiers::NONE,
            text: None,
            repeat: false,
        }))
    }

    fn arrow(named: iced::keyboard::key::Named) -> Message {
        nav_key(iced::keyboard::Key::Named(named))
    }

    /// 主页上的 ↑↓ 沿树行移动选中；没有选中时 ↓ 从第一行开始。
    #[test]
    fn arrow_keys_walk_the_tree_rows() {
        let temp = TempDir::new("nav-tree");
        let mut app = App::with_paths(temp.paths());

        send(&mut app, Message::NewSession(SessionKind::Ssh));
        fill(&mut app, "Web-01", "10.20.1.11", "22", "ops");
        send(&mut app, Message::ApplyDraft);
        let first = app.selected.clone().expect("新建后应处于选中态");

        send(&mut app, Message::NewSession(SessionKind::Ssh));
        fill(&mut app, "Web-02", "10.20.1.12", "22", "ops");
        send(&mut app, Message::ApplyDraft);
        let second = app.selected.clone().expect("第二条也应处于选中态");
        assert_eq!(app.rows.len(), 2, "两条根层连接应是两行");

        // 清空选中后再按 ↓：应落在第一行，而不是维持原位。
        app.selected = None;
        send(&mut app, arrow(iced::keyboard::key::Named::ArrowDown));
        assert_eq!(app.selected.as_deref(), Some(first.as_str()));

        send(&mut app, arrow(iced::keyboard::key::Named::ArrowDown));
        assert_eq!(
            app.selected.as_deref(),
            Some(second.as_str()),
            "再按 ↓ 到第二行"
        );

        send(&mut app, arrow(iced::keyboard::key::Named::ArrowUp));
        assert_eq!(
            app.selected.as_deref(),
            Some(first.as_str()),
            "↑ 回到第一行"
        );

        // 两端夹住：越界按键不许 panic，也不许改变选中。
        send(&mut app, arrow(iced::keyboard::key::Named::ArrowUp));
        assert_eq!(app.selected.as_deref(), Some(first.as_str()));
    }

    /// → 展开目录、← 折叠；Enter 在目录行上等价于双击（切换展开）。
    #[test]
    fn arrows_and_enter_drive_directories() {
        let temp = TempDir::new("nav-dir");
        let mut app = App::with_paths(temp.paths());

        send(&mut app, Message::NewDirectory);
        let dir_id = app.selected.clone().expect("新建目录后应处于选中态");
        send(&mut app, Message::NewSession(SessionKind::Ssh));
        fill(&mut app, "Db-01", "10.20.2.11", "22", "ops");
        send(&mut app, Message::ApplyDraft);
        let session_id = app.selected.clone().expect("目录里的连接也应选中");

        // 往目录里建连接会自动展开目录（`push_node`：新建的连接必须立刻可见），
        // 所以这里先按 ← 折叠，再验证 → / Enter 的切换语义。
        assert_eq!(app.rows.len(), 2, "建完连接后目录是展开的");
        send(&mut app, Message::SelectNode(dir_id.clone()));
        assert_eq!(app.selected.as_deref(), Some(dir_id.as_str()));

        send(&mut app, arrow(iced::keyboard::key::Named::ArrowLeft));
        assert_eq!(app.rows.len(), 1, "← 应折叠目录");

        // 已在目标状态时再按同向键不许有副作用。
        send(&mut app, arrow(iced::keyboard::key::Named::ArrowLeft));
        assert_eq!(app.rows.len(), 1);

        send(&mut app, arrow(iced::keyboard::key::Named::ArrowRight));
        assert_eq!(app.rows.len(), 2, "→ 应展开目录");
        assert_eq!(
            app.selected.as_deref(),
            Some(dir_id.as_str()),
            "展开不移动选中"
        );
        send(&mut app, arrow(iced::keyboard::key::Named::ArrowRight));
        assert_eq!(app.rows.len(), 2);

        send(&mut app, arrow(iced::keyboard::key::Named::Enter));
        assert_eq!(app.rows.len(), 1, "Enter 在目录行上应切换展开");

        send(&mut app, arrow(iced::keyboard::key::Named::Enter));
        assert_eq!(app.rows.len(), 2);

        // 展开后 ↓ 进到子行，Enter 在**连接行**上走「打开」。
        send(&mut app, arrow(iced::keyboard::key::Named::ArrowDown));
        assert_eq!(app.selected.as_deref(), Some(session_id.as_str()));
        send(&mut app, arrow(iced::keyboard::key::Named::Enter));
        assert_eq!(app.tabs.len(), 1, "连接行上的 Enter 应等价于「打开」");
        assert!(!app.show_home, "打开后停在会话页");
    }

    /// Delete 走右键「删除」的同一条路径；终端页上的方向键不属于导航。
    #[test]
    fn delete_via_keyboard_and_the_terminal_screen_keeps_arrows() {
        let temp = TempDir::new("nav-delete");
        let mut app = App::with_paths(temp.paths());

        send(&mut app, Message::NewSession(SessionKind::Ssh));
        fill(&mut app, "Web-01", "10.20.1.11", "22", "ops");
        send(&mut app, Message::ApplyDraft);

        send(&mut app, arrow(iced::keyboard::key::Named::Delete));
        assert_eq!(
            app.tree().unwrap().session_count(),
            0,
            "Delete 应删除选中行"
        );
        assert_eq!(app.selected, None);

        // 会话页（show_home=false）上的方向键属于远端 shell，不许挪树。
        send(&mut app, Message::NewSession(SessionKind::Ssh));
        fill(&mut app, "Web-02", "10.20.1.12", "22", "ops");
        send(&mut app, Message::ApplyDraft);
        let second = app.selected.clone().expect("重建后应处于选中态");
        send(
            &mut app,
            Message::TreeAction(TreeAction::Open(second.clone())),
        );
        assert!(!app.show_home, "应停在会话页");

        send(&mut app, arrow(iced::keyboard::key::Named::ArrowUp));
        assert_eq!(
            app.selected.as_deref(),
            Some(second.as_str()),
            "会话页上的 ↑ 不属于导航"
        );
    }

    /// 列表页的 ↑↓ 沿先序移动选中、停在列表页；Enter 走「打开」。
    #[test]
    fn the_list_page_navigates_by_keyboard() {
        let temp = TempDir::new("nav-list");
        let mut app = App::with_paths(temp.paths());

        for host in ["10.20.1.11", "10.20.1.12"] {
            send(&mut app, Message::NewSession(SessionKind::Ssh));
            fill(&mut app, "Web", host, "22", "ops");
            send(&mut app, Message::ApplyDraft);
        }
        let ids = app.tree().unwrap().session_ids(&app.search);
        assert_eq!(ids.len(), 2);

        send(&mut app, Message::ShowList);
        app.selected = None;
        send(&mut app, arrow(iced::keyboard::key::Named::ArrowDown));
        assert_eq!(
            app.selected.as_deref(),
            Some(ids[0].as_str()),
            "↓ 从第一条开始"
        );
        assert_eq!(app.screen, Screen::List, "导航不许跳页");

        send(&mut app, arrow(iced::keyboard::key::Named::ArrowDown));
        assert_eq!(app.selected.as_deref(), Some(ids[1].as_str()));

        send(&mut app, arrow(iced::keyboard::key::Named::ArrowUp));
        assert_eq!(app.selected.as_deref(), Some(ids[0].as_str()));

        send(&mut app, arrow(iced::keyboard::key::Named::Enter));
        assert_eq!(app.tabs.len(), 1, "列表页 Enter 等价于「打开」");
        assert_eq!(app.screen, Screen::Remote, "打开要把 screen 摘出来");
    }

    /// 树行的左键单击只选中、连点两次直接打开。Iced 的 `button` 只有
    /// `on_press`，没有双击事件，判定就落在 `SelectNode` 上，所以这条
    /// 契约必须钉住：否则以后改选择逻辑很容易把双击悄悄弄丢。
    #[test]
    fn double_clicking_a_row_opens_the_session() {
        let temp = TempDir::new("double-click");
        let mut app = App::with_paths(temp.paths());

        send(&mut app, Message::NewSession(SessionKind::Ssh));
        fill(&mut app, "Web-01", "10.20.1.11", "22", "ops");
        send(&mut app, Message::ApplyDraft);
        let id = app.selected.clone().expect("新建后应处于选中态");

        send(&mut app, Message::SelectNode(id.clone()));
        assert_eq!(app.tabs.len(), 0, "单击只选中，不该建标签");
        assert!(app.show_home, "单击应停在主页详情");

        send(&mut app, Message::SelectNode(id));
        assert_eq!(app.tabs.len(), 1, "第二次点击应直接打开");
        assert!(!app.show_home, "双击打开后应停在会话画面");
    }

    /// 复制一个连接：新 id、同一份数据，凭据也要跟着复制，否则新连接第一次
    /// 连接才发现没有口令。
    #[test]
    fn copying_a_session_duplicates_its_data_and_credentials() {
        let temp = TempDir::new("copy-node");
        let mut app = App::with_paths(temp.paths());

        send(&mut app, Message::NewSession(SessionKind::Ssh));
        fill(&mut app, "Web-01", "10.20.1.11", "22", "ops");
        send(
            &mut app,
            Message::DraftChanged(DraftField::Password, "s3cr3t".to_owned()),
        );
        send(&mut app, Message::ApplyDraft);
        let source = app.selected.clone().expect("新建后应处于选中态");

        send(
            &mut app,
            Message::TreeAction(TreeAction::Copy(source.clone())),
        );
        send(
            &mut app,
            Message::TreeAction(TreeAction::Paste(String::new())),
        );

        let copy = app.selected.clone().expect("粘贴后应选中副本");
        assert_ne!(copy, source, "副本必须换新 id");
        assert_eq!(app.tree().unwrap().session_count(), 2);
        let session = app.tree().unwrap().find_session(&copy).expect("副本还在");
        assert_eq!(session.name, "Web-01");
        assert_eq!(session.host, "10.20.1.11");
        assert_eq!(
            app.store
                .as_ref()
                .unwrap()
                .password(&copy)
                .unwrap()
                .as_deref(),
            Some("s3cr3t"),
            "复制连接时口令也要复制到新 id"
        );
    }

    /// 剪切/粘贴是移动：id 不变、数量不变，落到目录里后父子关系正确。
    #[test]
    fn cutting_and_pasting_moves_the_node_without_losing_its_id() {
        let temp = TempDir::new("cut-node");
        let mut app = App::with_paths(temp.paths());

        send(&mut app, Message::NewSession(SessionKind::Ssh));
        fill(&mut app, "Web-01", "10.20.1.11", "22", "ops");
        send(&mut app, Message::ApplyDraft);
        let source = app.selected.clone().unwrap();

        send(&mut app, Message::NewDirectory);
        let directory = app.selected.clone().unwrap();

        send(
            &mut app,
            Message::TreeAction(TreeAction::Cut(source.clone())),
        );
        send(
            &mut app,
            Message::TreeAction(TreeAction::Paste(directory.clone())),
        );

        let tree = app.tree().unwrap();
        assert_eq!(tree.session_count(), 1, "剪切粘贴不能复制出新节点");
        assert!(tree.find_session(&source).is_some(), "移动保留原 id");
        assert_eq!(tree.parent_id_of(&source), Some(directory));
        assert!(app.node_clipboard.is_none(), "剪切粘贴后剪贴板要清空");
    }

    /// 复制目录要递归复制整棵子树（含里面的连接），粘贴后连接数翻倍。
    #[test]
    fn copying_a_directory_copies_its_subtree() {
        let temp = TempDir::new("copy-dir");
        let mut app = App::with_paths(temp.paths());

        send(&mut app, Message::NewDirectory);
        let directory = app.selected.clone().unwrap();
        send(&mut app, Message::NewSession(SessionKind::Rdp));
        send(
            &mut app,
            Message::DraftChanged(DraftField::Host, "10.0.0.9".into()),
        );
        send(&mut app, Message::ApplyDraft);

        send(
            &mut app,
            Message::TreeAction(TreeAction::Copy(directory.clone())),
        );
        send(
            &mut app,
            Message::TreeAction(TreeAction::Paste(String::new())),
        );

        let tree = app.tree().unwrap();
        assert_eq!(tree.session_count(), 2, "目录里的连接也要复制");
        let copies = tree
            .roots
            .iter()
            .filter(|node| matches!(node, SessionNode::Directory(_)))
            .count();
        assert_eq!(copies, 2, "根层应出现一个复制出来的目录");
    }

    /// 拖拽移动：按下 → 位移超过阈值 → 落在目标行上，树要真的变。
    /// 顺带守住三条边界：没拖起来的点击、落在自己身上、落在空白处都不能动树。
    #[test]
    fn dragging_a_row_moves_it_only_when_it_is_a_real_drop() {
        let temp = TempDir::new("drag");
        let mut app = App::with_paths(temp.paths());

        // 会话建在根层（此刻没选中任何目录），目录也建在根层。
        send(&mut app, Message::NewSession(SessionKind::Ssh));
        fill(&mut app, "Web-01", "10.20.1.11", "22", "ops");
        send(&mut app, Message::ApplyDraft);
        let session_id = app.selected.clone().expect("新建后应处于选中态");
        assert_eq!(app.tree().unwrap().parent_id_of(&session_id), None);

        send(&mut app, Message::NewDirectory);
        let dir_id = app.selected.clone().expect("新建目录后应被选中");

        // 真正的拖拽：落到目录上要搬进去。
        send(&mut app, Message::DragStart(session_id.clone()));
        send(&mut app, Message::DragMove(Point::new(40.0, 120.0)));
        send(&mut app, Message::DragHover(Some(dir_id.clone())));
        assert!(app.is_drop_target(&dir_id), "悬停的行要被标成落点");
        send(&mut app, Message::DragDrop(dir_id.clone()));
        assert_eq!(
            app.tree().unwrap().parent_id_of(&session_id),
            Some(dir_id.clone()),
            "落到目录上应该搬进去"
        );

        // 只按下就松开（没有拖起来）= 一次点击，不能动树。
        send(&mut app, Message::DragStart(session_id.clone()));
        send(&mut app, Message::DragDrop(dir_id.clone()));
        assert_eq!(
            app.tree().unwrap().parent_id_of(&session_id),
            Some(dir_id.clone())
        );

        // 落在自己身上 = 原地不动，也不能报错。
        send(&mut app, Message::DragStart(session_id.clone()));
        send(&mut app, Message::DragMove(Point::new(0.0, 0.0)));
        send(&mut app, Message::DragDrop(session_id.clone()));
        assert_eq!(
            app.tree().unwrap().parent_id_of(&session_id),
            Some(dir_id.clone())
        );

        // 落在空白处：取消。
        send(&mut app, Message::DragStart(session_id.clone()));
        send(&mut app, Message::DragMove(Point::new(0.0, 0.0)));
        send(&mut app, Message::DragCancel);
        assert_eq!(app.tree().unwrap().parent_id_of(&session_id), Some(dir_id));
    }

    /// 标签条右键「关闭其他标签」：留下的必须是右键的那一个。
    /// 从后往前关是关键，否则下标左移会把要留的那个也关掉。
    #[test]
    fn closing_other_tabs_keeps_the_right_clicked_one() {
        let temp = TempDir::new("close-others");
        let mut app = App::with_paths(temp.paths());

        for (name, host) in [
            ("Web-01", "10.20.1.11"),
            ("Web-02", "10.20.1.12"),
            ("Web-03", "10.20.1.13"),
        ] {
            send(&mut app, Message::NewSession(SessionKind::Ssh));
            fill(&mut app, name, host, "22", "ops");
            send(&mut app, Message::ApplyDraft);
            send(&mut app, Message::OpenSelected);
        }
        assert_eq!(app.tabs.len(), 3);

        let keep = app.tabs.tabs()[1].instance;
        send(&mut app, Message::TabAction(TabAction::CloseOthers(keep)));

        assert_eq!(app.tabs.len(), 1);
        assert_eq!(
            app.tabs.active_tab().map(Tab::title),
            Some("Web-02"),
            "留下的必须是右键的那一个"
        );
    }

    /// 回归：首选打开方式按「单条覆盖 → 全局默认 → 内置（选项卡）」分发。
    ///
    /// 这个偏好过去只存在于右键菜单的三个动作里、每次都要手选；现在双击/「打开连接」
    /// 应当直接落到用户选好的位置。
    #[test]
    fn the_preferred_open_mode_decides_where_a_connection_lands() {
        let temp = TempDir::new("open-mode");
        let mut app = App::with_paths(temp.paths());

        send(&mut app, Message::NewSession(SessionKind::Ssh));
        fill(&mut app, "Web-01", "10.20.1.11", "22", "ops");
        send(&mut app, Message::ApplyDraft);
        let id = app.selected.clone().expect("新建后应处于选中态");

        // 没设过：内置默认是开到选项卡里。
        assert_eq!(app.effective_open_mode(&id), OpenMode::Tab);
        send(&mut app, Message::OpenSelected);
        assert_eq!(app.tabs.len(), 1);

        // 全局默认影响没单独设过的连接。
        send(
            &mut app,
            Message::OpenModeDefaultEdit(Some(OpenMode::Background)),
        );
        let second = {
            send(&mut app, Message::NewSession(SessionKind::Ssh));
            fill(&mut app, "Web-02", "10.20.1.12", "22", "ops");
            send(&mut app, Message::ApplyDraft);
            app.selected.clone().expect("新建后应处于选中态")
        };
        assert_eq!(app.effective_open_mode(&second), OpenMode::Background);

        // 单条覆盖优先于全局默认。
        send(&mut app, Message::SelectNode(second.clone()));
        send(&mut app, Message::OpenModeEdit(Some(OpenMode::Detached)));
        assert_eq!(app.effective_open_mode(&second), OpenMode::Detached);

        send(&mut app, Message::OpenSelected);
        assert!(
            app.windows
                .values()
                .any(|slot| matches!(slot, WindowSlot::Detached(_))),
            "首选「独立窗口」时要真的开一个窗口"
        );
    }

    #[test]
    fn the_preferred_open_mode_survives_a_restart() {
        let temp = TempDir::new("open-mode-persist");
        let id = {
            let mut app = App::with_paths(temp.paths());
            send(&mut app, Message::NewSession(SessionKind::Rdp));
            fill(&mut app, "跳板机", "10.0.0.9", "3389", "ops");
            send(&mut app, Message::ApplyDraft);
            let id = app.selected.clone().expect("新建后应处于选中态");
            send(&mut app, Message::OpenModeEdit(Some(OpenMode::Detached)));
            send(
                &mut app,
                Message::OpenModeDefaultEdit(Some(OpenMode::Background)),
            );
            app.save_now();
            id
        };

        let reopened = App::with_paths(temp.paths());
        assert_eq!(reopened.effective_open_mode(&id), OpenMode::Detached);
        assert_eq!(reopened.default_open_mode(), Some(OpenMode::Background));
    }

    /// daemon 不会在窗口关完后自己退出（iced 0.14 文档明说），所以关掉最后一个窗口
    /// 必须显式 `iced::exit()`。少了它，关窗只留下一个没有窗口的进程：占着 exe 不放，
    /// 下次构建撞「文件被占用」，用户还以为程序已经关了。
    #[test]
    fn closing_the_last_window_asks_the_process_to_exit() {
        let temp = TempDir::new("exit-last-window");
        let mut app = App::with_paths(temp.paths());

        let main = window::Id::unique();
        app.windows.insert(main, WindowSlot::Main);
        app.main_window = Some(main);

        send(&mut app, Message::WindowClosed(main));

        assert!(app.windows.is_empty());
        assert_eq!(app.main_window, None, "主窗口关掉后不能再指向它");
        assert!(app.should_exit_after_close());
    }

    /// 关主窗口时独立窗口可能还开着：那边有正在跑的会话，不能顺手把进程结束。
    #[test]
    fn a_detached_window_keeps_the_process_alive_after_the_main_window_closes() {
        let temp = TempDir::new("exit-detached-left");
        let mut app = App::with_paths(temp.paths());

        let main = window::Id::unique();
        let detached = window::Id::unique();
        app.windows.insert(main, WindowSlot::Main);
        app.main_window = Some(main);
        app.windows.insert(
            detached,
            WindowSlot::Detached(Tab {
                session: Session {
                    id: "detached".to_owned(),
                    name: "独立".to_owned(),
                    kind: SessionKind::Ssh,
                    host: "127.0.0.1".to_owned(),
                    port: 22,
                    username: None,
                    settings: Default::default(),
                    window: None,
                },
                instance: app.tabs.allocate(),
            }),
        );

        send(&mut app, Message::WindowClosed(main));
        assert!(!app.should_exit_after_close(), "独立窗口还开着就不能退出");

        send(&mut app, Message::WindowClosed(detached));
        assert!(app.should_exit_after_close(), "最后一个窗口没了才退出");
    }

    /// 回归：独立窗口的尺寸要被记住，下次打开直接用上。
    ///
    /// 尺寸来自 resize 事件而不是关窗时现查——窗口都关了再去问，拿到的多半是
    /// 上一次的缓存或者根本拿不到。
    #[test]
    fn a_detached_window_remembers_its_size_for_the_next_time() {
        let temp = TempDir::new("detached-size");
        let mut app = App::with_paths(temp.paths());

        send(&mut app, Message::NewSession(SessionKind::Ssh));
        fill(&mut app, "Web-01", "10.20.1.11", "22", "ops");
        send(&mut app, Message::ApplyDraft);
        let id = app.selected.clone().expect("新建后应处于选中态");

        send(&mut app, Message::OpenInNewWindow(id.clone()));
        let window = *app.windows.keys().next().expect("要登记一个独立窗口");
        // 用户把窗口拉大：这条消息是 `resize_events` 订阅送进来的。
        send(
            &mut app,
            Message::WindowResized(window, Size::new(1440.0, 900.0)),
        );
        send(&mut app, Message::WindowClosed(window));
        app.save_now();

        assert_eq!(
            app.tree()
                .and_then(|tree| tree.find_session(&id))
                .and_then(|session| session.window),
            Some(WindowSize {
                width: 1440,
                height: 900
            })
        );

        // 重新打开：窗口设置里应当是这个尺寸，而不是默认值。
        let reopened = App::with_paths(temp.paths());
        let session = reopened
            .tree()
            .and_then(|tree| tree.find_session(&id))
            .cloned()
            .expect("重启后连接还在");
        let size = detached_size(session.window);
        assert_eq!((size.width, size.height), (1440.0, 900.0));
    }

    /// 「画面设为 1920×1080」撑的是**画面**，不是窗口：主窗口要再多一条标题栏，
    /// 终端画面还要再多一条分隔线加状态条；RDP 是原生子窗口，画面区自己也让开了
    /// 左右与底下的缩放热区，所以窗口比画面宽 2×`RESIZE_EDGE`、高一条标题栏加
    /// 一条 `RESIZE_EDGE`。
    #[test]
    fn the_fit_size_hides_only_the_chrome_of_its_own_host() {
        let fit = |layout, kind| {
            window_size_for_pane(PaneFit::FullHd.size(), pane_chrome(layout, kind), 1.0)
        };

        assert_eq!(
            fit(rdp_host::PaneLayout::Main, SessionKind::Rdp),
            Size::new(
                1920.0 + 2.0 * ui::RESIZE_EDGE,
                1080.0 + ui::TITLEBAR_H + ui::RESIZE_EDGE
            )
        );
        assert_eq!(
            fit(rdp_host::PaneLayout::Main, SessionKind::Ssh),
            Size::new(
                1920.0,
                1080.0 + ui::TITLEBAR_H + ui::DIVIDER_H + ui::STATUS_BAR_H
            )
        );
        assert_eq!(
            fit(rdp_host::PaneLayout::Detached, SessionKind::Rdp),
            Size::new(
                1920.0 + 2.0 * ui::RESIZE_EDGE,
                1080.0 + ui::FULLSCREEN_BAR_H + ui::RESIZE_EDGE
            )
        );
    }

    /// 高 DPI 下画面按实际像素算：150% 屏上 1920 物理宽 = 1280 逻辑宽。
    /// 装饰不除 DPI——它们本来就按逻辑像素布局，窗口多大就多大。
    #[test]
    fn the_fit_size_divides_the_pane_by_the_display_scale() {
        assert_eq!(
            window_size_for_pane(
                PaneFit::FullHd.size(),
                pane_chrome(rdp_host::PaneLayout::Main, SessionKind::Rdp),
                1.5,
            ),
            Size::new(1286.0, 761.0)
        );
    }

    /// 提示条画在自己的窗口里，而窗口尺寸就是 `toast_size` 算出来的那一块——
    /// 算小了就会裁字。判据不是拿同一套算式自证：先在一个大窗里量出文字**完整**
    /// 的包围盒，再在实际尺寸里量一次，两者必须一样大。
    #[test]
    fn the_toast_window_is_exactly_big_enough_for_its_message() {
        for message in [
            "已保存",
            "无法连接：远端主机关闭了会话，请检查端口与防火墙是否放行 3389，以及口令是否已经过期",
        ] {
            let status = Status::info(message);
            let size = ui::toast_size(&status.message);

            let measure = |window: Size| {
                let mut sim = iced_test::Simulator::with_size(
                    iced::Settings {
                        default_font: theme::ui_font(),
                        ..iced::Settings::default()
                    },
                    window,
                    ui::toast_bubble(&status),
                );
                sim.find(message)
                    .expect("消息文字要画得出来")
                    .visible_bounds()
                    .expect("消息在可视区内")
            };

            let full = measure(Size::new(size.width + 200.0, size.height + 200.0));
            let fitted = measure(size);

            assert!(
                fitted.height + 0.5 >= full.height,
                "「{message}」的提示条被窗口裁掉了：可视高 {}，完整需要 {}",
                fitted.height,
                full.height
            );
            assert!(
                fitted.width + 0.5 >= full.width,
                "「{message}」的提示条被窗口裁掉了：可视宽 {}，完整需要 {}",
                fitted.width,
                full.width
            );
        }
    }

    /// 无边框窗口没有原生标题栏，独立窗口标题条右侧那三个格子是唯一的关闭入口。
    /// 这里按真实事件点一次「×」，要求它发的是**这个窗口**的 `WindowClose`——
    /// 拖拽区与按钮同一条时，按下会先被 `mouse_area` 吃掉，所以点击必须落在按钮上。
    #[test]
    fn the_detached_window_closes_from_its_own_title_bar() {
        let temp = TempDir::new("detached-close");
        let mut app = App::with_paths(temp.paths());
        send(&mut app, Message::NewSession(SessionKind::Ssh));
        fill(&mut app, "出口", "10.20.1.11", "22", "ops");
        send(&mut app, Message::ApplyDraft);
        let id = app.selected.clone().expect("新建后应处于选中态");
        send(&mut app, Message::OpenInNewWindow(id));

        let detached = app
            .windows
            .iter()
            .find_map(|(id, slot)| matches!(slot, WindowSlot::Detached(_)).then_some(*id))
            .expect("要登记一个独立窗口");

        let mut sim = iced_test::Simulator::with_size(
            iced::Settings {
                default_font: theme::ui_font(),
                ..iced::Settings::default()
            },
            Size::new(DETACHED_W, DETACHED_H),
            app.view(detached),
        );
        // 三个 44px 格子从右沿往左排：关闭在最右，取它的中心。
        sim.point_at(Point::new(DETACHED_W - 22.0, ui::FULLSCREEN_BAR_H / 2.0));
        sim.simulate([
            iced::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
            iced::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
        ]);
        let messages = sim.into_messages().collect::<Vec<_>>();

        assert!(
            messages
                .iter()
                .any(|m| matches!(m, Message::WindowClose(w) if *w == detached)),
            "点「×」没有关掉这个独立窗口：{messages:?}"
        );
        assert!(
            !messages
                .iter()
                .any(|m| matches!(m, Message::WindowDrag(_) | Message::WindowToggleMaximize(_))),
            "同一次点击不该又开始拖窗口或切最大化：{messages:?}"
        );
    }

    /// 独立窗口没有标签条，标题条右键就是它唯一的动作入口。两处必须共用一份
    /// （`session_actions`），否则改一处漏一处；差别只有「在新窗口打开」与
    /// 「关闭其他标签」这两条对独立窗口没有意义。
    #[test]
    fn a_detached_session_offers_the_fit_without_the_tab_only_actions() {
        let temp = TempDir::new("session-actions");
        let mut app = App::with_paths(temp.paths());
        let instance = app.tabs.allocate();

        let tab = |kind: SessionKind| Tab {
            session: Session {
                id: "s".to_owned(),
                name: "会话".to_owned(),
                kind,
                host: "10.0.0.1".to_owned(),
                port: 22,
                username: None,
                settings: Default::default(),
                window: None,
            },
            instance,
        };
        let detached_ssh = ui::workspace::session_actions(&tab(SessionKind::Ssh), true);
        let main_ssh = ui::workspace::session_actions(&tab(SessionKind::Ssh), false);
        let detached_rdp = ui::workspace::session_actions(&tab(SessionKind::Rdp), true);

        assert!(detached_ssh
            .0
            .iter()
            .any(|a| matches!(a, TabAction::FitSize(id, _) if *id == instance)));
        assert!(
            !detached_ssh.0.iter().any(|a| matches!(
                a,
                TabAction::Detach(_) | TabAction::CloseOthers(_) | TabAction::Fullscreen(_)
            )),
            "独立窗口的菜单不该有标签专属或 SSH 没有的动作：{:?}",
            detached_ssh.0
        );
        assert!(main_ssh.0.iter().any(|a| matches!(a, TabAction::Detach(_))));
        assert!(main_ssh
            .0
            .iter()
            .any(|a| matches!(a, TabAction::CloseOthers(_))));
        assert!(detached_rdp
            .0
            .iter()
            .any(|a| matches!(a, TabAction::Fullscreen(_))));
        assert!(detached_rdp
            .0
            .iter()
            .any(|a| matches!(a, TabAction::Disconnect(_))));

        // 分组线只能落在真实存在的条目上（越界值渲染时会忽略，但那是兜底不是设计）。
        for (actions, groups) in [&detached_ssh, &main_ssh, &detached_rdp] {
            for group in groups {
                assert!(
                    *group < actions.len(),
                    "分组下标 {group} 超出了 {} 条动作",
                    actions.len()
                );
            }
        }
    }

    /// 连续开两次菜单只能留下一个窗口：第二个必须先收掉第一个，
    /// 否则置顶的旧菜单会一直挂在屏幕上。
    #[test]
    fn opening_a_second_menu_replaces_the_first_window() {
        let temp = TempDir::new("menu-replace");
        let mut app = App::with_paths(temp.paths());
        send(&mut app, Message::NewSession(SessionKind::Ssh));
        fill(&mut app, "菜单", "10.20.1.11", "22", "ops");
        send(&mut app, Message::ApplyDraft);
        send(&mut app, Message::OpenSelected);
        let instance = app.tabs.active_tab().expect("应有一条标签").instance;

        let _ = app.show_menu(instance, Point::new(10.0, 20.0), (0.0, 0.0));
        let first = app.menu.as_ref().expect("第一次应登记菜单窗口").window;

        let _ = app.show_menu(instance, Point::new(30.0, 40.0), (0.0, 0.0));
        let second = app.menu.as_ref().expect("第二次应登记菜单窗口").window;

        assert_ne!(first, second, "第二次开的是新窗口");
    }

    /// 独立窗口标题条的空白处右键必须打开会话菜单（那里是「画面设为 …」在
    /// 独立窗口里唯一的入口），并且不能同时被当成拖窗口。
    #[test]
    fn right_clicking_the_detached_title_bar_opens_the_session_menu() {
        let temp = TempDir::new("detached-menu");
        let mut app = App::with_paths(temp.paths());
        send(&mut app, Message::NewSession(SessionKind::Ssh));
        fill(&mut app, "菜单", "10.20.1.11", "22", "ops");
        send(&mut app, Message::ApplyDraft);
        let id = app.selected.clone().expect("新建后应处于选中态");
        send(&mut app, Message::OpenInNewWindow(id));

        let detached = app
            .windows
            .iter()
            .find_map(|(id, slot)| matches!(slot, WindowSlot::Detached(_)).then_some(*id))
            .expect("要登记一个独立窗口");

        let mut sim = iced_test::Simulator::with_size(
            iced::Settings {
                default_font: theme::ui_font(),
                ..iced::Settings::default()
            },
            Size::new(DETACHED_W, DETACHED_H),
            app.view(detached),
        );
        // 标题条中间的空白：拖拽区与菜单都归它。
        sim.point_at(Point::new(DETACHED_W / 2.0, ui::FULLSCREEN_BAR_H / 2.0));
        sim.simulate([iced::Event::Mouse(mouse::Event::ButtonPressed(
            mouse::Button::Right,
        ))]);
        let messages = sim.into_messages().collect::<Vec<_>>();

        assert!(
            messages
                .iter()
                .any(|m| matches!(m, Message::SessionMenu(..))),
            "右键独立窗口标题条没有打开菜单：{messages:?}"
        );
        assert!(
            !messages.iter().any(|m| matches!(m, Message::WindowDrag(_))),
            "右键不该同时开始拖窗口：{messages:?}"
        );
    }

    /// 会话菜单是独立窗口，尺寸必须刚好容得下面板：小了会把最后几条裁掉
    /// （这个仓库有过「零宽菜单」那次事故，尺寸与内容同源才算守住）。
    #[test]
    fn the_menu_window_is_exactly_big_enough_for_its_panel() {
        let temp = TempDir::new("menu-size");
        let mut app = App::with_paths(temp.paths());
        send(&mut app, Message::NewSession(SessionKind::Ssh));
        fill(&mut app, "菜单", "10.20.1.11", "22", "ops");
        send(&mut app, Message::ApplyDraft);
        send(&mut app, Message::OpenSelected);
        let instance = app.tabs.active_tab().expect("应有一条标签").instance;

        let _ = app.show_menu(instance, Point::new(300.0, 20.0), (100.0, 50.0));
        let menu = app.menu.as_ref().expect("菜单窗口应已登记");

        // SSH 会话在标签条上有 8 条：重新连接 + 四档画面尺寸 + 在新窗口打开 +
        // 关闭标签 + 关闭其他标签，分组线在 1、5、6 三条之前。
        assert_eq!(menu.items.len(), 8);
        assert_eq!(menu.groups, vec![1, 5, 6]);

        let size = ui::context_menu::panel_size(&menu.items, &menu.groups);

        let mut sim = iced_test::Simulator::with_size(
            iced::Settings {
                default_font: theme::ui_font(),
                ..iced::Settings::default()
            },
            size,
            app.view(menu.window),
        );
        let last = sim
            .find("关闭其他标签")
            .expect("最后一条动作要画得出来")
            .visible_bounds()
            .expect("最后一条动作在可视区内");
        // 选择器抓到的是行里的文字，它的高度是 18 的行盒（整行 30 = 18 + 上下各 6）。
        // 窗口只要比面板矮一点，这条就会被裁短甚至落到可视区外。
        assert!(
            last.height >= 17.0,
            "最后一条被窗口裁掉了：文字可视高 {}，完整行盒应为 18",
            last.height
        );
        assert!(
            last.y + last.height <= size.height + 0.5,
            "面板超出窗口：最后一条下沿 {} > 窗口高 {}",
            last.y + last.height,
            size.height
        );

        // 尺寸与条目数同源：8 行 × 30 + 3 条分隔线 × 9 + 面板内边距 8 + 描边 2。
        assert_eq!(size.height, 8.0 * 30.0 + 3.0 * 9.0 + 10.0);
    }

    /// 菜单窗口失焦 = 用户点了别处，必须自动收起；否则它会一直挂在屏幕上。
    #[test]
    fn the_menu_closes_when_it_loses_focus() {
        let temp = TempDir::new("menu-focus");
        let mut app = App::with_paths(temp.paths());
        send(&mut app, Message::NewSession(SessionKind::Ssh));
        fill(&mut app, "菜单", "10.20.1.11", "22", "ops");
        send(&mut app, Message::ApplyDraft);
        send(&mut app, Message::OpenSelected);
        let instance = app.tabs.active_tab().expect("应有一条标签").instance;

        let _ = app.show_menu(instance, Point::new(300.0, 20.0), (100.0, 50.0));
        let window = app.menu.as_ref().expect("菜单窗口应已登记").window;

        // 别的窗口失焦不算事。
        let _ = app.on_window_event(window::Id::unique(), iced::window::Event::Unfocused);
        assert!(app.menu.is_some(), "不是菜单窗口的失焦不该收掉菜单");

        let _ = app.on_window_event(window, iced::window::Event::Unfocused);
        assert!(app.menu.is_none(), "菜单窗口失焦后必须登记清除");

        // 重复一次不能 panic，也不该再登记回来。
        let _ = app.on_window_event(window, iced::window::Event::Unfocused);
        assert!(app.menu.is_none());
    }

    /// 无边框窗口把原生缩放边框一起带走了，独立窗口必须自己补上八条热区——
    /// 而且热区带的是**它自己**的窗口 id：按 `window::latest()` 猜的话，
    /// 拖独立窗口的边缩的是主窗口。
    #[test]
    fn a_detached_window_resizes_from_its_own_edges() {
        let temp = TempDir::new("detached-edges");
        let mut app = App::with_paths(temp.paths());
        send(&mut app, Message::NewSession(SessionKind::Ssh));
        fill(&mut app, "边缘", "10.20.1.11", "22", "ops");
        send(&mut app, Message::ApplyDraft);
        let id = app.selected.clone().expect("新建后应处于选中态");
        send(&mut app, Message::OpenInNewWindow(id));

        let detached = app
            .windows
            .iter()
            .find_map(|(id, slot)| matches!(slot, WindowSlot::Detached(_)).then_some(*id))
            .expect("要登记一个独立窗口");

        let mut sim = iced_test::Simulator::with_size(
            iced::Settings {
                default_font: theme::ui_font(),
                ..iced::Settings::default()
            },
            Size::new(DETACHED_W, DETACHED_H),
            app.view(detached),
        );
        // 右边缘内侧 1px：这里是内容区（独立窗口没有侧栏），只有热区能吃到的位置。
        sim.point_at(Point::new(DETACHED_W - 1.0, DETACHED_H / 2.0));
        let statuses = sim.simulate([iced::Event::Mouse(mouse::Event::ButtonPressed(
            mouse::Button::Left,
        ))]);
        let messages = sim.into_messages().collect::<Vec<_>>();

        assert!(
            statuses.contains(&EventStatus::Captured),
            "右边缘 4px 热区没吃到这次按下：{statuses:?}"
        );
        assert!(
            messages.iter().any(
                |m| matches!(m, Message::WindowResize(w, window::Direction::East) if *w == detached)
            ),
            "独立窗口的边缘没有对它自己发起缩放：{messages:?}"
        );
    }

    /// 独立窗口：建运行实例并登记窗口，但**不进**主窗口的标签条。
    /// 关掉窗口要顺手把连接收掉，不能让会话一直挂着。
    #[test]
    fn opening_in_a_new_window_spawns_its_own_session_without_a_tab() {
        let temp = TempDir::new("detached");
        let mut app = App::with_paths(temp.paths());

        send(&mut app, Message::NewSession(SessionKind::Ssh));
        fill(&mut app, "Web-01", "10.20.1.11", "22", "ops");
        send(&mut app, Message::ApplyDraft);
        let id = app.selected.clone().expect("新建后应处于选中态");

        send(&mut app, Message::OpenInNewWindow(id));

        assert_eq!(app.tabs.len(), 0, "独立窗口不进主窗口的标签条");
        assert_eq!(app.terminals.len(), 1, "但运行实例要建起来");
        let (window, slot) = app.windows.iter().next().expect("要登记一个窗口");
        let WindowSlot::Detached(tab) = slot else {
            panic!("登记的应该是独立窗口");
        };
        assert_eq!(tab.title(), "Web-01");

        let window = *window;
        send(&mut app, Message::WindowClosed(window));

        assert!(app.windows.is_empty(), "关窗要摘掉登记");
        assert!(
            app.terminals.is_empty(),
            "关窗要顺手断掉连接，不能让它一直挂着"
        );
    }

    /// 任务栏 / Alt-Tab 用的是运行时窗口图标，和界面里的品牌标共用同一份像素。
    /// 圆角必须是透明的：图标带白底会在深色任务栏上变成一块白方块。
    #[test]
    fn the_window_icon_uses_the_same_transparent_logo_pixels() {
        let icon = app_window_icon().expect("窗口图标应当能从上送的 RGBA 构造出来");
        let (rgba, size) = icon.into_raw();

        assert_eq!(size.width, superremote_win::icon_rgba::ICON_SIZE);
        assert_eq!(size.height, superremote_win::icon_rgba::ICON_SIZE);
        assert_eq!(
            rgba.len(),
            (size.width * size.height * 4) as usize,
            "RGBA 字节数必须和尺寸对得上"
        );
        assert_eq!(rgba[3], 0, "左上角圆角应当透明，不能带白底");
    }

    #[test]
    fn activex_window_title_status_is_parsed() {
        assert_eq!(
            App::activex_status("SUPERREMOTE|r7|connected|", "r7"),
            Some(("connected", ""))
        );
        assert_eq!(
            App::activex_status("SUPERREMOTE|r7|failed|连接超时", "r7"),
            Some(("failed", "连接超时"))
        );
        // 共享宿主里别的会话窗口不能被当成本标签的状态。
        assert_eq!(
            App::activex_status("SUPERREMOTE|r8|failed|别的会话", "r7"),
            None
        );
        // 旧宿主没有会话段，升级期间仍然可用。
        assert_eq!(
            App::activex_status("SUPERREMOTE|connected|", "r7"),
            Some(("connected", ""))
        );
        assert_eq!(App::activex_status("not ours", "r7"), None);
    }
}
