//! 数据密钥（DEK）与它的两种包裹方式。
//!
//! - `KeyMode::Os`：用 Windows DPAPI 包裹，绑定当前登录用户，启动不问口令。
//! - `KeyMode::Password`：主口令经 Argon2id 派生 KEK，再用 KEK 包裹 DEK，启动必须输口令。
//!
//! 凭据本身永远是用 DEK 加密的，所以「换口令」或「在两种模式之间切换」只重包 DEK，
//! 不需要把 `secrets.toml` 里的每条记录重新加密一遍。

use aes_gcm::aead::{Aead, Nonce, Payload};
use aes_gcm::{Aes256Gcm, KeyInit};
use argon2::{Argon2, Params, Version};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::error::StoreError;
use crate::hex;

pub const DEK_LEN: usize = 32;
pub const NONCE_LEN: usize = 12;
pub const SALT_LEN: usize = 16;

/// 包裹 DEK 时的附加认证数据，防止 DEK 密文被当成凭据密文复用。
const DEK_AAD: &[u8] = b"SimpleRemote-dek-v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KeyMode {
    /// 系统密钥库（DPAPI）：无感，防的是「文件被别人拷走」。
    Os,
    /// 主口令：真正的安全边界，忘了口令数据不可恢复。
    Password,
}

/// 写在 `config.toml` 的 `[security]` 段。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Security {
    pub mode: KeyMode,
    /// Argon2id 的 salt（hex），仅 `Password` 模式需要。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub salt: Option<String>,
    /// `Password`：`nonce || 密文 || tag`；`Os`：DPAPI 输出。均为 hex。
    pub wrapped_dek: String,
}

/// 解密凭据用的对称密钥。包一层是为了让「用完清零」由类型保证，而不是靠调用方自觉。
pub struct DataKey(Zeroizing<Vec<u8>>);

impl DataKey {
    fn from_bytes(bytes: Vec<u8>) -> Result<Self, StoreError> {
        if bytes.len() != DEK_LEN {
            return Err(StoreError::Secret(format!(
                "数据密钥应为 {} 字节，实际 {}",
                DEK_LEN,
                bytes.len()
            )));
        }
        Ok(Self(Zeroizing::new(bytes)))
    }

    pub fn generate() -> Result<Self, StoreError> {
        Ok(Self(Zeroizing::new(random_bytes(DEK_LEN)?)))
    }

    pub fn bytes(&self) -> &[u8] {
        &self.0
    }
}

/// 生成一份全新 DEK 并按指定模式包裹。
pub fn create(mode: KeyMode, password: Option<&str>) -> Result<(Security, DataKey), StoreError> {
    let dek = DataKey::generate()?;
    let mut security = Security {
        mode,
        salt: None,
        wrapped_dek: String::new(),
    };
    rewrap(&mut security, &dek, mode, password)?;
    Ok((security, dek))
}

/// 解开 DEK。口令模式下缺口令返回 `PasswordRequired`，口令不对返回 `WrongPassword`。
pub fn unlock(security: &Security, password: Option<&str>) -> Result<DataKey, StoreError> {
    let blob = hex::decode(&security.wrapped_dek).map_err(StoreError::Secret)?;

    match security.mode {
        KeyMode::Os => DataKey::from_bytes(dpapi::unprotect(&blob)?),
        KeyMode::Password => {
            let password = password.ok_or(StoreError::PasswordRequired)?;
            let salt = hex::decode(
                security
                    .salt
                    .as_deref()
                    .ok_or_else(|| StoreError::Secret("口令模式缺少 salt".to_owned()))?,
            )
            .map_err(StoreError::Secret)?;
            let kek = derive_kek(password.as_bytes(), &salt)?;
            let dek = aead_decrypt(&kek, &blob, DEK_AAD).map_err(|_| StoreError::WrongPassword)?;
            DataKey::from_bytes(dek)
        }
    }
}

/// 用新模式/新口令重新包裹同一个 DEK，就地更新 `security`。
pub fn rewrap(
    security: &mut Security,
    dek: &DataKey,
    mode: KeyMode,
    password: Option<&str>,
) -> Result<(), StoreError> {
    match mode {
        KeyMode::Os => {
            security.mode = mode;
            security.salt = None;
            security.wrapped_dek = hex::encode(&dpapi::protect(dek.bytes())?);
        }
        KeyMode::Password => {
            let password = password.ok_or(StoreError::PasswordRequired)?;
            let salt = random_bytes(SALT_LEN)?;
            let kek = derive_kek(password.as_bytes(), &salt)?;
            security.mode = mode;
            security.salt = Some(hex::encode(&salt));
            security.wrapped_dek = hex::encode(&aead_encrypt(&kek, dek.bytes(), DEK_AAD)?);
        }
    }
    Ok(())
}

