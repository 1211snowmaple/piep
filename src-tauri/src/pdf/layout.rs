//! 文字の位置から行と段落を組み直す。タグが無い PDF のための経路。

use pdfium_render::prelude::*;
use std::os::raw::c_double;

/// ページ上の一文字。
pub(super) struct Glyph {
    pub ch: char,
    /// 文字の原点（ベースライン）。
    ///
    /// **字形の外接矩形で行をまとめてはいけない。** `FPDFText_GetCharBox` は
    /// インクの矩形を返すので、「。」「、」「っ」が本文と違う高さに出る。実物で
    /// 試すと、37行のページが 206 行に割れた。
    pub origin_y: f64,
    pub right: f32,
    /// この文字が属する marked content の番号。持たないときは負。
    pub mcid: i32,
}

/// 一行。
pub(super) struct Line {
    pub text: String,
    pub origin_y: f64,
    pub right: f32,
}

/// 組みの寸法。文書全体から一度だけ推定する。
///
/// ページごとに測ると、最後のページのように行数の少ないページで右端を見誤る。
pub(super) struct Metrics {
    /// 本文の行送り。最頻値を採る。
    pub pitch: f64,
    /// 本文の右端。
    pub right: f32,
}

impl Metrics {
    /// 行が右端まで届いたと見なす許容。
    ///
    /// **字幅ではなく行送りから採る。** 禁則処理で 1〜2 字ぶん手前で折り返す行が
    /// あり、字幅ちょうどで測ると、それを段落の終わりと読み違える。棚の実物で
    /// 両方を測ったところ、行送りから採るほうが作り手をまたいで安定していた
    /// （段落の一致率で 92〜99% 対 84〜100%）。
    pub fn tolerance(&self) -> f32 {
        if self.pitch > 0.0 {
            (self.pitch * 1.2) as f32
        } else {
            12.0
        }
    }
}

/// ページの文字を、位置と marked content の番号付きで読む。
pub(super) fn read_glyphs(
    bindings: &dyn PdfiumLibraryBindings,
    text_page: FPDF_TEXTPAGE,
) -> Vec<Glyph> {
    let count = unsafe { bindings.FPDFText_CountChars(text_page) };
    let mut glyphs = Vec::with_capacity(count.max(0) as usize);
    // 同じ文字オブジェクトの番号を何度も引かない。ページ 1,300 文字に対して
    // オブジェクトは数十しかない。
    let mut object_mcid: Vec<(usize, i32)> = Vec::new();
    for index in 0..count {
        let code = unsafe { bindings.FPDFText_GetUnicode(text_page, index) };
        let Some(ch) = char::from_u32(code) else {
            continue;
        };
        let (mut x, mut y): (c_double, c_double) = (0.0, 0.0);
        unsafe {
            bindings.FPDFText_GetCharOrigin(text_page, index, &mut x, &mut y);
        }
        let mut rect = FS_RECTF {
            left: 0.0,
            top: 0.0,
            right: 0.0,
            bottom: 0.0,
        };
        unsafe {
            bindings.FPDFText_GetLooseCharBox(text_page, index, &mut rect);
        }
        let object = unsafe { bindings.FPDFText_GetTextObject(text_page, index) };
        let key = object as usize;
        let mcid = match object_mcid.iter().find(|(k, _)| *k == key) {
            Some((_, mcid)) => *mcid,
            None => {
                let mcid = unsafe { bindings.FPDFPageObj_GetMarkedContentID(object) };
                object_mcid.push((key, mcid));
                mcid
            }
        };
        glyphs.push(Glyph {
            ch,
            origin_y: y,
            right: rect.right,
            mcid,
        });
    }
    glyphs
}

