"""bettercursor blob DAG — protobuf parsing + latestRootBlobId repair.

BORROWED FROM: vendored/cursaves/cursor_saves/export.py:_extract_agent_blob_ids

What we borrowed:
  - Wire-format protobuf parser: read_varint + tag check (field_no, wire_type).
  - Walk the protobuf, collect 32-byte length-delimited fields (those are
    SHA256 hex of agentKv:blob:<id> entries Cursor references internally).
  - Use proper varint length prefixes (not naive byte scanning).

What we changed:
  - We also compute transitive coverage (which blob references the most
    other blobs) for picking the DAG root.
  - We expose fix_root(store_db) to repair `meta[0].latestRootBlobId = ""`
    sessions. (cursaves doesn't do this; cursor-agent won't --resume them.)

Background:
  Cursor's agent loop stores conversation state as a Merkle DAG:
  - Leaf blobs are JSON content (system prompt, user message, etc.)
  - Internal "tree nodes" are protobuf containing refs (32-byte SHA256) to
    child blobs.
  - The root blob references all top-level children (system, user, etc.).
  - meta[0].latestRootBlobId points to the current root.

When a session is interrupted before the agent loop finalizes the root,
or when a snapshot is imported incompletely, the root ID ends up empty
string in the meta — making cursor-agent --resume silently fail.
"""

from __future__ import annotations

import sqlite3
from pathlib import Path
from typing import Optional


def read_varint(data: bytes, offset: int) -> tuple[int, int]:
    """Decode a base-128 varint. Returns (value, next_offset)."""
    result = 0
    shift = 0
    while offset < len(data):
        b = data[offset]
        offset += 1
        result |= (b & 0x7F) << shift
        if (b & 0x80) == 0:
            return result, offset
        shift += 7
    return result, offset


def extract_blob_ids_from_protobuf(raw: bytes) -> set[str]:
    """Extract all 32-byte SHA256 hashes from a Cursor protobuf blob.

    Walks wire-format tags. A 32-byte length-delimited field at the top level
    or inside a sub-message is treated as a blob ID reference.

    This mirrors cursaves' _extract_agent_blob_ids parser (export.py:321-380)
    but is more general: cursaves only walks the composerData.conversationState
    protobuf; this works on any Cursor protobuf blob.
    """
    blob_ids: set[str] = set()
    _walk(data=raw, into=blob_ids)
    return blob_ids


def _walk(data: bytes, into: set[str]) -> None:
    offset = 0
    end = len(data)
    while offset < end:
        try:
            tag, offset = read_varint(data, offset)
        except Exception:
            break
        wire_type = tag & 0x07
        if wire_type == 2:  # length-delimited
            length, data_start = read_varint(data, offset)
            if length == 32 and data_start + 32 <= end:
                into.add(data[data_start:data_start + 32].hex())
                offset = data_start + 32
            elif length > 0 and data_start + length <= end:
                _walk(data[data_start:data_start + length], into)
                offset = data_start + length
            else:
                offset = data_start if data_start > offset else offset + 1
        elif wire_type == 0:
            _, offset = read_varint(data, offset)
        elif wire_type == 5:  # 32-bit
            offset += 4
        elif wire_type == 1:  # 64-bit
            offset += 8
        else:
            offset += 1


def find_root_blob(store_db_path: Path) -> Optional[str]:
    """Given a Layer 2 store.db, find the blob most likely to be the root.

    Algorithm:
      1. Load all blobs.
      2. For each blob, find the set of OTHER blobs in this DB it references
         (via 32-byte protobuf hashes).
      3. The "root" candidate is the blob NOT referenced by any other blob
         (top of the DAG).
      4. Tie-break by transitive coverage (which root has the most descendants
         when we follow its references recursively).

    Returns the hex blob ID, or None if no candidate.
    """
    con = sqlite3.connect(str(store_db_path))
    try:
        blobs = con.execute("SELECT id, data FROM blobs").fetchall()
    finally:
        con.close()

    if not blobs:
        return None

    blob_ids = {row[0] for row in blobs}

    referenced_by: dict[str, set[str]] = {bid: set() for bid in blob_ids}
    references: dict[str, set[str]] = {}

    for bid, data in blobs:
        refs = extract_blob_ids_from_protobuf(data)
        matched = {r for r in refs if r in blob_ids and r != bid}
        references[bid] = matched
        for r in matched:
            referenced_by[r].add(bid)

    # Roots = blobs nobody references
    roots = [bid for bid in blob_ids if not referenced_by[bid]]
    if not roots:
        return None

    # Prefer the one with maximum transitive coverage
    def transitive(blob_id: str, seen: set[str] | None = None) -> set[str]:
        if seen is None:
            seen = set()
        if blob_id in seen:
            return seen
        seen.add(blob_id)
        for r in references.get(blob_id, ()):
            transitive(r, seen)
        return seen

    return max(roots, key=lambda b: len(transitive(b)))


def fix_latest_root(store_db_path: Path, dry_run: bool = True) -> Optional[str]:
    """Repair meta[0].latestRootBlobId = "" by finding and writing the root.

    Returns the chosen root blob ID (whether dry-run or applied), or None
    if no fix is needed or possible.

    Safe to call on a store.db the user is not currently using (Layer 2
    store.db is owned by cursor-agent CLI, not the Electron app).
    """
    import json

    con = sqlite3.connect(str(store_db_path))
    try:
        row = con.execute("SELECT value FROM meta WHERE key = 0").fetchone()
        if not row:
            return None
        try:
            meta = json.loads(bytes.fromhex(row[0]).decode())
        except Exception:
            return None
        if meta.get("latestRootBlobId", "") != "":
            return None  # already set; nothing to fix

        root = find_root_blob(store_db_path)
        if root is None:
            return None

        if dry_run:
            return root

        meta["latestRootBlobId"] = root
        new_hex = json.dumps(meta, separators=(",", ":")).encode().hex()
        con.execute("UPDATE meta SET value = ? WHERE key = 0", (new_hex,))
        con.commit()
        try:
            con.execute("PRAGMA wal_checkpoint(TRUNCATE)")
        except sqlite3.OperationalError:
            pass
        return root
    finally:
        con.close()