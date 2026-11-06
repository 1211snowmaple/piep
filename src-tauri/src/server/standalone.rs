//! 単独サーバー（`piep-server`）の本体。設定、置き場、待ち受け、コマンド。

use super::admin_assets::ADMIN_ASSETS;
use super::admin_store::TicketKind;
use super::http::admin_web::command_ticket;
use super::http::{
    admin_router, combined_router, reader_router, AdminWeb, ConnectionInfo, ServerContext,
};
use super::install::{self, InstallOptions};
use super::library::{Library, SwappableDb};
use super::store::{Role, ServerStore};
use super::sync_receive::SyncReceiver;
use super::tls::{self, TlsMaterial};
use super::update::{
    self, AfterStop, UpdateController, UpdateHistory, UpdateSource, UpdateWindow, EXIT_APPLY_UPDATE,
};
use super::watchdog;
use super::{pairing_payload, DEFAULT_PORT};
use axum::Router;
use hyper_util::rt::TokioTimer;
use serde::Deserialize;
use std::fs::{self, File};
use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

const ADMIN_PAIRING_LIFETIME: Duration = Duration::from_secs(30 * 60);
const CONFIG_FILE: &str = "server.toml";
/// 要求の頭を送らずに接続を握り続ける相手を切るまでの時間。
const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum TlsSetting {
    /// `tls = "self-signed"`
    Named(String),
    /// `tls = { cert = "...", key = "..." }`
    Files { cert: PathBuf, key: PathBuf },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    #[serde(default = "default_listen")]
    pub listen: SocketAddr,
    pub admin_listen: Option<SocketAddr>,
    #[serde(default)]
    pub public_url: String,
    pub tls: Option<TlsSetting>,
    #[serde(default)]
    pub update: UpdateConfig,
}

/// 自己更新。`install` で置いたときだけ動く。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct UpdateConfig {
    /// false なら、デスクトップ版から頼んだときだけ当てる。
    pub auto: bool,
    /// 自動で当てる時間帯（この機械の時刻）。
    pub window: String,
}

impl Default for UpdateConfig {
    fn default() -> Self {
        Self {
            auto: true,
            window: "03:00-06:00".into(),
        }
    }
}

fn default_listen() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], DEFAULT_PORT))
}

impl ServerConfig {
    /// 平文で待ち受けてよいのはこの機械の中だけ。それ以外は TLS を必須にする。
    pub fn validate(&self) -> Result<(), String> {
        if let Some(TlsSetting::Named(name)) = &self.tls {
            if name != "self-signed" {
                return Err(format!(
                    "{CONFIG_FILE} の tls は \"self-signed\" か cert と key の組にしてください"
                ));
            }
        }
        if self.tls.is_none() {
            for address in std::iter::once(self.listen).chain(self.admin_listen) {
                if !address.ip().is_loopback() {
                    return Err(format!(
                        "{address} で待ち受けるには {CONFIG_FILE} に tls を書いてください。平文で待ち受けられるのは 127.0.0.1 だけです"
                    ));
                }
            }
        }
        UpdateWindow::parse(&self.update.window)?;
        if !self.public_url.is_empty() && !self.public_url.starts_with("https://") {
            return Err(format!(
                "{CONFIG_FILE} の public_url は https:// で始めてください"
            ));
        }
        Ok(())
    }

    pub fn public_url(&self) -> Option<String> {
        (!self.public_url.is_empty()).then(|| self.public_url.clone())
    }
}

/// 置き場の中の場所。
#[derive(Debug, Clone)]
pub struct ServerPaths {
    pub root: PathBuf,
}

