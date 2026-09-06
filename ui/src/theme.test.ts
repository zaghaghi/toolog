//! The theme switch, and the duplication it forced in the token file.

import { beforeEach, describe, expect, test } from "vitest";

import { applyTheme, currentTheme, setTheme, THEMES } from "./theme";

beforeEach(() => {
  localStorage.clear();
  document.documentElement.removeAttribute("data-theme");
});

describe("choosing a theme", () => {
  test("follows the system until told otherwise", () => {
    expect(currentTheme()).toBe("system");
    applyTheme("system");
    // Absent, not "system": the stylesheet's dark block is a media query, and
    // an absent attribute is what lets it decide.
    expect(document.documentElement.hasAttribute("data-theme")).toBe(false);
  });

  test("an explicit choice is written to the root and remembered", () => {
    setTheme("dark");
    expect(document.documentElement.getAttribute("data-theme")).toBe("dark");
    expect(currentTheme()).toBe("dark");

    setTheme("light");
    expect(document.documentElement.getAttribute("data-theme")).toBe("light");
    expect(currentTheme()).toBe("light");
  });

  test("going back to System takes the attribute off again", () => {
    setTheme("dark");
    setTheme("system");
    expect(document.documentElement.hasAttribute("data-theme")).toBe(false);
    expect(currentTheme()).toBe("system");
  });

  test("a stored value that is not a theme is ignored", () => {
    // Whatever else is in this key — an old build's value, something a person
    // typed into devtools — must not become an attribute the CSS reacts to.
    localStorage.setItem("toolog.theme", "solarized");
    expect(currentTheme()).toBe("system");
  });

  test("storage that throws does not take the window with it", () => {
    const real = Storage.prototype.setItem;
    Storage.prototype.setItem = () => {
      throw new Error("site data is blocked");
    };
    try {
      expect(() => setTheme("dark")).not.toThrow();
      // Applied for this session even though it could not be written down.
      expect(document.documentElement.getAttribute("data-theme")).toBe("dark");
    } finally {
      Storage.prototype.setItem = real;
    }
  });

  test("offers System first, because it is the default", () => {
    expect(THEMES[0]).toBe("system");
  });
});
