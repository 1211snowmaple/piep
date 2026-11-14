use super::*;
use crate::library::{
    Capture, CapturedAttachment, CapturedMedia, EntityKind, Fingerprint, ReaderBinding, SourceBody,
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
        raw: serde_json::to_vec(&json!({"from": "source"})).unwrap().into(),
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
    let saved = library.capture(capture("pixiv", body, b"picture", 3)).unwrap();
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
        .render_document(saved.entity_id, ResourceScope::Snapshot(saved.snapshot_id), "pixiv")
        .unwrap();
    let html = rendered.pages.join("");
    let file = Fingerprint::of(b"picture").to_string();
    assert_eq!(html.matches(&format!(r#"data-local-path="{file}""#)).count(), 2, "{html}");
    // 取得できなかった画像は、旧棚と同じ印のまま。
    assert!(html.contains(r#"data-image-reference="[uploadedimage:404]" data-image-missing="true""#));
    // 保存名（saved-name.png）や旧棚のパスは出さない。
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
        .render_document(saved.entity_id, ResourceScope::Snapshot(saved.snapshot_id), "fanbox")
        .unwrap()
        .pages
        .join("");
    let image = Fingerprint::of(b"art").to_string();
    let file = Fingerprint::of(b"file").to_string();
    assert!(html.contains(&format!(r#"data-local-path="{image}""#)), "{html}");
    // 添付は印で照合する。置き場の 16 進が「f1」を含んでも画像には当たらない。
    assert!(html.contains(&format!(r#"data-local-path="{file}/piep-file-z0z""#)), "{html}");
    assert!(!html.contains(&format!(r#"data-local-path="{image}/"#)), "{html}");
}

#[test]
fn image_and_file_posts_bind_their_lists_by_position() {
    let dir = temp("lists");
    let library = Library::create(&dir.0).unwrap();
    // 画像だけ・添付だけの投稿は配列の何番目かで指す。ID の無い項目や、別の保存物を
    // 指しそうな置き場の手がかりがあっても、新しい棚の対応どおりに結ぶ。
    let body = SourceBody::FanboxBody(json!({
        "text": "本文",
        "images": [{"localPath": "./elsewhere.png"}, {"id": "i2"}],
        "files": [{"id": "f1", "name": "notes", "extension": "txt", "size": 4}, {"id": "f2", "name": "lost", "extension": "psd"}]
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
                bind(BodyAssetReference::ArticleImageAt { index: 0 }, picture.clone()),
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
        .render_document(saved.entity_id, ResourceScope::Snapshot(saved.snapshot_id), "fanbox")
        .unwrap()
        .pages
        .join("");
    let image = Fingerprint::of(b"art").to_string();
    let file = Fingerprint::of(b"file").to_string();
    assert_eq!(html.matches(&format!(r#"data-local-path="{image}""#)).count(), 1, "{html}");
    assert_eq!(html.matches("data-image-missing").count(), 1, "{html}");
    assert!(html.contains(&format!(r#"data-local-path="{file}/piep-file-z"#)), "{html}");
    assert!(html.contains("lost.psd(見つかりません)"), "{html}");
}

#[test]
fn a_work_whose_source_had_no_body_reads_as_an_empty_body() {
    let dir = temp("nobody");
    let library = Library::create(&dir.0).unwrap();
    let mut without = capture("fanbox", SourceBody::PlainText(String::new()), b"art", 0);
    without.body = None;
    let saved = library.capture(without).unwrap();
    let rendered = library
        .render_document(saved.entity_id, ResourceScope::Snapshot(saved.snapshot_id), "fanbox")
        .unwrap();
    assert!(rendered.pages.is_empty());
}

#[test]
fn versions_media_text_and_the_source_response_of_a_work() {
    let dir = temp("versions");
    let library = Library::create(&dir.0).unwrap();
    let first = library
        .capture(capture("local", SourceBody::PlainText("一行目".into()), b"a", 3))
        .unwrap();
    let second = library
        .capture(capture("local", SourceBody::PlainText("二行目\n三行目".into()), b"b", 7))
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
            ("media", Some("saved-name.png"), Some(Fingerprint::of(b"b").to_string())),
            ("file", Some("notes.txt"), Some(Fingerprint::of(b"file").to_string())),
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
        library.work_original(work, second.snapshot_id).unwrap().as_deref(),
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
            if !only.is_empty() && !only.contains(&format!("{}:{}", card.provider, card.external_key)) {
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
                        _ => failures.push(format!("{} {}: {error}", card.provider, card.external_key)),
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
            let html = library
                .render_document(card.id, scope, &card.provider)
                .unwrap()
                .pages
                .join("");
            let took = timer.elapsed();
            if took > slowest.0 {
                slowest = (took, Some(format!("{} {}", card.provider, card.external_key)));
            }
            let work_resolved = html.matches("data-local-path=").count();
            let work_missing =
                html.matches("data-image-missing").count() + html.matches("missing-file-placeholder").count();
            resolved += work_resolved;
            missing += work_missing;
            if !only.is_empty() {
                println!(
                    "{}:{} bindings={:?}",
                    card.provider,
                    card.external_key,
                    document.bindings.iter().map(|b| (&b.reference, b.resource.is_some())).collect::<Vec<_>>()
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
    for line in suspects.iter().take(40) {
        println!("suspect {line}");
    }
    for line in failures.iter().take(20) {
        println!("failure {line}");
    }
}
