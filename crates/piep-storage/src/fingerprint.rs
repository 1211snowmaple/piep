use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;

/// 圧縮前の元バイト列の SHA-256。serde の 32 byte 配列表現も既存形式の一部。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Fingerprint(pub [u8; 32]);

impl Fingerprint {
    pub fn of(bytes: &[u8]) -> Self {
        Self(Sha256::digest(bytes).into())
    }

    pub fn parse(value: &str) -> crate::Result<Self> {
        if value.len() != 64 || !value.bytes().all(|v| v.is_ascii_hexdigit()) {
            return Err(crate::Error::Integrity("不正なファイル識別子".into()));
        }
        let mut bytes = [0; 32];
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)
                .map_err(|_| crate::Error::Integrity("不正なファイル識別子".into()))?;
        }
        Ok(Self(bytes))
    }
}

impl fmt::Display for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for b in self.0 {
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}
