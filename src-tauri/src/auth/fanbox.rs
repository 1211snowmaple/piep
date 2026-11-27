use regex::Regex;
use reqwest::Client;
use std::error::Error;
use std::time::Duration;

use crate::fanbox_api::client::FanboxAPI;

const MAX_FANBOX_HOME_BYTES: usize = 8 * 1024 * 1024;

/// フォロー一覧の読み取りと API の呼び出しで同じ ID の規則を使う。
pub(crate) fn validate_creator_id(id: &str) -> Result<(), &'static str> {
    if (1..=64).contains(&id.len())
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
    {
        Ok(())
    } else {
        Err("クリエイターIDの形式が正しくありません")
    }
}

async fn bounded_home_text(mut response: reqwest::Response) -> Result<String, Box<dyn Error>> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_FANBOX_HOME_BYTES as u64)
    {
        return Err("FANBOX home response exceeded the size limit".into());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if body.len().saturating_add(chunk.len()) > MAX_FANBOX_HOME_BYTES {
            return Err("FANBOX home response exceeded the size limit".into());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8(body)?)
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct FanboxUser {
    pub user_id: String,
    pub name: String,
    pub icon_url: Option<String>,
}

fn fanbox_user_from_home(html: &str) -> Result<FanboxUser, Box<dyn Error>> {
    let re_meta = Regex::new(r#"id="metadata"\s+name="metadata"\s+content='([^']+)'"#)?;
    let metadata = re_meta
        .captures(html)
        .and_then(|caps| caps.get(1))
        .map(|m| m.as_str())
        .ok_or("Failed to extract Fanbox metadata from HTML")?;
    let json: serde_json::Value = serde_json::from_str(metadata)
        .map_err(|e| format!("Failed to parse Fanbox metadata JSON: {}", e))?;
    let user = json["context"]["user"]
        .as_object()
        .ok_or("User data not found in Fanbox metadata JSON")?;
    let user_id = user
        .get("userId")
        .and_then(|id| match id {
            serde_json::Value::String(id) if !id.is_empty() => Some(id.clone()),
            _ => id.as_u64().map(|id| id.to_string()),
        })
        .ok_or("接続した利用者の ID を取得できません")?;
    Ok(FanboxUser {
        user_id,
        name: user
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        icon_url: user
            .get("iconUrl")
            .and_then(|v| v.as_str())
            .map(str::to_string),
    })
}

/// Fanboxセッションの有効性を確認し、ユーザー情報を返す
pub async fn check_fanbox_session(
    full_cookie: &str,
    user_agent: &str,
) -> Result<FanboxUser, Box<dyn Error>> {
    let api = FanboxAPI::new(full_cookie.to_string(), user_agent.to_string())?;
    if let Err(e) = api.check_session().await {
        return Err(format!("Session validation failed: {e}").into());
    }

    let client = Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let html_res = client
        .get("https://www.fanbox.cc/")
        .header("Cookie", full_cookie)
        .header("User-Agent", user_agent)
        .send()
        .await?
        .error_for_status()?;

    let html = bounded_home_text(html_res).await?;
    fanbox_user_from_home(&html)
}

#[cfg(test)]
mod tests {
    use super::fanbox_user_from_home;

    #[test]
    fn home_metadata_is_parsed_without_accepting_nearby_scripts() {
        let html = r#"<script id="other">{}</script><meta id="metadata" name="metadata" content='{"context":{"user":{"userId":"7","name":"作者","iconUrl":"https://example.test/icon.png"}}}'>"#;
        let user = fanbox_user_from_home(html).unwrap();
        assert_eq!(user.user_id, "7");
        assert_eq!(user.name, "作者");
        assert_eq!(
            user.icon_url.as_deref(),
            Some("https://example.test/icon.png")
        );
        assert!(fanbox_user_from_home("<html></html>").is_err());
    }

    #[test]
    fn numeric_user_ids_are_kept_and_missing_ids_fail() {
        let html =
            r#"<meta id="metadata" name="metadata" content='{"context":{"user":{"userId":7}}}'>"#;
        assert_eq!(fanbox_user_from_home(html).unwrap().user_id, "7");
        assert!(fanbox_user_from_home(&html.replace("7", "null")).is_err());
    }
}
