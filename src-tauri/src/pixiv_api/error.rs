//! エラー型と共有型（pixivpy3.utils の移植版）。

/// 取得元の応答を、記録にだけ残す。
///
/// 文面から本文を外したぶん、**どこにも残らなくなっては調べようがない。**
/// 記録は開発者が読むものなので中身をそのまま置くが、長さは切る -
/// 取得ページ全体が入りうるので、切らないとログが一件で埋まる。
pub(crate) fn log_response_body(context: &str, body: &str) {
    const MAX_LOGGED: usize = 2_000;
    let trimmed = body.trim();
    if trimmed.is_empty() {
        log::warn!("{context}: 応答は空でした");
        return;
    }
    let cut = trimmed
        .char_indices()
        .map(|(index, _)| index)
        .take_while(|index| *index <= MAX_LOGGED)
        .last()
        .unwrap_or(0);
    if cut < trimmed.len() {
        log::warn!("{context}: {}…（以下略）", &trimmed[..cut]);
    } else {
        log::warn!("{context}: {trimmed}");
    }
}

/// 中身を表に出さない文字列。
///
/// `PixivError` は `std::error::Error` なので `Debug` を外せない。しかし
/// アクセストークンをそのまま持たせておくと、誰かが `{:?}` を1行書いた瞬間に
/// トークンがログへ落ちる。**持てるが、見せない**形にしておく。
/// `Display` も伏せる - 文面へ混ぜない方針は `RateLimited` と同じ。
#[derive(Clone, PartialEq, Eq)]
pub struct Redacted(String);

impl Redacted {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// 本当に中身が要るときだけ。ログや文面へ渡さないこと。
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Redacted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("<伏せ字>")
    }
}

impl std::fmt::Display for Redacted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("<伏せ字>")
    }
}

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
    /// 直前の接続更新に失敗したばかりで、まだやり直す時期ではない。
    ///
    /// 待っていた取得が順番に同じ失敗を踏み直すのを止めるためのもの。
    /// 相手が落ちているときに、こちらから何度も叩きに行かない。
    #[error("接続の更新に失敗した直後です。しばらく待ってからやり直してください")]
    RefreshCoolingDown,
    /// 次のページの宛先が、取得元とは違う場所を指していた。
    ///
    /// 宛先そのものは文面に混ぜない。読む人にできることは増えないうえ、
    /// 応答が寄越した文字列をそのまま画面へ出すことになる。
    #[error("取得元が想定外の宛先を返したため、続きの取得を中止しました")]
    UntrustedNextUrl,
    /// 不正なアクセストークン。
    #[error("アクセストークンが不正です: {message}")]
    BadAccessToken {
        /// 使用されたアクセストークン。**中身は表に出ない。**
        access_token: Redacted,
        /// 詳細メッセージ。
        message: String,
    },
    /// レスポンスにエラーが含まれている場合。
    ///
    /// 応答本文は記録には残すが、文面には混ぜない（`RateLimited` と同じ方針）。
    #[error("取得元がエラーを返しました。時間をおいてからやり直してください")]
    ErrResponse {
        /// レスポンスボディ。
        body: String,
    },
    /// 解析不能なレスポンス。
    ///
    /// **本文を文面に混ぜない。** ここに入るのは、正規表現が当たらなかった
    /// ときの取得ページ**全体**（最大64MB）でありうる。それがそのまま画面の
    /// エラー文言になっていた。
    #[error("取得元の応答を読み取れませんでした。取得元の作りが変わった可能性があります")]
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
    #[error("取得元に見つかりませんでした。削除されたか、非公開になった可能性があります")]
    NotFound {
        /// レスポンスボディ。
        body: String,
    },
    /// 一覧が、頼んだ数より少なく返ってきた。
    ///
    /// pixiv の web 一覧は、ログインしていないと R-18 の作品を **黙って** 落とす。
    /// `error` は false のまま、件数だけが減る。これを「変わっていない」と読むと、
    /// セッションが切れた日にライブラリ全体が「最新です」と表示される。
    /// 数が合わないことは、それ自体が失敗である。
    #[error("一覧が {requested} 件中 {returned} 件しか返りませんでした。pixivとの接続が切れている可能性があります")]
    PartialListing {
        /// 要求した件数。
        requested: usize,
        /// 返ってきた件数。
        returned: usize,
    },
    /// Serde（デシリアライズ）エラー。
    ///
    /// 応答本文は記録には残すが、文面には混ぜない。読めなかった JSON を
    /// 丸ごと画面に置いても、利用者にできることは増えないし、一覧に並ぶ
    /// 一件が本文まるごとの長さを抱えることになる。
    #[error("取得元の応答を読み取れませんでした: {error}")]
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

    /// 応答本文は文面へ混ぜない。`RateLimited` と同じ方針である。
    ///
    /// `UnintelligibleResponse` に入るのは、正規表現が当たらなかったときの
    /// 取得ページ**全体**でありうる。それが画面のエラー文言になっていた。
    #[test]
    fn response_bodies_never_reach_the_message() {
        for err in [
            PixivError::ErrResponse {
                body: "invalid request".to_string(),
            },
            PixivError::NotFound {
                body: "invalid request".to_string(),
            },
            PixivError::UnintelligibleResponse {
                body: "invalid request".to_string(),
            },
        ] {
            let displayed = err.to_string();
            assert!(
                !displayed.contains("invalid request"),
                "leaked: {displayed}"
            );
            assert!(!displayed.is_empty());
        }
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
    /// 「少なく返ってきた」を、利用者が次にできることの言葉で伝える。
    fn partial_listing_says_what_to_do() {
        let err = PixivError::PartialListing {
            requested: 100,
            returned: 7,
        };
        let shown = err.to_string();
        assert!(shown.contains("100"));
        assert!(shown.contains('7'));
        assert!(shown.contains("接続"));
    }

    #[test]
    fn bad_access_token_display_never_exposes_the_token() {
        let secret = "oauth-secret-that-must-not-be-logged";
        let err = PixivError::BadAccessToken {
            access_token: Redacted::new(secret),
            message: "invalid header value".to_string(),
        };
        let displayed = err.to_string();
        assert!(!displayed.contains(secret));
        assert!(displayed.contains("invalid header value"));
        // `{:?}` でも出ない。ここが漏れの入口になりやすい。
        let debugged = format!("{err:?}");
        assert!(
            !debugged.contains(secret),
            "Debug leaked the token: {debugged}"
        );
    }
}
