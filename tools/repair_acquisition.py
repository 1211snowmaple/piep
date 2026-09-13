"""Back up and repair derived acquisition state. Close piep before --apply.

No credentials, saved works, attachments, or logs are deleted. The same SQL is
used by application startup. By default this is a read-only inventory.
"""
import argparse
from datetime import datetime, timezone
import json
import hashlib
from pathlib import Path
import re
import sqlite3


def inventory(connection):
    digest = hashlib.sha256()
    for table in ("downloads", "download_versions", "assets"):
        digest.update(table.encode())
        for row in connection.execute(f"SELECT * FROM {table} ORDER BY id"):
            digest.update(json.dumps(row, ensure_ascii=False).encode("utf-8"))
    return {
        "saved_records_digest": digest.hexdigest(),
        "downloads": connection.execute("SELECT COUNT(*) FROM downloads").fetchone()[0],
        "versions": connection.execute("SELECT COUNT(*) FROM download_versions").fetchone()[0],
        "candidates": dict(connection.execute("SELECT status,COUNT(*) FROM update_candidates GROUP BY status")),
        "restricted_errors": connection.execute("SELECT COUNT(*) FROM update_job_items WHERE source='fanbox' AND status='failed' AND error LIKE '%[閲覧制限]%' ").fetchone()[0],
        "orphan_revisions": connection.execute("SELECT COUNT(*) FROM update_candidates c WHERE kind='revision' AND NOT EXISTS (SELECT 1 FROM downloads d WHERE d.source=c.source AND d.source_id=c.source_id)").fetchone()[0],
        "page_count": connection.execute("PRAGMA page_count").fetchone()[0],
        "free_pages": connection.execute("PRAGMA freelist_count").fetchone()[0],
        "quick_check": connection.execute("PRAGMA quick_check").fetchone()[0],
        "foreign_key_errors": len(connection.execute("PRAGMA foreign_key_check").fetchall()),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("database", type=Path)
    parser.add_argument("--apply", action="store_true")
    parser.add_argument("--known-creator", help="Creator identity independently verified for --source-ids only")
    parser.add_argument("--source-ids", nargs="*", default=[])
    args = parser.parse_args()
    database = args.database.resolve(strict=True)
    if database.name != "piep.db":
        raise SystemExit("Expected an explicitly named piep.db")
    with sqlite3.connect(database.as_uri() + "?mode=ro", uri=True, timeout=10) as read:
        before = inventory(read)
        if not args.apply:
            print(json.dumps(before, ensure_ascii=False, indent=2))
            return
        if before["quick_check"] != "ok" or before["foreign_key_errors"]:
            raise SystemExit("Integrity check failed; refusing mutation")
        if read.execute("SELECT COUNT(*) FROM update_jobs WHERE status IN ('queued','running','canceling')").fetchone()[0]:
            raise SystemExit("Active jobs exist; finish or pause them first")
        stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%S%fZ")
        backup = database.parent / ("backup-before-acquisition-" + stamp) / "piep.db"
        backup.parent.mkdir(parents=False, exist_ok=False)
        with sqlite3.connect(backup) as destination:
            read.backup(destination)
            if inventory(destination)["quick_check"] != "ok":
                raise SystemExit("Backup verification failed; refusing mutation")

    sql = (Path(__file__).resolve().parents[1] / "src-tauri/src/database/acquisition_repair.sql").read_text(encoding="utf-8")
    with sqlite3.connect(database, timeout=10) as connection:
        connection.execute("PRAGMA foreign_keys=ON")
        try:
            connection.executescript("BEGIN IMMEDIATE;\n" + sql)
            # Old failure messages predate structured permission metadata.
            for source_id, raw in connection.execute("SELECT source_id,payload_json FROM update_candidates WHERE source='fanbox' AND status IN ('held','dismissed')").fetchall():
                payload = json.loads(raw)
                if payload.get("holdKind") != "restricted":
                    continue
                original = payload.get("originalData") or {}
                requirement = re.search(r"月額(\d+)円", payload.get("holdReason", ""))
                if requirement:
                    original.setdefault("feeRequired", int(requirement.group(1)))
                if args.known_creator and source_id in args.source_ids:
                    original["creatorId"] = args.known_creator
                payload["originalData"] = original
                connection.execute("UPDATE update_candidates SET payload_json=? WHERE source='fanbox' AND source_id=?", (json.dumps(payload, ensure_ascii=False), source_id))
            after = inventory(connection)
            if after["saved_records_digest"] != before["saved_records_digest"]:
                raise RuntimeError("Saved data count changed")
            if after["quick_check"] != "ok" or after["foreign_key_errors"]:
                raise RuntimeError("Post-repair integrity check failed")
            connection.commit()
        except BaseException:
            connection.rollback()
            raise
        connection.execute("PRAGMA optimize")
        connection.execute("PRAGMA wal_checkpoint(TRUNCATE)")
        connection.execute("VACUUM")
        connection.execute("PRAGMA wal_checkpoint(TRUNCATE)")
        after = inventory(connection)
        if after["saved_records_digest"] != before["saved_records_digest"] or after["quick_check"] != "ok" or after["foreign_key_errors"]:
            raise RuntimeError("Post-optimization verification failed; backup is at " + str(backup))
    print(json.dumps({"backup": str(backup), "before": before, "after": after}, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
