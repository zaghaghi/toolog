//! The risk review (tasks 6.3 and 6.4).
//!
//! The first test is the phase's exit criterion as a sentence: an
//! auto-approved `rm -rf` is flagged, and the finding leads to the exact call.
//! The rest are about the property that makes a review trustworthy — setting a
//! rule aside changes the posture, not what is on the screen.

import { beforeEach, describe, expect, test, vi } from "vitest";

import type { Finding, RiskReview, ToolCall } from "./bindings";

const state = {
  review: null as RiskReview | null,
  dismissed: [] as [string, string][],
  restored: [] as string[],
  extraCalls: [] as ToolCall[],
  firstPage: [] as ToolCall[],
  asked: [] as [string, number][],
  revealed: 0,
  openedRule: [] as string[],
};

vi.mock("./bindings", () => ({
  risk: vi.fn(() => Promise.resolve(state.review)),
  revealRules: vi.fn(() => {
    state.revealed += 1;
    return Promise.resolve(null);
  }),
  ruleCalls: vi.fn((ruleId: string, page: { limit: number; offset: number }) => {
    state.asked.push([ruleId, page.offset]);
    // The first page is what a finding used to carry; later pages are the
    // drill-through past it.
    return Promise.resolve(page.offset === 0 ? state.firstPage : state.extraCalls);
  }),
  dismissRule: vi.fn((ruleId: string, note: string) => {
    state.dismissed.push([ruleId, note]);
    const review = state.review;
    if (review !== null) {
      state.review = {
        ...review,
        findings: review.findings.map((f) =>
          f.rule_id === ruleId ? { ...f, dismissed: { rule_id: ruleId, note, at: 1000 } } : f,
        ),
        projects: [],
      };
    }
    return Promise.resolve(state.review);
  }),
  restoreRule: vi.fn((ruleId: string) => {
    state.restored.push(ruleId);
    return Promise.resolve(state.review);
  }),
}));

const { RiskView } = await import("./risk");

function call(over: Partial<ToolCall> = {}): ToolCall {
  return {
    tool_use_id: "toolu_rm",
    session_id: "s1",
    prompt_id: null,
    message_uuid: null,
    parent_uuid: null,
    is_sidechain: null,
    agent_id: null,
    agent_name: null,
    tool_name: "Bash",
    tool_kind: "builtin",
    mcp_server: null,
    mcp_tool: null,
    called_at: Date.parse("2026-03-02T09:00:00Z"),
    completed_at: null,
    input_json: null,
    input_summary: "rm -rf /tmp/scratch",
    target_path: null,
    result_json: null,
    result_text: null,
    result_size: null,
    success: true,
    duration_ms: 12,
    error_type: null,
    decision: "accept",
    decision_source: "config",
    permission_mode: "default",
    provenance: 3,
    ...over,
  };
}

function finding(over: Partial<Finding> = {}): Finding {
  return {
    rule_id: "auto-approved-destructive-bash",
    title: "Destructive shell commands approved by a rule, not a person",
    explanation: "A permission rule let this through without anyone seeing it.",
    severity: "high",
    scope: "call",
    calls: 1,
    sessions: 1,
    projects: ["/work/scratch"],
    first_at: Date.parse("2026-03-02T09:00:00Z"),
    last_at: Date.parse("2026-03-02T09:00:00Z"),
    conditions: ["the tool is Bash", "the command's first line contains any of: rm -rf"],
    from_user: false,
    first_seen: Date.parse("2026-03-01T09:00:00Z"),
    new_calls: 0,
    unattributed_calls: 0,
    dismissed: null,
    ...over,
  };
}

function review(over: Partial<RiskReview> = {}): RiskReview {
  return {
    findings: [finding()],
    totals: [
      { severity: "high", calls: 1, rules: 1 },
      { severity: "medium", calls: 0, rules: 0 },
      { severity: "low", calls: 0, rules: 0 },
      { severity: "info", calls: 0, rules: 0 },
    ],
    projects: [
      {
        project_path: "/work/scratch",
        by_severity: [1, 0, 0, 0],
        rule_ids: ["auto-approved-destructive-bash"],
      },
    ],
    rules_path: "/Users/someone/Library/Application Support/toolog/rules.toml",
    rules_customized: false,
    first_review: false,
    ...over,
  };
}

/** Let the pending promises run — a finding fetches its calls when opened. */
const settle = async (): Promise<void> => {
  for (let i = 0; i < 4; i += 1) {
    await Promise.resolve();
    await new Promise((resolve) => setTimeout(resolve, 0));
  }
};

