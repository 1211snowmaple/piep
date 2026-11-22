-- 棚はもう旧棚の写しにならないので、正本へ切り替えた時刻を持たない。
ALTER TABLE library_meta DROP COLUMN switched_at;
