//! The detail pane: everything the store holds about one call (task 5.8).
//!
//! The row answers "what ran". This answers "under what circumstances, and with
//! what result" — the full input, the untruncated result, the envelope the call
//! ran inside, and which lanes witnessed it. Provenance is shown at the top
//! rather than buried, because a call only one lane saw is a different kind of
//! evidence from one both lanes agree on, and the reader should not have to
//! work that out.

import type { FileChange, SourceView, TimelineFilter, ToolCallDetail } from "./bindings";
import { getSource, getToolCall, revealTranscript } from "./bindings";
import { append, el, fill, orDash, span } from "./dom";
import { bytes, count, duration, EM_DASH, fullStamp, isByReference, lanes, shortPath } from "./format";
import { renderDiff } from "./diff";

/** How much of a result is shown before the pane offers the rest. */
const RESULT_PREVIEW = 6_000;

function heading(text: string): HTMLElement {
  return el("h3", { text });
}

function facts(rows: [string, string | null, string?][]): HTMLElement {
  const list = el("dl", { class: "kv" });
  for (const [label, value, title] of rows) {
    if (value === null) continue;
    list.append(
      el("dt", { text: label }),
      el("dd", title === undefined ? { text: value } : { text: value, title }),
    );
  }
  return list;
}

function pretty(json: string): string {
  try {
    return JSON.stringify(JSON.parse(json), null, 2);
  } catch {
    return json;
  }
}

/** A long block of text with the tail behind one click. */
function well(text: string, className = "well"): HTMLElement {
  const box = el("pre", { class: className });
  if (text.length <= RESULT_PREVIEW) {
    box.textContent = text;
    return box;
  }
  box.textContent = text.slice(0, RESULT_PREVIEW);
  const more = el("button", {
    class: "link",
    text: `Show the remaining ${count(text.length - RESULT_PREVIEW)} characters`,
  });
  more.addEventListener("click", () => {
    box.textContent = text;
    more.remove();
  });
  const wrap = el("div");
  wrap.append(box, more);
  return wrap;
}

function diffs(changes: FileChange[]): HTMLElement | null {
  if (changes.length === 0) return null;
  const added = changes.reduce((n, c) => n + c.lines_added, 0);
  const removed = changes.reduce((n, c) => n + c.lines_removed, 0);
  const box = el("div");
  box.append(
    heading(
      `Diff — ${count(changes.length)} ${changes.length === 1 ? "file" : "files"}, ` +
        `+${count(added)} −${count(removed)}`,
    ),
  );
  for (const change of changes) box.append(renderDiff(change));
  return box;
}

/** The "open the transcript" section (task 5.11), fetched on request. */
function sourceSection(toolUseId: string): HTMLElement {
  const box = el("div");
  box.append(heading("Source"));

  const body = el("div", { class: "source" });
  const find = el("button", { class: "link", text: "Find the transcript record" });

  find.addEventListener("click", () => {
    find.disabled = true;
    find.textContent = "Looking…";
    void getSource(toolUseId)
      .then((source: SourceView | null) => {
        find.remove();
        if (source === null) {
          fill(body, [
            span(
              "none",
              "No stored transcript line mentions this call. Only the OTLP lane witnessed it.",
            ),
          ]);
          return;
        }
        const at = source.line === null ? source.path : `${source.path}:${source.line}`;
        const open = el("button", {
          class: "link",
          text: source.exists ? "Reveal in Finder" : "The file is no longer on disk",
          disabled: !source.exists,
        });
        open.addEventListener("click", () => {
          void revealTranscript(source.path).catch((e: unknown) => {
            open.textContent = String(e);
          });
        });
        fill(body, [
          el("div", { class: "mono wrap", text: at, title: at }),
          well(pretty(source.body), "well small"),
          el("div", { class: "note" }, [
            "The stored line is the evidence; the file is a convenience. ",
            open,
          ]),
        ]);
      })
      .catch((error: unknown) => {
        find.disabled = false;
        find.textContent = "Find the transcript record";
        fill(body, [span("none", String(error))]);
      });
  });

  body.append(find);
  box.append(body);
  return box;
}

/** Narrow the timeline to something this call belongs to (task 5.6). */
function narrowTo(label: string, patch: Partial<TimelineFilter>, onFilter: Narrow): HTMLElement {
  const button = el("button", { class: "link", text: label });
  button.addEventListener("click", () => onFilter(patch));
  return button;
}

