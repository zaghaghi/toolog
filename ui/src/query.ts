//! The query bar's language: `@key:value` pairs, and everything else is search.
//!
//! One box replaces seven dropdowns (task 10.5). The shape is Datadog's and
//! GitHub's, and the sigil is not decoration: `crates/toolog-core/src/fts.rs`
//! opens by saying the corpus is two-thirds shell commands, in which `foo:bar`
//! is an ordinary thing to search for. A bare `key:value` would have been
//! ambiguous with the search itself; `@key:value` cannot be.
//!
//! **The filter stays the source of truth** (task 10.7). This is a second
//! *editor* of `TimelineFilter`, not a second representation of it — the hash
//! keeps its v1.0 encoding, an export still exports the filter, and a link
//! written before this phase still restores. So the two functions here are a
//! round trip over the filter, not over the text: `parse(format(f))` is `f`,
//! and a test asserts it for every field.

import type { TimelineFilter } from "./bindings";
import { emptyFilter, withLane, withThread } from "./view";
import type { Lane, Thread } from "./view";

/** What a key accepts, which is what autocomplete offers after the colon. */
export type Values = "text" | "outcome" | "lane" | "thread" | "boolean";

export interface KeySpec {
  /** The field this key edits, for the ones that are a plain assignment. */
  field?: keyof TimelineFilter;
  values: Values;
  /** The line under the key in the autocomplete list. */
  hint: string;
}

/**
 * Every key, and nothing else is one.
 *
 * Time is deliberately absent: it stays a control, because it pairs with the
 * histogram below the box and a brush across that chart is a far better way to
 * say "this hour" than typing two timestamps (task 10.6).
 */
export const KEYS: Record<string, KeySpec> = {
  project: { field: "project_path", values: "text", hint: "Project path" },
  tool: { field: "tool_name", values: "text", hint: "Tool name" },
  session: { field: "session_id", values: "text", hint: "Session id" },
  agent: { field: "agent_id", values: "text", hint: "Subagent instance" },
  source: { field: "decision_source", values: "text", hint: "What made the decision" },
  mode: { field: "permission_mode", values: "text", hint: "Permission mode" },
  decision: { field: "decision", values: "text", hint: "accept or reject" },
  outcome: { values: "outcome", hint: "ok, failed or refused" },
  lane: { values: "lane", hint: "both, transcript or otel" },
  thread: { values: "thread", hint: "main or sub" },
  sidechain: { field: "is_sidechain", values: "boolean", hint: "true or false" },
};

const OUTCOMES: string[] = ["ok", "failed", "refused"];
const LANES: Lane[] = ["both", "transcript", "otel"];
const THREADS: Thread[] = ["main", "sub"];

/** The values a key offers when the store cannot supply them. */
export function fixedValues(key: string): string[] {
  switch (KEYS[key]?.values) {
    case "outcome":
      return OUTCOMES;
    case "lane":
      return LANES;
    case "thread":
      return THREADS;
    case "boolean":
      return ["true", "false"];
    default:
      return [];
  }
}

export interface ParseError {
  /** The key as typed, without its sigil. */
  key: string;
  message: string;
}

export interface Parsed {
  filter: TimelineFilter;
  errors: ParseError[];
}

// ---------------------------------------------------------------------------
// Tokenizing
// ---------------------------------------------------------------------------

export interface Token {
  /** `@key:value`, or a bare run of text. */
  kind: "pair" | "text";
  key: string;
  value: string;
  /** Whether the value was written inside quotes. */
  quoted: boolean;
  /** Where the token starts and ends in the source, for autocomplete. */
  start: number;
  end: number;
}

/**
 * Split a query into pairs and free text.
 *
 * Hand-written rather than a regex because of quoting: a project path is a real
 * path and `@project:"/Users/me/some project"` has to survive as one value,
 * spaces included (task 10.9).
 */