impl ServerPaths {
    pub fn new(root: &Path) -> Result<Self, String> {
        let root = std::path::absolute(root)
            .map_err(|error| format!("置き場のパスを解決できません: {error}"))?;
        Ok(Self { root })
    }
    pub fn config(&self) -> PathBuf {
        self.root.join(CONFIG_FILE)
    }
    pub fn library(&self) -> PathBuf {
        self.root.join("library")
    }
    pub fn storage(&self) -> PathBuf {
        self.library().join("downloads")
    }
    pub fn database(&self) -> PathBuf {
        self.library().join("piep.db")
    }
    pub fn trash(&self) -> PathBuf {
        self.root.join("library-trash")
    }
    pub fn incoming(&self) -> PathBuf {
        self.root.join("incoming")
    }
    pub fn state(&self) -> PathBuf {
        self.root.join("state")
    }
    pub fn store(&self) -> PathBuf {
        self.state().join("state.db")
    }
    pub fn lock(&self) -> PathBuf {
        self.state().join("server.lock")
    }
    pub fn versions(&self) -> PathBuf {
        self.root.join("versions")
    }
    pub fn version_dir(&self, version: &str) -> PathBuf {
        self.versions().join(version)
    }
    pub fn current_version(&self) -> PathBuf {
        self.state().join("current-version")
    }
    pub fn previous_version(&self) -> PathBuf {
        self.state().join("previous-version")
    }
    pub fn pending_version(&self) -> PathBuf {
        self.state().join("pending-version")
    }
    pub fn update_record(&self) -> PathBuf {
        self.state().join("update.json")
    }
    pub fn update_history(&self) -> PathBuf {
        self.state().join("update-history.json")
    }
    pub fn update_backup(&self) -> PathBuf {
        self.state().join("update-backup")
    }
    pub fn ready_mark(&self) -> PathBuf {
        self.state().join("ready.json")
    }
    pub fn watchdog_log(&self) -> PathBuf {
        self.state().join("watchdog.log")
    }
    pub fn watchdog_lock(&self) -> PathBuf {
        self.state().join("watchdog.lock")
    }
}

pub fn load_config(paths: &ServerPaths) -> Result<ServerConfig, String> {
    let text = fs::read_to_string(paths.config()).map_err(|_| {
        format!(
            "{} がありません。先に piep-server init を実行してください",
            paths.config().display()
        )
    })?;
    let config: ServerConfig = toml::from_str(&text)
        .map_err(|error| format!("{CONFIG_FILE} を読み取れません: {error}"))?;
    config.validate()?;
    Ok(config)
}

const CONFIG_TEMPLATE: &str = r#"# piep-server の設定。説明は docs/plan/server-mode.md

listen = "127.0.0.1:38777"   # 読み手（携帯）。これ以外のアドレスには tls が要る
# admin_listen = "127.0.0.1:38778"   # 管理者を別の口に分ける。外へ公開するなら分ける
public_url = ""              # 携帯の登録文字列に載せる https の URL。空なら管理者が登録した URL
# tls = "self-signed"        # または { cert = "...", key = "..." }

# [update]                   # install で置いたときの自己更新
# auto = true                # false ならデスクトップ版から操作したときだけ更新する
# window = "03:00-06:00"     # 自動で更新する時間帯
"#;

/// 登録文字列を作って返す。
fn admin_pairing_text(
    store: &ServerStore,
    url: &str,
    fingerprint: Option<&str>,
) -> Result<String, String> {
    let secret = store.issue_pairing(Role::Admin, ADMIN_PAIRING_LIFETIME)?;
    Ok(pairing_payload(Role::Admin, url, fingerprint, &secret))
}

fn default_admin_url(config: &ServerConfig) -> Option<String> {
    let address = config.admin_listen.unwrap_or(config.listen);
    address.ip().is_loopback().then(|| {
        let scheme = if config.tls.is_some() {
            "https"
        } else {
            "http"
        };
        format!("{scheme}://{address}/")
    })
}

/// 自己署名の指紋を、待ち受けを始めずに読む。
async fn tls_material(
    paths: &ServerPaths,
    config: &ServerConfig,
) -> Result<Option<TlsMaterial>, String> {
    match &config.tls {
        None => Ok(None),
        Some(TlsSetting::Named(_)) => tls::self_signed(&paths.state().join("tls")).await.map(Some),
        Some(TlsSetting::Files { cert, key }) => tls::from_files(cert, key).await.map(Some),
    }
}

pub async fn command_init(data: &Path, url: Option<String>) -> Result<String, String> {
    let paths = ServerPaths::new(data)?;
    fs::create_dir_all(paths.state()).map_err(|error| format!("置き場を作れません: {error}"))?;
    fs::create_dir_all(paths.library()).map_err(|error| format!("置き場を作れません: {error}"))?;
    if !paths.config().exists() {
        fs::write(paths.config(), CONFIG_TEMPLATE)
            .map_err(|error| format!("設定を書けません: {error}"))?;
    }
    command_admin_pair(data, url, false).await
}

