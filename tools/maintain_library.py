"""Offline, non-networked library audit and conservative maintenance.

Default: read-only audit. --apply: acquire piep's process lock, create/verify a
SQLite online backup, retire known redundant indexes, refresh planner statistics
and compact. Remove ONLY wholly empty numeric work trees without a saved work
or any DB file reference. Never delete a database row or a nonempty file.
The backup manifest records each removed empty directory for reconstruction.
"""
from contextlib import contextmanager
from datetime import datetime, timezone
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import sqlite3
import stat
import sys
import time


PATH_COLUMNS = {
    "downloads": ("json_path", "original_json_path", "cover_path"),
    "download_versions": ("json_path", "original_json_path"),
    "assets": ("local_path",),
    "people": ("icon_path", "cover_path"),
    "series": ("cover_path",),
    "entity_versions": ("json_path",),
    "work_collections": ("cover_image_path",),
}
CHECKS = {
    "missing_current_versions": "SELECT COUNT(*) FROM downloads d WHERE NOT EXISTS (SELECT 1 FROM download_versions v WHERE v.download_id=d.id AND v.version=d.current_version)",
    "current_version_path_mismatch": "SELECT COUNT(*) FROM downloads d JOIN download_versions v ON v.download_id=d.id AND v.version=d.current_version WHERE d.json_path IS NOT v.json_path OR d.content_hash IS NOT v.content_hash",
    "missing_people": "SELECT COUNT(*) FROM download_people r WHERE NOT EXISTS (SELECT 1 FROM people p WHERE p.source=r.person_source AND p.source_key=r.person_key)",
    "missing_series": "SELECT COUNT(*) FROM download_series r WHERE NOT EXISTS (SELECT 1 FROM series s WHERE s.source=r.series_source AND s.source_key=r.series_key)",
    "stale_search_state": "SELECT COUNT(*) FROM search_index_state s JOIN downloads d ON d.id=s.download_id WHERE s.current_version != d.current_version OR s.content_hash IS NOT d.content_hash",
    "stale_semantic_state": "SELECT COUNT(*) FROM semantic_index_state s JOIN downloads d ON d.id=s.download_id WHERE s.current_version != d.current_version OR s.content_hash IS NOT d.content_hash",
    "unreferenced_tags_preserved": "SELECT COUNT(*) FROM tags t WHERE NOT EXISTS (SELECT 1 FROM download_tags r WHERE r.tag_id=t.id)",
    "duplicate_work_keys": "SELECT COUNT(*) FROM (SELECT source,source_id FROM downloads GROUP BY source,source_id HAVING COUNT(*)>1)",
    "orphan_revisions": "SELECT COUNT(*) FROM update_candidates c WHERE kind='revision' AND NOT EXISTS (SELECT 1 FROM downloads d WHERE d.source=c.source AND d.source_id=c.source_id)",
    "unresolved_collection_members_preserved": "SELECT COUNT(*) FROM work_collection_members WHERE download_id IS NULL",
}
BENCHMARKS = {
    "recent": "SELECT id,title FROM downloads ORDER BY downloaded_at DESC,id DESC LIMIT 60",
    "pixiv_recent": "SELECT id,title FROM downloads WHERE source='pixiv' AND content_type='novel' ORDER BY downloaded_at DESC,id DESC LIMIT 60",
    "title": "SELECT id,title FROM downloads ORDER BY title COLLATE NOCASE DESC,id DESC LIMIT 60",
    "author": "SELECT id,author_name FROM downloads ORDER BY author_name COLLATE NOCASE DESC,id DESC LIMIT 60",
    "length": "SELECT id,text_length FROM downloads ORDER BY text_length DESC,id DESC LIMIT 60",
    "size": "SELECT id,file_size_bytes FROM downloads ORDER BY file_size_bytes DESC,id DESC LIMIT 60",
    "favorite": "SELECT id,title FROM downloads WHERE favorite=1 ORDER BY downloaded_at DESC,id DESC LIMIT 60",
    "watched": "SELECT id,title FROM downloads WHERE watch_updates=1 ORDER BY downloaded_at DESC,id DESC LIMIT 60",
}


def quote(name):
    return '"' + name.replace('"', '""') + '"'


