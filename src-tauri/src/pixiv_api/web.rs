//! pixiv の web（ajax）側の口。
//!
//! [`aapi`](crate::pixiv_api::aapi) と同じ相手だが、返すものが違う。web だけが
//! 持っているのは次の四つで、どれもアプリAPIからは取れない。
//!
//! - **シリーズ自身の表紙**（`/v2/novel/series` は各話の画像しか返さない）
//! - **作品ごとの更新時刻** `updateDate`。アプリAPIの作品詳細には、投稿日は
//!   あっても改稿の時刻を表すフィールドが存在しない
//! - シリーズのタグと最新話ID
//! - 一覧を **1リクエスト100件** で返すこと。アプリAPIは30件固定
//!
//! 逆に web が持っていないのは、本文中の `[pixivimage:]` の解決である。
//! そこはアプリAPIの webview に任せる。**同じ仕事を両方にさせない。**
//!
//! # 認証
//!
//! 1件取得（[`novel_series`](WebPixivAPI::novel_series) など）はログイン不要で、
//! R-18 でも読める。しかし **一覧系はログインしていないと R-18 を黙って落とす** ため、
//! [`user_novels_by_ids`](WebPixivAPI::user_novels_by_ids) はセッションを要求する。
//!
//! 読み方（上限つきで読む・レートリミットと 404 を型で分ける）はアプリAPIと
//! 同じ道具に揃えてあるので、呼ぶ側から見た失敗の扱いは変わらない。

use std::time::Duration;

use serde_json::Value;

use crate::pixiv_api::error::PixivError;
use crate::pixiv_api::models::parse_response_into;

/// web 側のホスト。
pub const PIXIV_WEB_HOST: &str = "https://www.pixiv.net";
/// セッションを持たないときに名乗る UA。
///
/// ajax は UA を選ぶ。無くても既知のスクレイパ名（`Python-urllib` など）で
/// なければ通るが、Cloudflare の判定は予告なく変わるので、素直にブラウザとして
/// 名乗っておく。**セッションを送るときは話が別で、`cf_clearance` が発行時の
/// UA に紐づくため、必ず取得時の UA と一致させる**（[`WebSession`] を参照）。
pub const PIXIV_WEB_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
/// 一覧が1リクエストで受け取れる ID の数。101 件から 400 が返る。
pub const MAX_IDS_PER_REQUEST: usize = 100;

/// web にログイン済みとして名乗るための一式。
///
/// Cookie と UA は **対でしか意味を持たない**。`cf_clearance` は発行された
/// ときの UA に紐づいていて、別の UA で送ると弾かれる。片方だけ保存して
/// おいて後からもう片方を作る、ということはできない。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebSession {
    /// `PHPSESSID=…; cf_clearance=…` の形の Cookie 文字列。
    pub cookie: String,
    /// その Cookie を受け取ったときの User-Agent。
    pub user_agent: String,
}

impl WebSession {
    /// 両方が揃っているときだけ作る。
    ///
    /// 片方が空なら `None`。呼ぶ側で「Cookie はあるが UA が無い」状態を
    /// 組み立てられないようにしておく。
    pub fn new(cookie: &str, user_agent: &str) -> Option<Self> {
        let cookie = cookie.trim();
        let user_agent = user_agent.trim();
        (!cookie.is_empty() && !user_agent.is_empty()).then(|| Self {
            cookie: cookie.to_string(),
            user_agent: user_agent.to_string(),
        })
    }
}

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

/// web から読んだ本文まわり。
///
/// webview 経路が壊れたときの二番手として使う。**本文の中身は webview と
/// 完全に同じ**（実測で SHA-256 が一致）なので、保存した原本の見た目は変わらない。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NovelBodyWeb {
    /// 本文。`[newpage]` などの記法もそのまま。
    pub text: String,
    /// 表紙。
    pub cover_url: Option<String>,
    /// 本文に貼られた投稿画像。webview の `images` と同じ形。
    pub images: Value,
}

