//! The timeline: the forensic view (tasks 5.3–5.13).
//!
//! Everything below hangs off one `ViewState`. A change to it — typing in the
//! search box, choosing a project, collapsing a session, following a link with a
//! hash — rebuilds the item plan, resizes the list and lets the virtualizer ask
//! for the rows it can actually see. Nothing else fetches anything.

import type { SessionGroup, ToolCall } from "./bindings";
import { collectorStatus, facets, timelineCount, timelineGroups } from "./bindings";
import { DetailPane } from "./detail";
import { el, fill, span } from "./dom";
import { ActivityHistogram } from "./histogram";
import { count, dayLabel } from "./format";
import { FilterBar } from "./filterbar";
import { agentKey, Plan, RowStore, sessionKey } from "./plan";
import { renderAgentHeader, renderPending, renderRow, renderSessionHeader } from "./row";
import type { RowContext } from "./row";
import { VirtualList } from "./virtual";
import type { ViewState } from "./view";
import { emptyFilter, isFiltered, sameFilter } from "./view";

/** Must match `--row-h` in tokens.css. */
const ROW_HEIGHT = 28;

export interface TimelineOptions {
  onViewChange: (view: ViewState) => void;
  onNotice: (message: string) => void;
  onOpenSetup: () => void;
}

export class TimelineView {
  readonly node = el("div", { class: "timeline" });

  private view: ViewState;
  private readonly bar: FilterBar;
  private readonly viewport = el("div", { class: "viewport", role: "listbox", attrs: { tabindex: "0" } });
  private readonly context = el("div", { class: "context" });
  private readonly state = el("div", { class: "state", hidden: true });
  private readonly banner = el("div", { class: "banner", hidden: true });
  private readonly newCalls = el("button", { class: "newpill", hidden: true });
  private readonly detail: DetailPane;
  private readonly histogram: ActivityHistogram;
  private readonly list: VirtualList;
  private readonly store: RowStore;

  private plan = Plan.flat(emptyFilter(), 0);
  /** How many calls the current filter matches, from `timeline_count`. */
  private total = 0;
  private groups: SessionGroup[] = [];
  private collapsed = new Set<string>();
  private cursor = -1;
  private readonly pending = new Set<string>();
  /** Bumped per reload so a slow count for an old filter is ignored. */
  private generation = 0;

  constructor(
    view: ViewState,
    private readonly options: TimelineOptions,
  ) {
    this.view = view;
    this.store = new RowStore(() => this.list.refreshAll());

    this.bar = new FilterBar(view, {
      onChange: (next) => this.apply(next, true),
      onNotice: options.onNotice,
    });

    const pane = el("aside", { class: "pane" });
    this.detail = new DetailPane(
      pane,
      (patch) =>
        this.apply(
          { ...this.view, selected: null, filter: { ...this.view.filter, ...patch } },
          true,
        ),
      () => this.closeDetail(),
    );

    // The chart writes absolute bounds into the same filter the list reads, so
    // the two are one question asked twice rather than two questions.
    this.histogram = new ActivityHistogram({
      onRange: (since, until) =>
        this.apply(
          { ...this.view, selected: null, filter: { ...this.view.filter, since, until } },
          true,
        ),
    });

    this.node.append(
      this.bar.node,
      this.banner,
      this.histogram.node,
      el("div", { class: "split" }, [
        el("div", { class: "listwrap" }, [this.context, this.viewport, this.state, this.newCalls]),
        pane,
      ]),
    );

    this.list = new VirtualList(this.viewport, {
      rowHeight: ROW_HEIGHT,
      render: (index) => this.renderItem(index),
      onVisible: (first) => this.updateContext(first),
    });

    this.viewport.addEventListener("click", (event) => this.onClick(event));
    this.viewport.addEventListener("keydown", (event) => this.onKey(event));
    this.newCalls.addEventListener("click", () => {
      this.pending.clear();
      this.newCalls.hidden = true;
      void this.reload();
    });

    void this.loadFacets();
    void this.checkCollector();
    void this.reload();
  }

  /** Apply a new view. `push` writes it to the URL. */
  apply(next: ViewState, push: boolean): void {
    const filterChanged = !sameFilter(next.filter, this.view.filter);
    const groupingChanged = next.grouped !== this.view.grouped;
    const previous = this.view;
    this.view = next;
    this.bar.setView(next);
    if (push) this.options.onViewChange(next);

    if (filterChanged || groupingChanged) {
      this.collapsed.clear();
      void this.reload();
      return;
    }
    if (next.selected !== previous.selected) this.showSelected();
  }

  focusSearch(): void {
    this.bar.focusSearch();
  }