export function tokenize(text: string): Token[] {
  const tokens: Token[] = [];
  let i = 0;

  while (i < text.length) {
    if (/\s/.test(text[i]!)) {
      i += 1;
      continue;
    }
    const start = i;

    if (text[i] === "@") {
      const colon = text.indexOf(":", i);
      const wordEnd = nextSpace(text, i);
      // `@tool` with no colon yet is a key half-typed, not a search for "@tool".
      if (colon === -1 || colon > wordEnd) {
        tokens.push({
          kind: "pair",
          key: text.slice(i + 1, wordEnd),
          value: "",
          quoted: false,
          start,
          end: wordEnd,
        });
        i = wordEnd;
        continue;
      }
      const key = text.slice(i + 1, colon);
      const [value, end, quoted] = readValue(text, colon + 1);
      tokens.push({ kind: "pair", key, value, quoted, start, end });
      i = end;
      continue;
    }

    // Quoted free text, so a search for `@reboot` or a phrase is sayable at
    // all: bare `@…` is a key by construction, and this is the way out of it.
    const [value, end, quoted] = readValue(text, i);
    tokens.push({ kind: "text", key: "", value, quoted, start, end });
    i = end;
  }

  return tokens;
}

function nextSpace(text: string, from: number): number {
  const at = text.slice(from).search(/\s/);
  return at === -1 ? text.length : from + at;
}

/**
 * A quoted value, or everything up to the next space.
 *
 * Facet values are real paths and real MCP tool names, so quoting is needed on
 * day one (task 10.9) — and `\"` inside the quotes, because a path may contain
 * one and dropping it silently would mean the box could not express a filter
 * the store holds.
 */
