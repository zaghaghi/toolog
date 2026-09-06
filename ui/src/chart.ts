//! The column chart (task 6.6, trimmed to one form by task 9.6).
//!
//! Built to the `dataviz` skill's procedure rather than to taste, and the
//! decisions that procedure forces are worth stating because they are not the
//! obvious ones:
//!
//! - **A chart plots one measure.** Calls and spend are different scales, so
//!   they would be two charts. A second y-axis would let the reader see a
//!   correlation the data does not contain, which on an audit tool is not a
//!   cosmetic problem.
//! - **One hue, light to dark.** Colour's job here is magnitude, never
//!   identity, so there is a single mark colour and no legend to read. The four
//!   values it uses were run through the skill's validator against both
//!   surfaces; see `styles/tokens.css`.
//! - **Every chart has a table.** The plot is `role="img"` with a summary, and
//!   the same numbers are one keystroke away in a real `<table>`. A tooltip is
//!   an enhancement; it is never the only way to read a value.
//! - **Marks are hit targets, not painted pixels.** A column's hover area is
//!   the full height of its slot, so aiming at a date is enough.
//!
//! The bars, the table card, the meter and the stat tile went with the cost
//! analytics they were drawn for. What is left is what Phase 10's activity
//! histogram is built from.
//!
//! The marks are HTML rather than SVG. A `<div>` with a height in percent
//! resizes with the window for free, and text inside it stays at the size the
//! tokens set instead of being scaled by a `viewBox`.
//!
//! Every mark's size goes through `el`'s `style` option, which assigns on
//! `element.style`. A `style` **attribute** would be dropped by the window's
//! Content Security Policy and every bar would silently render at its natural
//! size — which is what the first build of this view did.

import { el, fill } from "./dom";

/** One plotted value. */
export interface Datum {
  /** What this bar or column is: a day, a tool, a project. */
  key: string;
  /** The axis label, when it should differ from the key. */
  tick?: string | undefined;
  value: number;
  /** Extra rows for the tooltip and the table, as label/value pairs. */
  detail?: [string, string][] | undefined;
}

export interface ChartSpec {
  title: string;
  /** The line under the title: what is plotted, and any caveat about it. */
  caption?: string | undefined;
  data: Datum[];
  /** How a value becomes a string, everywhere it is shown. */
  format: (value: number) => string;
  /** Headings for the table twin's two main columns. */
  columns?: [string, string] | undefined;
  /** What to say when there is nothing to plot. Never an empty frame. */
  empty?: string | undefined;
}

// ---------------------------------------------------------------------------
// Axis
// ---------------------------------------------------------------------------

/**
 * Round tick values: 0, 1,000, 2,000 — never 0, 833, 1,666.
 *
 * The step is 1, 2 or 5 times a power of ten, which is the set of intervals
 * people read without doing arithmetic. The last tick is always at or above
 * `max`, because the top gridline *is* the top of the scale: a series that ran
 * past its highest label would draw bars above a line claiming a lower number.
 */
export function ticks(max: number, count = 4): number[] {
  if (max <= 0) return [0];
  const rough = max / count;
  const magnitude = 10 ** Math.floor(Math.log10(rough));
  // At least 1: every measure plotted here is a whole thing — a call, a
  // millisecond, a micro-dollar — so "0, 0.5, 1 calls" is a scale for a
  // quantity that does not exist.
  const step = Math.max(
    1,
    [1, 2, 5, 10].map((m) => m * magnitude).find((s) => s >= rough) ?? magnitude * 10,
  );
  const out: number[] = [];
  for (let v = 0; ; v += step) {
    out.push(v);
    if (v >= max) return out;
  }
}

/** The top of the scale, which is the highest tick. */
export function scaleTop(values: number[]): number {
  const marks = ticks(Math.max(0, ...values));
  return Math.max(marks[marks.length - 1] ?? 0, 1);
}

// ---------------------------------------------------------------------------
// The container
// ---------------------------------------------------------------------------

/** A `<figure>` with its title, its table-view toggle, and the plot inside. */
function figure(spec: ChartSpec, plot: HTMLElement, summary: string): HTMLElement {
  const table = tableTwin(spec);
  table.hidden = true;

  const toggle = el("button", {
    class: "chart-toggle",
    text: "Table",
    attrs: { "aria-expanded": "false", type: "button" },
  });
  toggle.addEventListener("click", () => {
    const showing = table.hidden;
    table.hidden = !showing;
    plot.hidden = showing;
    toggle.textContent = showing ? "Chart" : "Table";
    toggle.setAttribute("aria-expanded", String(showing));
  });

  plot.setAttribute("role", "img");
  plot.setAttribute("aria-label", summary);

  return el("figure", { class: "chart" }, [
    el("figcaption", { class: "chart-head" }, [
      el("div", { class: "chart-titles" }, [
        el("span", { class: "chart-title", text: spec.title }),
        spec.caption === undefined
          ? null
          : el("span", { class: "chart-caption", text: spec.caption }),
      ]),
      spec.data.length === 0 ? null : toggle,
    ]),
    spec.data.length === 0
      ? el("p", { class: "chart-empty", text: spec.empty ?? "Nothing in this window." })
      : plot,
    table,
  ]);
}

