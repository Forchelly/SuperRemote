//! 界面文案：中英双语。
//!
//! 用「结构体字段」而不是 `match key` 查表：字段少一个会在编译期报错，
//! 查表方案漏一条只会在运行时显示成空白。
//!
//! 带数字的句子写成方法而不是模板：中英文的语序和单复数规则不同，
//! 把 `{n}` 塞进同一个模板里，两边总有一边读起来是翻译腔。

use std::sync::atomic::{AtomicU8, Ordering};

/// 界面语言。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Lang {
    #[default]
    Zh,
    En,
}

impl Lang {
    pub fn code(self) -> &'static str {
        match self {
            Self::Zh => "zh",
            Self::En => "en",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        match code {
            "zh" => Some(Self::Zh),
            "en" => Some(Self::En),
            _ => None,
        }
    }

    /// 下拉框里的自称：语言名永远用它自己的语言写，换错语言也能找回来。
    pub fn label(self) -> &'static str {
        match self {
            Self::Zh => "中文",
            Self::En => "English",
        }
    }

    pub fn t(self) -> &'static T {
        match self {
            Self::Zh => &ZH,
            Self::En => &EN,
        }
    }

    pub fn n_sessions(self, n: usize) -> String {
        match self {
            Self::Zh => format!("{n} 条连接"),
            Self::En => plural(n, "session", "sessions"),
        }
    }

    pub fn n_tabs(self, n: usize) -> String {
        match self {
            Self::Zh => format!("{n} 个标签"),
            Self::En => plural(n, "tab", "tabs"),
        }
    }

    pub fn n_secrets(self, n: usize) -> String {
        match self {
            Self::Zh => format!("{n} 条凭据已加密"),
            Self::En => format!("{} encrypted", plural(n, "secret", "secrets")),
        }
    }

    pub fn min_password(self, n: usize) -> String {
        match self {
            Self::Zh => format!("主口令至少 {n} 个字符"),
            Self::En => format!("Master password needs at least {n} characters"),
        }
    }

    pub fn connecting_to(self, address: &str) -> String {
        match self {
            Self::Zh => format!("正在与 {address} 建立会话，几秒内会显示远端画面。"),
            Self::En => format!("Connecting to {address}. The remote screen shows up in a moment."),
        }
    }

    pub fn inherit_with(self, default: &str) -> String {
        match self {
            Self::Zh => format!("继承默认（{default}）"),
            Self::En => format!("Use default ({default})"),
        }
    }

    /// 与 [`inherit_with`](Self::inherit_with) 是同一件事的两面：单条连接叫「继承」，
    /// 全局默认叫「用内置值」。合成一句会让设置页读起来像在改别人的连接。
    pub fn builtin_with(self, fallback: &str) -> String {
        match self {
            Self::Zh => format!("内置 · {fallback}"),
            Self::En => format!("Built-in · {fallback}"),
        }
    }

    /// 字体下拉里那个「配置存着、但系统枚举没给出」的选项。
    ///
    /// 只说「自定义」而不说「未安装」：这个名字既可能真的没装，也可能装了但
    /// 不是等宽（列表只收等宽），后者被写成前者就是说谎。
    pub fn font_unlisted(self, name: &str) -> String {
        match self {
            Self::Zh => format!("{name}（自定义）"),
            Self::En => format!("{name} (custom)"),
        }
    }

    pub fn protocol_port(self, protocol: &str, port: &str) -> String {
        match self {
            Self::Zh => format!("{protocol} · 端口 {port}"),
            Self::En => format!("{protocol} · port {port}"),
        }
    }

    pub fn received(self, bytes: &str) -> String {
        match self {
            Self::Zh => format!("已接收 {bytes}"),
            Self::En => format!("{bytes} received"),
        }
    }

    /// 菜单里的一档画面尺寸。中英都是「词 + 数字」，语序一致，所以不必各写一份
    /// 模板；数字是调用方给的档位，不是拼出来的句子。
    pub fn pane_size(self, width: u32, height: u32) -> String {
        match self {
            Self::Zh => format!("画面 {width}×{height}"),
            Self::En => format!("Pane {width}×{height}"),
        }
    }

    /// 终端翻历史时右上角的浮层。
    pub fn scroll_lines(self, n: usize) -> String {
        match self {
            Self::Zh => format!("↑ {n} 行"),
            Self::En => format!("↑ {n} lines"),
        }
    }

    pub fn exited_with(self, code: i32) -> String {
        match self {
            Self::Zh => format!("会话已结束（退出码 {code}）"),
            Self::En => format!("Session ended (exit code {code})"),
        }
    }

    pub fn rdp_credentials_cleared(self, host: &str) -> String {
        match self {
            Self::Zh => format!("已清除系统凭据管理器里的 TERMSRV/{host}"),
            Self::En => format!("Cleared TERMSRV/{host} from Windows Credential Manager"),
        }
    }

    pub fn config_restored(self, reason: &str, backup: &str) -> String {
        match self {
            Self::Zh => format!("数据文件读不了（{reason}），原件已存为 {backup}，本次从空配置开始"),
            Self::En => format!(
                "Config unreadable ({reason}). Original kept as {backup}; started from an empty config"
            ),
        }
    }

    pub fn open_dir_failed(self, err: &str) -> String {
        match self {
            Self::Zh => format!("无法打开数据目录：{err}"),
            Self::En => format!("Could not open the data folder: {err}"),
        }
    }

    pub fn save_failed(self, err: &str) -> String {
        match self {
            Self::Zh => format!("保存失败：{err}"),
            Self::En => format!("Save failed: {err}"),
        }
    }

    pub fn data_dir_change_failed(self, err: &str) -> String {
        match self {
            Self::Zh => format!("无法更改数据目录：{err}"),
            Self::En => format!("Could not change the data folder: {err}"),
        }
    }

    /// 旧版 AppData 数据自动复制到程序目录后的提示。
    pub fn data_moved(self, to: &str) -> String {
        match self {
            Self::Zh => format!("已把数据迁移到 {to}，旧目录保留作备份"),
            Self::En => format!("Data moved to {to}; the old folder was kept as a backup"),
        }
    }

    /// 新建条目的默认名：中文是「新建目录」，英文是「New folder」，
    /// 语序不同，不能靠拼接。
    pub fn new_folder_name(self) -> &'static str {
        match self {
            Self::Zh => "新建目录",
            Self::En => "New folder",
        }
    }

    pub fn new_session_name(self, protocol: &str) -> String {
        match self {
            Self::Zh => format!("新建 {protocol}"),
            Self::En => format!("New {protocol}"),
        }
    }

    pub fn window_title(self, page: Option<&str>) -> String {
        match (self, page) {
            (Self::Zh, Some(page)) => format!("{page} - SuperRemote"),
            (Self::Zh, None) => "SuperRemote - 运维连接管理".to_owned(),
            (Self::En, Some(page)) => format!("{page} - SuperRemote"),
            (Self::En, None) => "SuperRemote - Remote Connection Manager".to_owned(),
        }
    }
}

