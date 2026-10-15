"""Copy an existing piep library to a new application data directory.

Run with both applications closed. The old directory is never modified.
The source path is supplied by the user; no former application identifier is
embedded in a release.
"""

from __future__ import annotations

import argparse
from contextlib import closing
import json
import os
from pathlib import Path
import shutil
import sqlite3
import sys
import uuid


PATH_COLUMNS = {
    "downloads": ("cover_path", "json_path", "original_json_path"),
    "assets": ("local_path",),
    "download_versions": ("json_path", "original_json_path"),
    "people": ("icon_path", "cover_path"),
    "series": ("cover_path",),
    "entity_versions": ("json_path",),
}
JSON_COLUMNS = {
    "update_job_items": ("payload_json",),
    "collection_suggestions": ("members_json",),
}
SKIP_ROOT_FILES = {"piep.db", "piep.db-wal", "piep.db-shm", ".piep-library.lock"}


def is_link(path: Path) -> bool:
    return path.is_symlink() or (hasattr(os.path, "isjunction") and os.path.isjunction(path))


def validate(source: Path, destination: Path) -> None:
    if not source.is_absolute() or not destination.is_absolute():
        raise ValueError("Use absolute source and destination paths")
    if not source.is_dir() or is_link(source):
        raise ValueError("Source must be an ordinary existing directory")
    if not (source / "piep.db").is_file():
        raise ValueError("Source has no piep.db")
    if destination.exists() or is_link(destination):
        raise ValueError("Destination must not exist")
    if not destination.parent.is_dir():
        raise ValueError("Destination parent must exist")
    source_abs = source.resolve()
    destination_abs = destination.absolute()
    if source_abs == destination_abs or source_abs in destination_abs.parents or destination_abs in source_abs.parents:
        raise ValueError("Source and destination must not contain one another")
    with closing(sqlite3.connect((source / "piep.db").as_uri() + "?mode=ro", uri=True)) as db:
        columns = {row[1] for row in db.execute("PRAGMA table_info(downloads)")}
        if not {"id", "source", "source_id", "json_path"}.issubset(columns):
            raise ValueError("Source database is not a recognized piep library")


def inventory(source: Path) -> tuple[int, int]:
    count = size = 0
    for base, dirs, files in os.walk(source, followlinks=False):
        for name in dirs + files:
            path = Path(base) / name
            if is_link(path):
                raise ValueError("Source contains a link or junction")
        for name in files:
            path = Path(base) / name
            count += 1
            size += path.stat().st_size
    return count, size


