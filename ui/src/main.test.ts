//! The boot path: the window comes up, and the URL decides what it shows.
//!
//! Small, but it is the one test that would have caught "the window is blank",
//! which is the failure a bundled frontend fails with.

import { beforeEach, describe, expect, test, vi } from "vitest";

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(() => Promise.resolve(() => {})),
}));

vi.mock("./bindings", () => ({
  queryTimeline: vi.fn(() => Promise.resolve([])),
  timelineCount: vi.fn(() => Promise.resolve(0)),
  timelineGroups: vi.fn(() => Promise.resolve([])),
  facets: vi.fn(() =>
    Promise.resolve({
      projects: [],
      tools: [],
      decision_sources: [],
      permission_modes: [],
      agents: [],
    }),
  ),
  collectorStatus: vi.fn(() => Promise.resolve({ listening: true, paused: false })),
  getToolCall: vi.fn(() => Promise.resolve(null)),
  getSource: vi.fn(() => Promise.resolve(null)),
  revealTranscript: vi.fn(() => Promise.resolve(null)),
  saveExport: vi.fn(() => Promise.resolve(null)),
  doctorStatus: vi.fn(() =>
    Promise.resolve({
      configured: true,
      listening: true,
      endpoint: "http://127.0.0.1:47318",
      settings_path: "/tmp/settings.json",
      transcripts_dir: "/tmp/projects",
      transcript_files: 39,
      ingested_files: 39,
      agent_supported: true,
      agent_installed: false,
      problems: [],
      report: "all good",
    }),
  ),
  applyDoctorFix: vi.fn(),
  runBackfill: vi.fn(),
  setPaused: vi.fn(),
  setLoginAgent: vi.fn(),
  revealLogs: vi.fn(),
}));

async function settle(times = 8): Promise<void> {
  for (let i = 0; i < times; i += 1) {
    await Promise.resolve();
    await new Promise((resolve) => setTimeout(resolve, 0));
  }
}

beforeEach(() => {
  document.body.replaceChildren();
  vi.resetModules();
});

describe("the window", () => {
  test("mounts the timeline into #app and stops saying 'Starting…'", async () => {
    document.body.append(
      Object.assign(document.createElement("div"), { id: "app", className: "loading" }),
    );
    await import("./main");
    await settle();

    const app = document.getElementById("app")!;
    expect(app.className).toBe("");
    expect(app.querySelector(".tabs")).not.toBeNull();
    expect(app.querySelector(".timeline")).not.toBeNull();
    expect(app.textContent).not.toContain("Starting…");
  });

  test("offers three tabs — the two Phase 9 removed are not among them", async () => {
    document.body.append(
      Object.assign(document.createElement("div"), { id: "app", className: "loading" }),
    );
    await import("./main");
    await settle();

    const tabs = [...document.querySelectorAll(".tabs .tab")].map((t) => t.textContent);
    expect(tabs).toEqual(["Timeline", "Risk", "Status"]);
  });

  test("a hash asking for the status screen opens it instead", async () => {
    location.hash = "#v=setup";
    document.body.append(
      Object.assign(document.createElement("div"), { id: "app", className: "loading" }),
    );
    await import("./main");
    await settle();

    const app = document.getElementById("app")!;
    expect(app.querySelector(".setup")).not.toBeNull();
    expect(app.querySelector(".timeline")).toBeNull();
    location.hash = "";
  });
});
