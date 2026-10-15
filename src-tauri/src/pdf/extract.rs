//! PDF 一冊ぶんの取り出し。

use super::engine::bindings;
use super::layout::{self, Glyph, Line, Metrics};
use pdfium_render::prelude::*;
use std::path::Path;

/// どの経路で組んだか。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtractMethod {
    /// 構造ツリー。作者が書き残した段落の切れ目をそのまま使った。
    Tagged,
    /// 行の形から組み直した。段落の切れ目は推定である。
    Reflowed,
    /// ページによって経路が違う。
    Mixed,
}

impl ExtractMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            ExtractMethod::Tagged => "tagged",
            ExtractMethod::Reflowed => "reflowed",
            ExtractMethod::Mixed => "mixed",
        }
    }
}

/// 取り出した本文。
#[derive(Debug, Clone)]
pub struct PdfDocumentText {
    /// 段落。空文字列は作者が空けた行を表す。
    pub paragraphs: Vec<String>,
    pub method: ExtractMethod,
    pub page_count: usize,
    /// 空白を除いた文字数。
    pub char_count: usize,
}

/// 一ページぶんの中間結果。
struct PageWork {
    lines: Vec<Line>,
    tagged: Option<Vec<String>>,
    /// 構造ツリーが説明すべき、空白を除く文字数。
    ///
    /// タグ付きのページでは印の中にある文字だけを数える。外にあるものは
    /// `/Artifact`（ヘッダー・フッター・ページ番号）であって本文ではない。
    ink: usize,
    /// 印の外にあった、空白を除く文字数。
    unmarked: usize,
    /// 印の中にある最後の行の右端。ページ跨ぎの判断に使う。
    ///
    /// **`lines` の最後ではいけない。** フッターのあるページでは、紙面で最後に
    /// 来る行はフッターであって本文の続きではない。
    last_body_right: f32,
}

/// 添付 PDF から本文を取り出す。
pub fn extract_text(path: &Path) -> Result<PdfDocumentText, String> {
    let bindings = bindings()?;
    // 経路をファイル名に依存させない。pdfium の `FPDF_LoadDocument` が受け取る
    // のは C の文字列で、日本語を含む道の扱いが環境で変わる。読み込みはこちらで
    // 済ませ、pdfium には中身だけ渡す。
    let bytes = std::fs::read(path).map_err(|error| format!("PDF を読めません: {error}"))?;
    let document = unsafe { bindings.FPDF_LoadMemDocument64(&bytes, None) };
    if document.is_null() {
        return Err("PDF として開けません（壊れているか、鍵がかかっています）".to_string());
    }
    let result = read_document(bindings, document);
    unsafe { bindings.FPDF_CloseDocument(document) };
    // 文書を閉じるまで、pdfium はこの領域を読み続ける。
    drop(bytes);
    result
}

fn read_document(
    bindings: &dyn PdfiumLibraryBindings,
    document: FPDF_DOCUMENT,
) -> Result<PdfDocumentText, String> {
    let page_count = unsafe { bindings.FPDF_GetPageCount(document) };
    if page_count <= 0 {
        return Err("ページがありません".to_string());
    }
    let mut pages = Vec::with_capacity(page_count as usize);
    for index in 0..page_count {
        pages.push(read_page(bindings, document, index)?);
    }

    let line_pages: Vec<&[Line]> = pages.iter().map(|page| page.lines.as_slice()).collect();
    let metrics = layout::measure(&line_pages);

    let mut tagged_pages = 0usize;
    let mut reflowed_pages = 0usize;
    let mut per_page: Vec<Vec<String>> = Vec::with_capacity(pages.len());
    for page in &pages {
        // 構造ツリーが文字を取りこぼしたページは、行から組み直す。段落の精度を
        // 落としてでも、本文を欠けさせない。
        //
        // 印の外にある文字は落とす。ただし**落とす量が柱として説明できる大きさを
        // 超えたら、それは構造ツリーの取りこぼしである。** そのページはすべての
        // 文字を見る幾何経路へ回す。柱は題名とページ番号なので数十字に収まり、
        // 本文の1割に届くことはない。短いページのために絶対値も見る。
        let tagged = page.tagged.as_ref().filter(|paragraphs| {
            ink_count(paragraphs.iter().map(String::as_str)) == page.ink
                && (page.unmarked <= 40 || page.unmarked * 10 <= page.ink)
        });
        match tagged {
            Some(paragraphs) => {
                tagged_pages += 1;
                per_page.push(paragraphs.clone());
            }
            None => {
                reflowed_pages += 1;
                per_page.push(layout::reflow(&page.lines, &metrics));
            }
        }
    }

    let paragraphs = join_pages(&per_page, &pages, &metrics);
    if reflowed_pages > 0 && layout::looks_scrambled(&paragraphs) {
        return Err(
            "行の並びから本文を組み直せません（縦書きなど、まだ扱えない組みかもしれません）"
                .to_string(),
        );
    }
    let method = match (tagged_pages, reflowed_pages) {
        (_, 0) => ExtractMethod::Tagged,
        (0, _) => ExtractMethod::Reflowed,
        _ => ExtractMethod::Mixed,
    };
    let char_count = ink_count(paragraphs.iter().map(String::as_str));
    if char_count == 0 {
        return Err("この PDF には文字が入っていません（画像だけかもしれません）".to_string());
    }
    Ok(PdfDocumentText {
        paragraphs,
        method,
        page_count: page_count as usize,
        char_count,
    })
}

