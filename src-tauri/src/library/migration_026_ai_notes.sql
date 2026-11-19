-- AI の手伝いが作った覚え書き（作品のあらすじ・作者の作風・前回のあらすじ）。作り直せる派生物
-- なので版の記録は持たず、対象と種類ごとに一行を置き換える。対象の鍵は棚の ID（作品は ID の
-- 文字列、作者は `取得元:作者の番号`、前回のあらすじは `読む話:前の話`）。
CREATE TABLE ai_notes (
    subject_type TEXT NOT NULL,
    subject_key TEXT NOT NULL,
    note_kind TEXT NOT NULL,
    text TEXT NOT NULL,
    source_tail TEXT,
    claims_json TEXT,
    model_id TEXT NOT NULL,
    feature_id TEXT NOT NULL,
    prompt_version TEXT NOT NULL,
    input_fingerprint TEXT NOT NULL,
    config_fingerprint TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY(subject_type,subject_key,note_kind)
) STRICT;
