use piep_library_protocol::{
    Decimal, Hash, Id, ResourceReference, TransferManifest, ValidatedTransfer, MAX_MANIFEST_BYTES,
};
use piep_storage::Fingerprint;
use serde_json::{json, Value};

const GOLDEN: &[u8] = include_bytes!("fixtures/resource.json");
fn manifest() -> Value {
    serde_json::from_slice(GOLDEN).unwrap()
}
fn rejected(value: &Value) {
    assert!(ValidatedTransfer::parse(&serde_json::to_vec(value).unwrap()).is_err());
}

#[test]
fn frozen_wire_representation_keeps_ids_hex_and_sizes_in_strings() {
    let transfer = ValidatedTransfer::parse(GOLDEN).unwrap();
    assert_eq!(transfer.file().id, Fingerprint::of(b"original\r\n\0EXIF"));
    assert_eq!(transfer.file().length, 15);
    assert_eq!(
        transfer.manifest().encode().unwrap(),
        GOLDEN.trim_ascii_end()
    );
    assert_eq!(transfer.bytes(), GOLDEN);
    assert_eq!(transfer.id(), Fingerprint::of(GOLDEN));
    assert_eq!(
        serde_json::to_value(transfer.manifest()).unwrap(),
        manifest()
    );
}

#[test]
fn u64_survives_javascript_precision_boundary_and_rejects_numbers_and_noncanonical_strings() {
    for n in [0, 9_007_199_254_740_993, u64::MAX] {
        let bytes = serde_json::to_vec(&Decimal::new(n)).unwrap();
        assert_eq!(bytes, format!("\"{n}\"").as_bytes());
        assert_eq!(serde_json::from_slice::<Decimal>(&bytes).unwrap().get(), n);
    }
    for bad in [
        json!(9007199254740993u64),
        json!(null),
        json!(1.5),
        json!(""),
        json!("01"),
        json!("-1"),
        json!("+1"),
        json!("1e3"),
        json!("18446744073709551616"),
    ] {
        assert!(serde_json::from_value::<Decimal>(bad).is_err());
    }
}

#[test]
fn nil_ambiguous_uuid_forms_and_nonhex_hashes_are_rejected() {
    for bad in [
        "00000000-0000-0000-0000-000000000000",
        "11111111111141118111111111111111",
        "AAAAAAAA-AAAA-4AAA-8AAA-AAAAAAAAAAAA",
        "../../file",
    ] {
        assert!(serde_json::from_value::<Id>(json!(bad)).is_err());
    }
    for bad in ["A".repeat(64), "g".repeat(64), "a".repeat(63), "".into()] {
        assert!(serde_json::from_value::<Hash>(json!(bad)).is_err());
    }
}

#[test]
fn unknown_versions_fields_duplicates_and_trailing_documents_are_rejected() {
    for path in ["version", "target", "file", "scope", "resource", "chunk"] {
        let mut value = manifest();
        match path {
            "version" => value["format"] = json!("piep-resource-transfer-v2"),
            "target" => value["target"]["token"] = json!("forbidden"),
            "file" => value["file"]["path"] = json!("C:/private"),
            "scope" => value["target"]["scope"]["other"] = json!(1),
            "resource" => value["target"]["resource"]["other"] = json!(1),
            "chunk" => value["file"]["chunks"][0]["codec"] = json!("zstd"),
            _ => unreachable!(),
        }
        rejected(&value);
    }
    let duplicate = String::from_utf8(GOLDEN.to_vec()).unwrap().replace(
        "\"format\":",
        "\"format\":\"piep-resource-transfer-v1\",\"format\":",
    );
    assert!(ValidatedTransfer::parse(duplicate.as_bytes()).is_err());
    let duplicate_kind = String::from_utf8(GOLDEN.to_vec())
        .unwrap()
        .replace("\"kind\":\"media\"", "\"kind\":\"body\",\"kind\":\"media\"");
    assert!(ValidatedTransfer::parse(duplicate_kind.as_bytes()).is_err());
    let mut trailing = GOLDEN.to_vec();
    trailing.extend_from_slice(b"{}");
    assert!(ValidatedTransfer::parse(&trailing).is_err());
}

