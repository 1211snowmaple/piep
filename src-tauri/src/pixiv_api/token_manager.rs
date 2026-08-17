//! Pixiv OAuth認証のトークン管理（未認証、アクセストークンのみ、または自動更新付きリフレッシュトークン）。

use std::{sync::Arc, time::Duration};

use arc_swap::ArcSwapOption;
use chrono::{DateTime, Utc};
use kv_pairs::kv_pairs;
use tokio::sync::Mutex as AsyncMutex;

use crate::pixiv_api::error::PixivError;
use crate::pixiv_api::models::{parse_into, read_response_text_limited, TokenRefreshResult};
use log::{debug, info};

/// Pixiv OAuth トークンエンドポイント。
pub const AUTH_TOKEN_URL: &str = "https://oauth.secure.pixiv.net/auth/token";
/// デフォルトのOAuthクライアントID（Pixiv iOSアプリ）。
pub const DEFAULT_CLIENT_ID: &str = "MOBrBDS8blbauoSck0ZfDbtuzpyT";
/// デフォルトのOAuthクライアントシークレット（Pixiv iOSアプリ）。
pub const DEFAULT_CLIENT_SECRET: &str = "lsACyCD94FhDUtGTXi3QzcFE2uU1hqtDaKeqrdwj";
/// Pixiv認証に使用されるハッシュシークレット。
pub const HASH_SECRET: &str = "28c1fdd170a5204386cb1313c7077b34f83e4aaf4aa829ce78c231e05b0bae2c";
/// トークンリフレッシュ時に送信されるUser-Agent。
pub const AUTH_USER_AGENT: &str = "PixivAndroidApp/5.0.234 (Android 11; Pixel 5)";

/// アクセストークンのデフォルト有効期限（秒）。
pub const DEFAULT_EXPIRES_IN: u64 = 3600;
/// トークンリフレッシュの安全マージン（秒）。
pub const TOKEN_REFRESH_SAFE_MARGIN: u64 = 300;
const MAX_TOKEN_RESPONSE_BYTES: usize = 2 * 1024 * 1024;

fn token_cache_seconds(expires_in: Option<i64>) -> u64 {
    let advertised_seconds = match expires_in {
        Some(seconds) if seconds > 0 => seconds as u64,
        _ => DEFAULT_EXPIRES_IN,
    };
    // A token whose advertised lifetime is shorter than the safety margin is
    // still usable for the request that obtained it, but must not be cached for
    // a later request. `saturating_sub` also avoids a debug panic/release wrap.
    advertised_seconds.saturating_sub(TOKEN_REFRESH_SAFE_MARGIN)
}

/// トークンマネージャー：未認証、アクセストークンのみ、または自動更新付きリフレッシュトークンを保持します。
pub enum TokenManager {
    /// 認証なし。
    NoAuth,
    /// アクセストークンが提供されている状態。
    AccessToken {
        /// アクセストークン。
        access_token: String,
    },
    /// リフレッシュトークンが提供されている状態。
    RefreshToken {
        /// リフレッシュトークン。
        refresh_token: String,
        /// 現在のアクセストークンとその有効期限。
        access_token_and_expires_at: ArcSwapOption<(String, DateTime<Utc>)>,
        /// 更新用ロック。
        update_lock: AsyncMutex<()>,
    },
}

impl TokenManager {
    /// 認証なしのトークンマネージャーを作成します。すべての認証が必要なリクエストは失敗します。
    pub fn new_no_auth() -> Self {
        Self::NoAuth
    }

    /// 既存のアクセストークンからトークンマネージャーを作成します。リフレッシュは行われません。
    pub fn new_from_access_token(access_token: String) -> Self {
        Self::AccessToken { access_token }
    }

    /// リフレッシュトークンからトークンマネージャーを作成します。アクセストークンは必要に応じて取得・更新されます。
    pub fn new_from_refresh_token(refresh_token: String) -> Self {
        Self::RefreshToken {
            refresh_token,
            access_token_and_expires_at: ArcSwapOption::default(),
            update_lock: AsyncMutex::new(()),
        }
    }

