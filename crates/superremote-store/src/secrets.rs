//! `secrets.toml`：凭据 id → `hex(nonce || 密文 || tag)`。
//!
//! 附加认证数据用凭据 id 本身，这样一条密文不能被挪到另一个 id 下重用。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::StoreError;
use crate::hex;
use crate::keys::{aead_decrypt, aead_encrypt, DataKey};
use crate::paths::write_atomic;

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretsDoc {
    #[serde(default)]
    secrets: BTreeMap<String, String>,
}

pub struct SecretStore {
    path: PathBuf,
    doc: SecretsDoc,
    dirty: bool,
}

impl SecretStore {
    /// 只解析文件结构，不逐条解密：记录多时避免启动被 AEAD 全量校验拖慢，
    /// 单条解不开在 `get` 时报告。
    pub fn load(path: impl AsRef<Path>, key: &DataKey) -> Result<Self, StoreError> {
        let path = path.as_ref().to_path_buf();
        let doc = match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str::<SecretsDoc>(&text)
                .map_err(|e| StoreError::Decode(format!("secrets.toml：{e}")))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => SecretsDoc::default(),
            Err(e) => return Err(StoreError::io("读取凭据文件", e)),
        };

        let store = Self {
            path,
            doc,
            dirty: false,
        };
        // 用第一条记录做一次密钥自检，尽早暴露「配置换了密钥」这类不一致。
        //
        // 这里刻意不用 `get`：它把「hex 解不开」和「认证失败」都压成 `Secret`，
        // 前者是真损坏（备份后重建），后者是密钥换了（必须报错并保留文件），
        // 两种后果完全不同，混在一起就会把用户的凭据文件白白改名搬走。
        if let Some((id, blob)) = store.doc.secrets.iter().next() {
            let bytes = hex::decode(blob).map_err(StoreError::Secret)?;
            aead_decrypt(key.bytes(), &bytes, id.as_bytes()).map_err(|_| {
                StoreError::KeyMismatch(format!(
                    "凭据 {id} 无法解密，secrets.toml 与当前密钥不匹配"
                ))
            })?;
        }
        Ok(store)
    }

    pub fn get(&self, id: &str, key: &DataKey) -> Result<Option<String>, StoreError> {
        let Some(blob) = self.raw_get(id) else {
            return Ok(None);
        };

        let bytes = hex::decode(blob).map_err(StoreError::Secret)?;
        let plaintext = aead_decrypt(key.bytes(), &bytes, id.as_bytes())
            .map_err(|_| StoreError::Secret(format!("凭据 {id} 解密失败")))?;

        String::from_utf8(plaintext)
            .map(Some)
            .map_err(|e| StoreError::Secret(format!("凭据 {id} 不是合法文本：{e}")))
    }

    /// `value` 为 `None` 时删除该凭据。
    pub fn put(&mut self, id: &str, value: Option<&str>, key: &DataKey) -> Result<(), StoreError> {
        match value {
            Some(text) => {
                let blob = aead_encrypt(key.bytes(), text.as_bytes(), id.as_bytes())?;
                self.doc.secrets.insert(id.to_owned(), hex::encode(&blob));
            }
            None => {
                self.doc.secrets.remove(id);
            }
        }
        self.dirty = true;
        Ok(())
    }

    pub fn has(&self, id: &str) -> bool {
        self.doc.secrets.contains_key(id)
    }

    pub fn len(&self) -> usize {
        self.doc.secrets.len()
    }

    pub fn is_empty(&self) -> bool {
        self.doc.secrets.is_empty()
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// 数据目录搬迁后指向新文件；内存里的记录原样保留，下一次 save 写到新位置。
    pub fn set_path(&mut self, path: PathBuf) {
        self.path = path;
        self.dirty = true;
    }

    pub fn save(&mut self) -> Result<(), StoreError> {
        if !self.dirty {
            return Ok(());
        }

        let text = toml::to_string_pretty(&self.doc)
            .map_err(|e| StoreError::Decode(format!("secrets.toml：{e}")))?;
        let header = "# SuperRemote 凭据文件（已加密）。删除本文件等于清掉所有密码与私钥。\n";
        write_atomic(&self.path, format!("{header}{text}").as_bytes())?;
        self.dirty = false;
        Ok(())
    }

    fn raw_get(&self, id: &str) -> Option<&String> {
        self.doc.secrets.get(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::DataKey;

    fn temp_path(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sr-secrets-{}-{}", tag, std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        dir.join("secrets.toml")
    }

    #[test]
    fn put_get_and_remove_round_trip_through_disk() {
        let key = DataKey::generate().unwrap();
        let path = temp_path("roundtrip");
        let _ = std::fs::remove_file(&path);

        let mut store = SecretStore::load(&path, &key).unwrap();
        store.put("web-01", Some("p@ssw0rd"), &key).unwrap();
        store.save().unwrap();

        let reopened = SecretStore::load(&path, &key).unwrap();
        assert_eq!(
            reopened.get("web-01", &key).unwrap().as_deref(),
            Some("p@ssw0rd")
        );

        drop(reopened);
        let mut store = SecretStore::load(&path, &key).unwrap();
        store.put("web-01", None, &key).unwrap();
        store.save().unwrap();
        assert_eq!(
            SecretStore::load(&path, &key)
                .unwrap()
                .get("web-01", &key)
                .unwrap(),
            None
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn ciphertext_does_not_leak_plaintext_to_disk() {
        let key = DataKey::generate().unwrap();
        let path = temp_path("opaque");
        let _ = std::fs::remove_file(&path);

        let mut store = SecretStore::load(&path, &key).unwrap();
        store.put("db", Some("hunter2"), &key).unwrap();
        store.save().unwrap();

        let on_disk = std::fs::read_to_string(&path).unwrap();
        assert!(!on_disk.contains("hunter2"));
        assert!(on_disk.contains("[secrets]"));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_different_key_cannot_read_the_records() {
        let key = DataKey::generate().unwrap();
        let other = DataKey::generate().unwrap();
        let path = temp_path("wrongkey");
        let _ = std::fs::remove_file(&path);

        let mut store = SecretStore::load(&path, &key).unwrap();
        store.put("db", Some("secret"), &key).unwrap();
        store.save().unwrap();

        // 自检只覆盖第一条；这里直接验证用错密钥解不出来。
        let doc = std::fs::read_to_string(&path).unwrap();
        let parsed: SecretsDoc = toml::from_str(&doc).unwrap();
        let blob = hex::decode(parsed.secrets.get("db").unwrap()).unwrap();
        assert!(aead_decrypt(other.bytes(), &blob, b"db").is_err());

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn record_is_bound_to_its_id_by_the_aad() {
        let key = DataKey::generate().unwrap();
        let blob = aead_encrypt(key.bytes(), b"secret", b"db").unwrap();
        assert!(aead_decrypt(key.bytes(), &blob, b"other").is_err());
    }

    #[test]
    fn a_key_mismatch_is_reported_but_never_treated_as_corruption() {
        let key = DataKey::generate().unwrap();
        let other = DataKey::generate().unwrap();
        let path = temp_path("key-mismatch");
        let _ = std::fs::remove_file(&path);

        let mut store = SecretStore::load(&path, &key).unwrap();
        store.put("db", Some("secret"), &key).unwrap();
        store.save().unwrap();

        let Err(err) = SecretStore::load(&path, &other) else {
            panic!("用错密钥不应加载成功");
        };

        assert!(matches!(err, StoreError::KeyMismatch(_)), "{err:?}");
        assert!(
            !err.is_corruption(),
            "密钥不匹配绝不能走「备份后重建」，那会把完好的凭据文件搬走"
        );
        assert!(path.exists(), "文件必须原地保留");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_broken_hex_record_is_still_corruption() {
        let key = DataKey::generate().unwrap();
        let path = temp_path("broken-hex");
        let _ = std::fs::remove_file(&path);
        std::fs::write(&path, "[secrets]\ndb = \"not-hex\"\n").unwrap();

        let Err(err) = SecretStore::load(&path, &key) else {
            panic!("内容坏掉时不应加载成功");
        };

        assert!(err.is_corruption(), "hex 坏掉仍然属于损坏：{err:?}");

        let _ = std::fs::remove_file(&path);
    }
}