#[test]
fn native_chunk_layout_is_shared_and_large_files_keep_exact_boundaries() {
    let mut value = manifest();
    value["file"]["length"] = json!((8 * 1024 * 1024 + 1).to_string());
    let hash = value["file"]["id"].clone();
    value["file"]["chunks"] = json!([
        {"id":hash,"length":(4*1024*1024).to_string()},
        {"id":hash,"length":(4*1024*1024).to_string()},
        {"id":hash,"length":"1"},
    ]);
    let valid = serde_json::to_vec(&value).unwrap();
    assert!(ValidatedTransfer::parse(&valid).is_ok());
    value["file"]["chunks"][0]["length"] = json!((4 * 1024 * 1024 - 1).to_string());
    value["file"]["chunks"][2]["length"] = json!("2");
    rejected(&value); // 合計が同じでも分割位置が違えば失敗。
    let mut value = manifest();
    value["file"]["chunks"] = json!([]);
    rejected(&value);
    let mut value = manifest();
    value["file"]["chunks"][0]["length"] = json!("14");
    rejected(&value);
    let mut value = manifest();
    value["file"]["length"] = json!((512 * 1024 * 1024u64 + 1).to_string());
    rejected(&value);
}

#[test]
fn chunk_list_is_bounded_during_deserialization_and_input_is_bounded_before_parsing() {
    let mut value = manifest();
    let chunk = value["file"]["chunks"][0].clone();
    value["file"]["chunks"] = Value::Array(vec![chunk; 129]);
    let error = match ValidatedTransfer::parse(&serde_json::to_vec(&value).unwrap()) {
        Ok(_) => panic!("too many chunks"),
        Err(e) => e.to_string(),
    };
    assert!(error.contains("個数"));
    assert!(ValidatedTransfer::parse(&vec![b' '; MAX_MANIFEST_BYTES + 1]).is_err());
}

#[test]
fn raw_manifest_identity_is_not_changed_by_parsing_and_does_not_ignore_whitespace() {
    let first = ValidatedTransfer::parse(GOLDEN).unwrap();
    let mut bytes = GOLDEN.to_vec();
    bytes.push(b'\n');
    let second = ValidatedTransfer::parse(&bytes).unwrap();
    assert_eq!(first.manifest(), second.manifest());
    assert_ne!(first.id(), second.id());
    assert_eq!(second.bytes(), bytes);
}

#[test]
fn body_limits_are_separate_from_streamable_image_and_attachment_limits() {
    let mut value = manifest();
    value["file"]["length"] = json!((16 * 1024 * 1024 + 1).to_string());
    let id = value["file"]["id"].clone();
    value["file"]["chunks"] = json!([
        {"id":id,"length":"4194304"},{"id":id,"length":"4194304"},
        {"id":id,"length":"4194304"},{"id":id,"length":"4194304"},
        {"id":id,"length":"1"}
    ]);
    assert!(ValidatedTransfer::parse(&serde_json::to_vec(&value).unwrap()).is_ok());
    value["target"]["resource"] = json!({"kind":"body"});
    rejected(&value);
    value["target"]["resource"] = json!({"kind":"file","slot":"attachment"});
    assert!(ValidatedTransfer::parse(&serde_json::to_vec(&value).unwrap()).is_ok());
}

#[test]
fn empty_original_file_is_valid_but_resource_identifiers_must_be_nonempty_and_bounded() {
    let mut value = manifest();
    let empty = Fingerprint::of(b"").to_string();
    value["file"] = json!({"id":empty,"length":"0","chunks":[{"id":empty,"length":"0"}]});
    assert!(ValidatedTransfer::parse(&serde_json::to_vec(&value).unwrap()).is_ok());
    let transfer = ValidatedTransfer::parse(GOLDEN).unwrap();
    for slot in ["".to_owned(), "line\nbreak".into(), "x".repeat(4097)] {
        let mut target = transfer.manifest().target.clone();
        target.resource = ResourceReference::File { slot };
        assert!(TransferManifest::new(target, transfer.file()).is_err());
    }
}
