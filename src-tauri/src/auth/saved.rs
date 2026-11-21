//! 取得用の接続（pixiv と FANBOX）の置き場。
//!
//! **鍵は Windows の資格情報マネージャーにだけ置き、画面へは渡さない。**
//! 画面が知るのは、繋がっているかどうかと、繋がっている利用者の名前だけで
//! ある。保存や更新確認は、ここから鍵を読んで Rust の中で取得元へ行く。
//!
//! 取得元ごとに一つの資格情報へ、鍵と、その鍵と対でだけ意味を持つ UA と、
//! 画面に出す名前をまとめて置く。別々に置くと、片方だけ書けた状態が生まれる。
//!
//! 前の版は鍵を設定のファイル（`settings.json`）に平文で置いていた。起動時に
//! 一度だけここへ移し、ファイルからは消す（[`move_from_settings`]）。
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::credential_manager;
use crate::database::models::UpdateCredentials;

/// FANBOX の UA が分からないときに送るもの。
pub const DEFAULT_USER_AGENT: &str = "Mozilla/5.0";

/// pixiv との接続。
// **Debug を付けない。** 鍵を持つので、`{:?}` 一つでログへ落ちる。
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PixivAccount {
    pub refresh_token: String,
    /// web のセッション。`user_agent` と対でだけ意味を持つ。
    pub web_session: Option<WebSession>,
    pub user_name: String,
}

/// web のセッション Cookie と、それを受け取ったときの UA。
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebSession {
    pub cookie: String,
    pub user_agent: String,
}

impl WebSession {
    /// 二つが揃っているときだけ作る。片方だけでは Cloudflare に弾かれる。
    pub fn pair(cookie: Option<String>, user_agent: Option<String>) -> Option<Self> {
        match (cookie, user_agent) {
            (Some(cookie), Some(user_agent)) if !cookie.is_empty() && !user_agent.is_empty() => {
                Some(Self { cookie, user_agent })
            }
            _ => None,
        }
    }
}

/// FANBOX との接続。セッション Cookie そのものが鍵になる。
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FanboxAccount {
    pub session_id: String,
    pub user_agent: String,
    pub user_name: String,
}

