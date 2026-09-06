//! The status page's uninstall section (task 8.6) and its notification
//! switches (task 6.12, moved here by task 9.3).
//!
//! What is asserted about uninstall is not that it works but that it is **hard
//! to trigger by accident**: folded away, costing nothing until opened, with
//! history kept unless a separate box is ticked, and never acting on a preview
//! the disk has moved on from.
//!
//! The switches carry the assertions the deleted live view held: both start
//! off, and turning one on sends the whole `Prefs` back rather than a patch —
//! it is the resident process that acts on them, and it reads the file.

import { beforeEach, describe, expect, test, vi } from "vitest";

import type { LlmReport } from "./bindings";

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

interface Prefs {
  notify_refusals: boolean;
  notify_high_risk: boolean;
  redact_evidence: boolean;
  excluded_projects: string[];
}
const setPrefs = vi.fn((prefs: Prefs) => Promise.resolve(prefs));

/** What `llmReport` answers. Replaced per test; `null` is "could not be read". */
let llmModel: LlmReport | null = noModel();
const setModelCalls: (string | null)[] = [];
const pausedCalls: boolean[] = [];

/** A machine where nobody has configured a model — the default state. */
function noModel(): LlmReport {
  return {
    model: {
      supported: true,
      path: null,
      file: null,
      summary: null,
      problem: null,
      loaded: false,
      suggested: "google/gemma-4-E2B-it-qat-q4_0-gguf → gemma-4-E2B_q4_0-it.gguf",
      fetch_command: "curl -L -o gemma-4-E2B_q4_0-it.gguf https://example.invalid/x",
    },
    starting: false,
    error: null,
    analysis: null,
    progress: null,
    pair: null,
    prompt_fingerprint: "734b5913bf03",
    scores: [],
    worst: [],
  };
}

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
      model_path: null,
      analysis_paused: false,
    }),
  ),
  setPrefs,
  applyDoctorFix: vi.fn(),
  runBackfill: vi.fn(),
  setLoginAgent: vi.fn(),
  setPaused: vi.fn(),
  revealLogs: vi.fn(),
  uninstallPreview,
  uninstallRun,
  // Phase 13. `llmModel` is the fixture the tests below set; the default is a
  // machine where nobody has pointed at a model, which is the state the exit
  // criterion is about.
  llmReport: vi.fn(() => Promise.resolve(llmModel)),
  pickModel: vi.fn(() => Promise.resolve(llmModel)),
  setModel: vi.fn((path: string | null) => {
    setModelCalls.push(path);
    return Promise.resolve(llmModel);
  }),
  setAnalysisPaused: vi.fn((paused: boolean) => {
    pausedCalls.push(paused);
    return Promise.resolve(llmModel);
  }),
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

describe("the notification switches (task 9.3)", () => {
  beforeEach(() => {
    setPrefs.mockClear();
  });

  /** The two notification boxes, in the order the page draws them. */
  function switches(node: HTMLElement): HTMLInputElement[] {
    return [...node.querySelectorAll<HTMLInputElement>("input[id^='pref-notify']")];
  }

  test("are on the status page, both off, and both enabled once prefs arrive", async () => {
    const node = await mount();
    const boxes = switches(node);

    expect(boxes.map((b) => b.id)).toEqual(["pref-notify_refusals", "pref-notify_high_risk"]);
    expect(boxes.every((b) => !b.checked)).toBe(true);
    expect(boxes.every((b) => !b.disabled)).toBe(true);
  });

  test("turning one on sends the whole Prefs, leaving the other alone", async () => {
    const node = await mount();
    const [refusals] = switches(node);

    refusals!.checked = true;
    refusals!.dispatchEvent(new Event("change"));
    await settle();

    // The *whole* Prefs, including the fields this switch knows nothing about
    // — which is the assertion: a patch would let the window forget a
    // preference it never displayed. Phase 13 added two, and they ride along.
    expect(setPrefs).toHaveBeenCalledWith({
      notify_refusals: true,
      notify_high_risk: false,
      redact_evidence: false,
      excluded_projects: [],
      model_path: null,
      analysis_paused: false,
    });
  });

  test("a switch that could not be saved goes back to what the store holds", async () => {
    setPrefs.mockImplementationOnce(() => Promise.reject(new Error("disk full")));
    const node = await mount();
    const [, highRisk] = switches(node);

    highRisk!.checked = true;
    highRisk!.dispatchEvent(new Event("change"));
    await settle();

    expect(highRisk!.checked).toBe(false);
  });
});

