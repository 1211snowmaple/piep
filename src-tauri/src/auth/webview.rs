use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, WebviewBuilder, WebviewUrl, WebviewWindowBuilder};
use tokio::sync::{oneshot, Mutex as AsyncMutex};
use url::Url;

use super::fanbox::{check_fanbox_session, FanboxUser};
use super::pixiv::{login_with_code, PixivUser};
use crate::pixiv_api::web::PIXIV_WEB_HOST;

// ---------------------------------------------------------------------------
// Thread communication structures
// ---------------------------------------------------------------------------

// セッションを持つので Debug は付けない（`PixivConnection` と同じ理由）。
#[derive(serde::Serialize, serde::Deserialize)]
pub struct FanboxAuthData {
    pub session: String,
    pub ua: String,
}

const MAX_BROWSER_URL_LENGTH: usize = 16 * 1024;
const MAX_USER_AGENT_LENGTH: usize = 1024;
const MAX_AUTH_VALUE_LENGTH: usize = 8 * 1024;
const MAX_WEBVIEW_COORDINATE: f64 = 1_000_000.0;
const MAX_WEBVIEW_DIMENSION: f64 = 100_000.0;
const DEFAULT_BROWSER_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";
const BROWSER_ACCELERATOR_SCHEME: &str = "piep-browser";

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserAcceleratorEvent {
    pub action: String,
    pub browser: String,
    pub source: String,
    pub url: String,
}

static PIXIV_LOGIN_LOCK: AsyncMutex<()> = AsyncMutex::const_new(());
static FANBOX_LOGIN_LOCK: AsyncMutex<()> = AsyncMutex::const_new(());

/// 同じ一押しを二度届けないための間合い。
const ACCELERATOR_DEDUPE_WINDOW: Duration = Duration::from_millis(700);
static LAST_ACCELERATOR: Mutex<Option<(String, Instant)>> = Mutex::new(None);

/// この知らせは、直前と同じ一押しか。
///
/// Windows では Ctrl+S を二経路で拾っている。ページに差し込んだ keydown が
/// カスタムスキームへ飛ばす道と、WebView2 の AcceleratorKeyPressed である。
/// ページ側でキーが届かない場面（フォーカスが iframe にある、スクリプトが
/// 動く前）があるのでどちらも外せないが、両方鳴ったときは一度だけ渡す。
/// 二度渡ると、候補の取得が二周して同じ知らせが二つ並ぶ。
fn accelerator_is_repeat(previous: Option<&(String, Instant)>, key: &str, now: Instant) -> bool {
    match previous {
        Some((last_key, at)) => {
            last_key == key && now.saturating_duration_since(*at) < ACCELERATOR_DEDUPE_WINDOW
        }
        None => false,
    }
}

fn query_value(url: &Url, key: &str) -> Option<String> {
    url.query_pairs()
        .find_map(|(candidate, value)| (candidate == key).then(|| value.into_owned()))
        .filter(|value| !value.is_empty() && value.len() <= MAX_AUTH_VALUE_LENGTH)
}

fn pixiv_callback_code(url: &Url) -> Option<String> {
    let is_app_callback =
        url.scheme() == "pixiv" && url.host_str() == Some("account") && url.path() == "/login";
    let is_https_callback = url.scheme() == "https"
        && url.host_str() == Some("app-api.pixiv.net")
        && url.path() == "/web/v1/users/auth/pixiv/callback";
    (is_app_callback || is_https_callback)
        .then(|| query_value(url, "code"))
        .flatten()
}

fn fanbox_callback_data(url: &Url) -> Option<FanboxAuthData> {
    if url.scheme() != "fanbox-auth" || url.host_str() != Some("callback") {
        return None;
    }
    let session = query_value(url, "session")?;
    let ua = query_value(url, "ua")?;
    if session.chars().any(char::is_control) || ua.chars().any(char::is_control) {
        return None;
    }
    Some(FanboxAuthData { session, ua })
}

fn parse_browser_url(raw: &str) -> Result<Url, String> {
    if raw.len() > MAX_BROWSER_URL_LENGTH {
        return Err("Browser URL is too long".to_string());
    }
    let url = Url::parse(raw).map_err(|e| format!("Invalid browser URL: {e}"))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err("Only HTTP and HTTPS browser URLs are allowed".to_string());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("Browser URLs containing credentials are not allowed".to_string());
    }
    Ok(url)
}

fn is_safe_browser_navigation(url: &Url) -> bool {
    url.as_str().len() <= MAX_BROWSER_URL_LENGTH
        && matches!(url.scheme(), "http" | "https")
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
}

fn accelerator_action(url: &Url) -> Option<&str> {
    if url.scheme() != BROWSER_ACCELERATOR_SCHEME || url.host_str() != Some("accelerator") {
        return None;
    }
    match url.path() {
        "/save" => Some("save"),
        "/close" => Some("close"),
        _ => None,
    }
}

fn source_for_browser_url(url: &Url) -> &'static str {
    match url
        .host_str()
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "fanbox.cc" | "www.fanbox.cc" | "api.fanbox.cc" => "fanbox",
        host if host.ends_with(".fanbox.cc") => "fanbox",
        _ => "pixiv",
    }
}

fn emit_browser_accelerator(
    app: &AppHandle,
    browser: &str,
    source: &str,
    action: &str,
    current_url: &Url,
) {
    let key = format!("{browser}:{source}:{action}");
    {
        let now = Instant::now();
        let mut last = LAST_ACCELERATOR.lock();
        if accelerator_is_repeat(last.as_ref(), &key, now) {
            return;
        }
        *last = Some((key, now));
    }
    let payload = BrowserAcceleratorEvent {
        action: action.to_string(),
        browser: browser.to_string(),
        source: source.to_string(),
        url: current_url.to_string(),
    };
    if let Some(main) = app.get_webview_window("main") {
        let _ = main.emit("browser-accelerator", payload);
    }
}

