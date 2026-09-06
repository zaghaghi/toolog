//! The item index: what sits at row N of the list.
//!
//! One block of rows, newest first. It was two shapes until v1.1 — the second
//! grouped rows under a session header and a sub-header per subagent (task
//! 5.10) — and the owner did not use it. The plan keeps its indirection anyway:
//! the block is what `RowStore` keys its pages on, and a list that fetches a
//! window at a time needs something to name the window.

import type { TimelineFilter, TimelineRow } from "./bindings";
import { queryTimeline } from "./bindings";

/** A contiguous run of rows, and the query that returns them. */
export interface RowBlock {
  /** Stable across rebuilds, so a collapse does not discard what was loaded. */
  key: string;
  filter: TimelineFilter;
  count: number;
}

export type Item = { kind: "row"; index: number; block: RowBlock; offset: number };

interface Entry {
  start: number;
  size: number;
  build: (index: number, start: number) => Item;
}

export class Plan {
  private readonly entries: Entry[] = [];
  total = 0;

  private push(size: number, build: (index: number, start: number) => Item): void {
    if (size <= 0) return;
    this.entries.push({ start: this.total, size, build });
    this.total += size;
  }

  /** The timeline: one block, newest first. */
  static flat(filter: TimelineFilter, count: number): Plan {
    const plan = new Plan();
    const block: RowBlock = { key: "all", filter, count };
    plan.push(count, (index) => ({ kind: "row", index, block, offset: index }));
    return plan;
  }

  /** The item at a list index, or `null` past the end. */
  at(index: number): Item | null {
    const entry = this.entryAt(index);
    return entry ? entry.build(index, entry.start) : null;
  }

  private entryAt(index: number): Entry | null {
    if (index < 0 || index >= this.total) return null;
    let lo = 0;
    let hi = this.entries.length - 1;
    while (lo <= hi) {
      const mid = (lo + hi) >> 1;
      const entry = this.entries[mid];
      if (!entry) break;
      if (index < entry.start) hi = mid - 1;
      else if (index >= entry.start + entry.size) lo = mid + 1;
      else return entry;
    }
    return null;
  }
}

// ---------------------------------------------------------------------------
// Rows, fetched a window at a time
// ---------------------------------------------------------------------------

/**
 * How many rows one round trip brings back.
 *
 * Large enough that an ordinary scroll rarely waits, small enough that a jump
 * into the middle of 100k rows does not drag 100k rows across the IPC bridge.
 */
const PAGE = 200;

/** Rows already fetched, keyed by block and page. */
export class RowStore {
  private readonly pages = new Map<string, TimelineRow[]>();
  private readonly inflight = new Set<string>();
  private readonly failed = new Map<string, string>();
  /** Bumped on every clear, so a late reply for an old filter is discarded. */
  private generation = 0;

  constructor(private readonly onArrival: () => void) {}

  /** Forget everything. Called when the filter changes. */
  clear(): void {
    this.generation += 1;
    this.pages.clear();
    this.inflight.clear();
    this.failed.clear();
  }

  /**
   * The row at an offset within a block, fetching its page if necessary.
   *
   * Returns `undefined` while the page is in flight — the caller draws a
   * placeholder rather than a gap, so the list never changes height under the
   * scrollbar (task 5.5).
   */
  get(block: RowBlock, offset: number): TimelineRow | undefined {
    const page = Math.floor(offset / PAGE);
    const id = `${block.key}#${page}`;
    const held = this.pages.get(id);
    if (held) return held[offset - page * PAGE];
    if (!this.inflight.has(id) && !this.failed.has(id)) void this.fetch(block, page, id);
    return undefined;
  }

  /** The error that stopped a page loading, if one did. */
  errorFor(block: RowBlock, offset: number): string | undefined {
    return this.failed.get(`${block.key}#${Math.floor(offset / PAGE)}`);
  }

  /** Try the failed pages again. */
  retry(): void {
    this.failed.clear();
    this.onArrival();
  }

  private async fetch(block: RowBlock, page: number, id: string): Promise<void> {
    this.inflight.add(id);
    const generation = this.generation;
    try {
      const rows = await queryTimeline(block.filter, { limit: PAGE, offset: page * PAGE });
      if (generation !== this.generation) return;
      this.pages.set(id, rows);
    } catch (error) {
      if (generation !== this.generation) return;
      this.failed.set(id, String(error));
    } finally {
      this.inflight.delete(id);
      if (generation === this.generation) this.onArrival();
    }
  }
}
