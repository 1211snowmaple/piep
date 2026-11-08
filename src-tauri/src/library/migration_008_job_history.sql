-- 旧更新処理の履歴。新しい実行キューへ投入せず、当時の結果を不変に保つ。
CREATE TABLE job_history (
    id BLOB PRIMARY KEY CHECK(length(id)=16),
    origin TEXT NOT NULL CHECK(origin='legacy-update-v1'),
    scope TEXT NOT NULL,
    mode TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status='completed'),
    watch_saved INTEGER NOT NULL CHECK(watch_saved IN (0,1)),
    new_works_only INTEGER NOT NULL CHECK(new_works_only IN (0,1)),
    totals INTEGER NOT NULL CHECK(totals>=0),
    processed INTEGER NOT NULL CHECK(processed>=0),
    candidate_count INTEGER NOT NULL CHECK(candidate_count>=0),
    saved_count INTEGER NOT NULL CHECK(saved_count>=0),
    error_count INTEGER NOT NULL CHECK(error_count>=0),
    active_label TEXT,
    started_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    finished_at INTEGER,
    sealed INTEGER NOT NULL DEFAULT 0 CHECK(sealed IN (0,1))
) STRICT;
CREATE INDEX job_history_started ON job_history(started_at DESC,id DESC);
CREATE TABLE job_history_items (
    id BLOB PRIMARY KEY CHECK(length(id)=16),
    job_id BLOB NOT NULL REFERENCES job_history(id),
    sequence INTEGER NOT NULL,
    item_type TEXT NOT NULL CHECK(item_type IN ('work','target','candidate')),
    entity_id BLOB REFERENCES entities(id),
    target_type TEXT,
    title TEXT NOT NULL,
    payload_file_id BLOB NOT NULL REFERENCES content_files(id),
    status TEXT NOT NULL CHECK(status IN ('done','saved','skipped','held','failed')),
    error TEXT,
    result_work_id BLOB REFERENCES works(id),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE(job_id,sequence),
    CHECK(item_type<>'target' OR (target_type IS NOT NULL AND target_type IN ('author','creator','series','work')))
) STRICT;
CREATE TRIGGER job_history_item_scope BEFORE INSERT ON job_history_items
WHEN (NEW.entity_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM entities e WHERE e.id=NEW.entity_id AND e.kind=CASE
    WHEN NEW.item_type IN ('work','candidate') THEN 'work'
    WHEN NEW.target_type IN ('author','creator') THEN 'person'
    ELSE NEW.target_type END))
 OR (NEW.item_type IN ('work','candidate') AND NEW.result_work_id IS NOT NULL AND NEW.result_work_id IS NOT NEW.entity_id)
BEGIN SELECT RAISE(ABORT,'job history item scope is inconsistent'); END;
CREATE TRIGGER job_history_item_scope_update BEFORE UPDATE ON job_history_items
WHEN (NEW.entity_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM entities e WHERE e.id=NEW.entity_id AND e.kind=CASE
    WHEN NEW.item_type IN ('work','candidate') THEN 'work'
    WHEN NEW.target_type IN ('author','creator') THEN 'person'
    ELSE NEW.target_type END))
 OR (NEW.item_type IN ('work','candidate') AND NEW.result_work_id IS NOT NULL AND NEW.result_work_id IS NOT NEW.entity_id)
BEGIN SELECT RAISE(ABORT,'job history item scope is inconsistent'); END;
CREATE TABLE job_history_logs (
    id BLOB PRIMARY KEY CHECK(length(id)=16),
    job_id BLOB NOT NULL REFERENCES job_history(id),
    sequence INTEGER NOT NULL,
    log_type TEXT NOT NULL,
    message TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    UNIQUE(job_id,sequence)
) STRICT;
CREATE TRIGGER job_history_insert BEFORE INSERT ON job_history WHEN NEW.sealed<>0
BEGIN SELECT RAISE(ABORT,'job history must be assembled before sealing'); END;
CREATE TRIGGER job_history_update BEFORE UPDATE ON job_history WHEN OLD.sealed=1
BEGIN SELECT RAISE(ABORT,'sealed job history is immutable'); END;
CREATE TRIGGER job_history_delete BEFORE DELETE ON job_history WHEN OLD.sealed=1
BEGIN SELECT RAISE(ABORT,'sealed job history is immutable'); END;
CREATE TRIGGER job_history_item_insert BEFORE INSERT ON job_history_items
WHEN (SELECT sealed FROM job_history WHERE id=NEW.job_id)=1
BEGIN SELECT RAISE(ABORT,'sealed job history items are immutable'); END;
CREATE TRIGGER job_history_item_update BEFORE UPDATE ON job_history_items
WHEN (SELECT sealed FROM job_history WHERE id=OLD.job_id)=1 OR (SELECT sealed FROM job_history WHERE id=NEW.job_id)=1
BEGIN SELECT RAISE(ABORT,'sealed job history items are immutable'); END;
CREATE TRIGGER job_history_item_delete BEFORE DELETE ON job_history_items
WHEN (SELECT sealed FROM job_history WHERE id=OLD.job_id)=1
BEGIN SELECT RAISE(ABORT,'sealed job history items are immutable'); END;
CREATE TRIGGER job_history_log_insert BEFORE INSERT ON job_history_logs
WHEN (SELECT sealed FROM job_history WHERE id=NEW.job_id)=1
BEGIN SELECT RAISE(ABORT,'sealed job history logs are immutable'); END;
CREATE TRIGGER job_history_log_update BEFORE UPDATE ON job_history_logs
WHEN (SELECT sealed FROM job_history WHERE id=OLD.job_id)=1 OR (SELECT sealed FROM job_history WHERE id=NEW.job_id)=1
BEGIN SELECT RAISE(ABORT,'sealed job history logs are immutable'); END;
CREATE TRIGGER job_history_log_delete BEFORE DELETE ON job_history_logs
WHEN (SELECT sealed FROM job_history WHERE id=OLD.job_id)=1
BEGIN SELECT RAISE(ABORT,'sealed job history logs are immutable'); END;
