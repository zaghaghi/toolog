import { describe, expect, test } from "vitest";

import type { TimelineFilter } from "./bindings";
import {
  emptyFilter,
  emptyView,
  fromHash,
  isFiltered,
  laneOf,
  sameFilter,
  threadOf,
  toHash,
  withLane,
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
      risk: "high",
      rule_id: "auto-approved-destructive-bash",
    };
    const view = { filter, selected: "toolu_7" };

    const restored = fromHash(toHash(view));
    expect(restored.filter).toEqual(filter);
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
  });
});

describe("the controls that collapse several columns into one choice", () => {
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

describe("a v1.0 link still restores its view (task 10.7)", () => {
  /**
   * A hash written by v1.0, before the query bar existed.
   *
   * Copied out of a running v1.0 window: every dropdown set, grouping on and a
   * call open. The query bar is a second *editor* of the filter, not a second
   * representation of it, so this has to keep working — and the box has to be
   * able to say what the link restored, or a filter would be reachable by link
   * and unsayable in words.
   */
  const V1 =
    "#q=rm+-rf&project=%2Fwork%2Fapp&tool=Bash&session=s-1&agent=a-9" +
    "&since=1700000000000&until=1700000600000&source=user_reject&mode=acceptEdits" +
    "&decision=reject&lane=3&thread=false&ok=false&sidechain=true" +
    "&group=session&call=toolu_7";

  test("restores every field, the grouping and the open call", () => {
    const view = fromHash(V1);
    expect(view.filter).toEqual({
      session_id: "s-1",
      session_unknown: null,
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
      risk: null,
      rule_id: null,
    });
    expect(view.selected).toBe("toolu_7");
  });

  test("re-encodes to a hash that means the same thing", () => {
    expect(fromHash(toHash(fromHash(V1)))).toEqual(fromHash(V1));
  });
});