  /**
   * A call has just been stored (the task 6.9 event stream).
   *
   * Counted by `tool_use_id`, not by arrivals: the same call reaches here more
   * than once by design — the transcript creates the row and OTEL completes it
   * with a duration and a decision — and "3 new calls" for one command would
   * be a number the list then failed to produce.
   */
  noteLiveCall(call: ToolCall): void {
    // Only offer to refresh when the new row would actually appear. A filtered
    // view that would not show it should not blink at the user.
    if (isFiltered(this.view.filter) && this.view.filter.query !== null) return;
    if (this.view.filter.tool_name !== null && this.view.filter.tool_name !== call.tool_name) return;
    if (this.pending.has(call.tool_use_id)) return;
    this.pending.add(call.tool_use_id);
    const n = this.pending.size;
    this.newCalls.textContent = `${count(n)} new ${n === 1 ? "call" : "calls"}`;
    this.newCalls.hidden = false;
  }

  // -------------------------------------------------------------- loading

  private async loadFacets(): Promise<void> {
    try {
      this.bar.setFacets(await facets());
    } catch {
      // The filter controls degrade to "all"; the list still works.
    }
  }

  private async checkCollector(): Promise<void> {
    try {
      const status = await collectorStatus();
      if (status.listening && !status.paused) {
        this.banner.hidden = true;
        return;
      }
      const open = el("button", { class: "link", text: "Open Status" });
      open.addEventListener("click", () => this.options.onOpenSetup());
      fill(this.banner, [
        span(
          "btext",
          status.paused
            ? "Capture is paused. Calls made now are not being recorded."
            : "The collector is not listening. Nothing new is being recorded.",
        ),
        open,
      ]);
      this.banner.hidden = false;
    } catch {
      this.banner.hidden = true;
    }
  }

  private async reload(): Promise<void> {
    this.generation += 1;
    const generation = this.generation;
    this.store.clear();
    this.cursor = -1;
    this.pending.clear();
    this.newCalls.hidden = true;
    this.bar.setStatus(0, true);
    this.showState(el("div", { class: "empty", text: "Loading…" }));

    try {
      const filter = this.view.filter;
      void this.histogram.load(filter);
      const [total, groups] = await Promise.all([
        timelineCount(filter),
        this.view.grouped ? timelineGroups(filter) : Promise.resolve([]),
      ]);
      if (generation !== this.generation) return;

      this.total = total;
      this.groups = groups;
      this.rebuild();
      this.bar.setStatus(total, false);

      if (total === 0) this.showEmpty();
      else this.hideState();
      this.showSelected();
    } catch (error) {
      if (generation !== this.generation) return;
      const retry = el("button", { class: "link", text: "Try again" });
      retry.addEventListener("click", () => void this.reload());
      this.showState(
        el("div", { class: "empty" }, [
          el("p", { class: "problem", text: String(error) }),
          retry,
        ]),
      );
    }
  }

  /** Rebuild the item plan from the current groups and collapse set. */
  private rebuild(): void {
    const filter = this.view.filter;
    this.plan = this.view.grouped
      ? Plan.grouped(filter, this.groups, this.collapsed)
      : Plan.flat(filter, this.total);
    this.list.setTotal(this.plan.total);
  }

  // -------------------------------------------------------------- drawing

  private get rowContext(): RowContext {
    const query = this.view.filter.query;
    return {
      terms:
        query === null
          ? []
          : query
              .split(/\s+/)
              .filter((t) => t.length > 0)
              .map((t) => t.toLowerCase()),
      selected: this.view.selected,
    };
  }

  private renderItem(index: number): HTMLElement {
    const item = this.plan.at(index);
    if (item === null) return el("div", { class: "row" });

    if (item.kind === "session") {
      const node = renderSessionHeader(item.group, item.collapsed);
      node.dataset["group"] = sessionKey(item.group);
      if (index === this.cursor) node.classList.add("cursor");
      return node;
    }
    if (item.kind === "agent") {
      const node = renderAgentHeader(item.group, item.agent, item.collapsed);
      node.dataset["group"] = agentKey(item.group, item.agent);
      if (index === this.cursor) node.classList.add("cursor");
      return node;
    }

    const row = this.store.get(item.block, item.offset);
    if (row === undefined) {
      return renderPending(item.block, this.store.errorFor(item.block, item.offset));
    }
    const node = renderRow(row, item.block, this.rowContext);
    if (index === this.cursor) node.classList.add("cursor");
    return node;
  }

  private updateContext(first: number): void {
    const item = this.plan.at(first);
    let left = "";
    if (item?.kind === "row") {
      const row = this.store.get(item.block, item.offset);
      if (row?.call.called_at != null) left = dayLabel(row.call.called_at);
    } else if (item?.kind === "session" && item.group.last_at !== null) {
      left = dayLabel(item.group.last_at);
    } else if (item?.kind === "agent" && item.agent.last_at !== null) {
      left = dayLabel(item.agent.last_at);
    }
    fill(this.context, [
      span("cleft", left),
      span("cright", this.view.grouped ? `${count(this.groups.length)} sessions` : ""),
    ]);
  }

