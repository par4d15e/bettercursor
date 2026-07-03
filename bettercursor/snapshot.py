"""bettercursor snapshot codec — encode/decode session snapshot JSON.

BORROWED FROM: vendored/cursaves/cursor_saves/export.py (snapshot schema)
                  + cursor_saves/importer.py (read_snapshot_file)

What we borrowed:
  - Snapshot JSON shape (version, exportedAt, source*, composerId,
    composerData, contentBlobs, bubbleEntries, checkpoints, agentBlobs,
    transcript, messageContexts).
  - gzip compression with size sharding (we only use gzip for simplicity;
    sharding deferred until needed).
  - .meta.json sidecar for cheap listings.

What we changed:
  - No sharding (90 MB per shard is cursaves' GitHub-specific concern;
    we sync over SSH pipe, no file size limit).
  - Add `source_endpoint` field (Mac vs Linux CLI vs Linux Desktop) to
    distinguish origins for the canonical merge.
  - Strict schema version (3, matching cursaves v3).
"""

from __future__ import annotations

import base64
import gzip
import json
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Optional

SNAPSHOT_VERSION = 3


@dataclass
class Snapshot:
    """A self-contained snapshot of one Cursor session."""

    version: int = SNAPSHOT_VERSION
    exported_at: str = ""
    source_machine: str = ""
    source_host: Optional[str] = None
    source_endpoint: str = ""  # "mac" | "linux_cli" | "linux_desktop"
    source_project_path: str = ""
    project_identifier: str = ""
    composer_id: str = ""
    composer_data: dict = field(default_factory=dict)
    content_blobs: dict = field(default_factory=dict)        # hash -> base64 str
    bubble_entries: dict = field(default_factory=dict)       # bubble_id -> dict
    checkpoints: dict = field(default_factory=dict)          # cp_id -> dict
    agent_blobs: dict = field(default_factory=dict)          # hex -> base64 str
    transcript: Optional[str] = None
    message_contexts: dict = field(default_factory=dict)

    # ── (de)serialization ─────────────────────────────────────

    def to_dict(self) -> dict:
        return {
            "version": self.version,
            "exportedAt": self.exported_at,
            "sourceMachine": self.source_machine,
            "sourceHost": self.source_host,
            "sourceEndpoint": self.source_endpoint,
            "sourceProjectPath": self.source_project_path,
            "projectIdentifier": self.project_identifier,
            "composerId": self.composer_id,
            "composerData": self.composer_data,
            "contentBlobs": self.content_blobs,
            "bubbleEntries": self.bubble_entries,
            "checkpoints": self.checkpoints,
            "agentBlobs": self.agent_blobs,
            "transcript": self.transcript,
            "messageContexts": self.message_contexts,
        }

    @classmethod
    def from_dict(cls, d: dict) -> "Snapshot":
        return cls(
            version=d.get("version", SNAPSHOT_VERSION),
            exported_at=d.get("exportedAt", ""),
            source_machine=d.get("sourceMachine", ""),
            source_host=d.get("sourceHost"),
            source_endpoint=d.get("sourceEndpoint", ""),
            source_project_path=d.get("sourceProjectPath", ""),
            project_identifier=d.get("projectIdentifier", ""),
            composer_id=d.get("composerId", ""),
            composer_data=d.get("composerData", {}),
            content_blobs=d.get("contentBlobs", {}),
            bubble_entries=d.get("bubbleEntries", {}),
            checkpoints=d.get("checkpoints", {}),
            agent_blobs=d.get("agentBlobs", {}),
            transcript=d.get("transcript"),
            message_contexts=d.get("messageContexts", {}),
        )

    def is_empty(self) -> bool:
        """A snapshot of an empty draft (no real content yet)."""
        return (
            not self.composer_data.get("fullConversationHeadersOnly")
            and not self.composer_data.get("name")
        )


def encode_snapshot(snap: Snapshot) -> bytes:
    """Encode snapshot as gzipped JSON."""
    raw = json.dumps(snap.to_dict(), ensure_ascii=False).encode("utf-8")
    import io
    buf = io.BytesIO()
    with gzip.GzipFile(fileobj=buf, mode="wb", compresslevel=9) as f:
        f.write(raw)
    return buf.getvalue()


def decode_snapshot(blob: bytes) -> Snapshot:
    """Decode a gzipped JSON snapshot."""
    raw = gzip.decompress(blob)
    return Snapshot.from_dict(json.loads(raw))


def save_snapshot(snap: Snapshot, snapshots_dir: Path, compress: bool = True) -> Path:
    """Save snapshot to <snapshots_dir>/<project_id>/<composer_id>.json[.gz].

    Also writes a small .meta.json sidecar for cheap listings.
    """
    project_dir = snapshots_dir / snap.project_identifier
    project_dir.mkdir(parents=True, exist_ok=True)

    target = project_dir / f"{snap.composer_id}.json.gz" if compress \
             else project_dir / f"{snap.composer_id}.json"

    # Remove any previous file or shards
    for old in project_dir.glob(f"{snap.composer_id}.json*"):
        if not old.name.endswith(".meta.json"):
            old.unlink(missing_ok=True)

    if compress:
        target.write_bytes(encode_snapshot(snap))
    else:
        target.write_text(json.dumps(snap.to_dict(), ensure_ascii=False))

    # Write meta sidecar
    sidecar = {
        "composerId": snap.composer_id,
        "name": snap.composer_data.get("name"),
        "messageCount": len(snap.composer_data.get("fullConversationHeadersOnly", [])),
        "exportedAt": snap.exported_at,
        "sourceMachine": snap.source_machine,
        "sourceEndpoint": snap.source_endpoint,
        "sourceProjectPath": snap.source_project_path,
        "projectIdentifier": snap.project_identifier,
        "version": snap.version,
        "size": target.stat().st_size,
    }
    (project_dir / f"{snap.composer_id}.meta.json").write_text(
        json.dumps(sidecar, indent=2)
    )
    return target


def load_snapshot(path: Path) -> Snapshot:
    """Load a snapshot file (.json or .json.gz)."""
    if path.suffix == ".gz":
        return decode_snapshot(path.read_bytes())
    return Snapshot.from_dict(json.loads(path.read_text()))


def make_now_snapshot(
    *,
    composer_id: str,
    composer_data: dict,
    project_path: str,
    project_identifier: str,
    source_machine: str,
    source_endpoint: str,
    source_host: Optional[str] = None,
    content_blobs: dict | None = None,
    bubble_entries: dict | None = None,
    checkpoints: dict | None = None,
    agent_blobs_b64: dict | None = None,
    transcript: Optional[str] = None,
    message_contexts: dict | None = None,
) -> Snapshot:
    """Convenience constructor with current timestamp."""
    return Snapshot(
        exported_at=datetime.now(timezone.utc).isoformat(),
        source_machine=source_machine,
        source_host=source_host,
        source_endpoint=source_endpoint,
        source_project_path=project_path,
        project_identifier=project_identifier,
        composer_id=composer_id,
        composer_data=composer_data,
        content_blobs=content_blobs or {},
        bubble_entries=bubble_entries or {},
        checkpoints=checkpoints or {},
        agent_blobs=agent_blobs_b64 or {},
        transcript=transcript,
        message_contexts=message_contexts or {},
    )