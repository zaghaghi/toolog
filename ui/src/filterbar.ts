//! The controls above the list (tasks 5.6, 5.7, 5.12).
//!
//! Every control writes into one `TimelineFilter`, which is also what the URL
//! hash holds and what an export is taken from — so what you see, what you can
//! link to and what you can hand to someone else are the same thing by
//! construction, not by three code paths agreeing.
//!
//! The dropdown values come from the store rather than from a hard-coded list.
//! Claude Code adds tools and permission modes between releases, and a filter
//! that offers values nobody has used — or omits ones they have — is worse than
//! no filter at all.

import type { Facets, Format, TimelineFilter } from "./bindings";
import { saveExport } from "./bindings";
import { el, fill, span } from "./dom";
import { count } from "./format";
import type { Lane, Outcome, Thread, ViewState } from "./view";
import {
  emptyFilter,
  isFiltered,
  laneOf,
  outcomeOf,
  PRESETS,
  threadOf,
  withLane,
  withOutcome,
  withThread,
} from "./view";

/** How long the search box waits before asking the store (task 5.7). */
const DEBOUNCE = 140;

interface Option {
  value: string;
  label: string;
}

function select(
  label: string,
  options: Option[],
  current: string,
  onPick: (value: string) => void,
): HTMLElement {
  const node = el("select", { class: "pick", attrs: { "aria-label": label } });
  for (const option of options) {
    const item = el("option", { value: option.value, text: option.label });
    if (option.value === current) item.selected = true;
    node.append(item);
  }
  node.addEventListener("change", () => onPick(node.value));
  return node;
}

function names(values: string[], all: string): Option[] {
  return [{ value: "", label: all }, ...values.map((v) => ({ value: v, label: v }))];
}

/** The last path segment, for a project dropdown that has to fit. */
function leaf(path: string): string {
  const parts = path.split("/").filter(Boolean);
  return parts.at(-1) ?? path;
}

export interface FilterBarOptions {
  onChange: (next: ViewState) => void;
  onNotice: (message: string) => void;
}

export class FilterBar {
  readonly node = el("header", { class: "bar" });
  private readonly controls = el("div", { class: "controls" });
  private readonly summary = el("div", { class: "summary" });
  private readonly search: HTMLInputElement;
  private timer = 0;
  private view: ViewState;
  private facets: Facets = {
    projects: [],
    tools: [],
    decision_sources: [],
    permission_modes: [],
    agents: [],
  };

  constructor(
    view: ViewState,
    private readonly options: FilterBarOptions,
  ) {
    this.view = view;
    this.search = el("input", {
      class: "search",
      type: "search",
      placeholder: "Search commands, paths and results…",
      value: view.filter.query ?? "",
      attrs: { "aria-label": "Search", spellcheck: "false", autocomplete: "off" },
    });
    // Debounced, because every keystroke is an FTS query over the whole store.
    this.search.addEventListener("input", () => {
      clearTimeout(this.timer);
      this.timer = window.setTimeout(() => {
        const text = this.search.value.trim();
        this.change({ query: text === "" ? null : text });
      }, DEBOUNCE);
    });
    this.search.addEventListener("keydown", (event) => {
      if (event.key === "Escape") {
        this.search.value = "";
        this.change({ query: null });
      }
    });

    this.node.append(
      el("div", { class: "searchrow" }, [this.search, this.exportMenu()]),
      this.controls,
      this.summary,
    );
    this.draw();
  }

  focusSearch(): void {
    this.search.focus();
    this.search.select();
  }

  setFacets(facets: Facets): void {
    this.facets = facets;
    this.draw();
  }

  setView(view: ViewState): void {
    this.view = view;
    if (this.search.value !== (view.filter.query ?? "")) {
      this.search.value = view.filter.query ?? "";
    }
    this.draw();
  }

  /** The line under the controls: how many rows, and how they were narrowed. */
  setStatus(total: number, loading: boolean): void {
    const parts: (Node | string)[] = [
      loading ? "Counting…" : `${count(total)} ${total === 1 ? "call" : "calls"}`,
    ];
    if (isFiltered(this.view.filter)) {
      const clear = el("button", { class: "link", text: "Clear filters" });
      clear.addEventListener("click", () =>
        this.replace({ ...this.view, selected: null, filter: emptyFilter() }),
      );
      parts.push(span("dot", "·"), clear);
    }
    fill(this.summary, parts);
  }

  private change(patch: Partial<TimelineFilter>): void {
    this.replace({
      ...this.view,
      // A narrowed list makes the old selection meaningless; the pane clears
      // rather than showing a call the list no longer contains.
      selected: null,
      filter: { ...this.view.filter, ...patch },
    });
  }

