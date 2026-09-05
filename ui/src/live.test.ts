//! The live view (tasks 6.9–6.12).
//!
//! The three properties worth pinning: a call that arrives twice is one row
//! (the transcript creates it, OTEL completes it), the feed stops following
//! the stream the moment the reader scrolls, and both notification switches
//! start off.

import { beforeEach, describe, expect, test, vi } from "vitest";

import type { LiveSession, Prefs, ToolCall } from "./bindings";

const state = {
  sessions: [] as LiveSession[],
  prefs: {
    notify_refusals: false,
    notify_high_risk: false,
    redact_evidence: false,
    excluded_projects: [],
  } as Prefs,
  saved: [] as Prefs[],
};

vi.mock("./bindings", () => ({
  liveSessions: vi.fn(() => Promise.resolve(state.sessions)),
  getPrefs: vi.fn(() => Promise.resolve(state.prefs)),
  setPrefs: vi.fn((prefs: Prefs) => {
    state.saved.push(prefs);
    state.prefs = prefs;
    return Promise.resolve(prefs);
  }),
}));

const { LiveView } = await import("./live");

function call(over: Partial<ToolCall> = {}): ToolCall {
  return {
    tool_use_id: "toolu_1",
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
    input_summary: "cargo test",
    target_path: null,
    result_json: null,
    result_text: null,
    result_size: null,
    success: null,
    duration_ms: null,
    error_type: null,
    decision: null,
    decision_source: null,
    permission_mode: "default",
    provenance: 1,
    ...over,
  };
}

function session(over: Partial<LiveSession> = {}): LiveSession {
  return {
    session_id: "s1",
    project_path: "/work/alpha",
    git_branch: "main",
    current_tool: "Bash",
    current_success: null,
    last_call_at: Date.now(),
    first_call_at: Date.now() - 120_000,
    calls: 12,
    failures: 1,
    refused: 0,
    cost_usd_micros: 250_000,
    priced: true,
    permission_mode: "default",
    recent: [0, 1, 2, 3, 1, 0, 2, 4, 1, 0, 3, 2],
    ...over,
  };
}

async function mount(sessions: LiveSession[] = [session()]) {
  state.sessions = sessions;
  const view = new LiveView({ onNotice: () => {}, onOpenCall: () => {} });
  await view.refresh();
  return view;
}

beforeEach(() => {
  state.sessions = [];
  state.prefs = {
    notify_refusals: false,
    notify_high_risk: false,
    redact_evidence: false,
    excluded_projects: [],
  };
  state.saved = [];
});

describe("lanes", () => {
  test("one per session, with the tool in flight and a cost meter", async () => {
    const view = await mount();
    const lane = view.node.querySelector(".lane");
    expect(lane?.querySelector(".lane-project")?.textContent).toBe("alpha");
    expect(lane?.querySelector(".lane-tool")?.textContent).toBe("Bash");
    expect(lane?.querySelector(".meter-value")?.textContent).toBe("$0.25");
    expect(lane?.querySelector(".meter-fill")).not.toBeNull();
  });

  test("a session with no cost data says so rather than showing zero", async () => {
    const view = await mount([session({ priced: false, cost_usd_micros: 0 })]);
    const lane = view.node.querySelector(".lane");
    expect(lane?.querySelector(".meter-value")?.textContent).toBe("not captured");
    expect(lane?.querySelector(".meter-fill")).toBeNull();
  });

  test("a quiet session reads as idle, never as finished", async () => {
    const view = await mount([session({ last_call_at: Date.now() - 10 * 60_000 })]);
    const lane = view.node.querySelector(".lane");
    expect(lane?.className).toContain("idle");
    expect(lane?.querySelector(".pill")?.textContent).toContain("idle");
    expect(view.node.textContent).not.toContain("finished session");
  });

  test("two concurrent sessions are two lanes, each with its own attribution", async () => {
    const view = await mount([
      session(),
      session({ session_id: "s2", project_path: "/work/beta", current_tool: "Edit" }),
    ]);
    const lanes = [...view.node.querySelectorAll(".lane")];
    expect(lanes).toHaveLength(2);
    expect(lanes.map((l) => l.querySelector(".lane-project")?.textContent)).toEqual([
      "alpha",
      "beta",
    ]);
    expect(lanes.map((l) => l.querySelector(".lane-tool")?.textContent)).toEqual(["Bash", "Edit"]);
  });

  test("nothing running says how to make something appear", async () => {
    const view = await mount([]);
    expect(view.node.querySelector(".chart-empty")?.textContent).toContain("No session has run a tool");
  });
});

