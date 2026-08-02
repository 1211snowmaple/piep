#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum FanboxError {
    #[error("通信エラー: {0}")]
    Reqwest(#[from] reqwest::Error),

    #[error("認証が必要です（セッションが無効または未設定）")]
    NoAuth,

    #[error("FANBOX APIエラー（HTTP {status}）")]
    ApiError { status: u16, body: String },

    #[error("FANBOXのアクセス制限に達しました。時間をおいて再試行してください")]
    RateLimited { body: String },

    #[error("FANBOXの投稿が見つかりませんでした")]
    NotFound { body: String },

    #[error("FANBOX応答の形式を解釈できませんでした: {error}")]
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
