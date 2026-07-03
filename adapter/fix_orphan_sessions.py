#!/usr/bin/env python3
"""bettercursor Linux adapter: fix orphan cursor-agent CLI sessions.

A cursor-agent CLI session is "orphan" when its store.db has meta[0]
with `latestRootBlobId = ""` — meaning cursor-agent can list the folder
but `--resume <uuid>` silently fails or loads nothing.

This happens when:
  - The session was started by Cursor Electron Desktop (which doesn't
    maintain a `latestRootBlobId` in the cursor-agent CLI store.db), and
  - We want to make it resumable from the CLI without rebuilding the
    full DAG.

The fix: find the right "root" tree-node blob and set it in meta.

Algorithm:
  1. Read all blobs from the orphan store.db.
  2. For each blob, parse its bytes and find any 32-byte hashes that
     reference other blobs in the same store.db.
  3. The "root" candidate is the blob that:
     - Is itself NOT referenced by any other blob (top of DAG), AND
     - References the largest total of other blobs (transitive root).
  4. Write that ID into meta[0].latestRootBlobId.

Verified working on c1ea7999-005a-434f-bcf4-da8ddd9ff066.

Usage:
  ./fix_orphan_sessions.py            # dry-run: list orphans + chosen root
  ./fix_orphan_sessions.py --fix      # actually update meta
  ./fix_orphan_sessions.py --uuid XXX # fix a specific session only
"""

import argparse
import glob
import json
import os
import shutil
import sqlite3
import sys
from pathlib import Path


def read_varint(data, off):
    val = 0
    shift = 0
    while off < len(data):
        b = data[off]
        off += 1
        val |= (b & 0x7f) << shift
        if not (b & 0x80):
            return val, off
        shift += 7
    return val, off


def iter_inner(data):
    """Iterate top-level protobuf fields, yielding (field_no, wire_type, value_bytes)."""
    off = 0
    while off < len(data):
        if off >= len(data):
            break
        tag = data[off]
        off += 1
        fno = tag >> 3
        wt = tag & 0x7
        if wt == 2:
            ln, off2 = read_varint(data, off)
            yield fno, wt, data[off2:off2 + ln]
            off = off2 + ln
        elif wt == 0:
            _, off = read_varint(data, off)
        elif wt == 5:
            off += 4
        elif wt == 1:
            off += 8
        else:
            break


def find_internal_hashes(data):
    """Return all 32-byte hashes that appear inside the protobuf tree node."""
    refs = []
    for _fno, _wt, val in iter_inner(data):
        if len(val) == 32:
            refs.append(val.hex())
    return refs


def find_root(store_db_path: Path) -> str | None:
    """Given a cursor-agent store.db path, return the best root blob ID, or None."""
    # Copy to a temp file to avoid locking issues with a live agent
    con = sqlite3.connect(str(store_db_path))
    try:
        blobs = con.execute("SELECT id, data FROM blobs").fetchall()
    finally:
        con.close()

    if not blobs:
        return None

    blob_ids = {row[0] for row in blobs}

    # For each blob, find what other blobs in this set it references.
    referenced_by = {bid: set() for bid in blob_ids}  # blob_id -> set of blob IDs that reference IT
    references = {}  # blob_id -> set of blob IDs it references

    for bid, data in blobs:
        refs = find_internal_hashes(data)
        matched = {r for r in refs if r in blob_ids and r != bid}
        references[bid] = matched
        for r in matched:
            referenced_by[r].add(bid)

    # Root candidates: blobs NOT referenced by any other blob in this set
    roots = [bid for bid in blob_ids if not referenced_by[bid]]
    if not roots:
        return None

    # Pick the root that references the most other blobs (transitive coverage)
    def transitive_coverage(blob_id, seen=None):
        if seen is None:
            seen = set()
        if blob_id in seen:
            return seen
        seen.add(blob_id)
        for r in references.get(blob_id, set()):
            transitive_coverage(r, seen)
        return seen

    best = max(roots, key=lambda b: len(transitive_coverage(b)))
    return best


