-- 移行した旧棚の行の指紋。追いつき（同じ棚へもう一度移す）で、前回から変わっていない保存版を
-- 読み直さないために使う。指紋は旧棚の版の行・その作品の資産の行・原本 JSON の大きさと
-- 更新時刻から作る。棚の正本ではなく、消しても次の追いつきが全部を読み直すだけ。
CREATE TABLE migration_fingerprints (
    source_table TEXT NOT NULL,
    source_key TEXT NOT NULL,
    fingerprint BLOB NOT NULL CHECK(length(fingerprint)=32),
    PRIMARY KEY(source_table,source_key)
) STRICT, WITHOUT ROWID;
