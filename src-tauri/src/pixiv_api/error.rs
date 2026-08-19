//! エラー型と共有型（pixivpy3.utils の移植版）。

/// PixivAPIで発生したエラー。
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PixivError {
    /// I/Oエラー。
    #[error("I/Oエラー: {0}")]
    Io(#[from] std::io::Error),
    /// Reqwest（HTTPクライアント）エラー。
    #[error("通信エラー: {0}")]
    Reqwest(#[from] reqwest::Error),
    /// API レスポンスが安全上限を超えた場合。
    #[error("APIレスポンスが大きすぎます（上限 {limit_bytes} バイト）")]
    ResponseTooLarge {
        /// 許可した最大レスポンスサイズ。
        limit_bytes: usize,
    },
    /// 認証方法が提供されていないのにトークンが必要な場合。
    #[error("認証が必要ですが、認証情報が提供されていません")]
    NoAuth,
    /// 不正なアクセストークン。
    #[error("アクセストークンが不正です: {message}")]
    BadAccessToken {
        /// 使用されたアクセストークン。
        access_token: String,
        /// 詳細メッセージ。
        message: String,
    },
    /// レスポンスにエラーが含まれている場合。
    #[error("APIレスポンスにエラーが含まれています: {body}")]
    ErrResponse {
        /// レスポンスボディ。
        body: String,
    },
    /// 解析不能なレスポンス。
    #[error("レスポンスを解析できませんでした: {body}")]
    UnintelligibleResponse {
        /// レスポンスボディ。
        body: String,
    },
    /// レートリミット（回数制限）。
    ///
    /// 応答本文は記録には残すが、文面には混ぜない。利用者が読むのは
    /// 「どうすればいいか」であって、取得元が返した JSON ではない。
    #[error("アクセス制限（レートリミット）に達しました。時間をおいてからやり直してください")]
    RateLimited {
        /// レスポンスボディ。
        body: String,
    },
    /// 見つかりません（404）。
    #[error("リソースが見つかりませんでした: {body}")]
    NotFound {
        /// レスポンスボディ。
        body: String,
    },
    /// Serde（デシリアライズ）エラー。
    #[error("デシリアライズエラー: {error}, body: {body}")]
    Serde {
        /// 内部エラー。
        #[source]
        error: serde_json::Error,
        /// レスポンスボディ。
        body: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn io_error_conversion() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "not found");
        let pixiv_err: PixivError = io_err.into();
        assert!(matches!(pixiv_err, PixivError::Io(_)));
        assert!(pixiv_err.to_string().contains("not found"));
    }

    #[test]
    fn display_no_auth() {
        let err = PixivError::NoAuth;
        assert!(err.to_string().contains("認証が必要です"));
    }

    #[test]
    fn display_err_response() {
        let err = PixivError::ErrResponse {
            body: "invalid request".to_string(),
        };
        assert!(err.to_string().contains("invalid request"));
        assert!(err.to_string().contains("エラー"));
    }

    #[test]
    /// 取得制限の文面には、次にどうするかだけを書く。取得元が返した JSON を
    /// そのまま画面へ出しても、読む人にできることは増えない。
    fn display_rate_limited() {
        let err = PixivError::RateLimited {
            body: r#"{"error":{"message":"Rate Limit"}}"#.to_string(),
        };
        let shown = err.to_string();
        assert!(shown.contains("アクセス制限"));
        assert!(shown.contains("時間をおいて"));
        assert!(!shown.contains("{"), "生のレスポンスを混ぜない: {shown}");
    }

    #[test]
    fn bad_access_token_display_never_exposes_the_token() {
        let secret = "oauth-secret-that-must-not-be-logged";
        let err = PixivError::BadAccessToken {
            access_token: secret.to_string(),
            message: "invalid header value".to_string(),
        };
        let displayed = err.to_string();
        assert!(!displayed.contains(secret));
        assert!(displayed.contains("invalid header value"));
    }
}
