//! pixiv の web（ajax）側の口。
//!
//! [`aapi`](crate::pixiv_api::aapi) と同じ相手だが、返すものが違う。小説
//! シリーズについて言えば、アプリAPI `/v2/novel/series` が返すのは各話の
//! 一覧と概要だけで、**シリーズ自身の表紙も、公開話数も、完結かどうかも
//! 返さない**。表紙を各話の画像で代用していたころは、1話目が自前の表紙を
//! 持っていればそれが、持っていなければ結果的にシリーズの表紙が入る、という
//! 運任せになっていた。
//!
//! こちらはログインが要らず、R-18 のシリーズでも読める。読み方（上限つきで
//! 読む・レートリミットと 404 を型で分ける）はアプリAPIと同じ道具に揃えて
//! あるので、呼ぶ側から見た失敗の扱いは変わらない。

use std::time::Duration;

use serde_json::Value;

use crate::pixiv_api::error::PixivError;
use crate::pixiv_api::models::parse_response_into;

/// web 側のホスト。
pub const PIXIV_WEB_HOST: &str = "https://www.pixiv.net";
/// ajax は素っ気ない UA を嫌う。ブラウザとして名乗る。
pub const PIXIV_WEB_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

/// 小説シリーズについて、web だけが持っている情報。
///
/// どれも「取れなかった」と「無かった」を区別できるようにしてある。表紙の
/// 無いシリーズはあるし、返事の形が変われば取れなくなる - どちらの場合も、
/// 手元にある値を上書きしてはいけない。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NovelSeriesWeb {
    /// シリーズID（返事に入っていたもの）。
    pub id: Option<String>,
    pub title: Option<String>,
    pub caption: Option<String>,
    /// シリーズ自身の表紙。大きすぎない版を選んである。
    pub cover_url: Option<String>,
    /// 取得元で公開されている話数。
    pub published_content_count: Option<i64>,
    /// 完結しているか。
    pub is_concluded: Option<bool>,
    /// 取得元での最終更新（ISO8601）。
    pub updated_at: Option<String>,
    pub tags: Vec<String>,
}

/// 表紙として使う版。
///
/// 大きすぎない、確実にある順。`original` は PNG で数MBになることがあり、
/// 一覧のカードと 112px の枠に敷くだけなら要らない。
const COVER_PREFERENCE: [&str; 4] = ["1200x1200", "480mw", "240mw", "original"];

fn trimmed_string(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

/// 返事から表紙を選ぶ。
pub fn pick_cover_url(cover: Option<&Value>) -> Option<String> {
    let urls = cover?.get("urls")?;
    COVER_PREFERENCE
        .into_iter()
        .find_map(|key| trimmed_string(urls.get(key)))
}

/// `/ajax/novel/series/{id}` の返事を読む。
///
/// 通信から切り離してあるのは、返事の形が変わったときに困るのがここだから。
/// 形の検証はテストで固定できる。
pub fn parse_novel_series(payload: &Value) -> Result<NovelSeriesWeb, PixivError> {
    if payload
        .get("error")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Err(PixivError::ErrResponse {
            body: trimmed_string(payload.get("message"))
                .unwrap_or_else(|| "pixiv がシリーズを返しませんでした".to_string()),
        });
    }
    let body = payload
        .get("body")
        .filter(|body| body.is_object())
        .ok_or_else(|| PixivError::UnintelligibleResponse {
            body: "シリーズの本体が入っていません".to_string(),
        })?;

    Ok(NovelSeriesWeb {
        id: trimmed_string(body.get("id")),
        title: trimmed_string(body.get("title")),
        caption: trimmed_string(body.get("caption")),
        cover_url: pick_cover_url(body.get("cover")),
        // 数として返ってくるが、文字列で来ても読めるようにしておく。
        published_content_count: body
            .get("publishedContentCount")
            .and_then(|count| {
                count
                    .as_i64()
                    .or_else(|| count.as_str().and_then(|text| text.parse().ok()))
            })
            .filter(|count| *count >= 0),
        is_concluded: body.get("isConcluded").and_then(Value::as_bool),
        updated_at: trimmed_string(body.get("updateDate")),
        tags: body
            .get("tags")
            .and_then(Value::as_array)
            .map(|tags| {
                tags.iter()
                    .filter_map(|tag| trimmed_string(Some(tag)))
                    .collect()
            })
            .unwrap_or_default(),
    })
}

/// web 側の口。
pub struct WebPixivAPI {
    client: reqwest::Client,
    host: String,
}

impl WebPixivAPI {
    pub fn new() -> Result<Self, PixivError> {
        Self::with_host(PIXIV_WEB_HOST)
    }

    /// 宛先を差し替えられるようにしてあるのは、通信そのものを試すため。
    pub fn with_host(host: &str) -> Result<Self, PixivError> {
        let client = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .redirect(reqwest::redirect::Policy::limited(3))
            .build()?;
        Ok(Self {
            client,
            host: host.trim_end_matches('/').to_string(),
        })
    }

