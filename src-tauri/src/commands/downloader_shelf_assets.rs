//! 再取得の作業場所に、同じ URL の取得済みファイルを戻す。
use super::*;
use crate::downloader::asset_downloader::{extract_download_targets, fanbox_former_file_names};
use crate::library::asset_repairs::AssetRepairObservation;

pub(super) enum SavePlan {
    Save,
    Unchanged,
    ReaderOnly(Value),
}

pub(super) fn save_plan(
    library: &Library,
    work: LibraryId,
    data: &Value,
    source: &str,
) -> Result<SavePlan, String> {
    if !skip_save_on_shelf(library, work, data, source, true)? {
        return Ok(SavePlan::Save);
    }
    let snapshot = library.current_snapshot(work).map_err(|e| e.to_string())?;
    match library.reader_document(work, ResourceScope::Snapshot(snapshot)) {
        Ok(_) => Ok(SavePlan::Unchanged),
        Err(crate::library::Error::ResourceUnavailable) => {
            let original = library
                .work_original(work, snapshot)
                .map_err(|e| e.to_string())?
                .ok_or("読書の対応を補う原本がありません")?;
            Ok(SavePlan::ReaderOnly(
                serde_json::from_str(&original).map_err(|e| e.to_string())?,
            ))
        }
        Err(error) => Err(error.to_string()),
    }
}

pub(super) fn same_source(previous: &Value, next: &Value) -> Result<bool, String> {
    let previous = crate::fanbox_api::payload::post_or_self(previous);
    let next = crate::fanbox_api::payload::post_or_self(next);
    for key in ["title", "caption", "tags", "creatorId", "user", "type"] {
        if previous.get(key) != next.get(key) {
            return Ok(false);
        }
    }
    for key in ["publishedDatetime", "updatedDatetime"] {
        if !crate::source_time::same_instant(
            previous.get(key).and_then(Value::as_str),
            next.get(key).and_then(Value::as_str),
        ) {
            return Ok(false);
        }
    }
    let (Some(old), Some(new)) = (
        previous.get("body").filter(|b| b.is_object()),
        next.get("body").filter(|b| b.is_object()),
    ) else {
        return Ok(false);
    };
    let fingerprint = |body: &Value| {
        crate::library::SourceBody::FanboxBody(body.clone())
            .semantic_fingerprint()
            .map_err(|e| e.to_string())
    };
    if fingerprint(old)? != fingerprint(new)? {
        return Ok(false);
    }
    let targets = |data: &Value| {
        let mut targets = Vec::new();
        extract_download_targets(data, true, &mut targets);
        let mut keys = targets
            .into_iter()
            .map(|t| (t.sub_folder, t.filename, t.url))
            .collect::<Vec<_>>();
        keys.sort();
        keys.dedup();
        keys
    };
    Ok(targets(previous) == targets(next))
}

pub(super) fn can_record_attempt(
    library: &Library,
    work: Option<LibraryId>,
    data: &Value,
) -> Result<bool, String> {
    let Some(work) = work else {
        return Ok(false);
    };
    let fail = |e: crate::library::Error| e.to_string();
    if library
        .library_item_generation(work)
        .map_err(fail)?
        .is_none()
    {
        return Ok(false);
    }
    let snapshot = library.current_snapshot(work).map_err(fail)?;
    let Some(previous) = library.work_original(work, snapshot).map_err(fail)? else {
        return Ok(false);
    };
    same_source(
        &serde_json::from_str::<Value>(&previous).map_err(|e| e.to_string())?,
        data,
    )
}

pub(super) fn download_result(
    result: Result<(), String>,
    repair: bool,
) -> Result<Option<String>, String> {
    match result {
        Ok(()) => Ok(None),
        Err(error)
            if repair
                && error
                    .split_once(" asset downloads failed (")
                    .is_some_and(|(n, _)| n.parse::<usize>().is_ok_and(|n| n > 0)) =>
        {
            log::warn!("手元の版を保ち、添付の再取得の失敗を記録します: {error}");
            Ok(Some(error))
        }
        Err(error) => Err(error),
    }
}

