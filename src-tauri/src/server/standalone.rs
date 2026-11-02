//! 単独サーバー（`piep-server`）の本体。設定、置き場、待ち受け、コマンド。

use super::http::{combined_router, reader_router, ServerContext};
use super::library::{Library, SwappableDb};
use super::store::{Role, ServerStore};
use super::sync_receive::SyncReceiver;
use super::tls::{self, TlsMaterial};
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
    command_admin_pair(data, url).await
}

pub async fn command_admin_pair(data: &Path, url: Option<String>) -> Result<String, String> {
    let paths = ServerPaths::new(data)?;
    let config = load_config(&paths)?;
    let store = ServerStore::open(&paths.store())?;
    let url = url.or_else(|| default_admin_url(&config)).ok_or(
        "このサーバーへデスクトップ版から届く URL を --url で指定してください（例: --url https://example.ts.net/）",
    )?;
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
        format!("版: {}", env!("CARGO_PKG_VERSION")),
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
    db.open_existing()?;
    let receiver = Arc::new(SyncReceiver::new(
        paths.library(),
        paths.trash(),
        paths.incoming(),
        Arc::clone(&store),
        Arc::clone(&db),
    ));
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
    let service = router.into_make_service();
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

pub async fn command_serve(data: &Path) -> Result<(), String> {
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

    let mut tasks = tokio::task::JoinSet::new();
    let handle = axum_server::Handle::new();
    match config.admin_listen {
        Some(admin_address) => {
            let read = reader_router(Arc::clone(&parts.context));
            let admin = combined_router(Arc::clone(&parts.context));
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
            tasks.spawn(run_listener(
                config.listen,
                tls_config,
                combined_router(Arc::clone(&parts.context)),
                handle.clone(),
            ));
            log::info!("待ち受け: {}", config.listen);
        }
    }
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
    drop(lock);
    result
}

const USAGE: &str = "piep-server <command> --data <dir>

  init --data <dir> [--url <url>]        置き場と設定を作り、管理者用の登録文字列を出す
  serve --data <dir>                     サーバーを動かす
  admin-pair --data <dir> [--url <url>]  管理者用の登録文字列を出し直す
  status --data <dir>                    状態を出す

--url はデスクトップ版からこのサーバーへ届く URL。127.0.0.1 だけで待ち受けていて、
別の機械からつなぐなら、Funnel やリバースプロキシの URL を指定する。";

struct Arguments {
    command: String,
    data: PathBuf,
    url: Option<String>,
}

fn parse_arguments(args: &[String]) -> Result<Arguments, String> {
    let mut iter = args.iter();
    let command = iter.next().ok_or(USAGE)?.clone();
    let mut data = None;
    let mut url = None;
    while let Some(argument) = iter.next() {
        match argument.as_str() {
            "--data" => data = iter.next().cloned().map(PathBuf::from),
            "--url" => url = iter.next().cloned(),
            _ => return Err(USAGE.to_string()),
        }
    }
    Ok(Arguments {
        command,
        data: data.ok_or(USAGE)?,
        url,
    })
}

/// 登録文字列は、この画面にだけ出す。記録には残さない。
fn print_pairing(payload: &str) {
    println!(
        "デスクトップ版の「設定 → サーバー（β）→ 別の機械のサーバーにつなぐ」に貼ってください。"
    );
    println!("30 分有効で、一度だけ使えます。人に見せないでください。");
    println!();
    println!("{payload}");
}

/// `piep-server` の入口。プロセスの終了コードを返す。
pub async fn main_from_args(args: Vec<String>) -> i32 {
    let run = async {
        let parsed = parse_arguments(&args)?;
        match parsed.command.as_str() {
            "init" => {
                let payload = command_init(&parsed.data, parsed.url).await?;
                print_pairing(&payload);
                Ok(())
            }
            "admin-pair" => {
                let payload = command_admin_pair(&parsed.data, parsed.url).await?;
                print_pairing(&payload);
                Ok(())
            }
            "status" => {
                println!("{}", command_status(&parsed.data)?);
                Ok(())
            }
            "serve" => {
                let paths = ServerPaths::new(&parsed.data)?;
                fs::create_dir_all(paths.state())
                    .map_err(|error| format!("置き場を作れません: {error}"))?;
                crate::logging::install(&paths.state());
                command_serve(&parsed.data).await
            }
            _ => Err(USAGE.to_string()),
        }
    };
    match run.await {
        Ok(()) => 0,
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
