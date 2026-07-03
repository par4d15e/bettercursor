"""bettercursor conflict detection — decide whether to skip / merge / override.

BORROWED FROM: vendored/cursaves/cursor_saves/importer.py:_check_conflict

What we borrowed:
  - 5-way classification: new / identical / incoming_newer / local_ahead / diverged.
  - Comparison via bubble ID set + conversation header set.

What we changed:
  - Add lastUpdatedAt timestamp fallback (cursaves only compares bubble counts;
    we compare timestamps as a tie-breaker when bubble sets overlap).
  - Add Layer 2 state (cursaves only checks Layer 3 global DB).
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Optional


# ── Conflict classification ──────────────────────────────────

NEW             = "new"             # local doesn't have this composer
IDENTICAL       = "identical"       # same bubbles + same headers
INCOMING_NEWER  = "incoming_newer"  # incoming has bubbles/headers local doesn't
LOCAL_AHEAD     = "local_ahead"     # local has bubbles/headers incoming doesn't
DIVERGED        = "diverged"        # both sides have unique content


@dataclass
class LocalState:
    """What we know about a composer locally before deciding."""
    has_composer_data: bool
    local_bubble_ids: set[str]
    local_header_ids: set[str]
    local_last_updated_at: int  # ms epoch


@dataclass
class IncomingState:
    """What the incoming snapshot contains."""
    incoming_bubble_ids: set[str]
    incoming_header_ids: set[str]
    incoming_last_updated_at: int  # ms epoch


def classify(local: LocalState, incoming: IncomingState) -> str:
    """Return one of NEW / IDENTICAL / INCOMING_NEWER / LOCAL_AHEAD / DIVERGED."""
    if not local.has_composer_data and not local.local_bubble_ids:
        return NEW

    if not incoming.incoming_bubble_ids and not incoming.incoming_header_ids:
        return LOCAL_AHEAD  # nothing new from incoming

    local_only_bubbles = local.local_bubble_ids - incoming.incoming_bubble_ids
    incoming_only_bubbles = incoming.incoming_bubble_ids - local.local_bubble_ids
    incoming_only_headers = incoming.incoming_header_ids - local.local_header_ids

    has_local_only = bool(local_only_bubbles)
    has_incoming_only = bool(incoming_only_bubbles) or bool(incoming_only_headers)

    if not has_local_only and not has_incoming_only:
        return IDENTICAL
    if has_local_only and has_incoming_only:
        return DIVERGED
    if has_local_only:
        return LOCAL_AHEAD
    return INCOMING_NEWER


def should_skip(state: str) -> bool:
    """Whether to skip writing this snapshot (no change needed)."""
    return state in (IDENTICAL, LOCAL_AHEAD)


def should_archive_incoming(state: str) -> bool:
    """Whether the incoming snapshot should be archived (divergence handling)."""
    return state == DIVERGED


def should_apply_last_writer_wins(
    local: LocalState, incoming: IncomingState, classification: str
) -> bool:
    """Within non-trivial cases, decide by timestamp (tie-breaker).

    Used after classify() says INCOMING_NEWER or LOCAL_AHEAD: if timestamps
    disagree with bubble set comparison, trust the newer timestamp.
    """
    if classification in (NEW, IDENTICAL, DIVERGED):
        return False  # those are handled by other branches
    if classification == LOCAL_AHEAD:
        # bubble set says local ahead, but if incoming is much newer,
        # still apply (e.g. incoming has same # bubbles but newer header).
        return incoming.incoming_last_updated_at > local.local_last_updated_at + 5000
    if classification == INCOMING_NEWER:
        return incoming.incoming_last_updated_at >= local.local_last_updated_at
    return False