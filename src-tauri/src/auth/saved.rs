//! 取得用の接続（pixiv と FANBOX）の置き場。
//!
//! **鍵は Windows の資格情報マネージャーにだけ置き、画面へは渡さない。**
//! 画面に返すのは接続状態・名前・アカウントの識別子である。保存や更新確認は、
//! ここから鍵を読んで Rust の中で取得元へ行く。
//!
//! 取得元ごとに**複数のアカウントを登録でき、使うのはそのうち一つ**である。
//! 保存・更新確認・候補の取得は、どれも「使うアカウント」だけを読む。
//!
//! アカウントひとつにつき一つの資格情報へ、鍵と、その鍵と対でだけ意味を持つ UA と、
//! 画面に出す名前をまとめて置く。別々に置くと、片方だけ書けた状態が生まれる。
//! 登録の一覧（名前と、どれを使うか）は取得元ごとに別の資格情報に置く。一つの
//! 資格情報は 2,560 バイトまでなので、鍵を一覧へまとめると数件で溢れる。
use serde::{Deserialize, Serialize};
use std::sync::{Mutex, MutexGuard};

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

/// 一覧の名前に使えるバイト数。JSON のエスケープを含め、128 バイトの ID が
/// 8 件あっても収まるようにする。
const MAX_NAME_BYTES: usize = 112;
const MIGRATED_KEY: &str = "migrated";
static ACCOUNTS_LOCK: Mutex<()> = Mutex::new(());

fn lock_accounts() -> MutexGuard<'static, ()> {
    ACCOUNTS_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner())
}

/// 取得元ごとの、登録したアカウントの一覧。鍵は含まない。
#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AccountIndex {
    /// 保存・更新確認に使うアカウントの `key`。
    active: Option<String>,
    accounts: Vec<IndexEntry>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IndexEntry {
    key: String,
    /// 取得元での利用者 ID。同じ人を繋ぎ直したとき、項目を増やさず差し替える。
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
    memory: Mutex<std::collections::HashMap<String, Vec<u8>>>,
    #[cfg(test)]
    write_failure: Mutex<Option<String>>,
    #[cfg(test)]
    delete_failure: Mutex<Option<String>>,
    #[cfg(test)]
    write_pause: Mutex<Option<(String, std::sync::Arc<std::sync::Barrier>)>>,
}

