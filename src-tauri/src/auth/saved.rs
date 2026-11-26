//! 取得用の接続（pixiv と FANBOX）の置き場。
//!
//! **鍵は Windows の資格情報マネージャーにだけ置き、画面へは渡さない。**
//! 画面が知るのは、繋がっているかどうかと、繋がっている利用者の名前だけで
//! ある。保存や更新確認は、ここから鍵を読んで Rust の中で取得元へ行く。
//!
//! 取得元ごとに**複数のアカウントを登録でき、使うのはそのうち一つ**である。
//! 保存・更新確認・候補の取得は、どれも「使うアカウント」だけを読む。
//!
//! アカウントひとつにつき一つの資格情報へ、鍵と、その鍵と対でだけ意味を持つ UA と、
//! 画面に出す名前をまとめて置く。別々に置くと、片方だけ書けた状態が生まれる。
//! 登録の一覧（名前と、どれを使うか）は取得元ごとに別の資格情報に置く。一つの
//! 資格情報は 2,560 バイトまでなので、鍵を一覧へまとめると数件で溢れる。
use serde::{Deserialize, Serialize};

use crate::credential_manager;
use crate::models::UpdateCredentials;

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
///
/// `pixiv`・`fanbox` は使っているアカウントの様子、`accounts` は登録したもの全部。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionStatus {
    pub pixiv: Option<PixivStatus>,
    pub fanbox: Option<FanboxStatus>,
    pub accounts: AccountLists,
    /// 保存された接続を読めなかった取得元（`pixiv`・`fanbox`）。繋がっていないものとして
    /// 扱い、ほかの取得元の様子やログインの結果は巻き込まない。
    pub unreadable: Vec<&'static str>,
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

/// 一つの取得元に登録できるアカウントの数。一覧を一つの資格情報に収めるため。
pub const MAX_ACCOUNTS: usize = 8;

/// 一覧に置く名前の長さ。画面に出すのにも十分な長さ。
const MAX_NAME_CHARS: usize = 60;

/// 取得元ごとの、登録したアカウントの一覧。鍵は含まない。
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AccountIndex {
    /// 保存・更新確認に使うアカウントの `key`。
    active: Option<String>,
    accounts: Vec<IndexEntry>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IndexEntry {
    key: String,
    /// 取得元での利用者 ID。同じ人を繋ぎ直したとき、項目を増やさず差し替える。
    #[serde(default)]
    user_id: String,
    name: String,
}

/// 画面に返す、登録したアカウントひとつ。鍵は含まない。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountSummary {
    pub key: String,
    pub name: String,
    /// 保存・更新確認に使っているアカウントか。
    pub active: bool,
}

/// 取得元ごとの、登録したアカウントの一覧。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountLists {
    pub pixiv: Vec<AccountSummary>,
    pub fanbox: Vec<AccountSummary>,
}

fn known_source(source: &str) -> Result<&'static str, String> {
    match source {
        "pixiv" => Ok("pixiv"),
        "fanbox" => Ok("fanbox"),
        _ => Err(format!("知らない取得元です: {source}")),
    }
}

/// 資格情報マネージャーの中の、接続を置く名前の組。
///
/// 試験では資格情報マネージャーに触れず、手元の入れ物に置く。資格情報マネージャーは
/// 別々のプロセスから同時に書くと更新を取りこぼす（書けたと返った直後に読むと無い）。
/// 試験はプロセスごとに並んで走るので、本物に置くと時々落ちるうえ、利用者の本物の
/// 接続まで巻き込みかねない。Windows との受け渡しは `credential_manager` の試験が見る。
pub struct Vault {
    prefix: String,
    #[cfg(test)]
    memory: std::sync::Mutex<std::collections::HashMap<String, Vec<u8>>>,
}

impl Vault {
    /// アプリが使う置き場。
    pub fn app() -> Self {
        Self {
            prefix: credential_manager::PREFIX.to_string(),
            #[cfg(test)]
            memory: Default::default(),
        }
    }

    /// 登録の一覧を置く名前。
    fn index_name(&self, source: &str) -> String {
        format!("{}/{source}-accounts", self.prefix)
    }