/// Argon2id 参数用库默认（19 MiB / 2 轮 / 1 并行度），与 OWASP 推荐一致。
fn derive_kek(password: &[u8], salt: &[u8]) -> Result<Zeroizing<Vec<u8>>, StoreError> {
    let argon = Argon2::new(
        argon2::Algorithm::Argon2id,
        Version::V0x13,
        Params::default(),
    );
    let mut kek = Zeroizing::new(vec![0u8; DEK_LEN]);
    argon
        .hash_password_into(password, salt, &mut kek)
        .map_err(|e| StoreError::OsKey(format!("口令派生失败：{e}")))?;
    Ok(kek)
}

pub(crate) fn random_bytes(len: usize) -> Result<Vec<u8>, StoreError> {
    let mut bytes = vec![0u8; len];
    random_fill(&mut bytes)?;
    Ok(bytes)
}

/// 输出 `nonce || 密文 || tag`。
pub(crate) fn aead_encrypt(
    key: &[u8],
    plaintext: &[u8],
    aad: &[u8],
) -> Result<Vec<u8>, StoreError> {
    let cipher = cipher(key)?;
    let mut nonce_bytes = [0u8; NONCE_LEN];
    random_fill(&mut nonce_bytes)?;
    let ciphertext = cipher
        .encrypt(
            &Nonce::<Aes256Gcm>::from(nonce_bytes),
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|e| StoreError::OsKey(format!("加密失败：{e}")))?;

    let mut out = nonce_bytes.to_vec();
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

/// 输入 `nonce || 密文 || tag`；认证不通过时报 `Secret`，由调用方翻译成口令错误等语义。
pub(crate) fn aead_decrypt(key: &[u8], blob: &[u8], aad: &[u8]) -> Result<Vec<u8>, StoreError> {
    if blob.len() <= NONCE_LEN {
        return Err(StoreError::Secret("密文长度不足".to_owned()));
    }

    let (nonce_bytes, ciphertext) = blob.split_at(NONCE_LEN);
    let nonce = <&Nonce<Aes256Gcm>>::try_from(nonce_bytes)
        .map_err(|_| StoreError::Secret("nonce 长度不正确".to_owned()))?;
    let cipher = cipher(key)?;
    cipher
        .decrypt(
            nonce,
            Payload {
                msg: ciphertext,
                aad,
            },
        )
        .map_err(|_| StoreError::Secret("认证失败".to_owned()))
}

fn cipher(key: &[u8]) -> Result<Aes256Gcm, StoreError> {
    Aes256Gcm::new_from_slice(key)
        .map_err(|_| StoreError::Secret(format!("密钥应为 {} 字节", DEK_LEN)))
}

fn random_fill(bytes: &mut [u8]) -> Result<(), StoreError> {
    getrandom::fill(bytes).map_err(|e| StoreError::OsKey(format!("随机数生成失败：{e}")))?;
    Ok(())
}

#[cfg(windows)]
mod dpapi {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    use super::StoreError;

    /// 固定附加熵，把密文绑死在「只有 SimpleRemote 自己解」。改这个常量会让已有凭据全部失效。
    const ENTROPY: &[u8] = b"SimpleRemote-dpapi-v1";

    pub fn protect(data: &[u8]) -> Result<Vec<u8>, StoreError> {
        let mut owned = data.to_vec();
        let mut entropy = ENTROPY.to_vec();
        let in_blob = blob(&mut owned);
        let entropy_blob = blob(&mut entropy);
        let mut out_blob = CRYPT_INTEGER_BLOB::default();

        // SAFETY: 两个入参 blob 都指向上面 `owned`/`entropy` 的所有权内存，
        // 在本次调用期间既不会被移动也不会被改写（`owned`/`entropy` 活到函数末尾）；
        // 可选的 pOptionalEntropy 允许为 null，这里给的是合法 blob；出参 `out_blob`
        // 由系统填充，失败时不会分配，成功时我们在 [`take`] 里拷贝并 `LocalFree`。
        let ok = unsafe {
            CryptProtectData(
                &in_blob,
                std::ptr::null(),
                &entropy_blob,
                std::ptr::null_mut(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out_blob,
            )
        };

        if ok == 0 {
            return Err(StoreError::OsKey(format!(
                "CryptProtectData 失败：{}",
                std::io::Error::last_os_error()
            )));
        }

        Ok(take(out_blob))
    }

    pub fn unprotect(blob_data: &[u8]) -> Result<Vec<u8>, StoreError> {
        let mut owned = blob_data.to_vec();
        let mut entropy = ENTROPY.to_vec();
        let in_blob = blob(&mut owned);
        let entropy_blob = blob(&mut entropy);
        let mut out_blob = CRYPT_INTEGER_BLOB::default();

        // SAFETY: 同 [`protect`]——入参指向本地所有权缓冲，存活期间不被移动；
        // 出参缓冲由系统分配后交由 [`take`] 接管。`pDescrOut` 传 null 表示不要描述串，
        // 这正是该参数被允许的取值。
        let ok = unsafe {
            CryptUnprotectData(
                &in_blob,
                std::ptr::null_mut(),
                &entropy_blob,
                std::ptr::null_mut(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out_blob,
            )
        };

        if ok == 0 {
            return Err(StoreError::OsKey(format!(
                "CryptUnprotectData 失败：{}",
                std::io::Error::last_os_error()
            )));
        }

        Ok(take(out_blob))
    }

    fn blob(bytes: &mut [u8]) -> CRYPT_INTEGER_BLOB {
        CRYPT_INTEGER_BLOB {
            // 密钥与密文都是几十字节量级，`as` 不会截断；用 `try_from` 反而要在
            // 这里编造一个不可能发生的错误分支。
            cbData: bytes.len() as u32,
            pbData: bytes.as_mut_ptr(),
        }
    }

    /// 拷出系统分配的缓冲并 `LocalFree`——不释放的话每次解密密钥都漏一块内存。
    fn take(out_blob: CRYPT_INTEGER_BLOB) -> Vec<u8> {
        if out_blob.pbData.is_null() {
            return Vec::new();
        }

        // SAFETY: 走到这里说明对应的 `Crypt*` 调用返回了非零，按契约 `pbData` 指向
        // `cbData` 字节的系统分配缓冲，非空且可读。先拷贝成 `Vec` 再释放，
        // 拷贝之后不再解引用该指针。
        let bytes = unsafe {
            std::slice::from_raw_parts(out_blob.pbData, out_blob.cbData as usize).to_vec()
        };
        // SAFETY: 同一块缓冲只释放一次，且必须用系统分配的 `LocalFree`（不能用 `drop`）。
        // 释放后 `out_blob` 不再被使用。
        unsafe { LocalFree(out_blob.pbData as *mut _) };
        bytes
    }
}

#[cfg(not(windows))]
mod dpapi {
    use super::StoreError;

    pub fn protect(_: &[u8]) -> Result<Vec<u8>, StoreError> {
        Err(StoreError::OsKey("系统密钥库仅支持 Windows".to_owned()))
    }

    pub fn unprotect(_: &[u8]) -> Result<Vec<u8>, StoreError> {
        Err(StoreError::OsKey("系统密钥库仅支持 Windows".to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_mode_reports_wrong_password_instead_of_garbage() {
        let (security, dek) = create(KeyMode::Password, Some("正确口令")).unwrap();

        assert_eq!(
            unlock(&security, Some("正确口令")).unwrap().bytes(),
            dek.bytes()
        );
        assert!(matches!(
            unlock(&security, Some("错一个字符")),
            Err(StoreError::WrongPassword)
        ));
        assert!(matches!(
            unlock(&security, None),
            Err(StoreError::PasswordRequired)
        ));
    }

    #[test]
    fn rewrap_keeps_the_same_dek_so_secrets_need_no_reencryption() {
        let (mut security, dek) = create(KeyMode::Password, Some("旧口令")).unwrap();

        rewrap(&mut security, &dek, KeyMode::Password, Some("新口令")).unwrap();

        assert_eq!(
            unlock(&security, Some("新口令")).unwrap().bytes(),
            dek.bytes()
        );
        assert!(matches!(
            unlock(&security, Some("旧口令")),
            Err(StoreError::WrongPassword)
        ));
    }

    #[cfg(windows)]
    #[test]
    fn os_mode_round_trips_without_a_password() {
        let (security, dek) = create(KeyMode::Os, None).unwrap();
        assert_eq!(security.salt, None);
        assert_eq!(unlock(&security, None).unwrap().bytes(), dek.bytes());
    }

    #[cfg(windows)]
    #[test]
    fn switching_between_modes_preserves_the_dek() {
        let (mut security, dek) = create(KeyMode::Password, Some("口令")).unwrap();
        rewrap(&mut security, &dek, KeyMode::Os, None).unwrap();
        assert_eq!(unlock(&security, None).unwrap().bytes(), dek.bytes());

        rewrap(&mut security, &dek, KeyMode::Password, Some("口令")).unwrap();
        assert_eq!(
            unlock(&security, Some("口令")).unwrap().bytes(),
            dek.bytes()
        );
    }

    #[test]
    fn aead_rejects_tampered_blob() {
        let key = random_bytes(DEK_LEN).unwrap();
        let mut blob = aead_encrypt(&key, b"payload", DEK_AAD).unwrap();
        let last = blob.len() - 1;
        blob[last] ^= 0x01;
        assert!(aead_decrypt(&key, &blob, DEK_AAD).is_err());
    }

    #[test]
    fn aad_is_part_of_the_tag() {
        let key = random_bytes(DEK_LEN).unwrap();
        let blob = aead_encrypt(&key, b"payload", b"A").unwrap();
        assert!(aead_decrypt(&key, &blob, b"B").is_err());
    }
}
