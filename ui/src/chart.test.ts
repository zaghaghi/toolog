//! The chart primitives, against the rules they were built to (task 6.6).
//!
//! These are the checks the `dataviz` procedure makes falsifiable: axis ticks
//! land on round numbers, every plot has a real table behind it, a zero is
//! drawn as a zero rather than as absence, and a value nobody measured is a
//! dash rather than an empty track.

import { describe, expect, test } from "vitest";

import { barChart, columnChart, dataTable, meter, scaleTop, statTile, ticks } from "./chart";

describe("ticks", () => {
  test("land on numbers a reader does not have to do arithmetic on", () => {
    expect(ticks(0)).toEqual([0]);
    expect(ticks(100)).toEqual([0, 50, 100]);
    expect(ticks(3200)).toEqual([0, 1000, 2000, 3000, 4000]);
    expect(ticks(1)).toEqual([0, 1]);
    expect(ticks(7)).toEqual([0, 2, 4, 6, 8]);
  });

  test("every step is one, two or five times a power of ten", () => {
    for (const max of [1, 3, 9, 17, 64, 231, 4096, 99_999]) {
      const marks = ticks(max);
      const step = (marks[1] ?? 0) - (marks[0] ?? 0);
      const mantissa = step / 10 ** Math.floor(Math.log10(step));
      expect([1, 2, 5, 10]).toContain(Math.round(mantissa));
    }
  });

  test("the scale reaches at least the largest value", () => {
    expect(scaleTop([3, 17, 4])).toBeGreaterThanOrEqual(17);
    expect(scaleTop([])).toBe(1);
    expect(scaleTop([0, 0])).toBe(1);
  });

  test("the top gridline is the top of the scale", () => {
    // The gridlines are spread evenly across the plot, so the last tick has to
    // be the height a full-height bar means. A tick below the largest value
    // would put bars above a line labelled less than they are.
    for (const max of [1, 7, 100, 3200, 12_345]) {
      const marks = ticks(max);
      expect(marks[marks.length - 1]).toBe(scaleTop([max]));
      expect(marks[marks.length - 1] ?? 0).toBeGreaterThanOrEqual(max);
    }
  });
});

describe("a column chart", () => {
  const spec = {
    title: "Calls per day",
    columns: ["Day", "Calls"] as [string, string],
    format: (v: number) => String(v),
    data: [
      { key: "2026-03-02", tick: "2 Mar", value: 4, detail: [["Failed", "1"]] as [string, string][] },
      { key: "2026-03-03", tick: "3 Mar", value: 0, detail: [["Failed", "0"]] as [string, string][] },
      { key: "2026-03-04", tick: "4 Mar", value: 8, detail: [["Failed", "0"]] as [string, string][] },
    ],
  };

  test("draws one column per datum, scaled against the top of the axis", () => {
    const node = columnChart(spec);
    const marks = node.querySelectorAll<HTMLElement>(".chart-col-mark");
    expect(marks).toHaveLength(3);
    // The axis tops out at 8, so the tallest column fills the plot.
    expect(marks[2]?.style.height).toBe("100.00%");
    expect(marks[0]?.style.height).toBe("50.00%");
  });

  test("a day with nothing is a hairline on the baseline, not a missing column", () => {
    const node = columnChart(spec);
    const marks = node.querySelectorAll<HTMLElement>(".chart-col-mark");
    // Zero height, which the mark's 1px minimum turns into a visible baseline.
    expect(marks[1]?.style.height).toBe("0.00%");
  });

  test("carries a table with every value in it", () => {
    const node = columnChart(spec);
    const rows = node.querySelectorAll(".chart-table tbody tr");
    expect(rows).toHaveLength(3);
    expect(rows[0]?.textContent).toContain("2026-03-02");
    expect(rows[0]?.textContent).toContain("4");
    // The detail a tooltip would show is in the table too, so hovering is
    // never the only way to read it.
    const heads = [...node.querySelectorAll(".chart-table thead th")].map((h) => h.textContent);
    expect(heads).toEqual(["Day", "Calls", "Failed"]);
  });

  test("the table is the twin of the plot, not a replacement shown at once", () => {
    const node = columnChart(spec);
    const table = node.querySelector<HTMLElement>(".chart-table");
    const plot = node.querySelector<HTMLElement>(".chart-body");
    expect(table?.hidden).toBe(true);
    expect(plot?.hidden).toBe(false);

    node.querySelector<HTMLButtonElement>(".chart-toggle")?.click();
    expect(table?.hidden).toBe(false);
    expect(plot?.hidden).toBe(true);
  });

  test("keyboard focus shows what hover shows", () => {
    const node = columnChart(spec);
    const column = node.querySelectorAll<HTMLElement>(".chart-col")[2];
    column?.dispatchEvent(new FocusEvent("focus"));
    const tip = node.querySelector<HTMLElement>(".chart-tip");
    expect(tip?.hidden).toBe(false);
    expect(tip?.textContent).toContain("2026-03-04");

    column?.dispatchEvent(new FocusEvent("blur"));
    expect(tip?.hidden).toBe(true);
  });

  test("says so rather than drawing an empty frame", () => {
    const node = columnChart({ ...spec, data: [], empty: "Nothing was captured here." });
    expect(node.querySelector(".chart-empty")?.textContent).toBe("Nothing was captured here.");
    expect(node.querySelector(".chart-plot")).toBeNull();
    expect(node.querySelector(".chart-toggle")).toBeNull();
  });

  test("the plot is announced as an image with its own summary", () => {
    const node = columnChart(spec);
    const plot = node.querySelector<HTMLElement>(".chart-body");
    expect(plot?.getAttribute("role")).toBe("img");
    expect(plot?.getAttribute("aria-label")).toContain("highest 2026-03-04");
  });
});