pub(super) struct RestoredAsset {
    pub name: String,
    pub mime_type: Option<String>,
}
type Aliases = HashMap<(String, String), RestoredAsset>;

pub(super) fn restore_held(
    library: &Library,
    work: Option<LibraryId>,
    data: &Value,
    dir: &Path,
    source: &str,
) -> Result<Aliases, String> {
    let mut aliases = HashMap::new();
    let Some(work) = work else {
        return Ok(aliases);
    };
    let fail = |e: crate::library::Error| e.to_string();
    let snapshot = library.current_snapshot(work).map_err(fail)?;
    let Some(raw) = library.work_original(work, snapshot).map_err(fail)? else {
        return Ok(aliases);
    };
    let previous: Value = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
    let (mut old_targets, mut targets) = (Vec::new(), Vec::new());
    extract_download_targets(&previous, source == "fanbox", &mut old_targets);
    extract_download_targets(data, source == "fanbox", &mut targets);
    let former = if source == "fanbox" {
        fanbox_former_file_names(&previous)
    } else {
        HashMap::new()
    };
    let resources = library
        .resource_manifest(work, ResourceScope::Snapshot(snapshot))
        .map_err(fail)?
        .resources;
    let mut restored = HashSet::new();
    let mut reused_slots = std::collections::BTreeSet::new();
    for target in targets {
        let role = target_role(target.sub_folder);
        // 名前が同じでも URL が変わった資産は、以前の実体で埋めない。
        if !old_targets.iter().any(|old| {
            old.sub_folder == target.sub_folder
                && old.filename == target.filename
                && old.url == target.url
        }) {
            continue;
        }
        let mut names = vec![target.filename.as_str()];
        if role == "file" {
            names.extend(former.get(&target.filename).map(String::as_str));
        }
        let mut held = None;
        for name in names {
            let reference = asset_reference(role, name).map_err(fail)?;
            if let Some(info) = resources
                .iter()
                .find(|r| r.reference == reference && r.file.is_some())
            {
                // 古い同名添付が複数の URL を表す場合は対応を推測しない。
                if role == "file"
                    && name != target.filename
                    && old_targets
                        .iter()
                        .filter(|t| former.get(&t.filename).is_some_and(|n| n == name))
                        .count()
                        != 1
                {
                    continue;
                }
                held = Some(info);
                break;
            }
        }
        let Some(held) = held else {
            continue;
        };
        let destination = dir.join(target.sub_folder).join(&target.filename);
        if Path::new(&target.filename).components().count() != 1
            || target.filename.contains(['/', '\\'])
        {
            return Err("添付の保存名が不正です".into());
        }
        if !restored.insert(destination.clone()) {
            continue;
        }
        std::fs::create_dir_all(destination.parent().ok_or("添付の保存先がありません")?)
            .map_err(|e| e.to_string())?;
        // 参照だけでなく、圧縮された元バイト列を検証して復元する。破損は保存の失敗として返す。
        library
            .export_resource(
                work,
                ResourceScope::Snapshot(snapshot),
                &held.reference,
                &destination,
            )
            .map_err(fail)?;
        if let Some(name) = &held.original_name {
            let preserve =
                role == "file" || asset_reference(role, name).map_err(fail)? == held.reference;
            if preserve && reused_slots.insert(held.reference.clone()) {
                aliases.insert(
                    (role.into(), target.filename),
                    RestoredAsset {
                        name: name.clone(),
                        mime_type: held.mime_type.clone(),
                    },
                );
            }
        }
    }
    Ok(aliases)
}

