use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use sha2::{Digest, Sha256};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, WebviewBuilder, WebviewUrl, WebviewWindowBuilder};
use tokio::sync::oneshot;
use url::Url;

use super::fanbox::{check_fanbox_session, FanboxUser};
use super::pixiv::{login_with_code, PixivUser};

// ---------------------------------------------------------------------------
// Thread communication structures
// ---------------------------------------------------------------------------

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct FanboxAuthData {
    pub session: String,
    pub ua: String,
}

// ---------------------------------------------------------------------------
// Pixiv OAuth Login (separate window)
// ---------------------------------------------------------------------------

pub async fn open_pixiv_login(app: AppHandle) -> Result<(String, PixivUser), String> {
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
        let _ = existing.close();
    }

    let tx_mutex = Arc::new(Mutex::new(Some(tx)));

    let parsed_login_url = login_url
        .parse()
        .map_err(|e| format!("Failed to parse Pixiv login URL: {}", e))?;
    let mut builder =
        WebviewWindowBuilder::new(&app, window_label, WebviewUrl::External(parsed_login_url));

    builder = builder
        .title("Pixiv Login")
        .inner_size(400.0, 700.0)
        .on_navigation(move |url| {
            let url_str = url.as_str();
            if url_str.starts_with("pixiv://") || url_str.contains("code=") {
                if let Ok(parsed_url) = Url::parse(url_str) {
                    for (key, value) in parsed_url.query_pairs() {
                        if key == "code" {
                            let mut tx_guard = tx_mutex.lock().unwrap();
                            if let Some(sender) = tx_guard.take() {
                                let _ = sender.send(value.into_owned());
                            }
                            return false;
                        }
                    }
                }
            }
            true
        });

    let window = builder.build().map_err(|e| e.to_string())?;
    let window_clone = window.clone();

    let code = match tokio::time::timeout(Duration::from_secs(300), rx).await {
        Ok(Ok(c)) => c,
        _ => {
            let _ = window_clone.close();
            return Err("Login timed out or window closed".to_string());
        }
    };

    let _ = window_clone.close();

    let auth_res = login_with_code(&code, &code_verifier)
        .await
        .map_err(|e| e.to_string())?;

    if let Some(user) = auth_res.user {
        Ok((auth_res.refresh_token, user))
    } else {
        Err("Failed to get user info".to_string())
    }
}

// ---------------------------------------------------------------------------
// FANBOX Login (separate window)
// ---------------------------------------------------------------------------

