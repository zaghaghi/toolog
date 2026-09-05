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
  asked: [] as [string, number][],
};

vi.mock("./bindings", () => ({
  risk: vi.fn(() => Promise.resolve(state.review)),
  ruleCalls: vi.fn((ruleId: string, page: { limit: number; offset: number }) => {
    state.asked.push([ruleId, page.offset]);
    return Promise.resolve(state.extraCalls);
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
    examples: [call()],
    dismissed: null,
    ...over,
  };
}

function review(over: Partial<RiskReview> = {}): RiskReview {
  return {
    findings: [finding()],
    projects: [
      {
        project_path: "/work/scratch",
        calls: 1,
        by_severity: [1, 0, 0, 0],
        rule_ids: ["auto-approved-destructive-bash"],
      },
    ],
    rules_path: "/Users/someone/Library/Application Support/toolog/rules.toml",
    rules_customized: false,
    ...over,
  };
}

async function mount(data: RiskReview, onOpenCall: (id: string) => void = () => {}) {
  state.review = data;
  const view = new RiskView({ onNotice: () => {}, onOpenCall });
  await view.refresh();
  return view.node;
}

beforeEach(() => {
  state.review = null;
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

    // Open the finding, and the call behind it is right there.
    node.querySelector<HTMLButtonElement>(".finding-head")?.click();
    const call = node.querySelector<HTMLButtonElement>(".finding-call");
    expect(call?.textContent).toContain("rm -rf /tmp/scratch");

    call?.click();
    expect(opened).toEqual(["toolu_rm"]);
  });
});

describe("severity", () => {
  test("findings are counted by severity, worst first", async () => {
    const node = await mount(
      review({
        findings: [
          finding(),
          finding({ rule_id: "mcp-tool-usage", severity: "info", title: "MCP tools", calls: 62 }),
        ],
      }),
    );
    const counts = [...node.querySelectorAll(".risk-count")].map((n) => [
      n.querySelector(".risk-count-label")?.textContent,
      n.querySelector(".risk-count-n")?.textContent,
    ]);
    expect(counts).toEqual([
      ["high", "1"],
      ["medium", "0"],
      ["low", "0"],
      ["info", "1"],
    ]);
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
    const view = new View({ onNotice: (m) => notices.push(m), onOpenCall: () => {} });
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
  test("offers the rest of the calls past the examples", async () => {
    const node = await mount(review({ findings: [finding({ calls: 12 })] }));
    node.querySelector<HTMLButtonElement>(".finding-head")?.click();

    const more = node.querySelector<HTMLButtonElement>(".finding-more");
    expect(more?.textContent).toBe("Show the other 11");

    state.extraCalls = [call({ tool_use_id: "toolu_2", input_summary: "rm -rf build" })];
    more?.click();
    await Promise.resolve();
    await Promise.resolve();

    // Asked for the rule's own matches, from after the examples.
    expect(state.asked).toEqual([["auto-approved-destructive-bash", 1]]);
  });

  test("no button when the examples are all of them", async () => {
    const node = await mount(review());
    node.querySelector<HTMLButtonElement>(".finding-head")?.click();
    expect(node.querySelector(".finding-more")).toBeNull();
  });
});

describe("per-project posture", () => {
  test("names the project and its worst findings", async () => {
    const node = await mount(review());
    const row = node.querySelector(".risk-projects tbody tr");
    expect(row?.querySelector("th")?.textContent).toBe("scratch");
    const cells = [...(row?.querySelectorAll("td") ?? [])].map((c) => c.textContent);
    expect(cells).toEqual(["1", "—", "—", "—", "1"]);
  });
});

describe("nothing found", () => {
  test("is a result, not an empty screen", async () => {
    const node = await mount(review({ findings: [], projects: [] }));
    expect(node.querySelector(".empty")?.textContent).toContain("No rule matched");
    expect(node.querySelector(".empty")?.textContent).toContain("That is a real result");
  });

  test("and still says where a rules file would go", async () => {
    const node = await mount(review({ findings: [], projects: [] }));
    expect(node.querySelector(".footnote")?.textContent).toContain("rules.toml");
  });
});
