//! The window: five screens, one URL.
//!
//! The address bar is the application's state (task 5.6). Every filter, the
//! grouping and the open call live in the hash, so a view can be copied out of
//! the window and pasted back into it — and the back button undoes a filter,
//! which is the behaviour anyone who has used a browser already expects.
//!
//! Each screen is built once and kept. Three of them hold fetched state and
//! two of them poll, so switching tabs shows what was there rather than
//! reloading it, and the live view's timer runs only while it is on screen.
//!
//! The live event stream reaches both the timeline (which counts what it has
//! not shown yet) and the live view (which is the stream). A call arrives more
//! than once by design — the transcript creates the row, OTEL completes it —
//! so both of them key on `tool_use_id` rather than counting arrivals.

import "./styles/tokens.css";
import "./styles/app.css";

import { listen } from "@tauri-apps/api/event";

import { AnalyticsView } from "./analytics";
import type { ToolCall } from "./bindings";
import { el, fill, span } from "./dom";
import { LiveView } from "./live";
import { RiskView } from "./risk";
import { SetupView } from "./setup";
import { TimelineView } from "./timeline";
import type { ViewState } from "./view";
import { fromHash, toHash } from "./view";

type Screen = "timeline" | "risk" | "usage" | "live" | "setup";

const SCREENS: [Screen, string][] = [
  ["timeline", "Timeline"],
  ["risk", "Risk"],
  ["usage", "Usage"],
  ["live", "Live"],
  ["setup", "Status"],
];

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

const risk = new RiskView({ onNotice: notice, onOpenCall: openCall });
const usage = new AnalyticsView({ onNotice: notice });
const live = new LiveView({ onNotice: notice, onOpenCall: openCall });

const screens: Record<Screen, { node: HTMLElement }> = { timeline, risk, usage, live, setup };

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
  // The live view polls, so its timer follows the tab rather than the process.
  if (next !== "live") live.stop();
  if (next === "setup") setup.refresh();
  if (next === "risk") void risk.refresh();
  if (next === "usage") void usage.refresh();
  if (next === "live") live.start();
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
  live.noteCall(event.payload);
});

root.className = "";
fill(root, [tabs, body, toast]);
show(screen);
