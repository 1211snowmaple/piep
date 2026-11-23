use super::*;
struct Temp(std::path::PathBuf);
impl Drop for Temp {
    fn drop(&mut self) {
        assert_eq!(self.0.parent(), Some(std::env::temp_dir().as_path()));
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn fixture() -> (Temp, Library) {
    let root = std::env::temp_dir().join(format!("piep-discovery-{}", LibraryId::now_v7()));
    let lib = Library::create(&root).unwrap();
    (Temp(root), lib)
}
fn input<'a>(key: &'a str, payload: &'a str) -> DiscoveryInput<'a> {
    DiscoveryInput {
        provider: "pixiv",
        key,
        kind: "revision",
        title: "候補",
        payload,
        target_type: Some("work"),
    }
}

#[test]
fn original_lexemes_and_decisions_survive_observation_retry_clear_and_recreation() {
    let (_temp, lib) = fixture();
    let request = LibraryId::now_v7();
    let raw="{ \"kind\":\"revision\", \"unknown\": 1.230000000000000001, \"originalData\": {\"text\":\"本文\",\"user\":{\"name\":\"作者\"}} }";
    lib.record_discovery(request, input("77", raw), false)
        .unwrap();
    let first = lib.discovery("pixiv", "77").unwrap().unwrap();
    assert_eq!(lib.discovery_payload(first.id).unwrap(), raw);
    assert_eq!(first.generation, 1);
    assert_eq!(
        first.summary.as_ref().unwrap()["originalData"]["user"]["name"],
        "作者"
    );
    assert!(!first.summary.as_ref().unwrap().to_string().contains("本文"));
    lib.record_discovery(request, input("77", raw), false)
        .unwrap();
    assert_eq!(lib.discovery("pixiv", "77").unwrap().unwrap().generation, 1);
    assert!(lib
        .record_discovery(request, input("78", raw), false)
        .is_err());
    assert!(lib
        .entity_by_key(EntityKind::Work, "pixiv", "78")
        .unwrap()
        .is_none());
    let dismiss = LibraryId::now_v7();
    lib.decide_discovery(dismiss, "pixiv", "77", Some(1), true)
        .unwrap();
    lib.record_discovery(LibraryId::now_v7(), input("77", "{}"), false)
        .unwrap();
    let current = lib.discovery("pixiv", "77").unwrap().unwrap();
    assert_eq!(current.decision, "dismissed");
    assert_eq!(lib.discovery_payload(current.id).unwrap(), raw);
    lib.clear_discovery(LibraryId::now_v7(), "pixiv", "77", false)
        .unwrap();
    assert_eq!(
        lib.discovery("pixiv", "77").unwrap().unwrap().decision,
        "dismissed"
    );
    assert!(lib
        .decide_discovery(LibraryId::now_v7(), "pixiv", "77", Some(1), false)
        .is_err());
    let generation = lib.discovery("pixiv", "77").unwrap().unwrap().generation;
    lib.recheck_discovery(
        LibraryId::now_v7(),
        input("77", "{\"kind\":\"revision\"}"),
        generation,
    )
    .unwrap();
    assert_eq!(
        lib.discovery("pixiv", "77").unwrap().unwrap().decision,
        "pending"
    );
    // 古い非表示要求の再送は、その後の明示的な解除を戻さない。
    lib.decide_discovery(dismiss, "pixiv", "77", Some(1), true)
        .unwrap();
    assert_eq!(
        lib.discovery("pixiv", "77").unwrap().unwrap().decision,
        "pending"
    );
    lib.clear_discovery(LibraryId::now_v7(), "pixiv", "77", false)
        .unwrap();
    assert!(lib.discovery("pixiv", "77").unwrap().is_none());
    lib.record_discovery(request, input("77", raw), false)
        .unwrap();
    assert!(lib.discovery("pixiv", "77").unwrap().is_none());
    lib.record_discovery(LibraryId::now_v7(), input("77", raw), false)
        .unwrap();
    assert!(lib.discovery("pixiv", "77").unwrap().unwrap().generation > generation);
    lib.verify().unwrap();
}

#[test]
fn permission_recheck_is_atomic_and_cannot_overwrite_a_newer_choice() {
    let (_temp, lib) = fixture();
    let raw =
        r#"{"holdKind":"restricted","holdReason":"条件待ち","originalData":{"feeRequired":500}}"#;
    lib.record_discovery(LibraryId::now_v7(), input("11", raw), true)
        .unwrap();
    let held = lib.discovery("pixiv", "11").unwrap().unwrap();
    assert_eq!(held.hold_kind.as_deref(), Some("restricted"));
    lib.decide_discovery(
        LibraryId::now_v7(),
        "pixiv",
        "11",
        Some(held.generation),
        true,
    )
    .unwrap();
    lib.decide_discovery(LibraryId::now_v7(), "pixiv", "11", None, false)
        .unwrap();
    assert_eq!(
        lib.discovery("pixiv", "11").unwrap().unwrap().decision,
        "held"
    );
    let expected = lib.discovery("pixiv", "11").unwrap().unwrap().generation;
    lib.decide_discovery(LibraryId::now_v7(), "pixiv", "11", Some(expected), true)
        .unwrap();
    assert!(lib
        .recheck_discovery(LibraryId::now_v7(), input("11", "{}"), expected)
        .is_err());
    let after = lib.discovery("pixiv", "11").unwrap().unwrap();
    assert_eq!(after.decision, "dismissed");
    assert_eq!(lib.discovery_payload(after.id).unwrap(), raw);
    assert!(lib
        .decide_discovery(LibraryId::now_v7(), "pixiv", "missing", None, true)
        .is_err());
    lib.clear_discovery(LibraryId::now_v7(), "pixiv", "missing", true)
        .unwrap();
    assert!(lib
        .entity_by_key(EntityKind::Work, "pixiv", "missing")
        .unwrap()
        .is_none());
    lib.verify().unwrap();
}

#[test]
fn resolving_unavailable_keeps_other_holds_and_counts_do_not_need_original_files() {
    let (_temp, lib) = fixture();
    for (key, kind) in [
        ("a", "unavailable"),
        ("b", "incomplete"),
        ("c", "unavailable"),
    ] {
        let payload = json!({"holdKind":kind,"holdReason":"保留の理由"}).to_string();
        lib.record_discovery(LibraryId::now_v7(), input(key, &payload), true)
            .unwrap();
    }
    lib.decide_discovery(LibraryId::now_v7(), "pixiv", "c", None, true)
        .unwrap();
    lib.clear_discovery(LibraryId::now_v7(), "pixiv", "a", true)
        .unwrap();
    lib.clear_discovery(LibraryId::now_v7(), "pixiv", "b", true)
        .unwrap();
    lib.clear_discovery(LibraryId::now_v7(), "pixiv", "c", true)
        .unwrap();
    assert!(lib.discovery("pixiv", "a").unwrap().is_none());
    assert_eq!(
        lib.discovery("pixiv", "b").unwrap().unwrap().decision,
        "held"
    );
    assert_eq!(
        lib.discovery("pixiv", "c").unwrap().unwrap().decision,
        "dismissed"
    );
    // 一覧と表示用の写しは、圧縮原本が読めなくても台帳だけで読める。
    let objects = lib.root().join("objects");
    std::fs::rename(&objects, lib.root().join("objects-unavailable")).unwrap();
    let rows = lib.discoveries(Some("deferred"), None, 1).unwrap();
    assert_eq!(rows[0].key, "b");
    assert_eq!(
        rows[0].summary.as_ref().unwrap()["holdReason"],
        "保留の理由"
    );
    assert_eq!(
        lib.discoveries(Some("deferred"), Some(("pixiv", "b")), 100)
            .unwrap()
            .len(),
        1
    );
    assert!(lib.discovery_payload(rows[0].id).is_err());
    assert!(lib.discoveries(None, None, 0).is_err());
}