  private showState(content: HTMLElement): void {
    fill(this.state, [content]);
    this.state.hidden = false;
  }

  private hideState(): void {
    this.state.hidden = true;
  }

  private showEmpty(): void {
    if (isFiltered(this.view.filter)) {
      const clear = el("button", { class: "link", text: "Clear the filters" });
      clear.addEventListener("click", () =>
        this.apply({ ...this.view, selected: null, filter: emptyFilter() }, true),
      );
      this.showState(
        el("div", { class: "empty" }, [
          el("p", { text: "No calls match these filters." }),
          clear,
        ]),
      );
      return;
    }
    const open = el("button", { class: "link", text: "Open Status to import history" });
    open.addEventListener("click", () => this.options.onOpenSetup());
    this.showState(
      el("div", { class: "empty" }, [
        el("p", { text: "Nothing has been captured yet." }),
        el("p", {
          class: "note",
          text: "Calls appear here as Claude Code makes them. Existing history has to be imported once.",
        }),
        open,
      ]),
    );
  }

  // ------------------------------------------------------------ interaction

  private onClick(event: MouseEvent): void {
    const target = event.target as HTMLElement | null;
    const header = target?.closest<HTMLElement>(".ghead");
    if (header) {
      const key = header.dataset["group"];
      if (key !== undefined) this.toggle(key);
      return;
    }
    // A page that failed to load says so in place of its rows; clicking one
    // asks for it again rather than making the whole view be reloaded.
    if (target?.closest(".row.broken")) {
      this.store.retry();
      return;
    }

    const row = target?.closest<HTMLElement>(".row");
    const id = row?.dataset["id"];
    if (id === undefined) return;
    // Clicking the row that is already open closes it — the third way out of
    // the pane (task 10.12), and the one a reader tries first.
    if (id === this.view.selected) this.closeDetail();
    else this.select(id, this.indexOfNode(row));
  }

  /**
   * Close the detail pane, from wherever the reader asked.
   *
   * All three routes come through here and all three clear `selected` from the
   * hash, so the back button undoes each of them.
   */
  private closeDetail(): void {
    if (this.view.selected === null) return;
    this.view = { ...this.view, selected: null };
    this.options.onViewChange(this.view);
    this.showSelected();
    this.list.refreshAll();
    this.viewport.focus();
  }

  private indexOfNode(node: HTMLElement | null | undefined): number {
    if (!node) return -1;
    const top = Number.parseInt(node.style.top || "-1", 10);
    return top < 0 ? -1 : Math.round(top / ROW_HEIGHT);
  }

  private toggle(key: string): void {
    if (this.collapsed.has(key)) this.collapsed.delete(key);
    else this.collapsed.add(key);
    this.rebuild();
  }

  private select(id: string, index: number): void {
    const previous = this.view.selected;
    this.view = { ...this.view, selected: id };
    this.cursor = index;
    this.options.onViewChange(this.view);
    this.node.classList.add("has-detail");
    this.list.refreshAll();
    if (previous !== id) this.detail.show(id);
  }

  private showSelected(): void {
    // In a narrow window the pane covers the list, so whether anything is
    // selected is a layout fact as well as a state one.
    this.node.classList.toggle("has-detail", this.view.selected !== null);
    if (this.view.selected === null) this.detail.clear();
    else this.detail.show(this.view.selected);
  }

  private onKey(event: KeyboardEvent): void {
    const keys = ["ArrowDown", "ArrowUp", "Home", "End", "Enter", " ", "Escape"];
    if (!keys.includes(event.key)) return;
    event.preventDefault();

    if (event.key === "Escape") {
      this.cursor = -1;
      this.closeDetail();
      this.list.refreshAll();
      return;
    }

    if (event.key === "Enter" || event.key === " ") {
      const item = this.plan.at(this.cursor);
      if (item?.kind === "session") this.toggle(sessionKey(item.group));
      else if (item?.kind === "agent") this.toggle(agentKey(item.group, item.agent));
      return;
    }

    const last = this.plan.total - 1;
    let next = this.cursor;
    if (event.key === "ArrowDown") next = Math.min(last, this.cursor + 1);
    else if (event.key === "ArrowUp") next = Math.max(0, this.cursor - 1);
    else if (event.key === "Home") next = 0;
    else if (event.key === "End") next = last;
    if (next === this.cursor || next < 0) return;

    this.cursor = next;
    this.list.scrollTo(next);
    this.list.refreshAll();

    // Stepping onto a row opens it, so the pane follows the cursor rather than
    // needing a second key for every call inspected.
    const item = this.plan.at(next);
    if (item?.kind === "row") {
      const row = this.store.get(item.block, item.offset);
      if (row !== undefined) this.select(row.call.tool_use_id, next);
    }
  }
}
