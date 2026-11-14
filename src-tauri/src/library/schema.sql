-- 新ライブラリ専用。旧 downloads スキーマへの追加・互換ビューは作らない。
CREATE TABLE library_meta (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
    library_id BLOB NOT NULL CHECK(length(library_id) = 16),
    storage_version INTEGER NOT NULL DEFAULT 1 CHECK(storage_version>0),
    min_reader_version INTEGER NOT NULL DEFAULT 1 CHECK(min_reader_version>0),
    min_writer_version INTEGER NOT NULL DEFAULT 1 CHECK(min_writer_version>0),
    generation INTEGER NOT NULL DEFAULT 0 CHECK(generation >= 0)
) STRICT;
CREATE TABLE schema_migrations (
    version INTEGER PRIMARY KEY,
    checksum BLOB NOT NULL CHECK(length(checksum) = 32),
    applied_at INTEGER NOT NULL
) STRICT;
CREATE TABLE entities (
    id BLOB PRIMARY KEY CHECK(length(id) = 16),
    kind TEXT NOT NULL CHECK(kind IN ('work','person','series','collection')),
    created_at INTEGER NOT NULL,
    deleted_at INTEGER,
    UNIQUE(id,kind)
) STRICT;
CREATE TABLE external_identities (
    provider TEXT NOT NULL,
    kind TEXT NOT NULL,
    external_key TEXT NOT NULL CHECK(length(external_key) > 0),
    entity_id BLOB NOT NULL,
    PRIMARY KEY(provider,kind,external_key),
    FOREIGN KEY(entity_id,kind) REFERENCES entities(id,kind)
) STRICT;
CREATE INDEX external_identity_entity ON external_identities(entity_id);
CREATE TABLE works (
    id BLOB PRIMARY KEY,
    kind TEXT NOT NULL DEFAULT 'work' CHECK(kind = 'work'),
    content_type TEXT NOT NULL,
    FOREIGN KEY(id,kind) REFERENCES entities(id,kind)
) STRICT;
CREATE TABLE people (
    id BLOB PRIMARY KEY,
    kind TEXT NOT NULL DEFAULT 'person' CHECK(kind = 'person'),
    FOREIGN KEY(id,kind) REFERENCES entities(id,kind)
) STRICT;
CREATE TABLE series (
    id BLOB PRIMARY KEY,
    kind TEXT NOT NULL DEFAULT 'series' CHECK(kind = 'series'),
    FOREIGN KEY(id,kind) REFERENCES entities(id,kind)
) STRICT;
CREATE TABLE collections (
    id BLOB PRIMARY KEY,
    kind TEXT NOT NULL DEFAULT 'collection' CHECK(kind = 'collection'),
    title TEXT NOT NULL,
    description TEXT,
    appearance_json TEXT NOT NULL DEFAULT '{}' CHECK(json_valid(appearance_json)),
    cover_work_id BLOB REFERENCES works(id),
    cover_file_id BLOB REFERENCES content_files(id),
    ordered INTEGER NOT NULL DEFAULT 1 CHECK(ordered IN (0,1)),
    generation INTEGER NOT NULL DEFAULT 0 CHECK(generation >= 0),
    FOREIGN KEY(id,kind) REFERENCES entities(id,kind)
) STRICT;
CREATE TABLE collection_members (
    collection_id BLOB NOT NULL REFERENCES collections(id),
    work_id BLOB NOT NULL REFERENCES works(id),
    position INTEGER NOT NULL CHECK(position >= 0),
    title_at_add TEXT NOT NULL DEFAULT '',
    author_at_add TEXT NOT NULL DEFAULT '',
    role TEXT NOT NULL DEFAULT 'main',
    added_by TEXT NOT NULL DEFAULT 'manual',
    pinned INTEGER NOT NULL DEFAULT 0 CHECK(pinned IN (0,1)),
    note TEXT,
    created_at INTEGER NOT NULL DEFAULT 0,
    updated_at INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY(collection_id,work_id)
) STRICT;
CREATE INDEX collection_member_work ON collection_members(work_id);
CREATE TABLE chunks (
    id BLOB PRIMARY KEY CHECK(length(id) = 32),
    original_size INTEGER NOT NULL CHECK(original_size BETWEEN 0 AND 8388608)
) STRICT;
CREATE TABLE chunk_storage (
    chunk_id BLOB NOT NULL REFERENCES chunks(id),
    codec TEXT NOT NULL CHECK(codec IN ('raw','zstd')),
    stored_size INTEGER NOT NULL CHECK(stored_size >= 57),
    verified_at INTEGER NOT NULL,
    PRIMARY KEY(chunk_id,codec)
) STRICT;
CREATE TABLE content_files (
    id BLOB PRIMARY KEY CHECK(length(id) = 32),
    original_size INTEGER NOT NULL CHECK(original_size >= 0),
    sealed INTEGER NOT NULL DEFAULT 0 CHECK(sealed IN (0,1))
) STRICT;
CREATE TABLE file_chunks (
    file_id BLOB NOT NULL REFERENCES content_files(id),
    ordinal INTEGER NOT NULL CHECK(ordinal >= 0),
    offset INTEGER NOT NULL CHECK(offset >= 0),
    chunk_id BLOB NOT NULL REFERENCES chunks(id),
    PRIMARY KEY(file_id,ordinal),
    UNIQUE(file_id,offset)
) STRICT;
CREATE INDEX file_chunk_ref ON file_chunks(chunk_id);
CREATE TABLE source_revisions (
    id BLOB PRIMARY KEY CHECK(length(id) = 16),
    entity_id BLOB NOT NULL REFERENCES entities(id),
    ordinal INTEGER NOT NULL CHECK(ordinal > 0),
    classification TEXT NOT NULL CHECK(classification IN ('source','legacy_unclassified')),
    discovered_at INTEGER NOT NULL,
    UNIQUE(entity_id,ordinal),
    UNIQUE(entity_id,id)
) STRICT;
CREATE TABLE acquisition_observations (
    id BLOB PRIMARY KEY CHECK(length(id) = 16),
    entity_id BLOB NOT NULL REFERENCES entities(id),
    request_id BLOB NOT NULL UNIQUE CHECK(length(request_id) = 16),
    request_hash BLOB NOT NULL REFERENCES content_files(id) CHECK(length(request_hash) = 32),
    acquired_at INTEGER NOT NULL,
    recorded_at INTEGER NOT NULL,
    route TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('available','restricted','missing','error','imported')),
    raw_file_id BLOB REFERENCES content_files(id),
    body_file_id BLOB REFERENCES content_files(id),
    body_format TEXT CHECK(body_format IN ('plain_text_v1','pixiv_text_v1','fanbox_body_v1')),
    result_snapshot_id BLOB,
    created_revision INTEGER NOT NULL DEFAULT 0 CHECK(created_revision IN (0,1)),
    CHECK((body_file_id IS NULL)=(body_format IS NULL)),
    UNIQUE(entity_id,id),
    FOREIGN KEY(entity_id,result_snapshot_id) REFERENCES revision_snapshots(entity_id,id) DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE revision_snapshots (
    id BLOB PRIMARY KEY CHECK(length(id) = 16),
    entity_id BLOB NOT NULL,
    revision_id BLOB NOT NULL,
    sequence INTEGER NOT NULL CHECK(sequence>0),
    metadata_json TEXT NOT NULL CHECK(json_valid(metadata_json)),
    field_states_json TEXT NOT NULL CHECK(json_valid(field_states_json)),
    body_file_id BLOB REFERENCES content_files(id),
    body_format TEXT CHECK(body_format IN ('plain_text_v1','pixiv_text_v1','fanbox_body_v1')),
    body_semantic_hash BLOB CHECK(body_semantic_hash IS NULL OR length(body_semantic_hash)=32),
    body_semantic_rule INTEGER NOT NULL DEFAULT 1 CHECK(body_semantic_rule>0),
    interpretation_version INTEGER NOT NULL CHECK(interpretation_version > 0),
    semantic_rule INTEGER NOT NULL CHECK(semantic_rule > 0),
    manifest_rule INTEGER NOT NULL DEFAULT 1 CHECK(manifest_rule > 0),
    semantic_hash BLOB NOT NULL CHECK(length(semantic_hash) = 32),
    manifest_hash BLOB NOT NULL CHECK(length(manifest_hash) = 32),
    created_at INTEGER NOT NULL,
    published INTEGER NOT NULL DEFAULT 0 CHECK(published IN (0,1)),
    CHECK((body_file_id IS NULL)=(body_format IS NULL) AND (body_file_id IS NULL)=(body_semantic_hash IS NULL)),
    UNIQUE(entity_id,id),
    UNIQUE(entity_id,id,published),
    UNIQUE(entity_id,sequence),
    FOREIGN KEY(entity_id,revision_id) REFERENCES source_revisions(entity_id,id)
) STRICT;
CREATE INDEX snapshot_revision ON revision_snapshots(revision_id,created_at,id);
CREATE TRIGGER published_snapshot_immutable BEFORE UPDATE ON revision_snapshots
WHEN OLD.published = 1 BEGIN SELECT RAISE(ABORT,'published snapshot is immutable'); END;
CREATE TABLE snapshot_observations (
    entity_id BLOB NOT NULL,
    snapshot_id BLOB NOT NULL,
    observation_id BLOB NOT NULL,
    PRIMARY KEY(snapshot_id,observation_id),
    FOREIGN KEY(entity_id,snapshot_id) REFERENCES revision_snapshots(entity_id,id),
    FOREIGN KEY(entity_id,observation_id) REFERENCES acquisition_observations(entity_id,id)
) STRICT;
CREATE TABLE snapshot_files (
    id BLOB PRIMARY KEY CHECK(length(id) = 16),
    snapshot_id BLOB NOT NULL REFERENCES revision_snapshots(id),
    slot TEXT NOT NULL,
    file_id BLOB NOT NULL REFERENCES content_files(id),
    original_name TEXT NOT NULL,
    mime_type TEXT,
    role TEXT NOT NULL CHECK(role IN ('raw','attachment','document','other')),
    UNIQUE(snapshot_id,slot)
) STRICT;
CREATE TABLE source_media (
    id BLOB PRIMARY KEY CHECK(length(id) = 16),
    provider TEXT NOT NULL,
    kind TEXT NOT NULL,
    source_key TEXT,
    identity_rule INTEGER NOT NULL CHECK(identity_rule > 0),
    UNIQUE(provider,kind,identity_rule,source_key)
) STRICT;
CREATE TABLE media_renditions (
    id BLOB PRIMARY KEY CHECK(length(id) = 16),
    media_id BLOB NOT NULL REFERENCES source_media(id),
    file_id BLOB NOT NULL REFERENCES content_files(id),
    width INTEGER CHECK(width > 0),
    height INTEGER CHECK(height > 0),
    mime_type TEXT NOT NULL,
    quality TEXT NOT NULL CHECK(quality IN ('original','master','thumbnail','unknown')),
    UNIQUE(media_id,id)
) STRICT;
CREATE UNIQUE INDEX media_rendition_content ON media_renditions(
    media_id,file_id,mime_type,quality,coalesce(width,0),coalesce(height,0)
);
CREATE TABLE media_observations (
    id BLOB PRIMARY KEY CHECK(length(id) = 16),
    rendition_id BLOB NOT NULL REFERENCES media_renditions(id),
    url TEXT,
    acquired_at INTEGER NOT NULL,
    route TEXT NOT NULL
) STRICT;
CREATE TABLE snapshot_media (
    snapshot_id BLOB NOT NULL REFERENCES revision_snapshots(id),
    slot TEXT NOT NULL,
    role TEXT NOT NULL CHECK(role IN ('cover','inline','illustration','icon','banner')),
    media_id BLOB NOT NULL REFERENCES source_media(id),
    rendition_id BLOB,
    original_name TEXT NOT NULL,
    PRIMARY KEY(snapshot_id,slot),
    FOREIGN KEY(media_id,rendition_id) REFERENCES media_renditions(media_id,id)
) STRICT;
CREATE INDEX snapshot_media_rendition ON snapshot_media(rendition_id);
CREATE TABLE snapshot_people (
    snapshot_id BLOB NOT NULL REFERENCES revision_snapshots(id),
    person_id BLOB NOT NULL REFERENCES people(id),
    role TEXT NOT NULL,
    display_name TEXT NOT NULL,
    PRIMARY KEY(snapshot_id,person_id,role)
) STRICT;
CREATE TABLE snapshot_series (
    snapshot_id BLOB NOT NULL REFERENCES revision_snapshots(id),
    series_id BLOB NOT NULL REFERENCES series(id),
    title TEXT NOT NULL,
    position INTEGER,
    PRIMARY KEY(snapshot_id,series_id)
) STRICT;
CREATE TABLE tags (
    id BLOB PRIMARY KEY CHECK(length(id) = 16),
    provider TEXT NOT NULL,
    name TEXT NOT NULL,
    UNIQUE(provider,name)
) STRICT;
CREATE TABLE snapshot_tags (
    snapshot_id BLOB NOT NULL REFERENCES revision_snapshots(id),
    tag_id BLOB NOT NULL REFERENCES tags(id),
    PRIMARY KEY(snapshot_id,tag_id)
) STRICT;
CREATE TABLE entity_current (
    entity_id BLOB PRIMARY KEY REFERENCES entities(id),
    snapshot_id BLOB NOT NULL,
    published INTEGER NOT NULL DEFAULT 1 CHECK(published = 1),
    generation INTEGER NOT NULL CHECK(generation > 0),
    UNIQUE(entity_id,snapshot_id),
    FOREIGN KEY(entity_id,snapshot_id,published) REFERENCES revision_snapshots(entity_id,id,published)
) STRICT;
CREATE TABLE library_items (
    work_id BLOB PRIMARY KEY REFERENCES works(id),
    added_at INTEGER NOT NULL,
    generation INTEGER NOT NULL CHECK(generation > 0)
) STRICT;
CREATE TABLE source_entity_state (
    entity_id BLOB PRIMARY KEY REFERENCES entities(id),
    last_checked_at INTEGER,
    last_fetched_at INTEGER
) STRICT;
CREATE TABLE work_current_projection (
    work_id BLOB PRIMARY KEY REFERENCES library_items(work_id),
    snapshot_id BLOB NOT NULL,
    title TEXT NOT NULL,
    author_name TEXT NOT NULL,
    published_at INTEGER,
    text_length INTEGER NOT NULL CHECK(text_length >= 0),
    logical_size INTEGER NOT NULL CHECK(logical_size >= 0),
    recipe INTEGER NOT NULL CHECK(recipe > 0),
    FOREIGN KEY(work_id,snapshot_id) REFERENCES entity_current(entity_id,snapshot_id) DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE INDEX work_projection_recent ON work_current_projection(published_at DESC,work_id);
CREATE INDEX work_projection_title ON work_current_projection(title,work_id);
CREATE TABLE edit_revisions (
    id BLOB PRIMARY KEY CHECK(length(id) = 16),
    work_id BLOB NOT NULL REFERENCES works(id),
    base_snapshot_id BLOB NOT NULL,
    parent_id BLOB,
    content_file_id BLOB NOT NULL REFERENCES content_files(id),
    created_at INTEGER NOT NULL,
    sealed INTEGER NOT NULL DEFAULT 0 CHECK(sealed IN (0,1)),
    base_published INTEGER NOT NULL DEFAULT 1 CHECK(base_published=1),
    UNIQUE(work_id,id),
    UNIQUE(work_id,id,sealed),
    FOREIGN KEY(work_id,base_snapshot_id,base_published) REFERENCES revision_snapshots(entity_id,id,published),
    FOREIGN KEY(work_id,parent_id,base_published) REFERENCES edit_revisions(work_id,id,sealed)
) STRICT;
CREATE TABLE edit_documents (
    work_id BLOB NOT NULL REFERENCES works(id),
    user_id BLOB NOT NULL CHECK(length(user_id) = 16),
    active_id BLOB,
    draft_id BLOB,
    generation INTEGER NOT NULL CHECK(generation >= 0),
    sealed INTEGER NOT NULL DEFAULT 1 CHECK(sealed=1),
    PRIMARY KEY(work_id,user_id),
    FOREIGN KEY(work_id,active_id,sealed) REFERENCES edit_revisions(work_id,id,sealed),
    FOREIGN KEY(work_id,draft_id,sealed) REFERENCES edit_revisions(work_id,id,sealed)
) STRICT;
CREATE TABLE edit_media (
    edit_id BLOB NOT NULL REFERENCES edit_revisions(id),
    slot TEXT NOT NULL,
    file_id BLOB NOT NULL REFERENCES content_files(id),
    original_name TEXT NOT NULL,
    mime_type TEXT,
    PRIMARY KEY(edit_id,slot)
) STRICT;
CREATE TABLE edit_blocks (
    edit_id BLOB NOT NULL REFERENCES edit_revisions(id),
    block_id BLOB NOT NULL CHECK(length(block_id)=16),
    ordinal INTEGER NOT NULL CHECK(ordinal>=0),
    kind TEXT NOT NULL,
    text TEXT,
    attrs_json TEXT NOT NULL CHECK(json_valid(attrs_json)),
    media_slot TEXT,
    PRIMARY KEY(edit_id,block_id),
    UNIQUE(edit_id,ordinal),
    FOREIGN KEY(edit_id,media_slot) REFERENCES edit_media(edit_id,slot)
) STRICT;
CREATE TABLE work_user_state (
    work_id BLOB NOT NULL REFERENCES works(id),
    user_id BLOB NOT NULL CHECK(length(user_id) = 16),
    favorite INTEGER NOT NULL CHECK(favorite IN (0,1)),
    generation INTEGER NOT NULL CHECK(generation > 0),
    PRIMARY KEY(work_id,user_id)
) STRICT;
CREATE TABLE reading_positions (
    work_id BLOB NOT NULL REFERENCES works(id),
    user_id BLOB NOT NULL CHECK(length(user_id) = 16),
    snapshot_id BLOB,
    edit_id BLOB,
    anchor_json TEXT CHECK(anchor_json IS NULL OR json_valid(anchor_json)),
    cleared INTEGER NOT NULL CHECK(cleared IN (0,1)),
    finished INTEGER NOT NULL CHECK(finished IN (0,1)),
    device_id BLOB NOT NULL CHECK(length(device_id)=16),
    updated_at INTEGER NOT NULL,
    published INTEGER NOT NULL DEFAULT 1 CHECK(published=1),
    generation INTEGER NOT NULL CHECK(generation > 0),
    PRIMARY KEY(work_id,user_id),
    CHECK((cleared=1 AND snapshot_id IS NULL AND edit_id IS NULL AND anchor_json IS NULL AND finished=0)
       OR (cleared=0 AND ((snapshot_id IS NULL) <> (edit_id IS NULL)) AND anchor_json IS NOT NULL)),
    FOREIGN KEY(work_id,snapshot_id,published) REFERENCES revision_snapshots(entity_id,id,published),
    FOREIGN KEY(work_id,edit_id,published) REFERENCES edit_revisions(work_id,id,sealed)
) STRICT;
CREATE TABLE outbox (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    request_id BLOB NOT NULL UNIQUE CHECK(length(request_id) = 16),
    entity_id BLOB NOT NULL REFERENCES entities(id),
    operation TEXT NOT NULL,
    payload_json TEXT NOT NULL CHECK(json_valid(payload_json)),
    created_at INTEGER NOT NULL
) STRICT;
CREATE TABLE storage_operations (
    id BLOB PRIMARY KEY CHECK(length(id) = 16),
    kind TEXT NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('prepared','committed','complete','failed')),
    payload_json TEXT NOT NULL CHECK(json_valid(payload_json)),
    created_at INTEGER NOT NULL
) STRICT;
CREATE TABLE migration_records (
    source_table TEXT NOT NULL,
    source_key TEXT NOT NULL,
    target_id BLOB CHECK(target_id IS NULL OR length(target_id) = 16),
    evidence_file_id BLOB REFERENCES content_files(id),
    status TEXT NOT NULL CHECK(status IN ('imported','unresolved')),
    PRIMARY KEY(source_table,source_key)
) STRICT;
CREATE TABLE migration_requests (
    source_table TEXT NOT NULL,
    source_key TEXT NOT NULL,
    request_id BLOB NOT NULL UNIQUE CHECK(length(request_id) = 16),
    PRIMARY KEY(source_table,source_key)
) STRICT;
CREATE TABLE watch_targets (
    id BLOB PRIMARY KEY CHECK(length(id)=16),
    entity_id BLOB NOT NULL UNIQUE REFERENCES entities(id),
    display_name TEXT NOT NULL,
    enabled INTEGER NOT NULL CHECK(enabled IN (0,1)),
    last_checked_at INTEGER,
    last_seen_external_key TEXT,
    last_seen_source_updated_at INTEGER,
    last_hit_at INTEGER,
    consecutive_errors INTEGER NOT NULL DEFAULT 0 CHECK(consecutive_errors>=0),
    options_json TEXT NOT NULL DEFAULT '{}' CHECK(json_valid(options_json)),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
) STRICT;
CREATE TRIGGER watch_target_kind BEFORE INSERT ON watch_targets
WHEN (SELECT kind FROM entities WHERE id=NEW.entity_id) NOT IN ('work','person','series')
BEGIN SELECT RAISE(ABORT,'unsupported watch target kind'); END;
CREATE TABLE watch_groups (
    id BLOB PRIMARY KEY CHECK(length(id)=16),
    name TEXT NOT NULL UNIQUE,
    sort_order INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
) STRICT;
CREATE TABLE watch_group_members (
    group_id BLOB NOT NULL REFERENCES watch_groups(id),
    target_id BLOB NOT NULL REFERENCES watch_targets(id),
    PRIMARY KEY(group_id,target_id)
) STRICT;
CREATE INDEX watch_group_target ON watch_group_members(target_id);
CREATE TABLE user_tags (
    id BLOB PRIMARY KEY CHECK(length(id)=16),
    user_id BLOB NOT NULL CHECK(length(user_id)=16),
    name TEXT NOT NULL,
    UNIQUE(user_id,name),
    UNIQUE(user_id,id)
) STRICT;
CREATE TABLE work_user_tag_state (
    work_id BLOB NOT NULL REFERENCES works(id),
    user_id BLOB NOT NULL CHECK(length(user_id)=16),
    generation INTEGER NOT NULL CHECK(generation>0),
    PRIMARY KEY(work_id,user_id)
) STRICT;
CREATE TABLE work_user_tags (
    work_id BLOB NOT NULL REFERENCES works(id),
    user_id BLOB NOT NULL CHECK(length(user_id)=16),
    tag_id BLOB NOT NULL,
    provenance TEXT NOT NULL CHECK(provenance IN ('manual','accepted_model')),
    PRIMARY KEY(work_id,user_id,tag_id),
    FOREIGN KEY(user_id,tag_id) REFERENCES user_tags(user_id,id),
    FOREIGN KEY(work_id,user_id) REFERENCES work_user_tag_state(work_id,user_id)
) STRICT;
CREATE INDEX user_tag_work ON work_user_tags(tag_id);
CREATE TABLE discovery_candidates (
    id BLOB PRIMARY KEY CHECK(length(id)=16),
    work_id BLOB NOT NULL REFERENCES works(id),
    kind TEXT NOT NULL CHECK(kind IN ('new','revision')),
    title TEXT NOT NULL,
    decision TEXT NOT NULL CHECK(decision IN ('pending','held','saved','ignored','accepted','rejected')),
    payload_file_id BLOB NOT NULL REFERENCES content_files(id),
    target_type TEXT,
    first_seen_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE(work_id,kind)
) STRICT;
CREATE INDEX discovery_decision ON discovery_candidates(decision,updated_at,id);
CREATE TABLE work_links (
    id BLOB PRIMARY KEY CHECK(length(id)=16),
    from_work_id BLOB NOT NULL REFERENCES works(id),
    to_work_id BLOB NOT NULL REFERENCES works(id),
    relation TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('observed','accepted','rejected')),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE(from_work_id,to_work_id,relation)
) STRICT;
CREATE INDEX work_link_to ON work_links(to_work_id,status);
CREATE TABLE link_evidence (
    id BLOB PRIMARY KEY CHECK(length(id)=16),
    link_id BLOB NOT NULL REFERENCES work_links(id),
    kind TEXT NOT NULL,
    decision TEXT NOT NULL CHECK(decision IN ('observed','accepted','rejected')),
    anchor_text TEXT,
    context_text TEXT,
    confidence REAL NOT NULL CHECK(confidence>=0 AND confidence<=1),
    observed_at INTEGER NOT NULL
) STRICT;
CREATE INDEX link_evidence_link ON link_evidence(link_id);
CREATE TABLE collection_pair_feedback (
    left_work_id BLOB NOT NULL REFERENCES works(id),
    right_work_id BLOB NOT NULL REFERENCES works(id),
    decision TEXT NOT NULL CHECK(decision IN ('accept','reject')),
    rule_version TEXT NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY(left_work_id,right_work_id),
    CHECK(left_work_id<right_work_id)
) STRICT;
CREATE TABLE saved_searches (
    id BLOB PRIMARY KEY CHECK(length(id)=16),
    name TEXT NOT NULL UNIQUE,
    query TEXT,
    filters_json TEXT NOT NULL CHECK(json_valid(filters_json)),
    filter_version INTEGER NOT NULL CHECK(filter_version>0),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
) STRICT;