pub(super) fn require_all_held(
    library: &Library,
    work: LibraryId,
    assets: &[StagedAsset],
) -> Result<(), String> {
    let fail = |e: crate::library::Error| e.to_string();
    let snapshot = library.current_snapshot(work).map_err(fail)?;
    let expected = library
        .resource_manifest(work, ResourceScope::Snapshot(snapshot))
        .map_err(fail)?
        .resources
        .into_iter()
        .filter(|r| {
            r.file.is_some()
                && matches!(
                    r.reference,
                    ResourceReference::Media { .. } | ResourceReference::File { .. }
                )
        })
        .map(|r| r.reference)
        .collect::<std::collections::BTreeSet<_>>();
    let actual = assets
        .iter()
        .map(|a| asset_reference(&a.role, &a.name))
        .collect::<Result<std::collections::BTreeSet<_>, _>>()
        .map_err(fail)?;
    if expected != actual {
        return Err("読書の対応を補う前に、取得済みの画像・添付をすべて復元できませんでした。手元の版は変更しません".into());
    }
    Ok(())
}

pub(super) fn observation(data: &Value, assets: &[StagedAsset]) -> AssetRepairObservation {
    let mut targets = Vec::new();
    extract_download_targets(data, true, &mut targets);
    let complete = targets.iter().all(|target| {
        assets
            .iter()
            .any(|a| a.role == target_role(target.sub_folder) && a.name == target.filename)
    });
    let mut observation = AssetRepairObservation {
        expected_urls: targets.into_iter().map(|t| t.url).collect(),
        complete,
    };
    observation.normalize();
    observation
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::asset_repairs::MAX_REPAIR_ATTEMPTS;
    use serde_json::json;

    struct Fixture(PathBuf);
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn data() -> Value {
        json!({"id":"1","title":"投稿","creatorId":"author","type":"file","body":{"files":[
            {"id":"a","name":"alpha","url":"https://example.invalid/a.txt"},
            {"id":"b","name":"beta","url":"https://example.invalid/b.txt"}]}})
    }
    fn save(
        library: &Library,
        data: &Value,
        assets: Vec<StagedAsset>,
        complete: bool,
    ) -> crate::library::CaptureResult {
        let mut observed = observation(data, &assets);
        observed.complete = complete;
        library
            .save_readable_work(
                WorkSave {
                    request: LibraryId::now_v7(),
                    provider: "fanbox",
                    key: "1",
                    route: "save",
                    data,
                    assets,
                },
                Some(observed),
            )
            .unwrap()
    }
    #[test]
    fn retry_preserves_former_names_and_bytes_then_stops_only_for_the_same_intact_state() {
        let fixture = Fixture(
            std::env::temp_dir().join(format!("piep-shelf-assets-{}", LibraryId::now_v7())),
        );
        std::fs::create_dir_all(&fixture.0).unwrap();
        let library = Library::create(&fixture.0.join("shelf")).unwrap();
        let path = fixture.0.join("alpha.txt");
        std::fs::write(&path, b"held original bytes").unwrap();
        let data = data();
        let first = save(
            &library,
            &data,
            vec![StagedAsset {
                role: "file".into(),
                name: "alpha.txt".into(),
                path,
                url: Some("https://example.invalid/a.txt".into()),
                mime_type: Some("text/plain".into()),
            }],
            false,
        );
        for attempt in 2..=MAX_REPAIR_ATTEMPTS {
            let dir = fixture.0.join(format!("retry{attempt}"));
            let aliases =
                restore_held(&library, Some(first.entity_id), &data, &dir, "fanbox").unwrap();
            assert!(download_result(
                Err("1 asset downloads failed (1 succeeded): HTTP 404".into()),
                can_record_attempt(&library, Some(first.entity_id), &data).unwrap()
            )
            .unwrap()
            .is_some());
            assert_eq!(
                std::fs::read(dir.join("files/alpha_a.txt")).unwrap(),
                b"held original bytes"
            );
            let mut assets = staged_from(&dir, &data, "fanbox").unwrap();
            assert!(!observation(&data, &assets).complete);
            assert_eq!(assets.len(), 1);
            for asset in &mut assets {
                let held = &aliases[&(asset.role.clone(), asset.name.clone())];
                asset.name.clone_from(&held.name);
                asset.mime_type.clone_from(&held.mime_type);
            }
            let result = save(&library, &data, assets, false);
            assert_eq!(result.snapshot_id, first.snapshot_id);
        }
        assert!(!unchanged_on_shelf(&library, first.entity_id, &data, "fanbox").unwrap());
        assert!(skip_save_on_shelf(&library, first.entity_id, &data, "fanbox", true).unwrap());
        let mut changed = data.clone();
        changed["body"]["files"][1]["url"] = json!("https://example.invalid/new.txt");
        assert!(!skip_save_on_shelf(&library, first.entity_id, &changed, "fanbox", true).unwrap());
        changed["body"]["files"][0]["url"] = json!("https://example.invalid/changed.txt");
        let dir = fixture.0.join("changed");
        assert!(
            restore_held(&library, Some(first.entity_id), &changed, &dir, "fanbox")
                .unwrap()
                .is_empty()
        );
        assert!(!dir.exists());
        library.verify().unwrap();
        let resources = library
            .resource_manifest(first.entity_id, ResourceScope::Snapshot(first.snapshot_id))
            .unwrap();
        let file = resources
            .resources
            .iter()
            .find(|r| matches!(r.reference, ResourceReference::File { .. }))
            .unwrap()
            .file
            .as_ref()
            .unwrap();
        let chunk = library.file(file.id).unwrap().chunks[0].id.to_string();
        let object = library
            .root()
            .join("objects/v1")
            .join(&chunk[..2])
            .join(format!("{chunk}.obj"));
        let bytes = std::fs::read(&object).unwrap();
        std::fs::write(&object, b"broken storage object").unwrap();
        assert!(skip_save_on_shelf(&library, first.entity_id, &data, "fanbox", true).is_err());
        std::fs::write(&object, bytes).unwrap();
        std::fs::rename(
            library.root().join("objects"),
            library.root().join("objects-unavailable"),
        )
        .unwrap();
        assert!(!matches!(
            skip_save_on_shelf(&library, first.entity_id, &data, "fanbox", true),
            Ok(true)
        ));
        assert!(restore_held(
            &library,
            Some(first.entity_id),
            &data,
            &fixture.0.join("broken"),
            "fanbox"
        )
        .is_err());
    }

    #[test]
    fn duplicate_former_file_names_are_not_guessed_and_complete_assets_are_observed() {
        let fixture = Fixture(
            std::env::temp_dir().join(format!("piep-shelf-assets-{}", LibraryId::now_v7())),
        );
        std::fs::create_dir_all(&fixture.0).unwrap();
        let library = Library::create(&fixture.0.join("shelf")).unwrap();
        let path = fixture.0.join("alpha.txt");
        std::fs::write(&path, b"ambiguous").unwrap();
        let mut data = data();
        data["body"]["files"][1]["name"] = json!("alpha");
        let first = save(
            &library,
            &data,
            vec![StagedAsset {
                role: "file".into(),
                name: "alpha.txt".into(),
                path,
                url: None,
                mime_type: None,
            }],
            false,
        );
        let dir = fixture.0.join("retry");
        assert!(
            restore_held(&library, Some(first.entity_id), &data, &dir, "fanbox")
                .unwrap()
                .is_empty()
        );
        assert!(!dir.exists());
        let mut targets = Vec::new();
        extract_download_targets(&data, true, &mut targets);
        let assets = targets
            .into_iter()
            .map(|t| StagedAsset {
                role: target_role(t.sub_folder).into(),
                name: t.filename,
                path: PathBuf::new(),
                url: Some(t.url),
                mime_type: None,
            })
            .collect::<Vec<_>>();
        assert!(observation(&data, &assets).complete);
    }

    #[test]
    fn asset_failure_is_recorded_only_for_the_same_source_including_rich_body_and_metadata() {
        let mut previous = data();
        previous["body"]["text"] = json!("同じ文章");
        assert!(same_source(&previous, &previous).unwrap());
        for pointer in [
            "/title",
            "/body/files/1/url",
            "/body/files/1/name",
            "/body/text",
        ] {
            let mut next = previous.clone();
            *next.pointer_mut(pointer).unwrap() = json!("変更");
            assert!(!same_source(&previous, &next).unwrap(), "{pointer}");
        }
        let error = "1 asset downloads failed (1 succeeded): HTTP 404";
        assert_eq!(
            download_result(Err(error.into()), true).unwrap(),
            Some(error.into())
        );
        assert_eq!(
            download_result(Err(error.into()), false).unwrap_err(),
            error
        );
        assert_eq!(download_result(Ok(()), true).unwrap(), None);
        assert!(download_result(Err("Asset destinations collide".into()), true).is_err());
    }

    #[tokio::test]
    async fn real_http_failures_are_bounded_and_never_fetch_the_held_attachment_again() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(AtomicUsize::new(0));
        let seen = Arc::clone(&requests);
        let server = tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = [0u8; 4096];
                let count = socket.read(&mut request).await.unwrap();
                assert!(String::from_utf8_lossy(&request[..count]).starts_with("GET /b.txt "));
                seen.fetch_add(1, Ordering::SeqCst);
                socket
                    .write_all(
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    )
                    .await
                    .unwrap();
            }
        });
        let fixture = Fixture(
            std::env::temp_dir().join(format!("piep-shelf-assets-{}", LibraryId::now_v7())),
        );
        std::fs::create_dir_all(&fixture.0).unwrap();
        let library = Library::create(&fixture.0.join("shelf")).unwrap();
        let mut data = data();
        for (index, name) in ["a", "b"].iter().enumerate() {
            data["body"]["files"][index]["url"] = json!(format!("http://{address}/{name}.txt"));
        }
        let path = fixture.0.join("held.txt");
        std::fs::write(&path, b"already held").unwrap();
        let first = save(
            &library,
            &data,
            vec![StagedAsset {
                role: "file".into(),
                name: "alpha_a.txt".into(),
                path,
                url: Some(format!("http://{address}/a.txt")),
                mime_type: None,
            }],
            false,
        );
        for index in 0..5 {
            if skip_save_on_shelf(&library, first.entity_id, &data, "fanbox", true).unwrap() {
                continue;
            }
            let dir = fixture.0.join(format!("attempt{index}"));
            let aliases = restore_held(
                &library,
                Some(first.entity_id),
                &data,
                &dir.join("data_assets"),
                "fanbox",
            )
            .unwrap();
            let mut linked = data.clone();
            let result = crate::downloader::asset_downloader::download_and_link_assets(
                &mut linked,
                &dir.join("data.json"),
                true,
                None,
                None,
            )
            .await;
            assert!(download_result(
                result,
                can_record_attempt(&library, Some(first.entity_id), &data).unwrap()
            )
            .unwrap()
            .unwrap()
            .contains("404"));
            let mut assets = staged_from(&dir.join("data_assets"), &data, "fanbox").unwrap();
            for asset in &mut assets {
                let held = &aliases[&(asset.role.clone(), asset.name.clone())];
                asset.name.clone_from(&held.name);
                asset.mime_type.clone_from(&held.mime_type);
            }
            assert_eq!(
                save(&library, &data, assets, false).snapshot_id,
                first.snapshot_id
            );
        }
        assert_eq!(requests.load(Ordering::SeqCst), 2);
        assert_eq!(
            library
                .asset_repair_plan(
                    first.entity_id,
                    first.snapshot_id,
                    &observation(&data, &[]).expected_urls
                )
                .unwrap()
                .attempts,
            3
        );
        library.verify().unwrap();
        server.abort();
        assert!(server.await.unwrap_err().is_cancelled());
    }

    #[test]
    fn preparing_an_old_fanbox_reading_copy_does_not_download_or_spend_an_exhausted_attempt() {
        let fixture = Fixture(
            std::env::temp_dir().join(format!("piep-reader-preparation-{}", LibraryId::now_v7())),
        );
        std::fs::create_dir_all(&fixture.0).unwrap();
        let library = Library::create(&fixture.0.join("shelf")).unwrap();
        let data = data();
        let path = fixture.0.join("alpha.txt");
        std::fs::write(&path, b"held").unwrap();
        let asset = StagedAsset {
            role: "file".into(),
            name: "alpha.txt".into(),
            path,
            url: Some("https://example.invalid/a.txt".into()),
            mime_type: Some("text/plain".into()),
        };
        let mut first = None;
        for _ in 0..3 {
            first = Some(
                library
                    .save_work_with_asset_repair(
                        WorkSave {
                            request: LibraryId::now_v7(),
                            provider: "fanbox",
                            key: "1",
                            route: "save",
                            data: &data,
                            assets: vec![asset.clone()],
                        },
                        observation(&data, std::slice::from_ref(&asset)),
                    )
                    .unwrap(),
            );
        }
        let first = first.unwrap();
        assert!(library
            .reader_document(first.entity_id, ResourceScope::Snapshot(first.snapshot_id))
            .is_err());
        let SavePlan::ReaderOnly(previous) =
            save_plan(&library, first.entity_id, &data, "fanbox").unwrap()
        else {
            panic!("only reader preparation is needed")
        };
        let dir = fixture.0.join("prepare");
        let aliases =
            restore_held(&library, Some(first.entity_id), &previous, &dir, "fanbox").unwrap();
        let mut assets = staged_from(&dir, &previous, "fanbox").unwrap();
        for asset in &mut assets {
            let held = &aliases[&(asset.role.clone(), asset.name.clone())];
            asset.name.clone_from(&held.name);
            asset.mime_type.clone_from(&held.mime_type);
        }
        require_all_held(&library, first.entity_id, &assets).unwrap();
        let result = library
            .save_readable_work(
                WorkSave {
                    request: LibraryId::now_v7(),
                    provider: "fanbox",
                    key: "1",
                    route: "save",
                    data: &previous,
                    assets,
                },
                None,
            )
            .unwrap();
        assert_eq!(result.snapshot_id, first.snapshot_id);
        assert!(!result.created_revision);
        assert_eq!(
            library
                .asset_repair_plan(
                    first.entity_id,
                    first.snapshot_id,
                    &observation(&data, &[]).expected_urls
                )
                .unwrap()
                .attempts,
            3
        );
        library
            .reader_document(first.entity_id, ResourceScope::Snapshot(first.snapshot_id))
            .unwrap();
        assert!(matches!(
            save_plan(&library, first.entity_id, &data, "fanbox").unwrap(),
            SavePlan::Unchanged
        ));
        library.verify().unwrap();
    }

    #[test]
    fn preparing_an_old_pixiv_copy_restores_its_own_images_and_rejects_a_missing_held_asset() {
        let fixture = Fixture(
            std::env::temp_dir().join(format!("piep-reader-preparation-{}", LibraryId::now_v7())),
        );
        std::fs::create_dir_all(&fixture.0).unwrap();
        let library = Library::create(&fixture.0.join("shelf")).unwrap();
        let url = "https://i.pximg.net/original/inline.jpg";
        let data = json!({"detail":{"title":"題","user":{"id":"77","name":"保存時の作者"}},"text":"[uploadedimage:1]","images":{"1":{"urls":{"original":url}}}});
        let path = fixture.0.join("inline_1.jpg");
        std::fs::write(&path, b"held image bytes").unwrap();
        let first = library
            .save_work(WorkSave {
                request: LibraryId::now_v7(),
                provider: "pixiv",
                key: "1",
                route: "save",
                data: &data,
                assets: vec![StagedAsset {
                    role: "illustration".into(),
                    name: "inline_1.jpg".into(),
                    path,
                    url: Some(url.into()),
                    mime_type: None,
                }],
            })
            .unwrap();
        let mut received = data.clone();
        received["detail"]["user"]["name"] = json!("現在の作者");
        let SavePlan::ReaderOnly(previous) =
            save_plan(&library, first.entity_id, &received, "pixiv").unwrap()
        else {
            panic!("only reader preparation is needed")
        };
        assert_eq!(previous, data);
        assert!(require_all_held(&library, first.entity_id, &[]).is_err());
        let dir = fixture.0.join("prepare");
        let aliases =
            restore_held(&library, Some(first.entity_id), &previous, &dir, "pixiv").unwrap();
        let mut assets = staged_from(&dir, &previous, "pixiv").unwrap();
        for asset in &mut assets {
            let held = &aliases[&(asset.role.clone(), asset.name.clone())];
            asset.name.clone_from(&held.name);
            asset.mime_type.clone_from(&held.mime_type);
        }
        require_all_held(&library, first.entity_id, &assets).unwrap();
        let result = library
            .save_readable_work(
                WorkSave {
                    request: LibraryId::now_v7(),
                    provider: "pixiv",
                    key: "1",
                    route: "save",
                    data: &previous,
                    assets,
                },
                None,
            )
            .unwrap();
        assert_eq!(result.snapshot_id, first.snapshot_id);
        let document = library
            .reader_document(first.entity_id, ResourceScope::Snapshot(first.snapshot_id))
            .unwrap();
        assert!(document.bindings[0].resource.is_some());
        library.verify().unwrap();
    }

    #[tokio::test]
    async fn the_app_save_publishes_a_readable_work_and_keeps_the_old_copy_and_input_on_asset_failure(
    ) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(AtomicUsize::new(0));
        let seen = Arc::clone(&requests);
        let server = tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut bytes = [0u8; 4096];
                let count = socket.read(&mut bytes).await.unwrap();
                seen.fetch_add(1, Ordering::SeqCst);
                let response =
                    if String::from_utf8_lossy(&bytes[..count]).starts_with("GET /a.txt ") {
                        b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nheld"
                            .as_slice()
                    } else {
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                            .as_slice()
                    };
                socket.write_all(response).await.unwrap();
            }
        });
        let fixture =
            Fixture(std::env::temp_dir().join(format!("piep-app-save-{}", LibraryId::now_v7())));
        let library = Arc::new(Library::create(&fixture.0.join("shelf")).unwrap());
        library.switch_over().unwrap();
        let shelf = Arc::new(LibraryV2::with_test_library(Arc::clone(&library)));
        let mut data = json!({"title":"投稿","type":"file","body":{"files":[{"id":"a","name":"alpha","url":format!("http://{address}/a.txt")}]}});
        let saved = save_to_shelf(
            Arc::clone(&shelf),
            Arc::clone(&library),
            data.clone(),
            "fanbox".into(),
            "1".into(),
            None,
            None,
        )
        .await
        .unwrap();
        assert!(saved.created);
        let snapshot = library.current_snapshot(saved.work).unwrap();
        let document = library
            .reader_document(saved.work, ResourceScope::Snapshot(snapshot))
            .unwrap();
        let held = document.bindings[0].resource.as_ref().unwrap();
        let mut bytes = Vec::new();
        library
            .read_resource(
                saved.work,
                ResourceScope::Snapshot(snapshot),
                &held.reference,
                None,
                &mut bytes,
            )
            .unwrap();
        assert_eq!(bytes, b"held");
        data["title"] = json!("取得に失敗した新しい題");
        data["body"]["files"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id":"b","name":"beta","url":format!("http://{address}/b.txt")}));
        let error = save_to_shelf(
            Arc::clone(&shelf),
            Arc::clone(&library),
            data,
            "fanbox".into(),
            "1".into(),
            None,
            None,
        )
        .await
        .unwrap_err();
        assert!(error.contains("404"));
        assert_eq!(requests.load(Ordering::SeqCst), 2);
        assert_eq!(library.current_snapshot(saved.work).unwrap(), snapshot);
        assert_eq!(library.work_card(saved.work).unwrap().title, "投稿");
        let id:Vec<u8>=library.reader().unwrap().query_row("SELECT id FROM acquisition_attempts WHERE entity_id=?1 ORDER BY epoch DESC LIMIT 1",[saved.work.as_bytes().as_slice()],|r|r.get(0)).unwrap();
        let info = library
            .acquisition_status(LibraryId::from_slice(&id).unwrap())
            .unwrap();
        assert_eq!(info.state, crate::library::AcquisitionState::Cancelled);
        assert!(info.reader.is_none());
        assert_eq!(info.payloads.len(), 1);
        let mut bytes = Vec::new();
        library
            .objects
            .export(&library.file(info.payloads[0].file).unwrap(), &mut bytes)
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&bytes).unwrap()["title"],
            json!("取得に失敗した新しい題")
        );
        library.verify().unwrap();
        server.abort();
        assert!(server.await.unwrap_err().is_cancelled());
    }

    #[tokio::test]
    async fn the_app_prepares_existing_reading_bindings_without_http_or_spending_asset_attempts() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(AtomicUsize::new(0));
        let seen = Arc::clone(&requests);
        let server = tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = [0u8; 4096];
                let _ = socket.read(&mut request).await;
                seen.fetch_add(1, Ordering::SeqCst);
                socket
                    .write_all(
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    )
                    .await
                    .unwrap();
            }
        });
        let fixture =
            Fixture(std::env::temp_dir().join(format!("piep-app-reader-{}", LibraryId::now_v7())));
        std::fs::create_dir_all(&fixture.0).unwrap();
        let library = Arc::new(Library::create(&fixture.0.join("shelf")).unwrap());
        library.switch_over().unwrap();
        let data = json!({"title":"投稿","type":"file","body":{"files":[{"id":"a","name":"alpha","url":format!("http://{address}/a.txt")},{"id":"b","name":"beta","url":format!("http://{address}/b.txt")}]}});
        let path = fixture.0.join("alpha.txt");
        std::fs::write(&path, b"held").unwrap();
        let held = StagedAsset {
            role: "file".into(),
            name: "alpha.txt".into(),
            path,
            url: Some(format!("http://{address}/a.txt")),
            mime_type: None,
        };
        let mut first = None;
        for _ in 0..3 {
            first = Some(
                library
                    .save_work_with_asset_repair(
                        WorkSave {
                            request: LibraryId::now_v7(),
                            provider: "fanbox",
                            key: "1",
                            route: "save",
                            data: &data,
                            assets: vec![held.clone()],
                        },
                        observation(&data, std::slice::from_ref(&held)),
                    )
                    .unwrap(),
            );
        }
        let first = first.unwrap();
        let shelf = Arc::new(LibraryV2::with_test_library(Arc::clone(&library)));
        let saved = save_to_shelf(
            shelf,
            Arc::clone(&library),
            data.clone(),
            "fanbox".into(),
            "1".into(),
            None,
            None,
        )
        .await
        .unwrap();
        assert!(!saved.created);
        assert!(!saved.revised);
        assert_eq!(requests.load(Ordering::SeqCst), 0);
        assert_eq!(
            library.current_snapshot(first.entity_id).unwrap(),
            first.snapshot_id
        );
        assert_eq!(
            library
                .asset_repair_plan(
                    first.entity_id,
                    first.snapshot_id,
                    &observation(&data, &[]).expected_urls
                )
                .unwrap()
                .attempts,
            3
        );
        library
            .reader_document(first.entity_id, ResourceScope::Snapshot(first.snapshot_id))
            .unwrap();
        library.verify().unwrap();
        server.abort();
        assert!(server.await.unwrap_err().is_cancelled());
    }
}
