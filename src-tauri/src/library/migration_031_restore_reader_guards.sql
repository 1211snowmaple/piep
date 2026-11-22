-- 030 で指紋を計算し直したので、公開済みの保存状態・読む形の見出し・取得の読む形の記録を元どおり固める。
CREATE TRIGGER published_snapshot_immutable BEFORE UPDATE ON revision_snapshots
WHEN OLD.published = 1 BEGIN SELECT RAISE(ABORT,'published snapshot is immutable'); END;
CREATE TRIGGER reader_binding_header_update BEFORE UPDATE ON reader_binding_sets
WHEN OLD.sealed=1 OR NEW.entity_id<>OLD.entity_id OR NEW.snapshot_id<>OLD.snapshot_id OR NEW.recipe<>OLD.recipe
 OR NEW.source_manifest<>OLD.source_manifest OR NEW.body_file_id<>OLD.body_file_id OR NEW.bindings_hash<>OLD.bindings_hash
BEGIN SELECT RAISE(ABORT,'reader binding set is immutable'); END;
CREATE TRIGGER acquisition_reader_update BEFORE UPDATE ON acquisition_reader_outputs
BEGIN SELECT RAISE(ABORT,'acquisition reader outputs are immutable'); END;
DROP TABLE temp.unclassified_snapshots;