/// 管理者は 1 台だけで、登録すると前の管理者が接続できなくなる。動いている本拠を
/// 誤って締め出さないよう、すでに管理者がいれば `replace` を求める。
pub async fn command_admin_pair(
    data: &Path,
    url: Option<String>,
    replace: bool,
) -> Result<String, String> {
    let paths = ServerPaths::new(data)?;
    let config = load_config(&paths)?;
    let store = ServerStore::open(&paths.store())?;
    if !replace && store.has_admin()? {
        return Err(ADMIN_EXISTS.to_string());
    }
    let url = url
        .or_else(|| config.public_url())
        .or_else(|| default_admin_url(&config))
        .ok_or("このサーバーへデスクトップ版から届く URL を --url で指定してください")?;
    let material = tls_material(&paths, &config).await?;
    let fingerprint = material
        .as_ref()
        .and_then(|material| material.fingerprint.as_deref());
    admin_pairing_text(&store, &url, fingerprint)
}

pub fn command_status(data: &Path) -> Result<String, String> {
    let paths = ServerPaths::new(data)?;
    let config = load_config(&paths)?;
    let store = ServerStore::open(&paths.store())?;
    let devices = store.list_devices()?;
    let mut lines = vec![
        format!("版: {}", update::VERSION),
        format!("読み手の口: {}", config.listen),
        format!(
            "管理の口: {}",
            config
                .admin_listen
                .map_or("読み手と同じ".to_string(), |address| address
                    .to_string())
        ),
        format!(
            "本棚の世代: {}",
            store
                .meta("generation")?
                .unwrap_or_else(|| "まだ送られていません".into())
        ),
        format!(
            "最後の確定: {}",
            store.meta("committed_at")?.unwrap_or_else(|| "-".into())
        ),
        format!("端末: {} 台", devices.len()),
    ];
    if update::is_managed(&paths) {
        let record = update::UpdateRecord::load(&paths);
        lines.push(format!(
            "更新: {} 最後の確認 {} 最新 {}",
            if config.update.auto {
                "自動"
            } else {
                "頼まれたときだけ"
            },
            record.last_checked_at.as_deref().unwrap_or("-"),
            record.latest.as_deref().unwrap_or("-"),
        ));
        if let Some(error) = record.last_error {
            lines.push(format!("更新の問題: {error}"));
        }
    }
    for device in devices {
        lines.push(format!(
            "  {} {} 最後の接続 {}",
            device.role.as_str(),
            device.name,
            device.last_seen_at.as_deref().unwrap_or("-")
        ));
    }
    Ok(lines.join("\n"))
}

/// 組み立て済みのサーバー。テストと `serve` の両方で使う。
pub struct StandaloneParts {
    pub context: Arc<ServerContext>,
    pub receiver: Arc<SyncReceiver>,
    pub db: Arc<SwappableDb>,
}

pub fn assemble(
    paths: &ServerPaths,
    public_url: Option<String>,
    fingerprint: Option<String>,
) -> Result<StandaloneParts, String> {
    fs::create_dir_all(paths.storage()).map_err(|error| format!("置き場を作れません: {error}"))?;
    fs::create_dir_all(paths.incoming()).map_err(|error| format!("置き場を作れません: {error}"))?;
    let store = Arc::new(ServerStore::open(&paths.store())?);
    let db = Arc::new(SwappableDb::new(paths.database(), paths.storage()));
    let receiver = Arc::new(SyncReceiver::new(
        paths.library(),
        paths.trash(),
        paths.incoming(),
        Arc::clone(&store),
        Arc::clone(&db),
    ));
    receiver.recover_pending_publication()?;
    db.open_existing()?;
    let context = ServerContext::new(
        Library::Swappable(Arc::clone(&db)),
        store,
        paths.library(),
        Some(Arc::clone(&receiver)),
        public_url,
        fingerprint,
    );
    Ok(StandaloneParts {
        context,
        receiver,
        db,
    })
}

