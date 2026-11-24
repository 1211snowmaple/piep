use serde::{Deserialize, Serialize};

/// サーバーの HTTP API の版。登録文字列や実体の転送形式の版とは別。
pub const API_VERSION: u32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Reader,
    Admin,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Reader => "reader",
            Self::Admin => "admin",
        }
    }
}

/// 登録に成功した応答。認証鍵を含むので Debug は持たない。
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PairReply {
    pub device_id: String,
    pub token: String,
    pub role: Role,
    pub api_version: u32,
}

#[derive(Debug, PartialEq, Eq)]
pub enum PairReplyError {
    Version,
    Role,
}

impl PairReply {
    pub fn validate(&self, expected: Role) -> Result<(), PairReplyError> {
        if self.api_version != API_VERSION {
            return Err(PairReplyError::Version);
        }
        if self.role != expected {
            return Err(PairReplyError::Role);
        }
        Ok(())
    }
}