def digest_rows(connection):
    """All logical rows, not just works; planner statistics are derived only."""
    tables = [row[0] for row in connection.execute(
        "SELECT name FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_stat%' ORDER BY name"
    )]
    result = {}
    for table in tables:
        columns = list(connection.execute(f"PRAGMA table_info({quote(table)})"))
        order = ",".join(quote(row[1]) for row in columns)
        digest = hashlib.sha256()
        count = 0
        for row in connection.execute(f"SELECT * FROM {quote(table)} ORDER BY {order}"):
            encoded = json.dumps(row, ensure_ascii=False, default=lambda b: {"blob_hex": b.hex()})
            digest.update(encoded.encode("utf-8") + b"\n")
            count += 1
        result[table] = {"rows": count, "sha256": digest.hexdigest()}
    return result


def normalized(path):
    text = str(path)
    if text.startswith("\\\\?\\UNC\\"):
        text = "\\\\" + text[8:]
    elif text.startswith("\\\\?\\"):
        text = text[4:]
    return Path(os.path.abspath(text))


def is_link(path):
    info = path.lstat()
    return stat.S_ISLNK(info.st_mode) or bool(getattr(info, "st_file_attributes", 0) & 0x400)


def assert_plain_descendant(path, root):
    root = normalized(root)
    path = normalized(path)
    relative = path.relative_to(root)
    if not relative.parts:
        raise ValueError("Refusing operation on the storage root")
    for part in (path, *path.parents):
        if part.exists() and is_link(part):
            raise ValueError(f"Refusing linked/reparse path: {part}")
    if not path.resolve().is_relative_to(root.resolve()):
        raise ValueError("Resolved path escapes the named storage root")
    return path


def referenced_paths(connection):
    paths = set()
    for table, columns in PATH_COLUMNS.items():
        for column in columns:
            for (raw,) in connection.execute(f"SELECT {quote(column)} FROM {quote(table)} WHERE {quote(column)} IS NOT NULL"):
                if raw.strip():
                    paths.add(normalized(raw))
    return paths


def empty_work_directories(connection, storage, references):
    """Never infer an unsaved work merely from an empty folder."""
    saved = set(connection.execute("SELECT source,source_id FROM downloads"))
    removable = []
    for source in ("pixiv", "fanbox"):
        provider = storage / source
        if not provider.exists():
            continue
        assert_plain_descendant(provider, storage)
        for work in sorted(provider.iterdir()):
            if not work.name.isascii() or not work.name.isdecimal() or not work.is_dir() or is_link(work):
                continue
            if (source, work.name) in saved:
                continue
            if any(path == work or path.is_relative_to(work) for path in references):
                continue
            directories = []
            pending = [work]
            empty = True
            while pending:
                directory = pending.pop()
                assert_plain_descendant(directory, storage)
                directories.append(directory)
                for child in directory.iterdir():
                    if is_link(child) or not child.is_dir():
                        empty = False
                        break
                    pending.append(child)
                if not empty:
                    break
            if empty:
                removable.extend(directories)
    return sorted(removable, key=lambda path: (-len(path.parts), str(path)))


def remove_empty_directories(paths, storage):
    removed = []
    for path in paths:
        safe_path = assert_plain_descendant(path, storage)
        # rmdir, never recursive removal: a new file appearing fails closed.
        safe_path.rmdir()
        removed.append(str(safe_path.relative_to(storage)))
    return removed


def file_inventory(root):
    """Manifest of names/sizes/mtimes, not a claim to hash every media byte."""
    digest = hashlib.sha256()
    count = size = 0
    for category in ("downloads/pixiv", "downloads/fanbox", "profiles", "series", "collection-covers"):
        base = root / category
        if not base.exists():
            continue
        assert_plain_descendant(base, root)
        for directory, dirs, files in os.walk(base, followlinks=False):
            dirs[:] = sorted(name for name in dirs if not is_link(Path(directory) / name))
            for name in sorted(files):
                path = Path(directory) / name
                info = path.lstat()
                record = (str(path.relative_to(root)), info.st_size, info.st_mtime_ns, info.st_mode)
                digest.update(json.dumps(record, ensure_ascii=False).encode("utf-8") + b"\n")
                count += 1
                size += info.st_size
    return {"files": count, "bytes": size, "path_size_mtime_sha256": digest.hexdigest()}