    fn try_get_saved_token(
        access_token_and_expires_at: &ArcSwapOption<(String, DateTime<Utc>)>,
    ) -> Result<String, ()> {
        if let Some((access_token, expires_at)) = access_token_and_expires_at.load().as_deref() {
            if *expires_at > Utc::now() {
                return Ok(access_token.clone());
            }
        }
        Err(())
    }

    async fn try_refresh_token(refresh_token: &str) -> Result<(String, DateTime<Utc>), PixivError> {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            // OAuth credentials and refresh tokens must never follow a server
            // redirect to a different destination.
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        let request = client
            .post(AUTH_TOKEN_URL)
            .form(
                &kv_pairs![
                    "client_id" =>  DEFAULT_CLIENT_ID,
                    "client_secret" => DEFAULT_CLIENT_SECRET,
                    "grant_type" => "refresh_token",
                    "include_policy" => "true",
                    "refresh_token" => refresh_token,
                ]
                .content,
            )
            .header("User-Agent", AUTH_USER_AGENT);
        let response = request.send().await?;
        let (_, body) = read_response_text_limited(response, MAX_TOKEN_RESPONSE_BYTES).await?;
        let parsed: TokenRefreshResult = parse_into(body)?;

        let access_token = parsed.access_token;
        let expires_at = Utc::now() + Duration::from_secs(token_cache_seconds(parsed.expires_in));

        Ok((access_token, expires_at))
    }

    /// 現在のアクセストークンを返します。必要に応じてリフレッシュトークンを使用して更新します。
    pub async fn get_access_token(&self) -> Result<String, PixivError> {
        match self {
            Self::NoAuth => Err(PixivError::NoAuth),
            Self::AccessToken { access_token } => Ok(access_token.clone()),
            Self::RefreshToken {
                access_token_and_expires_at,
                update_lock,
                refresh_token,
            } => {
                // 保存されているトークンの取得を試みる
                if let Ok(access_token) = Self::try_get_saved_token(access_token_and_expires_at) {
                    return Ok(access_token);
                }

                debug!("トークンが設定されていないか期限切れです。リフレッシュを試みます。");

                // トークンが設定されていないか期限切れなので、更新を試みる
                let mut _lock = update_lock.lock().await;

                // 他のスレッドが既にトークンを更新していないか確認
                if let Ok(access_token) = Self::try_get_saved_token(access_token_and_expires_at) {
                    debug!("別のスレッドによってトークンが既に更新されました。");
                    return Ok(access_token);
                }

                // トークンのリフレッシュ
                info!("トークンをリフレッシュしています...");
                let (access_token, expires_at) = Self::try_refresh_token(refresh_token).await?;
                info!(
                    "トークンのリフレッシュに成功しました。有効期限: {}",
                    expires_at
                );
                access_token_and_expires_at
                    .store(Some(Arc::new((access_token.clone(), expires_at))));
                Ok(access_token)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_auth_returns_error() {
        let tm = TokenManager::new_no_auth();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(tm.get_access_token());
        assert!(matches!(result, Err(PixivError::NoAuth)));
    }

    #[test]
    fn access_token_returns_token() {
        let tm = TokenManager::new_from_access_token("test_token".into());
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(tm.get_access_token());
        assert_eq!(result.unwrap(), "test_token");
    }

    #[test]
    fn short_token_lifetimes_expire_immediately_without_underflow() {
        assert_eq!(token_cache_seconds(Some(1)), 0);
        assert_eq!(token_cache_seconds(Some(299)), 0);
        assert_eq!(token_cache_seconds(Some(300)), 0);
        assert_eq!(token_cache_seconds(Some(301)), 1);
    }

    #[test]
    fn missing_or_non_positive_lifetime_uses_the_default() {
        let expected = DEFAULT_EXPIRES_IN - TOKEN_REFRESH_SAFE_MARGIN;
        assert_eq!(token_cache_seconds(None), expected);
        assert_eq!(token_cache_seconds(Some(0)), expected);
        assert_eq!(token_cache_seconds(Some(-1)), expected);
    }
}
