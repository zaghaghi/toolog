//! The activity histogram over the timeline's own filter (tasks 10.3, 10.4).
//!
//! The one chart from the deleted usage view the owner asked to keep, moved to
//! where it belongs and re-keyed. On the usage page it was *Tool calls per day*
//! over an analytics `Period`; here it is calls per bucket over the
//! `TimelineFilter` the list is reading, which is the difference between a
//! chart beside a list and a chart *of* it.
//!
//! It both describes the range and sets it. Clicking a column narrows to that
//! column; dragging across brushes a range. Both write **absolute** `since` and
//! `until` into the same filter the list, the count and an export read, so the
//! four cannot disagree — the same reason task 5.6 made the hash absolute.

import type { Histogram as HistogramData, TimelineFilter } from "./bindings";
import { timelineHistogram } from "./bindings";
import { columnChart } from "./chart";
import type { Datum } from "./chart";
import { el, fill } from "./dom";
import { count } from "./format";

/** Remembered across sessions, because a collapsed chart should stay collapsed. */
const COLLAPSED_KEY = "toolog.histogram.collapsed";

/** How a bucket start is labelled, per size. */
const TICKS: Record<HistogramData["size"], Intl.DateTimeFormatOptions> = {
  minute: { hour: "2-digit", minute: "2-digit", hour12: false },
  hour: { hour: "2-digit", minute: "2-digit", hour12: false },
  day: { day: "numeric", month: "short" },
  week: { day: "numeric", month: "short" },
};

/** The full stamp a tooltip and the table twin carry. */
const FULL: Record<HistogramData["size"], Intl.DateTimeFormatOptions> = {
  minute: { dateStyle: "medium", timeStyle: "short" },
  hour: { dateStyle: "medium", timeStyle: "short" },
  day: { dateStyle: "full" },
  week: { dateStyle: "medium" },
};

const WIDTH_MS: Record<HistogramData["size"], number> = {
  minute: 60_000,
  hour: 3_600_000,
  day: 86_400_000,
  week: 7 * 86_400_000,
};

/** How the caption names the bucket. */
const NOUN: Record<HistogramData["size"], string> = {
  minute: "minute",
  hour: "hour",
  day: "day",
  week: "week",
};

/**
 * How many columns are labelled before the axis starts skipping.
 *
 * Sixty stamps under a chart this wide is a grey band, not an axis.
 */
const MAX_TICKS = 12;

function read(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    // A private window, or site data switched off. A remembered preference is
    // a convenience; losing it must not take the chart with it.
    return null;
  }
}

function write(key: string, value: string): void {
  try {
    localStorage.setItem(key, value);
  } catch {
    /* see `read` */
  }
}

export interface HistogramOptions {
  /** A range was chosen on the chart. Absolute, inclusive of both ends. */
  onRange: (since: number, until: number) => void;
}

export class ActivityHistogram {
  readonly node = el("section", { class: "histo" });

  private readonly body = el("div", { class: "histo-body" });
  private readonly toggle: HTMLButtonElement;
  private data: HistogramData | null = null;
  private collapsed = read(COLLAPSED_KEY) === "1";
  /** Bumped per load so a slow histogram for an old filter is dropped. */
  private generation = 0;

  constructor(private readonly options: HistogramOptions) {
    this.toggle = el("button", { class: "histo-toggle", attrs: { type: "button" } });
    this.toggle.addEventListener("click", () => {
      this.collapsed = !this.collapsed;
      write(COLLAPSED_KEY, this.collapsed ? "1" : "0");
      this.draw();
    });
    this.node.append(this.toggle, this.body);
    this.draw();
  }

  /** Fetch the histogram for a filter. Errors leave the chart out entirely. */
  async load(filter: TimelineFilter): Promise<void> {
    this.generation += 1;
    const generation = this.generation;
    try {
      // The window's own offset: a day boundary is a local fact and the store
      // keeps UTC.
      const data = await timelineHistogram(filter, -new Date().getTimezoneOffset());
      if (generation !== this.generation) return;
      this.data = data;
    } catch {
      if (generation !== this.generation) return;
      this.data = null;
    }
    this.draw();
  }

  private draw(): void {
    const data = this.data;
    const total = data === null ? 0 : data.buckets.reduce((sum, b) => sum + b.calls, 0);

    // Nothing to plot is not a collapsed chart and not an empty frame: the
    // section leaves, and the list gets the height back.
    this.node.hidden = data === null || data.buckets.length === 0;
    if (this.node.hidden) return;

    this.toggle.textContent = this.collapsed ? "Show activity" : "Hide activity";
    this.toggle.setAttribute("aria-expanded", String(!this.collapsed));
    this.body.hidden = this.collapsed;
    if (this.collapsed || data === null) return;

    const tick = new Intl.DateTimeFormat(undefined, TICKS[data.size]);
    const full = new Intl.DateTimeFormat(undefined, FULL[data.size]);
    const every = Math.max(1, Math.ceil(data.buckets.length / MAX_TICKS));

    const points: Datum[] = data.buckets.map((bucket, i) => ({
      key: full.format(bucket.start_ms),
      tick: i % every === 0 ? tick.format(bucket.start_ms) : "",
      value: bucket.calls,
      detail: [
        ["Failed", count(bucket.failures)],
        ["Refused", count(bucket.refusals)],
      ],
    }));

    fill(this.body, [
      columnChart({
        title: "Activity",
        caption: `${count(total)} ${total === 1 ? "call" : "calls"} by ${NOUN[data.size]} — click a column, or drag across to pick a range`,
        columns: [NOUN[data.size].replace(/^./, (c) => c.toUpperCase()), "Calls"],
        data: points,
        format: (v) => count(v),
        onPick: (index) => this.pick(index, index),
        onBrush: (from, to) => this.pick(from, to),
      }),
    ]);
  }

  /**
   * Turn a column range into an absolute time range.
   *
   * `until` is the last instant *inside* the final column, not the start of the
   * next one: `selection()` binds `until` as `called_at <= ?`, so an exclusive
   * bound written here would pull in the first call of the following bucket and
   * the chart would stop reproducing itself.
   */
  private pick(from: number, to: number): void {
    const buckets = this.data?.buckets;
    const size = this.data?.size;
    if (buckets === undefined || size === undefined) return;
    const first = buckets[from];
    const last = buckets[to];
    if (first === undefined || last === undefined) return;
    this.options.onRange(first.start_ms, last.start_ms + WIDTH_MS[size] - 1);
  }
}