/// 口を 1 つ、終わるまで動かす。
pub async fn run_listener(
    address: SocketAddr,
    tls: Option<axum_server::tls_rustls::RustlsConfig>,
    router: Router,
    handle: axum_server::Handle<SocketAddr>,
) -> Result<(), String> {
    if tls.is_none() && !address.ip().is_loopback() {
        return Err(format!("{address} は平文で待ち受けられません"));
    }
    let listener = TcpListener::bind(address)
        .map_err(|error| format!("{address} で待ち受けられません: {error}"))?;
    listener
        .set_nonblocking(true)
        .map_err(|error| format!("{address} を設定できません: {error}"))?;
    let service = router.into_make_service_with_connect_info::<SocketAddr>();
    let served = match tls {
        Some(config) => {
            let mut server = axum_server::from_tcp_rustls(listener, config)
                .map_err(|error| format!("{address} で待ち受けられません: {error}"))?
                .handle(handle);
            server
                .http_builder()
                .http1()
                .timer(TokioTimer::new())
                .header_read_timeout(HEADER_READ_TIMEOUT);
            server.serve(service).await
        }
        None => {
            let mut server = axum_server::from_tcp(listener)
                .map_err(|error| format!("{address} で待ち受けられません: {error}"))?
                .handle(handle);
            server
                .http_builder()
                .http1()
                .timer(TokioTimer::new())
                .header_read_timeout(HEADER_READ_TIMEOUT);
            server.serve(service).await
        }
    };
    served.map_err(|error| format!("{address} の待ち受けが止まりました: {error}"))
}

/// 自分の口に `hello` を通し、DB を検査してから、準備できた印を書く。
/// 見張り役はこの印を見て、更新が定着したと判断する。
async fn report_ready(paths: ServerPaths, config: ServerConfig) -> Result<(), String> {
    let address = config.admin_listen.unwrap_or(config.listen);
    let host = match address.ip() {
        std::net::IpAddr::V4(ip) if ip.is_unspecified() => "127.0.0.1".to_string(),
        std::net::IpAddr::V6(ip) if ip.is_unspecified() => "[::1]".to_string(),
        std::net::IpAddr::V6(ip) => format!("[{ip}]"),
        std::net::IpAddr::V4(ip) => ip.to_string(),
    };
    let scheme = if config.tls.is_some() {
        "https"
    } else {
        "http"
    };
    let url = format!("{scheme}://{host}:{}/api/v1/hello", address.port());
    // 自分自身への問い合わせなので、証明書は問わない。
    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|error| format!("自己確認の準備ができません: {error}"))?;
    let started = std::time::Instant::now();
    loop {
        let answered = match client.get(&url).send().await {
            Ok(response) if response.status().is_success() => response
                .json::<serde_json::Value>()
                .await
                .is_ok_and(|hello| hello.get("apiVersion").is_some()),
            _ => false,
        };
        if answered {
            break;
        }
        if started.elapsed() > Duration::from_secs(50) {
            return Err(format!("{url} が応答しません"));
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let database = paths.database();
    tokio::task::spawn_blocking(move || check_database(&database))
        .await
        .map_err(|error| format!("DB の検査が異常終了しました: {error}"))??;
    watchdog::write_ready_mark(&paths)
}

fn check_database(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    let connection =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|error| format!("DB を開けません: {error}"))?;
    let result: String = connection
        .query_row("PRAGMA quick_check(1)", [], |row| row.get(0))
        .map_err(|error| format!("DB を検査できません: {error}"))?;
    if result == "ok" {
        Ok(())
    } else {
        Err(format!("DB が壊れています: {result}"))
    }
}

/// 動き出したら、前回との違いを更新の履歴と記録に足す。
fn note_started(paths: &ServerPaths, store: &ServerStore) {
    let before = UpdateHistory::load(paths).entries.len();
    UpdateHistory::reconcile(paths, update::VERSION, chrono::Utc::now());
    let history = UpdateHistory::load(paths);
    if history.entries.len() > before || history.entries.len() == 50 {
        if let Some(entry) = history.entries.first() {
            let detail = format!("{} → {}", entry.from.as_deref().unwrap_or("不明"), entry.to);
            store.audit(
                "update",
                match entry.result.as_str() {
                    "failed" => "更新に失敗して戻した",
                    "rolled_back" => "前の版へ戻した",
                    _ => "更新した",
                },
                "サーバー",
                None,
                entry.result != "failed",
                Some(&detail),
            );
        }
    }
}

