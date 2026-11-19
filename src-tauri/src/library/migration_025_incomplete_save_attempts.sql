-- 一部保存の補完を今の版で試した印。旧棚から移した印（repair_attempt_markers）は原記録として
-- 変えず、切り替えた後の試行はここへ書く。現在の写しが進めば印は効かなくなる。
CREATE TABLE incomplete_save_attempts (
    work_id BLOB PRIMARY KEY REFERENCES works(id),
    snapshot_id BLOB NOT NULL,
    attempted_at INTEGER NOT NULL,
    FOREIGN KEY(snapshot_id,work_id) REFERENCES revision_snapshots(id,entity_id)
) STRICT;
