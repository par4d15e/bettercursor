"""bettercursor storage layer — WAL-safe SQLite I/O for Cursor databases.

BORROWED FROM: vendored/cursaves/cursor_saves/db.py (CursorDB class)

What we borrowed:
  - Read via temp copy of main + WAL/SHM + `PRAGMA wal_checkpoint(TRUNCATE)`
    to avoid lock contention with running Cursor.
  - Writes go to the ORIGINAL file (caller must ensure Cursor is closed
    or the workspace DB is not actively written).
  - Batch write with single transaction (write_batch / write_json_batch).
  - Backup before destructive operations, keep last 2 backups.

What we changed from cursaves:
  - No relative imports (we're a flat package, not nested).
  - Use pathlib.Path uniformly (no str/Path mixing).
  - Drop the .vscdb-specific naming; support both state.vscdb and
    chats/<md5>/<uuid>/store.db.
  - Add `read_meta(key)` / `write_meta(key, value)` convenience for
    Layer 2's key-value meta table.

Why own implementation:
  - cursaves is AGPL-3.0 — we want bettercursor to stay MIT.
  - We only need ~30% of CursorDB's API; full re-implementation is
    clearer than vendoring and pruning.
  - We add Layer 2 (chats/.../store.db) cursaves doesn't touch.
"""

from __future__ import annotations

import json
import shutil
import sqlite3
import tempfile
from pathlib import Path
from typing import Any, Optional


