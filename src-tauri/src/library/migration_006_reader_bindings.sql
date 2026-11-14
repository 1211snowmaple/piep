-- 本文の参照と、その保存版が選んだ画像・添付との対応。元の保存版は書き換えない。
CREATE TABLE reader_binding_sets (
    entity_id BLOB NOT NULL,
    snapshot_id BLOB NOT NULL,
    recipe INTEGER NOT NULL CHECK(recipe=1),
    source_manifest BLOB NOT NULL CHECK(length(source_manifest)=32),
    body_file_id BLOB NOT NULL REFERENCES content_files(id),
    bindings_hash BLOB NOT NULL CHECK(length(bindings_hash)=32),
    sealed INTEGER NOT NULL DEFAULT 0 CHECK(sealed IN (0,1)),
    PRIMARY KEY(snapshot_id,recipe),
    FOREIGN KEY(entity_id,snapshot_id) REFERENCES revision_snapshots(entity_id,id)
) STRICT;
CREATE TRIGGER reader_binding_source BEFORE INSERT ON reader_binding_sets
WHEN NEW.sealed<>0 OR NOT EXISTS(SELECT 1 FROM revision_snapshots s JOIN works w ON w.id=s.entity_id
    WHERE s.id=NEW.snapshot_id AND s.entity_id=NEW.entity_id AND s.published=1
      AND s.manifest_hash=NEW.source_manifest AND s.body_file_id=NEW.body_file_id)
BEGIN SELECT RAISE(ABORT,'reader bindings must match the published work and body'); END;
CREATE TRIGGER reader_binding_header_update BEFORE UPDATE ON reader_binding_sets
WHEN OLD.sealed=1 OR NEW.entity_id<>OLD.entity_id OR NEW.snapshot_id<>OLD.snapshot_id OR NEW.recipe<>OLD.recipe
 OR NEW.source_manifest<>OLD.source_manifest OR NEW.body_file_id<>OLD.body_file_id OR NEW.bindings_hash<>OLD.bindings_hash
BEGIN SELECT RAISE(ABORT,'reader binding set is immutable'); END;
CREATE TRIGGER reader_binding_header_delete BEFORE DELETE ON reader_binding_sets
WHEN OLD.sealed=1 BEGIN SELECT RAISE(ABORT,'reader binding set is immutable'); END;
CREATE TABLE reader_bindings (
    snapshot_id BLOB NOT NULL,
    recipe INTEGER NOT NULL,
    reference_json TEXT NOT NULL CHECK(json_valid(reference_json) AND json_type(reference_json)='object'),
    media_slot TEXT,
    file_slot TEXT,
    unavailable INTEGER NOT NULL CHECK(unavailable IN (0,1)),
    PRIMARY KEY(snapshot_id,recipe,reference_json),
    FOREIGN KEY(snapshot_id,recipe) REFERENCES reader_binding_sets(snapshot_id,recipe),
    FOREIGN KEY(snapshot_id,media_slot) REFERENCES snapshot_media(snapshot_id,slot),
    FOREIGN KEY(snapshot_id,file_slot) REFERENCES snapshot_files(snapshot_id,slot),
    CHECK((unavailable=1 AND media_slot IS NULL AND file_slot IS NULL)
       OR (unavailable=0 AND ((media_slot IS NOT NULL AND file_slot IS NULL) OR (media_slot IS NULL AND file_slot IS NOT NULL))))
) STRICT;
CREATE TRIGGER reader_binding_insert BEFORE INSERT ON reader_bindings
WHEN (SELECT sealed FROM reader_binding_sets WHERE snapshot_id=NEW.snapshot_id AND recipe=NEW.recipe)=1
BEGIN SELECT RAISE(ABORT,'reader bindings are immutable'); END;
CREATE TRIGGER reader_binding_update BEFORE UPDATE ON reader_bindings
BEGIN SELECT RAISE(ABORT,'reader bindings are immutable'); END;
CREATE TRIGGER reader_binding_delete BEFORE DELETE ON reader_bindings
WHEN (SELECT sealed FROM reader_binding_sets WHERE snapshot_id=OLD.snapshot_id AND recipe=OLD.recipe)=1
BEGIN SELECT RAISE(ABORT,'reader bindings are immutable'); END;

-- 先行する移行済みの棚でも、対応表を補う前に切替可能とは表示しない。
INSERT INTO migration_records(source_table,source_key,status)
SELECT '__normalization__','reader_bindings','unresolved'
WHERE EXISTS(SELECT 1 FROM migration_records WHERE source_table='download_versions' AND status='imported')
ON CONFLICT(source_table,source_key) DO NOTHING;
