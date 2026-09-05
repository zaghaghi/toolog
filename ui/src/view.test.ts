import { describe, expect, test } from "vitest";

import type { TimelineFilter } from "./bindings";
import {
  emptyFilter,
  emptyView,
  fromHash,
  isFiltered,
  laneOf,
  outcomeOf,
  sameFilter,
  threadOf,
  toHash,
  withLane,
  withOutcome,
  withThread,
} from "./view";

describe("the URL hash (task 5.6)", () => {
  test("an unfiltered view has an empty hash", () => {
    expect(toHash(emptyView())).toBe("");
  });

  test("every filter field survives a round trip", () => {
    const filter: TimelineFilter = {
      session_id: "s-1",
      session_unknown: true,
      project_path: "/work/app",
      tool_name: "Bash",
      since: 1_700_000_000_000,
      until: 1_700_000_600_000,
      success: false,
      is_sidechain: true,
      decision: "reject",
      decision_source: "user_reject",
      permission_mode: "acceptEdits",
      agent_id: "a-9",
      main_thread: false,
      query: "rm -rf",
      provenance: 3,
    };
    const view = { filter, grouped: true, selected: "toolu_7" };

    const restored = fromHash(toHash(view));
    expect(restored.filter).toEqual(filter);
    expect(restored.grouped).toBe(true);
    expect(restored.selected).toBe("toolu_7");
  });

  test("a time range is stored absolute, so a shared view keeps its meaning", () => {
    const since = Date.now() - 3_600_000;
    const hash = toHash({ ...emptyView(), filter: { ...emptyFilter(), since } });
    expect(hash).toContain(`since=${since}`);
    expect(fromHash(hash).filter.since).toBe(since);
  });

  test("shell syntax in a search term survives being a URL", () => {
    for (const query of ["rm -rf", "a | b", "*.rs", "foo:bar", "a&b=c", "#hash"]) {
      const hash = toHash({ ...emptyView(), filter: { ...emptyFilter(), query } });
      expect(fromHash(hash).filter.query).toBe(query);
    }
  });

  test("nonsense in the hash is ignored rather than fatal", () => {
    const view = fromHash("#since=notanumber&tool=Bash&group=nope");
    expect(view.filter.since).toBeNull();
    expect(view.filter.tool_name).toBe("Bash");
    expect(view.grouped).toBe(false);
  });
});

describe("the controls that collapse several columns into one choice", () => {
  test("outcome maps onto success and decision, and back", () => {
    for (const outcome of ["any", "ok", "failed", "refused"] as const) {
      expect(outcomeOf(withOutcome(emptyFilter(), outcome))).toBe(outcome);
    }
    expect(withOutcome(emptyFilter(), "refused").decision).toBe("reject");
    // Choosing one clears the other, or "failed" would survive under "refused".
    const refused = withOutcome(withOutcome(emptyFilter(), "failed"), "refused");
    expect(refused.success).toBeNull();
  });

  test("the lane control is exact, so 'transcript only' means only", () => {
    for (const lane of ["any", "both", "transcript", "otel"] as const) {
      expect(laneOf(withLane(emptyFilter(), lane))).toBe(lane);
    }
    expect(withLane(emptyFilter(), "transcript").provenance).toBe(1);
    expect(withLane(emptyFilter(), "otel").provenance).toBe(2);
    expect(withLane(emptyFilter(), "both").provenance).toBe(3);
  });

  test("the thread control reads agent_id, not is_sidechain", () => {
    for (const thread of ["any", "main", "sub"] as const) {
      expect(threadOf(withThread(emptyFilter(), thread))).toBe(thread);
    }
    expect(withThread(emptyFilter(), "main").main_thread).toBe(true);
    expect(withThread(emptyFilter(), "sub").main_thread).toBe(false);
    expect(withThread(emptyFilter(), "any").is_sidechain).toBeNull();
  });
});

describe("filter comparison", () => {
  test("an empty filter is not a filter", () => {
    expect(isFiltered(emptyFilter())).toBe(false);
    expect(isFiltered({ ...emptyFilter(), success: false })).toBe(true);
  });

  test("a filter that differs in one field is a different filter", () => {
    expect(sameFilter(emptyFilter(), emptyFilter())).toBe(true);
    expect(sameFilter(emptyFilter(), { ...emptyFilter(), query: "x" })).toBe(false);
  });
});