/** The WCAG-clean equivalent of any chart on this page. */
function tableTwin(spec: ChartSpec): HTMLElement {
  const [keyHead, valueHead] = spec.columns ?? ["", "Value"];
  const extra = spec.data[0]?.detail?.map(([label]) => label) ?? [];

  return el("table", { class: "chart-table" }, [
    el("thead", {}, [
      el("tr", {}, [
        el("th", { text: keyHead }),
        el("th", { class: "num", text: valueHead }),
        ...extra.map((label) => el("th", { class: "num", text: label })),
      ]),
    ]),
    el(
      "tbody",
      {},
      spec.data.map((d) =>
        el("tr", {}, [
          el("th", { attrs: { scope: "row" }, text: d.key }),
          el("td", { class: "num", text: spec.format(d.value) }),
          ...(d.detail ?? []).map(([, value]) => el("td", { class: "num", text: value })),
        ]),
      ),
    ),
  ]);
}

/**
 * The hover readout. The value leads and the label follows: the reader already
 * knows which bar they are pointing at and wants the number.
 */
function tooltip(): { node: HTMLElement; show: (d: Datum, format: (v: number) => string) => void } {
  const node = el("div", { class: "chart-tip", hidden: true });
  return {
    node,
    show(d, format) {
      fill(node, [
        el("span", { class: "tip-value", text: format(d.value) }),
        el("span", { class: "tip-key", text: d.key }),
        ...(d.detail ?? []).map(([label, value]) =>
          el("span", { class: "tip-row" }, [
            el("span", { class: "tip-row-label", text: label }),
            el("span", { class: "tip-row-value", text: value }),
          ]),
        ),
      ]);
      node.hidden = false;
    },
  };
}

// ---------------------------------------------------------------------------
// Columns — a trend over time
// ---------------------------------------------------------------------------

/**
 * Vertical columns with a crosshair.
 *
 * The crosshair is what makes the chart usable: the reader aims at a day, and
 * the nearest column answers. Nobody can reliably hit a 6px bar.
 */
export function columnChart(spec: ChartSpec): HTMLElement {
  const top = scaleTop(spec.data.map((d) => d.value));
  const marks = ticks(top);
  const tip = tooltip();
  const crosshair = el("div", { class: "chart-crosshair", hidden: true });

  const columns = spec.data.map((d, i) =>
    el(
      "div",
      {
        class: "chart-col",
        attrs: {
          tabindex: "0",
          role: "button",
          "aria-label": `${d.key}: ${spec.format(d.value)}`,
          "data-i": String(i),
        },
      },
      [
        el("div", {
          class: "chart-col-mark",
          // Zero is drawn as a hairline on the baseline rather than as nothing:
          // a day with no calls is a fact, not an absence.
          style: { height: `${((d.value / top) * 100).toFixed(2)}%` },
        }),
      ],
    ),
  );

  const plot = el("div", { class: "chart-plot" }, [
    el(
      "div",
      { class: "chart-grid" },
      marks
        .slice()
        .reverse()
        .map((v) => el("div", { class: "chart-gridline" }, [el("span", { class: "chart-ytick", text: spec.format(v) })])),
    ),
    el("div", { class: "chart-cols" }, columns),
    crosshair,
    tip.node,
  ]);

  const hover = (index: number): void => {
    const d = spec.data[index];
    const column = columns[index];
    if (d === undefined || column === undefined) return;
    for (const c of columns) c.classList.remove("on");
    column.classList.add("on");
    tip.show(d, spec.format);
    const centre = column.offsetLeft + column.offsetWidth / 2;
    crosshair.hidden = false;
    crosshair.style.left = `${centre}px`;
    // Keep the readout inside the plot: at the right-hand edge it flips to the
    // other side of the crosshair rather than being clipped.
    const flip = centre > plot.clientWidth / 2;
    tip.node.classList.toggle("flip", flip);
    tip.node.style.left = `${centre}px`;
  };

  const leave = (): void => {
    for (const c of columns) c.classList.remove("on");
    crosshair.hidden = true;
    tip.node.hidden = true;
  };

  plot.addEventListener("pointermove", (event) => {
    const rect = plot.getBoundingClientRect();
    const fraction = (event.clientX - rect.left) / Math.max(rect.width, 1);
    const index = Math.min(spec.data.length - 1, Math.max(0, Math.floor(fraction * spec.data.length)));
    hover(index);
  });
  plot.addEventListener("pointerleave", leave);
  for (const [i, column] of columns.entries()) {
    // Keyboard focus shows exactly what hover shows.
    column.addEventListener("focus", () => hover(i));
    column.addEventListener("blur", leave);
  }

  const axis = el(
    "div",
    { class: "chart-axis" },
    spec.data.map((d) => el("span", { class: "chart-xtick", text: d.tick ?? d.key })),
  );

  return figure(spec, el("div", { class: "chart-body" }, [plot, axis]), summarize(spec));
}

/** The `aria-label` a screen reader gets before it reaches the table. */
function summarize(spec: ChartSpec): string {
  if (spec.data.length === 0) return `${spec.title}: nothing to show.`;
  const top = spec.data.reduce((a, b) => (b.value > a.value ? b : a));
  return `${spec.title}. ${spec.data.length} values, highest ${top.key} at ${spec.format(top.value)}. The table below has all of them.`;
}
