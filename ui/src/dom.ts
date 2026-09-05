//! Element construction, without a framework.
//!
//! ADR-0001 rules out a component library for four views over a table. What is
//! actually needed is a terse way to build a subtree and a disciplined way to
//! put text into it — every string here comes from a shell command or a file
//! path, so nothing is ever assigned through `innerHTML`.

type Child = Node | string | null | undefined | false;

interface Attrs {
  class?: string;
  text?: string;
  title?: string;
  id?: string;
  role?: string;
  type?: string;
  value?: string;
  placeholder?: string;
  href?: string;
  disabled?: boolean;
  hidden?: boolean;
  /** `data-*` and `aria-*` attributes, written through verbatim. */
  attrs?: Record<string, string>;
  /**
   * Geometry, applied through the CSSOM rather than as a `style` attribute.
   *
   * Not a stylistic preference: the window runs under a Content Security
   * Policy with `style-src 'self'` and no `unsafe-inline`, so a `style`
   * *attribute* is discarded — silently, with the element rendering at its
   * natural size. Assigning properties on `element.style` is not an inline
   * style in the CSP's sense and is honoured. Everything that is not geometry
   * belongs in a class.
   */
  style?: Record<string, string>;
  on?: Partial<{
    [K in keyof HTMLElementEventMap]: (event: HTMLElementEventMap[K]) => void;
  }>;
}

export function el<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  attrs: Attrs = {},
  children: Child | Child[] = [],
): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  const { attrs: extra, on, text, style, ...rest } = attrs;

  for (const [key, value] of Object.entries(rest)) {
    if (value === undefined || value === false) continue;
    if (key === "class") node.className = value as string;
    else if (key === "value" && "value" in node) (node as HTMLInputElement).value = value as string;
    else if (value === true) node.setAttribute(key, "");
    else node.setAttribute(key, String(value));
  }
  if (text !== undefined) node.textContent = text;
  for (const [key, value] of Object.entries(extra ?? {})) node.setAttribute(key, value);
  for (const [key, value] of Object.entries(style ?? {})) node.style.setProperty(key, value);
  for (const [event, handler] of Object.entries(on ?? {})) {
    node.addEventListener(event, handler as EventListener);
  }
  append(node, children);
  return node;
}

export function append(parent: Node, children: Child | Child[]): void {
  for (const child of Array.isArray(children) ? children : [children]) {
    if (child === null || child === undefined || child === false) continue;
    parent.appendChild(typeof child === "string" ? document.createTextNode(child) : child);
  }
}

/** Replace an element's children in one operation. */
export function fill(parent: Element, children: Child | Child[]): void {
  parent.replaceChildren();
  append(parent, children);
}

/** A `<span>`, the shape most of this interface is made of. */
export function span(className: string, text: string, title?: string): HTMLSpanElement {
  return el("span", title === undefined ? { class: className, text } : { class: className, text, title });
}

/** Show `dash` in place of a value the store does not have (task 5.5). */
export function orDash(value: string | null | undefined, dash = "—"): string {
  return value === null || value === undefined || value === "" ? dash : value;
}