/// 画面に返す、接続の様子。鍵は含まない。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionStatus {
    pub pixiv: Option<PixivStatus>,
    pub fanbox: Option<FanboxStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PixivStatus {
    pub name: String,
    /// web のセッションも持っているか。無ければ一覧を使った速い確認ができない。
    pub web_session: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FanboxStatus {
    pub name: String,
}

/// 資格情報マネージャーの中の、接続を置く名前の組。
///
/// 試験では別の名前の組を使い、本物の接続に触れない。
pub struct Vault {
    prefix: String,
}

impl Vault {
    /// アプリが使う置き場。
    pub fn app() -> Self {
        Self {
            prefix: credential_manager::PREFIX.to_string(),
        }
    }

    #[cfg(test)]
    fn for_test() -> Self {
        Self {
            prefix: format!(
                "{}/test/{}",
                credential_manager::PREFIX,
                uuid::Uuid::now_v7()
            ),
        }
    }

    fn name(&self, source: &str) -> String {
        format!("{}/{source}-connection", self.prefix)
    }

    fn read<T: for<'de> Deserialize<'de>>(&self, source: &str) -> Result<Option<T>, String> {
        let Some(bytes) = credential_manager::read(&self.name(source))? else {
            return Ok(None);
        };
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|_| "保存された接続を読み取れません".to_string())
    }

    fn write<T: Serialize>(&self, source: &str, value: &T) -> Result<(), String> {
        let bytes = serde_json::to_vec(value).map_err(|error| error.to_string())?;
        credential_manager::write(&self.name(source), &bytes)
    }

    pub fn pixiv(&self) -> Result<Option<PixivAccount>, String> {
        self.read("pixiv")
    }

    pub fn fanbox(&self) -> Result<Option<FanboxAccount>, String> {
        self.read("fanbox")
    }

    pub fn set_pixiv(&self, account: &PixivAccount) -> Result<(), String> {
        self.write("pixiv", account)
    }

    pub fn set_fanbox(&self, account: &FanboxAccount) -> Result<(), String> {
        self.write("fanbox", account)
    }

    /// 接続を解く。`source` は `pixiv` か `fanbox`。
    pub fn forget(&self, source: &str) -> Result<(), String> {
        match source {
            "pixiv" | "fanbox" => credential_manager::delete(&self.name(source)),
            _ => Err(format!("知らない取得元です: {source}")),
        }
    }

    pub fn status(&self) -> Result<ConnectionStatus, String> {
        Ok(ConnectionStatus {
            pixiv: self.pixiv()?.map(|account| PixivStatus {
                name: account.user_name,
                web_session: account.web_session.is_some(),
            }),
            fanbox: self.fanbox()?.map(|account| FanboxStatus {
                name: account.user_name,
            }),
        })
    }

    /// 画面から始まる取得に使う pixiv の更新用トークン。無ければ繋ぐよう言う。
    pub fn pixiv_token(&self) -> Result<String, String> {
        self.pixiv()?
            .map(|account| account.refresh_token)
            .ok_or_else(|| "設定でpixivに接続してください".to_string())
    }

    /// 画面から始まる取得に使う FANBOX のセッションと UA。無ければ繋ぐよう言う。
    pub fn fanbox_session(&self) -> Result<(String, String), String> {
        self.fanbox()?
            .map(|account| (account.session_id, account.user_agent))
            .ok_or_else(|| "設定でFANBOXに接続してください".to_string())
    }

    /// 保存・更新確認に渡す鍵の組。
    ///
    /// 読めなかった取得元は「繋がっていない」として扱う。仕事の側がそれぞれ
    /// 「接続が要る」と知らせるので、ここで全体を止めない。
    pub fn update_credentials(&self) -> UpdateCredentials {
        let pixiv = self.pixiv().unwrap_or_else(|error| {
            log::warn!("pixiv の接続を読めません: {error}");
            None
        });
        let fanbox = self.fanbox().unwrap_or_else(|error| {
            log::warn!("FANBOX の接続を読めません: {error}");
            None
        });
        let web = pixiv
            .as_ref()
            .and_then(|account| account.web_session.clone());
        UpdateCredentials {
            pixiv_refresh_token: pixiv.map(|account| account.refresh_token),
            pixiv_cookie: web.as_ref().map(|session| session.cookie.clone()),
            pixiv_user_agent: web.map(|session| session.user_agent),
            fanbox_cookie: fanbox.as_ref().map(|account| account.session_id.clone()),
            fanbox_user_agent: Some(
                fanbox
                    .map(|account| account.user_agent)
                    .unwrap_or_else(|| DEFAULT_USER_AGENT.to_string()),
            ),
        }
    }
}

/// 前の版が `settings.json` に置いた鍵の名前。移したあと、すべて消す。
const SETTINGS_KEYS: [&str; 7] = [
    "pixiv_refresh_token",
    "pixiv_user",
    "pixiv_cookie",
    "pixiv_user_agent",
    "fanbox_session_id",
    "fanbox_user",
    "fanbox_user_agent",
];

