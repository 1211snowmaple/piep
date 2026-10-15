import importlib.util
from pathlib import Path
import sqlite3
import tempfile
import unittest


spec = importlib.util.spec_from_file_location("maintain_library", Path(__file__).with_name("maintain_library.py"))
maintenance = importlib.util.module_from_spec(spec)
spec.loader.exec_module(maintenance)


class MaintenanceSafetyTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.storage = self.root / "downloads"
        self.storage.mkdir()
        self.connection = sqlite3.connect(":memory:")
        self.connection.execute("CREATE TABLE downloads(source TEXT,source_id TEXT)")

    def tearDown(self):
        self.connection.close()
        self.temp.cleanup()

    def work(self, identifier):
        path = self.storage / "fanbox" / identifier
        path.mkdir(parents=True)
        return path

    def test_nested_empty_tree_removed_without_removing_provider(self):
        work = self.work("123")
        (work / "v1" / "assets").mkdir(parents=True)
        paths = maintenance.empty_work_directories(self.connection, self.storage, set())
        self.assertEqual(len(paths), 3)
        self.assertEqual(len(maintenance.remove_empty_directories(paths, self.storage)), 3)
        self.assertFalse(work.exists())
        self.assertTrue(work.parent.exists())

    def test_saved_empty_work_is_preserved_as_missing_data(self):
        work = self.work("123")
        self.connection.execute("INSERT INTO downloads VALUES('fanbox','123')")
        self.assertEqual(maintenance.empty_work_directories(self.connection, self.storage, set()), [])
        self.assertTrue(work.exists())

    def test_missing_file_reference_protects_empty_work(self):
        work = self.work("123")
        refs = {work / "v1" / "data.json"}
        self.assertEqual(maintenance.empty_work_directories(self.connection, self.storage, refs), [])

    def test_even_zero_byte_or_hidden_files_protect_work(self):
        work = self.work("123")
        (work / ".user-file").touch()
        self.assertEqual(maintenance.empty_work_directories(self.connection, self.storage, set()), [])

    def test_unrecognized_tree_is_never_removed(self):
        self.work("legacy-backup")
        self.work("１２３")
        self.assertEqual(maintenance.empty_work_directories(self.connection, self.storage, set()), [])

    def test_late_file_blocks_nonrecursive_removal(self):
        work = self.work("123")
        paths = maintenance.empty_work_directories(self.connection, self.storage, set())
        (work / "arrived.txt").write_text("preserve", encoding="utf-8")
        with self.assertRaises(OSError):
            maintenance.remove_empty_directories(paths, self.storage)
        self.assertEqual((work / "arrived.txt").read_text(encoding="utf-8"), "preserve")

    def test_root_and_outside_paths_rejected(self):
        outside = self.root / "outside"
        outside.mkdir()
        for path in (self.storage, outside, self.storage / ".." / "outside"):
            with self.assertRaises(ValueError):
                maintenance.remove_empty_directories([path], self.storage)
            self.assertTrue(path.exists())

    def test_symlink_tree_is_not_followed(self):
        outside = self.root / "outside"
        outside.mkdir()
        work = self.work("123")
        try:
            (work / "link").symlink_to(outside, target_is_directory=True)
        except OSError:
            self.skipTest("Creating symlinks requires Windows developer mode")
        self.assertEqual(maintenance.empty_work_directories(self.connection, self.storage, set()), [])
        self.assertTrue(outside.exists())

    def test_all_table_digests_ignore_indexes_but_detect_edits(self):
        self.connection.executescript("CREATE TABLE edits(id INTEGER PRIMARY KEY, text TEXT, data BLOB); INSERT INTO edits VALUES(1,'keep',x'0102');")
        before = maintenance.digest_rows(self.connection)
        self.connection.execute("CREATE INDEX edits_text ON edits(text)")
        self.assertEqual(before, maintenance.digest_rows(self.connection))
        self.connection.execute("UPDATE edits SET text='changed'")
        self.assertNotEqual(before, maintenance.digest_rows(self.connection))

    def test_existing_report_is_not_overwritten(self):
        path = self.root / "report.json"
        maintenance.write_report(path, {"original": True})
        with self.assertRaises(FileExistsError):
            maintenance.write_report(path, {})

    def test_process_lock_refuses_concurrent_maintenance(self):
        with maintenance.library_lock(self.root):
            with self.assertRaises(RuntimeError):
                with maintenance.library_lock(self.root):
                    self.fail("Second owner acquired the same lock")
        with maintenance.library_lock(self.root):
            pass
        self.assertTrue((self.root / ".piep-library.lock").exists())


if __name__ == "__main__":
    unittest.main()
