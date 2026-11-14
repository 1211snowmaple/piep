use super::*;
use crate::library::{CapturedAttachment, CapturedMedia};
use serde_json::json;
use std::collections::BTreeMap;
use std::path::PathBuf;

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("piep-acquisition-{}", LibraryId::now_v7())))
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        assert_eq!(self.0.parent(), Some(std::env::temp_dir().as_path()));
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn begin(library: &Library, key: &str) -> AcquisitionTicket {
    library
        .begin_acquisition(
            LibraryId::now_v7(),
            "pixiv",
            EntityKind::Work,
            key,
            "offline-test",
        )
        .unwrap()
        .ticket
}
fn capture(ticket: &AcquisitionTicket, text: &str) -> Capture {
    Capture {
        request_id: ticket.id,
        acquired_at: Some(1234),
        provider: ticket.provider.clone(),
        external_key: ticket.external_key.clone(),
        kind: ticket.kind,
        route: ticket.route.clone(),
        raw: format!("{{\"text\":{}}}\r\n", json!(text))
            .into_bytes()
            .into(),
        metadata: BTreeMap::from([("title".into(), json!(text))]),
        field_states: BTreeMap::new(),
        body: Some(text.into()),
        media: vec![],
        attachments: vec![],
    }
}
fn finish(library: &Library, ticket: &AcquisitionTicket, text: &str) -> CaptureResult {
    library
        .complete_acquisition(ticket.id, capture(ticket, text))
        .unwrap()
}
fn restore(library: &Library, file: Fingerprint) -> Vec<u8> {
    let mut bytes = Vec::new();
    library
        .objects
        .export(&library.file(file).unwrap(), &mut bytes)
        .unwrap();
    bytes
}

#[test]
fn latest_begin_wins_in_both_completion_orders_and_stale_inputs_are_retained() {
    for new_finishes_first in [false, true] {
        let temp = Temp::new();
        let library = Library::create(&temp.0).unwrap();
        let initial = begin(&library, "one");
        let initial = finish(&library, &initial, "initial");
        let old = begin(&library, "one");
        let new = begin(&library, "one");
        let part = library
            .record_acquisition_payload(
                old.id,
                "detail",
                &b"exact upstream bytes\r\n".to_vec().into(),
            )
            .unwrap();
        let mut stale = capture(&old, "stale");
        stale.media.push(CapturedMedia {
            slot: "cover".into(),
            role: "cover".into(),
            source_key: Some("image-source".into()),
            source_url: Some("https://example.invalid/original.png".into()),
            quality: "original".into(),
            original_name: "cover.png".into(),
            mime_type: "image/png".into(),
            bytes: Some(b"image original".to_vec().into()),
        });
        stale.attachments.push(CapturedAttachment {
            slot: "attachment".into(),
            original_name: "file.bin".into(),
            mime_type: None,
            bytes: b"attachment bytes".to_vec().into(),
        });
        let winner = if new_finishes_first {
            Some(finish(&library, &new, "new"))
        } else {
            None
        };
        assert!(matches!(
            library.complete_acquisition(old.id, stale),
            Err(Error::Superseded)
        ));
        assert_eq!(
            library.acquisition_status(old.id).unwrap().state,
            AcquisitionState::Superseded
        );
        assert_eq!(
            library.current_snapshot(initial.entity_id).unwrap(),
            winner
                .as_ref()
                .map_or(initial.snapshot_id, |r| r.snapshot_id)
        );
        let winner = winner.unwrap_or_else(|| finish(&library, &new, "new"));
        assert_eq!(
            library.current_snapshot(initial.entity_id).unwrap(),
            winner.snapshot_id
        );
        assert_eq!(library.snapshots(initial.entity_id).unwrap().len(), 2);
        assert_eq!(restore(&library, part), b"exact upstream bytes\r\n");
        let records = {
            let conn = library.conn.lock().unwrap();
            inputs(&conn, old.id).unwrap()
        };
        assert_eq!(records.len(), 6);
        for ((role, _), file) in records {
            let bytes = restore(&library, file);
            match role.as_str() {
                "raw" => assert_eq!(bytes, b"{\"text\":\"stale\"}\r\n"),
                "body" => assert_eq!(bytes, b"stale"),
                "media" => assert_eq!(bytes, b"image original"),
                "attachment" => assert_eq!(bytes, b"attachment bytes"),
                "raw_part" => assert_eq!(bytes, b"exact upstream bytes\r\n"),
                "request" => assert_eq!(
                    serde_json::from_slice::<AcquisitionManifest>(&bytes)
                        .unwrap()
                        .ticket,
                    old
                ),
                _ => panic!("unexpected input"),
            }
        }
        library.verify().unwrap();
    }
}

