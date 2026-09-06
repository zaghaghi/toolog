//! The query bar's language (tasks 10.5, 10.6, 10.9, 10.10).
//!
//! The load-bearing test here is the round trip. Seven dropdowns became one
//! text box, and the way that goes wrong is a filter you can reach with a click
//! and then cannot say in words — so `parse(format(f))` is asserted for every
//! field of `TimelineFilter`, one at a time and all at once, including a
//! project path with a space in it.

import { describe, expect, test } from "vitest";

import type { TimelineFilter } from "./bindings";
import { format, KEYS, parse, tokenize } from "./query";
import { emptyFilter, fromHash } from "./view";

function filter(patch: Partial<TimelineFilter>): TimelineFilter {
  return { ...emptyFilter(), ...patch };
}

/**
 * One value per field, chosen to be awkward where the field allows it.
 *
 * `since`, `until` and `session_unknown` are absent on purpose: the first two
 * belong to the time control, which pairs with the histogram, and the third is
 * reached by clicking the unattributed group. They are covered by the test
 * below asserting the query bar leaves them alone.
 */
const SAYABLE: Partial<TimelineFilter>[] = [
  { project_path: "/Users/me/some project" },
  { project_path: '/tmp/a "quoted" dir' },
  { tool_name: "mcp__linear__create_issue" },
  { session_id: "0198f2c1-1f2e-7a51-9b3e-2c4d5e6f7a8b" },
  { agent_id: "a-1" },
  { decision_source: "user_temporary" },
  { permission_mode: "acceptEdits" },
  { decision: "accept" },
  { decision: "reject" },
  { success: true },
  { success: false },
  { is_sidechain: true },
  { is_sidechain: false },
  { main_thread: true },
  { main_thread: false },
  { provenance: 1 },
  { provenance: 2 },
  { provenance: 3 },
  { query: "rm -rf" },
  { query: "@reboot" },
  { query: 'a "quoted" phrase' },
  { risk: "high" },
  { rule_id: "auto-approved-destructive-bash" },
  // Phase 13. `@intent` is free text like a project path, so it gets the same
  // quoting cases; `@llm-risk` is a comparison, so both forms it accepts are
  // here — a bare number and one with an operator.
  { intent: "deletes a directory" },
  { intent: 'writes a "config" file' },
  { llm_risk: ">=4" },
  { llm_risk: "5" },
];

describe("the round trip (task 10.5)", () => {
  test.each(SAYABLE)("parse(format(f)) is f for %o", (patch) => {
    const f = filter(patch);
    expect(parse(format(f)).filter).toEqual(f);
  });

  test("holds with every sayable field set at once", () => {
    const f = filter(Object.assign({}, ...SAYABLE) as Partial<TimelineFilter>);
    expect(parse(format(f)).filter).toEqual(f);
  });

  test("an empty filter is an empty query, and back", () => {
    expect(format(emptyFilter())).toBe("");
    expect(parse("").filter).toEqual(emptyFilter());
  });

  test("every key in the language round-trips something", () => {
    // Guards against a key that can be typed but never written back — which is
    // how a filter becomes reachable by clicking and unsayable in words.
    const written = new Set(
      SAYABLE.flatMap((patch) =>
        tokenize(format(filter(patch)))
          .filter((t) => t.kind === "pair")
          .map((t) => t.key),
      ),
    );
    expect([...written].sort()).toEqual(Object.keys(KEYS).sort());
  });
});

