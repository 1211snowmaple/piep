-- 提案と利用者の判断を、確定したコレクションと分けて保持する。
CREATE TABLE collection_suggestions (
    id BLOB PRIMARY KEY CHECK(length(id)=16),
    title TEXT NOT NULL,
    ordered INTEGER NOT NULL CHECK(ordered IN (0,1)),
    track TEXT NOT NULL CHECK(track IN ('sequence','theme')),
    origin TEXT NOT NULL CHECK(origin IN ('seed','sweep')),
    decision TEXT NOT NULL CHECK(decision IN ('pending','accepted','rejected')),
    score REAL NOT NULL CHECK(score>=0 AND score<=1),
    rule_version TEXT NOT NULL,
    explanation TEXT,
    name_options_json TEXT NOT NULL CHECK(json_valid(name_options_json) AND json_type(name_options_json)='array'),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    generation INTEGER NOT NULL CHECK(generation>0)
) STRICT;
CREATE INDEX collection_suggestion_decision ON collection_suggestions(decision,updated_at,id);
CREATE TABLE collection_suggestion_seeds (
    suggestion_id BLOB NOT NULL REFERENCES collection_suggestions(id),
    work_id BLOB NOT NULL REFERENCES works(id),
    position INTEGER NOT NULL CHECK(position>=0),
    PRIMARY KEY(suggestion_id,work_id),
    UNIQUE(suggestion_id,position)
) STRICT;
CREATE TABLE collection_suggestion_members (
    suggestion_id BLOB NOT NULL REFERENCES collection_suggestions(id),
    work_id BLOB NOT NULL REFERENCES works(id),
    title_at_proposal TEXT NOT NULL,
    author_at_proposal TEXT NOT NULL,
    preview_file_id BLOB REFERENCES content_files(id),
    preview_name TEXT,
    text_length INTEGER NOT NULL CHECK(text_length>=0),
    position INTEGER NOT NULL CHECK(position>=0),
    score REAL NOT NULL CHECK(score>=0 AND score<=1),
    selected INTEGER NOT NULL CHECK(selected IN (0,1)),
    evidence_json TEXT NOT NULL CHECK(json_valid(evidence_json) AND json_type(evidence_json)='array'),
    PRIMARY KEY(suggestion_id,work_id),
    UNIQUE(suggestion_id,position),
    CHECK((preview_file_id IS NULL)=(preview_name IS NULL))
) STRICT;
