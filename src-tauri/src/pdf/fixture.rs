//! 試験のための PDF を組み立てる。
//!
//! 棚の実物（すべて Word と Google ドキュメント由来のタグ付き）は CI から見えず、
//! 他人の作品なのでリポジトリにも置けない。代わりに、**実物で確かめた性質だけを
//! 持つ最小の PDF** をその場で書き出す。
//!
//! - 折り返した行が一つの `/P` に入っている
//! - 段落の終わりの行は右端まで届かない
//! - 段落がページをまたぐ
//!
//! 実物での確認は `cargo run --example pdf_attachment_probe` で行う。

use std::path::PathBuf;

/// Courier は標準14書体の一つで、**どの字も 600/1000 幅**である。
/// 12pt なら一字 7.2pt ちょうどになり、行の右端を計算で決められる。
pub(crate) const GLYPH: f32 = 7.2;
pub(crate) const LEFT: f32 = 20.0;
/// 右端まで届く行の字数。
pub(crate) const FULL: usize = 36;

pub(crate) fn temp_dir() -> PathBuf {
    let path = std::env::temp_dir().join(format!("piep_pdf_test_{}", rand::random::<u64>()));
    std::fs::create_dir_all(&path).unwrap();
    path
}

/// PDF を組み立てる。相互参照表の位置がずれると pdfium が開かないので、
/// 位置は書き出しながら数える。
pub(crate) struct Pdf {
    objects: Vec<String>,
}

impl Pdf {
    pub(crate) fn new() -> Self {
        Pdf {
            objects: Vec::new(),
        }
    }

    /// 追加した物体の番号を返す。
    pub(crate) fn add(&mut self, body: &str) -> usize {
        self.objects.push(body.to_string());
        self.objects.len()
    }

    /// 番号を先に取っておき、あとから中身を入れる。相互に指し合う物体のため。
    pub(crate) fn reserve(&mut self) -> usize {
        self.add("<<>>")
    }

    pub(crate) fn set(&mut self, number: usize, body: &str) {
        self.objects[number - 1] = body.to_string();
    }

    pub(crate) fn stream(&mut self, content: &str) -> usize {
        let body = format!(
            "<< /Length {} >>\nstream\n{}\nendstream",
            content.len(),
            content
        );
        self.add(&body)
    }

    pub(crate) fn finish(&self, root: usize) -> Vec<u8> {
        let mut out = String::from("%PDF-1.7\n");
        let mut offsets = Vec::with_capacity(self.objects.len());
        for (index, body) in self.objects.iter().enumerate() {
            offsets.push(out.len());
            out.push_str(&format!("{} 0 obj\n{}\nendobj\n", index + 1, body));
        }
        let xref = out.len();
        out.push_str(&format!("xref\n0 {}\n", self.objects.len() + 1));
        out.push_str("0000000000 65535 f \n");
        for offset in &offsets {
            out.push_str(&format!("{:010} 00000 n \n", offset));
        }
        out.push_str(&format!(
            "trailer\n<< /Size {} /Root {} 0 R >>\nstartxref\n{}\n%%EOF\n",
            self.objects.len() + 1,
            root,
            xref
        ));
        out.into_bytes()
    }
}

pub(crate) struct TestLine {
    pub(crate) text: &'static str,
    pub(crate) y: f32,
    pub(crate) mcid: usize,
}

pub(crate) fn page_one_lines() -> Vec<TestLine> {
    vec![
        // 右端まで届く行。次の行と同じ段落。
        TestLine {
            text: "Alpha beta gamma delta epsilon zetaX",
            y: 170.0,
            mcid: 0,
        },
        TestLine {
            text: "etatheta",
            y: 156.0,
            mcid: 1,
        },
        // 一行だけの段落。行送りが空く。
        TestLine {
            text: "SecondPara.",
            y: 128.0,
            mcid: 2,
        },
        // 右端まで届いたままページが終わる。次のページへ続く。
        TestLine {
            text: "Third paragraph reaches the far edge",
            y: 114.0,
            mcid: 3,
        },
    ]
}

pub(crate) fn page_two_lines() -> Vec<TestLine> {
    vec![
        TestLine {
            text: "and continues here.",
            y: 170.0,
            mcid: 0,
        },
        TestLine {
            text: "Final line.",
            y: 156.0,
            mcid: 1,
        },
    ]
}

/// タグ付きの紙面の下に置くページ番号。
///
/// `/Artifact` は「作者が書いたのではなく、組版が置いたもの」という印である。
/// 構造ツリーには現れず、marked content の番号も持たない。Word も
/// Google ドキュメントも、柱とページ番号をこの形で書き出す。
pub(crate) const FOOTER: &str = "- 1 -";

