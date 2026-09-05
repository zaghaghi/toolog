//! The usage view (tasks 6.5–6.8).
//!
//! The thing this view has to get right is not the charts. It is that **half of
//! it is missing on most stores and it has to say so.** Cost and tokens come
//! from the OTLP lane only, so a session imported from a transcript has no
//! spend and never will. Rendering that as `$0.00` would be a lie told in a
//! very confident font, so every cost surface here has three states — measured,
//! partly measured, not measured — and the third one says what it is (task
//! 6.8).
//!
//! Everything else follows from that: coverage travels with the numbers, the
//! period filter sits in one row above the whole page so no two figures can
//! describe different slices, and each comparison names the period it is
//! against rather than saying "vs previous".

import type { Bucket, Period, Usage } from "./bindings";
import { facets, usage } from "./bindings";
import { barChart, columnChart, dataTable, meter, statTile } from "./chart";
import type { Datum } from "./chart";
import { append, el, fill } from "./dom";
import { compact, cost, count, elapsed, percent } from "./format";

/** The presets, date-range first, as the reader reaches for them. */
const PRESETS = [
  { id: "today", label: "Today", days: 0 },
  { id: "7d", label: "Last 7 days", days: 7 },
  { id: "30d", label: "Last 30 days", days: 30 },
  { id: "90d", label: "Last 90 days", days: 90 },
  { id: "all", label: "All of it", days: null },
] as const;

type PresetId = (typeof PRESETS)[number]["id"];

/** How many tools a bar chart shows before the tail becomes "Other". */
const TOOL_SLOTS = 8;
/** How many projects the leaderboard shows. */
const PROJECT_SLOTS = 8;
/** Above this many days, the axis labels every nth column instead of all. */
const MAX_TICKS = 12;

const DAY_MS = 86_400_000;

/**
 * The window a preset resolves to, in absolute milliseconds.
 *
 * Absolute for the same reason the timeline's hash is (task 5.6): a period
 * that means something different tomorrow is not evidence. "Today" starts at
 * local midnight because that is what the word means to the person reading it.
 */
export function windowFor(preset: PresetId, project: string | null): Period {
  const now = Date.now();
  const chosen = PRESETS.find((p) => p.id === preset) ?? PRESETS[2];
  let since: number | null;
  if (chosen.days === null) since = null;
  else if (chosen.days === 0) since = new Date(new Date().setHours(0, 0, 0, 0)).getTime();
  else since = now - chosen.days * DAY_MS;

  return {
    since,
    until: since === null ? null : now,
    project_path: project,
    // Days are the reader's days, not Greenwich's.
    utc_offset_minutes: -new Date().getTimezoneOffset(),
  };
}

/** `2026-03-02` as `2 Mar`, for an axis tick. */
function dayTick(iso: string): string {
  const [, month, day] = iso.split("-");
  if (month === undefined || day === undefined) return iso;
  const months = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
  return `${Number(day)} ${months[Number(month) - 1] ?? month}`;
}

/**
 * Fill the days the store had nothing for.
 *
 * The query returns only days with activity, deliberately: a gap is the
 * chart's business, because only the chart knows how wide a column is. A run
 * of quiet days is information — it is what a week off looks like — so the
 * columns are there and empty rather than absent.
 */
export function fillDays(buckets: Bucket[], window: Period): Bucket[] {
  const present = new Map(buckets.map((b) => [b.key, b]));
  const keys = buckets.map((b) => b.key).filter((k): k is string => k !== null);
  const offset = window.utc_offset_minutes * 60_000;

  const iso = (ms: number): string => new Date(ms + offset).toISOString().slice(0, 10);
  const first = window.since ?? (keys.length > 0 ? Date.parse(`${keys[0] ?? ""}T00:00:00Z`) - offset : null);
  const last = window.until ?? Date.now();
  if (first === null || !Number.isFinite(first)) return buckets;

  const out: Bucket[] = [];
  for (let day = Date.parse(`${iso(first)}T00:00:00Z`) - offset; day <= last; day += DAY_MS) {
    const key = iso(day);
    out.push(
      present.get(key) ?? {
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
      },
    );
    // A store spanning years would otherwise build a column per day of it.
    if (out.length > 400) break;
  }
  return out;
}

/** Label every nth tick, so a 90-day axis does not overlap itself. */
function thin<T extends { tick?: string }>(data: T[]): T[] {
  const every = Math.ceil(data.length / MAX_TICKS);
  if (every <= 1) return data;
  return data.map((d, i) => (i % every === 0 ? d : { ...d, tick: "" }));
}

/**
 * A signed change against the period before, or nothing to compare with.
 *
 * `upIsBad` is what makes the colour meaningful, and it is `false` for almost
 * everything here: more tool calls is not better or worse, and an interface
 * that paints it green has an opinion it has no basis for. Only a metric with
 * a real direction — an error rate — passes `true`.
 */
