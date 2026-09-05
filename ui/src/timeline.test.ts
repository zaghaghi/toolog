//! The timeline, mounted against a fake store.
//!
//! These are the assertions the phase's exit criteria are made of: the row says
//! what happened, a half-populated row says which half is missing rather than
//! going blank, a search marks its hit, and grouping puts a subagent's calls
//! under the agent that made them.

import { beforeEach, describe, expect, test, vi } from "vitest";

import type { SessionGroup, TimelineFilter, TimelineRow, ToolCall } from "./bindings";

vi.mock("./bindings", () => ({
  queryTimeline: vi.fn((filter: TimelineFilter, page: { limit: number; offset: number }) => {
    if (store.failNextPage) {
      store.failNextPage = false;
      return Promise.reject(new Error("the database is locked"));
    }
    return Promise.resolve(store.page(filter, page));
  }),
  timelineCount: vi.fn((filter: TimelineFilter) => Promise.resolve(store.count(filter))),
  timelineGroups: vi.fn(() => Promise.resolve(store.groups)),
  facets: vi.fn(() =>
    Promise.resolve({
      projects: ["/work/app"],
      tools: ["Bash", "Edit"],
      decision_sources: ["config"],
      permission_modes: ["default"],
      agents: ["Explore"],
    }),
  ),
  collectorStatus: vi.fn(() => Promise.resolve(store.status)),
  getToolCall: vi.fn((id: string) =>
    Promise.resolve({
      call: store.rows.find((r) => r.call.tool_use_id === id)?.call ?? null,
      file_changes: [],
      session: {
        session_id: "s1",
        project_path: "/work/app",
        transcript_path: null,
        cwd: "/work/app",
        git_branch: "main",
        cc_version: "2.1.260",
        entrypoint: null,
        agent_name: null,
        slug: null,
        first_seen: null,
        last_seen: null,
      },
    }),
  ),
  getSource: vi.fn(() => Promise.resolve(null)),
  revealTranscript: vi.fn(() => Promise.resolve(null)),
  saveExport: vi.fn(() => Promise.resolve("/tmp/toolog.json")),
}));

// Imported after the mock so the module graph picks it up.
const { TimelineView } = await import("./timeline");
const { emptyView } = await import("./view");
type ViewState = ReturnType<typeof emptyView>;
type Timeline = InstanceType<typeof TimelineView>;

function call(over: Partial<ToolCall>): ToolCall {
  return {
    tool_use_id: "t0",
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
    called_at: Date.parse("2026-09-05T09:41:07Z"),
    completed_at: null,
    input_json: null,
    input_summary: "cargo test --workspace",
    target_path: null,
    result_json: null,
    result_text: null,
    result_size: null,
    success: true,
    duration_ms: null,
    error_type: null,
    decision: null,
    decision_source: null,
    permission_mode: null,
    provenance: 1,
    ...over,
  };
}

function row(over: Partial<ToolCall>, extra: Partial<TimelineRow> = {}): TimelineRow {
  return {
    call: call(over),
    project_path: "/work/app",
    git_branch: "main",
    snippet: null,
    lines_added: null,
    lines_removed: null,
    ...extra,
  };
}

const store = {
  rows: [] as TimelineRow[],
  groups: [] as SessionGroup[],
  status: { listening: true, paused: false } as { listening: boolean; paused: boolean },
  failNextPage: false,
  count(_filter: TimelineFilter): number {
    return this.rows.length;
  },
  page(filter: TimelineFilter, page: { limit: number; offset: number }): TimelineRow[] {
    let rows = this.rows;
    if (filter.agent_id !== null) rows = rows.filter((r) => r.call.agent_id === filter.agent_id);
    else if (filter.main_thread === true) rows = rows.filter((r) => r.call.agent_id === null);
    return rows.slice(page.offset, page.offset + page.limit);
  },
};

/** Let the pending promises and the next animation frame run. */
async function settle(times = 6): Promise<void> {
  for (let i = 0; i < times; i += 1) {
    await Promise.resolve();
    await new Promise((resolve) => setTimeout(resolve, 0));
  }
}

let lastView: ViewState | null = null;

