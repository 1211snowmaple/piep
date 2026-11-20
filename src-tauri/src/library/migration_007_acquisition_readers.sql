-- 読書用の対応まで一度に確定した取得の完了記録。
CREATE TABLE acquisition_reader_outputs (
    attempt_id BLOB PRIMARY KEY REFERENCES acquisition_attempts(id),
    entity_id BLOB NOT NULL,
    snapshot_id BLOB NOT NULL,
    recipe INTEGER NOT NULL CHECK(recipe=1),
    bindings_hash BLOB NOT NULL CHECK(length(bindings_hash)=32),
    FOREIGN KEY(entity_id,snapshot_id) REFERENCES revision_snapshots(entity_id,id),
    FOREIGN KEY(snapshot_id,recipe) REFERENCES reader_binding_sets(snapshot_id,recipe)
) STRICT;
CREATE TRIGGER acquisition_reader_scope BEFORE INSERT ON acquisition_reader_outputs
WHEN NOT EXISTS(SELECT 1 FROM acquisition_attempts a JOIN reader_binding_sets r
    ON r.entity_id=a.entity_id AND r.snapshot_id=NEW.snapshot_id AND r.recipe=NEW.recipe
    WHERE a.id=NEW.attempt_id AND a.state='pending' AND a.kind='work'
      AND a.entity_id=NEW.entity_id AND r.sealed=1 AND r.bindings_hash=NEW.bindings_hash)
BEGIN SELECT RAISE(ABORT,'reader output must match the pending acquisition and sealed bindings'); END;
CREATE TRIGGER acquisition_reader_completion BEFORE UPDATE ON acquisition_attempts
WHEN EXISTS(SELECT 1 FROM acquisition_reader_outputs r WHERE r.attempt_id=OLD.id
    AND (NEW.state<>'complete' OR NEW.entity_id<>r.entity_id OR NEW.result_snapshot_id IS NOT r.snapshot_id))
BEGIN SELECT RAISE(ABORT,'acquisition completion must preserve its reader output'); END;
CREATE TRIGGER acquisition_reader_update BEFORE UPDATE ON acquisition_reader_outputs
BEGIN SELECT RAISE(ABORT,'acquisition reader outputs are immutable'); END;
CREATE TRIGGER acquisition_reader_delete BEFORE DELETE ON acquisition_reader_outputs
BEGIN SELECT RAISE(ABORT,'acquisition reader outputs are immutable'); END;