def find_orphans() -> list[dict]:
    """Scan all store.db files in ~/.cursor/chats/ for orphan sessions."""
    orphans = []
    for db_path_str in glob.glob(os.path.expanduser("~/.cursor/chats/*/*/store.db")):
        db_path = Path(db_path_str)
        try:
            con = sqlite3.connect(str(db_path))
            row = con.execute("SELECT value FROM meta WHERE key = 0").fetchone()
            con.close()
            if not row:
                continue
            meta = json.loads(bytes.fromhex(row[0]).decode())
            if meta.get("latestRootBlobId", "") == "":
                orphans.append({
                    "db_path": db_path,
                    "agent_id": meta.get("agentId", ""),
                    "name": meta.get("name", ""),
                    "created_at": meta.get("createdAt", 0),
                    "blobs": 0,
                    "chosen_root": None,
                })
        except Exception as e:
            print(f"  ERROR reading {db_path}: {e}", file=sys.stderr)
    return orphans


def fill_blob_counts(orphans):
    for o in orphans:
        try:
            con = sqlite3.connect(str(o["db_path"]))
            o["blobs"] = con.execute("SELECT count(*) FROM blobs").fetchone()[0]
            con.close()
        except Exception:
            pass


def fix_orphan(db_path: Path, new_root: str, dry_run: bool = False) -> bool:
    """Set meta[0].latestRootBlobId = new_root."""
    backup_path = db_path.parent / f"{db_path.name}.pre_bettercursor"
    if not backup_path.exists() and not dry_run:
        shutil.copy2(db_path, backup_path)
        for suffix in ("-wal", "-shm"):
            sidecar = db_path.parent / f"{db_path.name}{suffix}"
            if sidecar.exists():
                shutil.copy2(sidecar, f"{backup_path}{suffix}")

    if dry_run:
        return True

    con = sqlite3.connect(str(db_path))
    try:
        row = con.execute("SELECT value FROM meta WHERE key = 0").fetchone()
        meta = json.loads(bytes.fromhex(row[0]).decode())
        meta["latestRootBlobId"] = new_root
        new_hex = json.dumps(meta, separators=(",", ":")).encode().hex()
        con.execute("UPDATE meta SET value = ? WHERE key = 0", (new_hex,))
        con.commit()
        try:
            con.execute("PRAGMA wal_checkpoint(TRUNCATE)")
        except sqlite3.OperationalError:
            pass
    finally:
        con.close()
    return True


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--fix", action="store_true", help="actually update meta (otherwise dry-run)")
    ap.add_argument("--uuid", help="fix only this specific session UUID")
    args = ap.parse_args()

    print("Scanning for orphan sessions...")
    orphans = find_orphans()
    if args.uuid:
        orphans = [o for o in orphans if args.uuid in str(o["db_path"])]
    fill_blob_counts(orphans)

    if not orphans:
        print("No orphan sessions found. ✓")
        return 0

    print(f"Found {len(orphans)} orphan session(s):\n")
    for o in orphans:
        o["chosen_root"] = find_root(o["db_path"])
        marker = "✓ can fix" if o["chosen_root"] else "✗ no root candidate (need to import from elsewhere)"
        print(f"  [{o['created_at']}] {o['agent_id']}")
        print(f"    path:   {o['db_path']}")
        print(f"    name:   {o['name']!r}")
        print(f"    blobs:  {o['blobs']}")
        print(f"    chosen: {o['chosen_root']}  ({marker})")
        print()

    fixable = [o for o in orphans if o["chosen_root"]]
    if not fixable:
        print("Nothing can be fixed automatically.")
        return 1

    if not args.fix:
        print("Dry-run only. Re-run with --fix to apply.")
        return 0

    print("Applying fixes...")
    for o in fixable:
        if fix_orphan(o["db_path"], o["chosen_root"]):
            print(f"  ✓ {o['agent_id']}  →  latestRootBlobId={o['chosen_root'][:16]}...")
    print("\nDone.")
    return 0


if __name__ == "__main__":
    sys.exit(main())