    /// アカウントひとつの鍵を置く名前。
    fn account_name(&self, source: &str, key: &str) -> String {
        format!("{}/{source}-account/{key}", self.prefix)
    }

    /// 一つしか登録できなかったころの置き場。最初に読んだとき一覧へ移して消す。
    fn single_name(&self, source: &str) -> String {
        format!("{}/{source}-connection", self.prefix)
    }

    #[cfg(not(test))]
    fn load(&self, name: &str) -> Result<Option<Vec<u8>>, String> {
        credential_manager::read(name)
    }

    #[cfg(not(test))]
    fn store(&self, name: &str, bytes: &[u8]) -> Result<(), String> {
        credential_manager::write(name, bytes)
    }

    #[cfg(not(test))]
    fn remove(&self, name: &str) -> Result<(), String> {
        credential_manager::delete(name)
    }

    #[cfg(test)]
    fn load(&self, name: &str) -> Result<Option<Vec<u8>>, String> {
        Ok(self.memory.lock().unwrap().get(name).cloned())
    }

    #[cfg(test)]
    fn store(&self, name: &str, bytes: &[u8]) -> Result<(), String> {
        if bytes.len() > credential_manager::MAX_BLOB {
            return Err("資格情報マネージャーに置ける大きさを超えています".into());
        }
        self.memory
            .lock()
            .unwrap()
            .insert(name.to_owned(), bytes.to_vec());
        Ok(())
    }

    #[cfg(test)]
    fn remove(&self, name: &str) -> Result<(), String> {
        self.memory.lock().unwrap().remove(name);
        Ok(())
    }