/// Runs in remote WebViews without exposing Tauri IPC to those origins. The
/// custom-scheme navigation is cancelled by Rust and converted into a narrow,
/// typed event for the trusted main window.
const BROWSER_ACCELERATOR_SCRIPT: &str = r#"
    (function() {
        const requestHostAction = (action) => {
            window.location.href = 'piep-browser://accelerator/' + action;
        };
        window.addEventListener('keydown', (event) => {
            const key = String(event.key || '').toLowerCase();
            const command = event.ctrlKey || event.metaKey;
            if (event.altKey && key === 'arrowleft') {
                event.preventDefault(); event.stopImmediatePropagation(); history.back(); return;
            }
            if (event.altKey && key === 'arrowright') {
                event.preventDefault(); event.stopImmediatePropagation(); history.forward(); return;
            }
            if (key === 'f5' || (command && key === 'r')) {
                event.preventDefault(); event.stopImmediatePropagation(); location.reload(); return;
            }
            if (command && key === 's') {
                event.preventDefault(); event.stopImmediatePropagation(); requestHostAction('save'); return;
            }
            if (command && key === 'w') {
                event.preventDefault(); event.stopImmediatePropagation(); requestHostAction('close');
            }
        }, true);
    })();
"#;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NativeBrowserAction {
    Back,
    Forward,
    Reload,
    Save,
    Close,
}

fn native_browser_action(key: u32, control: bool, alt: bool) -> Option<NativeBrowserAction> {
    match (key, control, alt) {
        (0x25, false, true) => Some(NativeBrowserAction::Back), // VK_LEFT
        (0x27, false, true) => Some(NativeBrowserAction::Forward), // VK_RIGHT
        (0x74, false, false) => Some(NativeBrowserAction::Reload), // VK_F5
        (0x52, true, false) => Some(NativeBrowserAction::Reload), // Ctrl+R
        (0x53, true, false) => Some(NativeBrowserAction::Save), // Ctrl+S
        (0x57, true, false) => Some(NativeBrowserAction::Close), // Ctrl+W
        _ => None,
    }
}

fn perform_native_browser_action(
    app: &AppHandle,
    label: &str,
    browser: &str,
    source: &str,
    action: NativeBrowserAction,
) {
    let Some(webview) = app.get_webview(label) else {
        return;
    };
    match action {
        NativeBrowserAction::Back => {
            let _ = webview.eval("history.back()");
        }
        NativeBrowserAction::Forward => {
            let _ = webview.eval("history.forward()");
        }
        NativeBrowserAction::Reload => {
            let _ = webview.reload();
        }
        NativeBrowserAction::Save | NativeBrowserAction::Close => {
            if let Ok(current_url) = webview.url() {
                emit_browser_accelerator(
                    app,
                    browser,
                    source,
                    if action == NativeBrowserAction::Save {
                        "save"
                    } else {
                        "close"
                    },
                    &current_url,
                );
            }
            if action == NativeBrowserAction::Close {
                if browser == "standalone" {
                    if let Some(window) = app.get_webview_window(label) {
                        let _ = window.close();
                    }
                } else {
                    let _ = webview.hide();
                }
            }
        }
    }
}

#[cfg(target_os = "windows")]
fn install_native_browser_accelerators(
    webview: &tauri::Webview,
    app: AppHandle,
    label: String,
    browser: &'static str,
    source: String,
) -> Result<(), String> {
    use webview2_com::AcceleratorKeyPressedEventHandler;
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_KEY_EVENT_KIND, COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN,
        COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_DOWN,
    };
    use windows::Win32::UI::Input::KeyboardAndMouse::GetKeyState;

    webview
        .with_webview(move |platform_webview| {
            let controller = platform_webview.controller();
            let callback_app = app.clone();
            let callback_label = label.clone();
            let callback_source = source.clone();
            let handler =
                AcceleratorKeyPressedEventHandler::create(Box::new(move |_sender, args| {
                    let Some(args) = args else {
                        return Ok(());
                    };
                    let mut kind = COREWEBVIEW2_KEY_EVENT_KIND::default();
                    let mut key = 0u32;
                    unsafe {
                        args.KeyEventKind(&mut kind)?;
                        args.VirtualKey(&mut key)?;
                    }
                    if kind != COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN
                        && kind != COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_DOWN
                    {
                        return Ok(());
                    }
                    let control = unsafe { GetKeyState(0x11) } < 0; // VK_CONTROL
                    let alt = unsafe { GetKeyState(0x12) } < 0; // VK_MENU
                    let Some(action) = native_browser_action(key, control, alt) else {
                        return Ok(());
                    };
                    unsafe { args.SetHandled(true)? };
                    perform_native_browser_action(
                        &callback_app,
                        &callback_label,
                        browser,
                        &callback_source,
                        action,
                    );
                    Ok(())
                }));
            let mut token = 0i64;
            if let Err(error) =
                unsafe { controller.add_AcceleratorKeyPressed(&handler, &mut token) }
            {
                log::error!("Failed to install WebView2 accelerator handler: {error}");
            }
        })
        .map_err(|error| error.to_string())
}

#[cfg(not(target_os = "windows"))]
fn install_native_browser_accelerators(
    _webview: &tauri::Webview,
    _app: AppHandle,
    _label: String,
    _browser: &'static str,
    _source: String,
) -> Result<(), String> {
    Ok(())
}

