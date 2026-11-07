//! pixiv の画像を、縮小版ではなく作者が上げたままの版で保存するための道具。
//!
//! pixiv の API は表紙を `c/240x480_80/novel-cover-master/...` の縮小版でしか
//! 返さず、本文から参照したイラストも `img-master/..._master1200.jpg`（1200px
//! に縮めて JPEG にした版）しか返さないことがある。原寸は同じ画像サーバーの
//! 決まった場所にあるが、拡張子（jpg/png/gif）だけは返事に書かれていない。
//! そこで候補を作り、`HEAD` で在るものを確かめてから使う。
//!
//! 取りに行く先は画像サーバー（`i.pximg.net`）だけで、アカウントの通信は
//! 増えない。
//!
//! **保存済みの JSON の `cover_url` は書き換えない。** 作者が表紙を変えたかの
//! 判断（内容の指紋）は `cover_url` で行うので、そこを原寸の URL に替えると、
//! 何も変わっていない作品がすべて改稿として数えられる。原寸は別の欄
//! `cover_original_url` に持つ。

use regex::Regex;
use serde_json::Value;
use std::sync::LazyLock;
use std::time::Duration;

/// 原寸の表紙を入れておく欄。`cover_url`（作者の変更を見る指紋）とは分ける。
pub(crate) const COVER_ORIGINAL_FIELD: &str = "cover_original_url";

const PIXIV_REFERER: &str = "https://www.pixiv.net/";
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

/// 表紙の縮小版と、縮めていない master 版。
static NOVEL_COVER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^https://i\.pximg\.net/(?P<crop>c/[^/]+/)?novel-cover-master/(?P<path>img/.+?)_master1200\.(?:jpg|png|gif)$",
    )
    .unwrap()
});

/// イラストの縮小版と master 版（`_p0_master1200.jpg`）。
static ILLUST: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^https://i\.pximg\.net/(?P<crop>c/[^/]+/)?img-master/(?P<path>img/.+?_p\d+)_master1200\.(?:jpg|png|gif)$",
    )
    .unwrap()
});

/// 縮小された画像の URL から、良い順に試す候補を作る。
///
/// 原寸（拡張子違い）を先に、最後に縮めていない master 版を置く（元が切り抜き
/// 版だったときだけ）。縮小版でない URL には空を返す。
pub(crate) fn original_candidates(url: &str) -> Vec<String> {
    let url = url.trim();
    let (captures, original_dir, extensions, master) =
        if let Some(captures) = NOVEL_COVER.captures(url) {
            (
                captures,
                "novel-cover-original",
                &["png", "jpg"][..],
                "novel-cover-master",
            )
        } else if let Some(captures) = ILLUST.captures(url) {
            (
                captures,
                "img-original",
                &["png", "jpg", "gif"][..],
                "img-master",
            )
        } else {
            return Vec::new();
        };
    let path = &captures["path"];
    let mut candidates: Vec<String> = extensions
        .iter()
        .map(|extension| format!("https://i.pximg.net/{original_dir}/{path}.{extension}"))
        .collect();
    if captures.name("crop").is_some() {
        candidates.push(format!(
            "https://i.pximg.net/{master}/{path}_master1200.jpg"
        ));
    }
    candidates
}

/// 縮小版として保存されている（原寸に取り直す対象になる）URL か。
pub(crate) fn is_reduced_pixiv_image(url: &str) -> bool {
    !original_candidates(url).is_empty()
}

/// 画像サーバー用の接続。認証は付けない。
pub(crate) fn cdn_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .read_timeout(Duration::from_secs(30))
        .build()
        .map_err(|error| format!("画像サーバーへの接続を作れません: {error}"))
}

/// 候補を一つずつ `HEAD` で確かめた結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OriginalLookup {
    /// 在った。良い順で最初のもの。
    Found(String),
    /// どの候補も 404 などで無かった。公開元で消えている。
    Unavailable,
}

