-- 実行するキュー。過去の封をした job_history とは分ける。
CREATE TABLE execution_jobs (
    id BLOB PRIMARY KEY CHECK(length(id)=16),
    scope TEXT NOT NULL,
    mode TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('queued','running','paused','canceling','canceled','auth_required','failed','completed')),
    watch_saved INTEGER NOT NULL CHECK(watch_saved IN (0,1)),
    new_works_only INTEGER NOT NULL CHECK(new_works_only IN (0,1)),
    totals INTEGER NOT NULL DEFAULT 0 CHECK(totals>=0),
    processed INTEGER NOT NULL DEFAULT 0 CHECK(processed>=0),
    candidate_count INTEGER NOT NULL DEFAULT 0 CHECK(candidate_count>=0),
    saved_count INTEGER NOT NULL DEFAULT 0 CHECK(saved_count>=0),
    error_count INTEGER NOT NULL DEFAULT 0 CHECK(error_count>=0),
    active_label TEXT,
    started_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    finished_at TEXT
) STRICT;
CREATE TABLE execution_items (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    source_sequence INTEGER,
    job_id BLOB NOT NULL REFERENCES execution_jobs(id) ON DELETE CASCADE,
    item_type TEXT NOT NULL CHECK(item_type IN ('work','target','candidate')),
    entity_id BLOB REFERENCES entities(id),
    source TEXT,
    source_id TEXT,
    target_type TEXT,
    title TEXT NOT NULL,
    payload_file_id BLOB NOT NULL REFERENCES content_files(id),
    display_json TEXT NOT NULL DEFAULT '{}' CHECK(json_valid(display_json)),
    known_hold INTEGER NOT NULL DEFAULT 0 CHECK(known_hold IN (0,1)),
    status TEXT NOT NULL CHECK(status IN ('queued','running','candidate','done','saved','skipped','held','failed')),
    error TEXT,
    result_work_id BLOB REFERENCES works(id),
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CHECK(result_work_id IS NULL OR result_work_id IS entity_id),
    CHECK((source IS NULL AND source_id IS NULL AND entity_id IS NULL)
       OR (source IS NOT NULL AND length(source)>0 AND source_id IS NOT NULL AND length(source_id)>0 AND entity_id IS NOT NULL))
) STRICT;
CREATE TRIGGER execution_item_identity BEFORE INSERT ON execution_items
WHEN NEW.entity_id IS NOT NULL AND NOT EXISTS(
    SELECT 1 FROM external_identities x WHERE x.entity_id=NEW.entity_id
      AND x.provider=NEW.source AND x.external_key=NEW.source_id
      AND x.kind=CASE WHEN NEW.item_type IN ('work','candidate') OR NEW.target_type='work' THEN 'work'
                      WHEN NEW.target_type IN ('author','creator') THEN 'person' ELSE 'series' END
) BEGIN SELECT RAISE(ABORT,'execution item identity mismatch'); END;
CREATE TRIGGER execution_item_result BEFORE UPDATE OF result_work_id ON execution_items
WHEN NEW.result_work_id IS NOT NULL AND NEW.result_work_id IS NOT NEW.entity_id
BEGIN SELECT RAISE(ABORT,'execution result belongs to another work'); END;
CREATE TRIGGER execution_item_identity_update BEFORE UPDATE OF source,source_id,entity_id,item_type,target_type ON execution_items
WHEN NEW.entity_id IS NOT NULL AND NOT EXISTS(
    SELECT 1 FROM external_identities x WHERE x.entity_id=NEW.entity_id
      AND x.provider=NEW.source AND x.external_key=NEW.source_id
      AND x.kind=CASE WHEN NEW.item_type IN ('work','candidate') OR NEW.target_type='work' THEN 'work'
                      WHEN NEW.target_type IN ('author','creator') THEN 'person' ELSE 'series' END
) BEGIN SELECT RAISE(ABORT,'execution item identity mismatch'); END;
CREATE TABLE execution_logs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    source_sequence INTEGER,
    job_id BLOB NOT NULL REFERENCES execution_jobs(id) ON DELETE CASCADE,
    log_type TEXT NOT NULL,
    message TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
) STRICT;
CREATE INDEX execution_job_status ON execution_jobs(status,updated_at DESC);
CREATE INDEX execution_item_queue ON execution_items(job_id,status,item_type,id);
CREATE INDEX execution_item_source ON execution_items(source,source_id,item_type,id DESC);
CREATE INDEX execution_log_page ON execution_logs(job_id,id);
