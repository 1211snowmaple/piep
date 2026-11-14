use piep_library_protocol::{Id, LibraryIdentity, MAX_IDENTITY_BYTES};
use serde_json::json;
fn id(value: &str) -> Id {
    Id::new(uuid::Uuid::parse_str(value).unwrap()).unwrap()
}
fn identity() -> LibraryIdentity {
    LibraryIdentity::new(
        id("11111111-1111-4111-8111-111111111111"),
        id("22222222-2222-4222-8222-222222222222"),
        id("33333333-3333-4333-8333-333333333333"),
    )
}
#[test]
fn authenticated_identity_keeps_library_subject_epoch_and_explicit_readiness() {
    let known = identity();
    let encoded = serde_json::to_vec(&known).unwrap();
    assert_eq!(LibraryIdentity::parse(&encoded).unwrap(), known);
    assert_ne!(known.library_id, known.subject_id);
    assert!(known.capabilities.resource_transfer);
    assert!(!known.capabilities.reader_bundle && !known.capabilities.mutations);
    let value: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(value["maxFileBytes"], "536870912");
    assert_eq!(value["maxBodyBytes"], "16777216");
}
#[test]
fn old_about_version_two_is_not_a_new_library_handshake() {
    assert!(LibraryIdentity::parse(br#"{"apiVersion":2,"name":"piep"}"#).is_err());
}
#[test]
fn unknown_formats_excessive_limits_and_unknown_fields_are_rejected() {
    for field in [
        "format",
        "apiVersion",
        "storageVersion",
        "transferFormat",
        "maxFileBytes",
        "maxBodyBytes",
        "maxManifestBytes",
        "unknown",
    ] {
        let mut value = serde_json::to_value(identity()).unwrap();
        value[field] = match field {
            "apiVersion" | "storageVersion" => json!(999),
            "format" | "transferFormat" => json!("unknown"),
            "unknown" => json!(true),
            _ => json!("18446744073709551615"),
        };
        assert!(LibraryIdentity::parse(&serde_json::to_vec(&value).unwrap()).is_err());
    }
}
#[test]
fn empty_and_oversized_identity_are_rejected_without_fallback() {
    assert!(LibraryIdentity::parse(b"").is_err());
    assert!(LibraryIdentity::parse(&vec![b' '; MAX_IDENTITY_BYTES + 1]).is_err());
    let mut value = serde_json::to_value(identity()).unwrap();
    value["maxFileBytes"] = json!("0");
    assert!(LibraryIdentity::parse(&serde_json::to_vec(&value).unwrap()).is_err());
}
