//! The item index: what sits at row N of the list.
//!
//! A flat timeline is one block of rows. A grouped one (task 5.10) is a
//! sequence of session headers, each followed by its main-thread rows and then
//! by one sub-header and block per subagent — so a subagent's calls read as
//! nested under the agent that made them rather than flattened into the thread
//! that spawned it.
//!
//! Every group's size is known before a single row of it is fetched, which is
//! what makes collapsing free: the plan is rebuilt, the list's height changes,
//! and nothing is loaded that is not on screen.

import type { AgentGroup, SessionGroup, TimelineFilter, TimelineRow } from "./bindings";
import { queryTimeline } from "./bindings";

/** A contiguous run of rows, and the query that returns them. */
export interface RowBlock {
  /** Stable across rebuilds, so a collapse does not discard what was loaded. */
  key: string;
  filter: TimelineFilter;
  count: number;
  /** Nesting depth: 0 for the main thread, 1 for a subagent's calls. */
  indent: number;
}

export type Item =
  | { kind: "session"; index: number; group: SessionGroup; collapsed: boolean }
  | { kind: "agent"; index: number; group: SessionGroup; agent: AgentGroup; collapsed: boolean }
  | { kind: "row"; index: number; block: RowBlock; offset: number };

interface Entry {
  start: number;
  size: number;
  build: (index: number, start: number) => Item;
}

export function sessionKey(group: SessionGroup): string {
  return `s:${group.session_id ?? "-none-"}`;
}

export function agentKey(group: SessionGroup, agent: AgentGroup): string {
  return `${sessionKey(group)}/a:${agent.agent_id}`;
}

export class Plan {
  private readonly entries: Entry[] = [];
  total = 0;

  private push(size: number, build: (index: number, start: number) => Item): void {
    if (size <= 0) return;
    this.entries.push({ start: this.total, size, build });
    this.total += size;
  }

  /** The ungrouped timeline: one block, newest first. */
  static flat(filter: TimelineFilter, count: number): Plan {
    const plan = new Plan();
    const block: RowBlock = { key: "all", filter, count, indent: 0 };
    plan.push(count, (index) => ({ kind: "row", index, block, offset: index }));
    return plan;
  }

  /** Sessions and subagents, with `collapsed` holding the keys that are shut. */
  static grouped(
    filter: TimelineFilter,
    groups: SessionGroup[],
    collapsed: ReadonlySet<string>,
  ): Plan {
    const plan = new Plan();

    for (const group of groups) {
      const key = sessionKey(group);
      const shut = collapsed.has(key);
      plan.push(1, (index) => ({ kind: "session", index, group, collapsed: shut }));
      if (shut) continue;

      // Every block of this session is the current filter plus "in this
      // session", so narrowing the timeline narrows the groups with it.
      const scope: TimelineFilter = {
        ...filter,
        session_id: group.session_id,
        session_unknown: group.session_id === null ? true : null,
      };

      const main: RowBlock = {
        key: `${key}/main`,
        filter: { ...scope, main_thread: true },
        count: group.main_thread_calls,
        indent: 0,
      };
      plan.push(main.count, (index, start) => ({
        kind: "row",
        index,
        block: main,
        offset: index - start,
      }));

      for (const agent of group.agents) {
        const aKey = agentKey(group, agent);
        const aShut = collapsed.has(aKey);
        plan.push(1, (index) => ({ kind: "agent", index, group, agent, collapsed: aShut }));
        if (aShut) continue;

        const rows: RowBlock = {
          key: `${aKey}/rows`,
          filter: { ...scope, agent_id: agent.agent_id, main_thread: null },
          count: agent.calls,
          indent: 1,
        };
        plan.push(rows.count, (index, start) => ({
          kind: "row",
          index,
          block: rows,
          offset: index - start,
        }));
      }
    }

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
