-- 利用者の検索条件は取得元の entity ではない。削除後も ID と更新世代を保持する。
ALTER TABLE saved_searches RENAME TO saved_searches_v4;
CREATE TABLE saved_searches (
    id BLOB PRIMARY KEY CHECK(length(id)=16),
    user_id BLOB NOT NULL CHECK(length(user_id)=16),
    name TEXT NOT NULL CHECK(length(name)>0),
    query TEXT,
    params_json TEXT NOT NULL CHECK(json_valid(params_json) AND json_type(params_json)='object'),
    definition_version INTEGER NOT NULL CHECK(definition_version=1),
    position INTEGER NOT NULL CHECK(position>=0),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    generation INTEGER NOT NULL CHECK(generation>0),
    deleted_at INTEGER,
    UNIQUE(id,user_id),
    UNIQUE(user_id,position)
) STRICT;
CREATE UNIQUE INDEX saved_search_name ON saved_searches(user_id,name) WHERE deleted_at IS NULL;
-- 先行する基盤の検索表に行があれば、既存 ID と条件の版もそのまま移す。
-- 未知の版や非オブジェクトの条件は制約違反で止め、旧表を失わない。
INSERT INTO saved_searches(id,user_id,name,query,params_json,definition_version,position,created_at,updated_at,generation)
SELECT id,(SELECT library_id FROM library_meta WHERE singleton=1),name,query,filters_json,filter_version,
       row_number() OVER (ORDER BY created_at,id)-1,created_at,updated_at,1 FROM saved_searches_v4;
DROP TABLE saved_searches_v4;
CREATE TRIGGER saved_search_identity BEFORE UPDATE ON saved_searches
WHEN NEW.id<>OLD.id OR NEW.user_id<>OLD.user_id OR NEW.created_at<>OLD.created_at OR NEW.position<>OLD.position
BEGIN SELECT RAISE(ABORT,'saved search identity is immutable'); END;
CREATE TRIGGER saved_search_generation BEFORE UPDATE ON saved_searches
WHEN NEW.generation<>OLD.generation+1
BEGIN SELECT RAISE(ABORT,'saved search generation must advance'); END;
CREATE TRIGGER saved_search_delete BEFORE DELETE ON saved_searches
BEGIN SELECT RAISE(ABORT,'saved searches require tombstones'); END;

-- 検索を架空の作品にせず、型付き FK で outbox の対象と所有者を照合する。
-- 消去済みの最大 sequence も再利用しない。
CREATE TABLE migration_outbox_sequence(value INTEGER NOT NULL);
INSERT INTO migration_outbox_sequence SELECT seq FROM sqlite_sequence WHERE name='outbox';
CREATE TABLE outbox_next (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    request_id BLOB NOT NULL UNIQUE CHECK(length(request_id)=16),
    entity_id BLOB REFERENCES entities(id),
    operation TEXT NOT NULL,
    payload_json TEXT NOT NULL CHECK(json_valid(payload_json)),
    created_at INTEGER NOT NULL,
    saved_search_id BLOB,
    user_id BLOB,
    FOREIGN KEY(saved_search_id,user_id) REFERENCES saved_searches(id,user_id),
    CHECK((entity_id IS NOT NULL AND saved_search_id IS NULL AND user_id IS NULL AND operation<>'saved_search')
       OR (entity_id IS NULL AND saved_search_id IS NOT NULL AND user_id IS NOT NULL AND operation='saved_search'))
) STRICT;
INSERT INTO outbox_next(sequence,request_id,entity_id,operation,payload_json,created_at)
SELECT sequence,request_id,entity_id,operation,payload_json,created_at FROM outbox;
DROP TABLE outbox;
ALTER TABLE outbox_next RENAME TO outbox;
UPDATE sqlite_sequence SET seq=max(seq,coalesce((SELECT max(value) FROM migration_outbox_sequence),0)) WHERE name='outbox';
INSERT INTO sqlite_sequence(name,seq)
SELECT 'outbox',max(value) FROM migration_outbox_sequence HAVING count(*)>0 AND NOT EXISTS(SELECT 1 FROM sqlite_sequence WHERE name='outbox');
DROP TABLE migration_outbox_sequence;
