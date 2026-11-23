-- 編集画面で選んだ画像を、保存するか期限を迎えるまで棚に残す。
CREATE TABLE pending_edit_images (
    id BLOB PRIMARY KEY NOT NULL CHECK(length(id)=16),
    file_id BLOB NOT NULL REFERENCES content_files(id),
    expires_at INTEGER NOT NULL
) STRICT;
CREATE INDEX pending_edit_images_expiry ON pending_edit_images(expires_at);
