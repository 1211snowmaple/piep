-- 旧棚から移した取得の記録を、今の取得と同じ形にそろえる。
-- ・移した取得の経路（legacy-import・legacy-profile-import）を、同じ取得をいま行ったときの経路
--   （save・profile-refresh）へ。画像の取得の記録も同じ。
-- ・移行で作品の行を写しただけの記録（legacy-current-row）は取得元の応答ではないので落とす。
--   どの版にも、ほかに取得元の応答の記録がある。画像の取得の記録は URL が本物なので save へ。
-- ・画質が「不明」の画像を、取得した URL から今の保存と同じ規則で決め直す（work_capture::quality）。
--   一つの画像の URL の区分が食い違うものは不明のまま残す。
-- 取得の記録と画質は保存状態の指紋に入っているので、触れた版の指紋はこの後で計算し直す
-- （schema.rs）。対応表の指紋は構成の指紋を含むので、取得が作った読む形の記録とともに移す。
-- その間だけ、公開済みの保存状態・読む形の見出し・取得の読む形の記録を書き換えられるようにする。
CREATE TEMP TABLE unclassified_snapshots(id BLOB PRIMARY KEY) STRICT;
INSERT OR IGNORE INTO temp.unclassified_snapshots
SELECT so.snapshot_id FROM snapshot_observations so
  JOIN acquisition_observations o ON o.id=so.observation_id
  JOIN revision_snapshots s ON s.id=so.snapshot_id AND s.published=1
 WHERE o.route IN ('legacy-import','legacy-profile-import','legacy-current-row');

CREATE TEMP TABLE requalified(id BLOB PRIMARY KEY, quality TEXT NOT NULL) STRICT;
INSERT INTO temp.requalified
SELECT rendition_id, min(class) FROM (
  SELECT o.rendition_id,
         CASE WHEN instr(o.url,'/c/')>0 THEN 'thumbnail'
              WHEN instr(o.url,'-original/')>0
                OR substr(o.url,1,28)='https://downloads.fanbox.cc/'
                OR substr(o.url,1,38)='https://pixiv.pximg.net/fanbox/public/' THEN 'original'
              WHEN instr(o.url,'-master/')>0 THEN 'master'
              ELSE 'unknown' END AS class
    FROM media_observations o JOIN media_renditions r ON r.id=o.rendition_id
   WHERE r.quality='unknown' AND o.url IS NOT NULL)
 GROUP BY rendition_id HAVING count(DISTINCT class)=1 AND min(class)<>'unknown';
-- 同じ画像・同じファイルを、決め直す画質で別に持っているものは不明のまま残す（同じ中身の行は一つまで）。
DELETE FROM temp.requalified WHERE EXISTS(
  SELECT 1 FROM media_renditions r JOIN media_renditions o ON o.media_id=r.media_id AND o.file_id=r.file_id
   AND o.mime_type=r.mime_type AND o.quality=temp.requalified.quality
   AND coalesce(o.width,0)=coalesce(r.width,0) AND coalesce(o.height,0)=coalesce(r.height,0)
   WHERE r.id=temp.requalified.id);
INSERT OR IGNORE INTO temp.unclassified_snapshots
SELECT sm.snapshot_id FROM snapshot_media sm
  JOIN temp.requalified q ON q.id=sm.rendition_id
  JOIN revision_snapshots s ON s.id=sm.snapshot_id AND s.published=1;

DROP TRIGGER acquisition_observations_immutable;
DROP TRIGGER media_observations_immutable;
DROP TRIGGER media_renditions_immutable;
DROP TRIGGER published_snapshot_immutable;
DROP TRIGGER reader_binding_header_update;
DROP TRIGGER acquisition_reader_update;

UPDATE media_renditions SET quality=(SELECT q.quality FROM temp.requalified q WHERE q.id=media_renditions.id)
 WHERE id IN (SELECT id FROM temp.requalified);
DROP TABLE temp.requalified;

UPDATE acquisition_observations SET route='save' WHERE route='legacy-import';
UPDATE acquisition_observations SET route='profile-refresh' WHERE route='legacy-profile-import';
UPDATE media_observations SET route='save' WHERE route IN ('legacy-import','legacy-current-row');
UPDATE media_observations SET route='profile-refresh' WHERE route='legacy-profile-import';
DELETE FROM snapshot_observations
 WHERE observation_id IN (SELECT id FROM acquisition_observations WHERE route='legacy-current-row');
DELETE FROM acquisition_observations WHERE route='legacy-current-row';

CREATE TRIGGER acquisition_observations_immutable BEFORE UPDATE ON acquisition_observations
BEGIN SELECT RAISE(ABORT,'published record is immutable'); END;
CREATE TRIGGER media_observations_immutable BEFORE UPDATE ON media_observations
BEGIN SELECT RAISE(ABORT,'media observations are immutable'); END;
CREATE TRIGGER media_renditions_immutable BEFORE UPDATE ON media_renditions
BEGIN SELECT RAISE(ABORT,'media renditions are immutable'); END;
