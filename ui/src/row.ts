//! One line of the timeline, and the headers that group them.
//!
//! Task 5.4's row: time, project, tool, outcome, what it did, how long it took,
//! and who let it. `Bash` is 71% of the corpus here, so the row is laid out for
//! a shell command first — a monospace column that gets all the space left over
//! once the fixed columns have taken theirs.
//!
//! Task 5.5 is the other half of the job. The two lanes arrive separately, so a
//! row is routinely half-populated: no duration until OTEL sees it, no decision
//! for anything imported from history, no outcome at all while a call is still
//! running. Every one of those renders as a stated absence — never a blank row,
//! never a zero, and never a row that disappears until it is complete.

import type { AgentGroup, SessionGroup, TimelineRow } from "./bindings";
import { el, orDash, span } from "./dom";
import { basename, clock, count, duration, EM_DASH, fullStamp, lanes } from "./format";
import type { RowBlock } from "./plan";

/** The delimiters `snippet()` puts around a match — see `query::MATCH_OPEN`. */
const MATCH_OPEN = "\u0001";
const MATCH_CLOSE = "\u0002";

export interface RowContext {
  /** The terms typed into the search box, for highlighting. */
  terms: string[];
  selected: string | null;
}

/** The outcome glyph, and what it means when there is nothing to show. */
function outcome(row: TimelineRow): HTMLElement {
  const call = row.call;
  if (call.decision === "reject") {
    return span("st ref", "⊘", `Refused${call.decision_source ? ` — ${call.decision_source}` : ""}`);
  }
  if (call.success === true) return span("st ok", "✓", "Succeeded");
  if (call.success === false) {
    return span("st no", "✕", orDash(call.error_type, "Failed"));
  }
  // Neither lane has reported a result. Either the call is in flight or the
  // transcript line that carries the result has not been written yet.
  return span("st pending", "·", "No result recorded yet");
}

/** The tool badge. MCP tools are named `mcp__Server__tool`, which will not fit. */
function toolBadge(row: TimelineRow): HTMLElement {
  const call = row.call;
  const label = call.mcp_tool ?? call.tool_name ?? "?";
  const badge = span("tool", label, call.tool_name ?? "");
  if (call.tool_kind && call.tool_kind !== "builtin") badge.dataset["kind"] = call.tool_kind;
  return badge;
}

/** Case-insensitive literal highlighting of the search terms in a line. */
function highlight(text: string, terms: string[]): DocumentFragment {
  const out = document.createDocumentFragment();
  if (terms.length === 0) {
    out.append(text);
    return out;
  }
  const lower = text.toLowerCase();
  let cursor = 0;
  while (cursor < text.length) {
    let at = -1;
    let hit = "";
    for (const term of terms) {
      const found = lower.indexOf(term, cursor);
      if (found !== -1 && (at === -1 || found < at)) {
        at = found;
        hit = term;
      }
    }
    if (at === -1) break;
    out.append(text.slice(cursor, at));
    out.append(el("mark", { text: text.slice(at, at + hit.length) }));
    cursor = at + hit.length;
  }
  out.append(text.slice(cursor));
  return out;
}

/** An FTS snippet, with its control-character delimiters turned into marks. */
function fromSnippet(snippet: string): DocumentFragment {
  const out = document.createDocumentFragment();
  for (const [i, part] of snippet.split(MATCH_OPEN).entries()) {
    if (i === 0) {
      out.append(part);
      continue;
    }
    const [marked, rest] = part.split(MATCH_CLOSE, 2);
    out.append(el("mark", { text: marked ?? "" }));
    out.append(rest ?? "");
  }
  return out;
}

/** The display line: what the call was asked to do. */
function summary(row: TimelineRow, ctx: RowContext): HTMLElement {
  const call = row.call;
  const cell = el("span", { class: "sum" });
  const text = call.input_summary ?? call.target_path ?? "";

  if (text === "") {
    // A call whose input was never recorded by either lane. Rare, and worth
    // saying rather than leaving the widest column of the row empty.
    cell.append(span("none", "no input recorded"));
  } else {
    cell.append(highlight(text, ctx.terms));
  }

  if (row.lines_added !== null || row.lines_removed !== null) {
    cell.append(
      " ",
      span("diffsize", `+${row.lines_added ?? 0} −${row.lines_removed ?? 0}`),
    );
  }

  // The match may have been in the result text, which this row never shows.
  // Saying where it was is the difference between a hit and a mystery.
  if (row.snippet !== null && !ctx.terms.some((t) => text.toLowerCase().includes(t))) {
    const found = el("span", { class: "insnip", title: "matched in the result" });
    found.append(fromSnippet(row.snippet));
    cell.append(" ", found);
  }
  return cell;
}