function render(detail: ToolCallDetail, onFilter: Narrow): HTMLElement {
  const { call, session } = detail;
  const box = el("div", { class: "detail" });

  const chips = el("span", { class: "chips" });
  if (call.decision === "reject") chips.append(span("chip ref", "refused"));
  else if (call.success === true) chips.append(span("chip ok", "ok"));
  else if (call.success === false) chips.append(span("chip no", orDash(call.error_type, "failed")));
  else chips.append(span("chip pending", "no result recorded"));
  chips.append(span("chip lane", lanes(call.provenance)));
  if (call.agent_name !== null || call.agent_id !== null) {
    chips.append(span("chip agent", call.agent_name ?? "subagent", call.agent_id ?? ""));
  }

  box.append(el("h2", {}, [span("dtool", call.tool_name ?? "Unknown tool"), chips]));

  const target = call.target_path ?? call.input_summary;
  if (target !== null) {
    box.append(el("div", { class: "mono wrap dtarget", text: target, title: target }));
  }

  box.append(
    heading("Call"),
    facts([
      ["Started", fullStamp(call.called_at)],
      ["Finished", call.completed_at === null ? EM_DASH : fullStamp(call.completed_at)],
      [
        "Duration",
        call.duration_ms === null ? "not recorded" : duration(call.duration_ms),
        call.duration_ms === null ? "Only the OTLP lane measures durations" : "",
      ],
      [
        "Decision",
        call.decision === null
          ? "not recorded"
          : `${call.decision}${call.decision_source === null ? "" : ` · ${call.decision_source}`}`,
        call.decision === null ? "Only the OTLP lane carries a decision" : "",
      ],
      ["Mode", orDash(call.permission_mode, "not recorded")],
      ["Result size", call.result_size === null ? EM_DASH : bytes(call.result_size)],
      ["Tool use id", call.tool_use_id],
    ]),
  );

  box.append(
    heading("Session"),
    session === null
      ? el("div", {
          class: "none",
          text: "The store never learned which session this call belonged to.",
        })
      : facts([
          ["Project", orDash(session.project_path), session.project_path ?? ""],
          ["Branch", orDash(session.git_branch)],
          ["Directory", session.cwd === null ? EM_DASH : shortPath(session.cwd, 3), session.cwd ?? ""],
          ["Claude Code", orDash(session.cc_version)],
          ["Entrypoint", orDash(session.entrypoint)],
          ["Session", orDash(session.slug ?? session.session_id), session.session_id],
        ]),
  );

  if (session !== null) {
    const narrow = el("div", { class: "narrow" });
    narrow.append(
      narrowTo("Only this session", { session_id: session.session_id }, onFilter),
      span("dot", "·"),
      narrowTo("Only this project", { project_path: session.project_path }, onFilter),
    );
    if (call.agent_id !== null) {
      narrow.append(
        span("dot", "·"),
        narrowTo("Only this subagent", { agent_id: call.agent_id }, onFilter),
      );
    }
    box.append(narrow);
  }

  const diff = diffs(detail.file_changes);
  if (diff !== null) box.append(diff);

  if (call.input_json !== null) {
    box.append(heading("Input"), well(pretty(call.input_json)));
  }

  if (call.result_json !== null && isByReference(call.result_json)) {
    // Task 7.5: the projection stopped keeping a second copy of a body this
    // large. Say where it is rather than showing the marker.
    box.append(
      heading("Result"),
      el("p", { class: "note" }, [
        `This result was ${bytes(call.result_size)} — too large to keep a second copy of. `,
        "The record is in the evidence store exactly as it arrived; open the source below to read it.",
      ]),
    );
    if (call.result_text !== null && call.result_text !== "") {
      box.append(heading("Result text"), well(call.result_text));
    }
  } else if (call.result_text !== null && call.result_text !== "") {
    box.append(heading("Result"), well(call.result_text));
  } else if (call.result_json !== null) {
    box.append(heading("Result"), well(pretty(call.result_json)));
  } else {
    box.append(
      heading("Result"),
      el("div", {
        class: "none",
        text:
          call.decision === "reject"
            ? "The call was refused, so it produced no result."
            : "No result has been recorded for this call.",
      }),
    );
  }

  box.append(sourceSection(call.tool_use_id));
  return box;
}

/** Narrowing the timeline from inside the pane. */
export type Narrow = (patch: Partial<TimelineFilter>) => void;

/** The pane: shows one call at a time, and says when it is showing nothing. */
export class DetailPane {
  private token = 0;
  /** The content, under a header that does not get replaced on every load. */
  private readonly body = el("div", { class: "pane-body" });

  constructor(
    private readonly host: HTMLElement,
    private readonly onFilter: Narrow,
    /**
     * The reader closed the pane (task 10.12).
     *
     * A button, because `Escape` was the only way out in v1.0 and it only
     * worked while focus was in the list — so a pane opened by clicking a row
     * and then scrolled had no visible exit at all. In a narrow window it
     * covers the list it was opened from, which makes that a trap rather than
     * an inconvenience.
     */
    onClose: () => void,
  ) {
    const close = el("button", {
      class: "pane-close",
      text: "✕",
      attrs: { type: "button", "aria-label": "Close the detail pane", title: "Close (Esc)" },
    });
    close.addEventListener("click", onClose);
    this.host.append(el("div", { class: "pane-head" }, [close]), this.body);

    // `Escape` from anywhere inside the pane, not only from the list. The
    // handler sits on the pane so a keystroke while reading a result body
    // reaches it.
    this.host.addEventListener("keydown", (event) => {
      if (event.key !== "Escape") return;
      event.stopPropagation();
      onClose();
    });
    this.clear();
  }

  /** Empty the pane. It is hidden whenever nothing is selected, so this is
   * about not holding the last call's result in a hidden subtree rather than
   * about what is shown. */
  clear(): void {
    this.token += 1;
    fill(this.body, []);
  }

  show(toolUseId: string): void {
    this.token += 1;
    const token = this.token;
    fill(this.body, [el("div", { class: "empty small", text: "Loading…" })]);

    void getToolCall(toolUseId)
      .then((detail) => {
        if (token !== this.token) return;
        if (detail === null) {
          fill(this.body, [
            el("div", { class: "empty small", text: "That call is no longer in the store." }),
          ]);
          return;
        }
        fill(this.body, [render(detail, this.onFilter)]);
        this.body.scrollTop = 0;
      })
      .catch((error: unknown) => {
        if (token !== this.token) return;
        fill(this.body, []);
        append(this.body, [el("div", { class: "problem", text: String(error) })]);
      });
  }
}