impl NovelBodyWeb {
    /// 本文が、web だけでは解決できない挿絵を参照しているか。
    ///
    /// `[pixivimage:…]` はイラスト作品への参照で、URL を得るには
    /// 1枚ごとに別の問い合わせが要る。webview はこれを解決して返すが、
    /// web は記法のまま置いていく。**欠けたまま保存しないための判定。**
    pub fn references_unresolved_illusts(&self) -> bool {
        self.text.contains("[pixivimage:")
    }
}

/// `/ajax/novel/{id}` の返事から、本文まわりだけを読む。
pub fn parse_novel_body(payload: &Value) -> Result<NovelBodyWeb, PixivError> {
    if payload
        .get("error")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Err(PixivError::ErrResponse {
            body: trimmed_string(payload.get("message"))
                .unwrap_or_else(|| "pixiv が作品を返しませんでした".to_string()),
        });
    }
    let body = payload
        .get("body")
        .filter(|body| body.is_object())
        .ok_or_else(|| PixivError::UnintelligibleResponse {
            body: "作品の本体が入っていません".to_string(),
        })?;
    // 本文が無いのは「空の作品」ではなく「読めなかった」。落とさずに気づく。
    let text = body
        .get("content")
        .and_then(Value::as_str)
        .ok_or_else(|| PixivError::UnintelligibleResponse {
            body: "本文が入っていません".to_string(),
        })?
        .to_string();
    Ok(NovelBodyWeb {
        text,
        cover_url: trimmed_string(body.get("coverUrl")),
        images: body
            .get("textEmbeddedImages")
            .filter(|images| images.is_object())
            .cloned()
            .unwrap_or_else(|| Value::Object(serde_json::Map::new())),
    })
}

/// 一覧が1件について返すもの。
///
/// 更新確認に要る材料が、本文を取りに行かずに全部そろう。`update_date` が
/// 主役で、残りは「本当に変わったのか」を裏取りするために持っている。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NovelListEntryWeb {
    /// 作品ID。
    pub id: String,
    /// 取得元での最終更新（ISO8601）。`/ajax/novel/{id}` の `uploadDate` と同じ瞬間。
    pub update_date: Option<String>,
    /// 投稿日時（ISO8601）。編集しても動かない。
    pub create_date: Option<String>,
    /// 文字数。piep が保存している `text_length` と同じ数え方。
    pub text_count: Option<i64>,
    pub title: Option<String>,
    /// キャプション。HTML のまま返る。
    pub description: Option<String>,
    pub tags: Vec<String>,
    /// R-18 かどうか（0 なら全年齢）。
    pub x_restrict: Option<i64>,
    /// 所属シリーズ。シリーズに入っていない作品では欠ける。
    pub series_id: Option<String>,
    pub series_title: Option<String>,
    /// シリーズの中での話数。
    pub series_content_order: Option<i64>,
    /// 伏せられている状態か。
    ///
    /// 伏せられた作品はメタデータが当てにならない。**変化の判定に使わず、
    /// 従来どおり詳細まで取りに行く**ための札として持つ。
    pub is_masked: bool,
}

fn number_at(value: Option<&Value>) -> Option<i64> {
    value.and_then(|value| {
        value
            .as_i64()
            .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
    })
}

