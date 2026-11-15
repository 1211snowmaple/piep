-- 以前の編集版（反映から外れた版）の一覧。自動保存の下書きも封をした版として残るので、一覧に
-- 載せる版はここに持つ。一覧から外しても版そのものは残る（封をした版は変えない）。
CREATE TABLE edit_archive (
    work_id BLOB NOT NULL REFERENCES works(id),
    user_id BLOB NOT NULL CHECK(length(user_id) = 16),
    edit_id BLOB NOT NULL,
    sealed INTEGER NOT NULL DEFAULT 1 CHECK(sealed=1),
    PRIMARY KEY(work_id,user_id,edit_id),
    FOREIGN KEY(work_id,edit_id,sealed) REFERENCES edit_revisions(work_id,id,sealed)
) STRICT;
-- 旧棚から移した棚では、指し先にも親にもなっていない版が旧棚の「以前の編集版」。
INSERT INTO edit_archive(work_id,user_id,edit_id)
SELECT r.work_id,m.library_id,r.id FROM edit_revisions r JOIN library_meta m ON m.singleton=1
WHERE r.sealed=1
  AND NOT EXISTS(SELECT 1 FROM edit_documents d WHERE d.work_id=r.work_id AND (d.active_id=r.id OR d.draft_id=r.id))
  AND NOT EXISTS(SELECT 1 FROM edit_revisions c WHERE c.work_id=r.work_id AND c.parent_id=r.id);
