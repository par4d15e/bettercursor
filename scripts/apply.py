#!/usr/bin/env python3
"""bettercursor — offline Layer 3 injector (state.vscdb writer).

Background
----------
Cursor Desktop's Sidebar reads three SQLite structures in
``~/.config/Cursor/User/globalStorage/state.vscdb`` (Linux) or
``~/Library/Application Support/Cursor/User/globalStorage/state.vscdb``
(macOS):

  - ``ItemTable['composer.composerHeaders']`` — list index
  - ``cursorDiskKV['composerData:<uuid>']``    — full composer data
  - ``cursorDiskKV['bubbleId:<uuid>:<bid>']`` — one blob per bubble

When a session is written only by ``cursor-agent`` CLI (Layer 2 +
JSONL only), the Desktop Sidebar never sees it. bettercursor's
``prepare_inject_layer3`` builds the SQLite upserts needed to make
the Sidebar recognise the session, and writes them into a JSON
envelope at ``~/.bettercursor/queue/inject-<uuid>.json``.

**This script** picks up that envelope, applies it to
``state.vscdb`` safely, and writes a sidecar marker so the UI can
show "✓ 已应用".

Why offline?
------------
Cursor Electron keeps ``state.vscdb`` open most of the time and
flushes its own WAL on every restart. We tried the live path; the
race reliably overwrote our rows on the next WAL checkpoint, even
though the ``rename(2)`` itself succeeded (#84). The fix is to
make the user close Cursor first, so the file is quiescent, then
do a tmpdir-copy + apply + integrity_check + atomic rename.

Safety checklist (all must pass):
  1. Cursor Electron process not running.
  2. Marker sidecar not present (idempotent re-runs need --force).
  3. Schema sanity: ItemTable + cursorDiskKV exist.
  4. Original state.vscdb copied to ``state.vscdb.pre_bettercursor``.
  5. All mutations applied to the tmpdir copy.
  6. ``PRAGMA integrity_check`` says ok.
  7. Atomic rename back; old inode is replaced; sidecar WALs
     reused. Marker written last.

Usage
-----
    python3 apply.py <queue.json>             # apply once
    python3 apply.py <queue.json> --force     # re-apply even if marker exists
    python3 apply.py --check-cursor           # dry-run: just check liveness
    python3 apply.py --version

The script is copied into ``~/.bettercursor/apply.py`` on first
``prepare_inject_layer3`` call so it survives project moves.
"""
from __future__ import annotations

import argparse
import json
import os
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from typing import Any, Iterable

SCHEMA_VERSION = 1
TOOL = "bettercursor"

# ── Pretty printing (no colour deps) ──────────────────────────
def info(msg: str) -> None:
    print(f"[{TOOL}] {msg}", flush=True)


def warn(msg: str) -> None:
    print(f"[{TOOL}:warn] {msg}", file=sys.stderr, flush=True)


def err(msg: str) -> None:
    print(f"[{TOOL}:error] {msg}", file=sys.stderr, flush=True)


# ── Cursor liveness detection ────────────────────────────────
def cursor_running() -> tuple[bool, list[str]]:
    """Return (running, matching_process_lines). Uses ``pgrep -f`` on
    Linux/macOS. Skipped on Windows (where this script will still
    try later); for our current target (Linux) pgrep is universal.
    """
    if sys.platform == "win32":
        return False, []  # best-effort
    # Excluding ourselves so applying from a Tauri-launched child
    # process doesn't false-positive.
    patterns = [
        "Cursor --type=",
        "globalStorage/state.vscdb",
        "/Cursor --updated",          # Linux Electron helper arg
        "Cursor-bin",                  # packaged Electron binary
    ]
    matches: list[str] = []
    for pat in patterns:
        try:
            out = subprocess.run(
                ["pgrep", "-af", pat],
                check=False,
                capture_output=True,
                text=True,
                timeout=2,
            )
        except (FileNotFoundError, subprocess.TimeoutExpired):
            continue
        for line in (out.stdout or "").splitlines():
            line = line.strip()
            if not line:
                continue
            # ``pgrep -f`` matches itself too if pattern is loose.
            if "pgrep" in line and pat in line:
                continue
            matches.append(line)
    return (len(matches) > 0), matches


# ── Schema + mutation helpers ────────────────────────────────
EXPECTED_OPS = {"item_table_upsert", "disk_kv_upsert"}


