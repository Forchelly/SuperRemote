//! `config.toml` 的文档结构与领域模型的双向转换。
//!
//! 这里刻意不复用 `superremote-core` 的类型做序列化：文件 schema 与内存模型分开演进，
//! 重命名字段不等于改表结构。原版 LiteDB 把两者混在一起，改属性名就得配迁移。

use serde::{Deserialize, Serialize};
use superremote_core::session::{
    Directory, OpenMode, RdpAudio, RdpColorDepth, RdpDesktopSize, RdpGraphics, RdpHostMode,
    RdpKeyboard, RdpPerformance, RdpSettings, Session, SessionKind, SessionNode, SessionSettings,
    SessionTree, TerminalBackspace, TerminalCharset, TerminalColorScheme, TerminalCursor,
    TerminalHomeEnd, TerminalKeypad, TerminalMouseMode, TerminalSettings, WindowSize,
};

use crate::error::StoreError;
use crate::keys::Security;

/// 当前文件格式版本。加字段不必升版；改名、改语义、删字段才升。
pub const SCHEMA: u32 = 1;

const HEADER: &str = concat!(
    "# SuperRemote 配置文件\n",
    "# 可以手工编辑，但请先退出程序；程序保存时会重写本文件。\n",
    "# 凭据不在这里，在同目录的 secrets.toml（已加密）。\n",
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigDoc {
    pub schema: u32,
    /// 会话树的根层节点，目录可嵌套。
    #[serde(default)]
    pub tree: Vec<NodeDoc>,
    /// 展开的目录 id。
    #[serde(default)]
    pub expanded: Vec<String>,
    /// 凭据加密参数；没有凭据时也可以先建好。
    pub security: Option<Security>,
    /// 界面偏好。没有这段时用程序默认值。
    #[serde(default)]
    pub ui: UiDoc,
    /// 各协议的全局默认设置。单条连接里没写的项从这里回退。
    #[serde(default)]
    pub defaults: DefaultsDoc,
}

/// 界面偏好：主题与语言。
///
/// 存字符串而不是枚举，是为了让**旧版本**读到新值时只是忽略这一项，
/// 而不是因为枚举变体不认识导致整份配置解析失败、进而被当成损坏文件备份掉。
/// 取值由 `superremote-app` 解释，这里只负责搬运。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiDoc {
    /// `system` / `light` / `dark`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,
    /// `zh` / `en`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
}

/// 全局默认设置。字段和单条覆盖完全一样，取值时再与程序内置默认合并。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DefaultsDoc {
    #[serde(default, skip_serializing_if = "RdpSettingsDoc::is_empty")]
    pub rdp: RdpSettingsDoc,
    #[serde(default, skip_serializing_if = "TerminalSettingsDoc::is_empty")]
    pub terminal: TerminalSettingsDoc,
    /// 三种协议共用的默认打开方式。没写时按 `OpenMode::default()`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_mode: Option<String>,
}

/// 一条连接上的设置覆盖。空表不落盘，保持配置文件干净。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingsDoc {
    #[serde(default, skip_serializing_if = "RdpSettingsDoc::is_empty")]
    pub rdp: RdpSettingsDoc,
    #[serde(default, skip_serializing_if = "TerminalSettingsDoc::is_empty")]
    pub terminal: TerminalSettingsDoc,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_mode: Option<String>,
    /// 上次独立窗口的尺寸。属于使用习惯，不进 `[defaults]`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<WindowDoc>,
    /// 备注（旧版「描述」）。空串不落盘。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// 独立窗口尺寸的落盘形态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowDoc {
    pub width: u32,
    pub height: u32,
}

impl SettingsDoc {
    pub fn is_empty(&self) -> bool {
        self.rdp.is_empty()
            && self.terminal.is_empty()
            && self.open_mode.is_none()
            && self.window.is_none()
            && self.note.is_none()
    }
}

/// 终端设置的落盘形态。枚举同样存字符串，未知值只忽略这一项。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalSettingsDoc {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_family: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_size: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backspace: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home_end: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_scheme: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub charset: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mouse_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keypad: Option<String>,
}

