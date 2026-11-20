//! SuperRemote 本地存储。
//!
//! 两个文件，都在同一个数据目录里：
//! - `config.toml`：版本、会话树、展开状态、`[security]`（数据密钥的包裹结果）。明文，可手改可备份。
//! - `secrets.toml`：凭据 id → AES-256-GCM 密文。除密文外不落任何凭据信息。
//!
//! 口令模型见 [`keys`]：默认用 Windows DPAPI 包裹数据密钥（启动不问口令），
//! 也可以在设置里改成主口令（Argon2id 派生 KEK 再包 DEK）。

mod document;
mod error;
mod hex;
mod keys;
mod paths;
mod secrets;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use superremote_core::session::{OpenMode, RdpSettings, SessionTree, TerminalSettings};

pub use crate::document::{ConfigDoc, NodeDoc, Protocol, UiDoc, SCHEMA};
pub use crate::error::StoreError;
pub use crate::keys::{KeyMode, Security};
pub use crate::paths::{DataPaths, PathNotice, CONFIG_FILE, SECRETS_FILE};
pub use crate::secrets::SecretStore;

use crate::keys::{create, random_bytes, rewrap, unlock, DataKey};

#[derive(Debug, thiserror::Error)]
pub enum OpenError {
    #[error("需要口令")]
    NeedsPassword,
    #[error("口令不正确")]
    WrongPassword,
    #[error(transparent)]
    Failed(#[from] StoreError),
}

/// 打开时做过的一次性补救，交给界面提示用户去哪儿找回原件。
#[derive(Debug, Clone)]
pub enum Notice {
    ConfigRestored { backup: PathBuf, reason: String },
    SecretsRestored { backup: PathBuf, reason: String },
}

pub struct Opened {
    pub store: Store,
    pub notices: Vec<Notice>,
}

/// 一个数据文件「本进程最后一次见到它」时的指纹（修改时间 + 长度）。
///
/// `save` 写前核对：磁盘上的文件与指纹不一致说明有别的程序（另一个实例、
/// 编辑器、备份恢复）在动它，盲写会把对方的改动整个冲掉，因此拒绝保存。
/// 文件缺失不算冲突——那等价于「外部删了它」，重建是被允许的。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct FileStamps {
    config: Option<(std::time::SystemTime, u64)>,
    secrets: Option<(std::time::SystemTime, u64)>,
}

impl FileStamps {
    fn from_disk(paths: &DataPaths) -> Self {
        Self {
            config: stamp_of(&paths.config()),
            secrets: stamp_of(&paths.secrets()),
        }
    }
}

