-- 再移行で差し替える前の原記録にも FK を残す。未知の列も回収から守る。
ALTER TABLE execution_items ADD COLUMN resume_generation INTEGER CHECK(resume_generation>=0);
CREATE TABLE job_import_evidence (
    source_table TEXT NOT NULL CHECK(source_table IN ('update_jobs','update_job_items','update_job_logs')),
    file_id BLOB NOT NULL REFERENCES content_files(id),
    PRIMARY KEY(source_table,file_id)
) STRICT;
CREATE UNIQUE INDEX execution_item_origin ON execution_items(job_id,source_sequence) WHERE source_sequence IS NOT NULL;
CREATE UNIQUE INDEX execution_log_origin ON execution_logs(job_id,source_sequence) WHERE source_sequence IS NOT NULL;
