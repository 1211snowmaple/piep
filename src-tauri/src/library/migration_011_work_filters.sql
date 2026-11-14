-- 一覧の絞り込み。作者・シリーズ・タグから作品を引く逆向きの索引と、作品ごとの更新の監視、
-- 取得元の候補の判断を旧棚と同じ三つ（未判断・保留・非表示）にして保留の理由を引けるようにする。
CREATE INDEX snapshot_people_person ON snapshot_people(person_id);
CREATE INDEX snapshot_people_name ON snapshot_people(display_name);
CREATE INDEX snapshot_series_series ON snapshot_series(series_id);
CREATE INDEX snapshot_tags_tag ON snapshot_tags(tag_id);
CREATE INDEX tags_name ON tags(name);
-- 旧棚の「修復」の版（同じ版を取り直して欠けを埋めたもの）から分けた版。作者の改稿には数えない。
ALTER TABLE source_revisions ADD COLUMN repair INTEGER NOT NULL DEFAULT 0 CHECK(repair IN (0,1));
-- 作品の更新を見張るかは、お気に入りと同じく利用者ごとの状態。
ALTER TABLE work_user_state ADD COLUMN watching INTEGER NOT NULL DEFAULT 0 CHECK(watching IN (0,1));
ALTER TABLE work_current_projection ADD COLUMN watching INTEGER NOT NULL DEFAULT 0 CHECK(watching IN (0,1));
-- 旧棚の候補の状態は pending・held・dismissed の三つ。保留の理由（holdKind）は原本の JSON に
-- 埋まっていて SQL で絞れないので列に出す。原本はそのまま残す。
CREATE TABLE discovery_candidates_next (
    id BLOB PRIMARY KEY CHECK(length(id)=16),
    work_id BLOB NOT NULL REFERENCES works(id),
    kind TEXT NOT NULL CHECK(kind IN ('new','revision')),
    title TEXT NOT NULL,
    decision TEXT NOT NULL CHECK(decision IN ('pending','held','dismissed')),
    hold_kind TEXT CHECK(hold_kind IS NULL OR (decision='held' AND hold_kind<>'')),
    payload_file_id BLOB NOT NULL REFERENCES content_files(id),
    target_type TEXT,
    first_seen_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE(work_id,kind)
) STRICT;
INSERT INTO discovery_candidates_next(id,work_id,kind,title,decision,payload_file_id,target_type,first_seen_at,updated_at)
SELECT id,work_id,kind,title,decision,payload_file_id,target_type,first_seen_at,updated_at FROM discovery_candidates;
DROP INDEX discovery_decision;
DROP TABLE discovery_candidates;
ALTER TABLE discovery_candidates_next RENAME TO discovery_candidates;
CREATE INDEX discovery_decision ON discovery_candidates(decision,updated_at,id);