fn ink_count<'a>(parts: impl Iterator<Item = &'a str>) -> usize {
    parts
        .flat_map(str::chars)
        .filter(|ch| !ch.is_whitespace())
        .count()
}

fn read_page(
    bindings: &dyn PdfiumLibraryBindings,
    document: FPDF_DOCUMENT,
    index: i32,
) -> Result<PageWork, String> {
    let page = unsafe { bindings.FPDF_LoadPage(document, index) };
    if page.is_null() {
        return Err(format!("{} ページ目を開けません", index + 1));
    }
    let text_page = unsafe { bindings.FPDFText_LoadPage(page) };
    if text_page.is_null() {
        unsafe { bindings.FPDF_ClosePage(page) };
        return Err(format!("{} ページ目の文字を読めません", index + 1));
    }

    let glyphs = layout::read_glyphs(bindings, text_page);
    // 幾何経路はすべての文字を見る。構造ツリーが取りこぼしたときの受け皿なので、
    // ここで印の有無を理由に落とすと、落ちた先でも同じものが欠ける。
    let lines = layout::group_lines(&glyphs);
    let tagged = read_tagged(bindings, page, &glyphs);
    let marked = || glyphs.iter().filter(|glyph| glyph.mcid >= 0);
    let (ink, unmarked) = match tagged {
        Some(_) => (
            marked().filter(|glyph| !glyph.ch.is_whitespace()).count(),
            glyphs
                .iter()
                .filter(|glyph| glyph.mcid < 0 && !glyph.ch.is_whitespace())
                .count(),
        ),
        None => (
            glyphs
                .iter()
                .filter(|glyph| !glyph.ch.is_whitespace())
                .count(),
            0,
        ),
    };
    let last_body_right = match tagged {
        Some(_) => layout::group_lines(marked())
            .last()
            .map(|line| line.right)
            .unwrap_or_default(),
        None => lines.last().map(|line| line.right).unwrap_or_default(),
    };

    unsafe { bindings.FPDFText_ClosePage(text_page) };
    unsafe { bindings.FPDF_ClosePage(page) };
    Ok(PageWork {
        lines,
        tagged,
        ink,
        unmarked,
        last_body_right,
    })
}

/// 構造ツリーから段落を取り出す。タグが無ければ `None`。
fn read_tagged(
    bindings: &dyn PdfiumLibraryBindings,
    page: FPDF_PAGE,
    glyphs: &[Glyph],
) -> Option<Vec<String>> {
    let tree = unsafe { bindings.FPDF_StructTree_GetForPage(page) };
    if tree.is_null() {
        return None;
    }
    let runs = runs_by_mcid(glyphs);
    let mut paragraphs = Vec::new();
    let children = unsafe { bindings.FPDF_StructTree_CountChildren(tree) };
    for index in 0..children {
        let element = unsafe { bindings.FPDF_StructTree_GetChildAtIndex(tree, index) };
        if !element.is_null() {
            walk(bindings, element, &runs, 0, &mut paragraphs);
        }
    }
    unsafe { bindings.FPDF_StructTree_Close(tree) };
    Some(paragraphs)
}

/// marked content の番号ごとに文字を集める。
///
/// **番号を持たない空白は、直前の番号へ付ける。** Word は語の切れ目に置く空白を
/// 別の断片として出すことがあり、落とすと「思った？ 残念ながら」の空白が消える。
///
/// **番号を持たない文字のほうは落とす。** タグ付きの PDF で印の外にあるものは
/// `/Artifact`、すなわちヘッダー・フッター・ページ番号である。直前の段落へ
/// 付けると、ページ番号が本文の末尾に貼り付く。
fn runs_by_mcid(glyphs: &[Glyph]) -> Vec<(i32, String)> {
    let mut runs: Vec<(i32, String)> = Vec::new();
    let mut last: Option<i32> = None;
    for glyph in glyphs {
        let target = if glyph.mcid >= 0 {
            last = Some(glyph.mcid);
            Some(glyph.mcid)
        } else if glyph.ch.is_whitespace() {
            last
        } else {
            None
        };
        let Some(target) = target else {
            continue;
        };
        match runs.iter_mut().find(|(mcid, _)| *mcid == target) {
            Some((_, text)) => text.push(glyph.ch),
            None => runs.push((target, glyph.ch.to_string())),
        }
    }
    runs
}