def audit(connection, database, deep=False):
    references = referenced_paths(connection)
    print(f"Auditing {len(references)} unique file references...", file=sys.stderr, flush=True)
    issues = {"missing": [], "empty": [], "invalid_json": [], "size_mismatch": [], "unsafe": []}
    for position, path in enumerate(sorted(references), 1):
        if position % 10000 == 0:
            print(f"Checked {position}/{len(references)} file references", file=sys.stderr, flush=True)
        try:
            assert_plain_descendant(path, database.parent)
            info = path.stat()
        except FileNotFoundError:
            issues["missing"].append(str(path))
            continue
        except (ValueError, OSError):
            issues["unsafe"].append(str(path))
            continue
        if info.st_size == 0:
            issues["empty"].append(str(path))
        elif deep and path.suffix.lower() == ".json":
            try:
                with path.open(encoding="utf-8-sig") as stream:
                    json.load(stream)
            except (OSError, ValueError):
                issues["invalid_json"].append(str(path))
    for raw, expected in connection.execute("SELECT local_path,file_size_bytes FROM assets WHERE file_size_bytes > 0"):
        path = normalized(raw)
        if path in references and path.exists() and str(path) not in issues["unsafe"]:
            if path.stat().st_size != expected:
                issues["size_mismatch"].append(str(path))
    invalid_json = {}
    for table, in connection.execute("SELECT name FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%'"):
        for column in connection.execute(f"PRAGMA table_info({quote(table)})"):
            if column[1].endswith("_json"):
                count = connection.execute(f"SELECT COUNT(*) FROM {quote(table)} WHERE {quote(column[1])} IS NOT NULL AND NOT json_valid({quote(column[1])})").fetchone()[0]
                if count:
                    invalid_json[f"{table}.{column[1]}"] = count
    print("Checking table integrity, row digests, and empty work trees...", file=sys.stderr, flush=True)
    return {
        "database_bytes": database.stat().st_size,
        "integrity_check": [r[0] for r in connection.execute("PRAGMA integrity_check")],
        "foreign_key_errors": list(connection.execute("PRAGMA foreign_key_check")),
        "tables": digest_rows(connection),
        "relations": {name: connection.execute(sql).fetchone()[0] for name, sql in CHECKS.items()},
        "invalid_database_json": invalid_json,
        "unique_file_references": len(references),
        "file_issue_counts": {name: len(paths) for name, paths in issues.items()},
        "file_issues": issues,
        "files": file_inventory(database.parent),
        "removable_empty_directories": [str(path.relative_to(database.parent / "downloads")) for path in empty_work_directories(connection, database.parent / "downloads", references)],
        "free_pages": connection.execute("PRAGMA freelist_count").fetchone()[0],
        "indexes": [r[0] for r in connection.execute("SELECT name FROM sqlite_schema WHERE type='index' ORDER BY name")],
    }


def benchmark(connection):
    result = {}
    for name, sql in BENCHMARKS.items():
        connection.execute(sql).fetchall()
        samples = []
        for _ in range(21):
            start = time.perf_counter()
            connection.execute(sql).fetchall()
            samples.append((time.perf_counter() - start) * 1000)
        result[name] = {"median_ms": round(sorted(samples)[10], 4), "plan": [r[3] for r in connection.execute("EXPLAIN QUERY PLAN " + sql)]}
    return result


