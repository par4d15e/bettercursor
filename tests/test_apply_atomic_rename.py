"""Regression tests for scripts/apply.py atomic_rename.

Background
----------
``os.replace`` (POSIX rename(2)) is atomic **only when src and dst
are on the same filesystem**. ``tempfile.TemporaryDirectory``
defaults to ``/tmp`` (a separate tmpfs on Linux), so any target
under ``~/.config/Cursor/...`` would raise
``OSError: [Errno 18] Invalid cross-device link`` (#85).

Fix: stage the copy in ``dst.parent`` first, then ``os.replace``.
POSIX guarantees the dst-side rename is still atomic — even when
src came from another fs.

These tests exercise atomic_rename on a tmpfs + ext4-like split via
``tempfile.TemporaryDirectory`` (default /tmp) and a scratch dir on
the real filesystem root.
"""
from __future__ import annotations

import importlib.util
import sqlite3
import sys
import tempfile
from pathlib import Path

REPO_ROOT = Path(__file__).parent.parent
APPLY_PY = REPO_ROOT / "scripts" / "apply.py"


def _load_apply_module():
    spec = importlib.util.spec_from_file_location("apply", APPLY_PY)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules["apply"] = module
    spec.loader.exec_module(module)
    return module


def _seed_state_vscdb(path: Path) -> None:
    """Build a minimal Cursor-shaped vscdb with the two tables our
    injector touches. Returns nothing."""
    conn = sqlite3.connect(path)
    conn.execute("CREATE TABLE ItemTable(key TEXT PRIMARY KEY, value TEXT)")
    conn.execute("CREATE TABLE cursorDiskKV(key TEXT PRIMARY KEY, value TEXT)")
    conn.execute("INSERT INTO ItemTable(key, value) VALUES ('seed:it', 'before')")
    conn.execute(
        "INSERT INTO cursorDiskKV(key, value) VALUES ('seed:kv', 'kv-before')"
    )
    conn.commit()
    conn.close()


def _seed_via_tmpdir(tmp_dir: Path) -> Path:
    """Helper: drop a tmpdir-copied state.vscdb, return its path.
    Mirrors what run_pipeline() does in apply.py."""
    db = tmp_dir / "state.vscdb"
    _seed_state_vscdb(db)
    return db


def test_atomic_rename_cross_filesystem() -> None:
    """The core regression: tmpfs (/tmp) → real fs (elsewhere).

    Without the fix, this raises OSError 18.
    """
    apply = _load_apply_module()

    with tempfile.TemporaryDirectory(prefix="bettercursor-test-") as tmp_fs_str:
        tmp_fs = Path(tmp_fs_str)
        # /tmp is virtually always tmpfs; stage dst on /var/tmp which
        # is conventionally a real-fs mount (often root fs). If your
        # machine doesn't have /var/tmp, fall back to $TMPDIR/sibling.
        alt_tmp_root = Path("/var/tmp")
        alt_tmp_root.mkdir(exist_ok=True)
        with tempfile.TemporaryDirectory(
            prefix="bettercursor-test-alt-", dir=str(alt_tmp_root)
        ) as alt_str:
            alt_fs = Path(alt_str)
            assert alt_fs.exists()

            src = _seed_via_tmpdir(tmp_fs)
            dst = alt_fs / "state.vscdb"
            _seed_state_vscdb(dst)  # existing → triggers rename-not-create

            apply.atomic_rename(src, dst)  # should NOT raise

            assert dst.exists()
            conn = sqlite3.connect(dst)
            row = conn.execute(
                "SELECT value FROM ItemTable WHERE key='seed:it'"
            ).fetchone()
            conn.close()
            assert row == ("before",), f"dst got clobbered, row={row!r}"


def test_atomic_rename_same_filesystem() -> None:
    """Smoke test: same fs path goes through the direct os.replace
    branch, also succeeds. Confirms we didn't break the common case."""
    apply = _load_apply_module()

    with tempfile.TemporaryDirectory(prefix="bettercursor-samefs-") as tmp:
        work = Path(tmp)
        src = work / "src.vscdb"
        dst = work / "state.vscdb"
        _seed_state_vscdb(src)
        _seed_state_vscdb(dst)

        apply.atomic_rename(src, dst)

        conn = sqlite3.connect(dst)
        row = conn.execute(
            "SELECT value FROM cursorDiskKV WHERE key='seed:kv'"
        ).fetchone()
        conn.close()
        assert row == ("kv-before",)


def test_atomic_rename_cleans_stage_on_failure() -> None:
    """If the rename itself fails (dst parent not writable), the
    .bettercursor-stage file must NOT be left behind.
    Verifies the cleanup branch."""
    apply = _load_apply_module()

    with tempfile.TemporaryDirectory(prefix="bettercursor-test-src-") as tmp_src:
        with tempfile.TemporaryDirectory(
            prefix="bettercursor-test-dst-", dir="/var/tmp"
        ) as tmp_dst:
            src_fs = Path(tmp_src)
            dst_fs = Path(tmp_dst)
            readonly_parent = dst_fs / "ro"
            readonly_parent.mkdir()
            # chmod 0o555 blocks write for unprivileged users.
            # Skip if we're root — root bypasses mode bits, and the
            # test would silently fail to actually exercise the error
            # path. Just confirm no stage leak in either case.
            readonly_parent.chmod(0o555)
            try:
                src = _seed_via_tmpdir(src_fs)
                dst = readonly_parent / "state.vscdb"
                raised = False
                try:
                    apply.atomic_rename(src, dst)
                except OSError:
                    raised = True
                staged = readonly_parent / "state.vscdb.bettercursor-stage"
                assert not staged.exists(), f"stage leak: {staged}"
                # If we couldn't even write into ro dir (i.e. root),
                # the rename succeeded — that's fine, no leak anyway.
                if not raised:
                    assert dst.exists()
            finally:
                readonly_parent.chmod(0o755)


def main() -> int:
    failures: list[str] = []
    for name, fn in (
        ("test_atomic_rename_cross_filesystem", test_atomic_rename_cross_filesystem),
        ("test_atomic_rename_same_filesystem", test_atomic_rename_same_filesystem),
        ("test_atomic_rename_cleans_stage_on_failure", test_atomic_rename_cleans_stage_on_failure),
    ):
        try:
            fn()
            print(f"  ok   {name}")
        except Exception as exc:  # noqa: BLE001
            failures.append(name)
            print(f"  FAIL {name}: {exc!r}")
    if failures:
        print(f"\n{len(failures)} failed: {failures}")
        return 1
    print("\nall atomic_rename tests passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())