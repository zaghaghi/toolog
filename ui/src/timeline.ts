//! The timeline: the forensic view (tasks 5.3–5.13).
//!
//! Everything below hangs off one `ViewState`. A change to it — typing in the
//! box, brushing the histogram, following a link with a hash — rebuilds the
//! item plan, resizes the list and lets the virtualizer ask for the rows it can
//! actually see. Nothing else fetches anything.
//!
//! The list is one flat run of rows. Grouping by session and subagent (task
//! 5.10) was built, lived with, and removed: the collapse state, the group
//! headers and the per-group queries all went with it. A subagent's calls are
//! still identifiable — the row says which agent made them — they are simply
//! not nested any more.

import type { ToolCall } from "./bindings";
import { collectorStatus, facets, timelineCount } from "./bindings";
import { DetailPane } from "./detail";
import { el, fill, span } from "./dom";
import { ActivityHistogram } from "./histogram";
import { count, dayLabel } from "./format";
import { FilterBar } from "./filterbar";
import { Plan, RowStore } from "./plan";
import { renderPending, renderRow } from "./row";
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
  /**
   * The column header, which is also the day marker.
   *
   * One strip, not two. The row above the list was already there, carrying the
   * day of the topmost visible row and nothing else; giving the other eight
   * columns their names in the same 22px costs no height at all. And the day
   * belongs over the time column anyway — `Sep 6` above `17:02:43` reads as the
   * label that column would have had.
   *
   * It sits outside the scrolling viewport, so it never scrolls away and never
   * has to be re-rendered as rows arrive. Only the date cell changes.
   */
  private readonly context = el("div", { class: "context", attrs: { role: "row" } });
  private readonly headDate = el("span", { class: "chd chd-date" });
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
  private cursor = -1;
  /** The topmost visible row, for the header's date cell. */
  private firstVisible = 0;
  private readonly pending = new Set<string>();
  /** Bumped per reload so a slow count for an old filter is ignored. */
  private generation = 0;

  constructor(
    view: ViewState,
    private readonly options: TimelineOptions,
  ) {
    this.view = view;
    // A page arriving refreshes the rows *and* the header's date cell: the
    // first paint happens before any row exists, so `onVisible` ran once with
    // nothing to read the day from and never ran again — which left the header
    // starting with an empty cell.
    this.store = new RowStore(() => {
      this.list.refreshAll();
      this.updateContext(this.firstVisible);
    });

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

    this.drawHeader();

    this.list = new VirtualList(this.viewport, {
      rowHeight: ROW_HEIGHT,
      render: (index) => this.renderItem(index),
      onVisible: (first) => {
        this.firstVisible = first;
        this.updateContext(first);
      },
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
    const previous = this.view;
    this.view = next;
    this.bar.setView(next);
    if (push) this.options.onViewChange(next);

    if (filterChanged) {
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
      const total = await timelineCount(filter);
      if (generation !== this.generation) return;

      this.total = total;
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

  /** Rebuild the item plan for the current filter. */
  private rebuild(): void {
    this.plan = Plan.flat(this.view.filter, this.total);
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

    const row = this.store.get(item.block, item.offset);
    if (row === undefined) {
      return renderPending(item.block, this.store.errorFor(item.block, item.offset));
    }
    const node = renderRow(row, item.block, this.rowContext);
    if (index === this.cursor) node.classList.add("cursor");
    return node;
  }

  /**
   * The column names, in the row's own grid so they line up with it.
   *
   * Built once. The outcome column is 18px of glyph and no word fits in it, so
   * its cell is empty and carries its name as a tooltip — a header that forced
   * the column wide enough to be labelled would cost every row the space.
   */
  private drawHeader(): void {
    // Header cells get their own classes rather than the row's. Sharing them
    // looked like a way to keep the two in step and instead gave the word
    // "Tool" the tool badge's grey pill.
    const head = (label: string, cls = "", title = ""): HTMLElement =>
      span(`chd ${cls}`.trim(), label, title);
    fill(this.context, [
      this.headDate,
      head("Project"),
      head("Tool"),
      head("", "", "Outcome: succeeded, failed, or refused"),
      head("Input", "", "The command a call ran, or the file it touched"),
      head("Risk", "chd-end", "The worst severity a live rule gives this call"),
      head("Model", "chd-end", "What a local model scored it, 1 to 5 — advisory, not a rule"),
      head("Took", "chd-end"),
      head("Decided by", "chd-end", "What allowed or refused the call"),
    ]);
  }

  private updateContext(first: number): void {
    const item = this.plan.at(first);
    let day = "";
    if (item !== null) {
      const row = this.store.get(item.block, item.offset);
      if (row?.call.called_at != null) day = dayLabel(row.call.called_at);
    }
    this.headDate.textContent = day;
    // 62px holds `04:44:59`, not `Today · Sep 6`, so the long form is the
    // tooltip and the cell ellipsizes. Widening the column would cost every
    // row the space to spell out a date that changes as you scroll.
    this.headDate.title = day;
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
    const keys = ["ArrowDown", "ArrowUp", "Home", "End", "Escape"];
    if (!keys.includes(event.key)) return;
    event.preventDefault();

    if (event.key === "Escape") {
      this.cursor = -1;
      this.closeDetail();
      this.list.refreshAll();
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
    if (item !== null) {
      const row = this.store.get(item.block, item.offset);
      if (row !== undefined) this.select(row.call.tool_use_id, next);
    }
  }
}
