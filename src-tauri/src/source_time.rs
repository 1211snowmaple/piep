/// Provider timestamps may describe the same instant with different offsets or precision.
pub(crate) fn same_instant(left: Option<&str>, right: Option<&str>) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => match (
            chrono::DateTime::parse_from_rfc3339(left),
            chrono::DateTime::parse_from_rfc3339(right),
        ) {
            (Ok(left), Ok(right)) => left == right,
            _ => false,
        },
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::same_instant;

    #[test]
    fn offsets_and_fractional_precision_do_not_create_revisions() {
        assert!(same_instant(
            Some("2026-02-18T23:41:56+09:00"),
            Some("2026-02-18T14:41:56.000Z")
        ));
        assert!(!same_instant(
            Some("2026-02-18T14:41:56Z"),
            Some("2026-02-18T14:41:57Z")
        ));
        assert!(!same_instant(Some("invalid"), Some("invalid")));
        assert!(!same_instant(None, Some("2026-02-18T14:41:56Z")));
    }
}
