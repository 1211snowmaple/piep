-- 一覧の絞り込みを速くする。保存状態から決まる旗を一覧の列に持ち、少ない側に部分索引を張る。
-- 絞り込みに使う、保存状態から決まる旗と数。値は一覧を作り直すときに入る（recipe）。
ALTER TABLE work_current_projection ADD COLUMN has_cover INTEGER NOT NULL DEFAULT 0 CHECK(has_cover IN (0,1));
ALTER TABLE work_current_projection ADD COLUMN in_series INTEGER NOT NULL DEFAULT 0 CHECK(in_series IN (0,1));
ALTER TABLE work_current_projection ADD COLUMN revised INTEGER NOT NULL DEFAULT 0 CHECK(revised IN (0,1));
ALTER TABLE work_current_projection ADD COLUMN image_count INTEGER NOT NULL DEFAULT 0 CHECK(image_count >= 0);
ALTER TABLE work_current_projection ADD COLUMN file_count INTEGER NOT NULL DEFAULT 0 CHECK(file_count >= 0);
-- 少ない側だけの索引。当てはまる作品が少なければ、ここから引いて並べ直す（統計で選ばれる）。
-- 保存日の順に持つので、既定の並べ替えはそのまま読める。
CREATE INDEX work_list_favorite ON work_current_projection(saved_at) WHERE favorite=1;
CREATE INDEX work_list_watching ON work_current_projection(saved_at) WHERE watching=1;
CREATE INDEX work_list_edited ON work_current_projection(saved_at) WHERE edit_id IS NOT NULL;
CREATE INDEX work_list_no_cover ON work_current_projection(saved_at) WHERE has_cover=0;
CREATE INDEX work_list_revised ON work_current_projection(saved_at) WHERE revised=1;
CREATE INDEX work_list_files ON work_current_projection(saved_at) WHERE file_count>0;
-- 作者・タグ・シリーズから集めた保存版で一覧を引く索引。編集の有無と作者の名前も持たせ、
-- 「編集なし」「作者を除く」の件数を表の行を読まずに数える。
CREATE INDEX work_list_snapshot ON work_current_projection(snapshot_id,edit_id,author_name);
-- 旗だけの細い索引。表紙あり・シリーズありのような多い側の件数を、表の行を読まずに数える。
-- 取得元を先頭にすると、取得元の絞り込みが並べ替えの索引でなくこちらを選んで全件を並べ直す。
CREATE INDEX work_list_flags ON work_current_projection(favorite,watching,has_cover,in_series,revised,image_count,file_count,provider,content_type);
