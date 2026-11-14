-- ネットワーク取得の開始を、保存準備より前に記録する。
CREATE TABLE acquisition_heads (
    entity_id BLOB PRIMARY KEY REFERENCES entities(id),
    epoch INTEGER NOT NULL CHECK(epoch>0)
) STRICT;
CREATE TRIGGER acquisition_epoch_monotonic BEFORE UPDATE ON acquisition_heads
WHEN NEW.entity_id<>OLD.entity_id OR NEW.epoch<=OLD.epoch
BEGIN SELECT RAISE(ABORT,'acquisition epochs must increase'); END;
CREATE TABLE acquisition_attempts (
    id BLOB PRIMARY KEY CHECK(length(id)=16),
    entity_id BLOB NOT NULL REFERENCES entities(id),
    provider TEXT NOT NULL,
    kind TEXT NOT NULL CHECK(kind IN ('work','person','series')),
    external_key TEXT NOT NULL,
    route TEXT NOT NULL,
    epoch INTEGER NOT NULL CHECK(epoch>0),
    base_snapshot_id BLOB,
    base_generation INTEGER NOT NULL CHECK(base_generation>=0),
    started_at INTEGER NOT NULL,
    state TEXT NOT NULL DEFAULT 'pending' CHECK(state IN ('pending','complete','superseded','cancelled')),
    input_manifest_file_id BLOB REFERENCES content_files(id),
    observation_id BLOB,
    result_snapshot_id BLOB,
    finished_at INTEGER,
    published INTEGER NOT NULL DEFAULT 1 CHECK(published=1),
    UNIQUE(entity_id,epoch),
    FOREIGN KEY(entity_id,base_snapshot_id,published) REFERENCES revision_snapshots(entity_id,id,published),
    FOREIGN KEY(entity_id,result_snapshot_id,published) REFERENCES revision_snapshots(entity_id,id,published) DEFERRABLE INITIALLY DEFERRED,
    FOREIGN KEY(entity_id,observation_id) REFERENCES acquisition_observations(entity_id,id) DEFERRABLE INITIALLY DEFERRED,
    CHECK((base_snapshot_id IS NULL AND base_generation=0) OR (base_snapshot_id IS NOT NULL AND base_generation>0)),
    CHECK((state='pending' AND input_manifest_file_id IS NULL AND observation_id IS NULL AND result_snapshot_id IS NULL AND finished_at IS NULL)
       OR (state='complete' AND input_manifest_file_id IS NOT NULL AND observation_id IS NOT NULL AND result_snapshot_id IS NOT NULL AND finished_at IS NOT NULL)
       OR (state='superseded' AND input_manifest_file_id IS NOT NULL AND observation_id IS NULL AND result_snapshot_id IS NULL AND finished_at IS NOT NULL)
       OR (state='cancelled' AND input_manifest_file_id IS NULL AND observation_id IS NULL AND result_snapshot_id IS NULL AND finished_at IS NOT NULL))
) STRICT;
CREATE TRIGGER acquisition_scope_insert BEFORE INSERT ON acquisition_attempts
WHEN NEW.state<>'pending'
 OR NEW.epoch IS NOT (SELECT epoch FROM acquisition_heads WHERE entity_id=NEW.entity_id)
 OR NEW.base_snapshot_id IS NOT (SELECT snapshot_id FROM entity_current WHERE entity_id=NEW.entity_id)
 OR NEW.base_generation<>coalesce((SELECT generation FROM entity_current WHERE entity_id=NEW.entity_id),0)
 OR NOT EXISTS(SELECT 1 FROM external_identities e WHERE e.entity_id=NEW.entity_id AND e.provider=NEW.provider AND e.kind=NEW.kind AND e.external_key=NEW.external_key)
BEGIN SELECT RAISE(ABORT,'acquisition scope does not own its entity'); END;
CREATE TRIGGER acquisition_header_immutable BEFORE UPDATE ON acquisition_attempts
WHEN OLD.state<>'pending' OR NEW.id<>OLD.id OR NEW.entity_id<>OLD.entity_id OR NEW.provider<>OLD.provider OR NEW.kind<>OLD.kind OR NEW.external_key<>OLD.external_key OR NEW.route<>OLD.route OR NEW.epoch<>OLD.epoch OR NEW.base_snapshot_id IS NOT OLD.base_snapshot_id OR NEW.base_generation<>OLD.base_generation OR NEW.started_at<>OLD.started_at
BEGIN SELECT RAISE(ABORT,'acquisition context is immutable'); END;
CREATE TRIGGER acquisition_completion_link BEFORE UPDATE ON acquisition_attempts
WHEN NEW.state='complete' AND NOT EXISTS(
    SELECT 1 FROM acquisition_observations o WHERE o.id=NEW.observation_id
    AND o.entity_id=NEW.entity_id AND o.request_id=NEW.id
    AND o.request_hash=NEW.input_manifest_file_id AND o.result_snapshot_id=NEW.result_snapshot_id
    AND o.route=NEW.route
)
BEGIN SELECT RAISE(ABORT,'acquisition completion does not match its observation'); END;
CREATE TABLE acquisition_inputs (
    attempt_id BLOB NOT NULL REFERENCES acquisition_attempts(id),
    role TEXT NOT NULL CHECK(role IN ('raw_part','raw','body','media','attachment','request')),
    slot TEXT NOT NULL,
    file_id BLOB NOT NULL REFERENCES content_files(id),
    PRIMARY KEY(attempt_id,role,slot)
) STRICT;
CREATE TRIGGER acquisition_input_pending BEFORE INSERT ON acquisition_inputs
WHEN (SELECT state FROM acquisition_attempts WHERE id=NEW.attempt_id)<>'pending'
BEGIN SELECT RAISE(ABORT,'closed acquisition inputs are immutable'); END;
CREATE TRIGGER acquisition_input_immutable BEFORE UPDATE ON acquisition_inputs
BEGIN SELECT RAISE(ABORT,'acquisition inputs are immutable'); END;
CREATE TRIGGER acquisition_input_delete BEFORE DELETE ON acquisition_inputs
WHEN (SELECT state FROM acquisition_attempts WHERE id=OLD.attempt_id)<>'pending'
BEGIN SELECT RAISE(ABORT,'closed acquisition inputs are immutable'); END;
CREATE TRIGGER external_identity_immutable BEFORE UPDATE ON external_identities
BEGIN SELECT RAISE(ABORT,'external identities are immutable'); END;