@contextmanager
def library_lock(root):
    # Same file and overlapping lock range as Rust File::try_lock(). Never
    # unlink this file; the OS releases ownership when the handle closes.
    path = root / ".piep-library.lock"
    assert_plain_descendant(path, root)
    with path.open("a+b") as handle:
        handle.seek(0)
        try:
            if os.name == "nt":
                import msvcrt
                msvcrt.locking(handle.fileno(), msvcrt.LK_NBLCK, 1)
            else:
                import fcntl
                fcntl.flock(handle.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError as error:
            raise RuntimeError("piep is using this library. Close it before maintenance.") from error
        try:
            yield
        finally:
            handle.seek(0)
            if os.name == "nt":
                msvcrt.locking(handle.fileno(), msvcrt.LK_UNLCK, 1)
            else:
                fcntl.flock(handle.fileno(), fcntl.LOCK_UN)


def write_report(path, report):
    with path.open("x", encoding="utf-8") as stream:
        json.dump(report, stream, ensure_ascii=False, indent=2)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("database", type=Path)
    parser.add_argument("--apply", action="store_true")
    parser.add_argument("--deep", action="store_true", help="Also parse every referenced JSON document")
    parser.add_argument("--report", type=Path, help="Create a new detailed audit report (no overwrite)")
    args = parser.parse_args()
    database = normalized(args.database)
    if database.name != "piep.db" or not database.is_file():
        raise SystemExit("Expected an existing, explicitly named piep.db")
    assert_plain_descendant(database, database.parent)
    if not args.apply:
        with sqlite3.connect(database.as_uri() + "?mode=ro", uri=True) as connection:
            connection.execute("BEGIN")
            report = audit(connection, database, args.deep)
            report["benchmark"] = benchmark(connection)
    else:
        with library_lock(database.parent), sqlite3.connect(database) as connection:
            connection.execute("PRAGMA foreign_keys=ON")
            before = audit(connection, database, args.deep)
            if before["integrity_check"] != ["ok"] or before["foreign_key_errors"]:
                raise RuntimeError("Database integrity failed; refusing maintenance")
            for table in ("download_save_journal", "restore_journal"):
                if before["tables"][table]["rows"]:
                    raise RuntimeError("Unfinished recovery journal; use piep recovery first")
            if connection.execute("SELECT COUNT(*) FROM update_jobs WHERE status IN ('queued','running','canceling')").fetchone()[0]:
                raise RuntimeError("Unfinished acquisition jobs; refusing maintenance")
            if shutil.disk_usage(database.parent).free < database.stat().st_size * 3 + 256 * 1024 * 1024:
                raise RuntimeError("Insufficient free space for backup and compaction")
            backup_dir = database.parent / ("backup-before-maintenance-" + datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%S%fZ"))
            backup_dir.mkdir()
            with sqlite3.connect(backup_dir / "piep.db") as backup:
                connection.backup(backup)
                if digest_rows(backup) != before["tables"] or backup.execute("PRAGMA integrity_check").fetchall() != [("ok",)]:
                    raise RuntimeError("Backup verification failed")
            write_report(backup_dir / "before.json", before)
            timings_before = benchmark(connection)
            sql = (Path(__file__).resolve().parents[1] / "src-tauri/src/database/index_cleanup.sql").read_text(encoding="utf-8")
            # Verify expected replacement indexes exist before retiring old keys.
            required = {"idx_downloads_" + suffix for suffix in ("source_type_date_id", "favorite_date_id", "author_id_sort", "date_id", "size_id", "text_length_id", "title_id", "watch_date_id")}
            if not required.issubset(before["indexes"]):
                raise RuntimeError("Replacement indexes missing; run the latest piep first")
            connection.executescript("BEGIN IMMEDIATE;\n" + sql)
            if digest_rows(connection) != before["tables"]:
                connection.rollback()
                raise RuntimeError("Logical rows changed; rolled back")
            connection.commit()
            connection.execute("ANALYZE")
            connection.commit()
            connection.execute("PRAGMA wal_checkpoint(TRUNCATE)")
            connection.execute("VACUUM")
            connection.execute("PRAGMA optimize")
            connection.execute("PRAGMA wal_checkpoint(TRUNCATE)")
            # The complete planned directory list is durably recorded before
            # removal; a crash never loses the information needed to recreate it.
            directories = [database.parent / "downloads" / p for p in before["removable_empty_directories"]]
            removed = remove_empty_directories(directories, database.parent / "downloads")
            after = audit(connection, database, args.deep)
            if before["tables"] != after["tables"] or before["files"] != after["files"]:
                raise RuntimeError("Post-maintenance manifest mismatch; backup: " + str(backup_dir))
            if after["integrity_check"] != ["ok"] or after["foreign_key_errors"]:
                raise RuntimeError("Post-maintenance integrity failure; backup: " + str(backup_dir))
            report = {"backup": str(backup_dir), "removed_empty_directories": removed, "before": before, "after": after, "benchmark_before": timings_before, "benchmark_after": benchmark(connection)}
            write_report(backup_dir / "after.json", report)
    if args.report:
        write_report(args.report, report)
    summary = report.get("after", report)
    print(json.dumps({"backup": report.get("backup"), "tables": len(summary["tables"]), "integrity": summary["integrity_check"], "relations": summary["relations"], "file_issue_counts": summary["file_issue_counts"], "files": summary["files"], "empty_directories": len(summary["removable_empty_directories"]), "removed_empty_directories": len(report.get("removed_empty_directories", [])), "database_bytes": summary["database_bytes"]}, indent=2))


if __name__ == "__main__":
    main()
