//! What the timeline is currently showing, and how that survives a reload.
//!
//! Task 5.6 asks for filter state in the URL hash so a view is shareable and
//! restorable. Two consequences follow from "restorable" that are easy to get
//! wrong:
//!
//! - **Time ranges are absolute.** A preset writes the instant it resolved to,
//!   not "24h". A link that means something different tomorrow is not a link to
//!   a view, and this is evidence.
//! - **Only what differs from the default is written.** An empty timeline has
//!   an empty hash, so the address bar reads as a description of the filter
//!   rather than as a form dump.

import type { TimelineFilter } from "./bindings";

export interface ViewState {
  filter: TimelineFilter;
  /** The `tool_use_id` whose detail pane is open. */
  selected: string | null;
}

export function emptyFilter(): TimelineFilter {
  return {
    session_id: null,
    session_unknown: null,
    project_path: null,
    tool_name: null,
    since: null,
    until: null,
    success: null,
    is_sidechain: null,
    decision: null,
    decision_source: null,
    permission_mode: null,
    agent_id: null,
    main_thread: null,
    query: null,
    provenance: null,
    risk: null,
    rule_id: null,
  };
}

export function emptyView(): ViewState {
  return { filter: emptyFilter(), selected: null };
}

/** Whether anything is narrowing the timeline. */
export function isFiltered(f: TimelineFilter): boolean {
  return Object.values(f).some((v) => v !== null);
}

/**
 * The lane control, over `provenance`'s two bits.
 *
 * The outcome collapse that used to sit above this went with the dropdown it
 * served (task 10.11). `@outcome` writes `success` and `decision` directly in
 * `query.ts`, and deliberately does not clear the one it did not name — see the
 * note there.
 */
export type Lane = "any" | "both" | "transcript" | "otel";

const LANE_BITS: Record<Exclude<Lane, "any">, number> = { both: 3, transcript: 1, otel: 2 };

export function laneOf(f: TimelineFilter): Lane {
  for (const [name, bits] of Object.entries(LANE_BITS)) {
    if (f.provenance === bits) return name as Lane;
  }
  return "any";
}

export function withLane(f: TimelineFilter, lane: Lane): TimelineFilter {
  return { ...f, provenance: lane === "any" ? null : LANE_BITS[lane] };
}

/** The thread control, over `main_thread`. */
export type Thread = "any" | "main" | "sub";

export function threadOf(f: TimelineFilter): Thread {
  if (f.main_thread === true) return "main";
  if (f.main_thread === false) return "sub";
  return "any";
}

export function withThread(f: TimelineFilter, thread: Thread): TimelineFilter {
  return { ...f, main_thread: thread === "any" ? null : thread === "main" };
}

// ---------------------------------------------------------------------------
// The hash
// ---------------------------------------------------------------------------

/** Short names, because the hash is meant to be read as well as parsed. */
const KEYS: Record<string, keyof TimelineFilter> = {
  q: "query",
  project: "project_path",
  tool: "tool_name",
  session: "session_id",
  agent: "agent_id",
  since: "since",
  until: "until",
  source: "decision_source",
  mode: "permission_mode",
  decision: "decision",
  lane: "provenance",
  thread: "main_thread",
  ok: "success",
  sidechain: "is_sidechain",
  nosession: "session_unknown",
  risk: "risk",
  rule: "rule_id",
};

const NUMERIC = new Set<keyof TimelineFilter>(["since", "until", "provenance"]);
const BOOLEAN = new Set<keyof TimelineFilter>([
  "success",
  "is_sidechain",
  "main_thread",
  "session_unknown",
]);

export function toHash(view: ViewState): string {
  const params = new URLSearchParams();
  for (const [short, key] of Object.entries(KEYS)) {
    const value = view.filter[key];
    if (value === null || value === undefined) continue;
    params.set(short, String(value));
  }
  if (view.selected !== null) params.set("call", view.selected);
  const query = params.toString();
  return query === "" ? "" : `#${query}`;
}

export function fromHash(hash: string): ViewState {
  const params = new URLSearchParams(hash.replace(/^#/, ""));
  const view = emptyView();

  for (const [short, key] of Object.entries(KEYS)) {
    const raw = params.get(short);
    if (raw === null) continue;
    if (NUMERIC.has(key)) {
      const n = Number(raw);
      if (Number.isFinite(n)) (view.filter[key] as number | null) = n;
    } else if (BOOLEAN.has(key)) {
      (view.filter[key] as boolean | null) = raw === "true" || raw === "1";
    } else {
      (view.filter[key] as string | null) = raw;
    }
  }

  // `group=session` was written by v1.0 and v1.1. Grouping is gone, so the key
  // is read and dropped rather than rejected: an old link still restores the
  // filter it carried, which is the part of it that still means something.
  view.selected = params.get("call");
  return view;
}

/** Whether two filters would produce the same list. */
export function sameFilter(a: TimelineFilter, b: TimelineFilter): boolean {
  return (Object.keys(a) as (keyof TimelineFilter)[]).every((k) => a[k] === b[k]);
}

// ---------------------------------------------------------------------------
// Time presets
// ---------------------------------------------------------------------------

export interface Preset {
  label: string;
  /** Milliseconds back from now, or `null` for "all of it". */
  ago: number | null;
}

export const PRESETS: Preset[] = [
  { label: "Any time", ago: null },
  { label: "Last hour", ago: 3_600_000 },
  { label: "Today", ago: 86_400_000 },
  { label: "Last 7 days", ago: 7 * 86_400_000 },
  { label: "Last 30 days", ago: 30 * 86_400_000 },
];