/// 原点の高さが変わったところで行を切る。
pub(super) fn group_lines<'a>(glyphs: impl IntoIterator<Item = &'a Glyph>) -> Vec<Line> {
    let mut lines: Vec<Line> = Vec::new();
    for glyph in glyphs {
        if glyph.ch == '\r' || glyph.ch == '\n' {
            continue;
        }
        match lines.last_mut() {
            Some(line) if (line.origin_y - glyph.origin_y).abs() <= 1.0 => {
                line.text.push(glyph.ch);
                line.right = line.right.max(glyph.right);
            }
            _ => lines.push(Line {
                text: glyph.ch.to_string(),
                origin_y: glyph.origin_y,
                right: glyph.right,
            }),
        }
    }
    lines
}

/// 文書全体の行から、行送りと右端を推定する。
pub(super) fn measure(pages: &[&[Line]]) -> Metrics {
    let mut gaps: Vec<i64> = Vec::new();
    let mut right = 0.0f32;
    for lines in pages {
        for pair in lines.windows(2) {
            let gap = pair[0].origin_y - pair[1].origin_y;
            if gap > 0.5 {
                // 0.1pt 刻みに丸めて最頻値を採る。生の値は一致しない。
                gaps.push((gap * 10.0).round() as i64);
            }
        }
        for line in lines.iter() {
            right = right.max(line.right);
        }
    }
    let pitch = mode(&gaps).map(|value| value as f64 / 10.0).unwrap_or(0.0);
    Metrics { pitch, right }
}

fn mode(values: &[i64]) -> Option<i64> {
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let mut best = None;
    let mut best_count = 0usize;
    let mut index = 0usize;
    while index < sorted.len() {
        let value = sorted[index];
        let mut end = index;
        while end < sorted.len() && sorted[end] == value {
            end += 1;
        }
        if end - index > best_count {
            best_count = end - index;
            best = Some(value);
        }
        index = end;
    }
    best
}

/// 行を段落へ組み直す。空文字列は空行を表す。
///
/// 判定は三つ。行送りが本文の 1.5 倍を超えたら空行、前の行が右端まで届いて
/// いれば折り返し、届いていなければ段落の終わり。
pub(super) fn reflow(lines: &[Line], metrics: &Metrics) -> Vec<String> {
    let Some(first) = lines.first() else {
        return Vec::new();
    };
    let tolerance = metrics.tolerance();
    let mut out = Vec::new();
    let mut current = first.text.clone();
    for pair in lines.windows(2) {
        let (previous, line) = (&pair[0], &pair[1]);
        let gap = previous.origin_y - line.origin_y;
        if metrics.pitch > 0.0 && gap > metrics.pitch * 1.5 {
            out.push(std::mem::take(&mut current));
            out.push(String::new());
        } else if previous.right >= metrics.right - tolerance {
            if needs_space(current.chars().last(), line.text.chars().next()) {
                current.push(' ');
            }
            current.push_str(&line.text);
            continue;
        } else {
            out.push(std::mem::take(&mut current));
        }
        current = line.text.clone();
    }
    out.push(current);
    out
}

/// 折り返しを繋ぐとき、間に空白が要るか。
///
/// 日本語は語を空白で分けないので、折り返した行はそのまま繋ぐ。**欧文は繋ぐと
/// 語が潰れる。** 行末で切れた単語は PDF に空白として残らないため、字の種類で
/// 判断するしかない。
pub(super) fn needs_space(previous: Option<char>, next: Option<char>) -> bool {
    let (Some(previous), Some(next)) = (previous, next) else {
        return false;
    };
    previous.is_ascii_alphanumeric() && next.is_ascii_alphanumeric()
}

/// 組み直した結果が、そもそも横書きとして読めているか。
///
/// 縦書きの PDF をこの経路に通すと、一文字ずつが別の行になる。**文字は一つも
/// 落ちないので、欠落の検査では気づけない。** 段落あたりの字数で見張る。
pub(super) fn looks_scrambled(paragraphs: &[String]) -> bool {
    let filled: Vec<usize> = paragraphs
        .iter()
        .map(|text| text.chars().count())
        .filter(|count| *count > 0)
        .collect();
    if filled.len() < 8 {
        return false;
    }
    let short = filled.iter().filter(|count| **count <= 2).count();
    short * 2 > filled.len()
}
