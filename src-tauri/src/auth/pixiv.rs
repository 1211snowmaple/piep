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
        let json_value: serde_json::Value =
            serde_json::from_str(&bounded_response_text(res).await?)?;
        let resp = json_value.get("response").unwrap_or(&json_value);
        let auth_resp: PixivAuthResponse = serde_json::from_value(resp.clone())?;
        Ok(auth_resp)
    } else {
        let err_text = bounded_response_text(res).await?;
        log::error!("Pixiv API Error Response: {}", err_text);
        Err(format!("Pixiv Auth Error: {}", err_text).into())
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
        let json_value: serde_json::Value =
            serde_json::from_str(&bounded_response_text(res).await?)?;
        let resp = json_value.get("response").unwrap_or(&json_value);
        let auth_resp: PixivAuthResponse = serde_json::from_value(resp.clone())?;
        Ok(auth_resp)
    } else {
        let err_text = bounded_response_text(res).await?;
        log::error!("Pixiv API Error Response: {}", err_text);
        Err(format!("Pixiv Auth Error: {}", err_text).into())
    }
}
