//! Rust 与 C# RDP 宿主之间的字符串契约。
//!
//! 宿主是独立进程（`host/Program.cs`，由 `build.rs` 用 `csc.exe` 编译后嵌进
//! exe），两边只靠窗口标题与命令行参数沟通。前缀或参数名一旦在某一端改名，
//! 另一端不会报错，只会静默失效（找不到窗口、参数取默认值）。所以字面量
//! **只在这里定义**，Rust 侧一律引用常量；本模块的测试直接读 `Program.cs`
//! 并按参数名对账，改一端时测试会先红。

/// 宿主窗口标题的前缀。标题格式是 `SUPERREMOTE|<session>|<state>|<message>`。
///
/// 会话段让一个宿主进程里的多个窗口可以各自被 Rust 认领；旧宿主的
/// `SUPERREMOTE|<state>|<message>` 仍然能解析（升级期间旧宿主可能被文件锁留住）。
pub const TITLE_PREFIX: &str = "SUPERREMOTE|";

/// 命令通道窗口的标题。Rust 按 pid + 此标题找到它，再发 `WM_COPYDATA`。
pub const CONTROL_TITLE: &str = "SUPERREMOTE|ctl";

/// 命令：在宿主进程里新增一路会话，载荷是 `add\n<session>\n<--flag>\n<value>...`。
pub const COMMAND_ADD: &str = "add";
/// 命令：关闭并回收一路会话。
pub const COMMAND_CLOSE: &str = "close";

/// `WM_COPYDATA.dwData` 携带的协议版本，两端必须一致。
pub const COMMAND_VERSION: usize = 1;

// 命令与响应分开走：命令是到 `CONTROL_TITLE` 的 `WM_COPYDATA`，
// 响应仍是每个会话窗口自己的标题（`state`/`message`）。

/// 宿主正在建立连接。
pub const STATE_CONNECTING: &str = "connecting";
/// 宿主已连上远端。
pub const STATE_CONNECTED: &str = "connected";
/// 宿主连接失败，标题后半段是错误信息。
pub const STATE_FAILED: &str = "failed";

/// 会话 id。一个宿主进程承载多路会话时，每个窗口标题都用它区分。
pub const ARG_SESSION: &str = "--session";

pub const ARG_SERVER: &str = "--server";
pub const ARG_PORT: &str = "--port";
pub const ARG_USER: &str = "--user";
pub const ARG_WIDTH: &str = "--width";
pub const ARG_HEIGHT: &str = "--height";
pub const ARG_BACKDROP: &str = "--backdrop";
pub const ARG_SMART_SIZING: &str = "--smart-sizing";
pub const ARG_COLOR: &str = "--color";
pub const ARG_AUDIO: &str = "--audio";
pub const ARG_AUDIO_CAPTURE: &str = "--audio-capture";
pub const ARG_KEYBOARD: &str = "--keyboard";
pub const ARG_PERFORMANCE: &str = "--performance";
pub const ARG_REDIRECT_PRINTERS: &str = "--redirect-printers";
pub const ARG_REDIRECT_CLIPBOARD: &str = "--redirect-clipboard";
pub const ARG_REDIRECT_SMARTCARDS: &str = "--redirect-smartcards";
pub const ARG_REDIRECT_PORTS: &str = "--redirect-ports";
pub const ARG_REDIRECT_DRIVES: &str = "--redirect-drives";
/// 用哪版 RDP ActiveX 控件（`10` = 旧版 C# 同款 RDP 8.1，`11` = RDP 10 + AVC）。
pub const ARG_RDP_CONTROL: &str = "--rdp-control";

/// Rust 侧会传给宿主的全部参数。新增参数时同时加进这里与 `Program.cs`。
pub const ARGS: [&str; 19] = [
    ARG_SESSION,
    ARG_SERVER,
    ARG_PORT,
    ARG_USER,
    ARG_WIDTH,
    ARG_HEIGHT,
    ARG_BACKDROP,
    ARG_SMART_SIZING,
    ARG_COLOR,
    ARG_AUDIO,
    ARG_AUDIO_CAPTURE,
    ARG_KEYBOARD,
    ARG_PERFORMANCE,
    ARG_REDIRECT_PRINTERS,
    ARG_REDIRECT_CLIPBOARD,
    ARG_REDIRECT_SMARTCARDS,
    ARG_REDIRECT_PORTS,
    ARG_REDIRECT_DRIVES,
    ARG_RDP_CONTROL,
];

/// 宿主标题拆出来的三段。`session` 为 `None` 表示旧宿主（标题里没有会话段）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostTitle<'a> {
    pub session: Option<&'a str>,
    pub state: &'a str,
    pub message: &'a str,
}

/// 解析 `SUPERREMOTE|<session>|<state>|<message>`，兼容旧格式 `SUPERREMOTE|<state>|<message>`。
///
/// 旧格式的 message 里可能含 `|`，所以先按「第二段是不是已知状态」判定；
/// 不是已知状态就说明第一段才是状态，整条按旧格式解释。
pub fn parse_title(title: &str) -> Option<HostTitle<'_>> {
    let rest = title.strip_prefix(TITLE_PREFIX)?;
    let (first, tail) = rest.split_once('|')?;

    if let Some((second, message)) = tail.split_once('|') {
        if is_state(second) {
            return Some(HostTitle {
                session: Some(first),
                state: second,
                message,
            });
        }
    }
    if is_state(first) {
        return Some(HostTitle {
            session: None,
            state: first,
            message: tail,
        });
    }
    None
}

fn is_state(state: &str) -> bool {
    state == STATE_CONNECTING || state == STATE_CONNECTED || state == STATE_FAILED
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_title_routes_a_session_to_its_state() {
        let host = parse_title("SUPERREMOTE|r7|connected|").expect("新格式应能解析");

        assert_eq!(host.session, Some("r7"));
        assert_eq!(host.state, STATE_CONNECTED);
        assert_eq!(host.message, "");
    }

    #[test]
    fn a_message_may_contain_separators() {
        let host = parse_title("SUPERREMOTE|r3|failed|证书 a|b 被拒绝").expect("新格式应能解析");

        assert_eq!(host.session, Some("r3"));
        assert_eq!(host.state, STATE_FAILED);
        assert_eq!(host.message, "证书 a|b 被拒绝");
    }

    /// 升级中旧宿主可能仍在跑（临时目录里的 exe 被锁住），它发的是三段标题。
    #[test]
    fn an_old_host_title_still_parses() {
        let connected = parse_title("SUPERREMOTE|connected|").expect("旧格式应能解析");
        assert_eq!(connected.session, None);
        assert_eq!(connected.state, STATE_CONNECTED);

        let failed = parse_title("SUPERREMOTE|failed|旧错误 a|b").expect("旧格式应能解析");
        assert_eq!(failed.session, None);
        assert_eq!(failed.state, STATE_FAILED);
        assert_eq!(failed.message, "旧错误 a|b");
    }

    #[test]
    fn the_control_window_is_not_a_session_status() {
        assert_eq!(parse_title(CONTROL_TITLE), None);
        assert_eq!(parse_title("SUPERREMOTE|"), None);
        assert_eq!(parse_title("SUPERREMOTE|r1|unknown|"), None);
    }
}
