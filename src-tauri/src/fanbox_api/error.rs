#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum FanboxError {
    #[error("通信エラー: {0}")]
    Reqwest(#[from] reqwest::Error),

    #[error("認証が必要です（セッションが無効または未設定）")]
    NoAuth,

    #[error("FANBOX APIエラー (status={status}): {body}")]
    ApiError { status: u16, body: String },

    #[error("アクセス制限（レートリミット）に達しました: {body}")]
    RateLimited { body: String },

    #[error("リソースが見つかりませんでした: {body}")]
    NotFound { body: String },

    #[error("デシリアライズエラー: {error}, body: {body}")]
    Serde {
        #[source]
        error: serde_json::Error,
        body: String,
    },

    #[error("{0}")]
    Other(String),
}

impl From<String> for FanboxError {
    fn from(s: String) -> Self {
        Self::Other(s)
    }
}

impl From<&str> for FanboxError {
    fn from(s: &str) -> Self {
        Self::Other(s.to_string())
    }
}
