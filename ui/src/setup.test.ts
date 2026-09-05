//! The status page's uninstall section (task 8.6).
//!
//! The one control in the window that destroys something, so what is asserted
//! here is not that it works but that it is **hard to trigger by accident**:
//! folded away, costing nothing until opened, with history kept unless a
//! separate box is ticked, and never acting on a preview the disk has moved
//! on from.

import { beforeEach, describe, expect, test, vi } from "vitest";

const doctor = {
  configured: true,
  listening: true,
  endpoint: "http://127.0.0.1:47318",
  settings_path: "/home/u/.claude/settings.json",
  transcripts_dir: "/home/u/.claude/projects",
  transcript_files: 39,
  ingested_files: 39,
  agent_supported: true,
  agent_installed: true,
  problems: [],
  report: "all good",
};

const uninstallPreview = vi.fn((deleteData: boolean) =>
  Promise.resolve({
    report: deleteData ? "PREVIEW WITH DELETE" : "PREVIEW KEEPING HISTORY",
    any_changes: true,
    data_bytes: deleteData ? 160_432_128 : 160_432_128,
    data_dir: "/home/u/Library/Application Support/toolog",
    restores_backup: true,
  }),
);
// Annotated rather than inferred: an empty `failed` would otherwise be typed
// `never[]`, and the failure case below could not be written at all.
const uninstallRun = vi.fn(
  (_deleteData: boolean): Promise<{ done: string[]; failed: string[] }> =>
    Promise.resolve({ done: ["Removed the login agent"], failed: [] }),
);

vi.mock("./bindings", () => ({
  doctorStatus: vi.fn(() => Promise.resolve(doctor)),
  collectorStatus: vi.fn(() =>
    Promise.resolve({
      listening: true,
      paused: false,
      endpoint: "http://127.0.0.1:47318",
      events_today: 4,
      tool_calls: 10,
      counters: { batches: 1, records: 4, dropped: 0, rejected_bodies: 0 },
    }),
  ),
  getPrefs: vi.fn(() =>
    Promise.resolve({
      notify_refusals: false,
      notify_high_risk: false,
      redact_evidence: false,
      excluded_projects: [],
    }),
  ),
  setPrefs: vi.fn(),
  applyDoctorFix: vi.fn(),
  runBackfill: vi.fn(),
  setLoginAgent: vi.fn(),
  setPaused: vi.fn(),
  revealLogs: vi.fn(),
  uninstallPreview,
  uninstallRun,
}));

const { SetupView } = await import("./setup");

/** Let the view's pending promises settle. */
const settle = (): Promise<void> => new Promise((resolve) => setTimeout(resolve, 0));

async function mount(): Promise<HTMLElement> {
  const view = new SetupView({ onNotice: () => {}, onChanged: () => {} });
  await settle();
  return view.node;
}

function section(node: HTMLElement): HTMLDetailsElement {
  const details = node.querySelector<HTMLDetailsElement>("details.uninstall");
  if (details === null) throw new Error("no uninstall section");
  return details;
}

describe("the uninstall section", () => {
  beforeEach(() => {
    uninstallPreview.mockClear();
    uninstallRun.mockClear();
  });

  test("is closed, and costs nothing, until it is opened", async () => {
    const node = await mount();
    const details = section(node);

    expect(details.open).toBe(false);
    expect(uninstallPreview).not.toHaveBeenCalled();

    details.open = true;
    details.dispatchEvent(new Event("toggle"));
    await settle();

    expect(uninstallPreview).toHaveBeenCalledTimes(1);
    expect(details.querySelector("pre.report")?.textContent).toBe("PREVIEW KEEPING HISTORY");
  });

  test("keeps recorded history unless a separate box is ticked", async () => {
    const node = await mount();
    const details = section(node);
    details.open = true;
    details.dispatchEvent(new Event("toggle"));
    await settle();

    const box = details.querySelector<HTMLInputElement>("input[type=checkbox]");
    expect(box?.checked).toBe(false);
    expect(uninstallPreview).toHaveBeenLastCalledWith(false);

    // Ticking it re-asks, so the text on screen describes what would happen.
    box!.checked = true;
    box!.dispatchEvent(new Event("change"));
    await settle();

    expect(uninstallPreview).toHaveBeenLastCalledWith(true);
    expect(details.querySelector("pre.report")?.textContent).toBe("PREVIEW WITH DELETE");
  });

  test("acts on the disk's state, not on the preview the window is holding", async () => {
    const node = await mount();
    const details = section(node);
    details.open = true;
    details.dispatchEvent(new Event("toggle"));
    await settle();

    details.querySelector<HTMLButtonElement>("button.danger")?.click();
    await settle();

    // The flag, and nothing else: the backend recomputes the plan, so a stale
    // preview cannot be replayed against a store that has since changed.
    expect(uninstallRun).toHaveBeenCalledWith(false);
    expect(details.textContent).toContain("Removed the login agent");
  });

  test("reports a step that failed rather than claiming a clean removal", async () => {
    uninstallRun.mockImplementationOnce(() =>
      Promise.resolve({ done: ["Removed the login agent"], failed: ["Could not write settings"] }),
    );
    const node = await mount();
    const details = section(node);
    details.open = true;
    details.dispatchEvent(new Event("toggle"));
    await settle();

    details.querySelector<HTMLButtonElement>("button.danger")?.click();
    await settle();

    const problem = details.querySelector(".problem");
    expect(problem?.textContent).toBe("Could not write settings");
  });
});
