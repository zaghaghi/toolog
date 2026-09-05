//! First run, and the state of the integration.
//!
//! Phase 4's wizard and status page, moved onto the generated bindings. It is
//! no longer the whole window — the timeline is — but it is still where
//! "is this thing actually recording?" is answered, and task 5.13 points at it
//! from the timeline's "the collector is not running" banner.

import type { Setup, Status } from "./bindings";
import {
  applyDoctorFix,
  collectorStatus,
  doctorStatus,
  revealLogs,
  runBackfill,
  setLoginAgent,
  setPaused,
} from "./bindings";
import { el, fill, span } from "./dom";
import { count } from "./format";

export interface SetupOptions {
  onNotice: (message: string) => void;
  /** Capture was switched on, so the timeline should re-check its banner. */
  onChanged: () => void;
}

function statRow(label: string, value: string): HTMLElement {
  return el("div", { class: "srow" }, [span("label", label), span("value", value)]);
}

export class SetupView {
  readonly node = el("div", { class: "setup" });

  constructor(private readonly options: SetupOptions) {
    void this.render();
  }

  refresh(): void {
    void this.render();
  }

  private async render(): Promise<void> {
    fill(this.node, [el("div", { class: "empty", text: "Loading…" })]);
    try {
      const [setup, status] = await Promise.all([
        doctorStatus(),
        collectorStatus().catch(() => null),
      ]);
      fill(this.node, [setup.configured ? this.home(setup, status) : this.wizard(setup)]);
    } catch (error) {
      fill(this.node, [el("div", { class: "problem", text: String(error) })]);
    }
  }

  // ------------------------------------------------------------- first run

  private wizard(setup: Setup): HTMLElement {
    const view = el("div", { class: "prose" });

    view.append(
      el("h1", { text: "Record what Claude Code does here" }),
      el("p", {
        class: "lede",
        text: "A local record of every tool call Claude Code makes on this machine.",
      }),
      el("h2", { text: "What is captured" }),
      el("ul", { class: "plain" }, [
        el("li", {}, [
          el("strong", { text: "From your transcripts: " }),
          "the full command or file each tool call ran, and its result.",
        ]),
        el("li", {}, [
          el("strong", { text: "From Claude Code's telemetry: " }),
          "who approved each call, how long it took, what it cost — and the calls you refused, which nothing else records.",
        ]),
      ]),
      el("h2", { text: "What leaves this machine" }),
      el("p", {}, [
        el("strong", { text: "Nothing. " }),
        "The receiver binds to 127.0.0.1 and the database is a file in your Library folder. " +
          "Prompts and assistant replies are not captured at all.",
      ]),
      el("h2", { text: "To switch it on" }),
      el("p", {}, [
        "toolog will add six environment variables to ",
        span("mono", setup.settings_path),
        ". Your existing settings are kept, and a timestamped backup is written first.",
      ]),
    );

    const enable = el("button", { class: "primary", text: "Enable capture" });
    enable.addEventListener("click", () => {
      enable.disabled = true;
      enable.textContent = "Writing…";
      void applyDoctorFix()
        .then(() => {
          this.options.onChanged();
          return this.render();
        })
        .catch((error: unknown) => {
          enable.disabled = false;
          enable.textContent = "Enable capture";
          view.append(el("div", { class: "problem", text: String(error) }));
        });
    });
    view.append(el("div", { class: "actions" }, [enable]));

    if (setup.problems.length > 0) {
      view.append(el("h2", { text: "Worth knowing first" }));
      for (const problem of setup.problems) {
        view.append(el("div", { class: "problem", text: problem }));
      }
    }
    return view;
  }

  // ----------------------------------------------------------------- home

  private home(setup: Setup, status: Status | null): HTMLElement {
    const view = el("div", { class: "prose" });
    const live = status !== null && !status.paused && status.listening;

    view.append(
      el("h1", { text: "Status" }),
      span(
        `pill ${status === null ? "off" : live ? "on" : "idle"}`,
        status === null
          ? "Not capturing"
          : live
            ? `Capturing on ${status.endpoint.replace("http://", "")}`
            : status.paused
              ? "Capture paused"
              : "The receiver is not listening",
      ),
    );

    if (status !== null) {
      view.append(
        el("h2", { text: "Today" }),
        el("div", { class: "card" }, [
          statRow("Events stored today", count(status.events_today)),
          statRow("Tool calls in the store", count(status.tool_calls)),
          statRow("OTLP batches received", count(status.counters.batches)),
          statRow("Batches dropped", count(status.counters.dropped)),
        ]),
      );
    }

    view.append(
      el("h2", { text: "History" }),
      el("div", { class: "card" }, [
        statRow("Transcripts on disk", count(setup.transcript_files)),
        statRow("Already imported", count(setup.ingested_files)),
      ]),
    );

    const backfill = el("button", { text: "Import history" });
    backfill.addEventListener("click", () => {
      backfill.disabled = true;
      backfill.textContent = "Importing…";
      void runBackfill()
        .then((summary) => {
          this.options.onNotice(`Imported ${count(summary.stored)} new records`);
          this.options.onChanged();
          return this.render();
        })
        .catch((error: unknown) => {
          backfill.disabled = false;
          backfill.textContent = "Import history";
          this.options.onNotice(String(error));
        });
    });

    const pause = el("button", { text: live ? "Pause capture" : "Resume capture" });
    pause.addEventListener("click", () => {
      pause.disabled = true;
      void setPaused(live)
        .then(() => {
          this.options.onChanged();
          return this.render();
        })
        .catch((error: unknown) => this.options.onNotice(String(error)));
    });

    const agent = el("button", {
      text: setup.agent_installed ? "Stop starting at login" : "Start at login",
      disabled: !setup.agent_supported,
    });
    agent.addEventListener("click", () => {
      agent.disabled = true;
      void setLoginAgent(!setup.agent_installed)
        .then(() => this.render())
        .catch((error: unknown) => this.options.onNotice(String(error)));
    });

    const logs = el("button", { text: "Reveal logs" });
    logs.addEventListener("click", () => {
      void revealLogs().catch((error: unknown) => this.options.onNotice(String(error)));
    });

    view.append(el("div", { class: "actions" }, [backfill, pause, agent, logs]));

    if (setup.problems.length > 0) {
      view.append(el("h2", { text: "Needs attention" }));
      for (const problem of setup.problems) {
        view.append(el("div", { class: "problem", text: problem }));
      }
    }

    view.append(
      el("h2", { text: "Diagnostics" }),
      el("pre", { class: "report", text: setup.report }),
    );
    return view;
  }
}