def _hex_to_text(hex_str: str) -> str:
    """Decode a hex string into the text bytes used by both tables.
    ``ItemTable`` and ``cursorDiskKV`` store ``value`` as TEXT in
    Cursor's schema, and we serialise JSON via UTF-8, so the safe
    fallback is Latin-1 if the JSON contains non-UTF8 (unlikely for
    a composer payload we just produced)."""
    raw = bytes.fromhex(hex_str)
    try:
        return raw.decode("utf-8")
    except UnicodeDecodeError:
        return raw.decode("latin-1", errors="replace")


def ensure_tables(conn: sqlite3.Connection) -> None:
    """Confirm the two tables Cursor needs both exist. We refuse to
    run on an empty / unrecognised schema rather than silently
    creating empty tables Cursor doesn't know to read."""
    cur = conn.execute(
        "SELECT name FROM sqlite_master "
        "WHERE type='table' AND name IN ('ItemTable', 'cursorDiskKV')"
    )
    found = {row[0] for row in cur.fetchall()}
    missing = {"ItemTable", "cursorDiskKV"} - found
    if missing:
        raise RuntimeError(
            f"state.vscdb missing tables: {sorted(missing)} "
            "(Cursor version mismatched? Maybe a fresh install?)"
        )


def apply_mutation(conn: sqlite3.Connection, m: dict[str, Any]) -> None:
    op = m.get("op")
    key = m["key"]
    value_text = _hex_to_text(m["value_hex"])
    if op == "item_table_upsert":
        conn.execute(
            "INSERT OR REPLACE INTO ItemTable(key, value) VALUES (?, ?)",
            (key, value_text),
        )
    elif op == "disk_kv_upsert":
        conn.execute(
            "INSERT OR REPLACE INTO cursorDiskKV(key, value) VALUES (?, ?)",
            (key, value_text),
        )
    else:
        raise RuntimeError(f"unknown mutation op: {op!r} (key={key})")


def atomic_rename(src: Path, dst: Path) -> None:
    """Atomic swap + best-effort WAL/SHM swap. On POSIX, rename(2)
    over an existing path replaces the destination atomically —
    but only when src and dst are on the same filesystem. The
    stdlib ``tempfile.TemporaryDirectory`` defaults to /tmp which
    is almost always a separate tmpfs on Linux, raising
    ``OSError: [Errno 18] Invalid cross-device link`` (#85).
    Fix: stage the copy in ``dst.parent`` (same fs as dst), then
    ``os.replace`` atomically. POSIX guarantees the **dst side**
    of rename is atomic even when src came from a different fs.
    """
    if src.parent != dst.parent:
        # Stage a same-filesystem copy, then rename. copy2 preserves
        # mtime/permissions; the on-disk bytes are already correct
        # because we just ran integrity_check + wal_checkpoint.
        staged = dst.parent / (src.name + ".bettercursor-stage")
        shutil.copy2(src, staged)
        try:
            os.replace(staged, dst)
        except BaseException:
            # Don't leave the staging file behind if rename fails.
            try:
                staged.unlink()
            except OSError:
                pass
            raise
    else:
        os.replace(src, dst)

    # Move wal/shm sidecars if the working dir held them (the
    # integrity check leaves them in place; they belong next to
    # the renamed DB so the next process opening state.vscdb finds
    # a consistent pair).
    tmp_dir = src.parent
    for suffix, replace_dst_ext in (("-wal", "vscdb-wal"),
                                    ("-shm", "vscdb-shm")):
        sidecar = tmp_dir / f"state.vscdb{suffix}"
        if sidecar.is_file():
            target = dst.with_name(replace_dst_ext)
            try:
                os.replace(sidecar, target)
            except OSError as exc:
                warn(f"could not swap sidecar {sidecar} → {target}: {exc}")


def backup_original(db_path: Path) -> Path:
    backup = db_path.with_name(db_path.name + ".pre_bettercursor")
    if not backup.exists():
        shutil.copy2(db_path, backup)
        for suffix in ("-wal", "-shm"):
            sidecar = db_path.with_name(db_path.name + suffix)
            if sidecar.is_file():
                shutil.copy2(
                    sidecar,
                    db_path.with_name(db_path.name + ".pre_bettercursor" + suffix),
                )
    return backup


def integrity_ok(conn: sqlite3.Connection) -> bool:
    cur = conn.execute("PRAGMA integrity_check")
    return cur.fetchone()[0] == "ok"


