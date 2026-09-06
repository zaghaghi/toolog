import { describe, expect, test } from "vitest";

import { basename, bytes, duration, EM_DASH, lanes, shortPath } from "./format";

describe("missing is not zero (task 5.5)", () => {
  test("a duration nobody measured is a dash", () => {
    expect(duration(null)).toBe(EM_DASH);
    expect(duration(0)).toBe("0 ms");
  });

  test("a result with no recorded size is a dash", () => {
    expect(bytes(null)).toBe(EM_DASH);
    expect(bytes(0)).toBe("0 B");
  });
});

describe("durations are shown at the precision worth reading", () => {
  test.each([
    [40, "40 ms"],
    [999, "999 ms"],
    [1_000, "1.00 s"],
    [1_234, "1.23 s"],
    [12_400, "12.4 s"],
    [61_000, "1m 01s"],
    [3_601_000, "60m 01s"],
  ])("%i ms reads as %s", (ms, expected) => {
    expect(duration(ms)).toBe(expected);
  });
});

describe("paths", () => {
  test("a project is named by its last segment", () => {
    expect(basename("/Users/x/Projects/toolog")).toBe("toolog");
    expect(basename("/Users/x/Projects/toolog/")).toBe("toolog");
    expect(basename(null)).toBe(EM_DASH);
  });

  test("a long path keeps the informative end", () => {
    expect(shortPath("/a/b/c/d/e.rs")).toBe("…/c/d/e.rs");
    expect(shortPath("/a/b.rs")).toBe("/a/b.rs");
  });
});

describe("provenance reads as words", () => {
  test.each([
    [3, "both lanes"],
    [1, "transcript only"],
    [2, "OTEL only"],
    [0, "neither lane"],
  ])("%i is %s", (bits, expected) => {
    expect(lanes(bits)).toBe(expected);
  });
});
