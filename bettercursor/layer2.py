"""bettercursor Layer 2 — write to ~/.cursor/chats/<md5>/<uuid>/store.db.

This is NEW code (cursaves doesn't touch Layer 2; it only writes Layer 3).

Layer 2 schema (cursor-agent CLI):
  Directory:  ~/.cursor/chats/<chat_root>/<composer_id>/
    - meta.json          : {schemaVersion, hasConversation, title, createdAt}
    - prompt_history.json: ["/resume", ...]
    - store.db           : SQLite with blobs(id TEXT, data BLOB) + meta(key, value)
       key=0  → hex(JSON{meta}) : contains agentId, latestRootBlobId, name, etc.
       key=1  → hex(JSON{messages}) : [{role, content}, ...]   (sometimes)
       agentKv:blob:<hex>  →  raw blob bytes (b64 in snapshot)

We use bettercursor.storage.CursorStorage for safe SQLite I/O.
"""

from __future__ import annotations

import base64
import json
import shutil
import sqlite3
from pathlib import Path
from typing import Optional

from .blob_dag import fix_latest_root, find_root_blob
from .paths import chat_dir_for, chat_root_for, store_db_for
from .snapshot import Snapshot
from .storage import CursorStorage, backup_db


def _ensure_chat_dir(cwd: str | Path, composer_id: str) -> Path:
    """Create ~/.cursor/chats/<md5>/<uuid>/ if missing."""
    chat_dir = chat_dir_for(cwd, composer_id)
    chat_dir.mkdir(parents=True, exist_ok=True)
    return chat_dir


def _init_store_db(store_db_path: Path):
    """Create empty store.db with the tables cursor-agent expects."""
    if store_db_path.exists():
        return
    con = sqlite3.connect(str(store_db_path))
    con.execute("CREATE TABLE IF NOT EXISTS blobs (id TEXT PRIMARY KEY, data BLOB)")
    con.execute("CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT)")
    con.commit()
    con.close()


def _meta_json_path(chat_dir: Path) -> Path:
    return chat_dir / "meta.json"


def _prompt_history_path(chat_dir: Path) -> Path:
    return chat_dir / "prompt_history.json"


def write_meta_json(
    cwd: str | Path,
    composer_id: str,
    *,
    title: str,
    schema_version: int = 1,
    has_conversation: bool = True,
    created_at: int = 0,
) -> Path:
    """Write the Layer 2 meta.json sidecar."""
    chat_dir = _ensure_chat_dir(cwd, composer_id)
    meta = {
        "schemaVersion": schema_version,
        "hasConversation": has_conversation,
        "title": title,
        "createdAt": created_at,
    }
    target = _meta_json_path(chat_dir)
    target.write_text(json.dumps(meta, ensure_ascii=False, indent=2))
    return target


def write_prompt_history(cwd: str | Path, composer_id: str, history: list[str] | None = None) -> Path:
    """Write prompt_history.json. Default: ['/resume']."""
    chat_dir = _ensure_chat_dir(cwd, composer_id)
    if history is None:
        history = ["/resume"]
    target = _prompt_history_path(chat_dir)
    target.write_text(json.dumps(history, ensure_ascii=False))
    return target


def write_store_db(
    cwd: str | Path,
    composer_id: str,
    snapshot: Snapshot,
    *,
    dry_run: bool = False,
    backup: bool = True,
) -> dict:
    """Write snapshot to Layer 2 store.db, then fix latestRootBlobId.

    Returns a report dict: {written_blobs, fixed_root, root_blob_id, skipped}.
    """
    store_db = store_db_for(cwd, composer_id)
    chat_dir = _ensure_chat_dir(cwd, composer_id)
    _init_store_db(store_db)

    # Backup if store.db already has data
    if backup and store_db.exists() and store_db.stat().st_size > 0:
        backup_db(store_db)

    if dry_run:
        # Just check if root would be fixable
        root = find_root_blob(store_db)
        return {
            "written_blobs": 0,
            "fixed_root": False,
            "root_blob_id": root,
            "skipped": True,
            "dry_run": True,
        }

    cs = CursorStorage(store_db)
    try:
        # 1. Write all agent blobs (binary) — Layer 2 schema uses
        #    blobs(id TEXT, data BLOB) with id = SHA256 hex (NO prefix).
        #    This differs from Layer 3 (globalStorage) where keys are
        #    'agentKv:blob:<hex>' in the cursorDiskKV table.
        if snapshot.agent_blobs:
            cs.write_blobs_batch([
                (bid, base64.b64decode(b64))
                for bid, b64 in snapshot.agent_blobs.items()
            ])

        # 2. Write meta[0] — the agent session metadata
        meta0 = {
            "agentId": composer_id,
            "latestRootBlobId": "",  # will be fixed below
            "name": snapshot.composer_data.get("name", "New Agent"),
            "mode": snapshot.composer_data.get("forceMode", "default"),
            "isRunEverything": snapshot.composer_data.get("isRunEverything", False),
            "createdAt": snapshot.composer_data.get("createdAt", 0),
        }
        cs.write_item(
            "0",
            json.dumps(meta0, separators=(",", ":")).encode().hex(),
            table="meta",
        )

        # 3. Fix latestRootBlobId (this is the unique value-add of bettercursor)
        cs.close()  # release write connection so fix_latest_root can open its own
        fixed = fix_latest_root(store_db, dry_run=False)

        return {
            "written_blobs": len(snapshot.agent_blobs),
            "fixed_root": fixed is not None,
            "root_blob_id": fixed,
            "skipped": False,
        }
    finally:
        cs.close()


def import_snapshot_to_layer2(
    cwd: str | Path,
    snapshot: Snapshot,
    *,
    dry_run: bool = False,
    backup: bool = True,
) -> dict:
    """High-level: import a snapshot to all of Layer 2 (meta.json + prompt_history + store.db).

    Returns the merged report from each sub-step.
    """
    if snapshot.is_empty():
        return {"skipped": "empty"}

    title = snapshot.composer_data.get("name") or "New Agent"
    created_at = snapshot.composer_data.get("createdAt", 0)

    if dry_run:
        return {"dry_run": True, "would_write": ["meta.json", "prompt_history.json", "store.db"]}

    write_meta_json(cwd, snapshot.composer_id, title=title, created_at=created_at)
    write_prompt_history(cwd, snapshot.composer_id)
    return write_store_db(cwd, snapshot.composer_id, snapshot, dry_run=False, backup=backup)