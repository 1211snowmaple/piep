use super::*;
use crate::library::{
    BodyAssetReference, Capture, CapturedAttachment, CapturedMedia, EntityKind, Fingerprint,
    ReaderBinding, SourceBody,
};
use serde_json::json;
use std::path::PathBuf;

struct Temp(PathBuf);
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn temp(name: &str) -> Temp {
    Temp(std::env::temp_dir().join(format!("piep-view-{name}-{}", LibraryId::now_v7())))
}

fn capture(provider: &str, body: SourceBody, image: &[u8], text_length: i64) -> Capture {
    Capture {
        request_id: LibraryId::now_v7(),
        acquired_at: None,
        provider: provider.into(),
        external_key: "one".into(),
        kind: EntityKind::Work,
        route: "test".into(),
        raw: serde_json::to_vec(&json!({"from": "source"}))
            .unwrap()
            .into(),
        metadata: BTreeMap::from([
            ("title".into(), json!("題")),
            ("text_length".into(), json!(text_length)),
        ]),
        field_states: BTreeMap::from([
            ("media".into(), "known".into()),
            ("attachments".into(), "known".into()),
        ]),
        body: Some(body),
        media: vec![CapturedMedia {
            slot: "picture".into(),
            role: "illustration".into(),
            source_key: Some("picture".into()),
            source_url: None,
            quality: "original".into(),
            original_name: "saved-name.png".into(),
            mime_type: "image/png".into(),
            bytes: Some(image.to_vec().into()),
        }],
        attachments: vec![CapturedAttachment {
            slot: "attachment".into(),
            original_name: "notes.txt".into(),
            mime_type: Some("text/plain".into()),
            bytes: b"file".to_vec().into(),
        }],
    }
}

fn bind(reference: BodyAssetReference, resource: ResourceReference) -> ReaderBinding {
    ReaderBinding {
        reference,
        resource: Some(resource),
    }
}