fn content_stream(lines: &[TestLine], marked: bool) -> String {
    let mut out = String::new();
    for line in lines {
        if marked {
            out.push_str(&format!("/P <</MCID {}>> BDC\n", line.mcid));
        }
        out.push_str(&format!(
            "BT /F1 12 Tf {} {} Td ({}) Tj ET\n",
            LEFT, line.y, line.text
        ));
        if marked {
            out.push_str("EMC\n");
        }
    }
    if marked {
        // 紙面のいちばん下、本文よりずっと右から始まる。**本文の最後の行だと
        // 読み違えると、ページをまたぐ段落が繋がらなくなる。**
        out.push_str("/Artifact <</Type /Pagination /Subtype /Footer>> BDC\n");
        out.push_str(&format!("BT /F1 12 Tf 140 30 Td ({FOOTER}) Tj ET\n"));
        out.push_str("EMC\n");
    }
    out
}

/// 試験用の PDF を書き出す。`tagged` が偽なら構造ツリーを付けない。
pub(crate) fn write_pdf(dir: &std::path::Path, tagged: bool) -> PathBuf {
    let mut pdf = Pdf::new();
    let catalog = pdf.reserve();
    let pages = pdf.reserve();
    let page_one = pdf.reserve();
    let page_two = pdf.reserve();
    let font = pdf.add("<< /Type /Font /Subtype /Type1 /BaseFont /Courier >>");
    let content_one = pdf.stream(&content_stream(&page_one_lines(), tagged));
    let content_two = pdf.stream(&content_stream(&page_two_lines(), tagged));

    let resources = format!("<< /Font << /F1 {font} 0 R >> >>");
    let page_extra = |index: usize| {
        if tagged {
            format!(" /StructParents {index}")
        } else {
            String::new()
        }
    };
    pdf.set(
        page_one,
        &format!(
            "<< /Type /Page /Parent {pages} 0 R /MediaBox [0 0 300 200] /Resources {resources} /Contents {content_one} 0 R{} >>",
            page_extra(0)
        ),
    );
    pdf.set(
        page_two,
        &format!(
            "<< /Type /Page /Parent {pages} 0 R /MediaBox [0 0 300 200] /Resources {resources} /Contents {content_two} 0 R{} >>",
            page_extra(1)
        ),
    );
    pdf.set(
        pages,
        &format!("<< /Type /Pages /Kids [{page_one} 0 R {page_two} 0 R] /Count 2 >>"),
    );

    if !tagged {
        pdf.set(catalog, &format!("<< /Type /Catalog /Pages {pages} 0 R >>"));
        return write(dir, "plain.pdf", pdf.finish(catalog));
    }

    let tree = pdf.reserve();
    let document = pdf.reserve();
    // 段落。ページ1に三つ、ページ2に二つ。三つ目はページをまたぐので、
    // 断片がページごとに分かれる（Word が実際にこの形で書き出す）。
    let wrapped = pdf.add(&format!(
        "<< /Type /StructElem /S /P /P {document} 0 R /Pg {page_one} 0 R /K [0 1] >>"
    ));
    let single = pdf.add(&format!(
        "<< /Type /StructElem /S /P /P {document} 0 R /Pg {page_one} 0 R /K [2] >>"
    ));
    let spanning = pdf.add(&format!(
        "<< /Type /StructElem /S /P /P {document} 0 R /Pg {page_one} 0 R /K [3] >>"
    ));
    let continued = pdf.add(&format!(
        "<< /Type /StructElem /S /P /P {document} 0 R /Pg {page_two} 0 R /K [0] >>"
    ));
    let last = pdf.add(&format!(
        "<< /Type /StructElem /S /P /P {document} 0 R /Pg {page_two} 0 R /K [1] >>"
    ));
    let parent_tree = pdf.add(&format!(
        "<< /Nums [0 [{wrapped} 0 R {wrapped} 0 R {single} 0 R {spanning} 0 R] 1 [{continued} 0 R {last} 0 R]] >>"
    ));
    pdf.set(
        document,
        &format!(
            "<< /Type /StructElem /S /Document /P {tree} 0 R /K [{wrapped} 0 R {single} 0 R {spanning} 0 R {continued} 0 R {last} 0 R] >>"
        ),
    );
    pdf.set(
        tree,
        &format!("<< /Type /StructTreeRoot /K [{document} 0 R] /ParentTree {parent_tree} 0 R >>"),
    );
    pdf.set(
        catalog,
        &format!(
            "<< /Type /Catalog /Pages {pages} 0 R /StructTreeRoot {tree} 0 R /MarkInfo << /Marked true >> >>"
        ),
    );
    write(dir, "tagged.pdf", pdf.finish(catalog))
}

pub(crate) fn write(dir: &std::path::Path, name: &str, bytes: Vec<u8>) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}
