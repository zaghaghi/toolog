//! The dark palette is written twice. This is what keeps the copies identical.
//!
//! `light-dark()` would collapse them into one block, and needs Safari 17.5;
//! `tauri.conf.json` sets a floor of macOS 10.15. So the duplication is forced,
//! and the drift it invites is guarded here rather than noticed later by
//! someone whose window is half one theme.

import { readFileSync } from "node:fs";
import { join } from "node:path";

import { describe, expect, test } from "vitest";

const css = readFileSync(join(import.meta.dirname, "styles/tokens.css"), "utf8");

/** Every `--token: value;` inside the block a selector opens. */
function declarations(selector: string): Record<string, string> {
  const at = css.indexOf(selector);
  expect(at, `${selector} is not in tokens.css`).toBeGreaterThan(-1);

  const open = css.indexOf("{", at);
  let depth = 0;
  let close = open;
  for (let i = open; i < css.length; i += 1) {
    if (css[i] === "{") depth += 1;
    if (css[i] === "}") {
      depth -= 1;
      if (depth === 0) {
        close = i;
        break;
      }
    }
  }

  const out: Record<string, string> = {};
  for (const [, name, value] of css
    .slice(open + 1, close)
    .matchAll(/(--[a-z0-9-]+)\s*:\s*([^;]+);/g)) {
    out[name!] = value!.trim();
  }
  return out;
}

describe("the two dark palettes", () => {
  const media = declarations(':root:not([data-theme="light"])');
  const explicit = declarations(':root[data-theme="dark"]');

  test("declare exactly the same tokens", () => {
    expect(Object.keys(explicit).sort()).toEqual(Object.keys(media).sort());
  });

  test("give every one of them the same value", () => {
    expect(explicit).toEqual(media);
  });

  test("cover every literal colour the light palette defines", () => {
    // A colour that exists in light and not in dark stays light in dark mode —
    // the failure mode hardest to see, because everything around it changed.
    //
    // Only *literal* colours: a token defined as `var(--line)` follows whatever
    // that resolves to, and type, spacing and radius are not colours at all.
    const light = declarations(":root {");
    const colours = Object.keys(light).filter((name) =>
      /#[0-9a-f]{3,8}|rgb\(/i.test(light[name] ?? ""),
    );

    expect(colours.length).toBeGreaterThan(15);
    expect(colours.filter((name) => !(name in media))).toEqual([]);
  });
});