#[test]
fn pixiv_images_are_resolved_through_the_new_shelf_bindings() {
    let dir = temp("pixiv");
    let library = Library::create(&dir.0).unwrap();
    let body = SourceBody::PixivText(
        "一[uploadedimage:123]二[pixivimage:9-2]三[uploadedimage:404]".into(),
    );
    let saved = library
        .capture(capture("pixiv", body, b"picture", 3))
        .unwrap();
    let picture = ResourceReference::Media {
        slot: "picture".into(),
    };
    library
        .publish_reader_bindings(
            saved.entity_id,
            saved.snapshot_id,
            vec![
                bind(
                    BodyAssetReference::PixivUploaded {
                        image_id: "123".into(),
                    },
                    picture.clone(),
                ),
                bind(
                    BodyAssetReference::PixivIllustration {
                        illustration_id: "9".into(),
                        page: 2,
                    },
                    picture,
                ),
                ReaderBinding {
                    reference: BodyAssetReference::PixivUploaded {
                        image_id: "404".into(),
                    },
                    resource: None,
                },
            ],
        )
        .unwrap();
    let rendered = library
        .render_document(
            saved.entity_id,
            ResourceScope::Snapshot(saved.snapshot_id),
            "pixiv",
        )
        .unwrap();
    let html = rendered.pages.join("");
    let file = Fingerprint::of(b"picture").to_string();
    assert_eq!(
        html.matches(&format!(r#"data-local-path="{file}""#))
            .count(),
        2,
        "{html}"
    );
    // 取得できなかった画像は、取れなかった印のまま。
    assert!(
        html.contains(r#"data-image-reference="[uploadedimage:404]" data-image-missing="true""#)
    );
    // 保存名（saved-name.png）や保存フォルダーのパスは出さない。
    assert!(!html.contains("saved-name"));
}

#[test]
fn article_images_and_files_point_at_their_files() {
    let dir = temp("article");
    let library = Library::create(&dir.0).unwrap();
    let body = SourceBody::FanboxBody(json!({
        "blocks": [
            {"type": "p", "text": "はじめ"},
            {"type": "image", "imageId": "img"},
            {"type": "file", "fileId": "f1"}
        ],
        "fileMap": {"f1": {"name": "notes", "extension": "txt", "size": 4}}
    }));
    let saved = library.capture(capture("fanbox", body, b"art", 3)).unwrap();
    library
        .publish_reader_bindings(
            saved.entity_id,
            saved.snapshot_id,
            vec![
                bind(
                    BodyAssetReference::ArticleImage { key: "img".into() },
                    ResourceReference::Media {
                        slot: "picture".into(),
                    },
                ),
                bind(
                    BodyAssetReference::ArticleFile { key: "f1".into() },
                    ResourceReference::File {
                        slot: "attachment".into(),
                    },
                ),
            ],
        )
        .unwrap();
    let html = library
        .render_document(
            saved.entity_id,
            ResourceScope::Snapshot(saved.snapshot_id),
            "fanbox",
        )
        .unwrap()
        .pages
        .join("");
    let image = Fingerprint::of(b"art").to_string();
    let file = Fingerprint::of(b"file").to_string();
    assert!(
        html.contains(&format!(r#"data-local-path="{image}""#)),
        "{html}"
    );
    // 添付は印で照合する。置き場の 16 進が「f1」を含んでも画像には当たらない。
    assert!(
        html.contains(&format!(r#"data-local-path="{file}/piep-file-z0z""#)),
        "{html}"
    );
    assert!(
        !html.contains(&format!(r#"data-local-path="{image}/"#)),
        "{html}"
    );
}

#[test]
fn image_and_file_posts_bind_their_lists_by_position() {
    let dir = temp("lists");
    let library = Library::create(&dir.0).unwrap();
    // 画像だけ・添付だけの投稿は配列の何番目かで指す。ID の無い項目や、別の保存物を
    // 指しそうな置き場の手がかりがあっても、棚の対応どおりに結ぶ。
    let body = SourceBody::FanboxBody(json!({
        "text": "本文",
        "images": [{"localPath": "./elsewhere.png"}, {"id": "i2"}],
        // 本文を取り込まない種類の添付（テキスト・PDF は下の試験）。
        "files": [{"id": "f1", "name": "notes", "extension": "zip", "size": 4}, {"id": "f2", "name": "lost", "extension": "psd"}]
    }));
    let saved = library.capture(capture("fanbox", body, b"art", 3)).unwrap();
    let picture = ResourceReference::Media {
        slot: "picture".into(),
    };
    library
        .publish_reader_bindings(
            saved.entity_id,
            saved.snapshot_id,
            vec![
                bind(
                    BodyAssetReference::ArticleImageAt { index: 0 },
                    picture.clone(),
                ),
                ReaderBinding {
                    reference: BodyAssetReference::ArticleImageAt { index: 1 },
                    resource: None,
                },
                bind(
                    BodyAssetReference::ArticleFileAt { index: 0 },
                    ResourceReference::File {
                        slot: "attachment".into(),
                    },
                ),
                ReaderBinding {
                    reference: BodyAssetReference::ArticleFileAt { index: 1 },
                    resource: None,
                },
            ],
        )
        .unwrap();
    let html = library
        .render_document(
            saved.entity_id,
            ResourceScope::Snapshot(saved.snapshot_id),
            "fanbox",
        )
        .unwrap()
        .pages
        .join("");
    let image = Fingerprint::of(b"art").to_string();
    let file = Fingerprint::of(b"file").to_string();
    assert_eq!(
        html.matches(&format!(r#"data-local-path="{image}""#))
            .count(),
        1,
        "{html}"
    );
    assert_eq!(html.matches("data-image-missing").count(), 1, "{html}");
    // 照合の印は画像の説明に残さない。
    assert!(!html.contains("piep-image-z"), "{html}");
    assert!(html.contains(r#"alt="image_""#), "{html}");
    assert!(
        html.contains(&format!(r#"data-local-path="{file}/piep-file-z"#)),
        "{html}"
    );
    assert!(html.contains("lost.psd(見つかりません)"), "{html}");
}

/// 添付のテキスト・PDF の本文は、添付の位置へ差し込む。読む画面と検索の文書は
/// 同じ差し込みを通すので、読める本文は検索にも入る。
#[test]
fn attachment_bodies_are_inserted_for_reading_and_search_alike() {
    let dir = temp("attachment-body");
    let library = Library::create(&dir.0).unwrap();
    let body = SourceBody::FanboxBody(json!({
        "text": "挨拶",
        "files": [{"id": "Abc123Def456", "name": "notes", "extension": "txt", "size": 4}]
    }));
    let mut captured = capture("fanbox", body, b"art", 2);
    captured.attachments[0].bytes = "取り込む本文

二段落目"
        .as_bytes()
        .to_vec()
        .into();
    let saved = library.capture(captured).unwrap();
    library
        .publish_reader_bindings(
            saved.entity_id,
            saved.snapshot_id,
            vec![bind(
                BodyAssetReference::ArticleFileAt { index: 0 },
                ResourceReference::File {
                    slot: "attachment".into(),
                },
            )],
        )
        .unwrap();
    let rendered = library
        .render_document(
            saved.entity_id,
            ResourceScope::Snapshot(saved.snapshot_id),
            "fanbox",
        )
        .unwrap();
    let html = rendered.pages.join("");
    assert!(html.contains("取り込む本文"), "{html}");
    assert!(html.contains("二段落目"), "{html}");
    assert!(html.contains("attachment-notice"), "{html}");
    // 差し込んだあとの `blocks` も、添付の箱は棚のファイルを指す。
    let file = Fingerprint::of(
        "取り込む本文

二段落目"
            .as_bytes(),
    )
    .to_string();
    assert!(
        html.contains(&format!(r#"data-local-path="{file}/piep-file-z"#)),
        "{html}"
    );
    // テキストには紙面が無いので「原本を見る」は出さない。
    assert!(!html.contains("attachment-original"), "{html}");
    // 残り時間の字数も、検索の文書の本文も、取り込んだ本文を含む。
    let document = library.search_document(saved.entity_id).unwrap().unwrap();
    assert!(document.body.contains("取り込む本文"), "{}", document.body);
    assert_eq!(rendered.text_chars, document.body.chars().count());
    // 携帯は添付を読み解けないので、渡す文書には差し込み済みの本文と字数が入る。携帯が
    // その文書から組んだページは、デスクトップの読む画面と同じになる。
    let bundle = library
        .reading_bundle(saved.entity_id, ResourceScope::Snapshot(saved.snapshot_id))
        .unwrap();
    let parsed = piep_library_protocol::ReaderBundle::parse(&bundle.encode().unwrap()).unwrap();
    let view = ReaderView::from_value(parsed.document).unwrap();
    let reading = view.reading.clone().unwrap();
    assert_eq!(reading.provider, "fanbox");
    assert_eq!(reading.text_chars, rendered.text_chars);
    let (pages, starts) = piep_reader_render::document::render_pages(&view, &reading.provider);
    assert_eq!(pages, rendered.pages);
    assert_eq!(starts, rendered.source_page_starts);
}

/// PDF の添付は「原本を見る」から紙面を開ける。ボタンは一時に書き出した場所ではなく
/// 棚のファイルの識別子を指し、開くのはその作品の PDF の添付だけ。
#[test]
fn a_pdf_attachment_opens_its_original_by_its_file() {
    let dir = temp("attachment-pdf");
    let pdf_dir = crate::pdf::fixture::temp_dir();
    let pdf = std::fs::read(crate::pdf::fixture::write_pdf(&pdf_dir, true)).unwrap();
    let _ = std::fs::remove_dir_all(&pdf_dir);
    let library = Library::create(&dir.0).unwrap();
    let body = SourceBody::FanboxBody(json!({
        "text": "挨拶",
        "files": [{"id": "Abc123Def456", "name": "sample", "extension": "pdf", "size": 4}]
    }));
    let mut captured = capture("fanbox", body, b"art", 2);
    captured.attachments[0].original_name = "sample.pdf".into();
    captured.attachments[0].mime_type = Some("application/pdf".into());
    captured.attachments[0].bytes = pdf.clone().into();
    let saved = library.capture(captured).unwrap();
    library
        .publish_reader_bindings(
            saved.entity_id,
            saved.snapshot_id,
            vec![bind(
                BodyAssetReference::ArticleFileAt { index: 0 },
                ResourceReference::File {
                    slot: "attachment".into(),
                },
            )],
        )
        .unwrap();
    let html = library
        .render_document(
            saved.entity_id,
            ResourceScope::Snapshot(saved.snapshot_id),
            "fanbox",
        )
        .unwrap()
        .pages
        .join("");
    let file = Fingerprint::of(&pdf);
    assert!(
        html.contains(&format!(
            r#"class="attachment-original" data-local-path="{file}" data-file-name="sample.pdf""#
        )),
        "{html}"
    );
    assert!(
        !html.contains("piep-search-"),
        "一時の場所を出さない: {html}"
    );

    let original = library
        .work_pdf_attachment(saved.entity_id, file)
        .unwrap()
        .expect("作品の PDF の添付は開ける");
    let mut bytes = Vec::new();
    library.objects.export(&original, &mut bytes).unwrap();
    let page = crate::pdf::render_page_bytes(&bytes, 1, 400).unwrap();
    assert_eq!(page.page_count, 2);
    // 同じ作品のファイルでも PDF の添付でないもの、棚に無いものは開かない。
    assert!(library
        .work_pdf_attachment(saved.entity_id, Fingerprint::of(b"art"))
        .unwrap()
        .is_none());
    assert!(library
        .work_pdf_attachment(LibraryId::now_v7(), file)
        .unwrap()
        .is_none());
}

#[test]
fn the_reader_gets_pages_outline_search_and_length_like_the_old_shelf() {
    use crate::database::queries::{reader_outline, search_reader_pages};
    let dir = temp("reader");
    let library = Library::create(&dir.0).unwrap();
    let body = SourceBody::PixivText("[chapter:一章]雨の朝[newpage][chapter:二章]カサの話".into());
    let saved = library.capture(capture("pixiv", body, b"art", 0)).unwrap();
    let scope = ResourceScope::Snapshot(saved.snapshot_id);
    let rendered = library
        .render_document(saved.entity_id, scope, "pixiv")
        .unwrap();
    // pixiv の [newpage] は原稿のページ。転送のページとの対応も返す。
    assert_eq!(rendered.pages.len(), 2);
    assert_eq!(rendered.source_page_starts, vec![0, 1]);
    // 字数は HTML ではなく本文の平文で数える。
    assert_eq!(
        rendered.text_chars,
        library
            .work_text(saved.entity_id, scope)
            .unwrap()
            .chars()
            .count()
    );
    let outline = reader_outline(&rendered.pages);
    let titles: Vec<_> = outline
        .iter()
        .map(|entry| (entry.page, entry.title.as_str()))
        .collect();
    assert_eq!(titles, vec![(1, "一章"), (2, "二章")]);
    // 本文内検索はカタカナとひらがなを同じに見る。
    let hits = search_reader_pages(&rendered.pages, "かさ", 50);
    assert_eq!(
        hits.iter()
            .map(|hit| (hit.page, hit.count))
            .collect::<Vec<_>>(),
        vec![(2, 1)]
    );
}

#[test]
fn a_work_whose_source_had_no_body_reads_as_an_empty_body() {
    let dir = temp("nobody");
    let library = Library::create(&dir.0).unwrap();
    let mut without = capture("fanbox", SourceBody::PlainText(String::new()), b"art", 0);
    without.body = None;
    let saved = library.capture(without).unwrap();
    let rendered = library
        .render_document(
            saved.entity_id,
            ResourceScope::Snapshot(saved.snapshot_id),
            "fanbox",
        )
        .unwrap();
    assert!(rendered.pages.is_empty());
}

#[test]
fn versions_media_text_and_the_source_response_of_a_work() {
    let dir = temp("versions");
    let library = Library::create(&dir.0).unwrap();
    let first = library
        .capture(capture(
            "local",
            SourceBody::PlainText("一行目".into()),
            b"a",
            3,
        ))
        .unwrap();
    let second = library
        .capture(capture(
            "local",
            SourceBody::PlainText("二行目\n三行目".into()),
            b"b",
            7,
        ))
        .unwrap();
    assert_eq!(first.entity_id, second.entity_id);
    let work = first.entity_id;

    let versions = library.work_versions(work).unwrap();
    assert_eq!(
        versions
            .iter()
            .map(|v| (v.version, v.snapshot, v.text_length))
            .collect::<Vec<_>>(),
        vec![
            (1, first.snapshot_id, Some(3)),
            (2, second.snapshot_id, Some(7))
        ]
    );

    let media = library
        .work_media(work, ResourceScope::Snapshot(second.snapshot_id))
        .unwrap();
    assert_eq!(
        media
            .iter()
            .map(|m| (m.kind, m.name.as_deref(), m.file.clone()))
            .collect::<Vec<_>>(),
        vec![
            (
                "media",
                Some("saved-name.png"),
                Some(Fingerprint::of(b"b").to_string())
            ),
            (
                "file",
                Some("notes.txt"),
                Some(Fingerprint::of(b"file").to_string())
            ),
        ]
    );

    assert_eq!(
        library
            .work_text(work, ResourceScope::Snapshot(first.snapshot_id))
            .unwrap(),
        "一行目"
    );
    let html = library
        .render_document(work, ResourceScope::Snapshot(second.snapshot_id), "local")
        .unwrap()
        .pages
        .join("");
    assert!(html.contains("二行目<br />\n三行目"), "{html}");
    assert_eq!(
        library
            .work_original(work, second.snapshot_id)
            .unwrap()
            .as_deref(),
        Some(r#"{"from":"source"}"#)
    );
}

/// 実際の棚で、全作品の本文を組んで画像・添付の結び付きを数える。
/// `PIEP_LIBRARY_VIEW_PROBE=<棚>` で `cargo test --lib library_view::tests::probe -- --ignored --nocapture`。
#[test]
#[ignore]
fn probe_a_real_shelf() {
    let Some(dir) = std::env::var_os("PIEP_LIBRARY_VIEW_PROBE") else {
        return;
    };
    let library = Library::open(std::path::Path::new(&dir)).unwrap();
    let mut query = crate::library::WorkListQuery {
        limit: 500,
        count: false,
        ..Default::default()
    };
    let (mut works, mut bound, mut unbound, mut resolved, mut missing) = (0, 0, 0, 0, 0);
    let mut empty = 0;
    // 読む画面: 目次の見出し・原稿のページの対応の崩れ・字数のずれ・本文内検索の一番遅い作品。
    let (mut headings, mut bad_starts, mut length_drift) = (0, Vec::new(), Vec::new());
    let mut slowest_search = (std::time::Duration::ZERO, None);
    let mut suspects = Vec::new();
    let mut failures = Vec::new();
    let mut slowest = (std::time::Duration::ZERO, None);
    let started = std::time::Instant::now();
    // 調べる作品を絞る（`取得元:キー` をカンマで）。絞ったときは見つからない印の中身も出す。
    let only: Vec<String> = std::env::var("PIEP_LIBRARY_VIEW_PROBE_KEYS")
        .map(|keys| keys.split(',').map(str::to_owned).collect())
        .unwrap_or_default();
    loop {
        let page = library.list_works(&query).unwrap();
        for card in &page.items {
            if !only.is_empty()
                && !only.contains(&format!("{}:{}", card.provider, card.external_key))
            {
                continue;
            }
            works += 1;
            let scope = ResourceScope::Snapshot(card.snapshot_id);
            let document = match library.reader_document(card.id, scope) {
                Ok(document) => document,
                Err(error) => {
                    // 本文の無い作品は空の本文として組めればよい。
                    match library.render_document(card.id, scope, &card.provider) {
                        Ok(rendered) if rendered.pages.is_empty() => empty += 1,
                        _ => failures
                            .push(format!("{} {}: {error}", card.provider, card.external_key)),
                    }
                    continue;
                }
            };
            let work_unbound = document
                .bindings
                .iter()
                .filter(|b| b.resource.as_ref().is_none_or(|r| r.file.is_none()))
                .count();
            bound += document.bindings.len() - work_unbound;
            unbound += work_unbound;
            let timer = std::time::Instant::now();
            let rendered = library
                .render_document(card.id, scope, &card.provider)
                .unwrap();
            let took = timer.elapsed();
            let html = rendered.pages.join("");
            headings += crate::database::queries::reader_outline(&rendered.pages).len();
            let starts = &rendered.source_page_starts;
            let pixiv = card.provider.eq_ignore_ascii_case("pixiv");
            if pixiv
                && (starts.first().is_some_and(|first| *first != 0)
                    || starts.windows(2).any(|pair| pair[0] >= pair[1])
                    || starts
                        .last()
                        .is_some_and(|last| *last >= rendered.pages.len()))
            {
                bad_starts.push(format!("{} {}", card.provider, card.external_key));
            }
            // 移行した字数と、読む画面の残り時間に使う字数。
            if rendered.text_chars as u64 != card.text_length {
                length_drift.push(format!(
                    "{} {} card={} reader={}",
                    card.provider, card.external_key, card.text_length, rendered.text_chars
                ));
            }
            let timer = std::time::Instant::now();
            let _ = crate::database::queries::search_reader_pages(&rendered.pages, "の", 50);
            let searched = timer.elapsed();
            if searched > slowest_search.0 {
                slowest_search = (
                    searched,
                    Some(format!("{} {}", card.provider, card.external_key)),
                );
            }
            if took > slowest.0 {
                slowest = (
                    took,
                    Some(format!("{} {}", card.provider, card.external_key)),
                );
            }
            let work_resolved = html.matches("data-local-path=").count();
            let work_missing = html.matches("data-image-missing").count()
                + html.matches("missing-file-placeholder").count();
            resolved += work_resolved;
            missing += work_missing;
            if !only.is_empty() {
                println!(
                    "{}:{} bindings={:?}",
                    card.provider,
                    card.external_key,
                    document
                        .bindings
                        .iter()
                        .map(|b| (&b.reference, b.resource.is_some()))
                        .collect::<Vec<_>>()
                );
                for (index, _) in html.match_indices("data-image-missing") {
                    let from = html[..index].rfind('<').unwrap_or(0);
                    println!("  missing {}", &html[from..index]);
                }
            }
            if work_missing > 0 && work_unbound == 0 {
                suspects.push(format!(
                    "{} {} missing={work_missing} bindings={}",
                    card.provider,
                    card.external_key,
                    document.bindings.len()
                ));
            }
        }
        match page.next {
            Some(next) => query.after = Some(next),
            None => break,
        }
    }
    println!(
        "works={works} empty={empty} bound={bound} unbound={unbound} resolved={resolved} missing={missing} suspects={} failures={} elapsed={:?} slowest={slowest:?}",
        suspects.len(),
        failures.len(),
        started.elapsed()
    );
    println!(
        "reader: headings={headings} bad_starts={} length_drift={} slowest_search={slowest_search:?}",
        bad_starts.len(),
        length_drift.len()
    );
    for line in bad_starts.iter().take(20) {
        println!("bad_starts {line}");
    }
    for line in length_drift.iter().take(20) {
        println!("length_drift {line}");
    }
    for line in suspects.iter().take(40) {
        println!("suspect {line}");
    }
    for line in failures.iter().take(20) {
        println!("failure {line}");
    }
}

/// 編集の本文は編集画面と同じ関数で組む。段の間の空行の数・見出し・画像の説明・改ページが編集画面と
/// 同じ行になり、「何ページ目の何行目」で憶えた読書位置としおりが同じ行を指す。
/// 編集のあとに改稿を取り込むと、編集版は古い版の上に載ったまま。読む画面がそれを名乗れるよう、
/// 基準の版の番号を返す。
#[test]
fn an_edit_knows_the_version_it_was_made_on() {
    use crate::library::{EditBlock, EditRequest};
    let dir = temp("edit-base");
    let library = Library::create(&dir.0).unwrap();
    let first = library
        .capture(capture(
            "pixiv",
            SourceBody::PixivText("一".into()),
            b"art",
            1,
        ))
        .unwrap();
    let content = EditContent {
        format: "piep-edit-v1".into(),
        title: None,
        blocks: vec![EditBlock {
            id: LibraryId::now_v7(),
            ordinal: 0,
            kind: "paragraph".into(),
            text: Some("直した一".into()),
            attrs: json!(null),
            media_slot: None,
        }],
        media: Vec::new(),
        provenance: None,
    };
    let edit = library
        .save_edit(EditRequest {
            request_id: LibraryId::now_v7(),
            work: first.entity_id,
            user: library.id,
            base: first.snapshot_id,
            parent: None,
            expected: 0,
            content: &content,
            activate: true,
        })
        .unwrap();
    assert_eq!(library.edit_base_version(first.entity_id, edit).unwrap(), 1);
    library
        .capture(capture(
            "pixiv",
            SourceBody::PixivText("二".into()),
            b"art",
            1,
        ))
        .unwrap();
    let card = library.work_card(first.entity_id).unwrap();
    assert_eq!(card.version_number, 2);
    assert_eq!(card.edit_id, Some(edit));
    assert_eq!(library.edit_base_version(first.entity_id, edit).unwrap(), 1);
}

#[test]
fn an_edit_is_laid_out_line_for_line_like_the_old_shelf() {
    use crate::database::models::WorkBlock;
    use crate::library::{EditBlock, EditMedia};
    let picture = Fingerprint::of(b"picture");
    let blocks = [
        ("paragraph", Some("一\n二"), json!(null), None),
        ("heading", Some("見出し"), json!({"gap": 2}), None),
        ("image", Some("説明"), json!(null), Some("legacy-asset:7")),
        ("page_break", None, json!(null), None),
        ("paragraph", Some("三"), json!({"gap": 0}), None),
        (
            "image",
            None,
            json!({"sourceRef": "[uploadedimage:404]"}),
            Some("legacy-asset:8"),
        ),
    ];
    let content = EditContent {
        format: "piep-edit-v1".into(),
        title: None,
        blocks: blocks
            .iter()
            .enumerate()
            .map(|(index, (kind, text, attrs, slot))| EditBlock {
                id: LibraryId::now_v7(),
                ordinal: index as i64,
                kind: (*kind).into(),
                text: text.map(str::to_owned),
                attrs: attrs.clone(),
                media_slot: slot.map(str::to_owned),
            })
            .collect(),
        media: vec![EditMedia {
            slot: "legacy-asset:7".into(),
            file: picture,
            original_name: "saved.png".into(),
            mime_type: None,
        }],
        provenance: None,
    };
    let entity = LibraryId::now_v7();
    let scope = ResourceScope::Edit(LibraryId::now_v7());
    let document = ReaderDocument {
        format: piep_reader_render::document::READER_DOCUMENT_FORMAT,
        entity,
        scope,
        metadata: BTreeMap::new(),
        body: ReaderBody::Edit(content),
        resources: crate::library::ResourceManifest {
            entity,
            scope,
            base_snapshot: None,
            resources: Vec::new(),
        },
        base_resources: None,
        bindings: Vec::new(),
        bindings_hash: None,
    };
    let old_blocks: Vec<WorkBlock> = blocks
        .iter()
        .enumerate()
        .map(|(index, (kind, text, attrs, slot))| WorkBlock {
            id: index as i64,
            edit_revision_id: 1,
            order: index as i64,
            block_type: (*kind).into(),
            text: text.map(str::to_owned),
            asset_id: slot.map(|slot| if slot.ends_with(":7") { 7 } else { 8 }),
            attrs_json: (!attrs.is_null()).then(|| attrs.to_string()),
        })
        .collect();
    // 画像 8 のファイルが無い（残っていない画像は渡さない）。
    let old_assets = [AssetEntry {
        id: 7,
        download_id: 1,
        asset_type: "illustration".into(),
        filename: "saved.png".into(),
        local_path: picture.to_string(),
        original_url: None,
        mime_type: None,
        file_size_bytes: 0,
    }];
    assert_eq!(
        render_html(&document).unwrap(),
        piep_reader_render::blocks::blocks_to_html(&old_blocks, &old_assets)
    );
}
