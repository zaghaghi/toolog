import { describe, expect, test } from "vitest";

import type { AgentGroup, SessionGroup } from "./bindings";
import { agentKey, Plan, sessionKey } from "./plan";
import { emptyFilter } from "./view";

function agent(id: string, calls: number, name = "Explore"): AgentGroup {
  return { agent_id: id, agent_name: name, calls, first_at: 1, last_at: 2 };
}

function group(id: string, calls: number, mainThread: number, agents: AgentGroup[] = []): SessionGroup {
  return {
    session_id: id,
    project_path: `/work/${id}`,
    git_branch: "main",
    slug: null,
    cc_version: null,
    calls,
    main_thread_calls: mainThread,
    failures: 0,
    refusals: 0,
    first_at: 1,
    last_at: 2,
    cost_usd_micros: null,
    agents,
  };
}

/** Walk every index and describe what the plan puts there. */
function shape(plan: Plan): string[] {
  const out: string[] = [];
  for (let i = 0; i < plan.total; i += 1) {
    const item = plan.at(i);
    if (item === null) out.push("null");
    else if (item.kind === "session") out.push(`S:${item.group.session_id}`);
    else if (item.kind === "agent") out.push(`A:${item.agent.agent_id}`);
    else out.push(`${item.block.key}[${item.offset}]`);
  }
  return out;
}

describe("the flat timeline", () => {
  test("is one block, one row per index", () => {
    const plan = Plan.flat(emptyFilter(), 3);
    expect(plan.total).toBe(3);
    expect(shape(plan)).toEqual(["all[0]", "all[1]", "all[2]"]);
    expect(plan.at(3)).toBeNull();
    expect(plan.at(-1)).toBeNull();
  });

  test("an empty result has no items at all", () => {
    expect(Plan.flat(emptyFilter(), 0).total).toBe(0);
  });
});

describe("grouping by session and subagent (task 5.10)", () => {
  const groups = [
    group("s1", 5, 3, [agent("a1", 2)]),
    group("s2", 2, 2),
  ];

  test("each group's rows sit under its own header, offset from zero", () => {
    const plan = Plan.grouped(emptyFilter(), groups, new Set());
    expect(shape(plan)).toEqual([
      "S:s1",
      "s:s1/main[0]",
      "s:s1/main[1]",
      "s:s1/main[2]",
      "A:a1",
      "s:s1/a:a1/rows[0]",
      "s:s1/a:a1/rows[1]",
      "S:s2",
      "s:s2/main[0]",
      "s:s2/main[1]",
    ]);
  });

  test("every call appears exactly once across the groups", () => {
    const plan = Plan.grouped(emptyFilter(), groups, new Set());
    const rows = shape(plan).filter((s) => s.includes("["));
    expect(rows.length).toBe(groups.reduce((n, g) => n + g.calls, 0));
    expect(new Set(rows).size).toBe(rows.length);
  });

  test("a block's filter narrows to exactly its own calls", () => {
    const base = { ...emptyFilter(), tool_name: "Bash" };
    const plan = Plan.grouped(base, groups, new Set());
    const main = plan.at(1);
    const sub = plan.at(5);
    if (main?.kind !== "row" || sub?.kind !== "row") throw new Error("expected rows");

    // The user's filter is still in force inside every group.
    expect(main.block.filter.tool_name).toBe("Bash");
    expect(main.block.filter.session_id).toBe("s1");
    expect(main.block.filter.main_thread).toBe(true);

    expect(sub.block.filter.agent_id).toBe("a1");
    expect(sub.block.filter.main_thread).toBeNull();
    expect(sub.block.indent).toBe(1);
  });

  test("collapsing a session removes its rows and its subagents", () => {
    const plan = Plan.grouped(emptyFilter(), groups, new Set([sessionKey(groups[0]!)]));
    expect(shape(plan)).toEqual(["S:s1", "S:s2", "s:s2/main[0]", "s:s2/main[1]"]);
  });

  test("collapsing one subagent leaves the rest of its session alone", () => {
    const collapsed = new Set([agentKey(groups[0]!, groups[0]!.agents[0]!)]);
    const plan = Plan.grouped(emptyFilter(), groups, collapsed);
    expect(shape(plan)).toEqual([
      "S:s1",
      "s:s1/main[0]",
      "s:s1/main[1]",
      "s:s1/main[2]",
      "A:a1",
      "S:s2",
      "s:s2/main[0]",
      "s:s2/main[1]",
    ]);
  });

  test("a session with no main-thread calls still shows its header and subagent", () => {
    const onlySub = [group("s3", 2, 0, [agent("a2", 2)])];
    expect(shape(Plan.grouped(emptyFilter(), onlySub, new Set()))).toEqual([
      "S:s3",
      "A:a2",
      "s:s3/a:a2/rows[0]",
      "s:s3/a:a2/rows[1]",
    ]);
  });

  test("a group whose session was never learned is asked for by name, not by silence", () => {
    const unattributed = { ...group("x", 1, 1), session_id: null, project_path: null };
    const plan = Plan.grouped(emptyFilter(), [unattributed], new Set());
    const row = plan.at(1);
    if (row?.kind !== "row") throw new Error("expected a row");
    expect(row.block.filter.session_id).toBeNull();
    expect(row.block.filter.session_unknown).toBe(true);
  });

  test("index lookup is stable across a large plan", () => {
    const many = Array.from({ length: 400 }, (_, i) => group(`s${i}`, 250, 250));
    const plan = Plan.grouped(emptyFilter(), many, new Set());
    expect(plan.total).toBe(400 * 251);

    // The first row of the last session: 399 headers + 399*250 rows before it.
    const first = 399 * 251 + 1;
    const item = plan.at(first);
    if (item?.kind !== "row") throw new Error("expected a row");
    expect(item.block.key).toBe("s:s399/main");
    expect(item.offset).toBe(0);
    expect(plan.at(first + 249)).toMatchObject({ offset: 249 });
  });
});