impl TerminalSettingsDoc {
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }

    pub fn from_settings(settings: &TerminalSettings) -> Self {
        Self {
            font_family: settings.font_family.clone(),
            font_size: settings.font_size,
            cursor: settings.cursor.map(TerminalCursor::code).map(str::to_owned),
            backspace: settings
                .backspace
                .map(TerminalBackspace::code)
                .map(str::to_owned),
            home_end: settings
                .home_end
                .map(TerminalHomeEnd::code)
                .map(str::to_owned),
            color_scheme: settings
                .color_scheme
                .map(TerminalColorScheme::code)
                .map(str::to_owned),
            charset: settings
                .charset
                .map(TerminalCharset::code)
                .map(str::to_owned),
            mouse_mode: settings
                .mouse_mode
                .map(TerminalMouseMode::code)
                .map(str::to_owned),
            keypad: settings.keypad.map(TerminalKeypad::code).map(str::to_owned),
        }
    }

    pub fn to_settings(&self) -> TerminalSettings {
        TerminalSettings {
            font_family: self.font_family.clone(),
            font_size: self.font_size,
            cursor: self.cursor.as_deref().and_then(TerminalCursor::from_code),
            backspace: self
                .backspace
                .as_deref()
                .and_then(TerminalBackspace::from_code),
            home_end: self
                .home_end
                .as_deref()
                .and_then(TerminalHomeEnd::from_code),
            color_scheme: self
                .color_scheme
                .as_deref()
                .and_then(TerminalColorScheme::from_code),
            charset: self.charset.as_deref().and_then(TerminalCharset::from_code),
            mouse_mode: self
                .mouse_mode
                .as_deref()
                .and_then(TerminalMouseMode::from_code),
            keypad: self.keypad.as_deref().and_then(TerminalKeypad::from_code),
        }
    }
}

/// `Option` 表示「这一项没有覆盖/没有默认值」。
///
/// 枚举存字符串而不是 Rust 枚举：以后加档位时，旧程序读到不认识的字符串
/// 只会忽略这一项，不会把整份配置判成损坏。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RdpSettingsDoc {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub performance: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_depth: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_capture: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keyboard: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redirect_printers: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redirect_clipboard: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redirect_smartcards: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redirect_ports: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redirect_drives: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub desktop_size: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graphics: Option<String>,
}

impl RdpSettingsDoc {
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }

    pub fn from_settings(settings: &RdpSettings) -> Self {
        Self {
            host_mode: settings.host_mode.map(RdpHostMode::code).map(str::to_owned),
            performance: settings
                .performance
                .map(RdpPerformance::code)
                .map(str::to_owned),
            color_depth: settings
                .color_depth
                .map(RdpColorDepth::code)
                .map(str::to_owned),
            audio: settings.audio.map(RdpAudio::code).map(str::to_owned),
            audio_capture: settings.audio_capture,
            keyboard: settings.keyboard.map(RdpKeyboard::code).map(str::to_owned),
            redirect_printers: settings.redirect_printers,
            redirect_clipboard: settings.redirect_clipboard,
            redirect_smartcards: settings.redirect_smartcards,
            redirect_ports: settings.redirect_ports,
            redirect_drives: settings.redirect_drives,
            desktop_size: settings.desktop_size.map(RdpDesktopSize::code),
            graphics: settings.graphics.map(RdpGraphics::code).map(str::to_owned),
        }
    }

    pub fn to_settings(&self) -> RdpSettings {
        RdpSettings {
            host_mode: self.host_mode.as_deref().and_then(RdpHostMode::from_code),
            performance: self
                .performance
                .as_deref()
                .and_then(RdpPerformance::from_code),
            color_depth: self
                .color_depth
                .as_deref()
                .and_then(RdpColorDepth::from_code),
            audio: self.audio.as_deref().and_then(RdpAudio::from_code),
            audio_capture: self.audio_capture,
            keyboard: self.keyboard.as_deref().and_then(RdpKeyboard::from_code),
            redirect_printers: self.redirect_printers,
            redirect_clipboard: self.redirect_clipboard,
            redirect_smartcards: self.redirect_smartcards,
            redirect_ports: self.redirect_ports,
            redirect_drives: self.redirect_drives,
            desktop_size: self
                .desktop_size
                .as_deref()
                .and_then(RdpDesktopSize::from_code),
            graphics: self.graphics.as_deref().and_then(RdpGraphics::from_code),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "node", rename_all = "lowercase")]