#[test]
fn acquisition_replay_is_exact_and_never_rewinds_the_current_snapshot() {
    let temp = Temp::new();
    let library = Library::create(&temp.0).unwrap();
    let first = begin(&library, "one");
    let same = library
        .begin_acquisition(first.id, "pixiv", EntityKind::Work, "one", "offline-test")
        .unwrap();
    assert_eq!(same.ticket, first);
    assert!(library
        .begin_acquisition(first.id, "pixiv", EntityKind::Work, "other", "offline-test")
        .is_err());
    let result = finish(&library, &first, "first");
    let second = begin(&library, "one");
    let current = finish(&library, &second, "second");
    let replay = finish(&library, &first, "first");
    assert_eq!(replay.snapshot_id, result.snapshot_id);
    assert_eq!(
        library.current_snapshot(result.entity_id).unwrap(),
        current.snapshot_id
    );
    assert!(matches!(
        library.complete_acquisition(first.id, capture(&first, "changed")),
        Err(Error::Integrity(_))
    ));
    let info = library
        .begin_acquisition(first.id, "pixiv", EntityKind::Work, "one", "offline-test")
        .unwrap();
    assert_eq!(info.state, AcquisitionState::Complete);
    assert_eq!(info.ticket.epoch, 1);
    library.verify().unwrap();
}

#[test]
fn unrelated_captures_and_user_operations_do_not_invalidate_a_ticket() {
    let temp = Temp::new();
    let library = Library::create(&temp.0).unwrap();
    let initial = begin(&library, "one");
    let result = finish(&library, &initial, "initial");
    let pending = begin(&library, "one");
    let other = begin(&library, "other");
    finish(&library, &other, "other");
    library
        .set_favorite(
            LibraryId::now_v7(),
            result.entity_id,
            LibraryId::now_v7(),
            0,
            true,
        )
        .unwrap();
    let result = finish(&library, &pending, "updated");
    assert!(result.created_revision);
    library.verify().unwrap();
}

#[test]
fn out_of_band_current_changes_are_fenced_and_reserved_requests_cannot_bypass_completion() {
    let temp = Temp::new();
    let library = Library::create(&temp.0).unwrap();
    let pending = begin(&library, "one");
    assert!(matches!(
        library.capture(capture(&pending, "bypass")),
        Err(Error::Integrity(_))
    ));
    let mut offline = capture(&pending, "offline import");
    offline.request_id = LibraryId::now_v7();
    let offline = library.capture(offline).unwrap();
    assert!(matches!(
        library.complete_acquisition(pending.id, capture(&pending, "old")),
        Err(Error::Superseded)
    ));
    assert_eq!(
        library.current_snapshot(pending.entity).unwrap(),
        offline.snapshot_id
    );
    library.verify().unwrap();
}

