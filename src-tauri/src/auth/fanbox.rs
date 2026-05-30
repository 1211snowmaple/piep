use regex::Regex;
use reqwest::Client;
use std::error::Error;

use crate::fanbox_api::client::FanboxAPI;

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct FanboxUser {
    pub user_id: String,
    pub name: String,
    pub icon_url: Option<String>,
}

/// Fanboxセッションの有効性を確認し、ユーザー情報を返す
pub async fn check_fanbox_session(
    full_cookie: &str,
    user_agent: &str,
) -> Result<FanboxUser, Box<dyn Error>> {
    let api = FanboxAPI::new(full_cookie.to_string(), user_agent.to_string());
    if let Err(e) = api.check_session().await {
        return Err(format!("Session validation failed: {:?}", e).into());
    }

    let client = Client::new();
    let html_res = client
        .get("https://www.fanbox.cc/")
        .header("Cookie", full_cookie)
        .header("User-Agent", user_agent)
        .send()
        .await?;

    let html = html_res.text().await?;
    let re_meta = Regex::new(r#"id="metadata"\s+name="metadata"\s+content='([^']+)'"#)?;
    let metadata = re_meta
        .captures(&html)
        .and_then(|caps| caps.get(1))
        .map(|m| m.as_str())
        .ok_or("Failed to extract Fanbox metadata from HTML")?;
    let json: serde_json::Value = serde_json::from_str(metadata)
        .map_err(|e| format!("Failed to parse Fanbox metadata JSON: {}", e))?;

    let user = json["context"]["user"]
        .as_object()
        .ok_or("User data not found in Fanbox metadata JSON")?;

    let user_data = FanboxUser {
        user_id: user
            .get("userId")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        name: user
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        icon_url: user
            .get("iconUrl")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
    };

    Ok(user_data)
}