pub enum NodeDoc {
    Dir {
        id: String,
        name: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        children: Vec<NodeDoc>,
    },
    Session {
        id: String,
        name: String,
        protocol: Protocol,
        host: String,
        port: u16,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        username: Option<String>,
        #[serde(default, skip_serializing_if = "boxed_settings_are_empty")]
        settings: Box<SettingsDoc>,
    },
}

fn boxed_settings_are_empty(settings: &SettingsDoc) -> bool {
    settings.is_empty()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    Rdp,
    Ssh,
    Telnet,
}

impl Protocol {
    fn from_kind(kind: SessionKind) -> Self {
        match kind {
            SessionKind::Rdp => Self::Rdp,
            SessionKind::Ssh => Self::Ssh,
            SessionKind::Telnet => Self::Telnet,
        }
    }

    fn to_kind(self) -> SessionKind {
        match self {
            Self::Rdp => SessionKind::Rdp,
            Self::Ssh => SessionKind::Ssh,
            Self::Telnet => SessionKind::Telnet,
        }
    }
}

impl ConfigDoc {
    pub fn empty(security: Option<Security>) -> Self {
        Self {
            schema: SCHEMA,
            tree: Vec::new(),
            expanded: Vec::new(),
            security,
            ui: UiDoc::default(),
            defaults: DefaultsDoc::default(),
        }
    }

    pub fn rdp_defaults(&self) -> RdpSettings {
        self.defaults.rdp.to_settings()
    }

    pub fn set_rdp_defaults(&mut self, settings: &RdpSettings) {
        self.defaults.rdp = RdpSettingsDoc::from_settings(settings);
    }

    pub fn terminal_defaults(&self) -> TerminalSettings {
        self.defaults.terminal.to_settings()
    }

    pub fn set_terminal_defaults(&mut self, settings: &TerminalSettings) {
        self.defaults.terminal = TerminalSettingsDoc::from_settings(settings);
    }

    /// 全局默认的打开方式。`None` 表示没设过，由调用方回退到 `OpenMode::default()`。
    pub fn open_mode(&self) -> Option<OpenMode> {
        self.defaults
            .open_mode
            .as_deref()
            .and_then(OpenMode::from_code)
    }

    pub fn set_open_mode(&mut self, mode: Option<OpenMode>) {
        self.defaults.open_mode = mode.map(OpenMode::code).map(str::to_owned);
    }

    pub fn parse(text: &str) -> Result<Self, StoreError> {
        let doc: ConfigDoc = toml::from_str(text).map_err(|e| StoreError::Decode(e.to_string()))?;

        if doc.schema > SCHEMA {
            return Err(StoreError::UnsupportedSchema(doc.schema));
        }

        Ok(doc)
    }

    pub fn render(&self) -> Result<String, StoreError> {
        let body = toml::to_string_pretty(self).map_err(|e| StoreError::Decode(e.to_string()))?;
        Ok(format!("{HEADER}{body}"))
    }

    /// 落盘的文档 → 内存里的会话树。展开状态整体替换，保证两边一致。
    pub fn to_tree(&self) -> SessionTree {
        let roots = self.tree.iter().map(node_from_doc).collect();
        SessionTree {
            roots,
            expanded: self.expanded.iter().cloned().collect(),
        }
    }

    /// 内存里的会话树 → 待落盘的文档（保留当前文档的 security 与 ui 段）。
    pub fn with_tree(&self, tree: &SessionTree) -> Self {
        // `expanded` 在内存里是 `HashSet`，迭代顺序跨进程不确定。不排序的话
        // 同一份树每次重启保存都会写出不同顺序的数组，手改对比和版本控制全是噪音。
        let mut expanded: Vec<String> = tree.expanded.iter().cloned().collect();
        expanded.sort();

        Self {
            schema: self.schema,
            tree: tree.roots.iter().map(node_to_doc).collect(),
            expanded,
            security: self.security.clone(),
            ui: self.ui.clone(),
            defaults: self.defaults.clone(),
        }
    }
}