/// 縮小版の URL から、実在する一番良い版を探す。
///
/// 通信が切れたときは `Err` を返す（呼び出し側が止まるか決める）。404 は
/// 「その拡張子では無い」の意味なので、次の候補へ進む。
pub(crate) async fn find_original(
    client: &reqwest::Client,
    url: &str,
) -> Result<OriginalLookup, String> {
    find_first_existing(client, original_candidates(url)).await
}

async fn find_first_existing(
    client: &reqwest::Client,
    candidates: Vec<String>,
) -> Result<OriginalLookup, String> {
    for candidate in candidates {
        let response = client
            .head(&candidate)
            .header("Referer", PIXIV_REFERER)
            .header("User-Agent", USER_AGENT)
            .send()
            .await
            .map_err(|error| format!("画像サーバーに接続できません: {error}"))?;
        let status = response.status();
        let is_image = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("image/"));
        if status.is_success() && is_image {
            return Ok(OriginalLookup::Found(candidate));
        }
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
            return Err(format!("画像サーバーが {status} を返しました"));
        }
    }
    Ok(OriginalLookup::Unavailable)
}

/// 保存する直前の pixiv の返事に、原寸の URL を書き足す。
///
/// - 表紙: `cover_original_url` に原寸を入れる（`cover_url` はそのまま）。
/// - webview のイラスト: `illust.images.original` が空なら原寸を入れる。
/// - App API から組んだイラスト: `pages[].urls.original` が縮小版なら原寸に
///   替え、元の縮小版は `regular` に残す。
///
/// 探せなかったものは触らない。保存は今までどおり縮小版で続く - 原寸を
/// 探す通信の失敗で、作品の保存そのものを止めない。
pub(crate) async fn add_original_urls(client: &reqwest::Client, data: &mut Value) {
    add_original_urls_with(data, |url| async move {
        match find_original(client, &url).await {
            Ok(OriginalLookup::Found(found)) => Some(found),
            Ok(OriginalLookup::Unavailable) => None,
            Err(error) => {
                log::warn!("原寸の画像を確認できません（縮小版で保存します）: {url}: {error}");
                None
            }
        }
    })
    .await
}

