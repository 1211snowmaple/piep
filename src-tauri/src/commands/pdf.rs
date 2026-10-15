//! 添付 PDF の原本を、アプリの中で見る。
//!
//! 取り込んだ本文は段落を組み直したものである。図版のある PDF はそこに載らず、
//! 組み直しを信じきれない場合もある。**原本を確かめる道が要る。**
//!
//! 外のアプリへ渡す道は使えない。[`crate::commands::shell`] は起動してよい
//! 種類を絞っており、PDF はそこに入っていない。保存先の中身は取得元と、他人の
//! 作った書庫から来るので、piep が作る種類だけを通すという判断である。
//! だから見る手段はアプリの中に作る。

use crate::AppState;
use base64::Engine;
use std::path::Path;
use std::sync::Arc;
use tauri::Manager;

/// 原本の1ページ。
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PdfPageImage {
    /// `data:image/webp;base64,...` の形。画面はそのまま `img` に渡せる。
    pub image: String,
    pub width: u32,
    pub height: u32,
    /// この PDF の総ページ数。
    pub page_count: usize,
}

/// 添付 PDF の1ページを絵にして返す。`page` は 0 から数える。
///
/// **道は作品のアセットとして登録されているものに限る。** 受け取った文字列を
/// そのまま開くと、画面から任意のファイルを読ませる口になる。作品に属さない道は
/// 断る。控えは持たず、要求のたびに描く。1ページ 20ms 前後で終わる。
#[tauri::command]
pub async fn render_pdf_attachment_page(
    app: tauri::AppHandle,
    download_id: i64,
    local_path: String,
    page: usize,
    width: u32,
) -> Result<PdfPageImage, String> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    tokio::task::spawn_blocking(move || {
        let assets = state.db.get_assets(download_id)?;
        let asset = assets
            .iter()
            .find(|asset| asset.local_path == local_path)
            .ok_or_else(|| "この作品の添付ファイルではありません".to_string())?;
        if !Path::new(&asset.local_path)
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value.eq_ignore_ascii_case("pdf"))
        {
            return Err("PDF ではありません".to_string());
        }
        let rendered = crate::pdf::render_page(Path::new(&asset.local_path), page, width)?;
        let encoded = base64::engine::general_purpose::STANDARD.encode(&rendered.webp);
        Ok(PdfPageImage {
            image: format!("data:image/webp;base64,{encoded}"),
            width: rendered.width,
            height: rendered.height,
            page_count: rendered.page_count,
        })
    })
    .await
    .map_err(|error| format!("ページを描けませんでした: {error}"))?
}
