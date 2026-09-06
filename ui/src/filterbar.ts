//! The controls above the list (tasks 5.6, 5.7, 5.12, and 10.5–10.11).
//!
//! One box, one time control, one grouping toggle and Export. Seven `<select>`s
//! used to sit here; the owner's report on v1.0 was "filters are too much, too
//! many items to filter — I like the style of filtering that is embedded in the
//! search edit box, like GitHub or Datadog", and this is that.
//!
//! Everything still writes into one `TimelineFilter`, which is what the URL
//! hash holds and what an export is taken from — so what you see, what you can
//! link to and what you can hand to someone else are the same thing by
//! construction. The query bar is a second **editor** of that filter, not a
//! second representation of it (task 10.7): a v1.0 link still restores, and
//! Export still exports the filter rather than the text in the box.
//!
//! **Time stays a control.** It is the one dimension with a chart under it, and
//! dragging across the histogram says "this hour" better than typing two
//! timestamps ever will.
//!
//! Autocomplete values come from the store, not from a hard-coded list: Claude
//! Code adds tools and permission modes between releases, and a filter offering
//! values nobody has used — or missing ones they have — is worse than none.

import type { Facets, Format, TimelineFilter } from "./bindings";
import { saveExport } from "./bindings";
import { el, fill, span } from "./dom";
import { count } from "./format";
import { fixedValues, format as formatQuery, KEYS, parse, quote, tokenize } from "./query";
import type { Token } from "./query";
import type { ViewState } from "./view";
import { emptyFilter, isFiltered, PRESETS } from "./view";

/** How long the box waits before asking the store (task 5.7). */
const DEBOUNCE = 140;

/** How many completions are offered at once. */
const SUGGESTIONS = 8;

export interface FilterBarOptions {
  onChange: (next: ViewState) => void;
  onNotice: (message: string) => void;
}

interface Suggestion {
  /** What replaces the token under the caret. */
  insert: string;
  label: string;
  hint: string;
}