// ─────────────────────── 当前语言（进程级） ───────────────────────
//
// 与主题同理：视图层有几十处要取文案，顺着签名传一个 `Lang` 只会让每个
// 函数多一个参数。启动时按配置写入，设置页切换时改写。

static LANG: AtomicU8 = AtomicU8::new(0);

pub fn set_lang(lang: Lang) {
    LANG.store(
        match lang {
            Lang::Zh => 0,
            Lang::En => 1,
        },
        Ordering::Relaxed,
    );
}

pub fn lang() -> Lang {
    if LANG.load(Ordering::Relaxed) == 1 {
        Lang::En
    } else {
        Lang::Zh
    }
}

/// 当前语言的词表。视图里写 `i18n::t().action_save` 即可。
pub fn t() -> &'static T {
    lang().t()
}

fn plural(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

/// 一份语言的完整文案表。
pub struct T {
    // ── 侧栏 ────────────────────────────────────────────────
    pub nav_sessions: &'static str,
    pub nav_settings: &'static str,
    pub search_placeholder: &'static str,
    pub tree_empty: &'static str,
    pub tree_locked: &'static str,
    pub tree_no_match: &'static str,
    /// 会话树右键菜单的动作名。
    pub menu_open: &'static str,
    pub menu_open_background: &'static str,
    /// 在独立窗口里打开。
    pub menu_open_new_window: &'static str,
    pub menu_rename: &'static str,
    pub menu_copy_address: &'static str,
    pub menu_delete: &'static str,
    pub menu_new_directory: &'static str,
    pub menu_new_ssh: &'static str,
    pub menu_new_rdp: &'static str,
    pub menu_new_telnet: &'static str,
    pub menu_cut: &'static str,
    pub menu_copy: &'static str,
    pub menu_paste: &'static str,
    /// 连接列表页的标题与图标提示。
    pub nav_list: &'static str,
    pub tooltip_minimize: &'static str,
    pub tooltip_maximize: &'static str,
    /// 窗口已经最大化时按钮的文字（图标同时换成「还原」图形）。
    pub tooltip_restore: &'static str,
    pub tooltip_close_window: &'static str,
    pub tooltip_new_tab: &'static str,
    pub tooltip_dismiss: &'static str,
    pub tooltip_hold_password: &'static str,
    /// 三态复选框的当前状态与下一击会切到什么。方框里的「–」不是未勾选，
    /// 不解释一句的话，用户会以为是禁用或者半选。
    pub check_inherit: &'static str,
    pub check_on: &'static str,
    pub check_off: &'static str,
    pub err_invalid_paste: &'static str,
    /// 标签条右键菜单。
    pub menu_close_tab: &'static str,
    pub menu_close_others: &'static str,
    /// 拖拽落点非法（比如把目录拖进它自己下面）。
    pub err_invalid_drop: &'static str,

    // ── 标签条 ──────────────────────────────────────────────
    pub tab_details: &'static str,
    pub close_tab: &'static str,
    pub reconnect: &'static str,

    // ── 连接状态 ────────────────────────────────────────────
    pub status_live: &'static str,
    pub status_connecting: &'static str,
    pub status_closed: &'static str,
    pub status_failed: &'static str,

    // ── 会话面板 ────────────────────────────────────────────
    pub session_missing: &'static str,
    pub session_missing_hint: &'static str,
    pub connecting: &'static str,
    pub connect_failed: &'static str,
    pub session_closed: &'static str,
    pub session_ended: &'static str,

    // ── 远控面板 ────────────────────────────────────────────
    pub rdp_title: &'static str,
    pub rdp_hint: &'static str,
    pub field_account: &'static str,
    pub account_unspecified: &'static str,
    pub field_credential: &'static str,
    pub credential_saved: &'static str,
    pub credential_unsaved: &'static str,
    pub field_clipboard: &'static str,
    pub clipboard_enabled: &'static str,
    pub rdp_open: &'static str,
    pub rdp_forget_credentials: &'static str,
    pub rdp_waiting: &'static str,
    pub rdp_disconnect: &'static str,
    pub rdp_fullscreen: &'static str,
    pub exit_fullscreen: &'static str,
    pub rdp_window_missing: &'static str,
    pub rdp_settings_title: &'static str,
    pub rdp_reconnect_hint: &'static str,
    pub rdp_performance: &'static str,
    pub rdp_color_depth: &'static str,
    pub rdp_audio: &'static str,
    pub rdp_audio_capture: &'static str,
    pub rdp_keyboard: &'static str,
    pub rdp_graphics: &'static str,
    pub rdp_graphics_compat: &'static str,
    pub rdp_graphics_modern: &'static str,
    pub rdp_desktop_size: &'static str,
    pub rdp_host_mode: &'static str,
    pub rdp_host_shared: &'static str,
    pub rdp_host_isolated: &'static str,
    pub rdp_redirects: &'static str,
    pub rdp_redirect_printers: &'static str,
    pub rdp_redirect_clipboard: &'static str,
    pub rdp_redirect_smartcards: &'static str,
    pub rdp_redirect_ports: &'static str,
    pub rdp_redirect_drives: &'static str,
    pub rdp_tuning_ok: &'static str,
    pub rdp_tuning_title: &'static str,
    pub rdp_tuning_hint: &'static str,
    pub rdp_tuning_fix: &'static str,
    pub rdp_tuning_pending: &'static str,
    pub rdp_tuning_fixed: &'static str,
    pub rdp_tuning_failed: &'static str,
    pub uac_desktop_title: &'static str,
    pub uac_desktop_on: &'static str,
    pub uac_desktop_off: &'static str,
    pub uac_desktop_disable: &'static str,
    pub uac_desktop_enable: &'static str,
    pub uac_desktop_warn: &'static str,
    pub uac_desktop_pending: &'static str,
    pub uac_desktop_applied: &'static str,
    pub uac_desktop_failed: &'static str,
    pub toggle_on: &'static str,
    pub toggle_off: &'static str,
    pub follow_window: &'static str,
    pub desktop_fullscreen: &'static str,
    pub perf_auto: &'static str,
    pub perf_modem: &'static str,
    pub perf_broadband_low: &'static str,
    pub perf_satellite: &'static str,
    pub perf_broadband_high: &'static str,
    pub perf_wan: &'static str,
    pub perf_lan: &'static str,
    pub audio_local: &'static str,
    pub audio_remote: &'static str,
    pub audio_none: &'static str,
    pub keyboard_local: &'static str,
    pub keyboard_remote: &'static str,
    pub keyboard_fullscreen: &'static str,
    pub color_15: &'static str,
    pub color_16: &'static str,
    pub color_24: &'static str,
    pub color_32: &'static str,
    pub rdp_reset: &'static str,
    pub rdp_set_default: &'static str,
    pub info_rdp_settings_reset: &'static str,
    pub info_rdp_defaults_saved: &'static str,
    pub terminal_settings_title: &'static str,
    pub terminal_font_family: &'static str,
    pub terminal_font_size: &'static str,
    pub terminal_cursor: &'static str,
    pub terminal_backspace: &'static str,
    pub terminal_home_end: &'static str,
    pub terminal_color_scheme: &'static str,
    pub terminal_cursor_block: &'static str,
    pub terminal_cursor_underline: &'static str,
    pub terminal_cursor_beam: &'static str,
    pub terminal_backspace_del: &'static str,
    pub terminal_backspace_bs: &'static str,
    pub terminal_home_end_standard: &'static str,
    pub terminal_home_end_rxvt: &'static str,
    pub terminal_scheme_default: &'static str,
    pub terminal_scheme_solarized_dark: &'static str,
    pub terminal_scheme_solarized_light: &'static str,
    pub terminal_scheme_linux: &'static str,
    pub terminal_scheme_xterm: &'static str,
    pub terminal_charset: &'static str,
    pub terminal_mouse: &'static str,
    pub terminal_keypad: &'static str,
    pub charset_utf8: &'static str,
    pub charset_gbk: &'static str,
    pub charset_big5: &'static str,
    pub charset_latin1: &'static str,
    pub mouse_hybrid: &'static str,
    pub mouse_xterm: &'static str,
    pub mouse_windows: &'static str,
    pub keypad_standard: &'static str,
    pub keypad_escn: &'static str,
    pub keypad_linux: &'static str,
    pub info_terminal_settings_reset: &'static str,
    pub info_terminal_defaults_saved: &'static str,
    pub info_copied: &'static str,
    pub private_key_title: &'static str,
    pub private_key_placeholder: &'static str,
    pub clear_private_key: &'static str,
    pub private_key_saved: &'static str,
    pub private_key_unsaved: &'static str,
    pub info_private_key_cleared: &'static str,

    // ── 编辑器 ──────────────────────────────────────────────
    pub section_directory: &'static str,
    pub section_connection: &'static str,
    pub field_name: &'static str,
    pub placeholder_directory_name: &'static str,
    pub placeholder_session_name: &'static str,
    pub field_host: &'static str,
    pub placeholder_host: &'static str,
    pub field_username: &'static str,
    pub placeholder_username: &'static str,
    pub field_password: &'static str,
    pub field_note: &'static str,
    pub placeholder_note: &'static str,
    pub placeholder_password: &'static str,
    pub placeholder_password_plain: &'static str,
    pub reveal_password: &'static str,
    pub clear_password: &'static str,
    pub action_save: &'static str,
    pub action_open: &'static str,
    pub action_delete_directory: &'static str,
    pub action_delete_session: &'static str,
    pub empty_title: &'static str,
    pub empty_hint: &'static str,
    pub empty_storage_note: &'static str,

    // ── 设置页 ──────────────────────────────────────────────
    pub settings_title: &'static str,
    pub settings_subtitle: &'static str,
    pub appearance_title: &'static str,
    pub appearance_hint: &'static str,
    pub theme_system: &'static str,
    pub theme_light: &'static str,
    pub theme_dark: &'static str,
    pub language_title: &'static str,
    pub language_hint: &'static str,
    pub data_dir_title: &'static str,
    pub data_dir_hint: &'static str,
    pub reveal_dir: &'static str,
    pub change_dir: &'static str,
    pub use_default_dir: &'static str,
    pub choose_data_dir: &'static str,
    pub data_dir_changed: &'static str,
    pub data_dir_env_locked: &'static str,
    /// 「+」新建连接标签页。
    pub new_tab_title: &'static str,
    /// 全局默认设置（RDP / 终端）。
    pub defaults_rdp_title: &'static str,
    pub defaults_rdp_hint: &'static str,
    pub defaults_terminal_title: &'static str,
    pub defaults_terminal_hint: &'static str,
    pub defaults_restore: &'static str,
    /// 首选打开方式。
    pub open_mode: &'static str,
    pub open_mode_tab: &'static str,
    pub open_mode_background: &'static str,
    pub open_mode_window: &'static str,
    pub defaults_open_title: &'static str,
    pub defaults_open_hint: &'static str,
    pub info_rdp_defaults_reset: &'static str,
    pub info_terminal_defaults_reset: &'static str,
    pub security_title: &'static str,
    pub security_hint_password: &'static str,
    pub security_hint_os: &'static str,
    pub mode_master_password: &'static str,
    pub mode_os_keychain: &'static str,
    pub startup_needs_password: &'static str,
    pub startup_no_password: &'static str,
    pub field_new_password: &'static str,
    pub placeholder_min_password: &'static str,
    pub change_password: &'static str,
    pub enable_password: &'static str,
    pub disable_password: &'static str,

    // ── 解锁页 ──────────────────────────────────────────────
    pub unlock_hint: &'static str,
    pub unlock_password: &'static str,
    pub unlock_action: &'static str,
    pub unlock_wrong: &'static str,
    pub fatal_title: &'static str,
    pub fatal_hint: &'static str,

    // ── 提示条 ──────────────────────────────────────────────
    pub err_empty_name: &'static str,
    pub err_empty_host: &'static str,
    pub err_bad_port: &'static str,
    pub err_node_gone: &'static str,
    pub info_saved: &'static str,
    pub info_password_cleared: &'static str,
    pub err_directory_not_openable: &'static str,
    pub info_password_changed: &'static str,
    pub info_password_enabled: &'static str,
    pub info_password_disabled: &'static str,
    pub err_selection_gone: &'static str,
}

pub static ZH: T = T {
    nav_sessions: "会话",
    nav_settings: "设置",
    search_placeholder: "搜索名称、地址或账号",
    tree_empty: "还没有会话。在这里右键即可新建目录或连接。",
    tree_locked: "数据尚未解锁",
    tree_no_match: "没有匹配的会话",
    menu_open: "打开",
    menu_open_background: "在后台打开",
    menu_open_new_window: "在新窗口打开",
    menu_rename: "重命名",
    menu_copy_address: "复制地址",
    menu_delete: "删除",
    menu_new_directory: "新建目录",
    menu_new_ssh: "新建 SSH",
    menu_new_rdp: "新建 RDP",
    menu_new_telnet: "新建 Telnet",
    menu_cut: "剪切",
    menu_copy: "复制",
    menu_paste: "粘贴",
    nav_list: "连接列表",
    tooltip_minimize: "最小化",
    tooltip_maximize: "最大化",
    tooltip_restore: "还原",
    tooltip_close_window: "关闭",
    tooltip_new_tab: "新建标签页",
    tooltip_dismiss: "关闭提示",
    tooltip_hold_password: "按住显示密码",
    check_inherit: "继承默认 · 点击切换为开",
    check_on: "已开启 · 点击切换为关",
    check_off: "已关闭 · 点击切换为继承",
    err_invalid_paste: "不能把文件夹粘贴到它自己的子目录里",
    menu_close_tab: "关闭标签",
    menu_close_others: "关闭其他标签",
    err_invalid_drop: "不能移到这里：目录不能搬进它自己下面",

    tab_details: "主页",
    close_tab: "关闭",
    reconnect: "重新连接",

    status_live: "已连接",
    status_connecting: "连接中",
    status_closed: "已断开",
    status_failed: "连接失败",

    session_missing: "会话尚未建立",
    session_missing_hint: "选中左侧条目后按「打开连接」，或直接用这里的重新连接按钮。",
    connecting: "正在连接…",
    connect_failed: "连接失败",
    session_closed: "会话已断开",
    session_ended: "会话已结束",

    rdp_title: "远程桌面",
    rdp_hint: "远程画面会嵌入当前标签。地址或凭据改动后，重新连接即可生效。",
    field_account: "账号",
    account_unspecified: "未指定（连接时询问）",
    field_credential: "凭据",
    credential_saved: "已加密保存，连接时自动交给系统凭据管理器",
    credential_unsaved: "未保存，连接时手动输入",
    field_clipboard: "剪贴板",
    clipboard_enabled: "已启用双向重定向",
    rdp_open: "连接远程桌面",
    rdp_forget_credentials: "清除系统凭据",
    rdp_waiting: "正在建立远程桌面会话",
    rdp_disconnect: "断开",
    rdp_fullscreen: "全屏",
    exit_fullscreen: "退出全屏",
    rdp_window_missing: "拿不到主窗口句柄，无法内嵌远程桌面",
    rdp_settings_title: "远程连接设置",
    rdp_reconnect_hint: "修改不会打断当前会话，点「重新连接」后生效。",
    rdp_performance: "性能",
    rdp_color_depth: "颜色",
    rdp_audio: "音频",
    rdp_audio_capture: "录音",
    rdp_keyboard: "组合键",
    rdp_graphics: "图形管线",
    rdp_graphics_compat: "兼容（旧版控件，RDP 8.1）",
    rdp_graphics_modern: "现代（新版控件，RDP 10 + AVC）",
    rdp_desktop_size: "分辨率",
    rdp_host_mode: "宿主进程",
    rdp_host_shared: "共用宿主（省内存）",
    rdp_host_isolated: "独立宿主（崩溃隔离）",
    rdp_redirects: "重定向",
    rdp_redirect_printers: "打印机",
    rdp_redirect_clipboard: "剪贴板",
    rdp_redirect_smartcards: "智能卡",
    rdp_redirect_ports: "端口",
    rdp_redirect_drives: "驱动器",
    rdp_tuning_ok: "系统 RDP 优化：正常",
    rdp_tuning_title: "系统 RDP 优化检测",
    rdp_tuning_hint: "检测到非默认的 AVC 4:4:4 或「仅 UDP」RDP 设置。恢复默认只修改这两项，其他设置不变。",
    rdp_tuning_fix: "修复系统设置",
    rdp_tuning_pending: "已请求管理员权限，请在 UAC 确认…",
    rdp_tuning_fixed: "已恢复默认，请断开并重新连接 RDP 会话",
    rdp_tuning_failed: "未能恢复（需要管理员权限，或 UAC 被取消）",
    uac_desktop_title: "UAC 安全桌面",
    uac_desktop_on: "已开启 — 提权提示在独立的安全桌面中显示",
    uac_desktop_off: "已关闭 — UAC 直接在当前桌面弹出，不再切桌面",
    uac_desktop_disable: "关闭安全桌面",
    uac_desktop_enable: "恢复开启",
    uac_desktop_warn: "关闭后提权提示不再运行在隔离桌面上，普通程序理论上可以干扰它；是否提权仍由你确认。",
    uac_desktop_pending: "已请求管理员权限，请在 UAC 确认…",
    uac_desktop_applied: "已应用，下次弹 UAC 生效",
    uac_desktop_failed: "未能修改（需要管理员权限，或 UAC 被取消）",
    toggle_on: "开",
    toggle_off: "关",
    follow_window: "跟随窗口",
    desktop_fullscreen: "全屏（主显示器）",
    perf_auto: "自动检测",
    perf_modem: "调制解调器（56 Kbps）",
    perf_broadband_low: "低速宽带（256 Kbps–2 Mbps）",
    perf_satellite: "卫星（高延迟）",
    perf_broadband_high: "高速宽带（2–10 Mbps）",
    perf_wan: "广域网（高延迟）",
    perf_lan: "局域网（10 Mbps+）",
    audio_local: "在本机播放",
    audio_remote: "在远端播放",
    audio_none: "不播放",
    keyboard_local: "仅在本机应用",
    keyboard_remote: "发送到远端",
    keyboard_fullscreen: "仅全屏",
    color_15: "15 位",
    color_16: "16 位",
    color_24: "24 位",
    color_32: "32 位",
    rdp_reset: "重置为继承",
    rdp_set_default: "设为默认",
    info_rdp_settings_reset: "已清空本连接的 RDP 覆盖，全部继承默认",
    info_rdp_defaults_saved: "已把当前生效设置设为 RDP 全局默认",
    terminal_settings_title: "终端设置",
    terminal_font_family: "字体",
    terminal_font_size: "字号",
    terminal_cursor: "光标",
    terminal_backspace: "回退键",
    terminal_home_end: "Home / End",
    terminal_color_scheme: "配色",
    terminal_cursor_block: "方块",
    terminal_cursor_underline: "下划线",
    terminal_cursor_beam: "竖线",
    terminal_backspace_del: "DEL",
    terminal_backspace_bs: "Backspace",
    terminal_home_end_standard: "标准",
    terminal_home_end_rxvt: "rxvt",
    terminal_scheme_default: "默认深色",
    terminal_scheme_solarized_dark: "Solarized Dark",
    terminal_scheme_solarized_light: "Solarized Light",
    terminal_scheme_linux: "Linux 控制台",
    terminal_scheme_xterm: "xterm 白底",
    terminal_charset: "字符集",
    terminal_mouse: "鼠标动作",
    terminal_keypad: "功能键",
    charset_utf8: "UTF-8",
    charset_gbk: "GBK",
    charset_big5: "Big5",
    charset_latin1: "Latin-1",
    mouse_hybrid: "混合",
    mouse_xterm: "xterm",
    mouse_windows: "Windows",
    keypad_standard: "标准",
    keypad_escn: "ESC[n~",
    keypad_linux: "Linux",
    info_terminal_settings_reset: "已清空本连接的终端覆盖，全部继承默认",
    info_terminal_defaults_saved: "已把当前生效设置设为终端全局默认",
    info_copied: "已复制到系统剪贴板",
    private_key_title: "私钥",
    private_key_placeholder: "-----BEGIN OPENSSH PRIVATE KEY-----",
    clear_private_key: "清除已存私钥",
    private_key_saved: "已保存私钥",
    private_key_unsaved: "未保存私钥",
    info_private_key_cleared: "已清除该连接的私钥",

    section_directory: "目录",
    section_connection: "身份认证设置",
    field_name: "名称",
    placeholder_directory_name: "目录名称",
    placeholder_session_name: "连接名称",
    field_host: "地址",
    placeholder_host: "host.example.com",
    field_username: "用户名",
    placeholder_username: "留空表示连接时询问",
    field_password: "密码",
    field_note: "描述",
    placeholder_note: "可选，仅本机可见",
    placeholder_password: "留空表示不修改已有凭据",
    placeholder_password_plain: "明文显示中",
    reveal_password: "显示口令",
    clear_password: "清除已存口令",
    action_save: "保存",
    action_open: "打开连接",
    action_delete_directory: "删除目录",
    action_delete_session: "删除连接",
    empty_title: "还没有选中任何条目",
    empty_hint: "先从左侧选一条连接，或者直接新建：",
    empty_storage_note: "配置保存在数据目录（默认是程序目录下的 data），口令单独加密存放",

    settings_title: "设置",
    settings_subtitle: "外观、语言、连接默认、数据目录与凭据加密",
    appearance_title: "外观",
    appearance_hint: "「跟随系统」在启动时读取 Windows 的亮暗设置。",
    theme_system: "跟随系统",
    theme_light: "亮色",
    theme_dark: "暗色",
    language_title: "语言",
    language_hint: "切换后立即生效，并写入配置文件。",
    data_dir_title: "数据目录",
    data_dir_hint: "配置与会话存放在这里，默认在程序目录的 data 下；更改时复制数据，旧目录保留备份。",
    reveal_dir: "打开目录",
    change_dir: "更改目录…",
    use_default_dir: "恢复默认",
    choose_data_dir: "选择数据目录",
    data_dir_changed: "数据目录已更改",
    data_dir_env_locked: "数据目录由 SUPERREMOTE_DATA_DIR 指定，界面里不能更改",
    new_tab_title: "新建连接",
    defaults_rdp_title: "远程桌面默认",
    defaults_rdp_hint: "新建连接与「继承默认」的连接都从这里取值；改动不影响已打开的连接。",
    defaults_terminal_title: "终端默认",
    defaults_terminal_hint: "SSH / Telnet 的字体、光标与按键行为默认值；重新连接后生效。",
    defaults_restore: "恢复内置默认",
    open_mode: "首选",
    open_mode_tab: "选项卡",
    open_mode_background: "后台选项卡",
    open_mode_window: "独立窗口",
    defaults_open_title: "打开方式默认",
    defaults_open_hint: "没单独设过的连接都按这个方式打开；菜单里的三项仍然是显式指定。",
    info_rdp_defaults_reset: "已清空远程桌面的全局默认，全部回到内置值",
    info_terminal_defaults_reset: "已清空终端的全局默认，全部回到内置值",
    security_title: "凭据加密",
    security_hint_password: "凭据用主口令派生的密钥加密，启动时必须输入口令。忘记口令数据不可恢复。",
    security_hint_os: "凭据用绑定当前 Windows 账户的系统密钥库加密，启动不问口令；别人拷走文件读不懂，但以你的身份运行的程序可以读。",
    mode_master_password: "主口令",
    mode_os_keychain: "系统密钥库",
    startup_needs_password: "启动需要口令",
    startup_no_password: "启动不问口令",
    field_new_password: "新主口令",
    placeholder_min_password: "至少 6 个字符",
    change_password: "更换口令",
    enable_password: "启用主口令",
    disable_password: "取消主口令",

    unlock_hint: "凭据已用主口令加密，请输入口令解锁",
    unlock_password: "主口令",
    unlock_action: "解锁",
    unlock_wrong: "口令不正确",
    fatal_title: "无法打开数据目录",
    fatal_hint: "修好后重新启动；若配置文件来自更新版本的程序，请勿降级打开。",

    err_empty_name: "名称不能为空",
    err_empty_host: "主机地址不能为空",
    err_bad_port: "端口必须是 0–65535 的整数",
    err_node_gone: "该条目已不存在，请重新选择",
    info_saved: "已保存",
    info_password_cleared: "已清除该凭据",
    err_directory_not_openable: "目录不能直接打开，请选择其中的连接",
    info_password_changed: "已更换主口令，只重包了数据密钥，凭据未重新加密",
    info_password_enabled: "已启用主口令，下次启动需要输入",
    info_password_disabled: "已改回系统密钥库，启动不再询问口令",
    err_selection_gone: "该条目已不存在",
};

pub static EN: T = T {
    nav_sessions: "Sessions",
    nav_settings: "Settings",
    search_placeholder: "Search name, address or account",
    tree_empty: "No sessions yet. Right-click here to add a folder or connection.",
    tree_locked: "Data is locked",
    tree_no_match: "No matching sessions",
    menu_open: "Open",
    menu_open_background: "Open in background",
    menu_open_new_window: "Open in new window",
    menu_rename: "Rename",
    menu_copy_address: "Copy address",
    menu_delete: "Delete",
    menu_new_directory: "New folder",
    menu_new_ssh: "New SSH",
    menu_new_rdp: "New RDP",
    menu_new_telnet: "New Telnet",
    menu_cut: "Cut",
    menu_copy: "Copy",
    menu_paste: "Paste",
    nav_list: "Connections",
    tooltip_minimize: "Minimize",
    tooltip_maximize: "Maximize",
    tooltip_restore: "Restore",
    tooltip_close_window: "Close",
    tooltip_new_tab: "New tab",
    tooltip_dismiss: "Dismiss",
    tooltip_hold_password: "Hold to reveal password",
    check_inherit: "Inherit default · click for on",
    check_on: "On · click for off",
    check_off: "Off · click to inherit",
    err_invalid_paste: "A folder cannot be pasted inside itself",
    menu_close_tab: "Close tab",
    menu_close_others: "Close other tabs",
    err_invalid_drop: "Cannot move there: a folder cannot go inside itself",

    tab_details: "Home",
    close_tab: "Close",
    reconnect: "Reconnect",

    status_live: "Connected",
    status_connecting: "Connecting",
    status_closed: "Disconnected",
    status_failed: "Failed",

    session_missing: "Session not started",
    session_missing_hint: "Select an item and press Open, or use Reconnect here.",
    connecting: "Connecting…",
    connect_failed: "Connection failed",
    session_closed: "Session closed",
    session_ended: "Session ended",

    rdp_title: "Remote Desktop",
    rdp_hint: "The remote screen is embedded in this tab. Reconnect after changing the address or credentials.",
    field_account: "Account",
    account_unspecified: "Not set (asked on connect)",
    field_credential: "Credential",
    credential_saved: "Encrypted and handed to Windows Credential Manager",
    credential_unsaved: "Not saved; you will be asked on connect",
    field_clipboard: "Clipboard",
    clipboard_enabled: "Two-way redirection on",
    rdp_open: "Connect",
    rdp_forget_credentials: "Clear credentials",
    rdp_waiting: "Connecting to the remote desktop",
    rdp_disconnect: "Disconnect",
    rdp_fullscreen: "Full screen",
    exit_fullscreen: "Exit full screen",
    rdp_window_missing: "Cannot embed Remote Desktop without the main window handle",
    rdp_settings_title: "Remote connection settings",
    rdp_reconnect_hint: "Changes do not interrupt the current session; press Reconnect to apply.",
    rdp_performance: "Performance",
    rdp_color_depth: "Color",
    rdp_audio: "Audio",
    rdp_audio_capture: "Recording",
    rdp_keyboard: "Keyboard",
    rdp_graphics: "Graphics pipeline",
    rdp_graphics_compat: "Compatible (older control, RDP 8.1)",
    rdp_graphics_modern: "Modern (newer control, RDP 10 + AVC)",
    rdp_desktop_size: "Resolution",
    rdp_host_mode: "Host process",
    rdp_host_shared: "Shared (less memory)",
    rdp_host_isolated: "Isolated (crash-safe)",
    rdp_redirects: "Redirection",
    rdp_redirect_printers: "Printers",
    rdp_redirect_clipboard: "Clipboard",
    rdp_redirect_smartcards: "Smart cards",
    rdp_redirect_ports: "Ports",
    rdp_redirect_drives: "Drives",
    rdp_tuning_ok: "System RDP tuning: normal",
    rdp_tuning_title: "System RDP tuning",
    rdp_tuning_hint: "Non-default AVC 4:4:4 or UDP-only RDP settings detected. Restoring defaults changes only these two settings.",
    rdp_tuning_fix: "Fix system settings",
    rdp_tuning_pending: "Requesting administrator rights; confirm the UAC prompt…",
    rdp_tuning_fixed: "Defaults restored. Reconnect the RDP session.",
    rdp_tuning_failed: "Could not restore (admin rights required or UAC cancelled).",
    uac_desktop_title: "UAC secure desktop",
    uac_desktop_on: "On - elevation prompts appear on an isolated secure desktop",
    uac_desktop_off: "Off - UAC prompts on the current desktop without switching",
    uac_desktop_disable: "Turn off secure desktop",
    uac_desktop_enable: "Turn back on",
    uac_desktop_warn: "With this off the elevation prompt no longer runs on an isolated desktop, so ordinary programs could theoretically interfere with it. You still decide whether to elevate.",
    uac_desktop_pending: "Requesting administrator rights; confirm the UAC prompt...",
    uac_desktop_applied: "Applied; takes effect at the next UAC prompt",
    uac_desktop_failed: "Could not change (admin rights required or UAC cancelled)",
    toggle_on: "On",
    toggle_off: "Off",
    follow_window: "Follow window",
    desktop_fullscreen: "Full screen (primary monitor)",
    perf_auto: "Autodetect",
    perf_modem: "Modem (56 Kbps)",
    perf_broadband_low: "Low broadband (256 Kbps–2 Mbps)",
    perf_satellite: "Satellite (high latency)",
    perf_broadband_high: "High broadband (2–10 Mbps)",
    perf_wan: "WAN (high latency)",
    perf_lan: "LAN (10 Mbps+)",
    audio_local: "Play on this computer",
    audio_remote: "Play on remote",
    audio_none: "Do not play",
    keyboard_local: "Apply locally only",
    keyboard_remote: "Send to remote",
    keyboard_fullscreen: "Fullscreen only",
    color_15: "15-bit",
    color_16: "16-bit",
    color_24: "24-bit",
    color_32: "32-bit",
    rdp_reset: "Reset to inherited",
    rdp_set_default: "Set as default",
    info_rdp_settings_reset: "Cleared this connection's RDP overrides; everything inherits the defaults",
    info_rdp_defaults_saved: "Saved the effective RDP settings as the global defaults",
    terminal_settings_title: "Terminal settings",
    terminal_font_family: "Font",
    terminal_font_size: "Size",
    terminal_cursor: "Cursor",
    terminal_backspace: "Backspace",
    terminal_home_end: "Home / End",
    terminal_color_scheme: "Colors",
    terminal_cursor_block: "Block",
    terminal_cursor_underline: "Underline",
    terminal_cursor_beam: "Beam",
    terminal_backspace_del: "DEL",
    terminal_backspace_bs: "Backspace",
    terminal_home_end_standard: "Standard",
    terminal_home_end_rxvt: "rxvt",
    terminal_scheme_default: "Default dark",
    terminal_scheme_solarized_dark: "Solarized Dark",
    terminal_scheme_solarized_light: "Solarized Light",
    terminal_scheme_linux: "Linux console",
    terminal_scheme_xterm: "xterm light",
    terminal_charset: "Charset",
    terminal_mouse: "Mouse",
    terminal_keypad: "Function keys",
    charset_utf8: "UTF-8",
    charset_gbk: "GBK",
    charset_big5: "Big5",
    charset_latin1: "Latin-1",
    mouse_hybrid: "Hybrid",
    mouse_xterm: "xterm",
    mouse_windows: "Windows",
    keypad_standard: "Standard",
    keypad_escn: "ESC[n~",
    keypad_linux: "Linux",
    info_terminal_settings_reset: "Cleared this connection's terminal overrides; everything inherits the defaults",
    info_terminal_defaults_saved: "Saved the effective terminal settings as the global defaults",
    info_copied: "Copied to the system clipboard",
    private_key_title: "Private key",
    private_key_placeholder: "-----BEGIN OPENSSH PRIVATE KEY-----",
    private_key_saved: "Key saved",
    private_key_unsaved: "No saved key",
    clear_private_key: "Clear saved key",
    info_private_key_cleared: "Cleared the private key for this connection",

    section_directory: "Folder",
    section_connection: "Authentication",
    field_name: "Name",
    placeholder_directory_name: "Folder name",
    placeholder_session_name: "Connection name",
    field_host: "Address",
    placeholder_host: "host.example.com",
    field_username: "Username",
    placeholder_username: "Leave empty to ask on connect",
    field_password: "Password",
    field_note: "Notes",
    placeholder_note: "Optional, stored locally",
    placeholder_password: "Leave empty to keep the saved password",
    placeholder_password_plain: "Shown as plain text",
    reveal_password: "Show",
    clear_password: "Clear saved password",
    action_save: "Save",
    action_open: "Open",
    action_delete_directory: "Delete folder",
    action_delete_session: "Delete",
    empty_title: "Nothing selected",
    empty_hint: "Pick a connection on the left, or create one:",
    empty_storage_note: "Stored in the data folder (default: data next to the program), passwords encrypted separately",

    settings_title: "Settings",
    settings_subtitle: "Appearance, language, connection defaults, data folder and encryption",
    appearance_title: "Appearance",
    appearance_hint: "\"Follow system\" reads the Windows light/dark setting at startup.",
    theme_system: "Follow system",
    theme_light: "Light",
    theme_dark: "Dark",
    language_title: "Language",
    language_hint: "Applies immediately and is written to the config file.",
    data_dir_title: "Data folder",
    data_dir_hint: "Config and sessions live here. Defaults to data next to the program; changes are copied, old folder kept.",
    reveal_dir: "Open folder",
    change_dir: "Change…",
    use_default_dir: "Use default",
    choose_data_dir: "Choose data folder",
    data_dir_changed: "Data folder changed",
    data_dir_env_locked: "The data folder is fixed by SUPERREMOTE_DATA_DIR and cannot be changed here",
    new_tab_title: "New connection",
    defaults_rdp_title: "Remote Desktop defaults",
    defaults_rdp_hint: "Used by new connections and by items set to inherit; open sessions are unaffected.",
    defaults_terminal_title: "Terminal defaults",
    defaults_terminal_hint: "Font, cursor and key behavior for SSH / Telnet; applies on reconnect.",
    defaults_restore: "Restore built-ins",
    open_mode: "Preferred",
    open_mode_tab: "Tab",
    open_mode_background: "Background tab",
    open_mode_window: "Separate window",
    defaults_open_title: "Default open mode",
    defaults_open_hint: "Connections without their own preference use this; the three menu entries stay explicit.",
    info_rdp_defaults_reset: "Cleared the Remote Desktop defaults; everything falls back to built-ins",
    info_terminal_defaults_reset: "Cleared the terminal defaults; everything falls back to built-ins",
    security_title: "Credential encryption",
    security_hint_password: "Key derived from your master password; a forgotten password cannot be recovered.",
    security_hint_os: "Windows key store; copied files stay unreadable to other accounts. Software running as you can decrypt them.",
    mode_master_password: "Master password",
    mode_os_keychain: "System key store",
    startup_needs_password: "Password required at startup",
    startup_no_password: "No prompt at startup",
    field_new_password: "New password",
    placeholder_min_password: "At least 6 characters",
    change_password: "Change password",
    enable_password: "Enable master password",
    disable_password: "Remove password",

    unlock_hint: "Credentials are encrypted with your master password",
    unlock_password: "Master password",
    unlock_action: "Unlock",
    unlock_wrong: "Wrong password",
    fatal_title: "Cannot open the data folder",
    fatal_hint: "Fix the problem and restart. If the config comes from a newer version, do not open it with this build.",

    err_empty_name: "Name cannot be empty",
    err_empty_host: "Host cannot be empty",
    err_bad_port: "Port must be an integer between 0 and 65535",
    err_node_gone: "That item no longer exists; select again",
    info_saved: "Saved",
    info_password_cleared: "Saved password cleared",
    err_directory_not_openable: "Folders cannot be opened; pick a connection inside",
    info_password_changed: "Master password changed; only the data key was rewrapped",
    info_password_enabled: "Master password enabled; required at next start",
    info_password_disabled: "Back to the system key store; no prompt at startup",
    err_selection_gone: "That item no longer exists",
};

#[cfg(test)]
mod tests {
    use super::*;

    /// 两份词表必须字段齐全——结构体已经保证了这一点，这里守的是「别留空串」：
    /// 漏填的字段在界面上的表现是一片空白，测试里看不出来。
    #[test]
    fn both_languages_are_filled_in() {
        assert!(ZH.tab_details.is_empty() == EN.tab_details.is_empty());
        // 逐条过一遍，任何一边少了文案都让测试失败。
        for (zh, en, name) in [
            (ZH.nav_sessions, EN.nav_sessions, "nav_sessions"),
            (ZH.nav_settings, EN.nav_settings, "nav_settings"),
            (
                ZH.search_placeholder,
                EN.search_placeholder,
                "search_placeholder",
            ),
            (ZH.tab_details, EN.tab_details, "tab_details"),
            (ZH.action_save, EN.action_save, "action_save"),
            (ZH.action_open, EN.action_open, "action_open"),
            (ZH.empty_title, EN.empty_title, "empty_title"),
            (ZH.settings_title, EN.settings_title, "settings_title"),
            (ZH.appearance_title, EN.appearance_title, "appearance_title"),
            (ZH.language_title, EN.language_title, "language_title"),
            (ZH.change_dir, EN.change_dir, "change_dir"),
            (ZH.use_default_dir, EN.use_default_dir, "use_default_dir"),
            (ZH.choose_data_dir, EN.choose_data_dir, "choose_data_dir"),
            (ZH.data_dir_changed, EN.data_dir_changed, "data_dir_changed"),
            (
                ZH.data_dir_env_locked,
                EN.data_dir_env_locked,
                "data_dir_env_locked",
            ),
            (ZH.new_tab_title, EN.new_tab_title, "new_tab_title"),
            (ZH.unlock_action, EN.unlock_action, "unlock_action"),
            (
                ZH.defaults_rdp_title,
                EN.defaults_rdp_title,
                "defaults_rdp_title",
            ),
            (
                ZH.defaults_terminal_title,
                EN.defaults_terminal_title,
                "defaults_terminal_title",
            ),
            (
                ZH.desktop_fullscreen,
                EN.desktop_fullscreen,
                "desktop_fullscreen",
            ),
        ] {
            assert!(!zh.trim().is_empty(), "中文缺文案：{name}");
            assert!(!en.trim().is_empty(), "英文缺文案：{name}");
        }
    }

    #[test]
    fn counts_agree_with_their_number() {
        assert_eq!(Lang::Zh.n_sessions(1), "1 条连接");
        assert_eq!(Lang::En.n_sessions(1), "1 session");
        assert_eq!(Lang::En.n_sessions(3), "3 sessions");
    }

    /// 两份词表是各写各的，字段齐全只保证「有值」，不保证真的翻译过。
    /// 抽查几条最容易复制粘贴后忘记改的。
    #[test]
    fn the_two_vocabularies_are_not_copies() {
        for (zh, en, name) in [
            (ZH.action_save, EN.action_save, "action_save"),
            (ZH.action_open, EN.action_open, "action_open"),
            (ZH.settings_title, EN.settings_title, "settings_title"),
            (ZH.tree_no_match, EN.tree_no_match, "tree_no_match"),
            (ZH.unlock_wrong, EN.unlock_wrong, "unlock_wrong"),
        ] {
            assert_ne!(zh, en, "{name} 两边还是同一句话");
        }
    }

    #[test]
    fn codes_round_trip() {
        for lang in [Lang::Zh, Lang::En] {
            assert_eq!(Lang::from_code(lang.code()), Some(lang));
        }
        assert_eq!(Lang::from_code("fr"), None);
    }
}
