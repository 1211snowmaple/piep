//! 添付 PDF の原本を、アプリの中で見る。
//!
//! 取り込んだ本文は段落を組み直したものである。図版のある PDF はそこに載らず、
//! 組み直しを信じきれない場合もある。**原本を確かめる道が要る。**
//!
//! 外のアプリへ渡す道は使えない。[`crate::commands::shell`] は起動してよい
//! 種類を絞っており、PDF はそこに入っていない。保存先の中身は取得元と、他人の
//! 作った書庫から来るので、piep が作る種類だけを通すという判断である。
//! だから見る手段はアプリの中に作る。

use base64::Engine;

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

impl From<crate::pdf::RenderedPage> for PdfPageImage {
    fn from(rendered: crate::pdf::RenderedPage) -> Self {
        let encoded = base64::engine::general_purpose::STANDARD.encode(&rendered.webp);
        Self {
            image: format!("data:image/webp;base64,{encoded}"),
            width: rendered.width,
            height: rendered.height,
            page_count: rendered.page_count,
        }
    }
}