async function mount(data: RiskReview, onOpenCall: (id: string) => void = () => {}) {
  state.review = data;
  const view = new RiskView({ onNotice: () => {}, onOpenCall, onOpenRule: (id) => state.openedRule.push(id) });
  await view.refresh();
  return view.node;
}

beforeEach(() => {
  state.review = null;
  state.firstPage = [call()];
  state.revealed = 0;
  state.openedRule = [];
  state.dismissed = [];
  state.restored = [];
  state.extraCalls = [];
  state.asked = [];
});

describe("the exit criterion", () => {
  test("an auto-approved rm -rf is flagged, and leads to the exact call", async () => {
    const opened: string[] = [];
    const node = await mount(review(), (id) => opened.push(id));

    const head = node.querySelector(".finding-head");
    expect(head?.textContent).toContain("Destructive shell commands approved by a rule");
    expect(node.querySelector(".sev-chip")?.textContent).toBe("high");

    // Open the finding, and the call behind it arrives.
    node.querySelector<HTMLButtonElement>(".finding-head")?.click();
    await settle();
    // Task 11.2: fetched on expand, not carried on all twelve findings.
    expect(state.asked).toEqual([["auto-approved-destructive-bash", 0]]);

    const call = node.querySelector<HTMLButtonElement>(".finding-call");
    expect(call?.textContent).toContain("rm -rf /tmp/scratch");

    call?.click();
    expect(opened).toEqual(["toolu_rm"]);
  });
});

describe("severity", () => {
  test("the hero number is distinct calls, with the rule count under it", async () => {
    // Task 11.7: the summary used to count *rules*, while the table counted
    // (rule, project) pairs — so nothing added up. Both numbers are still
    // worth having; only one of them can be the total.
    const node = await mount(
      review({
        findings: [
          finding(),
          finding({ rule_id: "mcp-tool-usage", severity: "info", title: "MCP tools", calls: 62 }),
        ],
        totals: [
          { severity: "high", calls: 1, rules: 1 },
          { severity: "medium", calls: 0, rules: 0 },
          { severity: "low", calls: 0, rules: 0 },
          { severity: "info", calls: 62, rules: 1 },
        ],
      }),
    );
    const counts = [...node.querySelectorAll(".risk-count")].map((n) => [
      n.querySelector(".risk-count-label")?.textContent,
      n.querySelector(".risk-count-n")?.textContent,
      n.querySelector(".risk-count-rules")?.textContent,
    ]);
    expect(counts).toEqual([
      ["high", "1", "1 rule, 1 call"],
      ["medium", "0", "0 rules, 0 calls"],
      ["low", "0", "0 rules, 0 calls"],
      ["info", "62", "1 rule, 62 calls"],
    ]);
  });

  test("says the four numbers do not add to a total, rather than letting it be found out", async () => {
    // Task 11.9: each column reconciles with its own number; the four do not
    // sum, because a call caught at two severities is one call at each.
    const node = await mount(review());
    expect(node.querySelector(".risk-summary")?.textContent).toContain(
      "The four numbers do not add up to a total",
    );
  });

  test("a session-scoped finding counts sessions, because it is about sessions", async () => {
    const node = await mount(
      review({
        findings: [
          finding({
            rule_id: "permission-mode-changed-mid-session",
            scope: "session",
            calls: 14,
            sessions: 14,
          }),
        ],
      }),
    );
    expect(node.querySelector(".finding-count")?.textContent).toBe("14 sessions");
  });
});