fn stamp_of(path: &Path) -> Option<(std::time::SystemTime, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

/// 磁盘上的文件是否偏离了本进程最后一次见到的样子。
/// 记录缺失（启动时文件就不在）时只认「文件出现了」为正常——外部删掉不算冲突。
fn external_changed(path: &Path, last: Option<(std::time::SystemTime, u64)>) -> bool {
    match (last, stamp_of(path)) {
        (Some(last), Some(current)) => last != current,
        _ => false,
    }
}

pub struct Store {
    paths: DataPaths,
    doc: ConfigDoc,
    tree: SessionTree,
    secrets: SecretStore,
    key: DataKey,
    config_dirty: bool,
    stamps: FileStamps,
    id_prefix: String,
    id_seq: AtomicU64,
}

impl Store {
    /// 打开数据目录。`password` 只在主口令模式下才会用到。
    pub fn open(paths: DataPaths, password: Option<&str>) -> Result<Opened, OpenError> {
        let mut notices = Vec::new();
        paths.ensure_dir()?;

        let mut doc = match read_config(&paths)? {
            Some(text) => match ConfigDoc::parse(&text) {
                Ok(doc) => doc,
                // 只有「读不懂」才备份重建；版本过新必须报错，否则会静默丢掉新格式数据。
                Err(err @ StoreError::UnsupportedSchema(_)) => return Err(err.into()),
                Err(other) => {
                    let backup = move_aside(&paths.config(), "config")?;
                    notices.push(Notice::ConfigRestored {
                        backup,
                        reason: other.to_string(),
                    });
                    ConfigDoc::empty(None)
                }
            },
            None => ConfigDoc::empty(None),
        };

        let key = match doc.security.clone() {
            Some(security) => match unlock(&security, password) {
                Ok(key) => key,
                Err(StoreError::PasswordRequired) => return Err(OpenError::NeedsPassword),
                Err(StoreError::WrongPassword) => return Err(OpenError::WrongPassword),
                Err(other) => return Err(other.into()),
            },
            None => {
                let (security, key) = create(KeyMode::Os, None)?;
                doc.security = Some(security);
                let mut store = Self::assemble(doc, key, paths, &mut notices, true)?;
                // 首次运行就落盘，用户能马上在磁盘上看到自己的配置文件。
                store.save()?;
                return Ok(Opened { store, notices });
            }
        };

        Ok(Opened {
            store: Self::assemble(doc, key, paths, &mut notices, false)?,
            notices,
        })
    }

    fn assemble(
        doc: ConfigDoc,
        key: DataKey,
        paths: DataPaths,
        notices: &mut Vec<Notice>,
        config_dirty: bool,
    ) -> Result<Store, OpenError> {
        let secrets = match SecretStore::load(paths.secrets(), &key) {
            Ok(store) => store,
            Err(err) if err.is_corruption() => {
                let backup = move_aside(&paths.secrets(), "secrets")?;
                notices.push(Notice::SecretsRestored {
                    backup,
                    reason: err.to_string(),
                });
                SecretStore::load(paths.secrets(), &key)?
            }
            Err(err) => return Err(err.into()),
        };

        let tree = doc.to_tree();
        // 启动时记下两个文件的现状；此后只信自己的写入，磁盘上再有变化就是
        // 外部修改，save 会拒绝盲写覆盖。
        let stamps = FileStamps::from_disk(&paths);
        Ok(Store {
            id_prefix: random_bytes(4)
                .map(|bytes| hex::encode(&bytes))
                .unwrap_or_default(),
            id_seq: AtomicU64::new(0),
            config_dirty,
            stamps,
            paths,
            doc,
            tree,
            secrets,
            key,
        })
    }

    pub fn paths(&self) -> &DataPaths {
        &self.paths
    }

    /// 把当前打开的存储搬到另一个目录。
    ///
    /// 调用方先负责把旧文件复制过去；这里只改路径并把内存中的配置/凭据写到
    /// 新目录。失败时恢复旧路径，避免出现「界面换了目录、数据还在旧目录」的
    /// 半截状态。
    pub fn relocate(&mut self, paths: DataPaths) -> Result<(), StoreError> {
        paths.ensure_dir()?;

        let old = self.paths.clone();
        self.paths = paths.clone();
        self.secrets.set_path(paths.secrets());
        self.config_dirty = true;
        // 新目录里的文件是调用方刚复制的，不是外部修改：按现状重置指纹，
        // 否则下面这轮 save 的核对会把「刚复制过来的文件」当成冲突拒掉。
        self.stamps = FileStamps::from_disk(&self.paths);

        if let Err(err) = self.save() {
            self.paths = old.clone();
            self.secrets.set_path(old.secrets());
            self.config_dirty = true;
            return Err(err);
        }

        Ok(())
    }

    /// 界面偏好（主题、语言）。改动会标脏，由 `save` 一并落盘。
    pub fn ui(&self) -> &UiDoc {
        &self.doc.ui
    }

    pub fn ui_mut(&mut self) -> &mut UiDoc {
        self.config_dirty = true;
        &mut self.doc.ui
    }

    /// 只读地窥一眼界面偏好，不需要解锁。
    ///
    /// 主口令模式下 `open` 会以 `NeedsPassword` 返回、拿不到 `Store`，
    /// 但解锁页本身也需要正确的主题与语言，所以这里单独读一次配置。
    /// 读不懂就退回默认值——这个函数永远不报错，它只影响外观。
    pub fn peek_ui(paths: &DataPaths) -> UiDoc {
        read_config(paths)
            .ok()
            .flatten()
            .and_then(|text| ConfigDoc::parse(&text).ok())
            .map(|doc| doc.ui)
            .unwrap_or_default()
    }

    pub fn tree(&self) -> &SessionTree {
        &self.tree
    }

    /// 经此处的修改会标脏，由 `save` 落盘。
    pub fn tree_mut(&mut self) -> &mut SessionTree {
        self.config_dirty = true;
        &mut self.tree
    }

    /// 全局 RDP 默认设置。未写的项由 `RdpSettings::resolve` 回退到程序内置默认。
    pub fn rdp_defaults(&self) -> RdpSettings {
        self.doc.rdp_defaults()
    }

    /// 把一组生效值固化为全局默认。只影响之后新建/继承的连接。
    pub fn set_rdp_defaults(&mut self, settings: &RdpSettings) {
        self.doc.set_rdp_defaults(settings);
        self.config_dirty = true;
    }

    pub fn terminal_defaults(&self) -> TerminalSettings {
        self.doc.terminal_defaults()
    }

    pub fn set_terminal_defaults(&mut self, settings: &TerminalSettings) {
        self.doc.set_terminal_defaults(settings);
        self.config_dirty = true;
    }

    /// 全局默认的打开方式。没设过时返回 `None`（调用方回退到 `OpenMode::default()`）。
    pub fn open_mode(&self) -> Option<OpenMode> {
        self.doc.open_mode()
    }

    pub fn set_open_mode(&mut self, mode: Option<OpenMode>) {
        self.doc.set_open_mode(mode);
        self.config_dirty = true;
    }

    pub fn password(&self, id: &str) -> Result<Option<String>, StoreError> {
        self.secrets.get(id, &self.key)
    }

    pub fn has_password(&self, id: &str) -> bool {
        self.secrets.has(id)
    }

    /// SSH 私钥与口令共用加密文件，但用独立的键，避免互相覆盖。
    fn private_key_id(id: &str) -> String {
        format!("{id}#private-key")
    }

    pub fn private_key(&self, id: &str) -> Result<Option<String>, StoreError> {
        self.secrets.get(&Self::private_key_id(id), &self.key)
    }

    pub fn has_private_key(&self, id: &str) -> bool {
        self.secrets.has(&Self::private_key_id(id))
    }

    /// `None` 表示清除该私钥。
    pub fn set_private_key(&mut self, id: &str, value: Option<&str>) -> Result<(), StoreError> {
        self.secrets
            .put(&Self::private_key_id(id), value, &self.key)
    }

    pub fn secret_count(&self) -> usize {
        self.secrets.len()
    }

    /// `None` 表示清除该凭据。
    pub fn set_password(&mut self, id: &str, value: Option<&str>) -> Result<(), StoreError> {
        self.secrets.put(id, value, &self.key)
    }

    /// 生成节点 id：随机前缀 + 递增序号，便于在 TOML 里肉眼分辨。
    pub fn next_id(&self) -> String {
        let seq = self.id_seq.fetch_add(1, Ordering::Relaxed);
        format!("{}-{}", self.id_prefix, seq)
    }

    pub fn key_mode(&self) -> KeyMode {
        self.doc
            .security
            .as_ref()
            .map(|security| security.mode)
            .unwrap_or(KeyMode::Os)
    }

    /// 启用或更换主口令。只重包 DEK，凭据密文不动。
    pub fn set_master_password(&mut self, password: &str) -> Result<(), StoreError> {
        let security = self.doc.security.get_or_insert(Security {
            mode: KeyMode::Os,
            salt: None,
            wrapped_dek: String::new(),
        });
        rewrap(security, &self.key, KeyMode::Password, Some(password))?;
        self.config_dirty = true;
        Ok(())
    }

    /// 退回系统密钥库包裹，之后启动不再问口令。
    pub fn clear_master_password(&mut self) -> Result<(), StoreError> {
        let security = self
            .doc
            .security
            .as_mut()
            .ok_or_else(|| StoreError::Secret("尚未初始化密钥".to_owned()))?;
        rewrap(security, &self.key, KeyMode::Os, None)?;
        self.config_dirty = true;
        Ok(())
    }

    pub fn is_dirty(&self) -> bool {
        self.config_dirty || self.secrets.is_dirty()
    }

    /// 原子写：临时文件 + 改名覆盖。两个文件各自独立提交。
    ///
    /// 写前核对指纹：磁盘上的文件与本进程最后一次见到的不一致（另一个实例、
    /// 编辑器或备份恢复动过它）时拒绝保存，避免盲写覆盖对方的改动；文件
    /// 缺失不算冲突，按「允许重建」处理。只核对**这次真的要写**的文件——
    /// 干净时的 save 是空操作，不该因为外部改动而报错挡住目录切换这类流程。
    pub fn save(&mut self) -> Result<(), StoreError> {
        if self.config_dirty && external_changed(&self.paths.config(), self.stamps.config) {
            return Err(StoreError::ExternalModified(CONFIG_FILE.into()));
        }
        if self.secrets.is_dirty() && external_changed(&self.paths.secrets(), self.stamps.secrets) {
            return Err(StoreError::ExternalModified(SECRETS_FILE.into()));
        }

        if self.config_dirty {
            self.doc = self.doc.with_tree(&self.tree);
            let text = self.doc.render()?;
            paths::write_atomic(&self.paths.config(), text.as_bytes())?;
            self.config_dirty = false;
            // 自己写的文件指纹立刻跟上：后半程 secrets 失败时，下一轮 save
            // 不能把这次由本进程写入的 config 误判成外部改动。
            self.stamps.config = stamp_of(&self.paths.config());
        }

        let wrote_secrets = self.secrets.is_dirty();
        self.secrets.save()?;
        if wrote_secrets {
            self.stamps.secrets = stamp_of(&self.paths.secrets());
        }
        Ok(())
    }
}

fn read_config(paths: &DataPaths) -> Result<Option<String>, StoreError> {
    match std::fs::read_to_string(paths.config()) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(StoreError::io("读取配置文件", e)),
    }
}

