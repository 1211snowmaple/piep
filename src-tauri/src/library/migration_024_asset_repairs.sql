CREATE UNIQUE INDEX acquisition_attempt_entity ON acquisition_attempts(id,entity_id);
CREATE TABLE asset_repair_state (
    work_id BLOB PRIMARY KEY REFERENCES works(id),
    snapshot_id BLOB NOT NULL,
    state_fingerprint BLOB NOT NULL CHECK(length(state_fingerprint)=32),
    attempts INTEGER NOT NULL CHECK(attempts BETWEEN 0 AND 4294967295),
    request_id BLOB,
    legacy_evidence_file_id BLOB REFERENCES content_files(id),
    FOREIGN KEY(snapshot_id,work_id) REFERENCES revision_snapshots(id,entity_id),
    FOREIGN KEY(request_id,work_id) REFERENCES acquisition_attempts(id,entity_id),
    CHECK(request_id IS NOT NULL OR legacy_evidence_file_id IS NOT NULL)
) STRICT;