fn text(settings: &Map<String, Value>, key: &str) -> Option<String> {
    settings
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn user_name(settings: &Map<String, Value>, key: &str) -> String {
    settings
        .get(key)
        .and_then(|user| user.get("name"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// `settings.json` に残っている鍵を資格情報マネージャーへ移し、ファイルから消す。
///
/// 画面が設定のファイルを開く前（`setup` の中、窓を作る前）に呼ぶ。開いたあとだと、
/// 画面の側が古い中身を書き戻す。
///
/// 移しきれなかったときはファイルに手を付けず、次の起動でやり直す。
pub fn move_from_settings(vault: &Vault, settings_path: &Path) -> Result<bool, String> {
    let bytes = match std::fs::read(settings_path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(format!("設定のファイルを読めません: {error}")),
    };
    let Ok(Value::Object(mut settings)) = serde_json::from_slice::<Value>(&bytes) else {
        return Ok(false);
    };
    if !SETTINGS_KEYS.iter().any(|key| settings.contains_key(*key)) {
        return Ok(false);
    }
    if let Some(refresh_token) = text(&settings, "pixiv_refresh_token") {
        vault.set_pixiv(&PixivAccount {
            refresh_token,
            web_session: WebSession::pair(
                text(&settings, "pixiv_cookie")
                    .map(|cookie| crate::auth::essential_cookies(&cookie)),
                text(&settings, "pixiv_user_agent"),
            ),
            user_name: user_name(&settings, "pixiv_user"),
        })?;
    }
    if let Some(session_id) = text(&settings, "fanbox_session_id") {
        vault.set_fanbox(&FanboxAccount {
            session_id,
            user_agent: text(&settings, "fanbox_user_agent")
                .unwrap_or_else(|| DEFAULT_USER_AGENT.to_string()),
            user_name: user_name(&settings, "fanbox_user"),
        })?;
    }
    for key in SETTINGS_KEYS {
        settings.remove(key);
    }
    let bytes =
        serde_json::to_vec_pretty(&Value::Object(settings)).map_err(|error| error.to_string())?;
    let temporary = settings_path.with_extension("json.tmp");
    std::fs::write(&temporary, bytes)
        .and_then(|()| std::fs::rename(&temporary, settings_path))
        .map_err(|error| format!("設定のファイルから鍵を消せません: {error}"))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Cleanup(Vault);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = self.0.forget("pixiv");
            let _ = self.0.forget("fanbox");
        }
    }

    fn temp_settings(content: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("piep-saved-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        std::fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn keys_move_out_of_the_settings_file_once() {
        let vault = Cleanup(Vault::for_test());
        let path = temp_settings(
            r#"{
              "theme": "dark",
              "pixiv_refresh_token": "pixiv-token",
              "pixiv_user": {"id": "1", "name": "ぴくし", "profile_image_urls": {}},
              "pixiv_cookie": "_ga=x; PHPSESSID=1_abc; cf_clearance=pass",
              "pixiv_user_agent": "UA/1",
              "fanbox_session_id": "fanbox-session",
              "fanbox_user": {"userId": "2", "name": "ふぁん"},
              "fanbox_user_agent": "UA/2"
            }"#,
        );
        assert!(move_from_settings(&vault.0, &path).unwrap());

        let left: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(left, serde_json::json!({ "theme": "dark" }));

        let credentials = vault.0.update_credentials();
        assert_eq!(
            credentials.pixiv_refresh_token.as_deref(),
            Some("pixiv-token")
        );
        assert_eq!(
            credentials.pixiv_cookie.as_deref(),
            Some("PHPSESSID=1_abc; cf_clearance=pass")
        );
        assert_eq!(credentials.pixiv_user_agent.as_deref(), Some("UA/1"));
        assert_eq!(credentials.fanbox_cookie.as_deref(), Some("fanbox-session"));
        assert_eq!(credentials.fanbox_user_agent.as_deref(), Some("UA/2"));
        assert_eq!(
            vault.0.status().unwrap(),
            ConnectionStatus {
                pixiv: Some(PixivStatus {
                    name: "ぴくし".into(),
                    web_session: true
                }),
                fanbox: Some(FanboxStatus {
                    name: "ふぁん".into()
                }),
            }
        );

        // 二度目は何も起きない。
        assert!(!move_from_settings(&vault.0, &path).unwrap());
    }

    #[test]
    fn a_cookie_without_its_user_agent_is_not_kept() {
        let vault = Cleanup(Vault::for_test());
        let path = temp_settings(r#"{"pixiv_refresh_token": "t", "pixiv_cookie": "PHPSESSID=1"}"#);
        assert!(move_from_settings(&vault.0, &path).unwrap());
        let credentials = vault.0.update_credentials();
        assert_eq!(credentials.pixiv_refresh_token.as_deref(), Some("t"));
        assert_eq!(credentials.pixiv_cookie, None);
        assert_eq!(credentials.pixiv_user_agent, None);
        assert!(!vault.0.status().unwrap().pixiv.unwrap().web_session);
    }

    #[test]
    fn settings_without_keys_are_left_untouched() {
        let vault = Cleanup(Vault::for_test());
        let content = "{\n  \"theme\": \"light\"\n}";
        let path = temp_settings(content);
        assert!(!move_from_settings(&vault.0, &path).unwrap());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), content);
        assert!(!move_from_settings(&vault.0, &path.with_file_name("missing.json")).unwrap());
    }

    #[test]
    fn nothing_connected_reads_as_missing_keys() {
        let vault = Cleanup(Vault::for_test());
        let credentials = vault.0.update_credentials();
        assert_eq!(credentials.pixiv_refresh_token, None);
        assert_eq!(credentials.fanbox_cookie, None);
        assert_eq!(
            credentials.fanbox_user_agent.as_deref(),
            Some(DEFAULT_USER_AGENT)
        );
        assert_eq!(vault.0.status().unwrap(), ConnectionStatus::default());
        vault.0.forget("pixiv").unwrap();
        assert!(vault.0.forget("other").is_err());
    }
}
