"""Safety checks for copying a library after the application identifier changes."""

from __future__ import annotations

import importlib.util
from contextlib import closing
import json
from pathlib import Path
import sqlite3
import tempfile
import unittest


SPEC = importlib.util.spec_from_file_location(
    "migrate_app_data", Path(__file__).with_name("migrate-app-data.py")
)
assert SPEC and SPEC.loader
migration = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(migration)


class MigrationTests(unittest.TestCase):
    def test_copies_library_and_rewrites_only_paths_inside_it(self) -> None:
        with tempfile.TemporaryDirectory(prefix="piep-migration-test-") as temporary:
            root = Path(temporary)
            source = root / "old-app"
            destination = root / "new-app"
            source.mkdir()
            (source / ".piep-library.lock").write_bytes(b"0")
            (source / "downloads").mkdir()
            (source / "downloads" / "book.txt").write_text("example", encoding="utf-8")
            inside = str(source / "downloads" / "book.txt")
            outside = str(root / "old-app-other" / "book.txt")
            (source / "settings.json").write_text(
                json.dumps({"inside": inside, "outside": outside}), encoding="utf-8"
            )
            with closing(sqlite3.connect(source / "piep.db")) as db, db:
                db.executescript(
                    "CREATE TABLE downloads (id INTEGER PRIMARY KEY, source TEXT, "
                    "source_id TEXT, json_path TEXT, cover_path TEXT);"
                    "CREATE TABLE assets (local_path TEXT);"
                    "CREATE TABLE update_job_items (payload_json TEXT);"
                )
                db.execute(
                    "INSERT INTO downloads VALUES (1, 'sample', '123456', ?, ?)",
                    (inside, outside),
                )
                db.execute("INSERT INTO assets VALUES (?)", ("\\\\?\\" + inside,))
                db.execute(
                    "INSERT INTO update_job_items VALUES (?)",
                    (json.dumps({"paths": [inside, outside]}),),
                )

            original_bytes = (source / "piep.db").read_bytes()
            counts = migration.migrate(source, destination)

            self.assertFalse((destination / ".piep-library.lock").exists())
            self.assertEqual((destination / "downloads" / "book.txt").read_text(), "example")
            self.assertEqual((source / "piep.db").read_bytes(), original_bytes)
            self.assertEqual(sum(counts.values()), 3)
            with closing(sqlite3.connect(destination / "piep.db")) as db:
                row = db.execute("SELECT json_path, cover_path FROM downloads").fetchone()
                self.assertEqual(row, (str(destination / "downloads" / "book.txt"), outside))
                self.assertEqual(
                    db.execute("SELECT local_path FROM assets").fetchone()[0],
                    "\\\\?\\" + str(destination / "downloads" / "book.txt"),
                )
                payload = json.loads(db.execute("SELECT payload_json FROM update_job_items").fetchone()[0])
                self.assertEqual(payload["paths"], [str(destination / "downloads" / "book.txt"), outside])
            settings = json.loads((destination / "settings.json").read_text(encoding="utf-8"))
            self.assertEqual(settings, {"inside": str(destination / "downloads" / "book.txt"), "outside": outside})

    def test_rejects_overlapping_or_existing_destination(self) -> None:
        with tempfile.TemporaryDirectory(prefix="piep-migration-test-") as temporary:
            source = Path(temporary) / "old-app"
            source.mkdir()
            with closing(sqlite3.connect(source / "piep.db")) as db, db:
                db.execute("CREATE TABLE downloads (id INTEGER, source TEXT, source_id TEXT, json_path TEXT)")
            with self.assertRaisesRegex(ValueError, "must not contain"):
                migration.validate(source, source / "new-app")
            with self.assertRaisesRegex(ValueError, "must not exist"):
                migration.validate(source, Path(temporary))


if __name__ == "__main__":
    unittest.main()