fn connection_info(config: &ServerConfig) -> ConnectionInfo {
    ConnectionInfo {
        listen: config.listen.to_string(),
        admin_listen: config.admin_listen.map(|address| address.to_string()),
        public_url: config.public_url(),
        tls: match &config.tls {
            None => "none",
            Some(TlsSetting::Named(_)) => "self-signed",
            Some(TlsSetting::Files { .. }) => "files",
        },
    }
}

/// 管理画面の一度きりのリンクを出す。アカウントがなければ作成、あればログインの券にする。
pub fn command_admin_setup(data: &Path, url: Option<String>) -> Result<String, String> {
    let paths = ServerPaths::new(data)?;
    let config = load_config(&paths)?;
    let store = ServerStore::open(&paths.store())?;
    let kind = if store.admin_account()?.is_some() {
        TicketKind::Login
    } else {
        TicketKind::Setup
    };
    admin_link(&config, &store, url, kind)
}

/// パスワードの再設定のリンクを出す。再設定すると、今のログインはすべて切れる。
pub fn command_admin_password(data: &Path, url: Option<String>) -> Result<String, String> {
    let paths = ServerPaths::new(data)?;
    let config = load_config(&paths)?;
    let store = ServerStore::open(&paths.store())?;
    admin_link(&config, &store, url, TicketKind::Setup)
}

fn admin_link(
    config: &ServerConfig,
    store: &ServerStore,
    url: Option<String>,
    kind: TicketKind,
) -> Result<String, String> {
    if config.admin_listen.is_none() && config.public_url().is_some() {
        return Err(format!(
            "外へ公開している口には管理画面を出しません。{CONFIG_FILE} に admin_listen を書いて、管理者の口を分けてください"
        ));
    }
    if store.admin_web_disabled() {
        return Err("管理画面は止めてあります。admin-web --enable で戻してください".into());
    }
    let base = url
        .or_else(|| default_admin_url(config))
        .ok_or("このサーバーへブラウザから届く URL を --url で指定してください")?;
    let ticket = command_ticket(store, Some(&base), kind)?;
    Ok(format!(
        "{}/admin/#ticket={ticket}",
        base.trim_end_matches('/')
    ))
}

/// 管理画面を止める、または戻す。
pub fn command_admin_web(data: &Path, enable: bool) -> Result<(), String> {
    let paths = ServerPaths::new(data)?;
    let store = ServerStore::open(&paths.store())?;
    store.set_admin_web_disabled(!enable)?;
    if !enable {
        store.delete_admin_sessions_except(None)?;
    }
    store.audit(
        "login",
        if enable {
            "管理画面の再開"
        } else {
            "管理画面の停止"
        },
        "サーバーのコマンド",
        None,
        true,
        None,
    );
    Ok(())
}

