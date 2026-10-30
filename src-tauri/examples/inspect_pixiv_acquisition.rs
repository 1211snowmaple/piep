//! Read-only acquisition probe. Credentials are read from a local settings file;
//! only counts and image-validation results are written, never credentials or bodies.
use serde_json::{json, Value};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 3 {
        return Err("expected settings file, report file and novel IDs".into());
    }
    let settings: Value = serde_json::from_str(&std::fs::read_to_string(&args[0])?)?;
    let token = settings["pixiv_refresh_token"]
        .as_str()
        .ok_or("download connection is missing")?;
    let cookie = settings["pixiv_cookie"].as_str().map(str::to_string);
    let ua = settings["pixiv_user_agent"].as_str().map(str::to_string);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;
    let mut results = Vec::new();
    for (index, id) in args[2..].iter().enumerate() {
        if index > 0 {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
        if !id.chars().all(|ch| ch.is_ascii_digit()) {
            return Err("invalid novel ID".into());
        }
        let content = piep_lib::commands::downloader::fetch_pixiv_novel_by_url(
            format!("https://www.pixiv.net/novel/show.php?id={id}"),
            token.to_string(),
            cookie.clone(),
            ua.clone(),
        )
        .await;
        let content = match content {
            Ok(value) => serde_json::to_value(value)?,
            Err(error) => {
                results.push(json!({"id":id,"error":error}));
                continue;
            }
        };
        let original = content["illusts"]
            .as_object()
            .into_iter()
            .flat_map(|map| map.values())
            .filter_map(|entry| entry["pages"].as_array())
            .flatten()
            .find_map(|page| page.pointer("/urls/original").and_then(Value::as_str))
            .or_else(|| {
                content["illusts"]
                    .as_object()
                    .into_iter()
                    .flat_map(|map| map.values())
                    .find_map(|entry| {
                        ["original", "medium", "small"].iter().find_map(|size| {
                            entry
                                .get("illust")
                                .and_then(|illust| illust.get("images"))
                                .and_then(|images| images.get(size))
                                .and_then(Value::as_str)
                        })
                    })
            });
        let mut image_verified = None;
        if let Some(url) = original {
            let host = reqwest::Url::parse(url)?
                .host_str()
                .unwrap_or("")
                .to_string();
            if host != "pximg.net" && !host.ends_with(".pximg.net") {
                return Err("unexpected image host".into());
            }
            let mut response = client
                .get(url)
                .header("referer", "https://www.pixiv.net/")
                .header("range", "bytes=0-511")
                .send()
                .await?;
            let success = response.status().is_success();
            let chunk = response.chunk().await?.ok_or("empty image response")?;
            image_verified = Some(success && image::guess_format(&chunk).is_ok());
        }
        results.push(json!({"id":id,"body_chars":content["text"].as_str().unwrap_or("").chars().count(),"image_references":content["text"].as_str().unwrap_or("").matches("[pixivimage:").count(),"illust_entries":content["illusts"].as_object().map(serde_json::Map::len),"caption_chars":content["detail"]["caption"].as_str().unwrap_or("").chars().count(),"tags":content["detail"]["tags"].as_array().map(Vec::len),"missing_illustrations":content.get("missing_illustrations").cloned().unwrap_or(json!([])),"available_image_verified":image_verified}));
    }
    std::fs::write(&args[1], serde_json::to_string_pretty(&results)?)?;
    println!(
        "Wrote acquisition counts and image checks for {} novels",
        results.len()
    );
    Ok(())
}
