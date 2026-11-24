-- 補完を試した記録を、取得元によらない一つの表にまとめる。pixiv の一部保存の補完の印
-- （incomplete_save_attempts）と FANBOX の資産の取り直しの状態（asset_repair_state）を移す。
-- 記録は今の版でしか効かないので、今の版を指す行だけを移す。
CREATE TABLE repair_attempts (
    work_id BLOB PRIMARY KEY REFERENCES works(id),
    snapshot_id BLOB NOT NULL,
    gap_fingerprint BLOB NOT NULL CHECK(length(gap_fingerprint)=32),
    attempts INTEGER NOT NULL CHECK(attempts BETWEEN 0 AND 4294967295),
    request_id BLOB,
    attempted_at INTEGER NOT NULL,
    FOREIGN KEY(snapshot_id,work_id) REFERENCES revision_snapshots(id,entity_id),
    FOREIGN KEY(request_id,work_id) REFERENCES acquisition_attempts(id,entity_id)
) STRICT;
INSERT INTO repair_attempts(work_id,snapshot_id,gap_fingerprint,attempts,request_id,attempted_at)
SELECT s.work_id,s.snapshot_id,s.state_fingerprint,s.attempts,s.request_id,coalesce(a.finished_at,a.started_at)
  FROM asset_repair_state s
  JOIN entity_current c ON c.entity_id=s.work_id AND c.snapshot_id=s.snapshot_id
  JOIN acquisition_attempts a ON a.id=s.request_id;
-- pixiv の印には欠けの指紋が無い。分からない指紋（0 を並べたもの）とし、回数は欠けた版を作った
-- 保存と、試した補完の 2 回とする。上限に届いているので、同じ版ではもう取り直さない。
INSERT OR IGNORE INTO repair_attempts(work_id,snapshot_id,gap_fingerprint,attempts,request_id,attempted_at)
SELECT i.work_id,i.snapshot_id,zeroblob(32),2,NULL,i.attempted_at
  FROM incomplete_save_attempts i
  JOIN entity_current c ON c.entity_id=i.work_id AND c.snapshot_id=i.snapshot_id;
DROP TABLE incomplete_save_attempts;
DROP TABLE asset_repair_state;