/// 本体。終了コードを返す。`EXIT_APPLY_UPDATE` なら、見張り役が新しい版へ切り替える。
pub async fn command_run(data: &Path) -> Result<i32, String> {
    let paths = ServerPaths::new(data)?;
    let config = load_config(&paths)?;
    fs::create_dir_all(paths.state()).map_err(|error| format!("置き場を作れません: {error}"))?;
    // 同じ置き場を 2 つのプロセスで動かさない。
    let lock =
        File::create(paths.lock()).map_err(|error| format!("置き場を確保できません: {error}"))?;
    lock.try_lock()
        .map_err(|_| "この置き場ではすでに piep-server が動いています".to_string())?;
    let material = tls_material(&paths, &config).await?;
    let fingerprint = material
        .as_ref()
        .and_then(|material| material.fingerprint.clone());
    let tls_config = material.map(|material| material.config);
    let parts = assemble(&paths, config.public_url(), fingerprint)?;
    parts.receiver.start_index_catch_up();
    note_started(&paths, &parts.context.store);
    // 外へ公開している口にログイン画面を置かない。
    let web_here = config.admin_listen.is_some() || config.public_url().is_none();
    if web_here {
        parts.context.set_admin_web(AdminWeb::new(
            ADMIN_ASSETS,
            paths.clone(),
            connection_info(&config),
        ));
    }

    let mut tasks = tokio::task::JoinSet::new();
    let handle = axum_server::Handle::new();
    match config.admin_listen {
        Some(admin_address) => {
            let read = reader_router(Arc::clone(&parts.context));
            let admin = admin_router(Arc::clone(&parts.context));
            tasks.spawn(run_listener(
                config.listen,
                tls_config.clone(),
                read,
                handle.clone(),
            ));
            tasks.spawn(run_listener(
                admin_address,
                tls_config,
                admin,
                handle.clone(),
            ));
            log::info!("待ち受け: 読み手 {} / 管理 {admin_address}", config.listen);
        }
        None => {
            let router = if web_here {
                admin_router(Arc::clone(&parts.context))
            } else {
                combined_router(Arc::clone(&parts.context))
            };
            tasks.spawn(run_listener(
                config.listen,
                tls_config,
                router,
                handle.clone(),
            ));
            log::info!("待ち受け: {}", config.listen);
        }
    }
    let updates = if update::is_managed(&paths) {
        let stop_handle = handle.clone();
        let controller = UpdateController::new(
            paths.clone(),
            UpdateSource::fixed(),
            config.update.clone(),
            Box::new(move || stop_handle.graceful_shutdown(Some(Duration::from_secs(5)))),
        )?;
        parts.context.set_update(Arc::clone(&controller));
        tokio::spawn(Arc::clone(&controller).run(Some(Arc::clone(&parts.receiver))));
        Some(controller)
    } else {
        None
    };
    let ready_paths = paths.clone();
    let ready_config = config.clone();
    tokio::spawn(async move {
        if let Err(error) = report_ready(ready_paths, ready_config).await {
            log::error!("起動の確認に失敗しました: {error}");
        }
    });
    let stop_handle = handle.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            stop_handle.graceful_shutdown(Some(Duration::from_secs(5)));
        }
    });
    let mut result = Ok(());
    while let Some(joined) = tasks.join_next().await {
        match joined {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                result = Err(error);
                handle.shutdown();
            }
            Err(error) => {
                result = Err(format!("待ち受けが異常終了しました: {error}"));
                handle.shutdown();
            }
        }
    }
    parts.receiver.stop_index_catch_up();
    result?;
    let Some(after) = updates.and_then(|controller| controller.take_after_stop()) else {
        drop(lock);
        return Ok(0);
    };
    // 口を閉じたあとなので、もう誰も書かない。
    let version = match after {
        AfterStop::Apply(version) => {
            update::back_up_databases(&paths)?;
            version
        }
        // 新しい版で書いた DB は古い版で開けないことがあるので、更新の前の写しへ戻す。
        AfterStop::RollBack(version) => {
            update::restore_databases(&paths)?;
            version
        }
    };
    update::write_atomic(&paths.pending_version(), version.as_bytes())?;
    log::info!("版 {version} へ切り替えるので止まります");
    drop(lock);
    Ok(EXIT_APPLY_UPDATE)
}

const ADMIN_EXISTS: &str = "管理者はすでに登録されています。新しく登録すると、今の管理者は接続できなくなります。管理者の PC を入れ替えるとき、または今の PC の接続を解除したあとだけ、--replace を付けて実行してください。";

const USAGE: &str = "piep-server <command>

  install [--data <dir>] [--expose proxy|direct] [--url <url>] [--no-task]
                                         置き場所と出し方を尋ね、起動時に動くよう登録して動かす
                                         （引数なしで開いたときもこれ）
  uninstall                              起動時の登録を消して止める。置き場所は消さない
  init --data <dir> [--url <url>]        置き場と設定だけを作り、管理者用の登録文字列を出す
  serve --data <dir>                     サーバーを動かす（落ちたら立ち上げ直す）
  admin-pair --data <dir> [--url <url>] [--replace]
                                         管理者用の登録文字列を出し直す。管理者がいるときは
                                         --replace が要る。使うと今の管理者は接続できなくなる
  admin-setup --data <dir> [--url <url>] 管理画面の一度きりのリンクを出す。管理者がまだいなければ、
                                         そのリンクで作る。30 分有効
  admin-password --data <dir> [--url <url>]
                                         パスワードを決め直すリンクを出す。決め直すと今のログインは切れる
  admin-web --data <dir> --enable|--disable
                                         管理画面を戻す、または止める
  status --data <dir>                    状態を出す
  --version                              版を出す