fn node_from_doc(doc: &NodeDoc) -> SessionNode {
    match doc {
        NodeDoc::Dir { id, name, children } => SessionNode::Directory(Directory {
            id: id.clone(),
            name: name.clone(),
            children: children.iter().map(node_from_doc).collect(),
        }),
        NodeDoc::Session {
            id,
            name,
            protocol,
            host,
            port,
            username,
            settings,
        } => SessionNode::Session(Session {
            id: id.clone(),
            name: name.clone(),
            kind: protocol.to_kind(),
            host: host.clone(),
            port: *port,
            username: username.clone(),
            settings: SessionSettings {
                rdp: settings.rdp.to_settings(),
                terminal: settings.terminal.to_settings(),
                open_mode: settings.open_mode.as_deref().and_then(OpenMode::from_code),
                note: settings.note.clone().unwrap_or_default(),
            },
            window: settings.window.map(|size| WindowSize {
                width: size.width,
                height: size.height,
            }),
        }),
    }
}

fn node_to_doc(node: &SessionNode) -> NodeDoc {
    match node {
        SessionNode::Directory(dir) => NodeDoc::Dir {
            id: dir.id.clone(),
            name: dir.name.clone(),
            children: dir.children.iter().map(node_to_doc).collect(),
        },
        SessionNode::Session(session) => NodeDoc::Session {
            id: session.id.clone(),
            name: session.name.clone(),
            protocol: Protocol::from_kind(session.kind),
            host: session.host.clone(),
            port: session.port,
            username: session.username.clone(),
            settings: Box::new(SettingsDoc {
                rdp: RdpSettingsDoc::from_settings(&session.settings.rdp),
                terminal: TerminalSettingsDoc::from_settings(&session.settings.terminal),
                open_mode: session
                    .settings
                    .open_mode
                    .map(OpenMode::code)
                    .map(str::to_owned),
                window: session.window.map(|size| WindowDoc {
                    width: size.width,
                    height: size.height,
                }),
                note: (!session.settings.note.is_empty()).then(|| session.settings.note.clone()),
            }),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> SessionTree {
        let mut tree = SessionTree::new(vec![
            SessionNode::Directory(Directory {
                id: "prod".to_owned(),
                name: "生产环境".to_owned(),
                children: vec![SessionNode::Session(Session {
                    id: "prod-web-01".to_owned(),
                    name: "Web-01".to_owned(),
                    kind: SessionKind::Ssh,
                    host: "10.20.1.11".to_owned(),
                    port: 22,
                    username: Some("ops".to_owned()),
                    settings: Default::default(),
                    window: None,
                })],
            }),
            SessionNode::Session(Session {
                id: "lab".to_owned(),
                name: "Lab".to_owned(),
                kind: SessionKind::Rdp,
                host: "10.0.0.5".to_owned(),
                port: 3389,
                username: None,
                settings: Default::default(),
                window: None,
            }),
        ]);
        tree.expand_all();
        tree
    }

    #[test]
    fn round_trip_preserves_tree_and_expansion() {
        let original = tree();
        let doc = ConfigDoc::empty(None).with_tree(&original);

        let text = doc.render().unwrap();
        let parsed = ConfigDoc::parse(&text).unwrap();
        let back = parsed.to_tree();

        assert_eq!(back.roots, original.roots);
        assert_eq!(back.expanded, original.expanded);
    }

    #[test]
    fn rendered_file_is_readable_toml_with_a_header() {
        let text = ConfigDoc::empty(None).with_tree(&tree()).render().unwrap();

        assert!(text.starts_with("# SuperRemote 配置文件"));
        assert!(text.contains("node = \"dir\""));
        assert!(text.contains("protocol = \"ssh\""));
        assert!(text.contains("name = \"生产环境\""));
        // 空的 username 不该出现在文件里。
        assert_eq!(text.matches("username").count(), 1);
    }

    #[test]
    fn missing_optional_sections_still_parse() {
        let doc = ConfigDoc::parse("schema = 1\n").unwrap();
        assert!(doc.tree.is_empty());
        assert!(doc.expanded.is_empty());
        assert_eq!(doc.security, None);
    }

    #[test]
    fn newer_schema_is_rejected_rather_than_silently_truncated() {
        assert!(matches!(
            ConfigDoc::parse("schema = 99\n"),
            Err(StoreError::UnsupportedSchema(99))
        ));
    }

    #[test]
    fn broken_text_is_reported_as_decode_not_panicked() {
        assert!(matches!(
            ConfigDoc::parse("schema = [ this is not toml\n"),
            Err(StoreError::Decode(_))
        ));
    }

    #[test]
    fn rdp_overrides_and_global_defaults_round_trip() {
        let mut tree = tree();
        tree.with_session_mut("lab", |session| {
            session.settings.rdp.performance = Some(RdpPerformance::Lan);
            session.settings.rdp.redirect_clipboard = Some(false);
        });

        let mut doc = ConfigDoc::empty(None).with_tree(&tree);
        doc.set_rdp_defaults(&RdpSettings {
            color_depth: Some(RdpColorDepth::Bpp16),
            ..RdpSettings::default()
        });

        let text = doc.render().unwrap();
        assert!(text.contains("[defaults.rdp]"));
        assert!(text.contains("color_depth = \"bpp16\""));

        let parsed = ConfigDoc::parse(&text).unwrap();
        let back = parsed.to_tree();
        let session = back.find_session("lab").expect("Lab 应还在");

        assert_eq!(session.settings.rdp.performance, Some(RdpPerformance::Lan));
        assert_eq!(session.settings.rdp.redirect_clipboard, Some(false));
        assert_eq!(
            parsed.rdp_defaults().color_depth,
            Some(RdpColorDepth::Bpp16)
        );
    }

    #[test]
    fn terminal_overrides_and_defaults_round_trip() {
        let mut tree = tree();
        tree.with_session_mut("lab", |session| {
            session.settings.terminal.font_family = Some("Cascadia Mono".to_owned());
            session.settings.terminal.font_size = Some(16);
            session.settings.terminal.cursor = Some(TerminalCursor::Beam);
        });

        let mut doc = ConfigDoc::empty(None).with_tree(&tree);
        doc.set_terminal_defaults(&TerminalSettings {
            color_scheme: Some(TerminalColorScheme::SolarizedLight),
            backspace: Some(TerminalBackspace::Backspace),
            ..TerminalSettings::default()
        });

        let text = doc.render().unwrap();
        assert!(text.contains("font_family = \"Cascadia Mono\""));
        assert!(text.contains("color_scheme = \"solarized-light\""));

        let parsed = ConfigDoc::parse(&text).unwrap();
        let back = parsed.to_tree();
        let settings = &back.find_session("lab").unwrap().settings.terminal;

        assert_eq!(settings.font_family.as_deref(), Some("Cascadia Mono"));
        assert_eq!(settings.font_size, Some(16));
        assert_eq!(settings.cursor, Some(TerminalCursor::Beam));
        assert_eq!(
            parsed.terminal_defaults().color_scheme,
            Some(TerminalColorScheme::SolarizedLight)
        );
        assert_eq!(
            parsed.terminal_defaults().backspace,
            Some(TerminalBackspace::Backspace)
        );
    }

    #[test]
    fn unknown_setting_codes_are_ignored_instead_of_failing_the_file() {
        let doc = ConfigDoc::parse("schema = 1\n[defaults.rdp]\nperformance = \"warp\"\n").unwrap();

        assert_eq!(doc.rdp_defaults().performance, None);
    }

    #[test]
    fn empty_session_settings_do_not_add_noise_to_the_file() {
        let text = ConfigDoc::empty(None).with_tree(&tree()).render().unwrap();

        assert!(!text.contains("settings"));
    }
}
