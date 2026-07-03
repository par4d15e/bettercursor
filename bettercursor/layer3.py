"""bettercursor Layer 3 — write to ~/.config/Cursor/User/globalStorage/state.vscdb
(workspace-scoped state.vscdb in workspaceStorage/<hash>/).

BORROWED FROM: vendored/cursaves/cursor_saves/importer.py:import_snapshot()
                  (Steps 2-4: global DB writes + workspace DB register)

What we borrowed:
  - globalStorage writes use ItemTable + cursorDiskKV (two tables).
  - Specific key conventions:
      ItemTable['composer.composerHeaders']    — central Sidebar index
      cursorDiskKV['composerData:<uuid>']      — full composer snapshot
      cursorDiskKV['bubbleId:<uuid>:<bid>']    — per-message blob
      cursorDiskKV['checkpointId:<uuid>:<cp>'] — checkpoint state
      cursorDiskKV['agentKv:blob:<hex>']       — binary state blob
  - Write ordering: composerData first, then content/message/bubble/checkpoint
    batches, then agent blobs (last because they're large).
  - Verify writes after committing (re-read sample key).

What we changed:
  - We do NOT auto-create workspace DB (cursaves does; we leave that to
    user running Cursor once first, OR we provide an explicit
    `ensure_workspace_for()` helper).
  - We add a `rewrite_paths` step by default (cursaves does this in
    import_snapshot but we always want it when source != target).
  - We do NOT touch the workspace's `composer.composerData` table —
    we only write to the globalStorage DB. (cursaves writes the
    workspace-level composer.composerData too; we found that causes
    cursor-server crashes when the user reopens the workspace.)
"""

from __future__ import annotations

import base64
import json
import os
from pathlib import Path
from typing import Optional

from .paths import get_global_db_path
from .snapshot import Snapshot
from .storage import CursorStorage, backup_db


def import_snapshot_to_layer3(
    snapshot: Snapshot,
    target_project_path: str,
    *,
    dry_run: bool = False,
    backup: bool = True,
    rewrite_paths: bool = True,
) -> dict:
    """Write snapshot to globalStorage/state.vscdb (Layer 3).

    Cursor must be closed for the workspace being targeted (we write to
    the global DB which the workspace DBs also read from).
    """
    if snapshot.is_empty():
        return {"skipped": "empty"}

    if dry_run:
        return {
            "dry_run": True,
            "would_write": [
                "composer.composerHeaders",
                f"composerData:{snapshot.composer_id}",
                f"bubbleId:{snapshot.composer_id}:*",
                f"agentKv:blob:*",
            ],
        }

    global_db = get_global_db_path()
    if not global_db.exists():
        return {"error": f"global DB not found: {global_db}"}

    if backup:
        backup_db(global_db)

    # Path rewriting if source != target
    composer_data = snapshot.composer_data
    source_path = snapshot.source_project_path
    target_path = os.path.normpath(target_project_path)
    if rewrite_paths and source_path and source_path != target_path:
        composer_data = _rewrite_paths_in_data(composer_data, source_path, target_path)

    cs = CursorStorage(global_db)
    try:
        # 1. composerData:<uuid> — the main composer snapshot
        cs.write_json(f"composerData:{snapshot.composer_id}", composer_data, table="cursorDiskKV")

        # 2. composer.composerHeaders — central Sidebar index (incremental update)
        _update_composer_headers(cs, snapshot.composer_id, composer_data)

        # 3. bubbleId entries
        if snapshot.bubble_entries:
            cs.write_json_batch([
                (f"bubbleId:{snapshot.composer_id}:{bid}", bdata)
                for bid, bdata in snapshot.bubble_entries.items()
            ], table="cursorDiskKV")

        # 4. checkpointId entries
        if snapshot.checkpoints:
            cs.write_json_batch([
                (f"checkpointId:{snapshot.composer_id}:{cp_id}", cp_data)
                for cp_id, cp_data in snapshot.checkpoints.items()
            ], table="cursorDiskKV")

        # 5. content blobs
        if snapshot.content_blobs:
            cs.write_batch([
                (f"composer.content.{h}", v)
                for h, v in snapshot.content_blobs.items()
            ], table="cursorDiskKV")

        # 6. messageRequestContext entries
        if snapshot.message_contexts:
            cs.write_json_batch([
                (f"messageRequestContext:{snapshot.composer_id}:{k}", v)
                for k, v in snapshot.message_contexts.items()
            ], table="cursorDiskKV")

        # 7. agentKv:blob (binary state blobs) — last because largest
        if snapshot.agent_blobs:
            cs.write_binary_batch([
                (f"agentKv:blob:{bid}", base64.b64decode(b64))
                for bid, b64 in snapshot.agent_blobs.items()
            ], table="cursorDiskKV")

        return {
            "ok": True,
            "composer_id": snapshot.composer_id,
            "bubble_count": len(snapshot.bubble_entries),
            "agent_blob_count": len(snapshot.agent_blobs),
        }
    finally:
        cs.close()


def _update_composer_headers(cs: CursorStorage, composer_id: str, composer_data: dict):
    """Add this composer to the central composerHeaders index (Cursor 3.0+).

    Borrowed from cursaves importer (which does this differently — it
    relies on workspace registration to populate the index). We update
    the global index directly because we're already writing to global DB.
    """
    headers = cs.get_json("composer.composerHeaders", table="ItemTable")
    if not headers or not isinstance(headers, dict):
        headers = {"allComposers": []}

    # Build a header entry mirroring Cursor's format
    name = composer_data.get("name", "Untitled")
    created_at = composer_data.get("createdAt", 0)
    last_updated = composer_data.get("lastUpdatedAt", created_at)

    new_entry = {
        "composerId": composer_id,
        "name": name,
        "createdAt": created_at,
        "lastUpdatedAt": last_updated,
        "unifiedMode": composer_data.get("unifiedMode", "agent"),
        "forceMode": composer_data.get("forceMode", ""),
        "isAgentic": True,
        "workspaceIdentifier": composer_data.get("workspaceIdentifier"),
    }

    # Remove any existing entry with same id, then add
    headers["allComposers"] = [
        c for c in headers.get("allComposers", []) if c.get("composerId") != composer_id
    ]
    headers["allComposers"].append(new_entry)

    cs.write_json("composer.composerHeaders", headers, table="ItemTable")


# ── Path rewriting (borrowed cursaves _SKIP_REWRITE_KEYS) ───

_SKIP_REWRITE_KEYS = frozenset({"conversationState"})


def _rewrite_paths_in_data(data, old_prefix: str, new_prefix: str):
    """Recursively replace old_prefix with new_prefix in string values that look like paths."""
    if isinstance(data, str):
        return data.replace(old_prefix, new_prefix) if old_prefix in data else data
    if isinstance(data, dict):
        return {
            k: (v if k in _SKIP_REWRITE_KEYS else _rewrite_paths_in_data(v, old_prefix, new_prefix))
            for k, v in data.items()
        }
    if isinstance(data, list):
        return [_rewrite_paths_in_data(item, old_prefix, new_prefix) for item in data]
    return data