function readValue(text: string, from: number): [string, number, boolean] {
  if (text[from] !== '"') {
    const end = nextSpace(text, from);
    return [text.slice(from, end), end, false];
  }
  let value = "";
  let i = from + 1;
  while (i < text.length) {
    if (text[i] === "\\" && i + 1 < text.length) {
      value += text[i + 1];
      i += 2;
      continue;
    }
    if (text[i] === '"') return [value, i + 1, true];
    value += text[i];
    i += 1;
  }
  // An unclosed quote is a value still being typed, not an error: the rest of
  // the line is the value so far.
  return [value, text.length, true];
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/**
 * A query as a filter, plus whatever could not be understood.
 *
 * An unrecognised key is reported and skipped; the rest of the query still
 * applies (task 10.10). A half-typed `@too` is not an error, because the reader
 * is mid-word and being told off for it is noise.
 */
export function parse(text: string): Parsed {
  const filter = emptyFilter();
  const errors: ParseError[] = [];
  const terms: string[] = [];

  const tokens = tokenize(text);
  for (const [i, token] of tokens.entries()) {
    if (token.kind === "text") {
      terms.push(token.value);
      continue;
    }
    // The last token, still being typed, is not yet a claim about anything.
    const typing = i === tokens.length - 1 && token.end === text.length;
    const spec = KEYS[token.key];
    if (spec === undefined) {
      if (!(typing && token.value === "")) {
        errors.push({
          key: token.key,
          message: `No filter called @${token.key}. Try ${Object.keys(KEYS)
            .map((k) => `@${k}`)
            .join(", ")}.`,
        });
      }
      continue;
    }
    // An empty value is a key with nothing after its colon yet — except when
    // it was quoted, which is how an empty string is actually said.
    if (token.value === "" && !token.quoted) continue;
    apply(filter, token.key, token.value, errors);
  }

  filter.query = terms.length === 0 ? null : terms.join(" ");
  return { filter, errors };
}

/** Write one pair into the filter, in place. */
function apply(
  filter: TimelineFilter,
  key: string,
  value: string,
  errors: ParseError[],
): void {
  const spec = KEYS[key]!;

  if (spec.field !== undefined) {
    if (spec.values === "boolean") {
      const bool = asBoolean(value);
      if (bool === null) {
        errors.push({ key, message: `@${key} takes true or false, not "${value}".` });
        return;
      }
      (filter[spec.field] as boolean | null) = bool;
      return;
    }
    (filter[spec.field] as string | null) = value;
    return;
  }

  // The three controls that are a word over a column rather than a column.
  const known = fixedValues(key);
  if (!known.includes(value)) {
    errors.push({ key, message: `@${key} takes ${known.join(", ")} — not "${value}".` });
    return;
  }
  if (spec.values === "lane") {
    Object.assign(filter, withLane(filter, value as Lane));
    return;
  }
  if (spec.values === "thread") {
    Object.assign(filter, withThread(filter, value as Thread));
    return;
  }

  // `@outcome` writes only the column its own word names — unlike the dropdown
  // it replaces, which cleared the other one on every change. A dropdown *is*
  // the whole control, so clearing was invisible; a token sitting beside
  // `@decision:accept` in the same box is not, and silently deleting the
  // neighbour the reader can see would be a surprise. It also makes the round
  // trip total: every (success, decision) pair the store can hold is sayable.
  if (value === "refused") filter.decision = "reject";
  else filter.success = value === "ok";
}

function asBoolean(value: string): boolean | null {
  if (value === "true") return true;
  if (value === "false") return false;
  return null;
}

// ---------------------------------------------------------------------------
// Formatting
// ---------------------------------------------------------------------------

/**
 * Quote a value that would not survive being read back unquoted.
 *
 * Whitespace and a quote character are the obvious cases. A leading `@` is the
 * third: unquoted it would parse back as a key, so a search for `@reboot` has
 * to come out of [`format`] already quoted or the round trip is not one.
 */
export function quote(value: string): string {
  // A sigil only makes a key when it leads, so only a leading one forces quotes.
  if (value !== "" && !/[\s"]/.test(value) && !value.startsWith("@")) return value;
  return `"${value.replace(/\\/g, "\\\\").replace(/"/g, '\\"')}"`;
}

/**
 * The filter as a query string — the inverse of [`parse`].
 *
 * `since`, `until` and `session_unknown` are deliberately not written: the
 * first two are the time control's, and the third is reached by clicking the
 * "Unattributed calls" group rather than by typing. Anything this omits stays
 * in the filter untouched, because the filter is what the list reads.
 */
export function format(filter: TimelineFilter): string {
  const parts: string[] = [];

  for (const [key, spec] of Object.entries(KEYS)) {
    if (spec.field === undefined) continue;
    // `@outcome:refused` already says this, and saying it twice would put a
    // token in the box the reader never typed.
    if (key === "decision" && filter.decision === "reject") continue;
    const value = filter[spec.field];
    if (value === null || value === undefined) continue;
    parts.push(`@${key}:${quote(String(value))}`);
  }

  if (filter.decision === "reject") parts.push("@outcome:refused");
  if (filter.success !== null) parts.push(`@outcome:${filter.success ? "ok" : "failed"}`);
  const lane = laneIn(filter);
  if (lane !== null) parts.push(`@lane:${lane}`);
  const thread = threadIn(filter);
  if (thread !== null) parts.push(`@thread:${thread}`);

  if (filter.query !== null && filter.query !== "") parts.push(quote(filter.query));
  return parts.join(" ");
}

/**
 * The word controls, read back out of the columns they wrote.
 *
 * Not `view.ts`'s `laneOf`/`threadOf`, which answer "any" for a filter that has
 * neither set. Here the difference between "not set" and "set to the everything
 * value" matters: writing `@lane:any` into the box would put a word there that
 * the reader did not type.
 */
const LANE_BITS: Record<Exclude<Lane, "any">, number> = { both: 3, transcript: 1, otel: 2 };

function laneIn(f: TimelineFilter): Lane | null {
  const found = Object.entries(LANE_BITS).find(([, bits]) => f.provenance === bits);
  return found === undefined ? null : (found[0] as Lane);
}

function threadIn(f: TimelineFilter): Thread | null {
  if (f.main_thread === true) return "main";
  if (f.main_thread === false) return "sub";
  return null;
}
