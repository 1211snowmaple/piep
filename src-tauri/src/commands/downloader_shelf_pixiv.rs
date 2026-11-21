//! 制限画像を再受信した場合も、同じ本文に対応する保存済みの実画像を使う。
use super::*;
use crate::downloader::asset_downloader::DownloadTarget;
use crate::library::{image_identity, ResourceInfo, SourceBody};

type ProtectedAssets = HashMap<(String, String), (ResourceReference, String)>;

pub(super) fn protected_gate_assets(
    library: &Library,
    work: LibraryId,
    snapshot: LibraryId,
    previous: &Value,
    next: &Value,
    resources: &[ResourceInfo],
    targets: (&[DownloadTarget], &[DownloadTarget]),
) -> Result<ProtectedAssets, String> {
    let (old_targets, targets) = targets;
    let gates: Vec<_> = targets
        .iter()
        .filter(|t| crate::library::is_pixiv_placeholder_image(&t.url))
        .collect();
    if gates.is_empty() {
        return Ok(HashMap::new());
    }
    let text = |v: &Value| {
        v.get("text")
            .or_else(|| v.pointer("/detail/text"))
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    let old_text = text(previous);
    let new_text = text(next);
    let body = new_text.as_ref().map(|t| SourceBody::PixivText(t.clone()));
    let references = body
        .as_ref()
        .map(SourceBody::asset_references)
        .transpose()
        .map_err(|e| e.to_string())?
        .unwrap_or_default();
    let plan = if gates.iter().any(|t| t.sub_folder == "illustrations") {
        match library.reader_bindings(work, snapshot) {
            Ok(plan) => Some(plan),
            Err(crate::library::Error::ResourceUnavailable)
                if !resources
                    .iter()
                    .any(|r| r.role == "illustration" && r.file.is_some()) =>
            {
                None
            }
            Err(error) => {
                return Err(format!(
                    "保存済みの画像との対応を確認できないため、今回の保存を止めました: {error}"
                ))
            }
        }
    } else {
        None
    };
    let mut protected = HashMap::new();
    for target in gates {
        let held = if target.sub_folder == "cover" {
            resources.iter().find(|r| {
                r.role == "cover"
                    && r.file.is_some()
                    && r.source_identity.as_deref().is_none_or(|s| {
                        !s.strip_prefix("url:v1:pixiv:")
                            .is_some_and(crate::library::is_pixiv_placeholder_image)
                    })
            })
        } else {
            let reference = references.iter().find(|reference| {
                body.as_ref().is_some_and(|b| {
                    crate::library::source_bindings::source_matches(
                        b,
                        reference,
                        &target.filename,
                        Some(&target.url),
                    )
                })
            });
            let resource = reference.and_then(|reference| {
                plan.as_ref()?
                    .bindings
                    .iter()
                    .find(|b| &b.reference == reference)?
                    .resource
                    .as_ref()
            });
            resource.and_then(|reference| {
                resources
                    .iter()
                    .find(|r| &r.reference == reference && r.file.is_some())
            })
        };
        let Some(held) = held else {
            continue;
        };
        // 違う本文へ過去の画像を引き継いだことにはしない。原応答を残して旧版を守る。
        if old_text.is_none() || old_text != new_text {
            return Err("本文が変わり、画像も閲覧制限の案内になったため、保存済みの版を残して今回の保存を止めました".into());
        }
        let url = old_targets
            .iter()
            .find(|old| {
                old.sub_folder == target.sub_folder
                    && !crate::library::is_pixiv_placeholder_image(&old.url)
                    && held.source_identity.as_deref()
                        == Some(image_identity("pixiv", &old.url).as_str())
            })
            .ok_or("保存済みの画像の取得元を確認できないため、今回の保存を止めました")?;
        protected.insert(
            (
                target_role(target.sub_folder).into(),
                target.filename.clone(),
            ),
            (held.reference.clone(), url.url.clone()),
        );
    }
    Ok(protected)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{AcquisitionState, Fingerprint};
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    struct Temp(PathBuf);
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn unknown_origin_or_invalid_held_cover_stops_before_replacing_it_with_a_gate() {
        for case in ["unknown", "invalid"] {
            let temp = Temp(
                std::env::temp_dir().join(format!("piep-gate-refuse-{}", LibraryId::now_v7())),
            );
            let library = Library::create(&temp.0.join("shelf")).unwrap();
            let mut original = json!({"detail":{"title":"題"}, "text":"本文"});
            let url = (case == "invalid").then_some("https://example.com/cover.png");
            if let Some(url) = url {
                original["cover_url"] = json!(url);
            }
            let path = temp.0.join("cover.png");
            if case == "invalid" {
                std::fs::write(&path, b"invalid original image").unwrap();
            } else {
                image::RgbImage::from_pixel(1, 1, image::Rgb([12, 34, 56]))
                    .save(&path)
                    .unwrap();
            }
            let first = library
                .save_readable_work(
                    WorkSave {
                        request: LibraryId::now_v7(),
                        provider: "pixiv",
                        key: "1",
                        route: "save",
                        data: &original,
                        assets: vec![StagedAsset {
                            role: "cover".into(),
                            name: "cover.png".into(),
                            path,
                            url: url.map(str::to_owned),
                            mime_type: Some("image/png".into()),
                        }],
                    },
                    None,
                )
                .unwrap();
            original["cover_url"] = json!("https://s.pximg.net/common/images/limit_r18.png");
            assert!(
                super::super::restore_held(
                    &library,
                    Some(first.entity_id),
                    &original,
                    &temp.0.join("staging"),
                    "pixiv"
                )
                .is_err(),
                "{case}"
            );
            assert_eq!(
                library.current_snapshot(first.entity_id).unwrap(),
                first.snapshot_id
            );
            library.verify().unwrap();
        }
    }

    #[tokio::test]
    async fn app_preserves_bound_images_without_gate_http_and_keeps_uncertain_new_input() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(AtomicUsize::new(0));
        let seen = Arc::clone(&requests);
        let server = tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut bytes = [0u8; 4096];
                let count = socket.read(&mut bytes).await.unwrap();
                if count == 0 {
                    continue;
                }
                seen.fetch_add(1, Ordering::SeqCst);
                socket
                    .write_all(
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    )
                    .await
                    .unwrap();
            }
        });
        let temp =
            Temp(std::env::temp_dir().join(format!("piep-pixiv-gate-{}", LibraryId::now_v7())));
        let library = Arc::new(Library::create(&temp.0.join("shelf")).unwrap());
        let shelf = Arc::new(Shelf::with_test_library(Arc::clone(&library)));
        let cover = format!("http://{address}/cover.png");
        let image = format!("http://{address}/one.png");
        let original = json!({"detail":{"title":"題", "caption":"説明", "tags":["タグ"]},
            "text":"本文[uploadedimage:1]", "cover_url":cover, "images":{"1":{"urls":{"original":image}}}});
        let mut assets = Vec::new();
        // 移した棚で残っている別名も、読書用の対応で照合する。
        for (role, name, url) in [
            ("cover", "cover.png", &cover),
            ("illustration", "illust_inline_1.png", &image),
        ] {
            let path = temp.0.join(name);
            image::RgbImage::from_pixel(1, 1, image::Rgb([12, 34, 56]))
                .save(&path)
                .unwrap();
            assets.push(StagedAsset {
                role: role.into(),
                name: name.into(),
                path,
                url: Some(url.clone()),
                mime_type: Some("image/png".into()),
            });
        }
        let first = library
            .save_readable_work(
                WorkSave {
                    request: LibraryId::now_v7(),
                    provider: "pixiv",
                    key: "1",
                    route: "save",
                    data: &original,
                    assets,
                },
                None,
            )
            .unwrap();
        let gate = format!("http://{address}/common/images/limit_r18.png");
        let mut masked = original.clone();
        masked["cover_url"] = json!(gate);
        masked["images"]["1"]["urls"]["original"] = json!(gate);
        let saved = save_to_shelf(
            Arc::clone(&shelf),
            Arc::clone(&library),
            masked.clone(),
            "pixiv".into(),
            "1".into(),
            None,
            None,
        )
        .await
        .unwrap();
        assert!(!saved.revised);
        assert!(saved.missing_references.is_empty());
        assert_eq!(
            library.current_snapshot(first.entity_id).unwrap(),
            first.snapshot_id
        );
        assert_eq!(requests.load(Ordering::SeqCst), 0);
        let document = library
            .reader_document(first.entity_id, ResourceScope::Snapshot(first.snapshot_id))
            .unwrap();
        assert_eq!(
            document.bindings[0]
                .resource
                .as_ref()
                .unwrap()
                .file
                .as_ref()
                .unwrap()
                .id,
            Fingerprint::of(&std::fs::read(temp.0.join("illust_inline_1.png")).unwrap())
        );
        // 本文が変わったときに、前の版の画像を新しい本文へ黙って引き継がない。
        masked["text"] = json!("別の本文[uploadedimage:1]");
        let error = save_to_shelf(
            Arc::clone(&shelf),
            Arc::clone(&library),
            masked.clone(),
            "pixiv".into(),
            "1".into(),
            None,
            None,
        )
        .await
        .unwrap_err();
        assert!(error.contains("本文が変わり"));
        assert_eq!(requests.load(Ordering::SeqCst), 0);
        assert_eq!(
            library.current_snapshot(first.entity_id).unwrap(),
            first.snapshot_id
        );
        let id: Vec<u8> = library.reader().unwrap().query_row("SELECT id FROM acquisition_attempts WHERE entity_id=?1 ORDER BY epoch DESC LIMIT 1", [first.entity_id.as_bytes().as_slice()], |r|r.get(0)).unwrap();
        let failed = library
            .acquisition_status(LibraryId::from_slice(&id).unwrap())
            .unwrap();
        assert_eq!(failed.state, AcquisitionState::Cancelled);
        assert_eq!(failed.payloads.len(), 1);
        let mut bytes = Vec::new();
        library
            .objects
            .export(&library.file(failed.payloads[0].file).unwrap(), &mut bytes)
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&bytes).unwrap()["text"],
            masked["text"]
        );
        // 実画像の URL が変われば使い回さず、取得の成否で保存する。
        masked["text"] = original["text"].clone();
        masked["images"]["1"]["urls"]["original"] = json!(format!("http://{address}/changed.png"));
        assert!(save_to_shelf(
            shelf,
            Arc::clone(&library),
            masked,
            "pixiv".into(),
            "1".into(),
            None,
            None
        )
        .await
        .unwrap_err()
        .contains("404"));
        assert_eq!(requests.load(Ordering::SeqCst), 1);
        assert_eq!(
            library.current_snapshot(first.entity_id).unwrap(),
            first.snapshot_id
        );
        library.verify().unwrap();
        server.abort();
    }

    #[tokio::test]
    async fn both_unchanged_and_saved_results_respect_the_active_edit_without_rewriting_the_source()
    {
        use crate::library::{
            CaptureSource, EditBlock, EditContent, EditMedia, EditPointerChange, EditRequest,
        };
        let temp =
            Temp(std::env::temp_dir().join(format!("piep-edited-save-{}", LibraryId::now_v7())));
        let library = Arc::new(Library::create(&temp.0.join("shelf")).unwrap());
        let shelf = Arc::new(Shelf::with_test_library(Arc::clone(&library)));
        let original = json!({"detail":{"title":"題", "caption":"説明", "tags":["a","b"]}, "text":"本文[uploadedimage:1]"});
        let first = library
            .save_readable_work(
                WorkSave {
                    request: LibraryId::now_v7(),
                    provider: "pixiv",
                    key: "1",
                    route: "save",
                    data: &original,
                    assets: vec![],
                },
                None,
            )
            .unwrap();
        let path = temp.0.join("replacement.png");
        image::RgbImage::from_pixel(1, 1, image::Rgb([12, 34, 56]))
            .save(&path)
            .unwrap();
        let file = library.store_original(&CaptureSource::File(path)).unwrap();
        let content = EditContent {
            format: "piep-edit-v1".into(),
            title: None,
            provenance: None,
            blocks: vec![EditBlock {
                id: LibraryId::now_v7(),
                ordinal: 0,
                kind: "image".into(),
                text: None,
                attrs: json!({"sourceRef":"[uploadedimage:1]"}),
                media_slot: Some("replacement".into()),
            }],
            media: vec![EditMedia {
                slot: "replacement".into(),
                file: file.id,
                original_name: "replacement.png".into(),
                mime_type: Some("image/png".into()),
            }],
        };
        library
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
        let saved = save_to_shelf(
            Arc::clone(&shelf),
            Arc::clone(&library),
            original.clone(),
            "pixiv".into(),
            "1".into(),
            None,
            None,
        )
        .await
        .unwrap();
        assert!(saved.missing_references.is_empty());
        let before: i64 = library
            .reader()
            .unwrap()
            .query_row("SELECT count(*) FROM acquisition_attempts", [], |r| {
                r.get(0)
            })
            .unwrap();
        // 取得前の比較では並びが違い、保存の正規化では同じ内容になる入力を通す。
        let mut reordered = original.clone();
        reordered["detail"]["tags"] = json!(["b", "a"]);
        assert_ne!(
            super::super::super::compute_content_details(&original, "pixiv").0,
            super::super::super::compute_content_details(&reordered, "pixiv").0
        );
        let saved = save_to_shelf(
            Arc::clone(&shelf),
            Arc::clone(&library),
            reordered.clone(),
            "pixiv".into(),
            "1".into(),
            None,
            None,
        )
        .await
        .unwrap();
        assert!(saved.missing_references.is_empty());
        assert!(!saved.revised);
        let after: i64 = library
            .reader()
            .unwrap()
            .query_row("SELECT count(*) FROM acquisition_attempts", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert!(after > before, "the regular save path must also run");
        let state = library.edit_state(first.entity_id, library.id).unwrap();
        library
            .change_edit_pointers(
                LibraryId::now_v7(),
                first.entity_id,
                library.id,
                state.generation,
                EditPointerChange::Unpublish,
            )
            .unwrap();
        let saved = save_to_shelf(
            shelf,
            Arc::clone(&library),
            reordered,
            "pixiv".into(),
            "1".into(),
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(saved.missing_references, ["[uploadedimage:1]"]);
        assert_eq!(
            library.current_snapshot(first.entity_id).unwrap(),
            first.snapshot_id
        );
        assert!(library
            .reader_document(first.entity_id, ResourceScope::Snapshot(first.snapshot_id))
            .unwrap()
            .bindings[0]
            .resource
            .is_none());
        library.verify().unwrap();
    }

    /// 縮小版の取り直し: 同じ応答でも保存し直し、原寸に変わった表紙だけを取りに行き、ほかは棚から戻す。
    #[tokio::test]
    async fn upgrading_to_originals_fetches_only_the_changed_cover_and_restores_the_rest() {
        let mut served = std::io::Cursor::new(Vec::new());
        image::RgbImage::from_pixel(4, 4, image::Rgb([200, 10, 10]))
            .write_to(&mut served, image::ImageFormat::Png)
            .unwrap();
        let served = served.into_inner();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let paths = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let seen = Arc::clone(&paths);
        let body = served.clone();
        tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut bytes = [0u8; 4096];
                let count = socket.read(&mut bytes).await.unwrap();
                if count == 0 {
                    continue;
                }
                let request = String::from_utf8_lossy(&bytes[..count]).to_string();
                let path = request
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or_default()
                    .to_string();
                seen.lock().unwrap().push(path.clone());
                let mut response = if path == "/cover_original.png" {
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    )
                    .into_bytes()
                } else {
                    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        .to_vec()
                };
                if path == "/cover_original.png" {
                    response.extend_from_slice(&body);
                }
                let _ = socket.write_all(&response).await;
                let _ = socket.shutdown().await;
            }
        });
        let temp = Temp(
            std::env::temp_dir().join(format!("piep-pixiv-originals-{}", LibraryId::now_v7())),
        );
        let library = Arc::new(Library::create(&temp.0.join("shelf")).unwrap());
        let shelf = Arc::new(Shelf::with_test_library(Arc::clone(&library)));
        let cover = format!("http://{address}/cover_master.png");
        let image = format!("http://{address}/one.png");
        let original = json!({"detail":{"title":"題", "caption":"説明", "tags":["タグ"]},
            "text":"本文[uploadedimage:1]", "cover_url":cover, "images":{"1":{"urls":{"original":image}}}});
        let mut assets = Vec::new();
        for (role, name, url, shade) in [
            ("cover", "cover_master.png", &cover, 1u8),
            ("illustration", "inline_1.png", &image, 2u8),
        ] {
            let path = temp.0.join(name);
            image::RgbImage::from_pixel(1, 1, image::Rgb([shade, 34, 56]))
                .save(&path)
                .unwrap();
            assets.push(StagedAsset {
                role: role.into(),
                name: name.into(),
                path,
                url: Some(url.clone()),
                mime_type: Some("image/png".into()),
            });
        }
        let first = library
            .save_readable_work(
                WorkSave {
                    request: LibraryId::now_v7(),
                    provider: "pixiv",
                    key: "1",
                    route: "save",
                    data: &original,
                    assets,
                },
                None,
            )
            .unwrap();
        let mut upgraded = original.clone();
        upgraded[crate::downloader::pixiv_originals::COVER_ORIGINAL_FIELD] =
            json!(format!("http://{address}/cover_original.png"));
        let saved = save_originals_to_shelf(
            Arc::clone(&shelf),
            Arc::clone(&library),
            upgraded,
            "1".into(),
        )
        .await
        .unwrap_or_else(|error| panic!("{error} {:?}", paths.lock().unwrap()));
        assert_eq!(saved.work, first.entity_id);
        // 版が増えないことは、pixiv の画像の住所で `library_original_images` の試験が確かめる
        // （ここの住所は手元のサーバーなので、縮小版と原寸を同じ画像とみなせない）。
        assert_eq!(
            *paths.lock().unwrap(),
            vec!["/cover_original.png".to_string()]
        );
        let snapshot = library.current_snapshot(first.entity_id).unwrap();
        assert_ne!(snapshot, first.snapshot_id);
        assert_eq!(
            library
                .media_file(first.entity_id, snapshot, "cover")
                .unwrap()
                .id,
            Fingerprint::of(&served)
        );
        // 挿絵は棚から戻したもの（取りに行っていない）。
        let document = library
            .reader_document(first.entity_id, ResourceScope::Snapshot(snapshot))
            .unwrap();
        assert_eq!(
            document.bindings[0]
                .resource
                .as_ref()
                .unwrap()
                .file
                .as_ref()
                .unwrap()
                .id,
            Fingerprint::of(&std::fs::read(temp.0.join("inline_1.png")).unwrap())
        );
        library.verify().unwrap();
    }
}
