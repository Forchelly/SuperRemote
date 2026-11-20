//! 数据目录与文件名。
//!
//! 默认位置是**程序目录下的 `data`**（便携安装：复制整个安装目录即可带走配置），
//! 用户可以在设置页改成任意目录；选择结果写在程序目录（不可写时退到
//! `%APPDATA%\SimpleRemote2`）的 `data-location.txt` 里。
//!
//! 旧版本的 `%APPDATA%\SimpleRemote2` 数据不会被丢掉：首次按新规则启动时若程序
//! 目录可写，会把两份数据文件**复制**过去并保留旧目录作为备份。环境变量
//! `SUPERREMOTE_DATA_DIR` 仍然优先，测试与临时验证都靠它隔离真实数据。
//!
//! **目录名不跟着产品改名**。产品已从 SimpleRemote2 更名为 SuperRemote，但
//! `%APPDATA%\SimpleRemote2` 里可能躺着老用户的配置与密文；改成 `SuperRemote`
//! 会让他们首次启动看到空列表（甚至以为数据丢了）。所以这里继续按旧目录名
//! 读取，也不做静默搬迁——便携安装本来就落在程序目录的 `data`，不受改名影响。

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::error::StoreError;

pub const CONFIG_FILE: &str = "config.toml";
pub const SECRETS_FILE: &str = "secrets.toml";
/// 程序目录旁边默认的数据文件夹名。
pub const DATA_DIR_NAME: &str = "data";
/// 自定义数据目录的指针文件。放在程序目录优先，程序目录不可写时放用户配置目录。
pub const LOCATION_FILE: &str = "data-location.txt";
/// 旧版固定数据目录名。**这是历史路径，不是产品名**：产品已叫 SuperRemote，
/// 但老用户的数据在 `%APPDATA%\SimpleRemote2`，改掉就等于让他们丢配置。
const LEGACY_APP_DIR: &str = "SimpleRemote2";

/// 启动时自动做过的一次目录调整，交给界面提示用户。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathNotice {
    /// 旧版 AppData 数据已复制到程序目录的 `data`，旧目录保留。
    Migrated { to: PathBuf },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataPaths {
    dir: PathBuf,
}

impl DataPaths {
    pub fn resolve_with_notice() -> (Self, Option<PathNotice>) {
        if let Some(from_env) = Self::env_override() {
            return (from_env, None);
        }

        let legacy = Self::legacy_dir();
        let portable = Self::portable_dir();
        let pointer = Self::read_location();

        choose_portable(portable, legacy, pointer)
    }

    /// `SUPERREMOTE_DATA_DIR` 非空时返回它。测试、便携启动都走这个口径。
    pub fn env_override() -> Option<Self> {
        let raw = std::env::var_os("SUPERREMOTE_DATA_DIR")?;
        let path = PathBuf::from(raw);
        if path.as_os_str().is_empty() {
            None
        } else {
            Some(Self { dir: path })
        }
    }