#[test]
fn cancelled_and_pending_payloads_survive_reopen_and_independent_backup() {
    let temp = Temp::new();
    let backup = Temp::new();
    let library = Library::create(&temp.0).unwrap();
    let cancelled = begin(&library, "cancel");
    let raw = b"raw with an unsupported body\0\r\n".to_vec();
    let file = library
        .record_acquisition_payload(cancelled.id, "response", &raw.clone().into())
        .unwrap();
    assert_eq!(
        library
            .record_acquisition_payload(cancelled.id, "response", &raw.clone().into())
            .unwrap(),
        file
    );
    assert!(library
        .record_acquisition_payload(cancelled.id, "response", &b"changed".to_vec().into())
        .is_err());
    assert_eq!(
        library.cancel_acquisition(cancelled.id).unwrap().state,
        AcquisitionState::Cancelled
    );
    assert_eq!(
        library.cancel_acquisition(cancelled.id).unwrap().state,
        AcquisitionState::Cancelled
    );
    assert_eq!(
        library
            .record_acquisition_payload(cancelled.id, "response", &raw.clone().into())
            .unwrap(),
        file
    );
    assert!(library
        .record_acquisition_payload(cancelled.id, "new", &raw.clone().into())
        .is_err());
    assert!(matches!(
        library.complete_acquisition(cancelled.id, capture(&cancelled, "cancelled")),
        Err(Error::Canceled)
    ));
    let pending = begin(&library, "pending");
    library
        .record_acquisition_payload(pending.id, "response", &raw.clone().into())
        .unwrap();
    assert!(library.works(10).unwrap().is_empty());
    library.backup(&backup.0).unwrap();
    drop(library);
    let reopened = Library::open(&temp.0).unwrap();
    assert_eq!(
        reopened.acquisition_status(pending.id).unwrap().state,
        AcquisitionState::Pending
    );
    assert_eq!(
        reopened.acquisition_status(pending.id).unwrap().payloads,
        vec![AcquisitionPayload {
            slot: "response".into(),
            file
        }]
    );
    drop(reopened);
    drop(temp); // バックアップの検証に元ルートを使えない状態にする。
    let restored = Library::open(&backup.0).unwrap();
    assert_eq!(restore(&restored, file), raw);
    assert_eq!(
        restored.acquisition_status(cancelled.id).unwrap().state,
        AcquisitionState::Cancelled
    );
    finish(&restored, &pending, "resumed");
    restored.verify().unwrap();
}

#[test]
fn closed_records_are_immutable_and_tampered_file_references_are_detected() {
    let temp = Temp::new();
    let library = Library::create(&temp.0).unwrap();
    let ticket = begin(&library, "one");
    library
        .record_acquisition_payload(ticket.id, "response", &b"upstream bytes".to_vec().into())
        .unwrap();
    finish(&library, &ticket, "saved");
    let replacement = library
        .store_original(&b"different valid file".to_vec().into())
        .unwrap();
    {
        let conn = library.conn.lock().unwrap();
        assert!(conn
            .execute(
                "UPDATE acquisition_attempts SET route='different' WHERE id=?1",
                [ticket.id.as_bytes().as_slice()]
            )
            .is_err());
        assert!(conn
            .execute(
                "DELETE FROM acquisition_inputs WHERE attempt_id=?1",
                [ticket.id.as_bytes().as_slice()]
            )
            .is_err());
        assert!(conn
            .execute("UPDATE acquisition_heads SET epoch=epoch-1", [])
            .is_err());
        conn.execute_batch("DROP TRIGGER acquisition_input_immutable")
            .unwrap();
        conn.execute(
            "UPDATE acquisition_inputs SET file_id=?2 WHERE attempt_id=?1 AND role='raw_part'",
            params![ticket.id.as_bytes().as_slice(), replacement.id.0.as_slice()],
        )
        .unwrap();
    }
    assert!(matches!(library.verify(), Err(Error::Integrity(_))));
}