--url はデスクトップ版からこのサーバーへ届く URL。127.0.0.1 だけで待ち受けていて、
別の機械からつなぐなら、トンネルやリバースプロキシの URL を指定する。
--no-task は起動の仕組みを自分で用意するときに指定する。";

#[derive(Default)]
struct Arguments {
    command: String,
    data: Option<PathBuf>,
    url: Option<String>,
    expose: Option<install::Exposure>,
    no_task: bool,
    pause: bool,
    replace: bool,
    enable: bool,
    disable: bool,
}

fn parse_arguments(args: &[String]) -> Result<Arguments, String> {
    let mut iter = args.iter();
    // 引数なしはエクスプローラーから開いたとき。
    let mut parsed = Arguments {
        command: iter.next().cloned().unwrap_or_else(|| "install".into()),
        pause: args.is_empty(),
        ..Arguments::default()
    };
    while let Some(argument) = iter.next() {
        match argument.as_str() {
            "--data" => parsed.data = iter.next().cloned().map(PathBuf::from),
            "--url" => parsed.url = iter.next().cloned(),
            "--expose" => {
                parsed.expose = Some(install::parse_exposure(
                    iter.next().map_or("", String::as_str),
                )?);
            }
            "--no-task" => parsed.no_task = true,
            "--pause" => parsed.pause = true,
            "--replace" => parsed.replace = true,
            "--enable" => parsed.enable = true,
            "--disable" => parsed.disable = true,
            _ => return Err(USAGE.to_string()),
        }
    }
    Ok(parsed)
}

fn required_data(parsed: &Arguments) -> Result<&Path, String> {
    parsed.data.as_deref().ok_or_else(|| USAGE.to_string())
}

/// 登録文字列は、この画面にだけ出す。記録には残さない。
fn print_pairing(payload: &str) {
    println!("デスクトップ版の「設定 → サーバー → サーバーに接続」に貼り付けてください。");
    println!("30 分有効で、一度だけ使えます。人に見せないでください。");
    println!();
    println!("{payload}");
}

