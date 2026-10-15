-- Keep the user's decisions and IDs; only repair derived acquisition state.
-- Idempotent and executed in the schema transaction, including older libraries.
CREATE INDEX IF NOT EXISTS idx_update_job_items_source_state ON update_job_items(source, source_id, status, id);
INSERT INTO update_candidates
    (source, source_id, kind, title, payload_json, target_type, status, first_seen_at, updated_at)
SELECT i.source, i.source_id,
       CASE WHEN EXISTS (SELECT 1 FROM downloads d WHERE d.source=i.source AND d.source_id=i.source_id)
            THEN 'revision' ELSE 'new' END,
       i.title,
       json_set(CASE WHEN json_valid(i.payload_json) THEN i.payload_json ELSE '{}' END,
           '$.holdReason', replace(i.error, '支援中のアカウントでFANBOXをつなぎ直してください', '支援プランや連携アカウントの変更後に、保留一覧から再確認してください'), '$.holdKind', 'restricted'),
       i.target_type, 'held', i.created_at, i.updated_at
FROM update_job_items i
WHERE i.source='fanbox' AND i.source_id IS NOT NULL AND i.status='failed'
  AND i.error LIKE '%[閲覧制限]%'
  AND i.id=(SELECT MAX(j.id) FROM update_job_items j WHERE j.source=i.source AND j.source_id=i.source_id
            AND j.status='failed' AND j.error LIKE '%[閲覧制限]%')
ON CONFLICT(source, source_id) DO UPDATE SET
    status=CASE WHEN update_candidates.status='dismissed' THEN 'dismissed' ELSE 'held' END,
    payload_json=json_set(CASE WHEN json_valid(update_candidates.payload_json) THEN update_candidates.payload_json ELSE '{}' END,
        '$.holdReason', json_extract(excluded.payload_json, '$.holdReason'), '$.holdKind', 'restricted');

UPDATE update_job_items SET status='held', error=replace(error,
    '支援中のアカウントでFANBOXをつなぎ直してください', '支援プランや連携アカウントの変更後に、保留一覧から再確認してください')
WHERE source='fanbox' AND status='failed' AND error LIKE '%[閲覧制限]%';

UPDATE update_job_items SET payload_json=json_set(json_remove(payload_json,
    '$.originalData.localVersion', '$.originalData.localSavedAt'), '$.kind', 'new', '$.subtitle', '未保存')
WHERE status IN ('candidate', 'failed', 'held', 'queued') AND json_valid(payload_json)
  AND json_extract(payload_json, '$.kind')='revision'
  AND NOT EXISTS (SELECT 1 FROM downloads d WHERE d.source=update_job_items.source AND d.source_id=update_job_items.source_id);

UPDATE update_candidates SET kind='new',
    target_type=CASE WHEN target_type='work' THEN NULL ELSE target_type END,
    payload_json=json_set(json_remove(CASE WHEN json_valid(payload_json) THEN payload_json ELSE '{}' END,
        '$.originalData.localVersion', '$.originalData.localSavedAt'), '$.kind', 'new', '$.subtitle', '未保存')
WHERE kind='revision' AND NOT EXISTS
    (SELECT 1 FROM downloads d WHERE d.source=update_candidates.source AND d.source_id=update_candidates.source_id);

-- Every deletion path (single, bulk, restore) obeys the same invariant.
CREATE TRIGGER IF NOT EXISTS normalize_deleted_revision_candidate
AFTER DELETE ON downloads BEGIN
    UPDATE update_candidates SET kind='new',
        target_type=CASE WHEN target_type='work' THEN NULL ELSE target_type END,
        payload_json=json_set(json_remove(CASE WHEN json_valid(payload_json) THEN payload_json ELSE '{}' END,
            '$.originalData.localVersion', '$.originalData.localSavedAt'), '$.kind', 'new', '$.subtitle', '未保存')
    WHERE source=OLD.source AND source_id=OLD.source_id AND kind='revision';
END;

UPDATE update_jobs SET
    error_count=(SELECT COUNT(*) FROM update_job_items i WHERE i.job_id=update_jobs.id AND i.status='failed'),
    status=CASE WHEN status='failed' AND NOT EXISTS
        (SELECT 1 FROM update_job_items i WHERE i.job_id=update_jobs.id AND i.status='failed')
        THEN 'completed' ELSE status END,
    active_label=CASE WHEN status='failed' AND NOT EXISTS
        (SELECT 1 FROM update_job_items i WHERE i.job_id=update_jobs.id AND i.status='failed')
        THEN '完了しました（閲覧制限の投稿は保留）' ELSE active_label END
WHERE EXISTS (SELECT 1 FROM update_job_items i WHERE i.job_id=update_jobs.id AND i.status='held');
