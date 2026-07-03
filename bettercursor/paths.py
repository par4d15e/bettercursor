"""bettercursor path resolution — where Cursor stores its 4 layers.

BORROWED FROM: vendored/cursaves/cursor_saves/paths.py

What we borrowed:
  - get_cursor_user_dir() : platform-aware base directory
  - get_global_db_path()  : ~/.config/Cursor/User/globalStorage/state.vscdb (Linux)
                            ~/Library/Application Support/Cursor/User/... (Mac)
  - get_chats_dir()       : ~/.cursor/chats/<md5(cwd)>/<uuid>/
                            (cursaves doesn't have this — we add it for Layer 2)
  - get_project_identifier() : git remote URL → sanitized name
  - get_machine_id()      : socket.gethostname()

What we changed:
  - Drop SSH workspace detection (cursaves has vscode-remote:// parsing
    we don't need — bettercursor runs on Linux, doesn't open Mac workspaces).
  - Add Layer 2 helpers: chat_root = MD5(cwd), chats_dir, transcript dir.
  - Drop workspaces enumeration (we watch specific known paths, not scan).
  - Simpler error handling — print + sys.exit(1) on missing dirs, no
    interactive prompts (cursaves has interactive TUI logic).
"""

from __future__ import annotations

import hashlib
import os
import platform
import re
import socket
import subprocess
import sys
from pathlib import Path
from typing import Optional


# ── Platform paths ─────────────────────────────────────────────

def get_cursor_user_dir() -> Path:
    """Return the Cursor User data directory for the current platform."""
    system = platform.system()
    if system == "Darwin":
        return Path.home() / "Library" / "Application Support" / "Cursor" / "User"
    if system == "Linux":
        return Path.home() / ".config" / "Cursor" / "User"
    print(f"Error: unsupported platform '{system}'.", file=sys.stderr)
    sys.exit(1)


def get_global_db_path() -> Path:
    """Path to Layer 3 SQLite: ~/.config/Cursor/User/globalStorage/state.vscdb (Linux)."""
    return get_cursor_user_dir() / "globalStorage" / "state.vscdb"


def get_workspace_storage_dir() -> Path:
    """Path to Layer 3 workspace storage dir (one state.vscdb per workspace)."""
    return get_cursor_user_dir() / "workspaceStorage"


def get_workspace_db(workspace_hash: str) -> Path:
    """Path to a specific workspace's state.vscdb."""
    return get_workspace_storage_dir() / workspace_hash / "state.vscdb"


# ── Layer 2 (cursor-agent CLI) ────────────────────────────────

def get_cursor_projects_dir() -> Path:
    """~/.cursor/projects/<slug>/agent-transcripts/<uuid>/<uuid>.jsonl (Layer 1)."""
    return Path.home() / ".cursor" / "projects"


def get_chats_dir() -> Path:
    """~/.cursor/chats/ — parent of all <md5(cwd)>/<uuid>/store.db directories."""
    return Path.home() / ".cursor" / "chats"


def chat_root_for(cwd: str | Path) -> str:
    """MD5 hex of cwd; identifies a project's Layer 2 root."""
    return hashlib.md5(str(cwd).encode("utf-8")).hexdigest()


def chat_dir_for(cwd: str | Path, composer_id: str) -> Path:
    """Layer 2 path for a specific session: ~/.cursor/chats/<md5>/<uuid>/."""
    return get_chats_dir() / chat_root_for(cwd) / composer_id


def store_db_for(cwd: str | Path, composer_id: str) -> Path:
    """The store.db file inside a chat_dir."""
    return chat_dir_for(cwd, composer_id) / "store.db"


def transcript_for(project_slug: str, composer_id: str) -> Path:
    """Layer 1 path: ~/.cursor/projects/<slug>/agent-transcripts/<uuid>/<uuid>.jsonl."""
    sanitized = sanitize_project_path(project_slug)
    return get_cursor_projects_dir() / sanitized / "agent-transcripts" / f"{composer_id}.jsonl"


# ── Project identification (borrowed) ────────────────────────

def get_project_identifier(project_path: str | Path) -> str:
    """Get a stable identifier for a project (used as snapshot subdir name).

    Strategy (same as cursaves):
      1. Use git remote origin URL if available → normalized to filesystem-safe.
      2. Fall back to directory basename.
    """
    remote_url = _get_git_remote_url(project_path)
    if remote_url:
        return _normalize_remote_url(remote_url)
    return os.path.basename(os.path.normpath(str(project_path)))


def _get_git_remote_url(project_path: str | Path) -> Optional[str]:
    try:
        result = subprocess.run(
            ["git", "-C", str(project_path), "config", "--get", "remote.origin.url"],
            capture_output=True, text=True, timeout=5,
        )
        if result.returncode == 0 and result.stdout.strip():
            return result.stdout.strip()
    except (FileNotFoundError, subprocess.TimeoutExpired):
        pass
    return None


def _normalize_remote_url(url: str) -> str:
    """git@github.com:user/repo.git  →  github.com-user-repo"""
    url = re.sub(r"\.git$", "", url)
    m = re.match(r"^[\w.-]+@([\w.-]+):(.*)", url)
    if m:
        return _sanitize_identifier(f"{m.group(1)}/{m.group(2)}")
    m = re.match(r"^(?:https?|ssh)://(?:[\w.-]+@)?([\w.-]+)/(.*)", url)
    if m:
        return _sanitize_identifier(f"{m.group(1)}/{m.group(2)}")
    return _sanitize_identifier(url)


def _sanitize_identifier(s: str) -> str:
    s = re.sub(r"[/:@\\]+", "-", s)
    s = re.sub(r"-+", "-", s)
    return s.strip("-")


def sanitize_project_path(project_path: str) -> str:
    """/Users/x/y → Users-x-y (cursaves' format for Layer 1 path segment)."""
    return project_path.strip("/").replace("/", "-")


# ── Machine & system ──────────────────────────────────────────

def get_machine_id() -> str:
    """Hostname used for snapshot metadata."""
    return socket.gethostname()


def get_bettercursor_dir() -> Path:
    """~/.bettercursor/ — daemon state, canonical sessions cache, archive."""
    p = Path.home() / ".bettercursor"
    p.mkdir(parents=True, exist_ok=True)
    return p


def get_snapshots_dir() -> Path:
    """~/.bettercursor/snapshots/ — analogous to cursaves' ~/.cursaves/snapshots/.

    We use ~/.bettercursor/ (not ~/.cursaves/) so we don't conflict if the user
    also uses cursaves. Snapshots here are written by bettercursor-syncd when
    it imports from Mac/Linux Desktop, and read by Phase 1 daemon for merge.
    """
    p = get_bettercursor_dir() / "snapshots"
    p.mkdir(parents=True, exist_ok=True)
    return p


def get_canonical_sessions_path() -> Path:
    """~/.bettercursor/canonical.json — merged view across all sources."""
    return get_bettercursor_dir() / "canonical.json"


def get_archive_dir() -> Path:
    """~/.bettercursor/archive/<composer_id>/ — for diverged/conflict snapshots."""
    p = get_bettercursor_dir() / "archive"
    p.mkdir(parents=True, exist_ok=True)
    return p