function mount(view = emptyView()): Timeline {
  const timeline = new TimelineView(view, {
    onViewChange: (next) => {
      lastView = next;
    },
    onNotice: () => {},
    onOpenSetup: () => {},
  });
  document.body.replaceChildren(timeline.node);
  return timeline;
}

function text(node: Element | null, selector: string): string {
  return node?.querySelector(selector)?.textContent ?? "";
}

function rowsOf(timeline: Timeline): HTMLElement[] {
  return [...timeline.node.querySelectorAll<HTMLElement>(".row")];
}

beforeEach(() => {
  lastView = null;
  store.failNextPage = false;
  store.rows = [];
  store.groups = [];
  store.status = { listening: true, paused: false };
  document.body.replaceChildren();
});

describe("the row (tasks 5.4, 5.5)", () => {
  test("says what ran, where, and how it went", async () => {
    store.rows = [
      row({ tool_use_id: "t1", duration_ms: 1234, decision_source: "config", provenance: 3 }),
    ];
    const timeline = mount();
    await settle();

    const first = rowsOf(timeline)[0];
    expect(text(first!, ".project")).toBe("app");
    expect(text(first!, ".tool")).toBe("Bash");
    expect(text(first!, ".sum")).toContain("cargo test --workspace");
    expect(text(first!, ".dur")).toBe("1.23 s");
    expect(text(first!, ".dec")).toBe("config");
    expect(text(first!, ".st")).toBe("✓");
  });

  test("a call only the transcript saw states what is missing, not a zero", async () => {
    store.rows = [row({ tool_use_id: "t2", duration_ms: null, decision_source: null })];
    const timeline = mount();
    await settle();

    const first = rowsOf(timeline)[0]!;
    expect(text(first, ".dur")).toBe("—");
    expect(text(first, ".dec")).toBe("—");
    expect(first.querySelector(".dur")?.getAttribute("title")).toContain("OTLP lane");
    expect(first.querySelector(".dec")?.getAttribute("title")).toContain("only the OTLP lane");
  });

  test("a call with no result yet is shown, not hidden", async () => {
    store.rows = [row({ tool_use_id: "t3", success: null })];
    const timeline = mount();
    await settle();

    const first = rowsOf(timeline)[0]!;
    expect(rowsOf(timeline)).toHaveLength(1);
    expect(first.querySelector(".st")?.className).toContain("pending");
    expect(first.querySelector(".st")?.getAttribute("title")).toBe("No result recorded yet");
  });

  test("a refusal reads as a refusal, with who refused it", async () => {
    store.rows = [
      row({
        tool_use_id: "t4",
        input_summary: "rm -rf /",
        success: null,
        decision: "reject",
        decision_source: "user_reject",
        provenance: 3,
      }),
    ];
    const timeline = mount();
    await settle();

    const first = rowsOf(timeline)[0]!;
    expect(first.className).toContain("refused");
    expect(first.querySelector(".st")?.className).toContain("ref");
    expect(text(first, ".dec")).toBe("user_reject");
  });

  test("an Edit shows the size of what it changed", async () => {
    store.rows = [
      row({ tool_use_id: "t5", tool_name: "Edit", input_summary: "src/main.rs" }, {
        lines_added: 18,
        lines_removed: 4,
      }),
    ];
    const timeline = mount();
    await settle();
    expect(text(rowsOf(timeline)[0]!, ".diffsize")).toBe("+18 −4");
  });
});

describe("search (task 5.7)", () => {
  test("marks the term inside the command", async () => {
    store.rows = [row({ tool_use_id: "t6", input_summary: "rm -rf target/debug" })];
    const timeline = mount({ ...emptyView(), filter: { ...emptyView().filter, query: "rm -rf" } });
    await settle();

    const marks = [...rowsOf(timeline)[0]!.querySelectorAll("mark")].map((m) => m.textContent);
    expect(marks).toContain("rm");
    expect(marks).toContain("-rf");
  });

  test("says so when the match was somewhere the row does not show", async () => {
    store.rows = [
      row({ tool_use_id: "t7", input_summary: "cargo build" }, {
        snippet: "error: ENOENT while opening",
      }),
    ];
    const timeline = mount({ ...emptyView(), filter: { ...emptyView().filter, query: "ENOENT" } });
    await settle();

    const snippet = rowsOf(timeline)[0]!.querySelector(".insnip");
    expect(snippet?.textContent).toBe("error: ENOENT while opening");
    expect(snippet?.querySelector("mark")?.textContent).toBe("ENOENT");
  });
});

