-- 棚を正本へ切り替えた時刻。NULL のあいだは旧棚から移した写しで、アプリは書き込まない
-- （旧棚からの追いつきを続けられるように）。切り替えた後は旧棚から移さない。
ALTER TABLE library_meta ADD COLUMN switched_at INTEGER CHECK(switched_at IS NULL OR switched_at > 0);