function delta(
  current: number,
  previous: number | undefined,
  colour: boolean,
  upIsBad = false,
): { text: string; direction: "up" | "down" | "flat"; tone: "good" | "bad" | "neutral" } | undefined {
  if (previous === undefined) return undefined;
  if (previous === 0 && current === 0) return { text: "no change", direction: "flat", tone: "neutral" };
  if (previous === 0) return { text: "new", direction: "up", tone: "neutral" };

  const change = (current - previous) / previous;
  const direction = Math.abs(change) < 0.005 ? "flat" : change > 0 ? "up" : "down";
  const moved = direction === "up";
  const tone: "good" | "bad" | "neutral" =
    !colour || direction === "flat" ? "neutral" : moved === upIsBad ? "bad" : "good";
  // Past about tenfold a percentage stops being a quantity anyone reads —
  // "9,658% more calls" is true and unusable — so it becomes a multiple.
  const size = Math.abs(change);
  const text =
    size >= 10
      ? `×${(current / previous).toFixed(size >= 100 ? 0 : 1)} vs the period before`
      : `${(size * 100).toFixed(0)}% vs the period before`;
  return { text, direction, tone };
}

export class AnalyticsView {
  readonly node: HTMLElement;

  private readonly controls: HTMLElement;
  private readonly content: HTMLElement;
  private readonly projectPick: HTMLSelectElement;
  private preset: PresetId = "30d";
  private project: string | null = null;
  private data: Usage | null = null;
  private loading = false;

  constructor(private readonly opts: { onNotice: (message: string) => void }) {
    const periodPick = el("select", { class: "pick", attrs: { "aria-label": "Period" } });
    for (const p of PRESETS) {
      append(periodPick, el("option", { value: p.id, text: p.label }));
    }
    periodPick.value = this.preset;
    periodPick.addEventListener("change", () => {
      this.preset = (periodPick.value as PresetId) || "30d";
      void this.refresh();
    });

    this.projectPick = el("select", { class: "pick", attrs: { "aria-label": "Project" } });
    this.projectPick.addEventListener("change", () => {
      this.project = this.projectPick.value === "" ? null : this.projectPick.value;
      void this.refresh();
    });

    // One filter row, above everything it scopes, so every figure below
    // describes the same slice.
    this.controls = el("div", { class: "bar" }, [
      el("div", { class: "controls" }, [periodPick, this.projectPick]),
    ]);
    this.content = el("div", { class: "sheet" });
    this.node = el("section", { class: "analytics" }, [this.controls, this.content]);
  }

  /** Fetch and draw. Called on every filter change and when the tab opens. */
  async refresh(): Promise<void> {
    if (this.loading) return;
    this.loading = true;
    // Refetch keeps the frame: the previous render dims rather than blanking.
    this.content.classList.add("busy");
    try {
      if (this.projectPick.options.length === 0) await this.loadProjects();
      this.data = await usage(windowFor(this.preset, this.project));
      this.draw();
    } catch (error) {
      this.opts.onNotice(`Analytics failed: ${String(error)}`);
    } finally {
      this.loading = false;
      this.content.classList.remove("busy");
    }
  }

