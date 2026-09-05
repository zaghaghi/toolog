//! Rendering `structuredPatch` as a diff (task 5.9).
//!
//! 366 rows in the owner's store carry one, and it is the most information-dense
//! thing this application can show: the exact lines an agent changed in a file,
//! months after the terminal that showed them has gone. Rendering it as raw JSON
//! would be a waste of the only record of that edit.
//!
//! Claude Code's `structuredPatch` is the shape `jsdiff` produces — hunks with
//! `oldStart`/`newStart` and lines prefixed `+`, `-` or a space. Both line
//! numbers are carried, because "which line was that?" is the question a diff in
//! an audit trail is asked.

import type { FileChange } from "./bindings";
import { el, span } from "./dom";
import { count, shortPath } from "./format";

interface Hunk {
  oldStart: number;
  oldLines: number;
  newStart: number;
  newLines: number;
  lines: string[];
}

function parseHunks(patch: string | null): Hunk[] | null {
  if (patch === null) return null;
  try {
    const value: unknown = JSON.parse(patch);
    if (!Array.isArray(value)) return null;
    return value.flatMap((h: unknown) => {
      if (typeof h !== "object" || h === null) return [];
      const hunk = h as Partial<Hunk>;
      if (!Array.isArray(hunk.lines)) return [];
      return [
        {
          oldStart: Number(hunk.oldStart ?? 0),
          oldLines: Number(hunk.oldLines ?? 0),
          newStart: Number(hunk.newStart ?? 0),
          newLines: Number(hunk.newLines ?? 0),
          lines: hunk.lines.filter((l): l is string => typeof l === "string"),
        },
      ];
    });
  } catch {
    return null;
  }
}

function line(oldNo: number | null, newNo: number | null, text: string, kind: string): HTMLElement {
  const row = el("div", { class: `dl ${kind}` });
  row.append(
    span("ln", oldNo === null ? "" : String(oldNo)),
    span("ln", newNo === null ? "" : String(newNo)),
    // The first character is the marker; keeping it makes the diff selectable
    // as a patch, which is what an evidence bundle wants pasted into it.
    span("dt", text),
  );
  return row;
}

/** One file's diff. */
export function renderDiff(change: FileChange): HTMLElement {
  const box = el("div", { class: "file" });
  box.append(
    el("div", { class: "fhead" }, [
      span("fpath", shortPath(change.file_path, 4), change.file_path),
      span(
        "fstat",
        `+${count(change.lines_added)} −${count(change.lines_removed)}`,
      ),
    ]),
  );

  const hunks = parseHunks(change.patch_json);
  if (hunks === null) {
    box.append(
      el("div", {
        class: "none pad",
        text: "No patch was recorded for this change.",
      }),
    );
    return box;
  }
  if (hunks.length === 0) {
    // A `Write` that creates a file has an empty patch and still changed
    // something. Saying so beats an empty box.
    box.append(
      el("div", {
        class: "none pad",
        text:
          change.lines_added === 0 && change.lines_removed === 0
            ? "The whole file was written; Claude Code recorded no line-level patch."
            : "No hunks recorded.",
      }),
    );
    return box;
  }

  const body = el("div", { class: "diff" });
  for (const hunk of hunks) {
    body.append(
      line(
        null,
        null,
        `@@ -${hunk.oldStart},${hunk.oldLines} +${hunk.newStart},${hunk.newLines} @@`,
        "hunk",
      ),
    );
    let oldNo = hunk.oldStart;
    let newNo = hunk.newStart;
    for (const text of hunk.lines) {
      const marker = text.charAt(0);
      if (marker === "+") {
        body.append(line(null, newNo, text, "add"));
        newNo += 1;
      } else if (marker === "-") {
        body.append(line(oldNo, null, text, "del"));
        oldNo += 1;
      } else if (marker === "\\") {
        // `\ No newline at end of file` is a note about the line above, not a
        // line of the file. Counting it would shift every number after it.
        body.append(line(null, null, text, "meta"));
      } else {
        body.append(line(oldNo, newNo, text, "ctx"));
        oldNo += 1;
        newNo += 1;
      }
    }
  }
  box.append(body);
  return box;
}
