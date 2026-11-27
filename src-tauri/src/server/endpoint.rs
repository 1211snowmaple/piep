//! サーバーに接続する URL の検査と正規化。

use url::Url;

const MAX_URL_BYTES: usize = 512;

pub(super) enum ServerUrlScheme {
    Https,
    HttpsOrLocalHttp,
}

/// 接続先を確かめ、パスの末尾に / が付いた URL を返す。
pub(super) fn server_url(text: &str, scheme: ServerUrlScheme) -> Result<Url, &'static str> {
    const TOO_LONG: &str = "サーバーの URL は 512 バイト以内にしてください";
    if text.len() > MAX_URL_BYTES {
        return Err(TOO_LONG);
    }
    if text.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("サーバーの URL に空白や制御文字は書けません");
    }
    // URL の解析で省かれる空の利用者名や、補われるホストの区切りも確かめる。
    let authority = text
        .split_once("://")
        .filter(|(scheme, _)| {
            scheme.eq_ignore_ascii_case("https") || scheme.eq_ignore_ascii_case("http")
        })
        .map(|(_, rest)| rest.split(['/', '?', '#']).next().unwrap_or_default())
        .filter(|authority| !authority.is_empty() && !text.contains('\\'))
        .ok_or("サーバーの URL は https:// とホスト名か IP アドレスを含めて書いてください")?;
    let mut url = Url::parse(text).map_err(|_| "サーバーの URL が正しくありません")?;
    if authority.contains('@') || !url.username().is_empty() || url.password().is_some() {
        return Err("サーバーの URL に利用者名やパスワードは書けません");
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err("サーバーの URL に問い合わせや断片は書けません");
    }
    match url.scheme() {
        "https" => {}
        "http"
            if matches!(scheme, ServerUrlScheme::HttpsOrLocalHttp)
                && matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]")) => {}
        _ => return Err("サーバーの URL は https:// で始めてください"),
    }
    url.set_path(&format!("{}/", url.path().trim_end_matches('/')));
    if url.as_str().len() > MAX_URL_BYTES {
        return Err(TOO_LONG);
    }
    Ok(url)
}