class CursorStorage:
    """WAL-safe interface to a Cursor SQLite database (state.vscdb or store.db).

    Reads: copy the DB + WAL/SHM to a temp file, checkpoint WAL, query the copy.
           Avoids lock contention with a running Cursor.
    Writes: operate on the ORIGINAL file. Caller must ensure Cursor is closed
            for that DB (or accept corruption risk).
    """

    def __init__(self, db_path: Path):
        self.db_path = Path(db_path)
        self._tmp_dir: Optional[Path] = None
        self._read_conn: Optional[sqlite3.Connection] = None
        self._write_conn: Optional[sqlite3.Connection] = None

    # ── Read (temp copy) ──────────────────────────────────────

    def _open_read_copy(self) -> sqlite3.Connection:
        if self._read_conn is not None:
            return self._read_conn
        if not self.db_path.exists():
            raise FileNotFoundError(f"DB not found: {self.db_path}")

        self._tmp_dir = Path(tempfile.mkdtemp(prefix="bettercursor-"))
        tmp_db = self._tmp_dir / self.db_path.name
        shutil.copy2(self.db_path, tmp_db)
        for suffix in ("-wal", "-shm"):
            sidecar = self.db_path.parent / (self.db_path.name + suffix)
            if sidecar.exists():
                shutil.copy2(sidecar, self._tmp_dir / (tmp_db.name + suffix))

        self._read_conn = sqlite3.connect(str(tmp_db))
        try:
            self._read_conn.execute("PRAGMA wal_checkpoint(TRUNCATE)")
        except sqlite3.OperationalError:
            pass  # not in WAL mode
        return self._read_conn

    def get_item(self, key: str, table: str = "ItemTable") -> Optional[str]:
        conn = self._open_read_copy()
        try:
            row = conn.execute(
                f"SELECT value FROM {table} WHERE key = ?", (key,)
            ).fetchone()
            if row is None:
                return None
            val = row[0]
            if isinstance(val, bytes):
                return val.decode("utf-8", errors="replace")
            return val
        except sqlite3.OperationalError:
            return None

    def get_item_binary(self, key: str, table: str = "ItemTable") -> Optional[bytes]:
        conn = self._open_read_copy()
        try:
            row = conn.execute(
                f"SELECT value FROM {table} WHERE key = ?", (key,)
            ).fetchone()
            if row is None:
                return None
            val = row[0]
            if isinstance(val, str):
                return val.encode("utf-8")
            return val
        except sqlite3.OperationalError:
            return None

    def get_json(self, key: str, table: str = "ItemTable") -> Optional[Any]:
        raw = self.get_item(key, table=table)
        if raw is None:
            return None
        try:
            return json.loads(raw)
        except json.JSONDecodeError:
            return None

    def list_keys(self, prefix: str = "", table: str = "ItemTable") -> list[str]:
        conn = self._open_read_copy()
        try:
            if prefix:
                rows = conn.execute(
                    f"SELECT key FROM {table} WHERE key LIKE ?", (prefix + "%",)
                ).fetchall()
            else:
                rows = conn.execute(f"SELECT key FROM {table}").fetchall()
            return [r[0] for r in rows]
        except sqlite3.OperationalError:
            return []

    # ── Write (original file) ─────────────────────────────────

    def _open_write_conn(self) -> sqlite3.Connection:
        if self._write_conn is None:
            self._write_conn = sqlite3.connect(str(self.db_path))
        return self._write_conn

    def write_item(self, key: str, value: str, table: str = "ItemTable"):
        conn = self._open_write_conn()
        conn.execute(
            f"INSERT OR REPLACE INTO {table} (key, value) VALUES (?, ?)",
            (key, value),
        )
        conn.commit()

    def write_json(self, key: str, data: Any, table: str = "ItemTable"):
        self.write_item(key, json.dumps(data, separators=(",", ":")), table=table)

    def write_binary(self, key: str, data: bytes, table: str = "ItemTable"):
        self.write_item(key, data.decode("utf-8", errors="replace"), table=table)

    def write_batch(self, items: list[tuple[str, str]], table: str = "ItemTable"):
        conn = self._open_write_conn()
        conn.execute("BEGIN")
        try:
            conn.executemany(
                f"INSERT OR REPLACE INTO {table} (key, value) VALUES (?, ?)",
                items,
            )
            conn.execute("COMMIT")
        except Exception:
            conn.execute("ROLLBACK")
            raise

    def write_json_batch(self, items: list[tuple[str, Any]], table: str = "ItemTable"):
        serialized = [
            (k, json.dumps(v, separators=(",", ":"))) for k, v in items
        ]
        self.write_batch(serialized, table=table)

    def write_binary_batch(self, items: list[tuple[str, bytes]], table: str = "ItemTable"):
        serialized = [(k, v.decode("utf-8", errors="replace")) for k, v in items]
        self.write_batch(serialized, table=table)

    def write_blobs_batch(self, items: list[tuple[str, bytes]]):
        """Write to Layer 2 `blobs(id TEXT, data BLOB)` table.

        Note column names differ from ItemTable/cursorDiskKV (key/value).
        """
        conn = self._open_write_conn()
        conn.execute("BEGIN")
        try:
            conn.executemany(
                "INSERT OR REPLACE INTO blobs (id, data) VALUES (?, ?)",
                items,
            )
            conn.execute("COMMIT")
        except Exception:
            conn.execute("ROLLBACK")
            raise

    def delete_keys(self, keys: list[str], table: str = "ItemTable") -> int:
        if not keys:
            return 0
        conn = self._open_write_conn()
        conn.execute("BEGIN")
        try:
            total = 0
            for batch_start in range(0, len(keys), 500):
                batch = keys[batch_start:batch_start + 500]
                placeholders = ",".join("?" for _ in batch)
                cur = conn.execute(
                    f"DELETE FROM {table} WHERE key IN ({placeholders})", batch
                )
                total += cur.rowcount
            conn.execute("COMMIT")
            return total
        except Exception:
            conn.execute("ROLLBACK")
            raise

    # ── Lifecycle ──────────────────────────────────────────────

    def close(self):
        if self._read_conn:
            self._read_conn.close()
            self._read_conn = None
        if self._write_conn:
            self._write_conn.close()
            self._write_conn = None
        if self._tmp_dir:
            shutil.rmtree(self._tmp_dir, ignore_errors=True)
            self._tmp_dir = None

    def __enter__(self):
        return self

    def __exit__(self, *args):
        self.close()


def backup_db(db_path: Path, keep: int = 2) -> Path:
    """Create a timestamped backup; keep only the most recent `keep`."""
    from datetime import datetime

    db_path = Path(db_path)
    ts = datetime.now().strftime("%Y%m%d_%H%M%S")
    backup = db_path.parent / f"{db_path.stem}.backup_{ts}{db_path.suffix}"
    shutil.copy2(db_path, backup)
    for suffix in ("-wal", "-shm"):
        sidecar = db_path.parent / (db_path.name + suffix)
        if sidecar.exists():
            shutil.copy2(sidecar, backup.parent / (backup.name + suffix))

    # Prune older backups
    pattern = f"{db_path.stem}.backup_*{db_path.suffix}"
    stale = sorted(
        db_path.parent.glob(pattern),
        key=lambda p: p.stat().st_mtime,
        reverse=True,
    )
    for old in stale[keep:]:
        old.unlink(missing_ok=True)
        for suffix in ("-wal", "-shm"):
            sidecar = old.parent / (old.name + suffix)
            sidecar.unlink(missing_ok=True)

    return backup