/// `piep-server` の入口。プロセスの終了コードを返す。
pub async fn main_from_args(args: Vec<String>) -> i32 {
    match args.first().map(String::as_str) {
        Some("--version") => {
            println!("{}", update::VERSION);
            return 0;
        }
        Some("--help") => {
            println!("{USAGE}");
            return 0;
        }
        // CI が、できた配布物を組み込みの鍵で確かめる。
        Some("verify-package") => {
            let verified = match (args.get(1), args.get(2)) {
                (Some(package), Some(signature)) => {
                    update::verify_package_file(Path::new(package), Path::new(signature))
                }
                _ => Err(USAGE.to_string()),
            };
            return match verified {
                Ok(()) => 0,
                Err(error) => {
                    eprintln!("{error}");
                    1
                }
            };
        }
        _ => {}
    }
    let run = async {
        let parsed = parse_arguments(&args)?;
        match parsed.command.as_str() {
            "install" => {
                let pause = parsed.pause;
                // 管理者権限で開き直した窓は、終わると閉じる。結果を読めるよう止める。
                let mut again = args.clone();
                if !again.iter().any(|argument| argument == "install") {
                    again.insert(0, "install".into());
                }
                if !again.iter().any(|argument| argument == "--pause") {
                    again.push("--pause".into());
                }
                let options = InstallOptions {
                    data: parsed.data,
                    url: parsed.url,
                    expose: parsed.expose,
                    no_task: parsed.no_task,
                };
                if let Some(payload) = install::command_install(options, &again, pause).await? {
                    print_pairing(&payload);
                    install::finish_install(pause);
                }
                Ok(0)
            }
            "uninstall" => {
                let again = vec!["uninstall".to_string(), "--pause".to_string()];
                if install::command_uninstall(&again)? {
                    install::finish_install(parsed.pause);
                }
                Ok(0)
            }
            "init" => {
                let payload = command_init(required_data(&parsed)?, parsed.url.clone()).await?;
                print_pairing(&payload);
                Ok(0)
            }
            "admin-pair" => {
                let payload =
                    command_admin_pair(required_data(&parsed)?, parsed.url.clone(), parsed.replace)
                        .await?;
                if parsed.replace {
                    println!("この文字列で登録すると、今の管理者は接続できなくなります。");
                }
                print_pairing(&payload);
                Ok(0)
            }
            "admin-setup" => {
                let link = command_admin_setup(required_data(&parsed)?, parsed.url.clone())?;
                println!("このリンクをブラウザで開いてください。30 分有効で、一度だけ使えます。人に見せないでください。");
                println!();
                println!("{link}");
                Ok(0)
            }
            "admin-password" => {
                let link = command_admin_password(required_data(&parsed)?, parsed.url.clone())?;
                println!("このリンクでパスワードを決め直してください。30 分有効で、一度だけ使えます。決め直すと、今のログインはすべて切れます。");
                println!();
                println!("{link}");
                Ok(0)
            }
            "admin-web" => {
                if parsed.enable == parsed.disable {
                    return Err(USAGE.to_string());
                }
                command_admin_web(required_data(&parsed)?, parsed.enable)?;
                println!(
                    "{}",
                    if parsed.enable {
                        "管理画面を戻しました。"
                    } else {
                        "管理画面を止めました。今のログインも切りました。"
                    }
                );
                Ok(0)
            }
            "status" => {
                println!("{}", command_status(required_data(&parsed)?)?);
                Ok(0)
            }
            "serve" | "watchdog" => {
                let paths = ServerPaths::new(required_data(&parsed)?)?;
                fs::create_dir_all(paths.state())
                    .map_err(|error| format!("置き場を作れません: {error}"))?;
                if update::is_managed(&paths) {
                    return tokio::task::spawn_blocking(move || watchdog::run(&paths))
                        .await
                        .map_err(|error| format!("見張り役が異常終了しました: {error}"))?;
                }
                crate::logging::install(&paths.state());
                command_run(&paths.root).await
            }
            "run" => {
                let paths = ServerPaths::new(required_data(&parsed)?)?;
                fs::create_dir_all(paths.state())
                    .map_err(|error| format!("置き場を作れません: {error}"))?;
                crate::logging::install(&paths.state());
                command_run(&paths.root).await
            }
            _ => Err(USAGE.to_string()),
        }
    };
    let result: Result<i32, String> = run.await;
    match result {
        Ok(code) => code,
        Err(error) => {
            eprintln!("{error}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(text: &str) -> ServerConfig {
        toml::from_str(text).expect("config")
    }

    #[test]
    fn plain_http_is_only_allowed_on_loopback() {
        assert!(config("").validate().is_ok());
        assert!(config(r#"listen = "127.0.0.1:1234""#).validate().is_ok());
        assert!(config(r#"listen = "0.0.0.0:1234""#).validate().is_err());
        assert!(config(r#"listen = "192.168.1.2:1234""#).validate().is_err());
        assert!(config("admin_listen = \"10.0.0.2:1234\"")
            .validate()
            .is_err());
        assert!(config("listen = \"0.0.0.0:1234\"\ntls = \"self-signed\"")
            .validate()
            .is_ok());
        assert!(config("listen = \"0.0.0.0:1234\"\ntls = \"none\"")
            .validate()
            .is_err());
    }

    #[test]
    fn public_url_must_be_https() {
        assert!(config(r#"public_url = "http://example.com/""#)
            .validate()
            .is_err());
        assert!(config(r#"public_url = "https://example.com/piep/""#)
            .validate()
            .is_ok());
    }

    #[test]
    fn the_config_template_is_valid() {
        assert!(toml::from_str::<ServerConfig>(CONFIG_TEMPLATE)
            .expect("template")
            .validate()
            .is_ok());
    }

    #[test]
    fn unknown_settings_are_rejected() {
        assert!(toml::from_str::<ServerConfig>(r#"lisen = "127.0.0.1:1""#).is_err());
    }
}