describe("grouping (task 5.10)", () => {
  test("puts a subagent's calls under its own header", async () => {
    store.rows = [
      row({ tool_use_id: "m1" }),
      row({ tool_use_id: "a1", agent_id: "ag-1", agent_name: "Explore" }),
    ];
    store.groups = [
      {
        session_id: "s1",
        project_path: "/work/app",
        git_branch: "main",
        slug: null,
        cc_version: null,
        calls: 2,
        main_thread_calls: 1,
        failures: 0,
        refusals: 1,
        first_at: 1,
        last_at: 2,
        cost_usd_micros: null,
        agents: [{ agent_id: "ag-1", agent_name: "Explore", calls: 1, first_at: 1, last_at: 2 }],
      },
    ];
    const timeline = mount({ ...emptyView(), grouped: true });
    await settle();

    const headers = [...timeline.node.querySelectorAll(".ghead")];
    expect(headers).toHaveLength(2);
    expect(text(headers[0]!, ".gname")).toBe("app");
    expect(text(headers[0]!, ".gmeta")).toContain("2 calls");
    expect(text(headers[0]!, ".gmeta")).toContain("1 refused");
    expect(text(headers[0]!, ".gmeta")).toContain("no cost recorded");
    expect(headers[1]!.className).toContain("agent");
    expect(text(headers[1]!, ".gname")).toBe("Explore");

    // The subagent's row is indented and asked for by agent, not by position.
    const sub = rowsOf(timeline).find((r) => r.className.includes("sub"));
    expect(sub).toBeDefined();
  });

  test("collapsing a session hides its rows and keeps its header", async () => {
    store.rows = [row({ tool_use_id: "m1" }), row({ tool_use_id: "m2" })];
    store.groups = [
      {
        session_id: "s1",
        project_path: "/work/app",
        git_branch: null,
        slug: null,
        cc_version: null,
        calls: 2,
        main_thread_calls: 2,
        failures: 0,
        refusals: 0,
        first_at: 1,
        last_at: 2,
        cost_usd_micros: 4200,
        agents: [],
      },
    ];
    const timeline = mount({ ...emptyView(), grouped: true });
    await settle();
    expect(rowsOf(timeline)).toHaveLength(2);

    timeline.node.querySelector<HTMLElement>(".ghead")!.click();
    await settle();
    expect(rowsOf(timeline)).toHaveLength(0);
    expect(timeline.node.querySelectorAll(".ghead")).toHaveLength(1);
  });
});

describe("states (task 5.13)", () => {
  test("an empty store offers the way out of being empty", async () => {
    const timeline = mount();
    await settle();
    const state = timeline.node.querySelector(".state");
    expect(state?.hasAttribute("hidden")).toBe(false);
    expect(state?.textContent).toContain("Nothing has been captured yet");
  });

  test("a filter that matches nothing offers to clear itself", async () => {
    const timeline = mount({
      ...emptyView(),
      filter: { ...emptyView().filter, tool_name: "Nope" },
    });
    await settle();
    const state = timeline.node.querySelector(".state");
    expect(state?.textContent).toContain("No calls match these filters");
    expect(state?.querySelector("button")?.textContent).toBe("Clear the filters");
  });

  test("a collector that is not listening says so above the list", async () => {
    store.rows = [row({ tool_use_id: "t1" })];
    store.status = { listening: false, paused: false };
    const timeline = mount();
    await settle();

    const banner = timeline.node.querySelector(".banner");
    expect(banner?.hasAttribute("hidden")).toBe(false);
    expect(banner?.textContent).toContain("not listening");
  });

  test("a page that fails to load holds its place and can be asked for again", async () => {
    store.rows = [row({ tool_use_id: "t1" })];
    store.failNextPage = true;
    const timeline = mount();
    await settle();

    const broken = timeline.node.querySelector<HTMLElement>(".row.broken");
    expect(broken?.textContent).toContain("the database is locked");
    // The list is still the right height: a failed page is not a shorter list.
    expect(rowsOf(timeline)).toHaveLength(1);

    broken!.click();
    await settle();
    expect(timeline.node.querySelector(".row.broken")).toBeNull();
    expect(text(rowsOf(timeline)[0]!, ".sum")).toContain("cargo test --workspace");
  });

  test("a paused collector is a different message from a stopped one", async () => {
    store.rows = [row({ tool_use_id: "t1" })];
    store.status = { listening: true, paused: true };
    const timeline = mount();
    await settle();
    expect(timeline.node.querySelector(".banner")?.textContent).toContain("paused");
  });
});

