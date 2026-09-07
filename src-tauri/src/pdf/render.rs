//! 添付 PDF のページを絵にする。
//!
//! 取り出した本文は作者の段落を組み直したものであって、紙面そのものではない。
//! **図版のある PDF も、取り出しを信じきれないときも、原本を見る道が要る。**
//! piep は PDF を外のアプリへ渡さない（[`crate::commands::shell`] が起動して
//! よい種類を絞っている）ので、見る手段はアプリの中にしか作れない。

use super::engine::bindings;
use pdfium_render::prelude::*;
use std::path::Path;

/// 一ページぶんの絵。
pub struct RenderedPage {
    /// WebP で符号化した画素。
    pub webp: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// この PDF の総ページ数。呼び出し側が次のページを決めるのに使う。
    pub page_count: usize,
}

/// 画素そのものは出さない。**導出すると、失敗した試験が数百 KB を吐く。**
impl std::fmt::Debug for RenderedPage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RenderedPage")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("page_count", &self.page_count)
            .field("webp_bytes", &self.webp.len())
            .finish()
    }
}

/// 描く幅の下限と上限。
///
/// 上限があるのは、画面の幅ではなく拡大率で決まる要求が来るからである。
/// A4 を 2400px で描くと約 210dpi で、紙に印刷したものより細かい。
const MIN_WIDTH: u32 = 320;
const MAX_WIDTH: u32 = 2400;

/// 注釈も紙面の一部として描く（`FPDF_ANNOT`）。
///
/// 取り消し線や図形注釈は、作者が紙面に置いたものである。落とすと、原本を
/// 見に来た人が見たいものが消える。
const RENDER_FLAGS: i32 = 1;

/// 白で塗ってから描く（BGRA の 0xFFFFFFFF）。
///
/// **塗らないと透明のまま残る。** 余白は白い紙のつもりで作られているので、
/// 透明のまま暗い背景に載せると、本文が黒地に黒で出る。
const PAPER: u32 = 0xFFFF_FFFF;

/// PDF の1ページを WebP にする。`index` は 0 から数える。
pub fn render_page(path: &Path, index: usize, target_width: u32) -> Result<RenderedPage, String> {
    let bindings = bindings()?;
    // 道ではなく中身を渡す理由は [`super::extract::extract_text`] と同じ。
    let bytes = std::fs::read(path).map_err(|error| format!("PDF を読めません: {error}"))?;
    let document = unsafe { bindings.FPDF_LoadMemDocument64(&bytes, None) };
    if document.is_null() {
        return Err("PDF として開けません（壊れているか、鍵がかかっています）".to_string());
    }
    let result = render_from_document(bindings, document, index, target_width);
    unsafe { bindings.FPDF_CloseDocument(document) };
    // 文書を閉じるまで、pdfium はこの領域を読み続ける。
    drop(bytes);
    result
}

fn render_from_document(
    bindings: &dyn PdfiumLibraryBindings,
    document: FPDF_DOCUMENT,
    index: usize,
    target_width: u32,
) -> Result<RenderedPage, String> {
    let page_count = unsafe { bindings.FPDF_GetPageCount(document) };
    if page_count <= 0 {
        return Err("ページがありません".to_string());
    }
    let page_count = page_count as usize;
    if index >= page_count {
        return Err(format!(
            "{} ページ目はありません（全 {page_count} ページ）",
            index + 1
        ));
    }
    let page = unsafe { bindings.FPDF_LoadPage(document, index as i32) };
    if page.is_null() {
        return Err(format!("{} ページ目を開けません", index + 1));
    }
    let result = render_loaded_page(bindings, page, target_width);
    unsafe { bindings.FPDF_ClosePage(page) };
    result.map(|(webp, width, height)| RenderedPage {
        webp,
        width,
        height,
        page_count,
    })
}

fn render_loaded_page(
    bindings: &dyn PdfiumLibraryBindings,
    page: FPDF_PAGE,
    target_width: u32,
) -> Result<(Vec<u8>, u32, u32), String> {
    let points_wide = unsafe { bindings.FPDF_GetPageWidthF(page) };
    let points_high = unsafe { bindings.FPDF_GetPageHeightF(page) };
    if !(points_wide.is_finite()
        && points_high.is_finite()
        && points_wide > 0.0
        && points_high > 0.0)
    {
        return Err("ページの大きさを読めません".to_string());
    }
    let width = target_width.clamp(MIN_WIDTH, MAX_WIDTH);
    let height = ((width as f32) * points_high / points_wide)
        .round()
        .max(1.0) as u32;

    let bitmap = unsafe { bindings.FPDFBitmap_Create(width as i32, height as i32, 0) };
    if bitmap.is_null() {
        return Err("ページを描く場所を用意できません".to_string());
    }
    let pixels = unsafe {
        bindings.FPDFBitmap_FillRect(bitmap, 0, 0, width as i32, height as i32, PAPER);
        bindings.FPDF_RenderPageBitmap(
            bitmap,
            page,
            0,
            0,
            width as i32,
            height as i32,
            0,
            RENDER_FLAGS,
        );
        copy_as_rgb(bindings, bitmap, width, height)
    };
    unsafe { bindings.FPDFBitmap_Destroy(bitmap) };
    let pixels = pixels?;

    let webp = webp::Encoder::from_rgb(&pixels, width, height)
        .encode(QUALITY)
        .to_vec();
    Ok((webp, width, height))
}

/// WebP の品質。
///
/// 紙面は文字が主なので、輪郭が甘くなると読めなくなる。実測では 18 ページの
/// 文庫組みで 1 ページ 200KB 前後に収まる。
const QUALITY: f32 = 82.0;

/// pdfium の書き込んだ画素を RGB へ移す。
///
/// `FPDFBitmap_Create` にアルファを求めなかったので、並びは BGRx である。
/// 行の間隔は幅から計算せず `FPDFBitmap_GetStride` を使う。pdfium は行の頭を
/// 揃えるために余りを足すことがある。
///
/// # Safety
///
/// `bitmap` は `width` × `height` で作られ、まだ壊されていないこと。
unsafe fn copy_as_rgb(
    bindings: &dyn PdfiumLibraryBindings,
    bitmap: FPDF_BITMAP,
    width: u32,
    height: u32,
) -> Result<Vec<u8>, String> {
    let stride = unsafe { bindings.FPDFBitmap_GetStride(bitmap) };
    let buffer = unsafe { bindings.FPDFBitmap_GetBuffer(bitmap) } as *const u8;
    if buffer.is_null() || stride <= 0 {
        return Err("描いたページを読み出せません".to_string());
    }
    let stride = stride as usize;
    if stride < width as usize * 4 {
        return Err("描いたページの並びが読めません".to_string());
    }
    let mut rgb = Vec::with_capacity(width as usize * height as usize * 3);
    for row in 0..height as usize {
        let line = unsafe { std::slice::from_raw_parts(buffer.add(row * stride), stride) };
        for column in 0..width as usize {
            let pixel = &line[column * 4..column * 4 + 3];
            rgb.extend_from_slice(&[pixel[2], pixel[1], pixel[0]]);
        }
    }
    Ok(rgb)
}
