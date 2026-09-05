//! The window: two views, one URL.
//!
//! The address bar is the application's state (task 5.6). Every filter, the
//! grouping and the open call live in the hash, so a view can be copied out of
//! the window and pasted back into it — and the back button undoes a filter,
//! which is the behaviour anyone who has used a browser already expects.

import "./styles/tokens.css";
import "./styles/app.css";

import { listen } from "@tauri-apps/api/event";

import type { ToolCall } from "./bindings";
import { el, fill, span } from "./dom";
import { SetupView } from "./setup";
import { TimelineView } from "./timeline";
import type { ViewState } from "./view";
import { fromHash, toHash } from "./view";

type Screen = "timeline" | "setup";

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

/** `#…&v=setup` selects the screen; everything else is the timeline's filter. */
function screenFromHash(hash: string): Screen {
  return new URLSearchParams(hash.replace(/^#/, "")).get("v") === "setup" ? "setup" : "timeline";
}

function hashFor(screen: Screen, view: ViewState): string {
  const hash = toHash(view);
  if (screen === "timeline") return hash;
  return hash === "" ? "#v=setup" : `${hash}&v=setup`;
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

function drawTabs(): void {
  fill(tabs, [
    tab("Timeline", "timeline"),
    tab("Status", "setup"),
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
  fill(body, [next === "timeline" ? timeline.node : setup.node]);
  if (next === "setup") setup.refresh();
  writeHash();
}

window.addEventListener("popstate", () => {
  if (writing) return;
  screen = screenFromHash(location.hash);
  view = fromHash(location.hash);
  drawTabs();
  fill(body, [screen === "timeline" ? timeline.node : setup.node]);
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

// The Phase 4 event stream: a call has just been stored.
void listen<ToolCall>("live_tool_call", (event) => timeline.noteLiveCall(event.payload));

root.className = "";
fill(root, [tabs, body, toast]);
show(screen);
