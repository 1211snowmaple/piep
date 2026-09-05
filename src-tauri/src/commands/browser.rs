//! 内蔵ブラウザの操作。
//!
//! 二種類ある。`embedded` はアプリの窓の中に重ねる子 WebView、`standalone`
//! は別窓である。中身は `auth::webview` にあり、ここは薄皮でしかない。
//!
//! **子 WebView の位置と大きさはフロントが持つ。** これは HTML の上に重なる
//! 別の面で、レイアウトの一部ではない。スクロールや折りたたみで動いたぶんを
//! `set_embedded_browser_bounds` で渡し直さないと、画面と絵がずれる。
//!
//! 閉じるのに二つあるのは、隠すだけの `close_` と、面ごと捨てる `destroy_`
//! を分けているからである。次に開くのが同じ場所なら、捨てないほうが速い。

use crate::auth::webview::{
    close_child_webview, close_standalone_webview, destroy_child_webview, get_child_webview_url,
    go_back_child_webview, go_forward_child_webview, navigate_child_webview, open_child_webview,
    open_standalone_webview, reload_child_webview, set_child_webview_bounds,
    set_child_webview_visible, standalone_webview_url,
};

/// アプリの窓の中に子 WebView を開く。
///
/// 位置と大きさは論理ピクセルで、**フロントが自分で計算して渡す**。子 WebView
/// は HTML の上に重なる別の面で、レイアウトの一部ではない。
///
/// `userAgent` を渡すと、その面での名乗りを固定できる。Cookie を受け取る用途
/// では、あとで確認に使う UA と同じものを渡すこと。
#[tauri::command]
pub async fn open_embedded_browser(
    app: tauri::AppHandle,
    url: String,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    user_agent: Option<String>,
) -> Result<(), String> {
    open_child_webview(app, url, x, y, width, height, user_agent)
}

/// 子 WebView の位置と大きさを置き直す。
///
/// **スクロール・折りたたみ・窓の大きさの変更のたびに呼ぶ。** 別の面なので、
/// 下の HTML が動いても勝手には追従しない。
///
/// `false` は「置き直す相手がいなかった」を意味する（まだ開いていない、または
/// すでに捨てられている）。誤りではないので、失敗として扱わないこと。
#[tauri::command]
pub async fn set_embedded_browser_bounds(
    app: tauri::AppHandle,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
) -> Result<bool, String> {
    set_child_webview_bounds(app, x, y, width, height)
}

/// 子 WebView を隠す／出す。捨てはしないので、次に出すのは速い。
///
/// `false` は「相手がいなかった」を意味する。
#[tauri::command]
pub async fn set_embedded_browser_visible(
    app: tauri::AppHandle,
    visible: bool,
) -> Result<bool, String> {
    set_child_webview_visible(app, visible)
}

/// 開いている子 WebView を別の URL へ動かす。
///
/// 履歴に積まれるので、直後に「戻る」で元の頁へ帰れる。
#[tauri::command]
pub async fn navigate_embedded_browser(app: tauri::AppHandle, url: String) -> Result<(), String> {
    navigate_child_webview(app, url)
}

/// 子 WebView が今いる URL を返す。
///
/// 利用者が中で辿った先を知るために使う。`navigate_embedded_browser` に渡した
/// URL と同じとは限らない。
#[tauri::command]
pub async fn get_embedded_browser_url(app: tauri::AppHandle) -> Result<String, String> {
    get_child_webview_url(app)
}

/// 子 WebView を隠す。**面は残る。**
///
/// 同じ場所をまた開くなら、こちらのほうが速い。面ごと捨てたいときは
/// `destroy_embedded_browser` を使う。
#[tauri::command]
pub async fn close_embedded_browser(app: tauri::AppHandle) -> Result<(), String> {
    close_child_webview(app)
}

/// 子 WebView を面ごと捨てる。
///
/// 次に開くときは作り直しになる。画面を離れるときや、別の取得元へ切り替える
/// ときのように、**中に残った状態を持ち越したくないとき**に使う。
#[tauri::command]
pub async fn destroy_embedded_browser(app: tauri::AppHandle) -> Result<(), String> {
    destroy_child_webview(app)
}

/// 子 WebView の履歴を一つ戻る。戻れる先が無ければ何も起きない。
#[tauri::command]
pub async fn go_back_embedded_browser(app: tauri::AppHandle) -> Result<(), String> {
    go_back_child_webview(app)
}

/// 子 WebView の履歴を一つ進む。進める先が無ければ何も起きない。
#[tauri::command]
pub async fn go_forward_embedded_browser(app: tauri::AppHandle) -> Result<(), String> {
    go_forward_child_webview(app)
}

/// 子 WebView を読み直す。今いる URL のまま更新する。
#[tauri::command]
pub async fn reload_embedded_browser(app: tauri::AppHandle) -> Result<(), String> {
    reload_child_webview(app)
}

/// 取得元ごとの別窓を開く。
///
/// `source` が窓の識別子になる。**同じ `source` の窓は一つしか作らない。**
///
/// `true` は「すでにあった窓を使った」（URL が違えばその窓を動かした）、
/// `false` は「新しく開いた」を意味する。
#[tauri::command]
pub async fn open_standalone_browser(
    app: tauri::AppHandle,
    url: String,
    source: String,
    user_agent: Option<String>,
) -> Result<bool, String> {
    open_standalone_webview(app, url, source, user_agent)
}

/// 取得元ごとの別窓を閉じる。
///
/// `false` は「その `source` の窓が無かった」を意味する。
#[tauri::command]
pub async fn close_standalone_browser(
    app: tauri::AppHandle,
    source: String,
) -> Result<bool, String> {
    close_standalone_webview(app, source)
}

/// Returns the page the large window is showing, or null when it is not open.
#[tauri::command]
pub async fn get_standalone_browser_url(
    app: tauri::AppHandle,
    source: String,
) -> Result<Option<String>, String> {
    standalone_webview_url(app, source)
}