def acquire_lock(source: Path):
    lock_path = source / ".piep-library.lock"
    if not lock_path.is_file() or is_link(lock_path):
        raise ValueError("Close the old app and check its library lock file")
    handle = lock_path.open("r+b")
    try:
        if os.name == "nt":
            import msvcrt

            handle.seek(0)
            msvcrt.locking(handle.fileno(), msvcrt.LK_NBLCK, 1)
        else:
            import fcntl

            fcntl.flock(handle.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
    except OSError as exc:
        handle.close()
        raise ValueError("The old library is in use; close the app first") from exc
    return handle


def replace_prefix(value: str, source: Path, destination: Path) -> str:
    old = str(source.absolute())
    new = str(destination.absolute())
    variants = (
        (old, new),
        (old.replace("\\", "/"), new.replace("\\", "/")),
        ("\\\\?\\" + old, "\\\\?\\" + new),
    )
    for old_path, new_path in variants:
        if value[: len(old_path)].casefold() == old_path.casefold():
            suffix = value[len(old_path) :]
            if not suffix or suffix[0] in ("\\", "/"):
                return new_path + suffix
    return value


def rewrite_json(value, source: Path, destination: Path):
    if isinstance(value, str):
        return replace_prefix(value, source, destination)
    if isinstance(value, list):
        return [rewrite_json(item, source, destination) for item in value]
    if isinstance(value, dict):
        return {key: rewrite_json(item, source, destination) for key, item in value.items()}
    return value


def copy_files(source: Path, stage: Path) -> None:
    for base, dirs, files in os.walk(source, followlinks=False):
        relative = Path(base).relative_to(source)
        (stage / relative).mkdir(parents=True, exist_ok=True)
        for name in dirs:
            if is_link(Path(base) / name):
                raise ValueError("Source changed during migration")
            (stage / relative / name).mkdir(exist_ok=True)
        for name in files:
            if is_link(Path(base) / name):
                raise ValueError("Source changed during migration")
            if relative == Path(".") and name in SKIP_ROOT_FILES:
                continue
            shutil.copy2(Path(base) / name, stage / relative / name)


def rewrite_database(db_path: Path, source: Path, destination: Path) -> dict[str, int]:
    counts: dict[str, int] = {}
    with closing(sqlite3.connect(db_path)) as db:
        db.execute("PRAGMA foreign_keys=ON")
        for table, columns in PATH_COLUMNS.items():
            available = {row[1] for row in db.execute(f'PRAGMA table_info("{table}")')}
            for column in columns:
                if column not in available:
                    continue
                changed = 0
                rows = db.execute(f'SELECT rowid, "{column}" FROM "{table}" WHERE "{column}" IS NOT NULL').fetchall()
                for rowid, value in rows:
                    updated = replace_prefix(value, source, destination)
                    if updated != value:
                        db.execute(f'UPDATE "{table}" SET "{column}"=? WHERE rowid=?', (updated, rowid))
                        changed += 1
                counts[f"{table}.{column}"] = changed
        for table, columns in JSON_COLUMNS.items():
            available = {row[1] for row in db.execute(f'PRAGMA table_info("{table}")')}
            for column in columns:
                if column not in available:
                    continue
                changed = 0
                rows = db.execute(f'SELECT rowid, "{column}" FROM "{table}" WHERE "{column}" IS NOT NULL').fetchall()
                for rowid, value in rows:
                    try:
                        parsed = json.loads(value)
                    except (TypeError, ValueError):
                        continue
                    updated = rewrite_json(parsed, source, destination)
                    if updated != parsed:
                        db.execute(f'UPDATE "{table}" SET "{column}"=? WHERE rowid=?', (json.dumps(updated, ensure_ascii=False), rowid))
                        changed += 1
                counts[f"{table}.{column}"] = changed
        if db.execute("PRAGMA foreign_key_check").fetchone():
            raise ValueError("Migrated database has broken foreign keys")
        if db.execute("PRAGMA integrity_check").fetchone()[0] != "ok":
            raise ValueError("Migrated database failed integrity check")
        db.commit()
    return counts


def migrate(source: Path, destination: Path) -> dict[str, int]:
    validate(source, destination)
    lock = acquire_lock(source)
    stage = destination.parent / f".{destination.name}.migration-{uuid.uuid4().hex}"
    try:
        stage.mkdir()
        copy_files(source, stage)
        with closing(sqlite3.connect((source / "piep.db").as_uri() + "?mode=ro", uri=True)) as old_db:
            with closing(sqlite3.connect(stage / "piep.db")) as new_db:
                old_db.backup(new_db)
        counts = rewrite_database(stage / "piep.db", source, destination)
        settings = stage / "settings.json"
        if settings.is_file():
            original = json.loads(settings.read_text(encoding="utf-8"))
            updated = rewrite_json(original, source, destination)
            if updated != original:
                settings.write_text(json.dumps(updated, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        stage.rename(destination)
        return counts
    finally:
        lock.close()
        if stage.exists():
            if stage.parent.resolve() != destination.parent.resolve() or not stage.name.startswith(f".{destination.name}.migration-"):
                raise RuntimeError("Unexpected staging path; manual cleanup required")
            shutil.rmtree(stage)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path, help="Existing app data directory shown by the old app")
    parser.add_argument("destination", type=Path, help="New app data directory")
    parser.add_argument("--execute", action="store_true", help="Copy and migrate; default is inspection only")
    args = parser.parse_args()
    try:
        validate(args.source, args.destination)
        count, size = inventory(args.source)
        free = shutil.disk_usage(args.destination.parent).free
        if free < size * 2:
            raise ValueError("Insufficient free space for a safe copied migration")
        print(f"files={count} source_bytes={size} free_bytes={free}")
        if not args.execute:
            print("Inspection only; nothing was changed")
            return 0
        counts = migrate(args.source, args.destination)
        print(f"Migration complete; database_path_updates={sum(counts.values())}")
        print("Keep the old directory until the new app and library have been verified")
        return 0
    except (OSError, sqlite3.Error, ValueError) as exc:
        print(f"Migration failed: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
