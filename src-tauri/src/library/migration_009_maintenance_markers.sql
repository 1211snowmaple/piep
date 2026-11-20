-- 旧棚の保守記録。検査結果や修復成功とは解釈せず、対象の保存版に固定する。
CREATE TABLE repair_attempt_markers (
    id BLOB PRIMARY KEY CHECK(length(id)=16),
    entity_id BLOB NOT NULL REFERENCES works(id),
    snapshot_id BLOB NOT NULL,
    published INTEGER NOT NULL DEFAULT 1 CHECK(published=1),
    origin TEXT NOT NULL CHECK(origin='legacy-incomplete-save-v1'),
    attempted_at INTEGER NOT NULL,
    evidence_file_id BLOB NOT NULL REFERENCES content_files(id),
    UNIQUE(entity_id,snapshot_id,origin),
    FOREIGN KEY(entity_id,snapshot_id,published) REFERENCES revision_snapshots(entity_id,id,published)
) STRICT;
CREATE TABLE asset_audit_markers (
    id BLOB PRIMARY KEY CHECK(length(id)=16),
    entity_id BLOB NOT NULL REFERENCES works(id),
    snapshot_id BLOB NOT NULL,
    published INTEGER NOT NULL DEFAULT 1 CHECK(published=1),
    origin TEXT NOT NULL CHECK(origin='legacy-saved-assets-v1'),
    legacy_rule INTEGER NOT NULL CHECK(legacy_rule>0),
    evidence_file_id BLOB NOT NULL REFERENCES content_files(id),
    UNIQUE(entity_id,snapshot_id,origin,legacy_rule),
    FOREIGN KEY(entity_id,snapshot_id,published) REFERENCES revision_snapshots(entity_id,id,published)
) STRICT;
CREATE TRIGGER repair_attempt_marker_update BEFORE UPDATE ON repair_attempt_markers
BEGIN SELECT RAISE(ABORT,'repair attempt markers are immutable'); END;
CREATE TRIGGER repair_attempt_marker_delete BEFORE DELETE ON repair_attempt_markers
BEGIN SELECT RAISE(ABORT,'repair attempt markers are immutable'); END;
CREATE TRIGGER asset_audit_marker_update BEFORE UPDATE ON asset_audit_markers
BEGIN SELECT RAISE(ABORT,'asset audit markers are immutable'); END;
CREATE TRIGGER asset_audit_marker_delete BEFORE DELETE ON asset_audit_markers
BEGIN SELECT RAISE(ABORT,'asset audit markers are immutable'); END;
