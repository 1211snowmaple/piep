//! 取り出しの試験。材料は [`super::fixture`] が組み立てる。

use super::fixture::*;
use super::*;

#[test]
fn courier_lines_reach_the_measure_we_designed_for() {
    // 試験の前提そのものを確かめる。ここが崩れると、下の試験は
    // 「折り返しを繋いだ」ではなく「たまたま繋がった」になる。
    let full = LEFT + GLYPH * FULL as f32;
    assert_eq!(page_one_lines()[0].text.len(), FULL);
    assert_eq!(page_one_lines()[3].text.len(), FULL);
    assert!(full < 300.0, "紙面に収まっていること");
    assert!(
        LEFT + GLYPH * page_one_lines()[1].text.len() as f32 + GLYPH * 2.0 < full,
        "段落の終わりの行は、許容を超えて右端に届かないこと"
    );
}

#[test]
fn tagged_pdf_keeps_the_authors_paragraphs() {
    let dir = temp_dir();
    let path = write_pdf(&dir, true);
    let text = extract_text(&path).expect("取り出せること");

    assert_eq!(text.method, ExtractMethod::Tagged);
    assert_eq!(text.page_count, 2);
    assert_eq!(
        text.paragraphs,
        vec![
            // 折り返した二行が一つの段落に戻る。欧文なので繋ぎ目に空白が入る。
            "Alpha beta gamma delta epsilon zetaX etatheta".to_string(),
            "SecondPara.".to_string(),
            // ページをまたいだ段落が繋がる。
            "Third paragraph reaches the far edge and continues here.".to_string(),
            "Final line.".to_string(),
        ]
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn page_numbers_stay_out_of_the_body() {
    // 柱とページ番号は `/Artifact` で、印を持たない。**印の外にある文字を
    // 直前の段落へ付けると、ページ番号が本文の末尾に貼り付く。** また、それを
    // 本文の一部として数えると、毎ページで文字数が合わずに幾何経路へ落ちる。
    let dir = temp_dir();
    let path = write_pdf(&dir, true);
    let text = extract_text(&path).expect("取り出せること");

    assert_eq!(
        text.method,
        ExtractMethod::Tagged,
        "ページ番号があっても構造ツリーの経路から落ちないこと"
    );
    for paragraph in &text.paragraphs {
        assert!(
            !paragraph.contains(FOOTER),
            "ページ番号が本文に混ざっている: {paragraph}"
        );
    }
    assert!(
        !text.paragraphs.is_empty()
            && text.paragraphs[2] == "Third paragraph reaches the far edge and continues here.",
        "フッターを本文の最後の行と読み違えると、ページをまたぐ段落が切れる: {:?}",
        text.paragraphs
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn untagged_pdf_falls_back_to_the_line_shape() {
    let dir = temp_dir();
    let path = write_pdf(&dir, false);
    let text = extract_text(&path).expect("取り出せること");

    assert_eq!(text.method, ExtractMethod::Reflowed);
    assert_eq!(
        text.paragraphs,
        vec![
            "Alpha beta gamma delta epsilon zetaX etatheta".to_string(),
            // タグが無いほうは行送りの開きから空行を拾う。タグ付きは空の `/P`
            // が無ければ拾わない。**同じ紙面でも経路によって空行の扱いが違う。**
            String::new(),
            "SecondPara.".to_string(),
            "Third paragraph reaches the far edge and continues here.".to_string(),
            "Final line.".to_string(),
        ]
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_pdf_without_text_is_refused_rather_than_returned_empty() {
    let dir = temp_dir();
    let mut pdf = Pdf::new();
    let catalog = pdf.reserve();
    let pages = pdf.reserve();
    let page = pdf.reserve();
    let content = pdf.stream("");
    pdf.set(
        page,
        &format!(
            "<< /Type /Page /Parent {pages} 0 R /MediaBox [0 0 300 200] /Contents {content} 0 R >>"
        ),
    );
    pdf.set(
        pages,
        &format!("<< /Type /Pages /Kids [{page} 0 R] /Count 1 >>"),
    );
    pdf.set(catalog, &format!("<< /Type /Catalog /Pages {pages} 0 R >>"));
    let path = write(&dir, "empty.pdf", pdf.finish(catalog));

    let error = extract_text(&path).expect_err("文字が無いことを伝えること");
    assert!(error.contains("文字が入っていません"), "{error}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_file_that_is_not_a_pdf_reports_that_it_cannot_be_opened() {
    let dir = temp_dir();
    let path = write(&dir, "not.pdf", b"this is not a pdf".to_vec());
    let error = extract_text(&path).expect_err("開けないことを伝えること");
    assert!(error.contains("開けません"), "{error}");
    let _ = std::fs::remove_dir_all(dir);
}

/// 文字も図も無い、同じ大きさの紙。描いたものを比べる相手に使う。
fn blank_pdf(dir: &std::path::Path) -> std::path::PathBuf {
    let mut pdf = Pdf::new();
    let catalog = pdf.reserve();
    let pages = pdf.reserve();
    let page = pdf.reserve();
    let content = pdf.stream("");
    pdf.set(
        page,
        &format!(
            "<< /Type /Page /Parent {pages} 0 R /MediaBox [0 0 300 200] /Contents {content} 0 R >>"
        ),
    );
    pdf.set(
        pages,
        &format!("<< /Type /Pages /Kids [{page} 0 R] /Count 1 >>"),
    );
    pdf.set(catalog, &format!("<< /Type /Catalog /Pages {pages} 0 R >>"));
    write(dir, "blank.pdf", pdf.finish(catalog))
}

#[test]
fn a_page_is_drawn_at_the_asked_width_with_the_papers_proportions() {
    let dir = temp_dir();
    let path = write_pdf(&dir, true);
    let drawn = render_page(&path, 0, 600).expect("描けること");

    assert_eq!(drawn.page_count, 2);
    assert_eq!(drawn.width, 600);
    // 紙は 300×200。縦横の比を保つ。
    assert_eq!(drawn.height, 400);
    assert_eq!(&drawn.webp[..4], b"RIFF", "WebP として書き出されていること");
    assert_eq!(&drawn.webp[8..12], b"WEBP");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn what_is_printed_on_the_page_reaches_the_image() {
    // 白いままの紙を返していないことを、復号器を持たずに確かめる。同じ大きさ・
    // 同じ設定で、文字のある紙は無地の紙より必ず大きく符号化される。
    let dir = temp_dir();
    let printed = render_page(&write_pdf(&dir, true), 0, 600).expect("描けること");
    let blank = render_page(&blank_pdf(&dir), 0, 600).expect("描けること");

    assert_eq!(printed.height, blank.height);
    assert!(
        printed.webp.len() > blank.webp.len() * 2,
        "文字が紙に載っていない: 印刷あり {} バイト、無地 {} バイト",
        printed.webp.len(),
        blank.webp.len()
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_absurd_width_is_brought_back_into_range_and_a_missing_page_is_refused() {
    let dir = temp_dir();
    let path = write_pdf(&dir, true);
    // 画面の幅ではなく拡大率から来る要求があるので、上下とも丸める。
    assert_eq!(render_page(&path, 0, 1).expect("描けること").width, 320);
    assert_eq!(
        render_page(&path, 0, 100_000).expect("描けること").width,
        2400
    );

    let error = render_page(&path, 9, 600).expect_err("無いページを断ること");
    assert!(error.contains("全 2 ページ"), "{error}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn wrapped_lines_join_without_a_space_in_japanese() {
    // 日本語は語を空白で分けない。欧文と同じ規則で繋ぐと、本文に無い空白が入る。
    assert!(!super::layout::needs_space(Some('端'), Some('の')));
    assert!(super::layout::needs_space(Some('e'), Some('a')));
    assert!(!super::layout::needs_space(Some('。'), Some('あ')));
    assert!(!super::layout::needs_space(Some('e'), Some('。')));
}

#[test]
fn one_character_per_line_is_reported_instead_of_returned_as_prose() {
    // 縦書きをこの経路に通すと一字ずつが別の段落になる。文字は一つも落ちないので、
    // 欠落の検査では気づけない。
    let scrambled: Vec<String> = "縦書きの本文である".chars().map(String::from).collect();
    assert!(super::layout::looks_scrambled(&scrambled));

    let prose: Vec<String> = (0..10)
        .map(|_| "ふつうの段落である。".to_string())
        .collect();
    assert!(!super::layout::looks_scrambled(&prose));
}