-- 公開後の構成は書き換えない。完全削除は親を先に外す遅延制約のトランザクションで行う。
CREATE TRIGGER snapshot_files_published_insert BEFORE INSERT ON snapshot_files
WHEN (SELECT published FROM revision_snapshots WHERE id=NEW.snapshot_id)=1 BEGIN SELECT RAISE(ABORT,'published snapshot references are immutable'); END;
CREATE TRIGGER snapshot_files_published_update BEFORE UPDATE ON snapshot_files
WHEN (SELECT published FROM revision_snapshots WHERE id=OLD.snapshot_id)=1 OR (SELECT published FROM revision_snapshots WHERE id=NEW.snapshot_id)=1 BEGIN SELECT RAISE(ABORT,'published snapshot references are immutable'); END;
CREATE TRIGGER snapshot_files_published_delete BEFORE DELETE ON snapshot_files
WHEN (SELECT published FROM revision_snapshots WHERE id=OLD.snapshot_id)=1 BEGIN SELECT RAISE(ABORT,'published snapshot references are immutable'); END;
CREATE TRIGGER snapshot_media_published_insert BEFORE INSERT ON snapshot_media
WHEN (SELECT published FROM revision_snapshots WHERE id=NEW.snapshot_id)=1 BEGIN SELECT RAISE(ABORT,'published snapshot references are immutable'); END;
CREATE TRIGGER snapshot_media_published_update BEFORE UPDATE ON snapshot_media
WHEN (SELECT published FROM revision_snapshots WHERE id=OLD.snapshot_id)=1 OR (SELECT published FROM revision_snapshots WHERE id=NEW.snapshot_id)=1 BEGIN SELECT RAISE(ABORT,'published snapshot references are immutable'); END;
CREATE TRIGGER snapshot_media_published_delete BEFORE DELETE ON snapshot_media
WHEN (SELECT published FROM revision_snapshots WHERE id=OLD.snapshot_id)=1 BEGIN SELECT RAISE(ABORT,'published snapshot references are immutable'); END;
CREATE TRIGGER snapshot_people_published_insert BEFORE INSERT ON snapshot_people
WHEN (SELECT published FROM revision_snapshots WHERE id=NEW.snapshot_id)=1 BEGIN SELECT RAISE(ABORT,'published snapshot references are immutable'); END;
CREATE TRIGGER snapshot_people_published_update BEFORE UPDATE ON snapshot_people
WHEN (SELECT published FROM revision_snapshots WHERE id=OLD.snapshot_id)=1 OR (SELECT published FROM revision_snapshots WHERE id=NEW.snapshot_id)=1 BEGIN SELECT RAISE(ABORT,'published snapshot references are immutable'); END;
CREATE TRIGGER snapshot_people_published_delete BEFORE DELETE ON snapshot_people
WHEN (SELECT published FROM revision_snapshots WHERE id=OLD.snapshot_id)=1 BEGIN SELECT RAISE(ABORT,'published snapshot references are immutable'); END;
CREATE TRIGGER snapshot_series_published_insert BEFORE INSERT ON snapshot_series
WHEN (SELECT published FROM revision_snapshots WHERE id=NEW.snapshot_id)=1 BEGIN SELECT RAISE(ABORT,'published snapshot references are immutable'); END;
CREATE TRIGGER snapshot_series_published_update BEFORE UPDATE ON snapshot_series
WHEN (SELECT published FROM revision_snapshots WHERE id=OLD.snapshot_id)=1 OR (SELECT published FROM revision_snapshots WHERE id=NEW.snapshot_id)=1 BEGIN SELECT RAISE(ABORT,'published snapshot references are immutable'); END;
CREATE TRIGGER snapshot_series_published_delete BEFORE DELETE ON snapshot_series
WHEN (SELECT published FROM revision_snapshots WHERE id=OLD.snapshot_id)=1 BEGIN SELECT RAISE(ABORT,'published snapshot references are immutable'); END;
CREATE TRIGGER snapshot_tags_published_insert BEFORE INSERT ON snapshot_tags
WHEN (SELECT published FROM revision_snapshots WHERE id=NEW.snapshot_id)=1 BEGIN SELECT RAISE(ABORT,'published snapshot references are immutable'); END;
CREATE TRIGGER snapshot_tags_published_update BEFORE UPDATE ON snapshot_tags
WHEN (SELECT published FROM revision_snapshots WHERE id=OLD.snapshot_id)=1 OR (SELECT published FROM revision_snapshots WHERE id=NEW.snapshot_id)=1 BEGIN SELECT RAISE(ABORT,'published snapshot references are immutable'); END;
CREATE TRIGGER snapshot_tags_published_delete BEFORE DELETE ON snapshot_tags
WHEN (SELECT published FROM revision_snapshots WHERE id=OLD.snapshot_id)=1 BEGIN SELECT RAISE(ABORT,'published snapshot references are immutable'); END;
CREATE TRIGGER file_chunks_sealed_insert BEFORE INSERT ON file_chunks
WHEN (SELECT sealed FROM content_files WHERE id=NEW.file_id)=1 BEGIN SELECT RAISE(ABORT,'sealed file references are immutable'); END;
CREATE TRIGGER file_chunks_sealed_update BEFORE UPDATE ON file_chunks
WHEN (SELECT sealed FROM content_files WHERE id=OLD.file_id)=1 OR (SELECT sealed FROM content_files WHERE id=NEW.file_id)=1 BEGIN SELECT RAISE(ABORT,'sealed file references are immutable'); END;
CREATE TRIGGER file_chunks_sealed_delete BEFORE DELETE ON file_chunks
WHEN (SELECT sealed FROM content_files WHERE id=OLD.file_id)=1 BEGIN SELECT RAISE(ABORT,'sealed file references are immutable'); END;
CREATE TRIGGER source_revisions_immutable BEFORE UPDATE ON source_revisions
BEGIN SELECT RAISE(ABORT,'published record is immutable'); END;
CREATE TRIGGER acquisition_observations_immutable BEFORE UPDATE ON acquisition_observations
BEGIN SELECT RAISE(ABORT,'published record is immutable'); END;
CREATE TRIGGER edit_revisions_immutable BEFORE UPDATE ON edit_revisions WHEN OLD.sealed=1
BEGIN SELECT RAISE(ABORT,'published record is immutable'); END;
CREATE TRIGGER chunks_immutable BEFORE UPDATE ON chunks
BEGIN SELECT RAISE(ABORT,'published record is immutable'); END;
CREATE TRIGGER sealed_file_immutable BEFORE UPDATE ON content_files WHEN OLD.sealed=1 BEGIN SELECT RAISE(ABORT,'sealed file is immutable'); END;
CREATE TRIGGER source_media_immutable BEFORE UPDATE ON source_media
BEGIN SELECT RAISE(ABORT,'source media identities are immutable'); END;
CREATE TRIGGER media_renditions_immutable BEFORE UPDATE ON media_renditions
BEGIN SELECT RAISE(ABORT,'media renditions are immutable'); END;
CREATE TRIGGER media_observations_immutable BEFORE UPDATE ON media_observations
BEGIN SELECT RAISE(ABORT,'media observations are immutable'); END;
CREATE TRIGGER source_tags_immutable BEFORE UPDATE ON tags
BEGIN SELECT RAISE(ABORT,'source tag names are immutable'); END;
CREATE TRIGGER edit_media_sealed_insert BEFORE INSERT ON edit_media
WHEN (SELECT sealed FROM edit_revisions WHERE id=NEW.edit_id)=1 BEGIN SELECT RAISE(ABORT,'sealed edit references are immutable'); END;
CREATE TRIGGER edit_media_sealed_update BEFORE UPDATE ON edit_media
WHEN (SELECT sealed FROM edit_revisions WHERE id=OLD.edit_id)=1 OR (SELECT sealed FROM edit_revisions WHERE id=NEW.edit_id)=1 BEGIN SELECT RAISE(ABORT,'sealed edit references are immutable'); END;
CREATE TRIGGER edit_media_sealed_delete BEFORE DELETE ON edit_media
WHEN (SELECT sealed FROM edit_revisions WHERE id=OLD.edit_id)=1 BEGIN SELECT RAISE(ABORT,'sealed edit references are immutable'); END;
CREATE TRIGGER edit_blocks_sealed_insert BEFORE INSERT ON edit_blocks
WHEN (SELECT sealed FROM edit_revisions WHERE id=NEW.edit_id)=1 BEGIN SELECT RAISE(ABORT,'sealed edit references are immutable'); END;
CREATE TRIGGER edit_blocks_sealed_update BEFORE UPDATE ON edit_blocks
WHEN (SELECT sealed FROM edit_revisions WHERE id=OLD.edit_id)=1 OR (SELECT sealed FROM edit_revisions WHERE id=NEW.edit_id)=1 BEGIN SELECT RAISE(ABORT,'sealed edit references are immutable'); END;
CREATE TRIGGER edit_blocks_sealed_delete BEFORE DELETE ON edit_blocks
WHEN (SELECT sealed FROM edit_revisions WHERE id=OLD.edit_id)=1 BEGIN SELECT RAISE(ABORT,'sealed edit references are immutable'); END;