fn validate_webview_bounds(x: f64, y: f64, width: f64, height: f64) -> Result<(), String> {
    if ![x, y, width, height].iter().all(|value| value.is_finite()) {
        return Err("Embedded browser bounds must be finite numbers".to_string());
    }
    if x.abs() > MAX_WEBVIEW_COORDINATE || y.abs() > MAX_WEBVIEW_COORDINATE {
        return Err("Embedded browser position is outside the supported range".to_string());
    }
    if width <= 0.0
        || height <= 0.0
        || width > MAX_WEBVIEW_DIMENSION
        || height > MAX_WEBVIEW_DIMENSION
    {
        return Err("Embedded browser size is outside the supported range".to_string());
    }
    Ok(())
}

fn validate_user_agent(user_agent: Option<String>) -> Result<Option<String>, String> {
    let Some(user_agent) = user_agent else {
        return Ok(None);
    };
    if user_agent.is_empty()
        || user_agent.len() > MAX_USER_AGENT_LENGTH
        || user_agent.chars().any(char::is_control)
    {
        return Err("Invalid embedded browser user agent".to_string());
    }
    Ok(Some(user_agent))
}

// ---------------------------------------------------------------------------
// Pixiv OAuth Login (separate window)
// ---------------------------------------------------------------------------

/// WebView2 が持っている Cookie を、送れる1行にする。
///
/// 拾ってきた山からは[要るものだけを残す](super::essential_cookies)。
/// 解析や広告の識別子を、相手に送ることも手元に残すこともしない。
fn cookie_header(cookies: &[tauri::webview::Cookie<'static>]) -> String {
    let all = cookies
        .iter()
        .map(|cookie| format!("{}={}", cookie.name(), cookie.value()))
        .collect::<Vec<_>>()
        .join("; ");
    super::essential_cookies(&all)
}

/// pixiv と接続したときに受け取るもの。
///
/// トークンだけでなく web のセッションも持ち帰る。pixiv のログイン画面は
/// pixiv 自身のものなので、そこを通った時点で `.pixiv.net` の Cookie は
/// この WebView2 プロファイルに書かれている。**新しく取りに行くのではなく、
/// もう置いてあるものを受け取るだけ。** 利用者の手順は増えない。
// **Debug を付けない。** リフレッシュトークンと Cookie を持つので、誰かが
// 調べもので `{:?}` を1行書いた瞬間に、その中身がログファイルへ落ちる。
// 付いていなければコンパイルが止める。同じ理由で `FanboxAPI` と
// `TokenManager` にも付いていない。
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PixivConnection {
    /// アプリAPI用のリフレッシュトークン。
    pub refresh_token: String,
    /// 接続した利用者。
    pub user: PixivUser,
    /// web の一覧を読むためのセッション。**取れなくても接続は成功とする。**
    pub cookie: Option<String>,
    /// その Cookie を受け取ったときの UA。`cookie` と対でだけ意味を持つ。
    pub user_agent: Option<String>,
}

/// ログイン窓から pixiv のセッションを受け取る。
///
/// 窓を閉じるとプロファイルへの参照ごと消えるので、**閉じる前に読む**。
/// `PHPSESSID` が無ければ「セッションは取れなかった」として `None` を返す。
/// FANBOX 側は Cookie が唯一の資格情報なので見つからなければ失敗にするが、
/// pixiv では Cookie は付け足しであって、接続の条件ではない。
fn take_pixiv_session(window: &tauri::WebviewWindow) -> Option<(String, String)> {
    let url: Url = format!("{PIXIV_WEB_HOST}/").parse().ok()?;
    let cookies = match window.cookies_for_url(url) {
        Ok(cookies) => cookies,
        Err(error) => {
            log::warn!("pixiv のセッションを読めませんでした: {error}");
            return None;
        }
    };
    if !cookies.iter().any(|cookie| cookie.name() == "PHPSESSID") {
        log::info!("pixiv のログインは成功しましたが、PHPSESSID は見つかりませんでした");
        return None;
    }
    Some((
        cookie_header(&cookies),
        DEFAULT_BROWSER_USER_AGENT.to_string(),
    ))
}

pub async fn open_pixiv_login(app: AppHandle) -> Result<PixivConnection, String> {
    let _login_guard = PIXIV_LOGIN_LOCK
        .try_lock()
        .map_err(|_| "A Pixiv login is already in progress".to_string())?;
    let mut random_bytes = [0u8; 32];
    rand::fill(&mut random_bytes);
    let code_verifier = URL_SAFE_NO_PAD.encode(random_bytes);

    let mut hasher = Sha256::new();
    hasher.update(code_verifier.as_bytes());
    let code_challenge = URL_SAFE_NO_PAD.encode(hasher.finalize());

    let login_url = format!(
        "https://app-api.pixiv.net/web/v1/login?client=pixiv-android&code_challenge={}&code_challenge_method=S256",
        code_challenge
    );

    let (tx, rx) = oneshot::channel::<String>();
    let window_label = "pixiv_login_window";

    if let Some(existing) = app.get_webview_window(window_label) {
        existing
            .destroy()
            .map_err(|e| format!("Failed to close stale Pixiv login window: {e}"))?;
    }

    let tx_mutex = Arc::new(Mutex::new(Some(tx)));

    let parsed_login_url = login_url
        .parse()
        .map_err(|e| format!("Failed to parse Pixiv login URL: {}", e))?;
    let mut builder =
        WebviewWindowBuilder::new(&app, window_label, WebviewUrl::External(parsed_login_url));

    let navigation_sender = tx_mutex.clone();
    builder = builder
        .title("Pixiv Login")
        .inner_size(400.0, 700.0)
        // 内蔵ブラウザと同じ UA で名乗る。`cf_clearance` は発行時の UA に
        // 紐づくので、ここで名乗ったものと、あとで一覧を読むときに送るものが
        // 食い違うと弾かれる。既定に任せず、静的に決めておく。
        .user_agent(DEFAULT_BROWSER_USER_AGENT)
        .on_navigation(move |url| {
            if let Some(code) = pixiv_callback_code(url) {
                if let Some(sender) = navigation_sender.lock().take() {
                    let _ = sender.send(code);
                }
                return false;
            }
            true
        });

    let window = builder.build().map_err(|e| e.to_string())?;
    let window_clone = window.clone();
    let cancel_sender = tx_mutex.clone();
    window.on_window_event(move |event| {
        if matches!(
            event,
            tauri::WindowEvent::CloseRequested { .. } | tauri::WindowEvent::Destroyed
        ) {
            cancel_sender.lock().take();
        }
    });

    let code = match tokio::time::timeout(Duration::from_secs(300), rx).await {
        Ok(Ok(c)) => c,
        _ => {
            let _ = window_clone.close();
            return Err("Login timed out or window closed".to_string());
        }
    };

    // 窓を閉じる前に読む。閉じたあとでは、もう聞く相手がいない。
    let session = take_pixiv_session(&window_clone);
    let _ = window_clone.close();

    let auth_res = login_with_code(&code, &code_verifier)
        .await
        .map_err(|e| e.to_string())?;

    let Some(user) = auth_res.user else {
        return Err("Failed to get user info".to_string());
    };
    let (cookie, user_agent) = match session {
        Some((cookie, user_agent)) => (Some(cookie), Some(user_agent)),
        None => (None, None),
    };
    Ok(PixivConnection {
        refresh_token: auth_res.refresh_token,
        user,
        cookie,
        user_agent,
    })
}

// ---------------------------------------------------------------------------
// FANBOX Login (separate window)
// ---------------------------------------------------------------------------

pub async fn open_fanbox_login(app: AppHandle) -> Result<(String, FanboxUser, String), String> {
    let _login_guard = FANBOX_LOGIN_LOCK
        .try_lock()
        .map_err(|_| "A FANBOX login is already in progress".to_string())?;
    let login_url = "https://www.fanbox.cc/login";
    let window_label = "fanbox_login_window";

    if let Some(existing) = app.get_webview_window(window_label) {
        existing
            .destroy()
            .map_err(|e| format!("Failed to close stale FANBOX login window: {e}"))?;
    }

    let (tx, rx) = oneshot::channel::<FanboxAuthData>();
    let tx_mutex = Arc::new(Mutex::new(Some(tx)));

    // FANBOXSESSID は HttpOnly なので document.cookie から読めない。
    // fetch() でAPIを叩いてログイン検知し、成功したらカスタムスキームでRust側に通知する。
    let init_script = r#"
        (function() {
            let _fanboxCheckStarted = false;

            function checkFanboxLogin() {
                if (_fanboxCheckStarted) return;
                const hostname = window.location.hostname.toLowerCase();
                if (hostname !== 'fanbox.cc' && !hostname.endsWith('.fanbox.cc')) return;

                _fanboxCheckStarted = true;

                const interval = setInterval(async () => {
                    try {
                        const res = await fetch('https://api.fanbox.cc/creator.listFollowing', {
                            credentials: 'include',
                            headers: {
                                'Accept': 'application/json'
                            }
                        });

                        if (res.ok) {
                            let sessid = '';
                            const cookieMatch = document.cookie.match(/FANBOXSESSID=([^;]+)/);
                            if (cookieMatch && cookieMatch[1]) {
                                sessid = cookieMatch[1];
                            }

                            if (!sessid) {
                                sessid = '__httponly_extract_needed__';
                            }

                            clearInterval(interval);
                            const ua = navigator.userAgent;
                            window.location.href = 'fanbox-auth://callback?session=' + encodeURIComponent(sessid) + '&ua=' + encodeURIComponent(ua);
                        }
                    } catch (e) {
                        // Not logged in yet
                    }
                }, 2000);
            }

            if (document.readyState === 'complete' || document.readyState === 'interactive') {
                checkFanboxLogin();
            }
            window.addEventListener('load', checkFanboxLogin);
            setTimeout(checkFanboxLogin, 1500);
        })();
    "#;

    let parsed_login_url = login_url
        .parse()
        .map_err(|e| format!("Failed to parse FANBOX login URL: {}", e))?;
    let mut builder =
        WebviewWindowBuilder::new(&app, window_label, WebviewUrl::External(parsed_login_url));

    let navigation_sender = tx_mutex.clone();
    builder = builder
        .title("FANBOX Login")
        .inner_size(400.0, 700.0)
        .initialization_script(init_script)
        .on_navigation(move |url| {
            if let Some(auth_data) = fanbox_callback_data(url) {
                if let Some(sender) = navigation_sender.lock().take() {
                    let _ = sender.send(auth_data);
                }
                return false;
            }
            true
        });

    let window = builder.build().map_err(|e| e.to_string())?;
    let window_clone = window.clone();
    let cancel_sender = tx_mutex.clone();
    window.on_window_event(move |event| {
        if matches!(
            event,
            tauri::WindowEvent::CloseRequested { .. } | tauri::WindowEvent::Destroyed
        ) {
            cancel_sender.lock().take();
        }
    });

    let auth_data = match tokio::time::timeout(Duration::from_secs(300), rx).await {
        Ok(Ok(d)) => d,
        _ => {
            let _ = window_clone.close();
            return Err("Login timed out or window closed".to_string());
        }
    };

    let session_val = &auth_data.session;
    let user_agent = &auth_data.ua;

    // HttpOnly cookie の抽出: WebView2 の CookieManager 経由で読み取る
    let final_cookie_str = if session_val == "__httponly_extract_needed__" {
        let fanbox_url: Url = "https://www.fanbox.cc/"
            .parse()
            .map_err(|e| format!("Failed to parse FANBOX base URL: {}", e))?;
        // Close the native window even when cookie extraction fails. Leaving it
        // alive retains the WebView2 profile and makes the next login race the
        // stale window label.
        let cookies_result = window_clone.cookies_for_url(fanbox_url);
        let close_result = window_clone.close();
        let cookies = cookies_result.map_err(|e| format!("Failed to get cookies: {e}"))?;
        if let Err(error) = close_result {
            log::warn!("Failed to close FANBOX login window: {error}");
        }

        let cookie_str = cookie_header(&cookies);

        if cookies.iter().any(|cookie| cookie.name() == "FANBOXSESSID") {
            cookie_str
        } else {
            return Err(
                "FANBOXログインは成功しましたが、FANBOXSESSIDが見つかりませんでした。".to_string(),
            );
        }
    } else {
        let _ = window_clone.close();
        format!("FANBOXSESSID={}", session_val)
    };

    let user = check_fanbox_session(&final_cookie_str, user_agent)
        .await
        .map_err(|e| e.to_string())?;
    Ok((final_cookie_str, user, user_agent.to_string()))
}

// ---------------------------------------------------------------------------
// Embedded browser (child webview inside main window)
// ---------------------------------------------------------------------------

use tauri::{LogicalPosition, LogicalSize};

static EMBEDDED_BROWSER_OPEN_LOCK: Mutex<()> = Mutex::new(());
static STANDALONE_BROWSER_OPEN_LOCK: Mutex<()> = Mutex::new(());

/// メインウィンドウ内に子WebViewを作成・管理する
pub fn open_child_webview(
    app: AppHandle,
    url: String,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    user_agent: Option<String>,
) -> Result<(), String> {
    // Initial layout and ResizeObserver can request the child WebView at the
    // same time. Serialize creation so both requests cannot add the same label.
    let _open_guard = EMBEDDED_BROWSER_OPEN_LOCK.lock();
    validate_webview_bounds(x, y, width, height)?;
    let url_parsed = parse_browser_url(&url)?;
    let browser_source = source_for_browser_url(&url_parsed).to_string();
    let user_agent = validate_user_agent(user_agent)?;

    // 既に存在する場合も要求されたURLへ同期する。レイアウト変更時に
    // `open_embedded_browser` が再度呼ばれるため、同一URLでは遷移しない。
    if let Some(existing) = app.get_webview("embedded_browser") {
        let current_url = existing.url().map_err(|e| e.to_string())?;
        if current_url != url_parsed {
            existing.navigate(url_parsed).map_err(|e| e.to_string())?;
        }
        existing
            .set_position(LogicalPosition::new(x, y))
            .map_err(|e| e.to_string())?;
        existing
            .set_size(LogicalSize::new(width, height))
            .map_err(|e| e.to_string())?;
        existing.show().map_err(|e| e.to_string())?;
        return Ok(());
    }

    let main_window_for_emit = app
        .get_webview_window("main")
        .ok_or("Main window not found")?;
    let main_window_for_child = app.get_window("main").ok_or("Main window not found")?;

    // Pixivの固定幅サイトを画面幅に自動縮小するスクリプト。
    //
    // このWebViewはリモートオリジンで読み込まれるためTauri IPCは許可せず、
    // URL追跡は on_page_load とフロント側ポーリングに任せる。
    let init_script = r#"
        (function() {
            let lastZoom = 1;

            const autofit = () => {
                const hostname = window.location.hostname.toLowerCase();
                if (hostname === 'pixiv.net' || hostname.endsWith('.pixiv.net')) {
                    const minWidth = 1080;
                    const clientWidth = document.documentElement.clientWidth;
                    if (clientWidth === 0) return;
                    let zoom = 1;
                    if (clientWidth < minWidth) {
                        zoom = clientWidth / minWidth;
                    }

                    if (Math.abs(zoom - lastZoom) > 0.01) {
                        lastZoom = zoom;
                        document.body.style.setProperty('zoom', zoom, 'important');
                        document.body.style.setProperty('transform-origin', 'top left', 'important');
                    }
                } else {
                    if (lastZoom !== 1) {
                        lastZoom = 1;
                        document.body.style.removeProperty('zoom');
                    }
                }
            };

            // SPA遷移でもレイアウトが変わるため、履歴APIをフックして再調整する
            const originalPushState = history.pushState;
            history.pushState = function() {
                const result = originalPushState.apply(this, arguments);
                autofit();
                return result;
            };
            const originalReplaceState = history.replaceState;
            history.replaceState = function() {
                const result = originalReplaceState.apply(this, arguments);
                autofit();
                return result;
            };
            window.addEventListener('popstate', autofit);
            window.addEventListener('resize', autofit);

            if (document.readyState === 'complete') { autofit(); }
            else { window.addEventListener('load', autofit); }
        })();
    "#;

    let main_window_clone = main_window_for_emit.clone();
    let navigation_app = app.clone();
    let popup_app = app.clone();

    let mut webview_builder =
        WebviewBuilder::new("embedded_browser", WebviewUrl::External(url_parsed))
            .initialization_script(init_script)
            .initialization_script(BROWSER_ACCELERATOR_SCRIPT)
            .user_agent(DEFAULT_BROWSER_USER_AGENT)
            .on_navigation(move |url| {
                if let Some(action) = accelerator_action(url) {
                    if let Some(webview) = navigation_app.get_webview("embedded_browser") {
                        if let Ok(current_url) = webview.url() {
                            emit_browser_accelerator(
                                &navigation_app,
                                "embedded",
                                source_for_browser_url(&current_url),
                                action,
                                &current_url,
                            );
                        }
                        if action == "close" {
                            let _ = webview.hide();
                        }
                    }
                    return false;
                }
                is_safe_browser_navigation(url)
            })
            .on_new_window(move |url, _features| {
                if is_safe_browser_navigation(&url) {
                    if let Some(webview) = popup_app.get_webview("embedded_browser") {
                        let _ = webview.navigate(url);
                    }
                }
                // Never let a remote origin create an unmanaged native window.
                tauri::webview::NewWindowResponse::Deny
            })
            .on_page_load(move |_webview, payload| {
                // ページ読み込み開始時・完了時にURL変更イベントを発火
                match payload.event() {
                    tauri::webview::PageLoadEvent::Started
                    | tauri::webview::PageLoadEvent::Finished => {
                        let _ = main_window_clone.emit("url-changed", payload.url().to_string());
                    }
                }
            });

    if let Some(ua) = user_agent {
        webview_builder = webview_builder.user_agent(&ua);
    }

    let webview = main_window_for_child
        .add_child(
            webview_builder,
            LogicalPosition::new(x, y),
            LogicalSize::new(width, height),
        )
        .map_err(|e| e.to_string())?;
    install_native_browser_accelerators(
        &webview,
        app,
        "embedded_browser".to_string(),
        "embedded",
        browser_source,
    )?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Standalone save browser window
// ---------------------------------------------------------------------------

fn standalone_window_details(source: &str) -> Result<(&'static str, &'static str), String> {
    match source {
        "pixiv" => Ok(("standalone_browser_pixiv", "pixiv — piep")),
        "fanbox" => Ok(("standalone_browser_fanbox", "FANBOX — piep")),
        _ => Err("Unsupported browser source".to_string()),
    }
}

fn fit_standalone_window_size(logical_width: f64, logical_height: f64) -> (f64, f64) {
    (
        (logical_width * 0.88).clamp(800.0, 1600.0),
        (logical_height * 0.86).clamp(560.0, 1000.0),
    )
}

fn standalone_window_size(app: &AppHandle) -> (f64, f64) {
    app.get_webview_window("main")
        .and_then(|window| window.current_monitor().ok().flatten())
        .map(|monitor| {
            let scale = monitor.scale_factor();
            let size = monitor.size();
            fit_standalone_window_size(size.width as f64 / scale, size.height as f64 / scale)
        })
        .unwrap_or((1360.0, 900.0))
}

/// Opens one large browser window per provider. Repeated requests reuse and
/// focus that window instead of racing duplicate WebView2 profiles/windows.
/// The return value is true when an existing window was reused.
pub fn open_standalone_webview(
    app: AppHandle,
    url: String,
    source: String,
    user_agent: Option<String>,
) -> Result<bool, String> {
    let _open_guard = STANDALONE_BROWSER_OPEN_LOCK.lock();
    let url = parse_browser_url(&url)?;
    let user_agent = validate_user_agent(user_agent)?;
    let (window_label, window_title) = standalone_window_details(&source)?;
    let (window_width, window_height) = standalone_window_size(&app);

    if let Some(existing) = app.get_webview_window(window_label) {
        let current_url = existing.url().map_err(|e| e.to_string())?;
        if current_url != url {
            existing.navigate(url).map_err(|e| e.to_string())?;
        }
        existing.show().map_err(|e| e.to_string())?;
        if existing.is_minimized().map_err(|e| e.to_string())? {
            existing.unminimize().map_err(|e| e.to_string())?;
        }
        existing.set_focus().map_err(|e| e.to_string())?;
        return Ok(true);
    }

    let navigation_app = app.clone();
    let navigation_label = window_label.to_string();
    let navigation_source = source.clone();
    let popup_app = app.clone();
    let popup_label = window_label.to_string();
    let page_load_main = app.get_webview_window("main");
    let page_load_source = source.clone();

    let mut builder = WebviewWindowBuilder::new(&app, window_label, WebviewUrl::External(url))
        .title(window_title)
        .inner_size(window_width, window_height)
        .min_inner_size(800.0, 560.0)
        .resizable(true)
        .center()
        .focused(true)
        .initialization_script(BROWSER_ACCELERATOR_SCRIPT)
        .user_agent(DEFAULT_BROWSER_USER_AGENT)
        .on_navigation(move |url| {
            if let Some(action) = accelerator_action(url) {
                if let Some(window) = navigation_app.get_webview_window(&navigation_label) {
                    if let Ok(current_url) = window.url() {
                        emit_browser_accelerator(
                            &navigation_app,
                            "standalone",
                            &navigation_source,
                            action,
                            &current_url,
                        );
                    }
                    if action == "close" {
                        let _ = window.close();
                    }
                }
                return false;
            }
            is_safe_browser_navigation(url)
        })
        .on_new_window(move |url, _features| {
            if is_safe_browser_navigation(&url) {
                if let Some(window) = popup_app.get_webview_window(&popup_label) {
                    let _ = window.navigate(url);
                }
            }
            tauri::webview::NewWindowResponse::Deny
        })
        .on_page_load(move |_webview, payload| {
            if let Some(main) = &page_load_main {
                let _ = main.emit(
                    "standalone-browser-url-changed",
                    serde_json::json!({
                        "source": page_load_source,
                        "url": payload.url().to_string(),
                    }),
                );
            }
        });

    if let Some(ua) = user_agent {
        builder = builder.user_agent(&ua);
    }
    let window = builder.build().map_err(|e| e.to_string())?;
    // The save workspace hands its browser pane over to this window while it is
    // open, so it has to be told when the window goes away or the pane would
    // stay disabled with nothing driving it.
    let closed_app = app.clone();
    let closed_source = source.clone();
    window.on_window_event(move |event| {
        // Both events are handled: the title-bar close button raises
        // CloseRequested, and Destroyed covers programmatic teardown. The
        // receiver ignores a repeat, so announcing twice is harmless.
        if matches!(
            event,
            tauri::WindowEvent::CloseRequested { .. } | tauri::WindowEvent::Destroyed
        ) {
            if let Some(main) = closed_app.get_webview_window("main") {
                let _ = main.emit(
                    "standalone-browser-closed",
                    serde_json::json!({ "source": closed_source }),
                );
            }
        }
    });
    install_native_browser_accelerators(
        window.as_ref(),
        app,
        window_label.to_string(),
        "standalone",
        source,
    )?;
    // Centering and sizing use logical units; WebView2/Tauri translate them at
    // the target monitor's DPI. Focus after construction avoids a z-order race
    // with the main window when the command was launched from the child view.
    window.set_focus().map_err(|e| e.to_string())?;
    Ok(false)
}

pub fn close_standalone_webview(app: AppHandle, source: String) -> Result<bool, String> {
    let (window_label, _) = standalone_window_details(&source)?;
    let Some(window) = app.get_webview_window(window_label) else {
        return Ok(false);
    };
    window.close().map_err(|e| e.to_string())?;
    Ok(true)
}

/// Lets the save workspace recover its handed-over state after being remounted
/// while the large window is still open.
pub fn standalone_webview_url(app: AppHandle, source: String) -> Result<Option<String>, String> {
    let (window_label, _) = standalone_window_details(&source)?;
    let Some(window) = app.get_webview_window(window_label) else {
        return Ok(None);
    };
    window
        .url()
        .map(|url| Some(url.to_string()))
        .map_err(|e| e.to_string())
}

/// 子WebViewの位置とサイズだけを更新する。
///
/// レイアウト変更のたびに `open_child_webview` を呼ぶと、フロント側が保持する
/// URLが古い場合に閲覧中のページから勝手に遷移してしまう。リサイズ経路では
/// URLに触れないこの関数を使う。WebViewが無い場合は `false` を返す。
pub fn set_child_webview_bounds(
    app: AppHandle,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
) -> Result<bool, String> {
    validate_webview_bounds(x, y, width, height)?;
    let Some(webview) = app.get_webview("embedded_browser") else {
        return Ok(false);
    };
    webview
        .set_position(LogicalPosition::new(x, y))
        .map_err(|e| e.to_string())?;
    webview
        .set_size(LogicalSize::new(width, height))
        .map_err(|e| e.to_string())?;
    Ok(true)
}

/// 子WebViewの表示・非表示を切り替える。
///
/// 子WebViewはネイティブレイヤーとして常にDOMより手前に描画されるため、
/// モーダルやメニューが重なる間は隠す必要がある。
pub fn set_child_webview_visible(app: AppHandle, visible: bool) -> Result<bool, String> {
    let Some(webview) = app.get_webview("embedded_browser") else {
        return Ok(false);
    };
    if visible {
        webview.show().map_err(|e| e.to_string())?;
    } else {
        webview.hide().map_err(|e| e.to_string())?;
    }
    Ok(true)
}

/// WebViewのURLを別のURLに変更する
pub fn navigate_child_webview(app: AppHandle, url: String) -> Result<(), String> {
    if let Some(webview) = app.get_webview("embedded_browser") {
        let url_parsed = parse_browser_url(&url)?;
        webview.navigate(url_parsed).map_err(|e| e.to_string())?;
        Ok(())
    } else {
        Err("Embedded browser not found".into())
    }
}

/// 現在のWebViewのURLを取得する
pub fn get_child_webview_url(app: AppHandle) -> Result<String, String> {
    if let Some(webview) = app.get_webview("embedded_browser") {
        let url = webview.url().map_err(|e| e.to_string())?;
        Ok(url.to_string())
    } else {
        Err("Embedded browser not found".into())
    }
}

/// WebViewを非表示にする（メモリには残る）
pub fn close_child_webview(app: AppHandle) -> Result<(), String> {
    if let Some(existing) = app.get_webview("embedded_browser") {
        existing.hide().map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// WebViewを完全に破棄する（メモリから解放）
pub fn destroy_child_webview(app: AppHandle) -> Result<(), String> {
    if let Some(existing) = app.get_webview("embedded_browser") {
        existing.close().map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// WebViewで「戻る」操作を実行する
pub fn go_back_child_webview(app: AppHandle) -> Result<(), String> {
    if let Some(webview) = app.get_webview("embedded_browser") {
        webview.eval("history.back()").map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// WebViewで「進む」操作を実行する
pub fn go_forward_child_webview(app: AppHandle) -> Result<(), String> {
    if let Some(webview) = app.get_webview("embedded_browser") {
        webview
            .eval("history.forward()")
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// WebViewを再読み込みする
pub fn reload_child_webview(app: AppHandle) -> Result<(), String> {
    if let Some(webview) = app.get_webview("embedded_browser") {
        webview
            .eval("location.reload()")
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixiv_callback_accepts_only_expected_callback_origins() {
        let app_callback = Url::parse("pixiv://account/login?code=good").unwrap();
        let https_callback =
            Url::parse("https://app-api.pixiv.net/web/v1/users/auth/pixiv/callback?code=good2")
                .unwrap();
        let attacker = Url::parse("https://example.com/?code=stolen").unwrap();
        let wrong_custom_origin = Url::parse("pixiv://attacker/login?code=stolen").unwrap();

        assert_eq!(pixiv_callback_code(&app_callback).as_deref(), Some("good"));
        assert_eq!(
            pixiv_callback_code(&https_callback).as_deref(),
            Some("good2")
        );
        assert_eq!(pixiv_callback_code(&attacker), None);
        assert_eq!(pixiv_callback_code(&wrong_custom_origin), None);
    }

    #[test]
    fn fanbox_callback_requires_exact_custom_origin_and_valid_values() {
        let valid = Url::parse("fanbox-auth://callback?session=token&ua=agent").unwrap();
        let wrong_host = Url::parse("fanbox-auth://attacker?session=token&ua=agent").unwrap();
        let missing = Url::parse("fanbox-auth://callback?session=token").unwrap();

        let auth = fanbox_callback_data(&valid).unwrap();
        assert_eq!(auth.session, "token");
        assert_eq!(auth.ua, "agent");
        assert!(fanbox_callback_data(&wrong_host).is_none());
        assert!(fanbox_callback_data(&missing).is_none());
    }

    #[test]
    fn embedded_browser_rejects_privileged_or_ambiguous_urls() {
        assert!(parse_browser_url("https://www.pixiv.net/").is_ok());
        assert!(parse_browser_url("http://localhost:1420/").is_ok());
        assert!(parse_browser_url("file:///C:/secret.txt").is_err());
        assert!(parse_browser_url("javascript:alert(1)").is_err());
        assert!(parse_browser_url("https://user:pass@example.com/").is_err());
        assert!(parse_browser_url("https://").is_err());
    }

    #[test]
    fn embedded_browser_bounds_reject_non_finite_and_zero_sizes() {
        assert!(validate_webview_bounds(0.0, 0.0, 800.0, 600.0).is_ok());
        assert!(validate_webview_bounds(f64::NAN, 0.0, 800.0, 600.0).is_err());
        assert!(validate_webview_bounds(0.0, 0.0, 0.0, 600.0).is_err());
        assert!(validate_webview_bounds(0.0, 0.0, 800.0, f64::INFINITY).is_err());
    }

    #[test]
    fn browser_accelerator_accepts_only_known_internal_actions() {
        let save = Url::parse("piep-browser://accelerator/save").unwrap();
        let close = Url::parse("piep-browser://accelerator/close").unwrap();
        let unknown = Url::parse("piep-browser://accelerator/delete-everything").unwrap();
        let remote = Url::parse("https://accelerator/save").unwrap();
        assert_eq!(accelerator_action(&save), Some("save"));
        assert_eq!(accelerator_action(&close), Some("close"));
        assert_eq!(accelerator_action(&unknown), None);
        assert_eq!(accelerator_action(&remote), None);
    }

    /// 一押しで二経路が鳴っても、渡すのは一度きり。押し直しは通す。
    #[test]
    fn browser_accelerator_collapses_the_second_report_of_one_keypress() {
        let start = Instant::now();
        let first = ("embedded:pixiv:save".to_string(), start);
        assert!(!accelerator_is_repeat(None, "embedded:pixiv:save", start));
        assert!(accelerator_is_repeat(
            Some(&first),
            "embedded:pixiv:save",
            start + Duration::from_millis(30)
        ));
        // 押し直しは別の押し。
        assert!(!accelerator_is_repeat(
            Some(&first),
            "embedded:pixiv:save",
            start + ACCELERATOR_DEDUPE_WINDOW
        ));
        // 別のウィンドウ・別の操作は、たまたま近くても別のもの。
        assert!(!accelerator_is_repeat(
            Some(&first),
            "standalone:pixiv:save",
            start + Duration::from_millis(30)
        ));
        assert!(!accelerator_is_repeat(
            Some(&first),
            "embedded:pixiv:close",
            start + Duration::from_millis(30)
        ));
    }

    #[test]
    fn native_accelerator_map_ignores_ambiguous_modifiers() {
        assert_eq!(
            native_browser_action(0x25, false, true),
            Some(NativeBrowserAction::Back)
        );
        assert_eq!(
            native_browser_action(0x53, true, false),
            Some(NativeBrowserAction::Save)
        );
        assert_eq!(
            native_browser_action(0x57, true, false),
            Some(NativeBrowserAction::Close)
        );
        assert_eq!(native_browser_action(0x53, true, true), None);
        assert_eq!(native_browser_action(0x53, false, false), None);
    }

    #[test]
    fn standalone_browser_labels_are_bounded_to_known_providers() {
        assert_eq!(
            standalone_window_details("pixiv").unwrap().0,
            "standalone_browser_pixiv"
        );
        assert_eq!(
            standalone_window_details("fanbox").unwrap().0,
            "standalone_browser_fanbox"
        );
        assert!(standalone_window_details("../main").is_err());
    }

    #[test]
    fn standalone_browser_size_scales_with_monitor_and_is_bounded() {
        assert_eq!(fit_standalone_window_size(1920.0, 1080.0), (1600.0, 928.8));
        assert_eq!(fit_standalone_window_size(700.0, 500.0), (800.0, 560.0));
        assert_eq!(fit_standalone_window_size(3840.0, 2160.0), (1600.0, 1000.0));
    }
}
