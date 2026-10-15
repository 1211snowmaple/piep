-- Replacement ordering indexes and UNIQUE constraints already cover these
-- keys. Keep the constraints and their sqlite_autoindex_* indexes intact.
-- Never recreate the superseded indexes on later startup.
DROP INDEX IF EXISTS idx_downloads_source_type_date;
DROP INDEX IF EXISTS idx_downloads_favorite_date;
DROP INDEX IF EXISTS idx_downloads_author_nocase;
DROP INDEX IF EXISTS idx_downloads_date;
DROP INDEX IF EXISTS idx_downloads_size;
DROP INDEX IF EXISTS idx_downloads_source_id;
DROP INDEX IF EXISTS idx_downloads_text_length;
DROP INDEX IF EXISTS idx_downloads_title;
DROP INDEX IF EXISTS idx_downloads_watch_date;
DROP INDEX IF EXISTS idx_tags_name;
DROP INDEX IF EXISTS idx_people_source_key;
DROP INDEX IF EXISTS idx_series_source_key;
DROP INDEX IF EXISTS idx_work_edit_blocks_revision_order;