impl Vault {
    /// アプリが使う置き場。
    pub fn app() -> Self {
        Self {
            prefix: credential_manager::PREFIX.to_string(),
            #[cfg(test)]
            memory: Default::default(),
            #[cfg(test)]
            write_failure: Default::default(),
            #[cfg(test)]
            delete_failure: Default::default(),
            #[cfg(test)]
            write_pause: Default::default(),
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

    #[cfg(not(test))]
    fn account_names(&self, source: &str) -> Result<Vec<String>, String> {
        credential_manager::names_with_prefix(&self.account_name(source, ""))
    }

    #[cfg(test)]
    fn account_names(&self, source: &str) -> Result<Vec<String>, String> {
        let prefix = self.account_name(source, "");
        Ok(self
            .memory
            .lock()
            .unwrap()
            .keys()
            .filter(|name| name.starts_with(&prefix))
            .cloned()
            .collect())
    }

    #[cfg(test)]
    fn load(&self, name: &str) -> Result<Option<Vec<u8>>, String> {
        Ok(self.memory.lock().unwrap().get(name).cloned())
    }

    #[cfg(test)]
    fn store(&self, name: &str, bytes: &[u8]) -> Result<(), String> {
        let pause = {
            let mut pause = self.write_pause.lock().unwrap();
            if pause.as_ref().is_some_and(|(target, _)| target == name) {
                pause.take()
            } else {
                None
            }
        };
        if let Some((_, barrier)) = pause {
            barrier.wait();
            barrier.wait();
        }
        let mut failure = self.write_failure.lock().unwrap();
        if failure.as_deref() == Some(name) {
            failure.take();
            return Err("書き込みの試験用エラー".into());
        }
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
        let mut failure = self.delete_failure.lock().unwrap();
        if failure.as_deref() == Some(name) {
            failure.take();
            return Err("削除の試験用エラー".into());
        }
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

    fn migrate(&self, source: &str, bytes: &[u8]) -> Result<AccountIndex, String> {
        let name = match source {
            "pixiv" => {
                serde_json::from_slice::<PixivAccount>(bytes).map(|account| account.user_name)
            }
            _ => serde_json::from_slice::<FanboxAccount>(bytes).map(|account| account.user_name),
        }
        .map_err(|_| "保存された接続を読み取れません".to_string())?;
        let index = AccountIndex {
            active: Some(MIGRATED_KEY.to_string()),
            accounts: vec![IndexEntry {
                key: MIGRATED_KEY.to_string(),
                user_id: String::new(),
                name: shorten_name(&name),
            }],
        };
        let index_bytes = encode_index(&index)?;
        // 途中で止まっても、同じ資格情報に書き直してから一覧を確定する。
        self.store(&self.account_name(source, MIGRATED_KEY), bytes)?;
        self.store(&self.index_name(source), &index_bytes)?;
        Ok(index)
    }

    /// 一覧を読み、中断した書き込みの後始末をする。呼び出し元が一覧のロックを持つ。
    fn index(&self, source: &str) -> Result<AccountIndex, String> {
        let index_name = self.index_name(source);
        let single = self.single_name(source);
        let saved_index = self.read_json(&index_name)?;
        let needs_migration = saved_index.is_none();
        let mut index = match saved_index {
            Some(index) => index,
            None => match self.load(&single)? {
                Some(bytes) => self.migrate(source, &bytes)?,
                None => AccountIndex::default(),
            },
        };
        let names = self.account_names(source)?;
        let count = index.accounts.len();
        let active = index.active.clone();
        index
            .accounts
            .retain(|entry| names.contains(&self.account_name(source, &entry.key)));
        if !index
            .accounts
            .iter()
            .any(|entry| Some(&entry.key) == index.active.as_ref())
        {
            index.active = index.accounts.first().map(|entry| entry.key.clone());
        }
        if index.accounts.len() != count || index.active != active {
            self.store(&index_name, &encode_index(&index)?)?;
        }
        if let Err(error) = self.remove_orphans(source, &index, names) {
            log::warn!("{source} の不要な資格情報を削除できません: {error}");
        }
        // 一覧が確定した直後に止まった場合も、古い一件をここで消す。
        if !needs_migration || index.accounts.iter().any(|entry| entry.key == MIGRATED_KEY) {
            if let Err(error) = self.remove(&single) {
                log::warn!("{source} の移行前の資格情報を削除できません: {error}");
            }
        }
        Ok(index)
    }

    fn remove_orphans(
        &self,
        source: &str,
        index: &AccountIndex,
        names: Vec<String>,
    ) -> Result<(), String> {
        for name in names {
            if !index
                .accounts
                .iter()
                .any(|entry| self.account_name(source, &entry.key) == name)
            {
                self.remove(&name)?;
            }
        }
        Ok(())
    }

    /// 使っているアカウントの鍵。登録が無ければ `None`。
    fn read_active<T: for<'de> Deserialize<'de>>(
        &self,
        source: &str,
        index: &AccountIndex,
    ) -> Result<Option<(String, T)>, String> {
        let Some(key) = &index.active else {
            return Ok(None);
        };
        Ok(self
            .read_json(&self.account_name(source, key))?
            .map(|account| (key.clone(), account)))
    }

    fn active<T: for<'de> Deserialize<'de>>(
        &self,
        source: &str,
    ) -> Result<Option<(String, T)>, String> {
        let _lock = lock_accounts();
        self.read_active(source, &self.index(source)?)
    }

    /// アカウントを登録し、使うアカウントにする。
    ///
    /// 同じ利用者 ID がすでにあれば、その項目の鍵を差し替える（繋ぎ直し）。
    fn add<T: Serialize + for<'de> Deserialize<'de>>(
        &self,
        source: &str,
        user_id: &str,
        name: &str,
        account: &T,
        same_session: impl FnOnce(&T, &T) -> bool,
    ) -> Result<(), String> {
        let _lock = lock_accounts();
        let mut index = self.index(source)?;
        let mut existing = index
            .accounts
            .iter()
            .position(|entry| !user_id.is_empty() && entry.user_id == user_id);
        // 移行した接続には利用者 ID が無い。同じ鍵で本人を確認できたときに ID を記録する。
        if existing.is_none() && !user_id.is_empty() {
            if let Some(position) = index
                .accounts
                .iter()
                .position(|entry| entry.key == MIGRATED_KEY)
            {
                let previous = self.read_json(&self.account_name(source, MIGRATED_KEY))?;
                if previous.is_some_and(|previous| same_session(&previous, account)) {
                    existing = Some(position);
                }
            }
        }
        if existing.is_none() && index.accounts.len() >= MAX_ACCOUNTS {
            return Err(format!(
                "登録できるアカウントは{MAX_ACCOUNTS}件までです。使わないものを解除してください"
            ));
        }
        for entry in &mut index.accounts {
            entry.name = shorten_name(&entry.name);
        }
        let key = new_key();
        let entry = IndexEntry {
            key: key.clone(),
            user_id: user_id.to_string(),
            name: shorten_name(name),
        };
        let previous = match existing {
            Some(position) => Some(std::mem::replace(&mut index.accounts[position], entry)),
            None => {
                index.accounts.push(entry);
                None
            }
        };
        index.active = Some(key.clone());
        let index_bytes = encode_index(&index)?;
        let account_bytes =
            serde_json::to_vec(account).map_err(|_| "接続を保存できません".to_string())?;
        // 新しい鍵を用意してから一覧を切り替え、最後に古い鍵を片付ける。
        self.store(&self.account_name(source, &key), &account_bytes)?;
        self.store(&self.index_name(source), &index_bytes)?;
        if let Some(previous) = previous {
            self.remove(&self.account_name(source, &previous.key))?;
        }
        Ok(())
    }

    pub fn pixiv(&self) -> Result<Option<PixivAccount>, String> {
        Ok(self.active("pixiv")?.map(|(_, account)| account))
    }

    /// pixiv のアカウントを登録して使う。`user_id` は pixiv の利用者 ID。
    pub fn add_pixiv(&self, user_id: &str, account: &PixivAccount) -> Result<(), String> {
        self.add("pixiv", user_id, &account.user_name, account, |old, new| {
            old.refresh_token == new.refresh_token
        })
    }

    /// FANBOX のアカウントを登録して使う。`user_id` は FANBOX の利用者 ID。
    pub fn add_fanbox(&self, user_id: &str, account: &FanboxAccount) -> Result<(), String> {
        self.add(
            "fanbox",
            user_id,
            &account.user_name,
            account,
            |old, new| old.session_id == new.session_id,
        )
    }

    /// 使うアカウントを切り替える。
    pub fn activate(&self, source: &str, key: &str) -> Result<(), String> {
        let source = known_source(source)?;
        let _lock = lock_accounts();
        let mut index = self.index(source)?;
        if !index.accounts.iter().any(|entry| entry.key == key) {
            return Err("そのアカウントは登録されていません".to_string());
        }
        index.active = Some(key.to_string());
        self.store(&self.index_name(source), &encode_index(&index)?)
    }

    /// アカウントひとつの登録を解く。使っていたものなら、残りの最初を使う。
    pub fn forget(&self, source: &str, key: &str) -> Result<(), String> {
        let source = known_source(source)?;
        let _lock = lock_accounts();
        let mut index = self.index(source)?;
        let count = index.accounts.len();
        index.accounts.retain(|entry| entry.key != key);
        if index.accounts.len() == count {
            return Ok(());
        }
        if index.active.as_deref() == Some(key) {
            index.active = index.accounts.first().map(|entry| entry.key.clone());
        }
        // 一覧から先に外す。鍵だけが先に消えると、一覧に読めない項目が残る。
        self.store(&self.index_name(source), &encode_index(&index)?)?;
        self.remove(&self.account_name(source, key))
    }

    fn source_status<T: for<'de> Deserialize<'de>>(
        &self,
        source: &str,
    ) -> Result<(Option<T>, Vec<AccountSummary>), String> {
        let index = self.index(source)?;
        let active = self
            .read_active(source, &index)?
            .map(|(_, account)| account);
        let accounts = index
            .accounts
            .into_iter()
            .map(|entry| AccountSummary {
                active: index.active.as_deref() == Some(entry.key.as_str()),
                key: entry.key,
                name: entry.name,
            })
            .collect();
        Ok((active, accounts))
    }

    /// 読めなくなった一覧と、その取得元に属する資格情報を消す。
    pub fn clear_unreadable(&self, source: &str) -> Result<(), String> {
        let source = known_source(source)?;
        let _lock = lock_accounts();
        let readable = match source {
            "pixiv" => self.source_status::<PixivAccount>(source).is_ok(),
            _ => self.source_status::<FanboxAccount>(source).is_ok(),
        };
        if readable {
            return Err("接続一覧は読み取れます。個別に接続を解除してください".into());
        }
        self.remove_orphans(
            source,
            &AccountIndex::default(),
            self.account_names(source)?,
        )?;
        self.remove(&self.single_name(source))?;
        self.remove(&self.index_name(source))
    }

    /// 取得元ごとの接続の様子。片方の保存された接続が読めなくても、もう片方の様子は返す
    /// （読めなかった方は `unreadable` に載せる）。
    pub fn status(&self) -> ConnectionStatus {
        fn readable<T>(
            source: &'static str,
            read: Result<(Option<T>, Vec<AccountSummary>), String>,
            unreadable: &mut Vec<&'static str>,
        ) -> (Option<T>, Vec<AccountSummary>) {
            read.unwrap_or_else(|error| {
                log::warn!("{source} の保存された接続を読めません: {error}");
                unreadable.push(source);
                (None, Vec::new())
            })
        }
        let _lock = lock_accounts();
        let mut unreadable = Vec::new();
        let (pixiv, pixiv_accounts) = readable(
            "pixiv",
            self.source_status::<PixivAccount>("pixiv"),
            &mut unreadable,
        );
        let (fanbox, fanbox_accounts) = readable(
            "fanbox",
            self.source_status::<FanboxAccount>("fanbox"),
            &mut unreadable,
        );
        ConnectionStatus {
            accounts: AccountLists {
                pixiv: pixiv_accounts,
                fanbox: fanbox_accounts,
            },
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
    pub fn fanbox_session(&self) -> Result<(String, FanboxAccount), String> {
        self.active("fanbox")?
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
        let fanbox = self
            .active::<FanboxAccount>("fanbox")
            .unwrap_or_else(|error| {
                log::warn!("FANBOX の接続を読めません: {error}");
                None
            });
        let web = pixiv
            .as_ref()
            .and_then(|account| account.web_session.clone());
        let (fanbox_account_key, fanbox) = fanbox.unzip();
        UpdateCredentials {
            pixiv_refresh_token: pixiv.map(|account| account.refresh_token),
            pixiv_cookie: web.as_ref().map(|session| session.cookie.clone()),
            pixiv_user_agent: web.map(|session| session.user_agent),
            fanbox_cookie: fanbox.as_ref().map(|account| account.session_id.clone()),
            fanbox_account_key,
            fanbox_user_agent: Some(
                fanbox
                    .map(|account| account.user_agent)
                    .unwrap_or_else(|| DEFAULT_USER_AGENT.to_string()),
            ),
        }
    }
}

fn encode_index(index: &AccountIndex) -> Result<Vec<u8>, String> {
    let bytes = serde_json::to_vec(index).map_err(|_| "接続一覧を保存できません".to_string())?;
    if bytes.len() > credential_manager::MAX_BLOB {
        return Err("接続一覧が保存できる大きさを超えています".into());
    }
    Ok(bytes)
}

fn shorten_name(name: &str) -> String {
    let mut end = name.floor_char_boundary(MAX_NAME_BYTES);
    // JSON の引用符二つは名前の外側として数える。
    while serde_json::to_string(&name[..end])
        .expect("文字列は JSON に変換できる")
        .len()
        > MAX_NAME_BYTES + 2
    {
        end = name.floor_char_boundary(end - 1);
    }
    name[..end].to_string()
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
        let emoji_name = format!("a{}", "🦉".repeat(60));
        let escaped_name = "\u{0000}\"\\".repeat(40);
        for id in 0..MAX_ACCOUNTS {
            vault
                .add_fanbox(
                    &format!("{id:0128}"),
                    &FanboxAccount {
                        session_id: "s".into(),
                        user_agent: "UA".into(),
                        user_name: if id % 2 == 0 {
                            emoji_name.clone()
                        } else {
                            escaped_name.clone()
                        },
                    },
                )
                .unwrap();
        }
        let accounts = vault.status().accounts.fanbox;
        assert_eq!(accounts.len(), MAX_ACCOUNTS);
        for (id, account) in accounts.iter().enumerate() {
            let name = if id % 2 == 0 {
                &emoji_name
            } else {
                &escaped_name
            };
            assert!(name.starts_with(&account.name));
            assert!(serde_json::to_string(&account.name).unwrap().len() <= MAX_NAME_BYTES + 2);
        }
        assert_eq!(accounts[0].name, format!("a{}", "🦉".repeat(27)));
        assert_eq!(accounts[1].name, "\u{0000}\"\\".repeat(11));
        vault
            .add_fanbox(
                &format!("{:0128}", 1),
                &FanboxAccount {
                    session_id: "new".into(),
                    user_agent: "UA".into(),
                    user_name: emoji_name,
                },
            )
            .unwrap();
        let replacement = vault.status().accounts.fanbox;
        assert_eq!(replacement.len(), MAX_ACCOUNTS);
        assert_ne!(replacement[1].key, accounts[1].key);
        assert_eq!(vault.fanbox_session().unwrap().1.session_id, "new");
        assert!(!vault
            .account_names("fanbox")
            .unwrap()
            .contains(&vault.account_name("fanbox", &accounts[1].key)));
    }

    #[test]
    fn a_list_at_the_byte_limit_fits_and_a_larger_list_writes_nothing() {
        let vault = Vault::app();
        let empty_id = AccountIndex {
            active: Some("0".repeat(32)),
            accounts: vec![IndexEntry {
                key: "0".repeat(32),
                user_id: String::new(),
                name: "人".into(),
            }],
        };
        let id = "1".repeat(credential_manager::MAX_BLOB - encode_index(&empty_id).unwrap().len());
        let account = pixiv_account("t", "人");
        assert!(vault.add_pixiv(&format!("{id}1"), &account).is_err());
        assert!(vault.account_names("pixiv").unwrap().is_empty());
        assert!(vault.load(&vault.index_name("pixiv")).unwrap().is_none());
        vault.add_pixiv(&id, &account).unwrap();
        assert_eq!(
            vault
                .load(&vault.index_name("pixiv"))
                .unwrap()
                .unwrap()
                .len(),
            credential_manager::MAX_BLOB
        );
    }

    #[test]
    fn interrupted_add_and_replacement_leave_the_old_account_intact() {
        let vault = Vault::app();
        vault.add_pixiv("1", &pixiv_account("old", "表")).unwrap();
        let old = vault.status().accounts.pixiv;
        for id in ["2", "1"] {
            *vault.write_failure.lock().unwrap() = Some(vault.index_name("pixiv"));
            assert!(vault.add_pixiv(id, &pixiv_account("new", "裏")).is_err());
            assert_eq!(vault.account_names("pixiv").unwrap().len(), 2);
            assert_eq!(vault.status().accounts.pixiv, old);
            assert_eq!(vault.pixiv_token().unwrap(), "old");
            assert_eq!(vault.account_names("pixiv").unwrap().len(), 1);
        }
    }

    #[test]
    fn interrupted_delete_is_completed_after_replacement_or_removal() {
        let vault = Vault::app();
        vault.add_pixiv("1", &pixiv_account("old", "表")).unwrap();
        let old = vault.status().accounts.pixiv[0].key.clone();
        let old_name = vault.account_name("pixiv", &old);
        *vault.delete_failure.lock().unwrap() = Some(old_name.clone());
        assert!(vault.add_pixiv("1", &pixiv_account("new", "裏")).is_err());
        assert!(vault.load(&old_name).unwrap().is_some());
        assert_eq!(vault.pixiv_token().unwrap(), "new");
        assert!(vault.load(&old_name).unwrap().is_none());

        let new = vault.status().accounts.pixiv[0].key.clone();
        let new_name = vault.account_name("pixiv", &new);
        *vault.write_failure.lock().unwrap() = Some(vault.index_name("pixiv"));
        assert!(vault.forget("pixiv", &new).is_err());
        assert_eq!(vault.pixiv_token().unwrap(), "new");
        *vault.delete_failure.lock().unwrap() = Some(new_name.clone());
        assert!(vault.forget("pixiv", &new).is_err());
        assert!(vault.load(&new_name).unwrap().is_some());
        assert!(vault.status().accounts.pixiv.is_empty());
        assert!(vault.load(&new_name).unwrap().is_none());
    }

    #[test]
    fn a_missing_secret_is_dropped_and_status_uses_the_repaired_active_account() {
        let vault = Vault::app();
        vault.add_pixiv("1", &pixiv_account("one", "表")).unwrap();
        vault.add_pixiv("2", &pixiv_account("two", "裏")).unwrap();
        let rows = vault.status().accounts.pixiv;
        vault
            .remove(&vault.account_name("pixiv", &rows[1].key))
            .unwrap();
        let status = vault.status();
        assert_eq!(status.pixiv.unwrap().name, "表");
        assert_eq!(
            status.accounts.pixiv,
            vec![AccountSummary {
                active: true,
                ..rows[0].clone()
            }]
        );
        let repaired: AccountIndex = vault
            .read_json(&vault.index_name("pixiv"))
            .unwrap()
            .unwrap();
        assert_eq!(repaired.accounts.len(), 1);
        assert_eq!(repaired.active.as_ref(), Some(&rows[0].key));
    }

    #[test]
    fn migration_resumes_on_both_sides_of_the_index_write() {
        let vault = Vault::app();
        let single = vault.single_name("pixiv");
        let bytes = serde_json::to_vec(&pixiv_account("old", "表")).unwrap();
        vault.store(&single, &bytes).unwrap();
        *vault.write_failure.lock().unwrap() = Some(vault.index_name("pixiv"));
        assert!(vault.pixiv().is_err());
        let names = vault.account_names("pixiv").unwrap();
        assert_eq!(names.len(), 1);
        *vault.delete_failure.lock().unwrap() = Some(single.clone());
        assert_eq!(vault.pixiv_token().unwrap(), "old");
        assert_eq!(vault.account_names("pixiv").unwrap(), names);
        assert!(vault.load(&single).unwrap().is_some());
        assert_eq!(vault.pixiv_token().unwrap(), "old");
        assert!(vault.load(&single).unwrap().is_none());
    }

    #[test]
    fn reconnecting_with_the_migrated_secret_records_the_confirmed_id() {
        let vault = Vault::app();
        let old = pixiv_account("old", "表");
        vault
            .store(
                &vault.single_name("pixiv"),
                &serde_json::to_vec(&old).unwrap(),
            )
            .unwrap();
        vault.add_pixiv("1", &old).unwrap();
        vault.add_pixiv("1", &pixiv_account("new", "裏")).unwrap();
        assert_eq!(vault.status().accounts.pixiv.len(), 1);
        assert_eq!(vault.pixiv_token().unwrap(), "new");
    }

    #[test]
    fn clearing_a_broken_list_clears_only_its_own_credentials() {
        let vault = Vault::app();
        vault.add_pixiv("1", &pixiv_account("kept", "表")).unwrap();
        assert!(vault.clear_unreadable("pixiv").is_err());
        vault.store(&vault.index_name("fanbox"), b"{").unwrap();
        vault
            .store(&vault.account_name("fanbox", "orphan"), b"secret")
            .unwrap();
        vault.store(&vault.single_name("fanbox"), b"old").unwrap();
        vault.clear_unreadable("fanbox").unwrap();
        assert!(vault.account_names("fanbox").unwrap().is_empty());
        assert!(vault.load(&vault.single_name("fanbox")).unwrap().is_none());
        assert!(vault.status().unreadable.is_empty());
        assert_eq!(vault.pixiv_token().unwrap(), "kept");
    }

    #[test]
    fn concurrent_adds_serialize_the_read_and_write() {
        let vault = Vault::app();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        *vault.write_pause.lock().unwrap() = Some((vault.index_name("pixiv"), barrier.clone()));
        std::thread::scope(|scope| {
            let first = scope.spawn(|| vault.add_pixiv("1", &pixiv_account("one", "表")));
            barrier.wait();
            let locked = ACCOUNTS_LOCK.try_lock().is_err();
            let second = scope.spawn(|| vault.add_pixiv("2", &pixiv_account("two", "裏")));
            barrier.wait();
            assert!(locked);
            first.join().unwrap().unwrap();
            second.join().unwrap().unwrap();
        });
        assert_eq!(vault.status().accounts.pixiv.len(), 2);
        assert_eq!(vault.pixiv_token().unwrap(), "two");
    }
}