    /// 程序目录：`<exe 所在目录>`；取不到时返回 `None`。
    pub fn exe_dir() -> Option<PathBuf> {
        std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(Path::to_path_buf))
    }

    /// 默认的便携数据目录：程序目录下的 `data`。
    pub fn portable_dir() -> Option<Self> {
        Self::exe_dir().map(|dir| Self {
            dir: dir.join(DATA_DIR_NAME),
        })
    }

    /// 旧版固定目录：`%APPDATA%\SimpleRemote2`。名字保持原样，见模块顶部说明。
    pub fn legacy_dir() -> Self {
        let appdata = std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        Self {
            dir: appdata.join(LEGACY_APP_DIR),
        }
    }

    pub fn from_dir(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &PathBuf {
        &self.dir
    }

    pub fn config(&self) -> PathBuf {
        self.dir.join(CONFIG_FILE)
    }

    pub fn secrets(&self) -> PathBuf {
        self.dir.join(SECRETS_FILE)
    }

    pub fn ensure_dir(&self) -> Result<(), StoreError> {
        fs::create_dir_all(&self.dir).map_err(|e| StoreError::io("创建数据目录", e))
    }

    /// 目录里已经有配置或凭据文件。不能用另一个数据目录覆盖它。
    pub fn has_data(&self) -> bool {
        self.config().exists() || self.secrets().exists()
    }

    /// 目录能否创建并写入。程序装在 Program Files 这类只读位置时用它回退。
    pub fn is_writable(&self) -> bool {
        if self.ensure_dir().is_err() {
            return false;
        }

        let probe = self
            .dir
            .join(format!(".write-probe-{}", std::process::id()));
        let written = fs::write(&probe, b"");
        let _ = fs::remove_file(&probe);
        written.is_ok()
    }

    /// 把当前目录里的配置与凭据**复制**到目标目录。旧目录不动。
    ///
    /// 目标已有数据时拒绝，避免静默覆盖另一个安装/另一份配置。
    pub fn copy_data_to(&self, target: &Self, replace: bool) -> Result<(), StoreError> {
        target.ensure_dir()?;

        if target.has_data() {
            if !replace {
                return Err(StoreError::DataDirOccupied(
                    target.dir.display().to_string(),
                ));
            }

            // 「恢复默认」时默认目录里通常还留着上一次的副本。先把旧文件改名
            // 备份，再写入当前数据；失败也不会丢掉旧内容。
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default();
            for file in [target.config(), target.secrets()] {
                if file.exists() {
                    let backup = file.with_file_name(format!(
                        "{}.bak-{stamp}",
                        file.file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_else(|| "data".to_owned())
                    ));
                    fs::rename(&file, backup).map_err(|e| StoreError::io("备份旧数据目录", e))?;
                }
            }
        }

        for (source, destination) in [
            (self.config(), target.config()),
            (self.secrets(), target.secrets()),
        ] {
            if !source.exists() {
                continue;
            }

            let bytes = fs::read(&source).map_err(|e| StoreError::io("读取旧数据", e))?;
            crate::paths::write_atomic(&destination, &bytes)?;

            let copied = fs::read(&destination).map_err(|e| StoreError::io("校验数据副本", e))?;
            if copied != bytes {
                return Err(StoreError::io(
                    "校验数据副本",
                    io::Error::new(io::ErrorKind::InvalidData, "副本内容与源文件不一致"),
                ));
            }
        }

        Ok(())
    }

    /// 指针文件的候选位置：程序目录优先，其次用户配置目录。
    fn location_files() -> Vec<PathBuf> {
        let mut files = Vec::new();
        if let Some(exe) = Self::exe_dir() {
            files.push(exe.join(LOCATION_FILE));
        }
        files.push(Self::legacy_dir().dir.join(LOCATION_FILE));
        files
    }

    /// 读自定义数据目录指针。目录不存在时视为失效指针。
    pub fn read_location() -> Option<PathBuf> {
        read_location_from(&Self::location_files())
    }

    /// 记住自定义数据目录。目标就是默认的 `data` 时改为清除指针。
    pub fn save_location(dir: &Path) -> Result<(), StoreError> {
        if Self::portable_dir().is_some_and(|portable| portable.dir == dir) {
            return Self::clear_location();
        }

        save_location_to(&Self::location_files(), dir)
    }

    /// 清除自定义指针，回到程序目录的 `data`。
    pub fn clear_location() -> Result<(), StoreError> {
        let mut last_error = None;

        for path in Self::location_files() {
            match fs::remove_file(&path) {
                Ok(()) => {}
                Err(err) if err.kind() == io::ErrorKind::NotFound => {}
                Err(err) => last_error = Some(StoreError::io("清除数据目录位置", err)),
            }
        }

        last_error.map_or(Ok(()), Err)
    }
}

