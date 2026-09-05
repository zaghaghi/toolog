import { defineConfig } from "vitest/config";

// There is no dev server. The bundle is compiled into the binary
// (`frontendDist` in tauri.conf.json), so `just run` rebuilds it and starts the
// application — one artifact, per ADR-0007, rather than a window pointed at a
// localhost server that only exists on a developer's machine.

// The window is served from disk in a WebView, not over a network, so there is
// nothing to gain from splitting the bundle and nothing to preload. Every
// module here is statically imported, so Rollup emits one chunk without being
// asked. `base: "./"` keeps asset references relative, which is what the
// `tauri://` scheme needs.
export default defineConfig({
  base: "./",
  clearScreen: false,
  build: {
    outDir: "dist",
    emptyOutDir: true,
    target: "safari15",
    // The WebView is the user's own machine; a sourcemap costs nothing to ship
    // and turns a stack trace in a log file into something readable.
    sourcemap: true,
  },
  test: {
    // The view layer is DOM code, so the tests run against one. `happy-dom` is
    // enough: nothing here measures layout, because the virtual list is
    // deliberately built on arithmetic rather than on measurement.
    environment: "happy-dom",
    include: ["src/**/*.test.ts"],
  },
});
