import { describe, expect, test } from "vitest";

import type { FileChange } from "./bindings";
import { renderDiff } from "./diff";

function change(over: Partial<FileChange> = {}): FileChange {
  return {
    tool_use_id: "t1",
    file_path: "/work/app/crates/toolog-core/src/query.rs",
    lines_added: 2,
    lines_removed: 1,
    patch_json: JSON.stringify([
      {
        oldStart: 84,
        oldLines: 3,
        newStart: 84,
        newLines: 4,
        lines: [
          ' bind!(f.decision_source, "tc.decision_source = ?");',
          '+bind!(f.permission_mode, "tc.permission_mode = ?");',
          '+bind!(f.agent_id, "tc.agent_id = ?");',
          "-// TODO: permission mode",
        ],
      },
    ]),
    ...over,
  };
}

function lines(node: HTMLElement): { kind: string; old: string; now: string; text: string }[] {
  return [...node.querySelectorAll(".dl")].map((row) => {
    const cells = [...row.querySelectorAll("span")];
    return {
      kind: row.className.replace("dl ", ""),
      old: cells[0]?.textContent ?? "",
      now: cells[1]?.textContent ?? "",
      text: cells[2]?.textContent ?? "",
    };
  });
}

describe("structuredPatch rendering (task 5.9)", () => {
  test("carries both line numbers, and only the side each line exists on", () => {
    const rows = lines(renderDiff(change()));

    expect(rows[0]).toMatchObject({ kind: "hunk", text: "@@ -84,3 +84,4 @@" });
    expect(rows[1]).toMatchObject({ kind: "ctx", old: "84", now: "84" });
    // An added line has no old number, and a removed line has no new one.
    expect(rows[2]).toMatchObject({ kind: "add", old: "", now: "85" });
    expect(rows[3]).toMatchObject({ kind: "add", old: "", now: "86" });
    expect(rows[4]).toMatchObject({ kind: "del", old: "85", now: "" });
  });

  test("keeps the +/- markers, so the diff can be copied out as a patch", () => {
    const rows = lines(renderDiff(change()));
    expect(rows[2]?.text.startsWith("+")).toBe(true);
    expect(rows[4]?.text.startsWith("-")).toBe(true);
  });

  test("shows the file and its size", () => {
    const node = renderDiff(change({ lines_added: 18, lines_removed: 4 }));
    expect(node.querySelector(".fstat")?.textContent).toBe("+18 −4");
    expect(node.querySelector(".fpath")?.getAttribute("title")).toBe(
      "/work/app/crates/toolog-core/src/query.rs",
    );
  });

  test("a whole-file write says so instead of showing an empty box", () => {
    const node = renderDiff(
      change({ patch_json: "[]", lines_added: 0, lines_removed: 0 }),
    );
    expect(node.textContent).toContain("no line-level patch");
    expect(node.querySelector(".diff")).toBeNull();
  });

  test("a missing or unparseable patch is stated, not thrown", () => {
    expect(renderDiff(change({ patch_json: null })).textContent).toContain("No patch was recorded");
    expect(renderDiff(change({ patch_json: "{oops" })).textContent).toContain(
      "No patch was recorded",
    );
    expect(renderDiff(change({ patch_json: '{"not":"an array"}' })).textContent).toContain(
      "No patch was recorded",
    );
  });

  // Four of the 342 hunks in the owner's real store carry one of these.
  test("'\\ No newline at end of file' is a note, not a line", () => {
    const rows = lines(
      renderDiff(
        change({
          patch_json: JSON.stringify([
            {
              oldStart: 10,
              oldLines: 2,
              newStart: 10,
              newLines: 2,
              lines: ["-old tail", "\\ No newline at end of file", "+new tail", " after"],
            },
          ]),
        }),
      ),
    );

    expect(rows[2]).toMatchObject({ kind: "meta", old: "", now: "" });
    // The context line after it is still line 11 on both sides, not 12.
    expect(rows[4]).toMatchObject({ kind: "ctx", old: "11", now: "11" });
  });

  test("a hunk missing its fields is skipped rather than rendered as NaN", () => {
    const node = renderDiff(change({ patch_json: '[{"lines":["+one"]},{"nope":1}]' }));
    const rows = lines(node);
    expect(rows.filter((r) => r.kind === "add")).toHaveLength(1);
    expect(node.textContent).not.toContain("NaN");
  });
});
