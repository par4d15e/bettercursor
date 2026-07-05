import { describe, expect, it } from "vitest";
import type { CanonicalSession } from "../lib/types";
import {
  ancestorSessionIds,
  buildSessionTree,
  countSessionTreeNodes,
  groupSessionsByProject,
} from "./sessionStore";

function sess(
  uuid: string,
  slug: string,
  opts: Partial<CanonicalSession> = {},
): CanonicalSession {
  return {
    uuid,
    project_slug: slug,
    project_path: "",
    chat_root: "",
    name: uuid.slice(0, 8),
    last_updated_at: 1,
    bubble_count: 1,
    is_empty_draft: false,
    is_broken: false,
    sources: {},
    first_user_message_preview: "",
    files_referenced: [],
    indexable_text: "",
    layer_3_present: false,
    ...opts,
  };
}

describe("buildSessionTree", () => {
  it("nests all subagents flat under rootParentAgentId", () => {
    const root = "33de2d97-940e-4335-a4ab-1f1a5b63243c";
    const mid = "f57a5d17-ccee-4e6b-94f2-4e97a0c7771a";
    const leaf = "804431cd-ff96-4562-a85d-f862e0cba564";
    const tree = buildSessionTree([
      sess(leaf, "proj", {
        is_subagent: true,
        subagent_info: {
          parent_agent_id: mid,
          root_parent_agent_id: root,
        },
      }),
      sess(mid, "proj", {
        is_subagent: true,
        subagent_info: {
          parent_agent_id: root,
          root_parent_agent_id: root,
        },
      }),
      sess(root, "proj"),
    ]);
    expect(tree).toHaveLength(1);
    expect(tree[0].session.uuid).toBe(root);
    expect(tree[0].children.map((n) => n.session.uuid).sort()).toEqual(
      [mid, leaf].sort(),
    );
    expect(tree[0].children.every((n) => n.children.length === 0)).toBe(true);
  });

  it("falls back to top-level when root parent is absent", () => {
    const root = "33de2d97-940e-4335-a4ab-1f1a5b63243c";
    const leaf = "975d32b0-4adb-4c5e-ae4f-cc7d012f036a";
    const tree = buildSessionTree([
      sess(leaf, "proj", {
        is_subagent: true,
        subagent_info: {
          parent_agent_id: root,
          root_parent_agent_id: root,
        },
      }),
    ]);
    expect(tree).toHaveLength(1);
    expect(tree[0].session.uuid).toBe(leaf);
    expect(tree[0].children).toHaveLength(0);
  });
});

describe("groupSessionsByProject", () => {
  it("keeps subagents inside the same project group tree", () => {
    const root = "33de2d97-940e-4335-a4ab-1f1a5b63243c";
    const leaf = "975d32b0-4adb-4c5e-ae4f-cc7d012f036a";
    const grouped = groupSessionsByProject([
      sess(root, "bettercursor"),
      sess(leaf, "bettercursor", {
        is_subagent: true,
        subagent_info: {
          parent_agent_id: root,
          root_parent_agent_id: root,
        },
      }),
    ]);
    expect(grouped).toHaveLength(1);
    expect(countSessionTreeNodes(grouped[0].sessions)).toBe(2);
    expect(grouped[0].sessions[0].children).toHaveLength(1);
  });
});

describe("ancestorSessionIds", () => {
  it("returns parent chain for nested selection", () => {
    const root = "33de2d97-940e-4335-a4ab-1f1a5b63243c";
    const leaf = "804431cd-ff96-4562-a85d-f862e0cba564";
    const grouped = groupSessionsByProject([
      sess(root, "proj"),
      sess(leaf, "proj", {
        is_subagent: true,
        subagent_info: {
          parent_agent_id: root,
          root_parent_agent_id: root,
        },
      }),
    ]);
    expect(ancestorSessionIds(grouped, leaf)).toEqual([root]);
  });
});