describe("the theme control", () => {
  beforeEach(() => {
    localStorage.clear();
    document.documentElement.removeAttribute("data-theme");
  });

  test("offers three states with System selected by default", async () => {
    const node = await mount();
    const segs = [...node.querySelectorAll<HTMLButtonElement>(".segmented .seg")];

    expect(segs.map((b) => b.textContent)).toEqual(["System", "Light", "Dark"]);
    expect(segs.map((b) => b.getAttribute("aria-checked"))).toEqual(["true", "false", "false"]);
  });

  test("choosing one applies it immediately, with no save button", async () => {
    const node = await mount();
    const dark = [...node.querySelectorAll<HTMLButtonElement>(".seg")].find(
      (b) => b.textContent === "Dark",
    );

    dark!.click();
    expect(document.documentElement.getAttribute("data-theme")).toBe("dark");
    expect(dark!.className).toContain("on");
    expect(dark!.getAttribute("aria-checked")).toBe("true");
  });

  test("opens on the choice already made", async () => {
    localStorage.setItem("toolog.theme", "light");
    const node = await mount();
    const on = node.querySelector(".seg.on");
    expect(on?.textContent).toBe("Light");
  });
});

describe("the model card (tasks 13.1, 13.2)", () => {
  test("with no model, it says so and shows the command it will never run", async () => {
    llmModel = noModel();
    const node = await mount();
    const card = node.querySelector(".model-card");

    expect(card).not.toBeNull();
    expect(card!.textContent).toContain("No model");
    // The whole of ADR-0008 in this card: the line is shown, never run.
    expect(card!.textContent).toContain("toolog has no network capability");
    expect(card!.querySelector("pre")?.textContent).toContain("curl -L -o");
  });

  test("a configured path that is not a model is named as the problem it is", async () => {
    const report = noModel();
    report.model.path = "/tmp/archive.tar.gz";
    report.model.problem = "/tmp/archive.tar.gz: not a GGUF model — it starts with a gzip archive";
    llmModel = report;

    const card = (await mount()).querySelector(".model-card");
    expect(card!.textContent).toContain("not a GGUF model");
    // And it still offers the way out, rather than only complaining.
    expect(card!.textContent).toContain("Choose a model…");
  });

  test("a loaded model shows its identity and how far the examination has got", async () => {
    const report = noModel();
    report.model.path = "/models/gemma.gguf";
    report.model.loaded = true;
    report.model.summary = "gemma4, 4.6B parameters, 3.1 GB";
    report.model.file = {
      path: "/models/gemma.gguf",
      size_bytes: 3_350_000_000,
      gguf_version: 3,
      architecture: "gemma4",
      name: "Gemma 4 E2B",
      parameters: 4_630_000_000,
      tensors: 541,
      sha256: null,
    };
    report.pair = "3646b4c147cd / 734b5913bf03";
    report.progress = {
      eligible: 3618,
      examined: 412,
      failed: 3,
      queued: 3203,
      mean_ms: 1249,
    };
    report.analysis = {
      running: true,
      paused: false,
      done_this_run: 412,
      failed_this_run: 3,
      skipped_live: 0,
      last_error: null,
    };
    llmModel = report;

    const card = (await mount()).querySelector(".model-card");
    expect(card!.textContent).toContain("gemma4, 4.6B parameters, 3.1 GB");
    // Both halves of the key a verdict is stored under (task 13.14).
    expect(card!.textContent).toContain("3646b4c147cd / 734b5913bf03");
    expect(card!.textContent).toContain("412");
    expect(card!.textContent).toContain("3,618");
    expect(card!.textContent).toContain("Pause examining");
  });

  test("forgetting a model clears the path rather than deleting anything", async () => {
    const report = noModel();
    report.model.path = "/models/gemma.gguf";
    llmModel = report;

    const node = await mount();
    const forget = [...node.querySelectorAll("button")].find(
      (b) => b.textContent === "Forget it",
    );
    forget!.click();
    await settle();

    expect(setModelCalls).toEqual([null]);
  });

  test("a build with no inference support says that, not \"no model\"", async () => {
    const report = noModel();
    report.model.supported = false;
    llmModel = report;

    const card = (await mount()).querySelector(".model-card");
    expect(card!.textContent).toContain("no inference support");
  });

  test("a report that cannot be read leaves the rest of the page alone", async () => {
    // The exit criterion, from the other side: the Status page is not the
    // model's, and a model that cannot be reported on must not take it down.
    llmModel = null;
    const node = await mount();
    expect(node.querySelector(".model-card")).toBeNull();
    expect(node.querySelector("details.uninstall")).not.toBeNull();
  });
});