    /// シリーズ1件を読む。
    pub async fn novel_series(&self, series_id: &str) -> Result<NovelSeriesWeb, PixivError> {
        let series_id = series_id.trim();
        if series_id.is_empty() || !series_id.chars().all(|c| c.is_ascii_digit()) {
            return Err(PixivError::NotFound {
                body: format!("シリーズIDが不正です: {series_id}"),
            });
        }
        let response = self
            .client
            .get(format!("{}/ajax/novel/series/{series_id}", self.host))
            .header("referer", format!("{}/", self.host))
            .header("user-agent", PIXIV_WEB_USER_AGENT)
            .header("accept", "application/json")
            .send()
            .await?;
        let payload: Value = parse_response_into(response).await?;
        parse_novel_series(&payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 実際の返事から、必要な枝だけを写したもの。
    fn sample() -> Value {
        serde_json::json!({
            "error": false,
            "message": "",
            "body": {
                "id": "12552619",
                "userId": "6031869",
                "title": "マイナーキャラ妄想短編集　陵辱物",
                "caption": "ハーメルンで書いている短編集です。",
                "language": "ja",
                "tags": ["NTR", "催眠", "無様エロ"],
                "publishedContentCount": 89,
                "isConcluded": false,
                "updateDate": "2026-08-09T21:30:12+09:00",
                "cover": { "urls": {
                    "240mw": "https://i.pximg.net/c/240x480_80/novel-cover-master/sci12552619_x.jpg",
                    "480mw": "https://i.pximg.net/c/480x960/novel-cover-master/sci12552619_x.jpg",
                    "1200x1200": "https://i.pximg.net/c/1200x1200/novel-cover-master/sci12552619_x.jpg",
                    "128x128": "https://i.pximg.net/c/128x128/novel-cover-master/sci12552619_sq.jpg",
                    "original": "https://i.pximg.net/novel-cover-original/sci12552619_x.png"
                }}
            }
        })
    }

    #[test]
    fn reads_what_the_app_api_does_not_return() {
        let series = parse_novel_series(&sample()).unwrap();
        assert_eq!(
            series.cover_url.as_deref(),
            Some("https://i.pximg.net/c/1200x1200/novel-cover-master/sci12552619_x.jpg"),
            "大きすぎない版を選ぶ"
        );
        assert_eq!(series.published_content_count, Some(89));
        assert_eq!(series.is_concluded, Some(false));
        assert_eq!(series.title.as_deref(), Some("マイナーキャラ妄想短編集　陵辱物"));
        assert_eq!(series.tags.len(), 3);
    }

    /// 表紙の無いシリーズはある。「取れなかった」ではなく「無い」。
    #[test]
    fn a_series_without_a_cover_is_not_a_failure() {
        let mut payload = sample();
        payload["body"]["cover"] = serde_json::json!({ "urls": {} });
        let series = parse_novel_series(&payload).unwrap();
        assert_eq!(series.cover_url, None);
        assert_eq!(series.published_content_count, Some(89));

        payload["body"].as_object_mut().unwrap().remove("cover");
        assert_eq!(parse_novel_series(&payload).unwrap().cover_url, None);
    }

    /// 空文字は「ある」ではない。
    #[test]
    fn blank_values_are_treated_as_absent() {
        let mut payload = sample();
        payload["body"]["caption"] = serde_json::json!("   ");
        payload["body"]["cover"]["urls"]["1200x1200"] = serde_json::json!("");
        let series = parse_novel_series(&payload).unwrap();
        assert_eq!(series.caption, None);
        assert_eq!(
            series.cover_url.as_deref(),
            Some("https://i.pximg.net/c/480x960/novel-cover-master/sci12552619_x.jpg"),
            "空いた版は飛ばして次の版へ"
        );
    }

    #[test]
    fn an_error_answer_is_an_error() {
        let payload = serde_json::json!({
            "error": true, "message": "作品が見つかりません", "body": []
        });
        let error = parse_novel_series(&payload).unwrap_err();
        assert!(matches!(error, PixivError::ErrResponse { body } if body.contains("見つかりません")));
    }

    #[test]
    fn a_missing_body_is_not_silently_empty() {
        let payload = serde_json::json!({ "error": false, "body": [] });
        assert!(matches!(
            parse_novel_series(&payload).unwrap_err(),
            PixivError::UnintelligibleResponse { .. }
        ));
    }

    /// 数えられない話数を 0 として扱わない。
    #[test]
    fn a_count_that_is_not_a_count_stays_unknown() {
        let mut payload = sample();
        payload["body"]["publishedContentCount"] = serde_json::json!(null);
        assert_eq!(parse_novel_series(&payload).unwrap().published_content_count, None);
        payload["body"]["publishedContentCount"] = serde_json::json!("89");
        assert_eq!(
            parse_novel_series(&payload).unwrap().published_content_count,
            Some(89)
        );
    }

    #[tokio::test]
    async fn a_series_id_that_is_not_a_number_never_leaves_the_app() {
        let api = WebPixivAPI::new().unwrap();
        for id in ["", "  ", "12; DROP TABLE", "../../etc/passwd"] {
            assert!(matches!(
                api.novel_series(id).await.unwrap_err(),
                PixivError::NotFound { .. }
            ));
        }
    }
}