pub async fn open_fanbox_login(app: AppHandle) -> Result<(String, FanboxUser, String), String> {
    let login_url = "https://www.fanbox.cc/login";
    let window_label = "fanbox_login_window";

    if let Some(existing) = app.get_webview_window(window_label) {
        let _ = existing.close();
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
                if (!window.location.hostname.includes('fanbox.cc')) return;

                _fanboxCheckStarted = true;

                const interval = setInterval(async () => {
                    try {
                        const res = await fetch('https://api.fanbox.cc/creator.listFollowing', {
                            credentials: 'include',
                            headers: {
                                'Origin': 'https://www.fanbox.cc',
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

    builder = builder
        .title("FANBOX Login")
        .inner_size(400.0, 700.0)
        .initialization_script(init_script)
        .on_navigation(move |url| {
            let url_str = url.as_str();
            if url_str.starts_with("fanbox-auth://") {
                if let Ok(parsed_url) = Url::parse(url_str) {
                    let mut session = None;
                    let mut ua = None;
                    for (key, value) in parsed_url.query_pairs() {
                        if key == "session" {
                            session = Some(value.into_owned());
                        } else if key == "ua" {
                            ua = Some(value.into_owned());
                        }
                    }

                    if let (Some(s), Some(u)) = (session, ua) {
                        let mut tx_guard = tx_mutex.lock().unwrap();
                        if let Some(sender) = tx_guard.take() {
                            let _ = sender.send(FanboxAuthData { session: s, ua: u });
                        }
                        return false;
                    }
                }
            }
            true
        });

    let window = builder.build().map_err(|e| e.to_string())?;
    let window_clone = window.clone();

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
        let cookies = window_clone
            .cookies_for_url(fanbox_url)
            .map_err(|e| format!("Failed to get cookies: {}", e))?;

        let cookie_str = cookies
            .iter()
            .map(|c| format!("{}={}", c.name(), c.value()))
            .collect::<Vec<_>>()
            .join("; ");

        let _ = window_clone.close();

        if cookie_str.contains("FANBOXSESSID") {
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

/// メインウィンドウ内に子WebViewを作成・管理する
pub fn open_child_webview(
    app: AppHandle,
    url: String,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    _user_agent: Option<String>,
) -> Result<(), String> {
    // Initial layout and ResizeObserver can request the child WebView at the
    // same time. Serialize creation so both requests cannot add the same label.
    let _open_guard = EMBEDDED_BROWSER_OPEN_LOCK
        .lock()
        .map_err(|_| "Embedded browser lock was poisoned".to_string())?;
    let url_parsed = Url::parse(&url).map_err(|e| e.to_string())?;

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

    // Pixivの固定幅サイトを画面幅に自動縮小するスクリプト
    // + SPAの遷移(pushState/replaceState)を検知して document.title や IPC を通じて
    //   Rustへ即座に通知する
    let init_script = r#"
        (function() {
            let lastZoom = 1;
            let lastNotifiedUrl = '';

            const notifyUrlChange = () => {
                const currentUrl = window.location.href;
                if (currentUrl !== lastNotifiedUrl) {
                    lastNotifiedUrl = currentUrl;
                    // Tauri v2 の IPC 経由で Rust 側に即座に通知する
                    if (window.__TAURI__ && window.__TAURI__.core && window.__TAURI__.core.invoke) {
                        window.__TAURI__.core.invoke('notify_url_changed', { url: currentUrl }).catch(() => {});
                    } else if (window.__tauri_ipc__) {
                        window.__tauri_ipc__({
                            cmd: 'notify_url_changed',
                            url: currentUrl
                        });
                    }
                }
            };

            const autofit = () => {
                if (window.location.hostname.includes('pixiv.net')) {
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

            // pushState/replaceState をフック
            const originalPushState = history.pushState;
            history.pushState = function() {
                const result = originalPushState.apply(this, arguments);
                notifyUrlChange();
                autofit();
                return result;
            };
            const originalReplaceState = history.replaceState;
            history.replaceState = function() {
                const result = originalReplaceState.apply(this, arguments);
                notifyUrlChange();
                autofit();
                return result;
            };
            window.addEventListener('popstate', () => { notifyUrlChange(); autofit(); });
            window.addEventListener('resize', autofit);
            window.addEventListener('hashchange', notifyUrlChange);

            if (document.readyState === 'complete') { autofit(); notifyUrlChange(); }
            else { window.addEventListener('load', () => { autofit(); notifyUrlChange(); }); }
        })();
    "#;

    let main_window_clone = main_window_for_emit.clone();

    let mut webview_builder =
        WebviewBuilder::new("embedded_browser", WebviewUrl::External(url_parsed))
            .initialization_script(init_script)
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
            .on_page_load(move |_webview, payload| {
                // ページ読み込み開始時・完了時にURL変更イベントを発火
                match payload.event() {
                    tauri::webview::PageLoadEvent::Started | tauri::webview::PageLoadEvent::Finished => {
                        let _ = main_window_clone.emit("url-changed", payload.url().to_string());
                    }
                }
            });

    if let Some(ua) = _user_agent {
        webview_builder = webview_builder.user_agent(&ua);
    }

    let _webview = main_window_for_child
        .add_child(
            webview_builder,
            LogicalPosition::new(x, y),
            LogicalSize::new(width, height),
        )
        .map_err(|e| e.to_string())?;

    Ok(())
}

/// WebViewのURLを別のURLに変更する
pub fn navigate_child_webview(app: AppHandle, url: String) -> Result<(), String> {
    if let Some(webview) = app.get_webview("embedded_browser") {
        let url_parsed = Url::parse(&url).map_err(|e| e.to_string())?;
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
