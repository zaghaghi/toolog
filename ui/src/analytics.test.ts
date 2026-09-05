//! The usage view, and the one thing it must never do (tasks 6.5–6.8).
//!
//! Most of these are about the honest empty state. A store where nothing was
//! captured live has to say so; rendering `$0.00` would claim a measurement
//! that was never taken, and there is no way for a reader to tell the two
//! apart afterwards.

import { beforeEach, describe, expect, test, vi } from "vitest";

import type { Analytics, Bucket, Comparison, Period, Usage } from "./bindings";

const state = {
  usage: null as Usage | null,
};

vi.mock("./bindings", () => ({
  usage: vi.fn(() => Promise.resolve(state.usage)),
  facets: vi.fn(() =>
    Promise.resolve({
      projects: ["/work/alpha", "/work/beta"],
      tools: ["Bash"],
      decision_sources: [],
      permission_modes: [],
      agents: [],
    }),
  ),
}));

const { AnalyticsView, fillDays, windowFor } = await import("./analytics");

function bucket(key: string, over: Partial<Bucket> = {}): Bucket {
  return {
    key,
    label: null,
    calls: 0,
    failures: 0,
    cost_usd_micros: 0,
    input_tokens: 0,
    output_tokens: 0,
    cache_read_tokens: 0,
    requests: 0,
    first_at: null,
    last_at: null,
    ...over,
  };
}

function analytics(over: Partial<Analytics> = {}): Analytics {
  return {
    window: { since: null, until: null, project_path: null, utc_offset_minutes: 0 },
    calls: {
      calls: 100,
      failures: 4,
      with_outcome: 90,
      refused: 2,
      sidechain: 10,
      sessions: 6,
      projects: 2,
      p50_ms: 120,
      p95_ms: 4000,
      active_ms: 3_600_000,
      first_at: 0,
      last_at: 0,
      error_rate: 4 / 90,
      sidechain_share: 0.1,
    },
    cost: {
      requests: 12,
      cost_usd_micros: 1_500_000,
      input_tokens: 1000,
      output_tokens: 500,
      cache_read_tokens: 9000,
      cache_creation_tokens: 0,
      cache_hit_ratio: 0.9,
      total_tokens: 10_500,
    },
    coverage: {
      sessions: 6,
      sessions_with_cost: 2,
      calls: 100,
      calls_with_cost: 40,
      measured: true,
      complete: false,
    },
    by_day: [bucket("2026-03-02", { calls: 60, cost_usd_micros: 1_000_000, requests: 8 })],
    by_project: [bucket("/work/alpha", { calls: 60, cost_usd_micros: 1_500_000, requests: 12 })],
    by_model: [bucket("claude-opus-5", { requests: 12, cost_usd_micros: 1_500_000 })],
    by_session: [bucket("s1", { calls: 60 })],
    tools: [{ tool_name: "Bash", calls: 60, failures: 3, p50_ms: 100, p95_ms: 900 }],
    ...over,
  };
}

function comparison(over: Partial<Comparison> = {}): Comparison {
  return {
    current: {
      calls: 100,
      failures: 4,
      sessions: 6,
      active_ms: 3_600_000,
      cost_usd_micros: 1_500_000,
      tokens: 10_500,
      sessions_with_cost: 2,
    },
    current_window: { since: 1000, until: 2000, project_path: null, utc_offset_minutes: 0 },
    previous: {
      calls: 50,
      failures: 5,
      sessions: 4,
      active_ms: 1_800_000,
      cost_usd_micros: 500_000,
      tokens: 5000,
      sessions_with_cost: 1,
    },
    previous_window: { since: 0, until: 1000, project_path: null, utc_offset_minutes: 0 },
    ...over,
  };
}

async function mount(data: Usage): Promise<HTMLElement> {
  state.usage = data;
  const view = new AnalyticsView({ onNotice: () => {} });
  await view.refresh();
  return view.node;
}

beforeEach(() => {
  state.usage = null;
});

describe("windowFor", () => {
  test("resolves a preset to absolute milliseconds", () => {
    const w = windowFor("7d", null);
    expect(w.since).not.toBeNull();
    expect(w.until).not.toBeNull();
    expect((w.until ?? 0) - (w.since ?? 0)).toBe(7 * 86_400_000);
  });

  test("all of it is open-ended, so nothing is invented to compare with", () => {
    const w = windowFor("all", null);
    expect(w.since).toBeNull();
    expect(w.until).toBeNull();
  });

  test("carries the reader's timezone, not Greenwich's", () => {
    expect(windowFor("30d", null).utc_offset_minutes).toBe(-new Date().getTimezoneOffset());
  });

  test("a project scopes the window", () => {
    expect(windowFor("30d", "/work/beta").project_path).toBe("/work/beta");
  });
});

describe("fillDays", () => {
  const window: Period = {
    since: Date.parse("2026-03-02T00:00:00Z"),
    until: Date.parse("2026-03-05T00:00:00Z"),
    project_path: null,
    utc_offset_minutes: 0,
  };

  test("a quiet day is a zero column, not a missing one", () => {
    const filled = fillDays(
      [bucket("2026-03-02", { calls: 3 }), bucket("2026-03-05", { calls: 1 })],
      window,
    );
    expect(filled.map((b) => b.key)).toEqual([
      "2026-03-02",
      "2026-03-03",
      "2026-03-04",
      "2026-03-05",
    ]);
    expect(filled[1]?.calls).toBe(0);
    expect(filled[0]?.calls).toBe(3);
  });

  test("an open-ended window fills from the first day it has", () => {
    const filled = fillDays([bucket("2026-03-02", { calls: 1 })], {
      since: null,
      until: Date.parse("2026-03-04T00:00:00Z"),
      project_path: null,
      utc_offset_minutes: 0,
    });
    expect(filled.map((b) => b.key)).toEqual(["2026-03-02", "2026-03-03", "2026-03-04"]);
  });

  test("nothing at all stays nothing", () => {
    expect(fillDays([], { ...window, since: null, until: null })).toEqual([]);
  });
});

