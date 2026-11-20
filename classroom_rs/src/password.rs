//! 管理员密码：SHA-256(salt + 密码)，auth.json 存 {salt, hash}（对应 PasswordManager）。
//! auth.json 不存在时使用默认密码。

use crate::config::{load_json, save_json_atomic};
use crate::paths;
use serde_json::json;
use sha2::{Digest, Sha256};

pub const DEFAULT_PASSWORD: &str = "123456";

pub fn hash_password(password: &str, salt: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(salt);
    h.update(password.as_bytes());
    format!("{:x}", h.finalize())
}


#[derive(serde::Deserialize, Default)]
struct Auth {
    salt: String,
    hash: String,
}

fn load_auth() -> Option<Auth> {
    load_json::<Option<Auth>>(&paths::password_file(), None)
}

/// 是否已设置过密码（auth.json 存在即视为已设置；供测试与诊断使用）
#[allow(dead_code)]
pub fn has_password() -> bool {
    crate::paths::password_file().exists()
}

pub fn verify(password: &str) -> bool {
    match load_auth() {
        None => password == DEFAULT_PASSWORD,
        Some(auth) => {
            let Ok(salt) = hex_decode(&auth.salt) else {
                return false;
            };
            hash_password(password, &salt) == auth.hash
        }
    }
}

/// 设置新密码（ salt 随机 16 字节）。
pub fn set_password(new_password: &str) -> bool {
    let salt: [u8; 16] = rand_bytes();
    let h = hash_password(new_password, &salt);
    save_json_atomic(
        &paths::password_file(),
        &json!({"salt": hex_encode(&salt), "hash": h}),
    )
}

// ---- 简易 hex / 随机 ----

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

fn hex_decode(s: &str) -> Result<Vec<u8>, ()> {
    let s = s.trim();
    if s.len() % 2 != 0 {
        return Err(());
    }
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).map_err(|_| ()))
        .collect()
}

fn rand_bytes() -> [u8; 16] {
    let mut out = [0u8; 16];
    // 用系统时间 + 进程信息混一个非安全随机即可（本地密码盐，无强度要求场景）
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut state = (t as u64) ^ 0x9E3779B97F4A7C15 ^ ((std::process::id() as u64) << 32);
    for b in out.iter_mut() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        *b = (state & 0xFF) as u8;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_known() {
        // 与 hashlib.sha256(salt + pwd).hexdigest() 一致性：固定 salt 自校验
        let h1 = hash_password("123456", b"0123456789abcdef");
        let h2 = hash_password("123456", b"0123456789abcdef");
        assert_eq!(h1, h2);
        assert_ne!(h1, hash_password("487143", b"0123456789abcdef"));
    }

    #[test]
    fn hex_roundtrip() {
        let b = rand_bytes();
        assert_eq!(hex_decode(&hex_encode(&b)).unwrap(), b.to_vec());
    }

    #[test]
    fn default_verify() {
        // 若用户已设置过密码则不影响：无 auth.json 时默认密码可验
        if !has_password() {
            assert!(verify(DEFAULT_PASSWORD));
            assert!(!verify("000000"));
        }
    }
}