/// `/ajax/user/{id}/novels` の返事を読む。
///
/// `requested` は投げた ID の数。**返ってきた数がこれに満たなければ、
/// 中身がどれだけ正しく読めてもエラーにする。** pixiv は R-18 を落とすとき
/// 何も言わないので、ここで数えないと「更新なし」と区別がつかなくなる。
pub fn parse_user_novels(
    payload: &Value,
    requested: usize,
) -> Result<Vec<NovelListEntryWeb>, PixivError> {
    if payload
        .get("error")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Err(PixivError::ErrResponse {
            body: trimmed_string(payload.get("message"))
                .unwrap_or_else(|| "pixiv が一覧を返しませんでした".to_string()),
        });
    }
    let body = payload
        .get("body")
        .and_then(Value::as_object)
        .ok_or_else(|| PixivError::UnintelligibleResponse {
            body: "一覧の本体が入っていません".to_string(),
        })?;

    if body.len() != requested {
        return Err(PixivError::PartialListing {
            requested,
            returned: body.len(),
        });
    }

    let mut entries = Vec::with_capacity(body.len());
    for (key, value) in body {
        entries.push(NovelListEntryWeb {
            id: trimmed_string(value.get("id")).unwrap_or_else(|| key.clone()),
            update_date: trimmed_string(value.get("updateDate")),
            create_date: trimmed_string(value.get("createDate")),
            text_count: number_at(value.get("textCount")).filter(|count| *count >= 0),
            title: trimmed_string(value.get("title")),
            description: trimmed_string(value.get("description")),
            tags: value
                .get("tags")
                .and_then(Value::as_array)
                .map(|tags| {
                    tags.iter()
                        .filter_map(|tag| trimmed_string(Some(tag)))
                        .collect()
                })
                .unwrap_or_default(),
            x_restrict: number_at(value.get("xRestrict")),
            series_id: trimmed_string(value.get("seriesId"))
                .or_else(|| number_at(value.get("seriesId")).map(|id| id.to_string())),
            series_title: trimmed_string(value.get("seriesTitle")),
            series_content_order: number_at(value.get("seriesContentOrder")),
            is_masked: value
                .get("isMasked")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        });
    }
    Ok(entries)
}

/// 数字だけの ID か。おかしなものは通信に出さない。
fn is_numeric_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_digit())
}