describe("the feed", () => {
  test("says what to expect before anything has arrived", async () => {
    const view = await mount();
    expect(view.node.querySelector<HTMLElement>(".feed")?.hidden).toBe(true);
    expect(view.node.querySelector<HTMLElement>(".feed-empty")?.hidden).toBe(false);

    view.noteCall(call());
    expect(view.node.querySelector<HTMLElement>(".feed")?.hidden).toBe(false);
    expect(view.node.querySelector<HTMLElement>(".feed-empty")?.hidden).toBe(true);
  });

  test("a call that arrives twice is one row, updated", async () => {
    const view = await mount();
    view.noteCall(call());
    expect(view.node.querySelectorAll(".feed-row")).toHaveLength(1);
    expect(view.node.querySelector(".fc-dur")).toBeNull();

    // The same call again, now with what OTEL added.
    view.noteCall(call({ duration_ms: 1200, success: true }));
    expect(view.node.querySelectorAll(".feed-row")).toHaveLength(1);
    expect(view.node.querySelector(".fc-dur")?.textContent).toBe("1.20 s");
  });

  test("newest last, and a refusal is marked", async () => {
    const view = await mount();
    view.noteCall(call({ tool_use_id: "a", input_summary: "first" }));
    view.noteCall(call({ tool_use_id: "b", input_summary: "second", decision: "reject" }));

    const rows = [...view.node.querySelectorAll(".feed-row")];
    expect(rows.map((r) => r.querySelector(".fc-what")?.textContent)).toEqual(["first", "second"]);
    expect(rows[1]?.querySelector(".fc-flag.refused")?.textContent).toBe("refused");
  });

  test("only the command's first line, so a heredoc body is not the command", async () => {
    const view = await mount();
    view.noteCall(call({ input_summary: "cat > notes.md <<'EOF'\nrm -rf everything\nEOF" }));
    expect(view.node.querySelector(".fc-what")?.textContent).toBe("cat > notes.md <<'EOF'");
  });

  test("scrolling up pauses the follow, and there is a way back", async () => {
    const view = await mount();
    const feed = view.node.querySelector<HTMLElement>(".feed");
    const resume = view.node.querySelector<HTMLElement>(".feed-resume");
    if (feed === null || resume === null) throw new Error("no feed");

    expect(resume.hidden).toBe(true);

    // happy-dom reports zero heights, so the scroll geometry is set directly:
    // what is under test is the decision, not the layout.
    Object.defineProperty(feed, "scrollHeight", { value: 1000, configurable: true });
    Object.defineProperty(feed, "clientHeight", { value: 200, configurable: true });
    feed.scrollTop = 0;
    feed.dispatchEvent(new Event("scroll"));
    expect(resume.hidden).toBe(false);

    // A call arriving while pinned must not yank the view.
    view.noteCall(call({ tool_use_id: "late" }));
    expect(feed.scrollTop).toBe(0);

    resume.dispatchEvent(new Event("click"));
    expect(resume.hidden).toBe(true);
    expect(feed.scrollTop).toBe(1000);
  });
});

describe("notifications", () => {
  test("both switches start off", async () => {
    const view = await mount();
    const boxes = [...view.node.querySelectorAll<HTMLInputElement>(".switch input")];
    expect(boxes).toHaveLength(2);
    expect(boxes.map((b) => b.checked)).toEqual([false, false]);
  });

  test("each one is toggled on its own and remembered", async () => {
    const view = await mount();
    const boxes = [...view.node.querySelectorAll<HTMLInputElement>(".switch input")];
    const refusals = boxes[0];
    if (refusals === undefined) throw new Error("no switch");

    refusals.checked = true;
    refusals.dispatchEvent(new Event("change"));
    await Promise.resolve();

    expect(state.saved).toEqual([
      {
        notify_refusals: true,
        notify_high_risk: false,
        redact_evidence: false,
        excluded_projects: [],
      },
    ]);
  });
});
