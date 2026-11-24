-- 補完で取りに行く回数の上限を、棚の設定にする（repair_attempts）。欠けたまま確定した最初の
-- 保存も 1 回に数えるので、1 は「取り直さない」。範囲は repair_attempts.rs の REPAIR_ATTEMPT_LIMITS。
ALTER TABLE library_meta ADD COLUMN repair_attempt_limit INTEGER NOT NULL DEFAULT 2
    CHECK(repair_attempt_limit BETWEEN 1 AND 5);