describe("setting a rule aside", () => {
  test("needs a reason, and keeps the finding on the screen", async () => {
    const notices: string[] = [];
    state.review = review();
    const { RiskView: View } = await import("./risk");
    const view = new View({ onNotice: (m) => notices.push(m), onOpenCall: () => {}, onOpenRule: () => {} });
    await view.refresh();
    const node = view.node;

    node.querySelector<HTMLButtonElement>(".finding-head")?.click();

    // An empty note is refused: a dismissal without a reason is just a hidden
    // finding, which is what this view exists not to have.
    const buttons = [...node.querySelectorAll<HTMLButtonElement>(".actions button")];
    buttons[0]?.click();
    expect(state.dismissed).toEqual([]);
    expect(notices[0]).toContain("needs a reason");

    const note = node.querySelector<HTMLInputElement>(".finding-note");
    if (note !== null) note.value = "A scratch directory, deliberately.";
    buttons[0]?.click();
    await Promise.resolve();
    await Promise.resolve();

    expect(state.dismissed).toEqual([
      ["auto-approved-destructive-bash", "A scratch directory, deliberately."],
    ]);
    // Still listed, and now carrying the note.
    expect(view.node.querySelector(".finding")?.className).toContain("set-aside");
    expect(view.node.querySelector(".finding-dismissal")?.textContent).toContain(
      "A scratch directory, deliberately.",
    );
  });

  test("a set-aside finding offers to come back", async () => {
    const node = await mount(
      review({
        findings: [finding({ dismissed: { rule_id: "auto-approved-destructive-bash", note: "fine", at: 1 } })],
        projects: [],
      }),
    );
    node.querySelector<HTMLButtonElement>(".finding-head")?.click();
    const button = node.querySelector<HTMLButtonElement>(".actions button");
    expect(button?.textContent).toBe("Bring it back");
    button?.click();
    expect(state.restored).toEqual(["auto-approved-destructive-bash"]);
  });
});

describe("the drill-through", () => {
  test("offers the rest of the calls past the first page", async () => {
    const node = await mount(review({ findings: [finding({ calls: 12 })] }));
    node.querySelector<HTMLButtonElement>(".finding-head")?.click();
    await settle();

    const more = node.querySelector<HTMLButtonElement>(".finding-more");
    expect(more?.textContent).toBe("Show the other 4");

    state.extraCalls = [call({ tool_use_id: "toolu_2", input_summary: "rm -rf build" })];
    more?.click();
    await settle();

    // The rule's own matches, continuing from where the first page stopped —
    // one source of truth for "which calls did this rule catch".
    expect(state.asked).toEqual([
      ["auto-approved-destructive-bash", 0],
      ["auto-approved-destructive-bash", 8],
    ]);
  });

  test("no button when the first page is all of them", async () => {
    const node = await mount(review());
    node.querySelector<HTMLButtonElement>(".finding-head")?.click();
    await settle();
    expect(node.querySelector(".finding-more")).toBeNull();
  });
});

describe("per-project posture", () => {
  test("names the project and its worst findings", async () => {
    const node = await mount(review());
    const row = node.querySelector(".risk-projects tbody tr");
    expect(row?.querySelector("th")?.textContent).toBe("scratch");
    // Four severity columns and no row total: a call caught at two severities
    // is one call at each, so a row total would not be one either.
    const cells = [...(row?.querySelectorAll("td") ?? [])].map((c) => c.textContent);
    expect(cells).toEqual(["1", "—", "—", "—"]);
  });

  test("gives calls with no recorded project a row of their own", async () => {
    // Task 11.8: these were dropped from the table and counted in the summary,
    // which is half of why the two could not be made to agree.
    const node = await mount(
      review({
        totals: [
          { severity: "high", calls: 2, rules: 1 },
          { severity: "medium", calls: 0, rules: 0 },
          { severity: "low", calls: 0, rules: 0 },
          { severity: "info", calls: 0, rules: 0 },
        ],
        projects: [
          {
            project_path: "/work/scratch",
            by_severity: [1, 0, 0, 0],
            rule_ids: ["auto-approved-destructive-bash"],
          },
          {
            project_path: null,
            by_severity: [1, 0, 0, 0],
            rule_ids: ["auto-approved-destructive-bash"],
          },
        ],
      }),
    );

    const rows = [...node.querySelectorAll(".risk-projects tbody tr")];
    expect(rows.at(-1)?.querySelector("th")?.textContent).toBe("No project recorded");

    // And the column adds up to the number above it, which is the whole point.
    const column = rows
      .map((r) => Number(r.querySelector("td")?.textContent))
      .reduce((a, b) => a + b, 0);
    expect(column).toBe(2);
  });
});