#[test]
fn a_failure_at_publication_rolls_back_the_snapshot_inputs_and_attempt_together() {
    let temp = Temp::new();
    let library = Library::create(&temp.0).unwrap();
    let first = begin(&library, "one");
    let first = finish(&library, &first, "initial");
    let pending = begin(&library, "one");
    let payload = library
        .record_acquisition_payload(
            pending.id,
            "response",
            &b"raw retained before parsing".to_vec().into(),
        )
        .unwrap();
    library.conn.lock().unwrap().execute_batch("CREATE TRIGGER refuse_capture BEFORE INSERT ON outbox WHEN NEW.operation='capture' BEGIN SELECT RAISE(ABORT,'injected publication failure'); END;").unwrap();
    assert!(library
        .complete_acquisition(pending.id, capture(&pending, "new"))
        .is_err());
    assert_eq!(
        library.acquisition_status(pending.id).unwrap().state,
        AcquisitionState::Pending
    );
    assert_eq!(
        library.current_snapshot(first.entity_id).unwrap(),
        first.snapshot_id
    );
    assert_eq!(library.snapshots(first.entity_id).unwrap().len(), 1);
    let records = {
        let conn = library.conn.lock().unwrap();
        inputs(&conn, pending.id).unwrap()
    };
    assert_eq!(records.len(), 1);
    assert_eq!(restore(&library, payload), b"raw retained before parsing");
    library.verify().unwrap();
    library
        .conn
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER refuse_capture")
        .unwrap();
    finish(&library, &pending, "new");
    assert_eq!(library.snapshots(first.entity_id).unwrap().len(), 2);
    library.verify().unwrap();
}

#[test]
fn completion_cannot_change_the_ticket_scope_or_append_to_a_closed_attempt() {
    let temp = Temp::new();
    let library = Library::create(&temp.0).unwrap();
    let old = begin(&library, "one");
    let new = begin(&library, "one");
    let mut wrong = capture(&new, "wrong scope");
    wrong.external_key = "other".into();
    assert!(matches!(
        library.complete_acquisition(new.id, wrong),
        Err(Error::Integrity(_))
    ));
    assert!(matches!(
        library.complete_acquisition(old.id, capture(&old, "old")),
        Err(Error::Superseded)
    ));
    assert!(matches!(
        library.complete_acquisition(old.id, capture(&old, "old")),
        Err(Error::Superseded)
    ));
    assert!(matches!(
        library.complete_acquisition(old.id, capture(&old, "different")),
        Err(Error::Integrity(_))
    ));
    assert_eq!(
        library.cancel_acquisition(old.id).unwrap().state,
        AcquisitionState::Superseded
    );
    assert!(library
        .record_acquisition_payload(old.id, "late", &b"late".to_vec().into())
        .is_err());
    finish(&library, &new, "new");
    assert_eq!(library.snapshots(new.entity).unwrap().len(), 1);
    library.verify().unwrap();
}

#[test]
fn the_database_rejects_a_completion_borrowed_from_another_request_of_the_same_work() {
    let temp = Temp::new();
    let library = Library::create(&temp.0).unwrap();
    let first = begin(&library, "one");
    finish(&library, &first, "first");
    let second = begin(&library, "one");
    {
        let conn = library.conn.lock().unwrap();
        assert!(conn.execute("UPDATE acquisition_attempts SET state='complete',input_manifest_file_id=(SELECT request_hash FROM acquisition_observations WHERE request_id=?2),observation_id=(SELECT id FROM acquisition_observations WHERE request_id=?2),result_snapshot_id=(SELECT result_snapshot_id FROM acquisition_observations WHERE request_id=?2),finished_at=1 WHERE id=?1",params![second.id.as_bytes().as_slice(),first.id.as_bytes().as_slice()]).is_err());
    }
    assert_eq!(
        library.acquisition_status(second.id).unwrap().state,
        AcquisitionState::Pending
    );
    finish(&library, &second, "second");
    library.verify().unwrap();
}

#[test]
fn advanced_epoch_without_a_matching_attempt_is_not_a_valid_ledger() {
    let temp = Temp::new();
    let library = Library::create(&temp.0).unwrap();
    begin(&library, "one");
    library
        .conn
        .lock()
        .unwrap()
        .execute("UPDATE acquisition_heads SET epoch=epoch+1", [])
        .unwrap();
    assert!(matches!(library.verify(), Err(Error::Integrity(_))));
}
