-- 取得元の内容ではなく、アプリがいつ詳細を確認したかを保持する。
ALTER TABLE source_entity_state ADD COLUMN last_deep_checked_at INTEGER;
CREATE TRIGGER source_entity_state_kind BEFORE INSERT ON source_entity_state
WHEN (SELECT kind FROM entities WHERE id=NEW.entity_id) NOT IN ('work','person','series')
BEGIN SELECT RAISE(ABORT,'source state must belong to a source entity'); END;
CREATE TRIGGER source_entity_state_owner BEFORE UPDATE ON source_entity_state
WHEN NEW.entity_id<>OLD.entity_id
BEGIN SELECT RAISE(ABORT,'source state owner is immutable'); END;
