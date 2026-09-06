//! The window: three screens, one URL.
//!
//! The address bar is the application's state (task 5.6). Every filter, the
//! grouping and the open call live in the hash, so a view can be copied out of
//! the window and pasted back into it — and the back button undoes a filter,
//! which is the behaviour anyone who has used a browser already expects.
//!
//! Each screen is built once and kept, so switching tabs shows what was there
//! rather than reloading it.
//!
//! The live event stream reaches the timeline, which counts what it has not
//! shown yet. A call arrives more than once by design — the transcript creates
//! the row, OTEL completes it — so the count keys on `tool_use_id` rather than
//! counting arrivals (task 9.2: the pill is what survives of the live view).

import "./styles/tokens.css";
import "./styles/app.css";

import { listen } from "@tauri-apps/api/event";

import type { ToolCall } from "./bindings";
import { el, fill, span } from "./dom";
import { RiskView } from "./risk";
import { SetupView } from "./setup";
import { applyTheme, currentTheme } from "./theme";
import { TimelineView } from "./timeline";
import type { ViewState } from "./view";
import { parse as parseQuery } from "./query";
import { emptyFilter, emptyView, fromHash, toHash } from "./view";

type Screen = "timeline" | "risk" | "setup";

const SCREENS: [Screen, string][] = [
  ["timeline", "Timeline"],
  ["risk", "Risk"],
  ["setup", "Status"],
];

// Before anything is built, so the first paint is already the right theme
// rather than a light frame corrected a moment later.
applyTheme(currentTheme());

const root = document.getElementById("app");
if (root === null) throw new Error("no #app to mount into");

const body = el("main", { class: "body" });
const tabs = el("nav", { class: "tabs" });
const toast = el("div", { class: "toast", hidden: true });
let toastTimer = 0;

function notice(message: string): void {
  toast.textContent = message;
  toast.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => {
    toast.hidden = true;
  }, 6000);
}

// ---------------------------------------------------------------- routing

/** `#…&v=risk` selects the screen; everything else is the timeline's filter. */
function screenFromHash(hash: string): Screen {
  const asked = new URLSearchParams(hash.replace(/^#/, "")).get("v");
  return SCREENS.some(([id]) => id === asked) ? (asked as Screen) : "timeline";
}

function hashFor(screen: Screen, view: ViewState): string {
  const hash = toHash(view);
  if (screen === "timeline") return hash;
  return hash === "" ? `#v=${screen}` : `${hash}&v=${screen}`;
}

let screen: Screen = screenFromHash(location.hash);
let view: ViewState = fromHash(location.hash);
/** Set while we are the ones changing the hash, so we do not reload ourselves. */
let writing = false;

function writeHash(): void {
  const next = hashFor(screen, view);
  const current = location.hash;
  if (next === current || (next === "" && current === "")) return;
  writing = true;
  // `replaceState` for a filter tweak would lose the back button, which is the
  // cheapest undo this interface has.
  history.pushState(null, "", next === "" ? location.pathname : next);
  writing = false;
}

// ------------------------------------------------------------------ views

const timeline = new TimelineView(view, {
  onViewChange: (next) => {
    view = next;
    writeHash();
  },
  onNotice: notice,
  onOpenSetup: () => show("setup"),
});

const setup = new SetupView({
  onNotice: notice,
  onChanged: () => {
    // Enabling capture or importing history changes what the timeline can show.
    timeline.apply({ ...view }, false);
  },
});

/** Take the reader to one call, wherever they asked from. */
function openCall(toolUseId: string): void {
  view = { ...view, selected: toolUseId };
  show("timeline");
  timeline.apply(view, true);
}

/** Take the reader to every call one rule matched (task 12.12). */
function openRule(ruleId: string): void {
  // The rule replaces whatever was narrowing the list: arriving at a filtered
  // timeline that shows fewer calls than the finding claimed would be worse
  // than arriving at the wrong place.
  view = { ...emptyView(), filter: { ...emptyFilter(), rule_id: ruleId } };
  show("timeline");
  timeline.apply(view, true);
}

/**
 * Take the reader to a query, written the way they would have typed it.
 *
 * `openRule` builds a filter directly because a rule id is not something a
 * reader composes; this takes the query *language* instead, so what arrives in
 * the box is a sentence they could have written and can now edit. Phase 13's
 * section is the first caller: "the calls the model scored 4 or above" is
 * `@llm-risk:>=4`, and being able to change the 4 is most of its value.
 */
function openQuery(query: string): void {
  view = { ...emptyView(), filter: parseQuery(query).filter };
  show("timeline");
  timeline.apply(view, true);
}

const risk = new RiskView({
  onNotice: notice,
  onOpenCall: openCall,
  onOpenRule: openRule,
  onOpenQuery: openQuery,
});

const screens: Record<Screen, { node: HTMLElement }> = { timeline, risk, setup };

function drawTabs(): void {
  fill(tabs, [
    ...SCREENS.map(([id, label]) => tab(label, id)),
    span("grow", ""),
    span("brand", "toolog"),
  ]);
}

function tab(label: string, target: Screen): HTMLElement {
  const button = el("button", {
    class: screen === target ? "tab on" : "tab",
    text: label,
    attrs: { "aria-current": String(screen === target) },
  });
  button.addEventListener("click", () => show(target));
  return button;
}

function show(next: Screen): void {
  screen = next;
  drawTabs();
  fill(body, [screens[next].node]);
  if (next === "setup") setup.refresh();
  if (next === "risk") void risk.refresh();
  writeHash();
}

window.addEventListener("popstate", () => {
  if (writing) return;
  view = fromHash(location.hash);
  show(screenFromHash(location.hash));
  timeline.apply(view, false);
});

// `/` is the search box everywhere except inside a field.
window.addEventListener("keydown", (event) => {
  const target = event.target as HTMLElement | null;
  const typing = target instanceof HTMLInputElement || target instanceof HTMLSelectElement;
  if (event.key === "/" && !typing) {
    event.preventDefault();
    show("timeline");
    timeline.focusSearch();
  }
});

// The event stream (task 6.9): a call has just been committed.
void listen<ToolCall>("live_tool_call", (event) => {
  timeline.noteLiveCall(event.payload);
});

root.className = "";
fill(root, [tabs, body, toast]);
show(screen);