describe("narrowing from the detail pane (task 5.6)", () => {
  test("an open call offers the session and project it belongs to", async () => {
    store.rows = [row({ tool_use_id: "t1" })];
    const timeline = mount();
    await settle();
    rowsOf(timeline)[0]!.click();
    await settle();

    const links = [...timeline.node.querySelectorAll<HTMLElement>(".narrow button")];
    expect(links.map((l) => l.textContent)).toEqual([
      "Only this session",
      "Only this project",
    ]);

    links[1]!.click();
    await settle();
    expect(lastView?.filter.project_path).toBe("/work/app");
    // Narrowing drops the open call: it may not be in the new list.
    expect(lastView?.selected).toBeNull();
  });
});

describe("selection", () => {
  test("the pane is not there until a call is opened", async () => {
    store.rows = [row({ tool_use_id: "t1" }), row({ tool_use_id: "t2" })];
    const timeline = mount();
    await settle();

    // The pane is laid out by `has-detail`; with nothing selected it holds no
    // width and no leftover content.
    expect(timeline.node.className).not.toContain("has-detail");
    expect(timeline.node.querySelector(".pane")?.textContent).toBe("");

    rowsOf(timeline)[1]!.click();
    await settle();

    expect(rowsOf(timeline)[1]!.className).toContain("sel");
    expect(timeline.node.className).toContain("has-detail");
    expect(timeline.node.querySelector(".pane")?.textContent).toContain("Bash");
  });

  test("closing a call takes the pane away again", async () => {
    store.rows = [row({ tool_use_id: "t1" })];
    const timeline = mount();
    await settle();
    rowsOf(timeline)[0]!.click();
    await settle();
    expect(timeline.node.className).toContain("has-detail");

    timeline.node
      .querySelector<HTMLElement>(".viewport")!
      .dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    await settle();

    expect(timeline.node.className).not.toContain("has-detail");
    expect(timeline.node.querySelector(".pane")?.textContent).toBe("");
  });

  test("narrowing the filter closes the pane with the selection", async () => {
    store.rows = [row({ tool_use_id: "t1" })];
    const timeline = mount();
    await settle();
    rowsOf(timeline)[0]!.click();
    await settle();

    timeline.node.querySelectorAll<HTMLElement>(".narrow button")[1]!.click();
    await settle();
    expect(timeline.node.className).not.toContain("has-detail");
  });

  test("the arrow keys walk the list and the pane follows", async () => {
    store.rows = [row({ tool_use_id: "t1" }), row({ tool_use_id: "t2" })];
    const timeline = mount();
    await settle();

    const viewport = timeline.node.querySelector<HTMLElement>(".viewport")!;
    viewport.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }));
    await settle();
    expect(rowsOf(timeline)[0]!.className).toContain("sel");

    viewport.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }));
    await settle();
    expect(rowsOf(timeline)[1]!.className).toContain("sel");

    viewport.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    await settle();
    expect(rowsOf(timeline).some((r) => r.className.includes("sel"))).toBe(false);
  });
});
describe("the live stream", () => {
  test("the same call arriving twice is one new call, not two", async () => {
    store.rows = [row({ tool_use_id: "t1" })];
    const timeline = mount();
    await settle();
    const call = store.rows[0]?.call;
    if (call === undefined) throw new Error("no fixture call");

    // The transcript creates the row; OTEL completes it. One command.
    timeline.noteLiveCall(call);
    timeline.noteLiveCall({ ...call, duration_ms: 900, decision: "accept" });

    const pill = timeline.node.querySelector<HTMLElement>(".newpill");
    expect(pill?.hidden).toBe(false);
    expect(pill?.textContent).toBe("1 new call");
  });
});