export class FilterBar {
  readonly node = el("header", { class: "bar" });
  private readonly controls = el("div", { class: "controls" });
  private readonly summary = el("div", { class: "summary" });
  private readonly errors = el("div", { class: "qerrors", hidden: true });
  private readonly complete = el("div", { class: "qmenu", hidden: true });
  private readonly search: HTMLInputElement;
  private timer = 0;
  private view: ViewState;
  private suggestions: Suggestion[] = [];
  private highlighted = 0;
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
      type: "text",
      placeholder: "Filter with @tool:Bash @outcome:refused, and search for the rest…",
      value: formatQuery(view.filter),
      attrs: {
        "aria-label": "Filter and search",
        spellcheck: "false",
        autocomplete: "off",
        role: "combobox",
        "aria-expanded": "false",
        "aria-autocomplete": "list",
      },
    });

    // Debounced, because every keystroke is an FTS query over the whole store.
    this.search.addEventListener("input", () => {
      this.drawComplete();
      clearTimeout(this.timer);
      this.timer = window.setTimeout(() => this.commit(), DEBOUNCE);
    });
    this.search.addEventListener("keydown", (event) => this.onKey(event));
    this.search.addEventListener("blur", () => {
      // A click on a suggestion fires before blur settles, so the menu closes
      // on the next tick rather than under the pointer.
      window.setTimeout(() => this.closeComplete(), 120);
    });
    this.search.addEventListener("click", () => this.drawComplete());

    this.node.append(
      el("div", { class: "searchrow" }, [
        el("div", { class: "qbox" }, [this.search, this.complete]),
        this.exportMenu(),
      ]),
      this.errors,
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
    // Only rewrite the box when it would say something different. Reformatting
    // under the caret while someone is typing is how a text field fights back.
    const text = formatQuery(view.filter);
    if (parse(this.search.value).filter.query !== view.filter.query || !this.saysSame(text)) {
      this.search.value = text;
    }
    this.draw();
  }

  /** Whether the box already expresses this filter, whatever its spelling. */
  private saysSame(text: string): boolean {
    const typed = parse(this.search.value).filter;
    const wanted = parse(text).filter;
    return (Object.keys(wanted) as (keyof TimelineFilter)[]).every((k) => typed[k] === wanted[k]);
  }

  /** The line under the controls: how many rows, and how they were narrowed. */
  setStatus(total: number, loading: boolean): void {
    const parts: (Node | string)[] = [
      loading ? "Counting…" : `${count(total)} ${total === 1 ? "call" : "calls"}`,
    ];
    if (isFiltered(this.view.filter)) {
      const clear = el("button", { class: "link", text: "Clear filters" });
      clear.addEventListener("click", () => {
        this.search.value = "";
        this.replace({ ...this.view, selected: null, filter: emptyFilter() });
      });
      parts.push(span("dot", "·"), clear);
    }
    fill(this.summary, parts);
  }

  // ------------------------------------------------------------- the box

  /**
   * Read the box into the filter.
   *
   * The time bounds and the unattributed-group flag are carried over rather
   * than taken from the text: the query bar does not write them, so it must not
   * clear them either (task 10.7).
   */
  private commit(): void {
    const { filter, errors } = parse(this.search.value);
    const next: TimelineFilter = {
      ...filter,
      since: this.view.filter.since,
      until: this.view.filter.until,
      session_unknown: this.view.filter.session_unknown,
    };

    fill(
      this.errors,
      errors.map((e) => el("div", { class: "qerror", text: e.message })),
    );
    this.errors.hidden = errors.length === 0;

    this.replace({ ...this.view, selected: null, filter: next });
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

  // ------------------------------------------------------- autocomplete

  /** The token the caret sits in, which is what a completion replaces. */
  private tokenAtCaret(): Token | null {
    const caret = this.search.selectionStart ?? this.search.value.length;
    const tokens = tokenize(this.search.value);
    return tokens.find((t) => caret >= t.start && caret <= t.end) ?? null;
  }

  /** The values a key offers: from the store where there are any (task 10.8). */
  private valuesFor(key: string): string[] {
    switch (key) {
      case "project":
        return this.facets.projects;
      case "tool":
        return this.facets.tools;
      case "source":
        return this.facets.decision_sources;
      case "mode":
        return this.facets.permission_modes;
      case "agent":
        return this.facets.agents;
      case "decision":
        return ["accept", "reject"];
      default:
        return fixedValues(key);
    }
  }

  private suggest(token: Token | null): Suggestion[] {
    if (token === null || token.kind !== "pair") return [];
    const colon = this.search.value.slice(token.start, token.end).includes(":");

    if (!colon) {
      const typed = token.key.toLowerCase();
      return Object.entries(KEYS)
        .filter(([key]) => key.startsWith(typed))
        .slice(0, SUGGESTIONS)
        .map(([key, spec]) => ({ insert: `@${key}:`, label: `@${key}`, hint: spec.hint }));
    }

    if (KEYS[token.key] === undefined) return [];
    const typed = token.value.toLowerCase();
    return this.valuesFor(token.key)
      .filter((v) => v.toLowerCase().includes(typed))
      .slice(0, SUGGESTIONS)
      .map((value) => ({
        insert: `@${token.key}:${quote(value)} `,
        label: value,
        hint: `@${token.key}`,
      }));
  }

  private drawComplete(): void {
    this.suggestions = this.suggest(this.tokenAtCaret());
    this.highlighted = 0;
    if (this.suggestions.length === 0) {
      this.closeComplete();
      return;
    }
    fill(
      this.complete,
      this.suggestions.map((item, i) => {
        const row = el("button", {
          class: i === this.highlighted ? "qitem on" : "qitem",
          attrs: { type: "button", role: "option", "aria-selected": String(i === this.highlighted) },
        });
        row.append(span("qlabel", item.label), span("qhint", item.hint));
        // `mousedown`, not `click`: the input's blur would close the menu first.
        row.addEventListener("mousedown", (event) => {
          event.preventDefault();
          this.accept(i);
        });
        return row;
      }),
    );
    this.complete.hidden = false;
    this.search.setAttribute("aria-expanded", "true");
  }

  private closeComplete(): void {
    this.suggestions = [];
    this.complete.hidden = true;
    this.search.setAttribute("aria-expanded", "false");
  }

  /** Put a suggestion in place of the token under the caret. */
  private accept(index: number): void {
    const item = this.suggestions[index];
    const token = this.tokenAtCaret();
    if (item === undefined || token === null) return;

    const text = this.search.value;
    this.search.value = text.slice(0, token.start) + item.insert + text.slice(token.end);
    const caret = token.start + item.insert.length;
    this.search.setSelectionRange(caret, caret);
    this.search.focus();

    this.closeComplete();
    // A completed key still needs its value, so only a completed *value*
    // is worth asking the store about.
    if (item.insert.endsWith(" ")) this.commit();
    else this.drawComplete();
  }

  private onKey(event: KeyboardEvent): void {
    if (event.key === "Escape") {
      if (this.suggestions.length > 0) {
        event.preventDefault();
        this.closeComplete();
        return;
      }
      this.search.value = "";
      this.commit();
      return;
    }

    if (this.suggestions.length === 0) {
      if (event.key === "Enter") {
        clearTimeout(this.timer);
        this.commit();
      }
      return;
    }

    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      const step = event.key === "ArrowDown" ? 1 : -1;
      const n = this.suggestions.length;
      this.highlighted = (this.highlighted + step + n) % n;
      for (const [i, row] of [...this.complete.children].entries()) {
        row.className = i === this.highlighted ? "qitem on" : "qitem";
        row.setAttribute("aria-selected", String(i === this.highlighted));
      }
      return;
    }

    if (event.key === "Enter" || event.key === "Tab") {
      event.preventDefault();
      this.accept(this.highlighted);
    }
  }

  // --------------------------------------------------------- the controls

  private draw(): void {
    const f = this.view.filter;
    const now = Date.now();

    // `since` is stored absolute so a shared link keeps its meaning, which
    // means the preset it came from has to be recognised rather than read back.
    // A minute of tolerance covers the gap between choosing and redrawing.
    // A range brushed on the histogram matches no preset, and lands on
    // "Custom range" — which is the state this control already had.
    const elapsed = f.since === null ? null : now - f.since;
    const preset =
      elapsed === null
        ? "null"
        : f.until !== null
          ? "custom"
          : (PRESETS.find((p) => p.ago !== null && Math.abs(elapsed - p.ago) < 60_000)?.ago ??
            "custom");

    const time = el("select", { class: "pick", attrs: { "aria-label": "Time" } });
    for (const option of [
      ...PRESETS.map((p) => ({ value: String(p.ago), label: p.label })),
      ...(preset === "custom" ? [{ value: "custom", label: "Custom range" }] : []),
    ]) {
      time.append(el("option", { value: option.value, text: option.label }));
    }
    // Assigned after the options exist rather than by marking one `selected`
    // before it is inserted. The two are equivalent in a browser and not in
    // every DOM: selectedness set on a detached option is reset on insertion
    // unless the implementation tracks dirtiness, and this does not depend on
    // it doing so.
    time.value = String(preset);
    time.addEventListener("change", () => {
      if (time.value === "custom") return;
      this.change({ since: time.value === "null" ? null : now - Number(time.value), until: null });
    });

    fill(this.controls, [time, this.groupToggle()]);
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