    fn read_json<T: for<'de> Deserialize<'de>>(&self, name: &str) -> Result<Option<T>, String> {
        let Some(bytes) = self.load(name)? else {
            return Ok(None);
        };
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|_| "保存された接続を読み取れません".to_string())
    }

    fn write_json<T: Serialize>(&self, name: &str, value: &T) -> Result<(), String> {
        let bytes = serde_json::to_vec(value).map_err(|error| error.to_string())?;
        self.store(name, &bytes)
    }

    /// 登録の一覧。一つだけの置き場が残っていれば、ここで一覧の一件目へ移す。
    fn index(&self, source: &str) -> Result<AccountIndex, String> {
        if let Some(index) = self.read_json(&self.index_name(source))? {
            return Ok(index);
        }
        let single = self.single_name(source);
        let Some(bytes) = self.load(&single)? else {
            return Ok(AccountIndex::default());
        };
        let value: serde_json::Value = serde_json::from_slice(&bytes)
            .map_err(|_| "保存された接続を読み取れません".to_string())?;
        let name = value
            .get("userName")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        let key = new_key();
        self.store(&self.account_name(source, &key), &bytes)?;
        let index = AccountIndex {
            active: Some(key.clone()),
            accounts: vec![IndexEntry {
                key,
                user_id: String::new(),
                name,
            }],
        };
        self.write_json(&self.index_name(source), &index)?;
        self.remove(&single)?;
        Ok(index)
    }

    /// 使っているアカウントの鍵。登録が無ければ `None`。
    fn active<T: for<'de> Deserialize<'de>>(&self, source: &str) -> Result<Option<T>, String> {
        let index = self.index(source)?;
        let Some(key) = index.active else {
            return Ok(None);
        };
        self.read_json(&self.account_name(source, &key))
    }

    /// アカウントを登録し、使うアカウントにする。
    ///
    /// 同じ利用者 ID がすでにあれば、その項目の鍵を差し替える（繋ぎ直し）。
    fn add<T: Serialize>(
        &self,
        source: &str,
        user_id: &str,
        name: &str,
        account: &T,
    ) -> Result<(), String> {
        // 名前は一覧に入る。長い名前で一覧が一つの資格情報から溢れないよう切る。
        let name: String = name.chars().take(MAX_NAME_CHARS).collect();
        let name = name.as_str();
        let mut index = self.index(source)?;
        let existing = index
            .accounts
            .iter_mut()
            .find(|entry| !user_id.is_empty() && entry.user_id == user_id);
        let key = match existing {
            Some(entry) => {
                entry.name = name.to_string();
                entry.key.clone()
            }
            None => {
                if index.accounts.len() >= MAX_ACCOUNTS {
                    return Err(format!(
                        "登録できるアカウントは{MAX_ACCOUNTS}件までです。使わないものを解除してください"
                    ));
                }
                let key = new_key();
                index.accounts.push(IndexEntry {
                    key: key.clone(),
                    user_id: user_id.to_string(),
                    name: name.to_string(),
                });
                key
            }
        };
        // 鍵を先に書く。一覧だけが先に書けると、鍵の無い項目を「使う」ことになる。
        self.write_json(&self.account_name(source, &key), account)?;
        index.active = Some(key);
        self.write_json(&self.index_name(source), &index)
    }

    pub fn pixiv(&self) -> Result<Option<PixivAccount>, String> {
        self.active("pixiv")
    }

    pub fn fanbox(&self) -> Result<Option<FanboxAccount>, String> {
        self.active("fanbox")
    }

    /// pixiv のアカウントを登録して使う。`user_id` は pixiv の利用者 ID。
    pub fn add_pixiv(&self, user_id: &str, account: &PixivAccount) -> Result<(), String> {
        self.add("pixiv", user_id, &account.user_name, account)
    }

    /// FANBOX のアカウントを登録して使う。`user_id` は FANBOX の利用者 ID。
    pub fn add_fanbox(&self, user_id: &str, account: &FanboxAccount) -> Result<(), String> {
        self.add("fanbox", user_id, &account.user_name, account)
    }

    /// 使うアカウントを切り替える。
    pub fn activate(&self, source: &str, key: &str) -> Result<(), String> {
        let source = known_source(source)?;
        let mut index = self.index(source)?;
        if !index.accounts.iter().any(|entry| entry.key == key) {
            return Err("そのアカウントは登録されていません".to_string());
        }
        index.active = Some(key.to_string());
        self.write_json(&self.index_name(source), &index)
    }

    /// アカウントひとつの登録を解く。使っていたものなら、残りの最初を使う。
    pub fn forget(&self, source: &str, key: &str) -> Result<(), String> {
        let source = known_source(source)?;
        let mut index = self.index(source)?;
        index.accounts.retain(|entry| entry.key != key);
        if index.active.as_deref() == Some(key) {
            index.active = index.accounts.first().map(|entry| entry.key.clone());
        }
        // 一覧から先に外す。鍵だけが先に消えると、一覧に読めない項目が残る。
        self.write_json(&self.index_name(source), &index)?;
        self.remove(&self.account_name(source, key))
    }

    fn summaries(&self, source: &str) -> Result<Vec<AccountSummary>, String> {
        let index = self.index(source)?;
        Ok(index
            .accounts
            .into_iter()
            .map(|entry| AccountSummary {
                active: index.active.as_deref() == Some(entry.key.as_str()),
                key: entry.key,
                name: entry.name,
            })
            .collect())
    }

    /// 取得元ごとの接続の様子。片方の保存された接続が読めなくても、もう片方の様子は返す
    /// （読めなかった方は `unreadable` に載せる）。
    pub fn status(&self) -> ConnectionStatus {
        fn readable<T>(
            source: &'static str,
            read: Result<Option<T>, String>,
            unreadable: &mut Vec<&'static str>,
        ) -> Option<T> {
            read.unwrap_or_else(|error| {
                log::warn!("{source} の保存された接続を読めません: {error}");
                unreadable.push(source);
                None
            })
        }
        let mut unreadable = Vec::new();
        let pixiv = readable("pixiv", self.pixiv(), &mut unreadable);
        let fanbox = readable("fanbox", self.fanbox(), &mut unreadable);
        // 一覧が読めない取得元は、もう読めないと載せてあるので、空の一覧にする。
        let accounts = AccountLists {
            pixiv: self.summaries("pixiv").unwrap_or_default(),
            fanbox: self.summaries("fanbox").unwrap_or_default(),
        };
        ConnectionStatus {
            accounts,
            pixiv: pixiv.map(|account| PixivStatus {
                name: account.user_name,
                web_session: account.web_session.is_some(),
            }),
            fanbox: fanbox.map(|account| FanboxStatus {
                name: account.user_name,
            }),
            unreadable,
        }
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

/// アカウントの項目の名前に使う、推測されにくい印。
fn new_key() -> String {
    uuid::Uuid::now_v7().simple().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_accounts_come_back_as_credentials_and_status() {
        let vault = Vault::app();
        vault
            .add_pixiv(
                "1",
                &PixivAccount {
                    refresh_token: "pixiv-token".into(),
                    web_session: WebSession::pair(
                        Some(crate::auth::essential_cookies(
                            "_ga=x; PHPSESSID=1_abc; cf_clearance=pass",
                        )),
                        Some("UA/1".into()),
                    ),
                    user_name: "ぴくし".into(),
                },
            )
            .unwrap();
        vault
            .add_fanbox(
                "2",
                &FanboxAccount {
                    session_id: "fanbox-session".into(),
                    user_agent: "UA/2".into(),
                    user_name: "ふぁん".into(),
                },
            )
            .unwrap();

        let credentials = vault.update_credentials();
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
        let status = vault.status();
        assert_eq!(
            status.pixiv,
            Some(PixivStatus {
                name: "ぴくし".into(),
                web_session: true
            })
        );
        assert_eq!(
            status.fanbox,
            Some(FanboxStatus {
                name: "ふぁん".into()
            })
        );
        assert!(status.unreadable.is_empty());
        assert_eq!(status.accounts.pixiv.len(), 1);
        assert!(status.accounts.pixiv[0].active);
    }

    #[test]
    fn a_cookie_without_its_user_agent_is_not_kept() {
        assert!(WebSession::pair(Some("PHPSESSID=1".into()), None).is_none());
        let vault = Vault::app();
        vault
            .add_pixiv(
                "1",
                &PixivAccount {
                    refresh_token: "t".into(),
                    web_session: WebSession::pair(Some("PHPSESSID=1".into()), None),
                    user_name: String::new(),
                },
            )
            .unwrap();
        let credentials = vault.update_credentials();
        assert_eq!(credentials.pixiv_refresh_token.as_deref(), Some("t"));
        assert_eq!(credentials.pixiv_cookie, None);
        assert_eq!(credentials.pixiv_user_agent, None);
        assert!(!vault.status().pixiv.unwrap().web_session);
    }

    #[test]
    fn nothing_connected_reads_as_missing_keys() {
        let vault = Vault::app();
        let credentials = vault.update_credentials();
        assert_eq!(credentials.pixiv_refresh_token, None);
        assert_eq!(credentials.fanbox_cookie, None);
        assert_eq!(
            credentials.fanbox_user_agent.as_deref(),
            Some(DEFAULT_USER_AGENT)
        );
        assert_eq!(vault.status(), ConnectionStatus::default());
        vault.forget("pixiv", "none").unwrap();
        assert!(vault.forget("other", "none").is_err());
    }

    /// 片方の保存された接続が壊れていても、もう片方の様子と、その取得元へのログインの
    /// 結果は巻き込まない。
    #[test]
    fn a_broken_connection_does_not_spoil_the_other_source() {
        let vault = Vault::app();
        vault.store(&vault.index_name("fanbox"), b"{").unwrap();
        vault
            .add_pixiv(
                "1",
                &PixivAccount {
                    refresh_token: "t".into(),
                    web_session: None,
                    user_name: "ぴくし".into(),
                },
            )
            .unwrap();
        let status = vault.status();
        assert_eq!(status.pixiv.unwrap().name, "ぴくし");
        assert_eq!(status.fanbox, None);
        assert_eq!(status.unreadable, vec!["fanbox"]);
        // 仕事に渡す鍵も、読めた方は渡す。
        let credentials = vault.update_credentials();
        assert_eq!(credentials.pixiv_refresh_token.as_deref(), Some("t"));
        assert_eq!(credentials.fanbox_cookie, None);
    }

    fn pixiv_account(token: &str, name: &str) -> PixivAccount {
        PixivAccount {
            refresh_token: token.into(),
            web_session: None,
            user_name: name.into(),
        }
    }

    /// 二つ目を足すと、それを使う。切り替えると、仕事に渡す鍵も切り替わる。
    #[test]
    fn accounts_can_be_added_and_switched() {
        let vault = Vault::app();
        vault.add_pixiv("1", &pixiv_account("t1", "表")).unwrap();
        vault.add_pixiv("2", &pixiv_account("t2", "裏")).unwrap();
        assert_eq!(vault.pixiv_token().unwrap(), "t2");
        let accounts = vault.status().accounts.pixiv;
        assert_eq!(
            accounts
                .iter()
                .map(|a| (a.name.as_str(), a.active))
                .collect::<Vec<_>>(),
            vec![("表", false), ("裏", true)]
        );
        vault.activate("pixiv", &accounts[0].key).unwrap();
        assert_eq!(
            vault.update_credentials().pixiv_refresh_token.as_deref(),
            Some("t1")
        );
        assert!(vault.activate("pixiv", "missing").is_err());
        assert!(vault.activate("fanbox", &accounts[0].key).is_err());
    }

    /// 同じ人を繋ぎ直すと、項目を増やさず鍵を差し替える。
    #[test]
    fn reconnecting_the_same_user_replaces_the_key() {
        let vault = Vault::app();
        vault.add_pixiv("1", &pixiv_account("old", "表")).unwrap();
        vault.add_pixiv("2", &pixiv_account("t2", "裏")).unwrap();
        vault
            .add_pixiv("1", &pixiv_account("new", "表の新しい名前"))
            .unwrap();
        let accounts = vault.status().accounts.pixiv;
        assert_eq!(accounts.len(), 2);
        assert_eq!(accounts[0].name, "表の新しい名前");
        assert!(accounts[0].active);
        assert_eq!(vault.pixiv_token().unwrap(), "new");
    }

    /// 使っていたものを解くと残りの最初を使い、全部解くと繋がっていない。
    #[test]
    fn forgetting_the_active_account_falls_back_to_the_next() {
        let vault = Vault::app();
        vault.add_pixiv("1", &pixiv_account("t1", "表")).unwrap();
        vault.add_pixiv("2", &pixiv_account("t2", "裏")).unwrap();
        let accounts = vault.status().accounts.pixiv;
        vault.forget("pixiv", &accounts[1].key).unwrap();
        assert_eq!(vault.pixiv_token().unwrap(), "t1");
        assert!(vault
            .load(&vault.account_name("pixiv", &accounts[1].key))
            .unwrap()
            .is_none());
        vault.forget("pixiv", &accounts[0].key).unwrap();
        assert!(vault.pixiv().unwrap().is_none());
        assert!(vault.status().accounts.pixiv.is_empty());
    }

    #[test]
    fn the_number_of_accounts_is_bounded() {
        let vault = Vault::app();
        for id in 0..MAX_ACCOUNTS {
            vault
                .add_pixiv(&id.to_string(), &pixiv_account("t", "人"))
                .unwrap();
        }
        assert!(vault.add_pixiv("extra", &pixiv_account("t", "人")).is_err());
        // 繋ぎ直しは数に入らない。
        vault.add_pixiv("0", &pixiv_account("t", "人")).unwrap();
    }

    /// 一つしか登録できなかったころの接続は、最初に読んだとき一覧へ移す。
    #[test]
    fn the_single_connection_moves_into_the_list() {
        let vault = Vault::app();
        let old = serde_json::to_vec(&pixiv_account("kept", "前からの人")).unwrap();
        vault.store(&vault.single_name("pixiv"), &old).unwrap();
        assert_eq!(vault.pixiv_token().unwrap(), "kept");
        let accounts = vault.status().accounts.pixiv;
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].name, "前からの人");
        assert!(vault.load(&vault.single_name("pixiv")).unwrap().is_none());
    }

    /// 一覧はいっぱいまで登録しても一つの資格情報に収まる。
    #[test]
    fn a_full_list_fits_in_one_credential() {
        let vault = Vault::app();
        let long_name = "名".repeat(60);
        for id in 0..MAX_ACCOUNTS {
            vault
                .add_fanbox(
                    &format!("{id:020}"),
                    &FanboxAccount {
                        session_id: "s".into(),
                        user_agent: "UA".into(),
                        user_name: long_name.clone(),
                    },
                )
                .unwrap();
        }
        assert_eq!(vault.status().accounts.fanbox.len(), MAX_ACCOUNTS);
    }
}