describe("tokenizing", () => {
  test("splits pairs from the text around them", () => {
    const tokens = tokenize("@tool:Bash rm -rf @outcome:refused");
    expect(tokens.map((t) => [t.kind, t.key, t.value])).toEqual([
      ["pair", "tool", "Bash"],
      ["text", "", "rm"],
      ["text", "", "-rf"],
      ["pair", "outcome", "refused"],
    ]);
  });

  test("a quoted value keeps its spaces (task 10.9)", () => {
    const [token] = tokenize('@project:"/Users/me/some project"');
    expect(token?.value).toBe("/Users/me/some project");
    expect(token?.quoted).toBe(true);
  });

  test("an unclosed quote is a value being typed, not a failure", () => {
    const [token] = tokenize('@project:"/Users/me/some pro');
    expect(token?.value).toBe("/Users/me/some pro");
  });

  test("a colon in the free text is text, which is why the sigil exists", () => {
    // Two thirds of the corpus is shell commands; `foo:bar` is ordinary there.
    const tokens = tokenize("git log --format=%H:%s");
    expect(tokens.every((t) => t.kind === "text")).toBe(true);
  });
});

describe("parsing", () => {
  test("narrows on the pairs and searches for the rest", () => {
    const { filter: f, errors } = parse("@tool:Bash @project:toolog @outcome:refused rm -rf");
    expect(errors).toEqual([]);
    expect(f.tool_name).toBe("Bash");
    expect(f.project_path).toBe("toolog");
    expect(f.decision).toBe("reject");
    expect(f.query).toBe("rm -rf");
  });

  test("an unknown key is an error that names the valid ones, and the rest still applies", () => {
    const { filter: f, errors } = parse("@nonsense:x @tool:Bash");
    expect(f.tool_name).toBe("Bash");
    expect(errors).toHaveLength(1);
    expect(errors[0]?.key).toBe("nonsense");
    expect(errors[0]?.message).toContain("@tool");
  });

  test("a half-typed key is not an error", () => {
    expect(parse("@proj").errors).toEqual([]);
    expect(parse("@project:").errors).toEqual([]);
    expect(parse("@").errors).toEqual([]);
  });

  test("a value the key does not take says what it does take", () => {
    const { errors } = parse("@outcome:maybe");
    expect(errors[0]?.message).toContain("ok, failed, refused");
  });

  test("@outcome and @decision write their own columns and leave each other alone", () => {
    // The dropdown this replaces cleared the other column on every change.
    // Beside a token the reader can see, that would be a silent deletion.
    const { filter: f } = parse("@outcome:failed @decision:accept");
    expect(f.success).toBe(false);
    expect(f.decision).toBe("accept");
  });

  test("the query bar does not touch the time bounds or the unattributed group", () => {
    // Task 10.7: it is a second editor of the filter, not a second filter.
    const f = filter({ since: 1_000, until: 2_000, session_unknown: true, tool_name: "Bash" });
    expect(format(f)).toBe("@tool:Bash");
  });
});

describe("a filter reachable by link is sayable in the box (task 10.7)", () => {
  test("everything a v1.0 hash restores comes back out as text", () => {
    // The failure this guards against is a filter you can reach with a link
    // and then cannot see, edit or clear — the box would silently disagree
    // with the list it sits above.
    const restored = fromHash(
      "#q=rm+-rf&project=%2Fwork%2Fapp&tool=Bash&session=s-1&agent=a-9" +
        "&since=1700000000000&until=1700000600000&source=user_reject&mode=acceptEdits" +
        "&decision=reject&lane=3&thread=false&ok=false&sidechain=true",
    ).filter;

    const said = parse(format(restored)).filter;
    // Time is the one thing the box does not write, so it is the one thing
    // that does not come back — the control and the histogram own it.
    expect(said).toEqual({ ...restored, since: null, until: null });
  });
});

describe("risk as a filter (tasks 12.9, 12.10)", () => {
  test("@risk narrows to a severity and @rule to one rule", () => {
    const { filter: f, errors } = parse("@risk:high @rule:secrets-read rm");
    expect(errors).toEqual([]);
    expect(f.risk).toBe("high");
    expect(f.rule_id).toBe("secrets-read");
    expect(f.query).toBe("rm");
  });

  test("a severity the rules do not have is an error naming the four", () => {
    const { errors } = parse("@risk:critical");
    expect(errors[0]?.message).toContain("high, medium, low, info");
  });
});