  private async loadProjects(): Promise<void> {
    const f = await facets();
    append(this.projectPick, el("option", { value: "", text: "Every project" }));
    for (const path of f.projects) {
      append(this.projectPick, el("option", { value: path, text: path.replace(/^.*\//, "") , title: path }));
    }
  }

  private draw(): void {
    const data = this.data;
    if (data === null) return;
    const a = data.analytics;

    if (a.calls.calls === 0) {
      fill(this.content, [
        el("p", { class: "empty" }, [
          el("strong", { text: "Nothing in this period." }),
          " Widen it, or import history from the Status tab.",
        ]),
      ]);
      return;
    }

    fill(this.content, [
      this.tiles(),
      this.coverageNote(),
      columnChart({
        title: "Tool calls per day",
        caption: "Every call, from both lanes.",
        columns: ["Day", "Calls"],
        format: count,
        data: thin(
          fillDays(a.by_day, a.window).map((b) => ({
            key: b.key ?? "unknown",
            tick: dayTick(b.key ?? ""),
            value: b.calls,
            detail: [
              ["Failed", count(b.failures)],
              ["Spend", b.requests === 0 ? "not captured" : cost(b.cost_usd_micros)],
            ] as [string, string][],
          })),
        ),
      }),
      this.spendPerDay(),
      el("div", { class: "pair" }, [this.tools(), this.projects()]),
      el("div", { class: "pair" }, [this.ratios(), this.models()]),
      el("p", { class: "footnote" }, [
        `Active time counts wall-clock with a tool call at least every five minutes, per session. `,
        `Percentiles come from the OTLP lane, which is the only one that times a call. `,
        `Comparisons are against the ${this.periodName()} immediately before this one.`,
      ]),
    ]);
  }

  private periodName(): string {
    const chosen = PRESETS.find((p) => p.id === this.preset);
    if (chosen === undefined || chosen.days === null) return "period";
    return chosen.days === 0 ? "day" : `${chosen.days} days`;
  }

  private tiles(): HTMLElement {
    const data = this.data;
    if (data === null) return el("div");
    const a = data.analytics;
    // `null` from Rust means the window is open-ended and has no "before".
    const prev = data.comparison.previous ?? undefined;

    const errorRate = (h: { failures: number; calls: number }): number =>
      h.calls === 0 ? 0 : h.failures / h.calls;

    return el("div", { class: "tiles" }, [
      statTile({
        label: "Tool calls",
        value: compact(a.calls.calls),
        delta: delta(a.calls.calls, prev?.calls, false),
      }),
      statTile({
        label: "Sessions",
        value: count(a.calls.sessions),
        delta: delta(a.calls.sessions, prev?.sessions, false),
      }),
      statTile({
        label: "Active time",
        value: elapsed(a.calls.active_ms),
        delta: delta(a.calls.active_ms, prev?.active_ms, false),
      }),
      statTile({
        label: "Spend",
        // Three states, and the third one says what it is (task 6.8).
        value: a.coverage.measured ? cost(a.cost.cost_usd_micros) : "not captured",
        note: a.coverage.measured
          ? a.coverage.complete
            ? undefined
            : `${count(a.coverage.sessions_with_cost)} of ${count(a.coverage.sessions)} sessions priced`
          : "no session here was captured live",
        // A spend delta against a period with different coverage would be
        // comparing a measurement with a gap, so it is only offered when both
        // periods have some.
        delta:
          a.coverage.measured && (prev?.sessions_with_cost ?? 0) > 0
            ? delta(a.cost.cost_usd_micros, prev?.cost_usd_micros, false)
            : undefined,
      }),
      statTile({
        label: "Error rate",
        value: percent(a.calls.error_rate),
        note: `${count(a.calls.failures)} of ${count(a.calls.with_outcome)} with a recorded outcome`,
        // The one metric here with a direction worth colouring.
        delta: delta(errorRate(a.calls), prev === undefined ? undefined : errorRate(prev), true, true),
      }),
      statTile({
        label: "Refused",
        value: count(a.calls.refused),
        note: a.calls.refused === 0 ? "nothing was denied here" : "denied by a person or a rule",
      }),
    ]);
  }

  /** Task 6.8, in one paragraph rather than a silent zero. */
  private coverageNote(): HTMLElement | null {
    const a = this.data?.analytics;
    if (a === undefined || a.coverage.complete) return null;

    return el("p", { class: a.coverage.measured ? "banner" : "banner strong" }, [
      el("strong", { text: a.coverage.measured ? "Cost is partly measured. " : "No cost was captured here. " }),
      a.coverage.measured
        ? `${count(a.coverage.sessions_with_cost)} of ${count(a.coverage.sessions)} sessions in this period were captured live, covering ${count(a.coverage.calls_with_cost)} of ${count(a.coverage.calls)} calls. `
        : `All ${count(a.coverage.sessions)} sessions here were imported from transcripts. `,
      "Spend and tokens are recorded by the OTLP lane as a session runs; a transcript records neither, so imported history has no cost and cannot be given one retrospectively.",
    ]);
  }

  private spendPerDay(): HTMLElement {
    const a = this.data?.analytics;
    if (a === undefined) return el("div");
    const days = fillDays(a.by_day, a.window).filter((b) => b.requests > 0);

    return columnChart({
      title: "Spend per day",
      caption: a.coverage.complete
        ? "US dollars, from the API requests behind these calls."
        : "US dollars — for the sessions that were captured live.",
      columns: ["Day", "Spend"],
      format: (v) => cost(v),
      empty:
        "No priced day in this period. Cost arrives with the OTLP lane while a session runs, so there is nothing to plot for imported history.",
      data: thin(
        days.map((b) => ({
          key: b.key ?? "unknown",
          tick: dayTick(b.key ?? ""),
          value: b.cost_usd_micros,
          detail: [
            ["Requests", count(b.requests)],
            ["Input", compact(b.input_tokens)],
            ["Output", compact(b.output_tokens)],
            ["Cached", compact(b.cache_read_tokens)],
          ] as [string, string][],
        })),
      ),
    });
  }

  private tools(): HTMLElement {
    const a = this.data?.analytics;
    if (a === undefined) return el("div");

    const shown = a.tools.slice(0, TOOL_SLOTS);
    const tail = a.tools.slice(TOOL_SLOTS);
    const data: Datum[] = shown.map((t) => ({
      key: t.tool_name,
      value: t.calls,
      detail: [
        ["Failed", count(t.failures)],
        ["p50", t.p50_ms === null ? "—" : `${count(t.p50_ms)} ms`],
        ["p95", t.p95_ms === null ? "—" : `${count(t.p95_ms)} ms`],
      ],
    }));
    if (tail.length > 0) {
      // The tail folds into one bar rather than becoming more bars: past eight
      // the reader is reading a table, and the table twin is right there.
      data.push({
        key: `Other (${tail.length})`,
        value: tail.reduce((sum, t) => sum + t.calls, 0),
        detail: [
          ["Failed", count(tail.reduce((sum, t) => sum + t.failures, 0))],
          ["p50", "—"],
          ["p95", "—"],
        ],
      });
    }

    return barChart({
      title: "Tools by use",
      caption: `${count(a.tools.length)} distinct tools. Latency is from the OTLP lane.`,
      columns: ["Tool", "Calls"],
      format: count,
      data,
    });
  }

  private projects(): HTMLElement {
    const a = this.data?.analytics;
    if (a === undefined) return el("div");
    const priced = a.coverage.measured;
    const rows = a.by_project.slice(0, PROJECT_SLOTS);

    return barChart({
      // Ranked by spend when spend exists, and honestly renamed when it does
      // not — a leaderboard of zeroes ranks nothing.
      title: priced ? "Projects by spend" : "Projects by use",
      caption: priced
        ? "Only the sessions captured live carry cost."
        : "No cost recorded in this period, so this is call volume.",
      columns: ["Project", priced ? "Spend" : "Calls"],
      format: priced ? (v) => cost(v) : count,
      data: rows.map((b) => ({
        key: b.key === null ? "unknown project" : b.key.replace(/^.*\//, ""),
        value: priced ? b.cost_usd_micros : b.calls,
        detail: [
          ["Calls", count(b.calls)],
          ["Failed", count(b.failures)],
          ["Spend", b.requests === 0 ? "not captured" : cost(b.cost_usd_micros)],
        ],
      })),
    });
  }

  private ratios(): HTMLElement {
    const a = this.data?.analytics;
    if (a === undefined) return el("div");

    return el("div", { class: "chart" }, [
      el("figcaption", { class: "chart-head" }, [
        el("div", { class: "chart-titles" }, [
          el("span", { class: "chart-title", text: "Ratios" }),
          el("span", { class: "chart-caption", text: "Each against what was actually measured." }),
        ]),
      ]),
      meter({
        label: "Prompt cache hits",
        ratio: a.cost.cache_hit_ratio,
        value: percent(a.cost.cache_hit_ratio),
        ...(a.cost.cache_hit_ratio === null
          ? { note: "No token counts in this period, so there is no ratio to take." }
          : {}),
      }),
      meter({
        label: "Subagent share of calls",
        ratio: a.calls.sidechain_share,
        value: percent(a.calls.sidechain_share),
      }),
      meter({
        label: "Sessions with cost data",
        ratio: a.coverage.sessions === 0 ? null : a.coverage.sessions_with_cost / a.coverage.sessions,
        value: `${count(a.coverage.sessions_with_cost)} / ${count(a.coverage.sessions)}`,
        ...(a.coverage.complete
          ? {}
          : { note: "The unfilled part is imported history, which records no cost." }),
      }),
      el("dl", { class: "kv" }, [
        el("dt", { text: "p50" }),
        el("dd", { text: a.calls.p50_ms === null ? "—" : `${count(a.calls.p50_ms)} ms` }),
        el("dt", { text: "p95" }),
        el("dd", { text: a.calls.p95_ms === null ? "—" : `${count(a.calls.p95_ms)} ms` }),
        el("dt", { text: "Tokens" }),
        el("dd", { text: a.cost.total_tokens === 0 ? "—" : compact(a.cost.total_tokens) }),
      ]),
    ]);
  }

  private models(): HTMLElement {
    const a = this.data?.analytics;
    if (a === undefined) return el("div");

    return dataTable({
      title: "Models",
      caption: "Requests, not tool calls: a call is made in a turn a model asked for.",
      head: ["Model", "Requests", "Spend", "Input", "Output", "Cached"],
      empty: "No API request in this period carried a model, which means none was captured live.",
      rows: a.by_model.map((b) => [
        b.key ?? "unknown",
        count(b.requests),
        cost(b.cost_usd_micros),
        compact(b.input_tokens),
        compact(b.output_tokens),
        compact(b.cache_read_tokens),
      ]),
    });
  }
}
