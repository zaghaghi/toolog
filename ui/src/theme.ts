//! Light, dark, or whatever the machine is set to.
//!
//! Three states, not two. "Dark" and "not dark" would make the third one — the
//! one almost everybody wants — unreachable: macOS switches appearance at
//! sunset, and a window that had to be told twice a day is a window that gets
//! told once and then left wrong.
//!
//! **Stored in `localStorage`, not in `Prefs`.** `Prefs` is a file the resident
//! process reads because it acts on what is in it — notifications fire from the
//! capture thread, redaction happens on the write path. Nothing outside this
//! window cares which theme it is drawn in. Keeping it here also means it is
//! applied *synchronously at boot*: a round trip to Rust would paint the wrong
//! theme first and correct it a frame later, every launch. The activity
//! histogram's collapsed state is remembered the same way, for the same reason.

/** The three states, in the order the control offers them. */
export const THEMES = ["system", "light", "dark"] as const;

export type Theme = (typeof THEMES)[number];

const KEY = "toolog.theme";

function isTheme(value: string | null): value is Theme {
  return value !== null && (THEMES as readonly string[]).includes(value);
}

/**
 * What the reader last chose, defaulting to following the machine.
 *
 * A private window, cleared site data or a browser set to block storage all
 * throw here rather than returning null, so the read is guarded: losing the
 * preference must not take the window with it, and "follow the system" is the
 * right thing to fall back to.
 */
export function currentTheme(): Theme {
  try {
    const stored = localStorage.getItem(KEY);
    return isTheme(stored) ? stored : "system";
  } catch {
    return "system";
  }
}

/**
 * Put the choice on the root element, where the tokens read it.
 *
 * `system` removes the attribute rather than setting it to anything: the
 * stylesheet's dark block is a `prefers-color-scheme` media query guarded
 * against an explicit *light*, so an absent attribute is exactly "let the
 * media query decide" — see `styles/tokens.css`.
 */
export function applyTheme(theme: Theme): void {
  const root = document.documentElement;
  if (theme === "system") root.removeAttribute("data-theme");
  else root.setAttribute("data-theme", theme);
}

/** Choose, apply and remember. */
export function setTheme(theme: Theme): void {
  applyTheme(theme);
  try {
    localStorage.setItem(KEY, theme);
  } catch {
    // Applied for this session; not remembered for the next one. Better than
    // refusing to change the theme because it could not be written down.
  }
}