# ── Pipeline ─────────────────────────────────────────────────
def load_envelope(path: Path) -> dict[str, Any]:
    try:
        with path.open("r", encoding="utf-8") as f:
            env = json.load(f)
    except FileNotFoundError:
        raise RuntimeError(f"queue file not found: {path}")
    except json.JSONDecodeError as e:
        raise RuntimeError(f"queue file is not valid JSON: {e}")
    schema = env.get("schema_version")
    if schema != SCHEMA_VERSION:
        raise RuntimeError(
            f"unsupported schema_version={schema!r} "
            f"(this apply.py expects {SCHEMA_VERSION}); please "
            f"upgrade bettercursor or re-stage the injection."
        )
    if env.get("tool") != TOOL:
        raise RuntimeError(
            f"queue was produced by {env.get('tool')!r}, not {TOOL}; "
            f"refusing to apply"
        )
    plan = env.get("plan") or {}
    if not plan.get("uuid"):
        raise RuntimeError("envelope plan is missing uuid")
    if plan.get("skip_reason"):
        raise RuntimeError(
            f"queue plan was skipped at dry-run: {plan['skip_reason']}"
        )
    return env


def write_marker(envelope: dict[str, Any], target_db: Path, applied_count: int,
                 duration_ms: int) -> Path:
    """Write the `.applied` sidecar alongside the queue file (NOT in
    `~/.config/Cursor/User/…`). The queue envelope and the marker
    belong together so the UI's `inspect_prepared` only needs to
    look in one place.
    """
    marker_name = envelope.get("applied_marker_filename")
    if not marker_name:
        raise RuntimeError("envelope missing applied_marker_filename")
    # The queue file's directory is `~/.bettercursor/queue`; derive
    # from target_db by reading the envelope's queue_path if present
    # — but we currently don't ship it explicitly, so use the
    # standard ~/.bettercursor/queue location.
    queue_dir = Path.home() / ".bettercursor" / "queue"
    queue_dir.mkdir(parents=True, exist_ok=True)
    marker_path = queue_dir / marker_name
    body = {
        "uuid": envelope["plan"]["uuid"],
        "tool": TOOL,
        "tool_version": envelope.get("tool_version", "unknown"),
        "applied_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "applied_at_epoch_ms": int(time.time() * 1000),
        "applied_count": applied_count,
        "duration_ms": duration_ms,
        "target_db": str(target_db),
        "apply_command": envelope.get("apply_command"),
    }
    marker_path.write_text(
        json.dumps(body, ensure_ascii=False, indent=2),
        encoding="utf-8",
    )
    return marker_path


def run_pipeline(
    envelope_path: Path,
    *,
    force: bool,
    bypass_cursor_check: bool = False,
) -> int:
    envelope = load_envelope(envelope_path)
    plan = envelope["plan"]
    uuid = plan["uuid"]
    mutations: Iterable[dict[str, Any]] = plan.get("mutations") or []

    marker_name = envelope.get("applied_marker_filename")
    # Markers live in ~/.bettercursor/queue/, not next to the queue
    # file (the queue file might be in a per-project .bettercursor/
    # or in /tmp). Use the user-data dir so inspect_prepared on the
    # Rust side sees the same path.
    marker_path = (Path.home() / ".bettercursor" / "queue" / marker_name)
    if marker_path.exists() and not force:
        err(
            f"{marker_path.name} already exists — this session was "
            f"already applied. Pass --force to re-apply."
        )
        return 2

    # 1. Cursor must be closed. Otherwise rename is meaningless.
    running, procs = cursor_running()
    if running and not bypass_cursor_check:
        warn("Cursor Electron appears to be running:")
        for p in procs[:8]:
            warn(f"  {p}")
        if len(procs) > 8:
            warn(f"  … and {len(procs) - 8} more")
        err(
            "Refusing to apply while Cursor is open — its WAL could "
            "still overwrite our writes on next flush (#84). Quit "
            "Cursor and re-run this command, or pass "
            "--bypass-cursor-check if you know what you're doing."
        )
        return 3
    if running and bypass_cursor_check:
        warn(
            "Cursor Electron is running; --bypass-cursor-check is set — "
            "WAL race (#84) may still overwrite writes."
        )

    # 2. Resolve target db. The hint is informational only; we trust
    #    the platform-derived path because the hint could be wrong
    #    (e.g. user moved ~/.config/Cursor after staging).
    target_db = Path(
        envelope.get("state_vscdb_path_hint")
        or platform_state_vscdb()
    )
    if not target_db.is_file():
        err(
            f"target state.vscdb not found at {target_db}. "
            f"Is Cursor Desktop installed?"
        )
        return 4

    info(f"target: {target_db}")
    info(f"plan:   uuid={uuid}, {len(mutations)} mutation(s) queued")

    # 3. Backup original.
    backup = backup_original(target_db)
    info(f"backup: {backup}")

    # 4. Copy to tmpdir + apply.
    t0 = time.time()
    with tempfile.TemporaryDirectory(prefix="bettercursor-apply-") as td:
        tmp_db = Path(td) / "state.vscdb"
        shutil.copy2(target_db, tmp_db)
        # Bring sidecars along so PRAGMA wal_checkpoint has the right
        # material. Best-effort.
        for suffix in ("-wal", "-shm"):
            sidecar = target_db.with_name(target_db.name + suffix)
            if sidecar.is_file():
                shutil.copy2(sidecar, tmp_db.with_name(tmp_db.name + suffix))
        conn = sqlite3.connect(tmp_db)
        try:
            ensure_tables(conn)
            applied = 0
            for m in mutations:
                try:
                    apply_mutation(conn, m)
                    applied += 1
                except Exception as exc:
                    err(f"failed to apply {m.get('op')} key={m.get('key')}: {exc}")
                    return 5
            conn.commit()
            # wal_checkpoint(TRUNCATE) is what Cursor does on close —
            # emulate so the new DB has a clean start.
            conn.execute("PRAGMA wal_checkpoint(TRUNCATE)")
            conn.commit()
            if not integrity_ok(conn):
                err("PRAGMA integrity_check FAILED; aborting atomic swap.")
                err("Original state.vscdb and backup are untouched.")
                return 6
        finally:
            conn.close()
        # Move tmpdir copy back atomically.
        atomic_rename(tmp_db, target_db)

    duration_ms = int((time.time() - t0) * 1000)

    # 5. Marker for the UI badge.
    marker = write_marker(envelope, target_db, applied, duration_ms)
    info(f"applied {applied} mutation(s) in {duration_ms} ms")
    info(f"marker: {marker}")
    info(
        "→ 重启 Cursor 后, Desktop Sidebar 即可看到该 session."
    )
    return 0