/// `add_original_urls` の本体。探し方を差し替えて試験できるように分けてある。
pub(crate) async fn add_original_urls_with<F, Fut>(data: &mut Value, mut find: F)
where
    F: FnMut(String) -> Fut,
    Fut: std::future::Future<Output = Option<String>>,
{
    if data
        .get(COVER_ORIGINAL_FIELD)
        .and_then(Value::as_str)
        .is_none_or(str::is_empty)
    {
        let cover = data
            .get("cover_url")
            .and_then(Value::as_str)
            .or_else(|| data.pointer("/detail/cover_url").and_then(Value::as_str))
            .filter(|url| is_reduced_pixiv_image(url))
            .map(str::to_string);
        if let Some(cover) = cover {
            if let Some(found) = find(cover).await {
                data[COVER_ORIGINAL_FIELD] = Value::String(found);
            }
        }
    }

    let Some(illusts) = data.get_mut("illusts").and_then(Value::as_object_mut) else {
        return;
    };
    for entry in illusts.values_mut() {
        if let Some(images) = entry
            .get_mut("illust")
            .and_then(|illust| illust.get_mut("images"))
            .and_then(Value::as_object_mut)
        {
            let has_original = images
                .get("original")
                .and_then(Value::as_str)
                .is_some_and(|url| !url.is_empty());
            if !has_original {
                let reduced = ["medium", "small"]
                    .iter()
                    .filter_map(|key| images.get(*key).and_then(Value::as_str))
                    .find(|url| is_reduced_pixiv_image(url))
                    .map(str::to_string);
                if let Some(reduced) = reduced {
                    if let Some(found) = find(reduced).await {
                        images.insert("original".to_string(), Value::String(found));
                    }
                }
            }
            continue;
        }
        let Some(pages) = entry.get_mut("pages").and_then(Value::as_array_mut) else {
            continue;
        };
        for page in pages {
            let Some(urls) = page.get_mut("urls").and_then(Value::as_object_mut) else {
                continue;
            };
            let Some(reduced) = urls
                .get("original")
                .and_then(Value::as_str)
                .filter(|url| is_reduced_pixiv_image(url))
                .map(str::to_string)
            else {
                continue;
            };
            if let Some(found) = find(reduced.clone()).await {
                urls.entry("regular".to_string())
                    .or_insert(Value::String(reduced));
                urls.insert("original".to_string(), Value::String(found));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const COVER_240: &str = "https://i.pximg.net/c/240x480_80/novel-cover-master/img/2024/11/15/19/39/13/ci23413112_bbb0c381096512a7d02ab80ef9ec4d6b_master1200.jpg";

    #[test]
    fn a_cropped_cover_offers_originals_then_the_uncropped_master() {
        assert_eq!(
            original_candidates(COVER_240),
            vec![
                "https://i.pximg.net/novel-cover-original/img/2024/11/15/19/39/13/ci23413112_bbb0c381096512a7d02ab80ef9ec4d6b.png",
                "https://i.pximg.net/novel-cover-original/img/2024/11/15/19/39/13/ci23413112_bbb0c381096512a7d02ab80ef9ec4d6b.jpg",
                "https://i.pximg.net/novel-cover-master/img/2024/11/15/19/39/13/ci23413112_bbb0c381096512a7d02ab80ef9ec4d6b_master1200.jpg",
            ]
        );
    }

    #[test]
    fn a_master_illustration_offers_only_originals() {
        let candidates = original_candidates(
            "https://i.pximg.net/img-master/img/2018/09/17/00/00/11/70731269_p0_master1200.jpg",
        );
        assert_eq!(
            candidates,
            vec![
                "https://i.pximg.net/img-original/img/2018/09/17/00/00/11/70731269_p0.png",
                "https://i.pximg.net/img-original/img/2018/09/17/00/00/11/70731269_p0.jpg",
                "https://i.pximg.net/img-original/img/2018/09/17/00/00/11/70731269_p0.gif",
            ]
        );
        let cropped = original_candidates(
            "https://i.pximg.net/c/600x1200_90/img-master/img/2018/10/05/01/57/48/71023777_p1_master1200.jpg",
        );
        assert_eq!(cropped.len(), 4);
        assert_eq!(
            cropped[3],
            "https://i.pximg.net/img-master/img/2018/10/05/01/57/48/71023777_p1_master1200.jpg"
        );
    }

    #[test]
    fn originals_placeholders_and_other_hosts_are_left_alone() {
        for url in [
            "https://i.pximg.net/novel-cover-original/img/2024/11/15/ci1_x.png",
            "https://i.pximg.net/img-original/img/2018/09/17/00/00/11/70731269_p0.png",
            "https://s.pximg.net/common/images/limit_r18_100.png",
            "https://example.com/c/240x480_80/novel-cover-master/img/a_master1200.jpg",
            "https://i.pximg.net/c/128x128/novel-cover-master/img/2024/x_square1200.jpg",
            "",
        ] {
            assert!(original_candidates(url).is_empty(), "{url}");
        }
    }

    #[tokio::test]
    async fn the_payload_gains_originals_without_touching_the_fingerprinted_cover() {
        let mut data = json!({
            "detail": {"cover_url": COVER_240},
            "cover_url": COVER_240,
            "illusts": {
                "70731269-1": {"page": 1, "illust": {"images": {
                    "small": null, "original": null,
                    "medium": "https://i.pximg.net/img-master/img/2018/09/17/00/00/11/70731269_p0_master1200.jpg"
                }}},
                "71023777": {"pages": [{"urls": {"original": "https://i.pximg.net/c/600x1200_90/img-master/img/2018/10/05/01/57/48/71023777_p0_master1200.jpg"}}]},
                "1": {"pages": [{"urls": {"original": "https://i.pximg.net/img-original/img/2020/01/01/00/00/00/1_p0.png"}}]}
            }
        });
        let before = crate::commands::downloader::compute_content_details(&data, "pixiv").0;
        add_original_urls_with(&mut data, |url| async move {
            original_candidates(&url).into_iter().nth(1)
        })
        .await;
        assert_eq!(data["cover_url"], COVER_240);
        assert_eq!(
            data[COVER_ORIGINAL_FIELD],
            "https://i.pximg.net/novel-cover-original/img/2024/11/15/19/39/13/ci23413112_bbb0c381096512a7d02ab80ef9ec4d6b.jpg"
        );
        assert_eq!(
            data["illusts"]["70731269-1"]["illust"]["images"]["original"],
            "https://i.pximg.net/img-original/img/2018/09/17/00/00/11/70731269_p0.jpg"
        );
        let page = &data["illusts"]["71023777"]["pages"][0]["urls"];
        assert_eq!(
            page["original"],
            "https://i.pximg.net/img-original/img/2018/10/05/01/57/48/71023777_p0.jpg"
        );
        assert!(page["regular"].as_str().unwrap().contains("600x1200_90"));
        assert_eq!(
            data["illusts"]["1"]["pages"][0]["urls"]["original"],
            "https://i.pximg.net/img-original/img/2020/01/01/00/00/00/1_p0.png"
        );
        // 作者の変更を見る指紋は、原寸を足しても変わらない。
        assert_eq!(
            before,
            crate::commands::downloader::compute_content_details(&data, "pixiv").0
        );
    }

    #[tokio::test]
    async fn nothing_changes_when_no_original_is_found() {
        let mut data = json!({"cover_url": COVER_240, "illusts": {}});
        let original = data.clone();
        add_original_urls_with(&mut data, |_| async { None }).await;
        assert_eq!(data, original);
    }

    #[tokio::test]
    async fn head_requests_pick_the_first_existing_extension() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let mut paths = Vec::new();
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = [0u8; 2048];
                let count = stream.read(&mut request).await.unwrap();
                let request = String::from_utf8_lossy(&request[..count]).to_string();
                assert!(request.starts_with("HEAD "), "{request}");
                assert!(request
                    .to_lowercase()
                    .contains("referer: https://www.pixiv.net/"));
                let path = request.split_whitespace().nth(1).unwrap().to_string();
                let response = if path.ends_with(".png") {
                    "HTTP/1.1 404 Not Found\r\nContent-Type: text/html\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                } else {
                    "HTTP/1.1 200 OK\r\nContent-Type: image/jpeg\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                };
                stream.write_all(response.as_bytes()).await.unwrap();
                paths.push(path);
            }
            paths
        });
        // 候補は本物と同じものを、手元の試験サーバーへ向け直して使う。
        let candidates: Vec<String> = original_candidates(COVER_240)
            .into_iter()
            .map(|url| url.replacen("https://i.pximg.net", &format!("http://{address}"), 1))
            .collect();
        let found = find_first_existing(&cdn_client().unwrap(), candidates)
            .await
            .unwrap();
        let paths = server.await.unwrap();
        assert!(paths[0].ends_with(".png"));
        assert!(paths[1].ends_with(".jpg"));
        let OriginalLookup::Found(found) = found else {
            panic!("jpg の原寸が見つかるはず");
        };
        assert!(found.ends_with("ci23413112_bbb0c381096512a7d02ab80ef9ec4d6b.jpg"));
    }

    #[tokio::test]
    async fn no_existing_candidate_means_unavailable_not_an_error() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for _ in 0..3 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = [0u8; 2048];
                let _ = stream.read(&mut request).await.unwrap();
                stream
                    .write_all(
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    )
                    .await
                    .unwrap();
            }
        });
        let candidates: Vec<String> = original_candidates(COVER_240)
            .into_iter()
            .map(|url| url.replacen("https://i.pximg.net", &format!("http://{address}"), 1))
            .collect();
        assert_eq!(
            find_first_existing(&cdn_client().unwrap(), candidates).await,
            Ok(OriginalLookup::Unavailable)
        );
        server.await.unwrap();
    }
}