describe("the view", () => {
  test("leads with the headline figures and their comparison", async () => {
    const node = await mount({ analytics: analytics(), comparison: comparison() });
    const labels = [...node.querySelectorAll(".tile-label")].map((n) => n.textContent);
    expect(labels).toEqual(["Tool calls", "Sessions", "Active time", "Spend", "Error rate", "Refused"]);
    // 100 against 50 the period before.
    expect(node.querySelector(".tile-delta")?.textContent).toContain("100%");
  });

  test("a falling error rate is the one delta shown as good", async () => {
    const node = await mount({ analytics: analytics(), comparison: comparison() });
    const tiles = [...node.querySelectorAll(".tile")];
    const rate = tiles.find((t) => t.querySelector(".tile-label")?.textContent === "Error rate");
    // 4/100 now against 5/50 before: down, and down is the good direction.
    expect(rate?.querySelector(".tile-delta")?.className).toContain("good");
    expect(rate?.querySelector(".tile-delta")?.textContent).toContain("↓");

    const calls = tiles.find((t) => t.querySelector(".tile-label")?.textContent === "Tool calls");
    expect(calls?.querySelector(".tile-delta")?.className).toContain("neutral");
  });

  test("partial cost coverage is stated, not implied", async () => {
    const node = await mount({ analytics: analytics(), comparison: comparison() });
    const banner = node.querySelector(".banner");
    expect(banner?.textContent).toContain("Cost is partly measured");
    expect(banner?.textContent).toContain("2 of 6 sessions");
    expect(banner?.textContent).toContain("40 of 100 calls");
  });

  test("no cost captured reads as not captured, never as zero", async () => {
    const bare = analytics({
      cost: {
        requests: 0,
        cost_usd_micros: 0,
        input_tokens: 0,
        output_tokens: 0,
        cache_read_tokens: 0,
        cache_creation_tokens: 0,
        cache_hit_ratio: null,
        total_tokens: 0,
      },
      coverage: {
        sessions: 6,
        sessions_with_cost: 0,
        calls: 100,
        calls_with_cost: 0,
        measured: false,
        complete: false,
      },
      by_day: [bucket("2026-03-02", { calls: 60 })],
      by_model: [],
    });
    const node = await mount({ analytics: bare, comparison: comparison() });

    const tiles = [...node.querySelectorAll(".tile")];
    const spend = tiles.find((t) => t.querySelector(".tile-label")?.textContent === "Spend");
    expect(spend?.querySelector(".tile-value")?.textContent).toBe("not captured");
    expect(spend?.textContent).not.toContain("$0.00");

    expect(node.querySelector(".banner")?.textContent).toContain("No cost was captured here");

    // And the spend chart says why it is empty rather than plotting a flat line.
    const spendChart = [...node.querySelectorAll(".chart")].find(
      (c) => c.querySelector(".chart-title")?.textContent === "Spend per day",
    );
    expect(spendChart?.querySelector(".chart-empty")?.textContent).toContain("No priced day");
  });

  test("the project leaderboard is renamed when there is no spend to rank by", async () => {
    const priced = await mount({ analytics: analytics(), comparison: comparison() });
    expect([...priced.querySelectorAll(".chart-title")].map((n) => n.textContent)).toContain(
      "Projects by spend",
    );

    const unpriced = analytics({
      coverage: {
        sessions: 6,
        sessions_with_cost: 0,
        calls: 100,
        calls_with_cost: 0,
        measured: false,
        complete: false,
      },
    });
    const node = await mount({ analytics: unpriced, comparison: comparison() });
    expect([...node.querySelectorAll(".chart-title")].map((n) => n.textContent)).toContain(
      "Projects by use",
    );
  });

  test("an empty period says so rather than drawing empty charts", async () => {
    const nothing = analytics({
      calls: {
        calls: 0,
        failures: 0,
        with_outcome: 0,
        refused: 0,
        sidechain: 0,
        sessions: 0,
        projects: 0,
        p50_ms: null,
        p95_ms: null,
        active_ms: 0,
        first_at: null,
        last_at: null,
        error_rate: null,
        sidechain_share: null,
      },
    });
    const node = await mount({ analytics: nothing, comparison: comparison() });
    expect(node.querySelector(".empty")?.textContent).toContain("Nothing in this period");
    expect(node.querySelector(".chart")).toBeNull();
  });

  test("the filter row sits above everything it scopes", async () => {
    const node = await mount({ analytics: analytics(), comparison: comparison() });
    const bar = node.querySelector(".bar");
    expect(bar).not.toBeNull();
    expect(bar?.querySelectorAll("select")).toHaveLength(2);
    // Nothing inside a chart card filters anything.
    expect(node.querySelector(".chart select")).toBeNull();
  });

  test("an open-ended period offers no comparison at all", async () => {
    const node = await mount({
      analytics: analytics(),
      comparison: comparison({ previous: null, previous_window: null }),
    });
    expect(node.querySelector(".tile-delta")).toBeNull();
  });
});
