-- 旧棚から移したときの原記録と保守の印を落とす。棚はもう旧棚と突き合わせない。
-- 補完を試した印のうち今の版のものだけ、今の印へ移す。同じ版で補完を繰り返さないため。
INSERT OR IGNORE INTO incomplete_save_attempts(work_id,snapshot_id,attempted_at)
SELECT m.entity_id,m.snapshot_id,max(m.attempted_at) FROM repair_attempt_markers m
  JOIN work_current_projection p ON p.work_id=m.entity_id AND p.snapshot_id=m.snapshot_id
 GROUP BY m.entity_id,m.snapshot_id;
DROP TABLE repair_attempt_markers;
DROP TABLE asset_audit_markers;
DROP TABLE migration_fingerprints;
DROP TABLE migration_requests;
DROP TABLE migration_records;
-- 添付の取り直しの覚え書きは、取得の要求から作ったものだけになる。
CREATE TABLE asset_repair_state_next (
    work_id BLOB PRIMARY KEY REFERENCES works(id),
    snapshot_id BLOB NOT NULL,
    state_fingerprint BLOB NOT NULL CHECK(length(state_fingerprint)=32),
    attempts INTEGER NOT NULL CHECK(attempts BETWEEN 0 AND 4294967295),
    request_id BLOB NOT NULL,
    FOREIGN KEY(snapshot_id,work_id) REFERENCES revision_snapshots(id,entity_id),
    FOREIGN KEY(request_id,work_id) REFERENCES acquisition_attempts(id,entity_id)
) STRICT;
INSERT INTO asset_repair_state_next(work_id,snapshot_id,state_fingerprint,attempts,request_id)
SELECT work_id,snapshot_id,state_fingerprint,attempts,request_id FROM asset_repair_state
 WHERE request_id IS NOT NULL;
DROP TABLE asset_repair_state;
ALTER TABLE asset_repair_state_next RENAME TO asset_repair_state;
-- 版の区分（取得元の改稿か、区分の分からない移した版か）をやめ、どの版も取得元の版として扱う。
-- 区分は保存状態の指紋に入っているので、移した版の指紋はこの後で計算し直す（schema.rs）。
-- その間だけ、公開済みの保存状態と読む形の見出しを書き換えられるようにする。
CREATE TEMP TABLE unclassified_snapshots(id BLOB PRIMARY KEY) STRICT;
INSERT INTO temp.unclassified_snapshots
SELECT s.id FROM revision_snapshots s JOIN source_revisions r ON r.id=s.revision_id
 WHERE r.classification<>'source' AND s.published=1;
ALTER TABLE source_revisions DROP COLUMN classification;
DROP TRIGGER published_snapshot_immutable;
DROP TRIGGER reader_binding_header_update;
