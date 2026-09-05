//! A virtualized list over a fixed row height.
//!
//! Task 5.3's target is a smooth scroll at 100k rows and a first paint under
//! 200 ms, which rules out putting the table in the DOM and rules out asking
//! the WebView to measure anything. Every item is one row high — rows, session
//! headers and agent headers alike — so "which item is at this scroll offset?"
//! is a division rather than a walk, and the list can jump to item 90,000
//! without having drawn the 89,999 above it.
//!
//! Nodes are rebuilt only when the visible window moves. A scroll that stays
//! inside the rendered band does nothing at all.

export interface VirtualOptions {
  /** Height of one item, in CSS pixels. Must match `--row-h`. */
  rowHeight: number;
  /** Extra items rendered above and below the viewport. */
  overscan?: number;
  /** Build the element for one index. */
  render: (index: number) => HTMLElement;
  /** Called after each paint with the first fully visible index. */
  onVisible?: (first: number, last: number) => void;
}

export class VirtualList {
  readonly viewport: HTMLElement;
  private readonly canvas: HTMLElement;
  private readonly options: Required<Omit<VirtualOptions, "onVisible">> &
    Pick<VirtualOptions, "onVisible">;
  private total = 0;
  private first = -1;
  private last = -1;
  private frame = 0;
  /** The nodes currently mounted, by item index. */
  private mounted = new Map<number, HTMLElement>();

  constructor(viewport: HTMLElement, options: VirtualOptions) {
    this.viewport = viewport;
    this.options = { overscan: 8, ...options };
    this.canvas = document.createElement("div");
    this.canvas.className = "vl-canvas";
    this.viewport.replaceChildren(this.canvas);
    this.viewport.addEventListener("scroll", () => this.schedule(), { passive: true });
    new ResizeObserver(() => this.schedule()).observe(this.viewport);
  }

  /** How many items the list holds. Resets the rendered window. */
  setTotal(total: number): void {
    this.total = Math.max(0, total);
    this.canvas.style.height = `${this.total * this.options.rowHeight}px`;
    this.reset();
  }

  get count(): number {
    return this.total;
  }

  /** Drop every rendered node and draw the visible window again. */
  reset(): void {
    this.first = -1;
    this.last = -1;
    this.mounted.clear();
    this.canvas.replaceChildren();
    this.draw();
  }

  /** Redraw one item in place, if it is on screen. */
  refresh(index: number): void {
    const existing = this.mounted.get(index);
    if (!existing) return;
    const next = this.build(index);
    existing.replaceWith(next);
    this.mounted.set(index, next);
  }

  /** Redraw every mounted item. Cheaper than `reset` — no scroll jump. */
  refreshAll(): void {
    for (const index of [...this.mounted.keys()]) this.refresh(index);
  }

  /** Bring an item into view, scrolling as little as possible. */
  scrollTo(index: number): void {
    const { rowHeight } = this.options;
    const top = index * rowHeight;
    const bottom = top + rowHeight;
    const viewTop = this.viewport.scrollTop;
    const viewBottom = viewTop + this.viewport.clientHeight;
    if (top < viewTop) this.viewport.scrollTop = top;
    else if (bottom > viewBottom) this.viewport.scrollTop = bottom - this.viewport.clientHeight;
    this.draw();
  }

  private schedule(): void {
    if (this.frame !== 0) return;
    this.frame = requestAnimationFrame(() => {
      this.frame = 0;
      this.draw();
    });
  }

  private build(index: number): HTMLElement {
    const node = this.options.render(index);
    node.style.position = "absolute";
    node.style.top = `${index * this.options.rowHeight}px`;
    node.style.left = "0";
    node.style.right = "0";
    return node;
  }

  private draw(): void {
    const { rowHeight, overscan, onVisible } = this.options;
    const height = this.viewport.clientHeight || rowHeight;
    const firstVisible = Math.floor(this.viewport.scrollTop / rowHeight);
    const visibleCount = Math.ceil(height / rowHeight);

    const first = Math.max(0, firstVisible - overscan);
    const last = Math.min(this.total - 1, firstVisible + visibleCount + overscan);

    if (first === this.first && last === this.last) return;
    this.first = first;
    this.last = last;

    // Drop what has scrolled out, then add what has scrolled in. Reusing the
    // band this way keeps a one-row scroll to one node created and one removed.
    for (const [index, node] of this.mounted) {
      if (index < first || index > last) {
        node.remove();
        this.mounted.delete(index);
      }
    }
    const fragment = document.createDocumentFragment();
    for (let index = first; index <= last; index += 1) {
      if (this.mounted.has(index)) continue;
      const node = this.build(index);
      this.mounted.set(index, node);
      fragment.appendChild(node);
    }
    this.canvas.appendChild(fragment);

    onVisible?.(firstVisible, Math.min(this.total - 1, firstVisible + visibleCount));
  }
}
