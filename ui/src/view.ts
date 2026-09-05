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
  /** Group rows by session and subagent (task 5.10). */
  grouped: boolean;
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
  };
}

export function emptyView(): ViewState {
  return { filter: emptyFilter(), grouped: false, selected: null };
}

/** Whether anything is narrowing the timeline. */
export function isFiltered(f: TimelineFilter): boolean {
  return Object.values(f).some((v) => v !== null);
}

/** The status control collapses three different columns into one choice. */
export type Outcome = "any" | "ok" | "failed" | "refused";

export function outcomeOf(f: TimelineFilter): Outcome {
  if (f.decision === "reject") return "refused";
  if (f.success === true) return "ok";
  if (f.success === false) return "failed";
  return "any";
}

export function withOutcome(f: TimelineFilter, outcome: Outcome): TimelineFilter {
  const next: TimelineFilter = { ...f, success: null, decision: null };
  if (outcome === "ok") next.success = true;
  if (outcome === "failed") next.success = false;
  if (outcome === "refused") next.decision = "reject";
  return next;
}

/** The lane control, over `provenance`'s two bits. */
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
  if (view.grouped) params.set("group", "session");
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

  view.grouped = params.get("group") === "session";
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