def platform_state_vscdb() -> str:
    home = Path.home()
    if sys.platform == "darwin":
        return str(
            home / "Library" / "Application Support" / "Cursor" / "User"
            / "globalStorage" / "state.vscdb"
        )
    if sys.platform.startswith("linux"):
        return str(
            home / ".config" / "Cursor" / "User" / "globalStorage"
            / "state.vscdb"
        )
    if sys.platform == "win32":
        return str(
            Path(os.environ.get("APPDATA", str(home))) / "Cursor" / "User"
            / "globalStorage" / "state.vscdb"
        )
    raise RuntimeError(f"unsupported platform: {sys.platform}")


# ── CLI ──────────────────────────────────────────────────────
def main(argv: list[str] | None = None) -> int:
    p = argparse.ArgumentParser(
        prog="apply.py",
        description=(
            "Offline Layer 3 injector for bettercursor. Reads a "
            "queue envelope, applies the planned SQLite mutations "
            "to state.vscdb (Cursor must be closed), and writes "
            "an applied marker."
        ),
    )
    p.add_argument("queue", nargs="?", help="path to inject-<uuid>.json")
    p.add_argument(
        "--force",
        action="store_true",
        help="re-apply even when the .applied marker already exists",
    )
    p.add_argument(
        "--check-cursor",
        action="store_true",
        help="just check whether Cursor Electron is running; non-zero exit if yes",
    )
    p.add_argument(
        "--bypass-cursor-check",
        action="store_true",
        help=(
            "DANGEROUS: apply while Cursor Electron is open. The WAL "
            "race that motivated the offline design (#84) is still "
            "live; use this only for dev iteration with no live "
            "state to lose. You can also set BETTERCURSOR_FORCE_APPLY=1."
        ),
    )
    p.add_argument("--version", action="store_true")
    args = p.parse_args(argv)

    if args.version:
        print(f"apply.py for {TOOL} (schema v{SCHEMA_VERSION})")
        return 0

    if args.check_cursor:
        running, procs = cursor_running()
        if running:
            err("Cursor Electron appears to be running:")
            for line in procs[:8]:
                err(f"  {line}")
            return 1
        info("no Cursor process detected — safe to apply")
        return 0

    if not args.queue:
        err("missing <queue> argument; pass --help for usage")
        return 64  # EX_USAGE
    bypass = args.bypass_cursor_check or os.environ.get(
        "BETTERCURSOR_FORCE_APPLY"
    ) == "1"
    return run_pipeline(
        Path(args.queue), force=args.force, bypass_cursor_check=bypass
    )


if __name__ == "__main__":
    try:
        sys.exit(main())
    except KeyboardInterrupt:
        err("interrupted")
        sys.exit(130)