/// 从候选目录里选出本次启动要用的位置。
///
/// 单独拆出来是为了能在测试里注入临时目录；`resolve_with_notice` 只负责取
/// 环境变量、`current_exe` 与 APPDATA。
fn read_location_from(files: &[PathBuf]) -> Option<PathBuf> {
    for path in files {
        let Ok(text) = fs::read_to_string(path) else {
            continue;
        };
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        let candidate = PathBuf::from(text);
        if candidate.is_dir() {
            return Some(candidate);
        }
    }

    None
}

fn save_location_to(files: &[PathBuf], dir: &Path) -> Result<(), StoreError> {
    let text = format!("{}\n", dir.display());
    let mut last_error = None;

    for path in files {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }

        match crate::paths::write_atomic(path, text.as_bytes()) {
            Ok(()) => return Ok(()),
            Err(err) => last_error = Some(err),
        }
    }

    Err(last_error.unwrap_or_else(|| {
        StoreError::io(
            "保存数据目录位置",
            io::Error::new(io::ErrorKind::NotFound, "没有可写的位置指针目录"),
        )
    }))
}

fn choose_portable(
    portable: Option<DataPaths>,
    legacy: DataPaths,
    pointer: Option<PathBuf>,
) -> (DataPaths, Option<PathNotice>) {
    if let Some(pointer) = pointer {
        return (DataPaths::from_dir(pointer), None);
    }

    if let Some(portable) = portable.as_ref() {
        if portable.has_data() {
            return (portable.clone(), None);
        }
    }

    if legacy.has_data() {
        if let Some(portable) = portable {
            if portable.is_writable() && legacy.copy_data_to(&portable, false).is_ok() {
                return (
                    portable.clone(),
                    Some(PathNotice::Migrated { to: portable.dir }),
                );
            }
        }

        // 程序目录不可写或复制失败：继续用旧目录，绝不能为了「默认便携」
        // 把用户已有数据留在原地却启动一个空配置。
        return (legacy, None);
    }

    if let Some(portable) = portable {
        if portable.is_writable() {
            return (portable, None);
        }
    }

    (legacy, None)
}

