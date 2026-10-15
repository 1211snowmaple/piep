pub use crate::pixiv_api::models::PixivUser;
use crate::pixiv_api::token_manager::{
    AUTH_TOKEN_URL, AUTH_USER_AGENT, DEFAULT_CLIENT_ID, DEFAULT_CLIENT_SECRET,
};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::error::Error;
use std::time::Duration;

const MAX_AUTH_RESPONSE_BYTES: usize = 2 * 1024 * 1024;

fn auth_client() -> Result<Client, reqwest::Error> {
    Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()
}

async fn bounded_response_text(mut response: reqwest::Response) -> Result<String, Box<dyn Error>> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_AUTH_RESPONSE_BYTES as u64)
    {
        return Err("Pixiv auth response exceeded the size limit".into());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if body.len().saturating_add(chunk.len()) > MAX_AUTH_RESPONSE_BYTES {
            return Err("Pixiv auth response exceeded the size limit".into());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8(body)?)
}

#[derive(Serialize, Deserialize, Debug)]
pub struct PixivAuthResponse {
    pub access_token: String,
    pub refresh_token: String,
    pub user: Option<PixivUser>,
}

fn parse_auth_response(text: &str) -> Result<PixivAuthResponse, Box<dyn Error>> {
    let json_value: serde_json::Value = serde_json::from_str(text)?;
    let response = json_value.get("response").unwrap_or(&json_value);
    Ok(serde_json::from_value(response.clone())?)
}
pub async fn login_with_refresh_token(
    refresh_token: &str,
) -> Result<PixivAuthResponse, Box<dyn Error>> {
    let client = auth_client()?;

    let params = [
        ("client_id", DEFAULT_CLIENT_ID),
        ("client_secret", DEFAULT_CLIENT_SECRET),
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
    ];

    let res = client
        .post(AUTH_TOKEN_URL)
        .header("User-Agent", AUTH_USER_AGENT)
        .header("App-OS", "ios")
        .header("App-OS-Version", "14.6")
        .form(&params)
        .send()
        .await?;

    if res.status().is_success() {
        parse_auth_response(&bounded_response_text(res).await?)
    } else {
        // 取得元が返した本文は記録にだけ残す。何が返るかは向こう次第で、
        // 利用者が読んでも次にできることは増えない（`PixivError` の他の
        // 変種と同じ方針）。
        let err_text = bounded_response_text(res).await?;
        log::error!("Pixiv API Error Response: {}", err_text);
        Err("pixivとの接続に失敗しました。トークンを取り直してから、もう一度お試しください".into())
    }
}

pub async fn login_with_code(
    code: &str,
    code_verifier: &str,
) -> Result<PixivAuthResponse, Box<dyn Error>> {
    let client = auth_client()?;

    let params = [
        ("client_id", DEFAULT_CLIENT_ID),
        ("client_secret", DEFAULT_CLIENT_SECRET),
        ("grant_type", "authorization_code"),
        ("code", code),
        ("code_verifier", code_verifier),
        (
            "redirect_uri",
            "https://app-api.pixiv.net/web/v1/users/auth/pixiv/callback",
        ),
        ("include_policy", "true"),
    ];

    let res = client
        .post(AUTH_TOKEN_URL)
        .header("User-Agent", AUTH_USER_AGENT)
        .header("App-OS", "ios")
        .header("App-OS-Version", "14.6")
        .form(&params)
        .send()
        .await?;

    if res.status().is_success() {
        parse_auth_response(&bounded_response_text(res).await?)
    } else {
        // 取得元が返した本文は記録にだけ残す。何が返るかは向こう次第で、
        // 利用者が読んでも次にできることは増えない（`PixivError` の他の
        // 変種と同じ方針）。
        let err_text = bounded_response_text(res).await?;
        log::error!("Pixiv API Error Response: {}", err_text);
        Err("pixivとの接続に失敗しました。トークンを取り直してから、もう一度お試しください".into())
    }
}

#[cfg(test)]
mod tests {
    use super::parse_auth_response;

    #[test]
    fn auth_payload_accepts_both_documented_envelopes() {
        for json in [
            r#"{"access_token":"a","refresh_token":"r","user":null}"#,
            r#"{"response":{"access_token":"a","refresh_token":"r","user":null}}"#,
        ] {
            let parsed = parse_auth_response(json).unwrap();
            assert_eq!(parsed.access_token, "a");
            assert_eq!(parsed.refresh_token, "r");
        }
        assert!(parse_auth_response(r#"{"response":{}}"#).is_err());
    }
}
