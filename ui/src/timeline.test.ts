//! The timeline, mounted against a fake store.
//!
//! These are the assertions the phase's exit criteria are made of: the row says
//! what happened, a half-populated row says which half is missing rather than
//! going blank, a search marks its hit, and grouping puts a subagent's calls
//! under the agent that made them.

import { beforeEach, describe, expect, test, vi } from "vitest";

import type {
  MatchedRule,
  SecondOpinion,
  TimelineFilter,
  TimelineRow,
  ToolCall,
} from "./bindings";

vi.mock("./bindings", () => ({
  queryTimeline: vi.fn((filter: TimelineFilter, page: { limit: number; offset: number }) => {
    if (store.failNextPage) {
      store.failNextPage = false;
      return Promise.reject(new Error("the database is locked"));
    }
    return Promise.resolve(store.page(filter, page));
  }),
  timelineCount: vi.fn((filter: TimelineFilter) => Promise.resolve(store.count(filter))),
  timelineHistogram: vi.fn((filter: TimelineFilter) => Promise.resolve(store.histogram(filter))),
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
      matched_rules: store.matched,
      second_opinion: store.opinion,
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
const { emptyView, fromHash, toHash } = await import("./view");
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
    model_score: null,
    model_intent: null,
    risk: null,
    rule_titles: [],
    ...extra,
  };
}

const store = {
  rows: [] as TimelineRow[],
  status: { listening: true, paused: false } as { listening: boolean; paused: boolean },
  failNextPage: false,
  opinion: null as SecondOpinion | null,
  matched: [] as MatchedRule[],
  count(_filter: TimelineFilter): number {
    return this.rows.length;
  },
  page(filter: TimelineFilter, page: { limit: number; offset: number }): TimelineRow[] {
    let rows = this.rows;
    if (filter.agent_id !== null) rows = rows.filter((r) => r.call.agent_id === filter.agent_id);
    else if (filter.main_thread === true) rows = rows.filter((r) => r.call.agent_id === null);
    return rows.slice(page.offset, page.offset + page.limit);
  },
  /** Three hour-wide columns, the middle one empty. */
  histogram(_filter: TimelineFilter) {
    if (this.rows.length === 0) {
      return { size: "hour" as const, buckets: [], since_ms: null, until_ms: null };
    }
    const buckets = [0, 1, 2].map((i) => ({
      start_ms: i * 3_600_000,
      calls: i === 1 ? 0 : 1,
      failures: 0,
      refusals: 0,
    }));
    return {
      size: "hour" as const,
      buckets,
      since_ms: 0,
      until_ms: 3 * 3_600_000,
    };
  },
};

/** Let the pending promises and the next animation frame run. */
async function settle(times = 6): Promise<void> {
  for (let i = 0; i < times; i += 1) {
    await Promise.resolve();
    await new Promise((resolve) => setTimeout(resolve, 0));
  }
}

/**
 * Type into the query bar and wait for it to reach the store.
 *
 * The box is debounced by 140 ms, because every keystroke would otherwise be an
 * FTS query over the whole corpus — so a test that only flushes microtasks is
 * testing the debounce rather than the parse.
 */
async function typeQuery(timeline: Timeline, text: string): Promise<void> {
  const box = timeline.node.querySelector<HTMLInputElement>(".search")!;
  box.value = text;
  box.dispatchEvent(new Event("input"));
  await new Promise((resolve) => setTimeout(resolve, 200));
  await settle();
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
  store.status = { listening: true, paused: false };
  store.opinion = null;
  store.matched = [];
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
    // width and no leftover content. Its header — the close button — is always
    // built, so "empty" is about the body.
    expect(timeline.node.className).not.toContain("has-detail");
    expect(timeline.node.querySelector(".pane-body")?.textContent).toBe("");

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
    expect(timeline.node.querySelector(".pane-body")?.textContent).toBe("");
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

describe("the activity histogram (tasks 10.3, 10.4)", () => {
  test("sits above the list and draws a column per bucket, empty ones included", async () => {
    store.rows = [row({ tool_use_id: "t1" })];
    const timeline = mount();
    await settle();

    const marks = timeline.node.querySelectorAll(".histo .chart-col-mark");
    expect(marks).toHaveLength(3);
    // A bucket with nothing in it is a hairline on the baseline, not a gap.
    expect([...marks].map((m) => (m as HTMLElement).style.height)).toEqual([
      "100.00%",
      "0.00%",
      "100.00%",
    ]);
  });

  test("clicking a column writes that column's absolute range into the filter", async () => {
    store.rows = [row({ tool_use_id: "t1" })];
    const timeline = mount();
    await settle();

    const plot = timeline.node.querySelector<HTMLElement>(".histo .chart-plot")!;
    // A click is a drag that went nowhere: down and up on the same column.
    plot.dispatchEvent(new PointerEvent("pointerdown", { button: 0, bubbles: true }));
    plot.dispatchEvent(new PointerEvent("pointerup", { button: 0, bubbles: true }));
    await settle();

    expect(lastView?.filter.since).toBe(0);
    // Inclusive of the last instant inside the column, because `until` binds
    // as `called_at <= ?`.
    expect(lastView?.filter.until).toBe(3_600_000 - 1);
  });

  test("dragging across columns brushes a range, and the hash reproduces it", async () => {
    store.rows = [row({ tool_use_id: "t1" })];
    const timeline = mount();
    await settle();

    const plot = timeline.node.querySelector<HTMLElement>(".histo .chart-plot")!;
    // happy-dom lays nothing out, so the chart is given a width to measure
    // against: the column under a pointer is `clientX` over that width.
    plot.getBoundingClientRect = () =>
      ({ left: 0, width: 300, top: 0, height: 78 }) as DOMRect;

    plot.dispatchEvent(new PointerEvent("pointerdown", { button: 0, clientX: 10, bubbles: true }));
    plot.dispatchEvent(new PointerEvent("pointerup", { button: 0, clientX: 290, bubbles: true }));
    await settle();

    // The first column's start to the last instant inside the third.
    expect(lastView?.filter.since).toBe(0);
    expect(lastView?.filter.until).toBe(3 * 3_600_000 - 1);

    // And the view that produced it survives the address bar.
    expect(fromHash(toHash(lastView!))).toEqual(lastView);
  });

  test("a range chosen on the chart shows as Custom range in the time control", async () => {
    store.rows = [row({ tool_use_id: "t1" })];
    const timeline = mount({
      ...emptyView(),
      filter: { ...emptyView().filter, since: 0, until: 3_599_999 },
    });
    await settle();

    const time = timeline.node.querySelector<HTMLSelectElement>(".timepick .pick")!;
    expect(time.value).toBe("custom");
    expect([...time.options].map((o) => o.textContent)).toContain("Custom range");
  });

  test("an empty store has no chart rather than an empty frame", async () => {
    store.rows = [];
    const timeline = mount();
    await settle();
    expect(timeline.node.querySelector<HTMLElement>(".histo")?.hidden).toBe(true);
  });
});

describe("the query bar (tasks 10.5–10.11)", () => {
  test("has one dropdown left, beside the box rather than under it", async () => {
    store.rows = [row({ tool_use_id: "t1" })];
    const timeline = mount();
    await settle();

    // Seven `<select>`s used to sit in a row under the box; Time is the only
    // one left, and it sits on the same line as the box and Export.
    const selects = timeline.node.querySelectorAll(".bar select");
    expect(selects).toHaveLength(1);
    expect(timeline.node.querySelector(".searchrow .timepick select")).toBe(selects[0]);
    expect(timeline.node.querySelector(".controls")).toBeNull();
  });

  test("shows the current filter as text, and typing narrows the list", async () => {
    store.rows = [row({ tool_use_id: "t1" })];
    const timeline = mount({
      ...emptyView(),
      filter: { ...emptyView().filter, tool_name: "Bash" },
    });
    await settle();

    const box = timeline.node.querySelector<HTMLInputElement>(".search")!;
    expect(box.value).toBe("@tool:Bash");

    await typeQuery(timeline, "@tool:Edit rm -rf");

    expect(lastView?.filter.tool_name).toBe("Edit");
    expect(lastView?.filter.query).toBe("rm -rf");
  });

  test("an unknown key is said under the box, and the rest still applies", async () => {
    store.rows = [row({ tool_use_id: "t1" })];
    const timeline = mount();
    await settle();

    await typeQuery(timeline, "@nonsense:x @tool:Bash");

    const errors = timeline.node.querySelector<HTMLElement>(".qerrors")!;
    expect(errors.hidden).toBe(false);
    expect(errors.textContent).toContain("@nonsense");
    expect(lastView?.filter.tool_name).toBe("Bash");
  });

  test("offers keys after @ and store values after the colon", async () => {
    store.rows = [row({ tool_use_id: "t1" })];
    const timeline = mount();
    await settle();

    const box = timeline.node.querySelector<HTMLInputElement>(".search")!;
    const menu = timeline.node.querySelector<HTMLElement>(".qmenu")!;

    box.value = "@to";
    box.setSelectionRange(3, 3);
    box.dispatchEvent(new Event("input"));
    expect([...menu.querySelectorAll(".qlabel")].map((n) => n.textContent)).toEqual(["@tool"]);

    box.value = "@tool:";
    box.setSelectionRange(6, 6);
    box.dispatchEvent(new Event("input"));
    // From `facets()`, not from a hard-coded list.
    expect([...menu.querySelectorAll(".qlabel")].map((n) => n.textContent)).toEqual([
      "Bash",
      "Edit",
    ]);
  });

  test("the time control is not reachable by typing, because it pairs with the chart", async () => {
    store.rows = [row({ tool_use_id: "t1" })];
    const timeline = mount({
      ...emptyView(),
      filter: { ...emptyView().filter, since: 1_000, until: 2_000 },
    });
    await settle();

    const box = timeline.node.querySelector<HTMLInputElement>(".search")!;
    expect(box.value).toBe("");

    // And editing the text leaves the bounds where the chart put them.
    await typeQuery(timeline, "@tool:Bash");
    expect(lastView?.filter.since).toBe(1_000);
    expect(lastView?.filter.until).toBe(2_000);
  });
});

describe("closing the detail pane (task 10.12)", () => {
  async function opened(): Promise<Timeline> {
    store.rows = [row({ tool_use_id: "t1" })];
    const timeline = mount();
    await settle();
    rowsOf(timeline)[0]!.click();
    await settle();
    expect(timeline.node.className).toContain("has-detail");
    return timeline;
  }

  test("closes from the button in its own header", async () => {
    const timeline = await opened();
    timeline.node.querySelector<HTMLButtonElement>(".pane-close")!.click();
    await settle();

    expect(timeline.node.className).not.toContain("has-detail");
    expect(lastView?.selected).toBeNull();
  });

  test("closes on Escape from inside the pane, not only from the list", async () => {
    const timeline = await opened();
    timeline.node
      .querySelector<HTMLElement>(".pane-body")!
      .dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    await settle();

    expect(timeline.node.className).not.toContain("has-detail");
    expect(lastView?.selected).toBeNull();
  });

  test("closes when the row that opened it is clicked again", async () => {
    const timeline = await opened();
    rowsOf(timeline)[0]!.click();
    await settle();

    expect(timeline.node.className).not.toContain("has-detail");
    expect(lastView?.selected).toBeNull();
  });
});

describe("the second opinion, on one call (Phase 13)", () => {
  function said(over: Partial<SecondOpinion> = {}): SecondOpinion {
    return {
      pair: "a1b2c3d4e5f6 / 0f1e2d3c4b5a",
      verdict: {
        intent_summary: "Deletes the build directory.",
        category: "filesystem",
        risk_score: 4,
        is_destructive: true,
        violates_sandbox: false,
      },
      error: null,
      at: Date.parse("2026-09-06T11:00:00Z"),
      ms: 1_250,
      ...over,
    };
  }

  async function pane(opinion: SecondOpinion | null): Promise<HTMLElement> {
    store.opinion = opinion;
    store.rows = [row({ tool_use_id: "t1" })];
    const timeline = mount();
    await settle();
    rowsOf(timeline)[0]!.click();
    await settle();
    return timeline.node.querySelector<HTMLElement>(".pane-body")!;
  }

  test("shows what the model said, and whose opinion it is", async () => {
    const body = await pane(said());

    expect(body.textContent).toContain("A second opinion");
    expect(body.textContent).toContain("Deletes the build directory.");
    expect(body.querySelector(".dllm .llm-score")?.textContent).toBe("4");
    // A score whose author cannot be named is not evidence (ADR-0013).
    expect(body.textContent).toContain("a1b2c3d4e5f6 / 0f1e2d3c4b5a");
    expect(body.textContent).toContain("Advisory");
  });

  test("never borrows one of the rules' four severity words", async () => {
    const said5 = said({ verdict: { ...said().verdict!, risk_score: 5 } });
    const text = (await pane(said5)).querySelector(".dllm")!.textContent ?? "";

    for (const word of ["high", "medium", "low", "info", "severity"]) {
      expect(text.toLowerCase()).not.toContain(word);
    }
  });

  test("an answer the schema rejected is not the same as no answer", async () => {
    const body = await pane(
      said({ verdict: null, error: "risk_score is 9, and the scale is 1 to 5" }),
    );

    expect(body.textContent).toContain("the schema rejected it");
    expect(body.textContent).toContain("risk_score is 9");
  });

  test("an unexamined call says so, and does not say it is fine", async () => {
    const body = await pane(said({ verdict: null, error: null, at: null, ms: null }));

    expect(body.textContent).toContain("Not examined yet");
    // The reasoning is still reachable, it is just not four lines of the pane.
    expect(body.querySelector(".dllm-none")?.getAttribute("title")).toContain("not fine");
  });

  test("is absent entirely when no model has ever answered here", async () => {
    const body = await pane(null);

    expect(body.textContent).toContain("Bash");
    expect(body.querySelector(".dllm")).toBeNull();
  });
});

describe("the two judgement columns", () => {
  async function cell(over: Partial<TimelineRow>, sel: string): Promise<HTMLElement | null> {
    store.rows = [row({ tool_use_id: "t1" }, over)];
    const timeline = mount();
    await settle();
    return rowsOf(timeline)[0]!.querySelector<HTMLElement>(sel);
  }

  test("the rules' severity is a column, in the risk page's words", async () => {
    const rsk = await cell(
      { risk: "high", rule_titles: ["Destructive shell commands approved by a rule, not a person"] },
      ".rsk",
    );

    expect(rsk?.textContent).toBe("HIGH");
    expect(rsk?.className).toContain("sev-high");
    expect(rsk?.title).toContain("Destructive shell commands");
  });

  test("the model's score is a separate column, as a digit", async () => {
    const mdl = await cell(
      { model_score: 5, model_intent: "Deletes the whole repository." },
      ".mdl",
    );

    expect(mdl?.textContent).toBe("5");
    expect(mdl?.title).toBe("Deletes the whole repository.");
    // A digit in its own hue, never one of the rules' four words.
    expect(mdl?.className).toContain("llm-score-5");
    expect(mdl?.className).not.toContain("sev-");
  });

  test("the two are never the same column", async () => {
    store.rows = [
      row({ tool_use_id: "t1" }, { risk: "high", rule_titles: ["A rule"], model_score: 5 }),
    ];
    const timeline = mount();
    await settle();

    const node = rowsOf(timeline)[0]!;
    expect(node.querySelector(".rsk")).not.toBe(node.querySelector(".mdl"));
    expect(node.children.length).toBe(9);
    // To the right of the command, with the other judgement columns.
    expect([...node.children].indexOf(node.querySelector(".rsk")!)).toBeGreaterThan(
      [...node.children].indexOf(node.querySelector(".sum")!),
    );
    // And a pending row holds the same columns open, or the list jumps as
    // pages arrive.
    expect(timeline.node.querySelectorAll(".row.pending .rsk").length).toBeLessThanOrEqual(1);
  });

  test("every column is named, and the names line up with the cells", async () => {
    store.rows = [row({ tool_use_id: "t1" }, { risk: "high", rule_titles: ["A rule"] })];
    const timeline = mount();
    await settle();

    const header = timeline.node.querySelector<HTMLElement>(".context")!;
    // One cell per column, in the row's own order — a header with a different
    // number of cells is a header pointing at the wrong columns.
    expect(header.children.length).toBe(rowsOf(timeline)[0]!.children.length);
    expect([...header.children].map((c) => c.textContent)).toEqual([
      // The day of the topmost row, in the slot the time column occupies.
      expect.any(String),
      "Project",
      "Tool",
      // 18px of glyph: no word fits, so the name is a tooltip.
      "",
      "Input",
      "Risk",
      "Model",
      "Took",
      "Decided by",
    ]);
    expect(header.children[3]!.getAttribute("title")).toContain("Outcome");
  });

  test("the first cell carries the day of the topmost row", async () => {
    store.rows = [row({ tool_use_id: "t1" })];
    const timeline = mount();
    await settle();

    // The first paint happens before any row exists, so this is only right if
    // the arriving page refreshes it.
    const date = timeline.node.querySelector<HTMLElement>(".chd-date")!;
    expect(date.textContent).not.toBe("");
  });

  test("a call no rule matched leaves the column empty rather than saying low", async () => {
    const rsk = await cell({ risk: null }, ".rsk");

    expect(rsk?.textContent).toBe("");
    expect(rsk?.title).toContain("No live rule");
  });

  test("the everyday scores are not drawn at all", async () => {
    expect((await cell({ model_score: 2 }, ".mdl"))?.textContent).toBe("");
    expect((await cell({ model_score: null }, ".mdl"))?.textContent).toBe("");
    expect((await cell({ model_score: 3 }, ".mdl"))?.textContent).toBe("3");
  });
});

describe("the rules that matched one call, in the pane", () => {
  async function pane(matched: MatchedRule[]): Promise<HTMLElement> {
    store.matched = matched;
    store.rows = [row({ tool_use_id: "t1" })];
    const timeline = mount();
    await settle();
    rowsOf(timeline)[0]!.click();
    await settle();
    return timeline.node.querySelector<HTMLElement>(".pane-body")!;
  }

  test("names every rule that flagged the call", async () => {
    const body = await pane([
      { id: "destructive", title: "Destructive shell commands", severity: "high" },
      { id: "outside", title: "Files written outside the session", severity: "medium" },
    ]);

    expect(body.textContent).toContain("Destructive shell commands");
    expect(body.textContent).toContain("Files written outside the session");
    expect(body.querySelectorAll(".drisk-rule").length).toBe(2);
  });

  test("a rule is a way into every other call it caught", async () => {
    const body = await pane([
      { id: "destructive", title: "Destructive shell commands", severity: "high" },
    ]);
    body.querySelector<HTMLButtonElement>(".drisk-rule")!.click();
    await settle();

    expect(lastView?.filter.rule_id).toBe("destructive");
  });

  test("a call no rule matched gets no risk section at all", async () => {
    const body = await pane([]);

    expect(body.querySelector(".drisk")).toBeNull();
  });
});