  private replace(next: ViewState): void {
    this.view = next;
    this.draw();
    this.options.onChange(next);
  }

  private draw(): void {
    const f = this.view.filter;
    const now = Date.now();

    // `since` is stored absolute so a shared link keeps its meaning, which
    // means the preset it came from has to be recognised rather than read back.
    // A minute of tolerance covers the gap between choosing and redrawing.
    const elapsed = f.since === null ? null : now - f.since;
    const preset =
      elapsed === null
        ? "null"
        : (PRESETS.find((p) => p.ago !== null && Math.abs(elapsed - p.ago) < 60_000)?.ago ??
          "custom");

    fill(this.controls, [
      select(
        "Project",
        [
          { value: "", label: "All projects" },
          ...this.facets.projects.map((p) => ({ value: p, label: leaf(p) })),
        ],
        f.project_path ?? "",
        (v) => this.change({ project_path: v === "" ? null : v }),
      ),
      select(
        "Tool",
        names(this.facets.tools, "All tools"),
        f.tool_name ?? "",
        (v) => this.change({ tool_name: v === "" ? null : v }),
      ),
      select(
        "Outcome",
        [
          { value: "any", label: "Any outcome" },
          { value: "ok", label: "Succeeded" },
          { value: "failed", label: "Failed" },
          { value: "refused", label: "Refused" },
        ],
        outcomeOf(f),
        (v) => this.replace({ ...this.view, selected: null, filter: withOutcome(f, v as Outcome) }),
      ),
      select(
        "Time",
        [
          ...PRESETS.map((p) => ({ value: String(p.ago), label: p.label })),
          ...(preset === "custom" ? [{ value: "custom", label: "Custom range" }] : []),
        ],
        String(preset),
        (v) => {
          if (v === "custom") return;
          this.change({ since: v === "null" ? null : now - Number(v), until: null });
        },
      ),
      select(
        "Thread",
        [
          { value: "any", label: "All threads" },
          { value: "main", label: "Main thread" },
          { value: "sub", label: "Subagents" },
        ],
        threadOf(f),
        (v) => this.replace({ ...this.view, selected: null, filter: withThread(f, v as Thread) }),
      ),
      select(
        "Decision source",
        names(this.facets.decision_sources, "Any decision source"),
        f.decision_source ?? "",
        (v) => this.change({ decision_source: v === "" ? null : v }),
      ),
      select(
        "Permission mode",
        names(this.facets.permission_modes, "Any permission mode"),
        f.permission_mode ?? "",
        (v) => this.change({ permission_mode: v === "" ? null : v }),
      ),
      select(
        "Lanes",
        [
          { value: "any", label: "Any lane" },
          { value: "both", label: "Both lanes" },
          { value: "transcript", label: "Transcript only" },
          { value: "otel", label: "OTEL only" },
        ],
        laneOf(f),
        (v) => this.replace({ ...this.view, selected: null, filter: withLane(f, v as Lane) }),
      ),
      this.groupToggle(),
    ]);
  }

  private groupToggle(): HTMLElement {
    const button = el("button", {
      class: this.view.grouped ? "toggle on" : "toggle",
      text: "Group by session",
      attrs: { "aria-pressed": String(this.view.grouped) },
    });
    button.addEventListener("click", () =>
      this.replace({ ...this.view, grouped: !this.view.grouped }),
    );
    return button;
  }

  /** Export the current filter, not the current page (task 5.12). */
  private exportMenu(): HTMLElement {
    const formats: [Format, string][] = [
      ["json", "JSON"],
      ["jsonl", "JSON Lines"],
      ["csv", "CSV"],
      ["markdown", "Markdown"],
    ];
    const menu = el("div", { class: "menu" });
    const button = el("button", { class: "toggle", text: "Export…" });
    const list = el("div", { class: "menupop", hidden: true });

    for (const [format, label] of formats) {
      const item = el("button", { class: "menuitem", text: label });
      item.addEventListener("click", () => {
        list.hidden = true;
        void saveExport(this.view.filter, format, null)
          .then((path) => {
            if (path !== null) this.options.onNotice(`Exported to ${path}`);
          })
          .catch((error: unknown) => this.options.onNotice(String(error)));
      });
      list.append(item);
    }

    button.addEventListener("click", (event) => {
      event.stopPropagation();
      list.hidden = !list.hidden;
    });
    document.addEventListener("click", () => {
      list.hidden = true;
    });

    menu.append(button, list);
    return menu;
  }
}