describe("a bar chart", () => {
  const spec = {
    title: "Tools by use",
    columns: ["Tool", "Calls"] as [string, string],
    format: (v: number) => String(v),
    data: [
      { key: "Bash", value: 40 },
      { key: "Read", value: 10 },
    ],
  };

  test("labels the value at the tip rather than inside the bar", () => {
    const node = barChart(spec);
    const row = node.querySelector<HTMLElement>(".chart-bar-row");
    expect(row?.querySelector(".chart-bar-value")?.textContent).toBe("40");
    // Nothing is written inside the mark, so nothing can be clipped by it.
    expect(row?.querySelector(".chart-bar-mark")?.textContent).toBe("");
  });

  test("scales bars against the same axis top", () => {
    const node = barChart(spec);
    const marks = node.querySelectorAll<HTMLElement>(".chart-bar-mark");
    expect(marks[0]?.style.width).toBe("100.00%");
    expect(marks[1]?.style.width).toBe("25.00%");
  });
});

describe("a meter", () => {
  test("shows a fraction of its track", () => {
    const node = meter({ label: "Cache hits", ratio: 0.75, value: "75.0%" });
    expect(node.querySelector<HTMLElement>(".meter-fill")?.style.width).toBe("75%");
    expect(node.querySelector(".meter-value")?.textContent).toBe("75.0%");
  });

  test("an unmeasured ratio has no fill and says why", () => {
    const node = meter({
      label: "Cache hits",
      ratio: null,
      value: "—",
      note: "No token counts in this period.",
    });
    expect(node.querySelector(".meter-fill")).toBeNull();
    expect(node.querySelector(".meter-track")?.className).toContain("empty");
    expect(node.querySelector(".meter-note")?.textContent).toContain("No token counts");
  });
});

describe("a stat tile", () => {
  test("colours a delta only when the direction means something", () => {
    const neutral = statTile({
      label: "Tool calls",
      value: "1,284",
      delta: { text: "12% vs the period before", direction: "up", tone: "neutral" },
    });
    expect(neutral.querySelector(".tile-delta")?.className).toContain("neutral");
    expect(neutral.querySelector(".tile-delta")?.textContent).toContain("↑");

    const bad = statTile({
      label: "Error rate",
      value: "4.0%",
      delta: { text: "50% vs the period before", direction: "up", tone: "bad" },
    });
    expect(bad.querySelector(".tile-delta")?.className).toContain("bad");
  });

  test("a trend is a sparkline, and one point is not a trend", () => {
    const withTrend = statTile({ label: "Calls per minute", value: "3", trend: [1, 2, 3] });
    expect(withTrend.querySelector(".spark-line")?.getAttribute("points")).toBeTruthy();

    const tooShort = statTile({ label: "Calls per minute", value: "3", trend: [3] });
    expect(tooShort.querySelector(".spark")).toBeNull();
  });
});

describe("a data table", () => {
  test("puts the first column in a row header and the rest in cells", () => {
    const node = dataTable({
      title: "Models",
      head: ["Model", "Requests"],
      rows: [["claude-opus-5", "12"]],
    });
    expect(node.querySelector("tbody th")?.textContent).toBe("claude-opus-5");
    expect(node.querySelector("tbody td")?.textContent).toBe("12");
  });

  test("an empty table says what is missing", () => {
    const node = dataTable({
      title: "Models",
      head: ["Model"],
      rows: [],
      empty: "Nothing was captured live.",
    });
    expect(node.querySelector(".chart-empty")?.textContent).toBe("Nothing was captured live.");
    expect(node.querySelector("table")).toBeNull();
  });
});

describe("the content security policy", () => {
  test("a mark's size is set on the element, never as a style attribute", () => {
    // `style-src 'self'` discards a `style` attribute and the bar then renders
    // at its natural size — silently, and in the application only. This is the
    // assertion that would have caught it.
    const node = columnChart({
      title: "Calls per day",
      format: (v) => String(v),
      data: [
        { key: "2026-03-02", value: 4 },
        { key: "2026-03-03", value: 8 },
      ],
    });
    for (const mark of node.querySelectorAll<HTMLElement>(".chart-col-mark")) {
      expect(mark.style.height).not.toBe("");
    }

    const bars = barChart({
      title: "Tools",
      format: (v) => String(v),
      data: [
        { key: "Bash", value: 40 },
        { key: "Read", value: 10 },
      ],
    });
    for (const mark of bars.querySelectorAll<HTMLElement>(".chart-bar-mark")) {
      expect(mark.style.width).not.toBe("");
    }

    const gauge = meter({ label: "Cache hits", ratio: 0.5, value: "50%" });
    expect(gauge.querySelector<HTMLElement>(".meter-fill")?.style.width).toBe("50%");
  });
});

describe("untrusted labels", () => {
  test("a key that looks like markup is text, not markup", () => {
    // Tool names and project paths come from a transcript, so they are data.
    const node = barChart({
      title: "Tools",
      format: (v) => String(v),
      data: [{ key: "<img src=x onerror=alert(1)>", value: 1 }],
    });
    expect(node.querySelector("img")).toBeNull();
    expect(node.querySelector(".chart-bar-key")?.textContent).toBe("<img src=x onerror=alert(1)>");
  });
});