/// 把坏文件改名保留，返回新路径。
fn move_aside(path: &Path, tag: &str) -> Result<PathBuf, StoreError> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();
    let backup = path.with_file_name(format!("{tag}.toml.broken-{stamp}"));

    std::fs::rename(path, &backup).map_err(|e| StoreError::io("备份损坏的数据文件", e))?;
    Ok(backup)
}

#[cfg(test)]
mod tests {
    use super::*;
    use superremote_core::session::{Directory, Session, SessionKind, SessionNode};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "superremote-store-{}-{}-{}",
                tag,
                std::process::id(),
                random_bytes(4)
                    .unwrap()
                    .iter()
                    .map(|b| *b as u32)
                    .sum::<u32>()
            ));
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

    fn session(id: &str) -> SessionNode {
        SessionNode::Session(Session {
            id: id.to_owned(),
            name: "Web-01".to_owned(),
            kind: SessionKind::Ssh,
            host: "10.20.1.11".to_owned(),
            port: 22,
            username: Some("ops".to_owned()),
            settings: Default::default(),
            window: None,
        })
    }

    #[test]
    fn relocating_keeps_the_tree_and_writes_to_the_new_folder() {
        let old = TempDir::new("relocate-old");
        let new = TempDir::new("relocate-new");
        let Opened { mut store, .. } = Store::open(old.paths(), None).unwrap();

        store.tree_mut().roots = vec![session("web")];
        store.relocate(new.paths()).unwrap();

        assert!(new.paths().config().exists());
        assert!(old.paths().config().exists(), "旧目录保留作备份");

        let Opened { store, .. } = Store::open(new.paths(), None).unwrap();
        assert_eq!(store.tree().roots.len(), 1, "搬迁后重新打开仍能看到会话");
    }

    #[test]
    fn first_run_creates_both_files_with_the_os_key() {
        let temp = TempDir::new("first-run");
        let Opened { store, notices } = Store::open(temp.paths(), None).unwrap();

        assert!(notices.is_empty());
        assert_eq!(store.key_mode(), KeyMode::Os);
        assert!(temp.paths().config().exists());
        // 没有凭据时 secrets.toml 不必存在，避免留下空文件。
        assert!(!temp.paths().secrets().exists());

        let text = std::fs::read_to_string(temp.paths().config()).unwrap();
        assert!(text.contains("[security]"));
        assert!(text.contains("mode = \"os\""));
    }

    #[test]
    fn tree_and_secret_survive_a_reopen() {
        let temp = TempDir::new("reopen");

        {
            let mut store = Store::open(temp.paths(), None).unwrap().store;
            store.tree_mut().roots = vec![SessionNode::Directory(Directory {
                id: "prod".to_owned(),
                name: "生产环境".to_owned(),
                children: vec![session("web-01")],
            })];
            store.tree_mut().expanded.insert("prod".to_owned());
            store.set_password("web-01", Some("s3cr3t")).unwrap();
            store.save().unwrap();
        }

        let reopened = Store::open(temp.paths(), None).unwrap().store;
        assert_eq!(reopened.tree().session_count(), 1);
        assert_eq!(
            reopened.tree().find_session("web-01").map(|s| s.address()),
            Some("10.20.1.11:22".to_owned())
        );
        assert!(reopened.tree().expanded.contains("prod"));
        assert_eq!(
            reopened.password("web-01").unwrap().as_deref(),
            Some("s3cr3t")
        );
    }

    #[test]
    fn an_externally_modified_config_refuses_to_save_and_keeps_disk_intact() {
        let temp = TempDir::new("external-modified");
        let mut store = Store::open(temp.paths(), None).unwrap().store;
        store.tree_mut().roots = vec![session("ours")];
        store.save().unwrap();

        // 外部程序（另一个实例 / 编辑器 / 备份恢复）改写了 config.toml。
        let external = "# 外部写入\n";
        std::fs::write(temp.paths().config(), external).unwrap();

        store.tree_mut().roots = vec![session("ours"), session("new-one")];
        let err = store.save().unwrap_err();
        assert!(
            matches!(err, StoreError::ExternalModified(_)),
            "应为外部修改冲突，实际是 {err:?}"
        );
        assert_eq!(
            std::fs::read_to_string(temp.paths().config()).unwrap(),
            external,
            "外部写入必须原样留在盘上"
        );
    }

    #[test]
    fn a_clean_store_saves_fine_even_when_the_disk_file_changed() {
        let temp = TempDir::new("external-clean");
        let mut store = Store::open(temp.paths(), None).unwrap().store;
        store.tree_mut().roots = vec![session("ours")];
        store.save().unwrap();

        // 外部改写后本进程没有待保存的内容：save 是空操作，不该报冲突
        // 挡住「目录切换前先落盘」这类流程。
        std::fs::write(temp.paths().config(), "# 外部写入\n").unwrap();
        store.save().unwrap();
        assert!(
            !store.tree_mut().roots.is_empty(),
            "内存里的树不受影响，改动的丢失风险只在于不再落盘"
        );
    }

    #[test]
    fn master_password_gates_reopen_and_can_be_removed_again() {
        let temp = TempDir::new("master-password");
        let mut store = Store::open(temp.paths(), None).unwrap().store;
        store.set_password("web-01", Some("p1")).unwrap();
        store.set_master_password("口令123").unwrap();
        store.save().unwrap();
        drop(store);

        assert!(matches!(
            Store::open(temp.paths(), None),
            Err(OpenError::NeedsPassword)
        ));
        assert!(matches!(
            Store::open(temp.paths(), Some("口令12")),
            Err(OpenError::WrongPassword)
        ));

        // 口令模式下的凭据仍要能解出来——这是 DEK 不变的好处。
        let mut store = Store::open(temp.paths(), Some("口令123")).unwrap().store;
        assert_eq!(store.key_mode(), KeyMode::Password);
        assert_eq!(store.password("web-01").unwrap().as_deref(), Some("p1"));

        store.set_master_password("新口令456").unwrap();
        store.save().unwrap();
        drop(store);

        let store = Store::open(temp.paths(), Some("新口令456")).unwrap().store;
        assert_eq!(store.password("web-01").unwrap().as_deref(), Some("p1"));

        let mut store = Store::open(temp.paths(), Some("新口令456")).unwrap().store;
        store.clear_master_password().unwrap();
        store.save().unwrap();
        drop(store);

        let store = Store::open(temp.paths(), None).unwrap().store;
        assert_eq!(store.key_mode(), KeyMode::Os);
        assert_eq!(store.password("web-01").unwrap().as_deref(), Some("p1"));
    }

    #[test]
    fn a_half_written_config_is_backed_up_rather_than_crashing() {
        let temp = TempDir::new("broken-config");
        std::fs::write(
            temp.paths().config(),
            "schema = 1\ntree = [ this line was cut off mid write\n",
        )
        .unwrap();

        let Opened { store, notices } = Store::open(temp.paths(), None).unwrap();

        assert!(matches!(
            notices.as_slice(),
            [Notice::ConfigRestored { .. }]
        ));
        assert_eq!(store.tree().session_count(), 0);
        let backups: Vec<_> = std::fs::read_dir(temp.paths().dir())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .contains("config.toml.broken-")
            })
            .collect();
        assert_eq!(backups.len(), 1, "原件要保留下来供用户找回");
    }

    #[test]
    fn a_newer_schema_is_reported_and_left_untouched() {
        let temp = TempDir::new("newer-schema");
        let text = "schema = 99\n\n[[tree]]\nnode = \"session\"\nid = \"x\"\nname = \"未来的记录\"\nprotocol = \"ssh\"\nhost = \"1.2.3.4\"\nport = 22\n";
        std::fs::write(temp.paths().config(), text).unwrap();

        assert!(matches!(
            Store::open(temp.paths(), None),
            Err(OpenError::Failed(StoreError::UnsupportedSchema(99)))
        ));
        assert_eq!(
            std::fs::read_to_string(temp.paths().config()).unwrap(),
            text,
            "版本过新绝不能改写文件"
        );
    }

    #[test]
    fn private_keys_round_trip_and_clear_without_touching_the_password_slot() {
        let temp = TempDir::new("private-key");

        {
            let mut store = Store::open(temp.paths(), None).unwrap().store;
            store.set_password("web-01", Some("pw")).unwrap();
            store.set_private_key("web-01", Some("KEY")).unwrap();
            store.save().unwrap();
        }

        let mut store = Store::open(temp.paths(), None).unwrap().store;
        assert_eq!(store.private_key("web-01").unwrap().as_deref(), Some("KEY"));
        assert!(store.has_private_key("web-01"));
        assert_eq!(store.password("web-01").unwrap().as_deref(), Some("pw"));

        store.set_private_key("web-01", None).unwrap();
        assert!(store.private_key("web-01").unwrap().is_none());
        assert!(!store.has_private_key("web-01"));
        assert_eq!(store.password("web-01").unwrap().as_deref(), Some("pw"));
    }

    #[test]
    fn generated_ids_are_unique_and_saved_tree_is_not_rewritten_when_clean() {
        let temp = TempDir::new("clean-save");
        let mut store = Store::open(temp.paths(), None).unwrap().store;

        let first = store.next_id();
        let second = store.next_id();
        assert_ne!(first, second);
        assert!(first.contains('-'));

        let written = std::fs::metadata(temp.paths().config())
            .unwrap()
            .modified()
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(30));
        store.save().unwrap();
        assert_eq!(
            std::fs::metadata(temp.paths().config())
                .unwrap()
                .modified()
                .unwrap(),
            written,
            "没有改动就不该重写文件"
        );
    }
}