/// web 側の口。
pub struct WebPixivAPI {
    client: reqwest::Client,
    host: String,
    session: Option<WebSession>,
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
            session: None,
        })
    }

    /// ログイン済みとして名乗る。
    ///
    /// 一覧系は、これが無いと R-18 を落とした返事が返る。落とされたことは
    /// 返事からは分からないので、**セッションの有無は呼ぶ側が知っていなければ
    /// ならない**。だからこれは省略可能な設定ではなく、型で要求する
    /// （[`user_novels_by_ids`](Self::user_novels_by_ids) を参照）。
    pub fn with_session(mut self, session: WebSession) -> Self {
        self.session = Some(session);
        self
    }

    /// この口がログイン済みか。
    pub fn has_session(&self) -> bool {
        self.session.is_some()
    }

    /// 送る UA を決める。
    ///
    /// セッションがあるなら、それを受け取ったときの UA でなければならない。
    /// `cf_clearance` は UA に紐づいていて、揃っていないと弾かれる。
    fn user_agent(&self) -> &str {
        self.session
            .as_ref()
            .map(|session| session.user_agent.as_str())
            .unwrap_or(PIXIV_WEB_USER_AGENT)
    }

    fn get(&self, url: String) -> reqwest::RequestBuilder {
        let request = self
            .client
            .get(url)
            .header("referer", format!("{}/", self.host))
            .header("user-agent", self.user_agent())
            .header("accept", "application/json");
        match self.session.as_ref() {
            Some(session) => request.header("cookie", session.cookie.as_str()),
            None => request,
        }
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
            .get(format!("{}/ajax/novel/series/{series_id}", self.host))
            .send()
            .await?;
        let payload: Value = parse_response_into(response).await?;
        parse_novel_series(&payload)
    }

    /// 作品1件を読む。本文まで返る。
    ///
    /// ログインは要らず、R-18 でも読める（実測）。webview 経路が壊れたときの
    /// 二番手として使う。
    pub async fn novel(&self, novel_id: &str) -> Result<NovelBodyWeb, PixivError> {
        let novel_id = novel_id.trim();
        if !is_numeric_id(novel_id) {
            return Err(PixivError::NotFound {
                body: format!("作品IDが不正です: {novel_id}"),
            });
        }
        let response = self
            .get(format!("{}/ajax/novel/{novel_id}", self.host))
            .send()
            .await?;
        let payload: Value = parse_response_into(response).await?;
        parse_novel_body(&payload)
    }

    /// 作者の作品を、ID を指定してまとめて読む。
    ///
    /// 1回で最大 [`MAX_IDS_PER_REQUEST`] 件。それを超える分は切って繰り返す。
    /// 返る順序は取得元の都合で決まるので、呼ぶ側は ID で引き当てること。
    ///
    /// **セッションが要る。** 無いまま呼ぶと R-18 が黙って落ちた返事が返り、
    /// それを「更新なし」と読んでしまう。だから未設定は
    /// [`PixivError::NoAuth`] で断る — 静かに間違うより、うるさく止まるほうがよい。
    ///
    /// 1件でも足りなければ [`PixivError::PartialListing`] を返す。**足りた分だけ
    /// 返す、という妥協はしない。**
    pub async fn user_novels_by_ids(
        &self,
        user_id: &str,
        novel_ids: &[String],
    ) -> Result<Vec<NovelListEntryWeb>, PixivError> {
        if self.session.is_none() {
            return Err(PixivError::NoAuth);
        }
        let user_id = user_id.trim();
        if !is_numeric_id(user_id) {
            return Err(PixivError::NotFound {
                body: format!("作者IDが不正です: {user_id}"),
            });
        }
        if let Some(bad) = novel_ids.iter().find(|id| !is_numeric_id(id.trim())) {
            return Err(PixivError::NotFound {
                body: format!("作品IDが不正です: {bad}"),
            });
        }

        let mut entries = Vec::with_capacity(novel_ids.len());
        for chunk in novel_ids.chunks(MAX_IDS_PER_REQUEST) {
            let query = chunk
                .iter()
                .map(|id| format!("ids%5B%5D={}", id.trim()))
                .collect::<Vec<_>>()
                .join("&");
            let url = format!(
                "{}/ajax/user/{user_id}/novels?{query}&work_category=novels&is_first_page=0&lang=ja",
                self.host
            );
            let response = self.get(url).send().await?;
            let payload: Value = parse_response_into(response).await?;
            entries.extend(parse_user_novels(&payload, chunk.len())?);
        }
        Ok(entries)
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

    /// 一覧の返事から、必要な枝だけを写したもの。
    fn listing(ids: &[&str]) -> Value {
        let mut body = serde_json::Map::new();
        for (index, id) in ids.iter().enumerate() {
            body.insert(
                (*id).to_string(),
                serde_json::json!({
                    "id": id,
                    "title": format!("作品 {index}"),
                    "xRestrict": 1,
                    "tags": ["R-18", "連載"],
                    "textCount": 25885,
                    "wordCount": 13013,
                    "description": "書きました。",
                    "createDate": "2026-07-26T00:00:01+09:00",
                    "updateDate": "2026-07-27T00:02:20+09:00",
                    "isMasked": false,
                    "seriesId": "8434120",
                    "seriesTitle": "続きもの",
                    "seriesContentOrder": 150,
                }),
            );
        }
        serde_json::json!({ "error": false, "message": "", "body": body })
    }

    #[test]
    fn reads_what_the_app_api_has_no_field_for() {
        let entries = parse_user_novels(&listing(&["28692755"]), 1).unwrap();
        let entry = &entries[0];
        assert_eq!(entry.id, "28692755");
        assert_eq!(
            entry.update_date.as_deref(),
            Some("2026-07-27T00:02:20+09:00"),
            "アプリAPIの作品詳細には、これに当たるフィールドが無い"
        );
        assert_eq!(entry.create_date.as_deref(), Some("2026-07-26T00:00:01+09:00"));
        assert_eq!(entry.text_count, Some(25_885));
        assert_eq!(entry.x_restrict, Some(1));
        assert_eq!(entry.tags.len(), 2);
    }

    /// これが今回いちばん大事なテスト。
    ///
    /// pixiv はログインしていないと R-18 を落とすが、そのことを返事に書かない。
    /// `error` は false のままで、件数だけが減る。足りないまま素通しすると、
    /// セッションが切れた日にライブラリ全体が「最新です」になる。
    #[test]
    fn an_answer_that_came_back_short_is_never_silence() {
        let payload = listing(&["1", "2", "3", "4", "5", "6", "7"]);
        let error = parse_user_novels(&payload, 100).unwrap_err();
        assert!(
            matches!(
                error,
                PixivError::PartialListing {
                    requested: 100,
                    returned: 7
                }
            ),
            "{error:?}"
        );
    }

    /// 多く返ってきたときも、頼んだものと違う返事であることに変わりはない。
    #[test]
    fn an_answer_that_came_back_long_is_also_refused() {
        let payload = listing(&["1", "2", "3"]);
        assert!(matches!(
            parse_user_novels(&payload, 2).unwrap_err(),
            PixivError::PartialListing {
                requested: 2,
                returned: 3
            }
        ));
    }

    /// シリーズに入っていない作品では、シリーズの枝がそもそも無い。
    /// 「0話」ではなく「無い」。
    #[test]
    fn a_work_outside_a_series_has_no_series_fields() {
        let mut payload = listing(&["100"]);
        let entry = payload["body"]["100"].as_object_mut().unwrap();
        entry.remove("seriesId");
        entry.remove("seriesTitle");
        entry.remove("seriesContentOrder");
        let parsed = parse_user_novels(&payload, 1).unwrap();
        assert_eq!(parsed[0].series_id, None);
        assert_eq!(parsed[0].series_content_order, None);
        assert_eq!(
            parsed[0].update_date.as_deref(),
            Some("2026-07-27T00:02:20+09:00"),
            "シリーズが無いだけで、更新時刻まで捨てない"
        );
    }

    /// 伏せられた作品は、メタデータを当てにしない札を立てて返す。
    #[test]
    fn a_masked_work_carries_its_own_warning() {
        let mut payload = listing(&["100"]);
        payload["body"]["100"]["isMasked"] = serde_json::json!(true);
        assert!(parse_user_novels(&payload, 1).unwrap()[0].is_masked);
    }

    /// 数えられない文字数を 0 として扱わない。
    #[test]
    fn a_text_count_that_is_not_a_count_stays_unknown() {
        let mut payload = listing(&["100"]);
        payload["body"]["100"]["textCount"] = serde_json::json!(null);
        assert_eq!(parse_user_novels(&payload, 1).unwrap()[0].text_count, None);
        payload["body"]["100"]["textCount"] = serde_json::json!("25885");
        assert_eq!(
            parse_user_novels(&payload, 1).unwrap()[0].text_count,
            Some(25_885)
        );
    }

    #[test]
    fn an_error_answer_from_the_listing_is_an_error() {
        let payload = serde_json::json!({
            "error": true, "message": "不正なリクエストです。", "body": []
        });
        assert!(matches!(
            parse_user_novels(&payload, 1).unwrap_err(),
            PixivError::ErrResponse { body } if body.contains("不正")
        ));
    }

    /// 400 のときの `body` は配列で返る。空の一覧として読まない。
    #[test]
    fn a_body_that_is_not_a_map_is_not_an_empty_listing() {
        let payload = serde_json::json!({ "error": false, "body": [] });
        assert!(matches!(
            parse_user_novels(&payload, 1).unwrap_err(),
            PixivError::UnintelligibleResponse { .. }
        ));
    }

    /// 退避路が読むのは本文・表紙・投稿画像の三つ。
    #[test]
    fn the_fallback_reads_the_body_the_same_shape_as_the_webview() {
        let payload = serde_json::json!({
            "error": false, "body": {
                "content": "一行目\n[newpage]\n二行目",
                "coverUrl": "https://i.pximg.net/novel-cover-original/example.jpg",
                "textEmbeddedImages": { "1": { "novelImageId": "1", "sl": 0, "urls": {} } },
            }
        });
        let body = parse_novel_body(&payload).unwrap();
        assert!(body.text.contains("[newpage]"), "記法はそのまま残す");
        assert_eq!(
            body.cover_url.as_deref(),
            Some("https://i.pximg.net/novel-cover-original/example.jpg")
        );
        assert_eq!(body.images.as_object().map(|images| images.len()), Some(1));
        assert!(!body.references_unresolved_illusts());
    }

    /// 本文が空なのと、本文が入っていないのは別のこと。
    #[test]
    fn a_missing_body_text_is_not_an_empty_novel() {
        let payload = serde_json::json!({ "error": false, "body": { "coverUrl": "x" } });
        assert!(matches!(
            parse_novel_body(&payload).unwrap_err(),
            PixivError::UnintelligibleResponse { .. }
        ));
        // 空文字は「空の本文」。読めているので落とさない。
        let empty = serde_json::json!({ "error": false, "body": { "content": "" } });
        assert_eq!(parse_novel_body(&empty).unwrap().text, "");
    }

    /// web は挿絵の参照を記法のまま置いていく。欠けたまま保存しないよう、
    /// 呼ぶ側が気づけるようにしておく。
    #[test]
    fn a_body_that_references_illusts_says_so() {
        let payload = serde_json::json!({
            "error": false, "body": { "content": "本文\n[pixivimage:96933079-1]\n続き" }
        });
        assert!(parse_novel_body(&payload)
            .unwrap()
            .references_unresolved_illusts());
    }

    /// 画像が無い作品では、空の入れ物を返す。null を持ち回らない。
    #[test]
    fn a_body_without_images_carries_an_empty_map() {
        let payload = serde_json::json!({
            "error": false, "body": { "content": "本文", "textEmbeddedImages": null }
        });
        let body = parse_novel_body(&payload).unwrap();
        assert_eq!(body.images.as_object().map(|images| images.len()), Some(0));
    }

    /// Cookie と UA は対でしか意味を持たない。片方だけでは組み立てられない。
    #[test]
    fn a_session_needs_both_halves() {
        assert!(WebSession::new("PHPSESSID=x", "piep/0.9.0").is_some());
        assert!(WebSession::new("", "piep/0.9.0").is_none());
        assert!(WebSession::new("PHPSESSID=x", "   ").is_none());
    }

    /// セッションが無いまま一覧を呼ぶと、静かに間違った答えが返る。
    /// 通信に出る前に断る。
    #[tokio::test]
    async fn the_listing_refuses_to_run_without_a_session() {
        let api = WebPixivAPI::new().unwrap();
        assert!(!api.has_session());
        let error = api
            .user_novels_by_ids("281681", &["28692755".to_string()])
            .await
            .unwrap_err();
        assert!(matches!(error, PixivError::NoAuth), "{error:?}");
    }

    #[tokio::test]
    async fn ids_that_are_not_numbers_never_leave_the_app() {
        let api = WebPixivAPI::new()
            .unwrap()
            .with_session(WebSession::new("PHPSESSID=x", "piep/0.9.0").unwrap());
        for bad in ["", "  ", "12; DROP TABLE", "../../etc/passwd"] {
            assert!(matches!(
                api.user_novels_by_ids("281681", &[bad.to_string()])
                    .await
                    .unwrap_err(),
                PixivError::NotFound { .. }
            ));
            assert!(matches!(
                api.user_novels_by_ids(bad, &["28692755".to_string()])
                    .await
                    .unwrap_err(),
                PixivError::NotFound { .. }
            ));
        }
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
