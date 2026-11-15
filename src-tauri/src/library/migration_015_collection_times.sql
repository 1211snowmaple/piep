-- コレクションを最後に変えた時（ミリ秒）。旧棚の work_collections.updated_at にあたり、
-- 一覧とカードの日付・「更新日」の並べ替えに使う。移行の初めの作りはこの値を写して
-- いなかったので、NULL の行は開いたときに旧棚の行の写し（移行の原記録）から埋める。
ALTER TABLE collections ADD COLUMN updated_at INTEGER;