/// 原子写入：先写同目录临时文件，再改名覆盖。
///
/// Windows 上 `rename` 走 `MoveFileExW(MOVEFILE_REPLACE_EXISTING)`，可以直接覆盖已存在目标。
pub fn write_atomic(path: &std::path::Path, bytes: &[u8]) -> Result<(), StoreError> {
    let tmp = path.with_extension("toml.tmp");

    {
        use std::io::Write;

        let mut file =
            std::fs::File::create(&tmp).map_err(|e| StoreError::io("写入数据文件", e))?;
        file.write_all(bytes)
            .map_err(|e| StoreError::io("写入数据文件", e))?;
        // 必须先把内容刷到磁盘再改名。少了这一步，断电后可能出现「改名成功、
        // 内容还在系统缓存里没落地」的零长度文件，凭据与配置会凭空消失一轮。
        file.sync_all()
            .map_err(|e| StoreError::io("落盘数据文件", e))?;
    }

    std::fs::rename(&tmp, path).map_err(|e| StoreError::io("替换数据文件", e))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "superremote-paths-{tag}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or_default()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn data(&self, name: &str) -> DataPaths {
            DataPaths::from_dir(self.0.join(name))
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn write_data(paths: &DataPaths) {
        paths.ensure_dir().unwrap();
        std::fs::write(paths.config(), "config").unwrap();
        std::fs::write(paths.secrets(), "secrets").unwrap();
    }

    #[test]
    fn copying_keeps_the_old_folder_and_refuses_an_occupied_target() {
        let temp = TempDir::new("copy");
        let source = temp.data("source");
        let target = temp.data("target");
        write_data(&source);

        source.copy_data_to(&target, false).unwrap();

        assert_eq!(std::fs::read(target.config()).unwrap(), b"config");
        assert_eq!(std::fs::read(target.secrets()).unwrap(), b"secrets");
        assert!(source.config().exists(), "旧目录必须保留作备份");

        let occupied = temp.data("occupied");
        write_data(&occupied);
        assert!(source.copy_data_to(&occupied, false).is_err());
        assert_eq!(
            std::fs::read(occupied.config()).unwrap(),
            b"config",
            "目标已有数据时不能覆盖"
        );

        // 「恢复默认」允许替换：旧文件改名备份，目标换成当前数据。
        source.copy_data_to(&occupied, true).unwrap();
        assert_eq!(std::fs::read(occupied.config()).unwrap(), b"config");
        assert!(
            std::fs::read_dir(occupied.dir())
                .unwrap()
                .filter_map(Result::ok)
                .any(|entry| entry.file_name().to_string_lossy().contains(".bak-")),
            "覆盖前必须留下备份"
        );
    }

    #[test]
    fn legacy_data_moves_to_the_program_folder_but_stays_as_backup() {
        let temp = TempDir::new("migrate");
        let legacy = temp.data("legacy");
        let portable = temp.data("portable");
        write_data(&legacy);

        let (chosen, notice) = choose_portable(Some(portable.clone()), legacy.clone(), None);

        assert_eq!(chosen, portable);
        assert_eq!(
            notice,
            Some(PathNotice::Migrated {
                to: portable.dir().clone()
            })
        );
        assert_eq!(std::fs::read(portable.config()).unwrap(), b"config");
        assert!(legacy.config().exists(), "旧数据必须保留");
    }

    #[test]
    fn existing_program_data_wins_over_legacy_data() {
        let temp = TempDir::new("existing");
        let legacy = temp.data("legacy");
        let portable = temp.data("portable");
        write_data(&legacy);
        write_data(&portable);

        let (chosen, notice) = choose_portable(Some(portable.clone()), legacy, None);

        assert_eq!(chosen, portable);
        assert_eq!(notice, None);
    }

    #[test]
    fn a_saved_pointer_wins_over_both_default_locations() {
        let temp = TempDir::new("pointer");
        let legacy = temp.data("legacy");
        let portable = temp.data("portable");
        let custom = temp.data("custom");
        write_data(&legacy);
        write_data(&portable);
        write_data(&custom);

        let (chosen, notice) = choose_portable(Some(portable), legacy, Some(custom.dir().clone()));

        assert_eq!(chosen, custom);
        assert_eq!(notice, None);
    }

    #[test]
    fn a_fresh_install_uses_the_program_folder() {
        let temp = TempDir::new("fresh");
        let legacy = temp.data("legacy");
        let portable = temp.data("portable");

        let (chosen, notice) = choose_portable(Some(portable.clone()), legacy, None);

        assert_eq!(chosen, portable);
        assert_eq!(notice, None);
    }

    #[test]
    fn a_read_only_program_folder_falls_back_to_legacy_appdata() {
        let temp = TempDir::new("readonly");
        let legacy = temp.data("legacy");
        let portable = temp.data("portable");
        write_data(&legacy);

        // 用同名文件占住路径：create_dir_all 会失败，等价于程序目录不可写。
        std::fs::create_dir_all(portable.dir().parent().unwrap()).unwrap();
        std::fs::write(portable.dir(), b"not a directory").unwrap();

        let (chosen, notice) = choose_portable(Some(portable), legacy.clone(), None);

        assert_eq!(chosen, legacy);
        assert_eq!(notice, None);
    }

    #[test]
    fn a_location_pointer_round_trips_and_uses_the_next_candidate() {
        let temp = TempDir::new("pointer-file");
        let target = temp.data("target");
        target.ensure_dir().unwrap();

        // 第一个候选的父路径是文件：创建目录失败，应自动落到第二个候选。
        let blocked = temp.0.join("blocked");
        std::fs::write(&blocked, b"not a directory").unwrap();
        let first = blocked.join(LOCATION_FILE);
        let second = temp.0.join("config-root").join(LOCATION_FILE);

        save_location_to(&[first.clone(), second.clone()], target.dir()).unwrap();

        assert_eq!(
            read_location_from(&[second.clone(), first]),
            Some(target.dir().clone())
        );
        assert!(std::fs::read_to_string(second)
            .unwrap()
            .contains(&target.dir().display().to_string()));
    }
}
