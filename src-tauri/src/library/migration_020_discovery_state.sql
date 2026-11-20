-- 候補を片付けても判断の世代を捨てず、古い要求の再送で復活させない。
CREATE TABLE discovery_state (
    work_id BLOB PRIMARY KEY REFERENCES works(id),
    generation INTEGER NOT NULL CHECK(generation>=0)
) STRICT;
-- 原入力から作り直せる表示用の小さな写し。本文や認証情報を持たない。
CREATE TABLE discovery_summaries (
    candidate_id BLOB PRIMARY KEY REFERENCES discovery_candidates(id) ON DELETE CASCADE,
    payload_file_id BLOB NOT NULL REFERENCES content_files(id),
    summary_json TEXT NOT NULL CHECK(json_valid(summary_json))
) STRICT;
