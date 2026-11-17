//! 再取得の作業場所に、同じ URL の取得済みファイルを戻す。
use super::*;
use crate::downloader::asset_downloader::{extract_download_targets, fanbox_former_file_names};
use crate::library::asset_repairs::AssetRepairObservation;

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
    extract_download_targets(&previous, true, &mut old_targets);
    extract_download_targets(data, true, &mut targets);
    let former = fanbox_former_file_names(&previous);
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
            .save_work_with_asset_repair(
                WorkSave {
                    request: LibraryId::now_v7(),
                    provider: "fanbox",
                    key: "1",
                    route: "save",
                    data,
                    assets,
                },
                observed,
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
            let aliases = restore_held(&library, Some(first.entity_id), &data, &dir).unwrap();
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
            restore_held(&library, Some(first.entity_id), &changed, &dir)
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
            &fixture.0.join("broken")
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
        assert!(restore_held(&library, Some(first.entity_id), &data, &dir)
            .unwrap()
            .is_empty());
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
}