/** One timeline row. */
export function renderRow(row: TimelineRow, block: RowBlock, ctx: RowContext): HTMLElement {
  const call = row.call;
  const node = el("div", {
    class: "row",
    role: "option",
    attrs: {
      "data-id": call.tool_use_id,
      "aria-selected": String(ctx.selected === call.tool_use_id),
    },
  });
  if (ctx.selected === call.tool_use_id) node.classList.add("sel");
  if (block.indent > 0) node.classList.add("sub");
  if (call.decision === "reject") node.classList.add("refused");

  node.append(
    span("time", clock(call.called_at), fullStamp(call.called_at)),
    span("project", basename(row.project_path), row.project_path ?? "no project recorded"),
    toolBadge(row),
    outcome(row),
    summary(row, ctx),
    span(
      call.duration_ms === null ? "dur none" : "dur",
      duration(call.duration_ms),
      call.duration_ms === null ? "No duration — the OTLP lane did not see this call" : "",
    ),
    span(
      call.decision_source === null ? "dec none" : `dec ${call.decision === "reject" ? "user" : ""}`,
      orDash(call.decision_source),
      call.decision_source === null
        ? "No decision recorded — only the OTLP lane carries one"
        : `${call.decision ?? "decided"} · ${call.decision_source} · ${lanes(call.provenance)}`,
    ),
  );
  return node;
}

/** A row whose page has not arrived yet. Holds the height; claims nothing. */
export function renderPending(block: RowBlock, error?: string): HTMLElement {
  const node = el("div", { class: error === undefined ? "row pending" : "row broken" });
  if (block.indent > 0) node.classList.add("sub");
  node.append(
    span("time", ""),
    span("project", ""),
    span("tool skeleton", ""),
    span("st", ""),
    error === undefined
      ? span("sum skeleton", "")
      : span("sum none", `could not load these rows — ${error}. Click to try again.`),
    span("dur", ""),
    span("dec", ""),
  );
  return node;
}

/** The header over one session's calls (task 5.10). */
export function renderSessionHeader(group: SessionGroup, collapsed: boolean): HTMLElement {
  const node = el("div", {
    class: "ghead",
    role: "option",
    attrs: { "aria-expanded": String(!collapsed) },
  });

  const parts: string[] = [];
  if (group.git_branch) parts.push(group.git_branch);
  parts.push(`${count(group.calls)} ${group.calls === 1 ? "call" : "calls"}`);
  if (group.failures > 0) parts.push(`${count(group.failures)} failed`);
  if (group.refusals > 0) parts.push(`${count(group.refusals)} refused`);
  if (group.first_at !== null && group.last_at !== null) {
    parts.push(`${clock(group.first_at)}–${clock(group.last_at)}`);
    parts.push(duration(group.last_at - group.first_at));
  }

  node.append(
    span("caret", collapsed ? "›" : "‹"),
    span(
      "gname",
      group.project_path === null ? "Unattributed calls" : basename(group.project_path),
      group.project_path ?? "These calls name no session the store ever learned",
    ),
    span("gmeta", parts.join(" · ")),
  );
  return node;
}

/** The header over one subagent's calls, nested inside its session. */
export function renderAgentHeader(
  _group: SessionGroup,
  agent: AgentGroup,
  collapsed: boolean,
): HTMLElement {
  const node = el("div", {
    class: "ghead agent",
    role: "option",
    attrs: { "aria-expanded": String(!collapsed) },
  });
  const parts = [
    "subagent",
    `${count(agent.calls)} ${agent.calls === 1 ? "call" : "calls"}`,
    agent.first_at === null ? EM_DASH : `${clock(agent.first_at)}–${clock(agent.last_at)}`,
  ];
  node.append(
    span("caret", collapsed ? "›" : "‹"),
    span("gname", agent.agent_name ?? "Unnamed agent", agent.agent_id),
    span("gmeta", parts.join(" · ")),
  );
  return node;
}