/// 段落になる型の要素で切り出しながら、木を深さ優先で辿る。
///
/// 深さに上限を置くのは、壊れた PDF が作る循環から戻ってこなくならないため。
fn walk(
    bindings: &dyn PdfiumLibraryBindings,
    element: FPDF_STRUCTELEMENT,
    runs: &[(i32, String)],
    depth: usize,
    out: &mut Vec<String>,
) -> String {
    if depth > 32 {
        return String::new();
    }
    let kind = element_type(bindings, element);
    let children = unsafe { bindings.FPDF_StructElement_CountChildren(element) };
    let mut collected = String::new();
    for index in 0..children {
        let child = unsafe { bindings.FPDF_StructElement_GetChildAtIndex(element, index) };
        if !child.is_null() {
            collected.push_str(&walk(bindings, child, runs, depth + 1, out));
            continue;
        }
        let mcid = unsafe { bindings.FPDF_StructElement_GetChildMarkedContentID(element, index) };
        if mcid >= 0 {
            if let Some((_, text)) = runs.iter().find(|(id, _)| *id == mcid) {
                collected.push_str(text);
            }
        }
    }
    if is_block(&kind) {
        out.push(tidy(&collected));
        return String::new();
    }
    collected
}

fn is_block(kind: &str) -> bool {
    matches!(
        kind,
        "P" | "H1" | "H2" | "H3" | "H4" | "H5" | "H6" | "LI" | "LBody" | "Lbl" | "Figure"
    )
}

/// 段落の中の改行を畳む。
///
/// この改行は折り返しであって、作者が入れた行送りではない。棚の実物 4 冊の
/// 1,123 か所を、行が右端まで届いているかで調べたところ、折り返し以外の改行は
/// 一つも無かった。作者が行を空けるときは段落そのものが変わる。
fn tidy(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\r' && ch != '\n' {
            out.push(ch);
            continue;
        }
        // 続く改行をまとめて食べてから、繋ぎ目を見る。
        while chars
            .peek()
            .is_some_and(|next| *next == '\r' || *next == '\n')
        {
            chars.next();
        }
        if layout::needs_space(out.chars().last(), chars.peek().copied()) {
            out.push(' ');
        }
    }
    out.trim().to_string()
}

fn element_type(bindings: &dyn PdfiumLibraryBindings, element: FPDF_STRUCTELEMENT) -> String {
    let length = unsafe { bindings.FPDF_StructElement_GetType(element, std::ptr::null_mut(), 0) };
    if length <= 2 {
        return String::new();
    }
    let mut buffer = vec![0u8; length as usize];
    unsafe {
        bindings.FPDF_StructElement_GetType(
            element,
            buffer.as_mut_ptr() as *mut std::ffi::c_void,
            length,
        )
    };
    decode_utf16(&buffer)
}

fn decode_utf16(buffer: &[u8]) -> String {
    let units: Vec<u16> = buffer
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .take_while(|unit| *unit != 0)
        .collect();
    String::from_utf16_lossy(&units)
}

/// ページごとの段落を一本に繋ぐ。
///
/// **ページの最後の行が右端まで届いていれば、その段落は次のページへ続いている。**
/// 文末の約物で見分ける方法もあるが、段落の途中の改ページが句点の直後に来ると
/// 切り違える。棚の実物 63 か所では、二つの判定は完全に一致した。
fn join_pages(per_page: &[Vec<String>], pages: &[PageWork], metrics: &Metrics) -> Vec<String> {
    let tolerance = metrics.tolerance();
    let mut out: Vec<String> = Vec::new();
    for (index, paragraphs) in per_page.iter().enumerate() {
        let continues = index > 0 && pages[index - 1].last_body_right >= metrics.right - tolerance;
        for (position, paragraph) in paragraphs.iter().enumerate() {
            let merge = position == 0
                && continues
                && !paragraph.is_empty()
                && out.last().is_some_and(|last| !last.is_empty());
            if merge {
                if let Some(last) = out.last_mut() {
                    if layout::needs_space(last.chars().last(), paragraph.chars().next()) {
                        last.push(' ');
                    }
                    last.push_str(paragraph);
                }
                continue;
            }
            push_paragraph(&mut out, paragraph);
        }
    }
    while out.last().is_some_and(String::is_empty) {
        out.pop();
    }
    out
}

/// 空行が続いても一つにまとめる。先頭の空行は落とす。
fn push_paragraph(out: &mut Vec<String>, paragraph: &str) {
    if paragraph.is_empty() && (out.is_empty() || out.last().is_some_and(String::is_empty)) {
        return;
    }
    out.push(paragraph.to_string());
}
