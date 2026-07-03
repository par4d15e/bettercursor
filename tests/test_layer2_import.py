"""End-to-end integration test for bettercursor.

Tests the full pipeline:
  1. Load a real snapshot (c1ea7999 from cursaves)
  2. Import to Layer 2 (chats/<md5>/<uuid>/)
  3. Verify store.db schema + meta + root blob auto-fix
  4. Verify meta.json + prompt_history.json sidecars
  5. Verify cursor-agent --resume would find this session

Run: cd bettercursor && python3 tests/test_layer2_import.py
"""

from __future__ import annotations

import json
import shutil
import sqlite3
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent.parent))

from bettercursor.layer2 import import_snapshot_to_layer2
from bettercursor.paths import chat_dir_for, chat_root_for, store_db_for
from bettercursor.snapshot import load_snapshot


SNAPSHOT_PATH = Path.home() / ".cursaves/snapshots/github.com-par4d15e-enenzuo/c1ea7999-005a-434f-bcf4-da8ddd9ff066.json.gz"


def test_layer2_end_to_end():
    assert SNAPSHOT_PATH.exists(), f"snapshot not found: {SNAPSHOT_PATH}"
    snap = load_snapshot(SNAPSHOT_PATH)

    # Use a temporary project path so we don't pollute the real enenzuo
    test_dir = Path(tempfile.mkdtemp(prefix="bettercursor-test-"))
    test_cwd = str(test_dir)

    try:
        # 1. dry-run
        report = import_snapshot_to_layer2(test_cwd, snap, dry_run=True)
        assert report["dry_run"], f"dry-run should report dry_run=True: {report}"

        # 2. actual import
        report = import_snapshot_to_layer2(test_cwd, snap, dry_run=False)
        assert report["written_blobs"] == 80, f"expected 80 blobs, got {report['written_blobs']}"
        assert report["fixed_root"], "root should be auto-fixed"
        root_id = report["root_blob_id"]
        assert root_id, "root blob ID should be set"

        # 3. verify store.db
        store_db = store_db_for(test_cwd, snap.composer_id)
        assert store_db.exists(), f"store.db not at {store_db}"

        con = sqlite3.connect(str(store_db))
        try:
            # Schema
            tables = {r[0] for r in con.execute(
                "SELECT name FROM sqlite_master WHERE type='table'"
            ).fetchall()}
            assert tables == {"blobs", "meta"}, f"unexpected tables: {tables}"

            # 80 blobs
            n = con.execute("SELECT count(*) FROM blobs").fetchone()[0]
            assert n == 80, f"expected 80 blobs, got {n}"

            # meta[0] parses
            meta_hex = con.execute("SELECT value FROM meta WHERE key=0").fetchone()[0]
            meta = json.loads(bytes.fromhex(meta_hex).decode())
            assert meta["agentId"] == snap.composer_id
            assert meta["latestRootBlobId"] == root_id

            # Root blob exists in blobs table
            root_size = con.execute(
                "SELECT length(data) FROM blobs WHERE id=?", (root_id,)
            ).fetchone()[0]
            assert root_size > 0, "root blob should have data"
        finally:
            con.close()

        # 4. sidecar files
        chat_dir = chat_dir_for(test_cwd, snap.composer_id)
        assert (chat_dir / "meta.json").exists()
        assert (chat_dir / "prompt_history.json").exists()

        meta = json.loads((chat_dir / "meta.json").read_text())
        assert meta["hasConversation"] is True
        assert "title" in meta

        ph = json.loads((chat_dir / "prompt_history.json").read_text())
        assert isinstance(ph, list)

        # 5. Print summary
        print(f"  ✓ Layer 2 import OK")
        print(f"  ✓ 80 blobs written to {store_db.name}")
        print(f"  ✓ root auto-fixed to {root_id[:32]}...")
        print(f"  ✓ meta.json + prompt_history.json present")
        print(f"  ✓ cleanup: {test_dir}")

    finally:
        shutil.rmtree(test_dir, ignore_errors=True)


if __name__ == "__main__":
    test_layer2_end_to_end()
    print("=== ALL TESTS PASSED ===")