describe("the rules panel (tasks 11.11–11.13)", () => {
  test("lists a rule that matched nothing, with what it looks for", async () => {
    const node = await mount(
      review({
        findings: [finding({ rule_id: "curl-piped-to-a-shell", title: "Curl into a shell", calls: 0 })],
        totals: [
          { severity: "high", calls: 0, rules: 0 },
          { severity: "medium", calls: 0, rules: 0 },
          { severity: "low", calls: 0, rules: 0 },
          { severity: "info", calls: 0, rules: 0 },
        ],
        projects: [],
      }),
    );

    expect(node.querySelector(".finding-head")?.textContent).toContain("Curl into a shell");
    node.querySelector<HTMLButtonElement>(".finding-head")?.click();
    await settle();

    // Its conditions are there whether or not it matched — that is the point.
    expect(node.querySelector(".finding-conditions")?.textContent).toContain(
      "the command's first line contains any of: rm -rf",
    );
    expect(node.textContent).toContain("This rule matched nothing in this store.");
    // And nothing was fetched for it.
    expect(state.asked).toEqual([]);
  });

  test("says a rule came from the user's file", async () => {
    const node = await mount(review({ findings: [finding({ from_user: true })] }));
    node.querySelector<HTMLButtonElement>(".finding-head")?.click();
    await settle();
    expect(node.querySelector(".kv")?.textContent).toContain("from your rules file");
  });

  test("opens the rules file rather than only naming it", async () => {
    const node = await mount(review());
    const buttons = [...node.querySelectorAll<HTMLButtonElement>(".rules-note button")];
    expect(buttons.map((b) => b.textContent)).toEqual(["Show the folder"]);
    buttons[0]?.click();
    expect(state.revealed).toBe(1);
  });
});

describe("nothing found", () => {
  /** Every rule ran and none matched — which is not the same as no rules. */
  const clean = () =>
    review({
      findings: [
        finding({ calls: 0, projects: [] }),
        finding({ rule_id: "curl-piped-to-a-shell", title: "Curl into a shell", calls: 0, projects: [] }),
      ],
      totals: [
        { severity: "high", calls: 0, rules: 0 },
        { severity: "medium", calls: 0, rules: 0 },
        { severity: "low", calls: 0, rules: 0 },
        { severity: "info", calls: 0, rules: 0 },
      ],
      projects: [],
    });

  test("is a result, not an empty screen", async () => {
    const node = await mount(clean());
    expect(node.querySelector(".empty")?.textContent).toContain("No rule matched");
    expect(node.querySelector(".empty")?.textContent).toContain("That is a real result");
  });

  test("still lists every rule, so 'clean' can be told from 'not looking'", async () => {
    const node = await mount(clean());
    expect(node.querySelectorAll(".finding")).toHaveLength(2);
  });

  test("and still says where a rules file would go", async () => {
    const node = await mount(clean());
    expect(node.querySelector(".footnote")?.textContent).toContain("rules.toml");
  });
});

describe("findings in time (tasks 12.4, 12.5, 12.12)", () => {
  test("a first review says so rather than reporting nothing new", async () => {
    // "Nobody has looked yet" and "nothing was found" are different, and
    // reporting the first as "0 new" is reassurance it has not earned.
    const node = await mount(review({ first_review: true }));
    expect(node.querySelector(".risk-new")?.textContent).toContain("First review");
  });

  test("counts what is new since the last review", async () => {
    const node = await mount(
      review({ findings: [finding({ calls: 9, new_calls: 4 })], first_review: false }),
    );
    expect(node.querySelector(".risk-new")?.textContent).toContain("4 calls are new");
  });

  test("says nothing is new when nothing is, without implying nothing was found", async () => {
    const node = await mount(review({ findings: [finding({ calls: 9, new_calls: 0 })] }));
    expect(node.querySelector(".risk-new")?.textContent).toBe(
      "Nothing new since the last review. ",
    );
  });

  test("shows when a finding was first seen, not when its calls ran", async () => {
    const node = await mount(review({ findings: [finding({ first_seen: null })] }));
    node.querySelector<HTMLButtonElement>(".finding-head")?.click();
    await settle();
    expect(node.querySelector(".kv")?.textContent).toContain("not yet recorded");
  });

  test("links the whole rule into the timeline", async () => {
    const node = await mount(review({ findings: [finding({ calls: 12 })] }));
    node.querySelector<HTMLButtonElement>(".finding-head")?.click();
    await settle();

    const open = node.querySelector<HTMLButtonElement>(".finding-open button");
    expect(open?.textContent).toBe("Show all 12 in the timeline");
    open?.click();
    expect(state.openedRule).toEqual(["auto-approved-destructive-bash"]);
  });

  test("a rule that matched nothing has nowhere to go", async () => {
    const node = await mount(review({ findings: [finding({ calls: 0 })] }));
    node.querySelector<HTMLButtonElement>(".finding-head")?.click();
    await settle();
    expect(node.querySelector(".finding-open")).toBeNull();
  });
});
