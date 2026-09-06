//! First run, and the state of the integration.
//!
//! Phase 4's wizard and status page, moved onto the generated bindings. It is
//! no longer the whole window — the timeline is — but it is still where
//! "is this thing actually recording?" is answered, and task 5.13 points at it
//! from the timeline's "the collector is not running" banner.

import type { Prefs, Setup, Status } from "./bindings";
import {
  applyDoctorFix,
  collectorStatus,
  doctorStatus,
  getPrefs,
  revealLogs,
  runBackfill,
  setLoginAgent,
  setPaused,
  setPrefs,
  uninstallPreview,
  uninstallRun,
} from "./bindings";
import { el, fill, span } from "./dom";
import { bytes, count } from "./format";
import { currentTheme, setTheme, THEMES } from "./theme";
import type { Theme } from "./theme";

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

    view.append(el("h2", { text: "Appearance" }), this.appearance());

    view.append(el("h2", { text: "Notifications" }), this.notifications());

    view.append(el("h2", { text: "Privacy" }), this.privacy());

    view.append(
      el("h2", { text: "Diagnostics" }),
      el("pre", { class: "report", text: setup.report }),
    );

    view.append(el("h2", { text: "Remove toolog" }), this.uninstall());
    return view;
  }

  /**
   * Task 8.6's UI half, and the same operation `toolog uninstall` performs.
   *
   * Built as a preview that has to be opened, read and then confirmed, because
   * two of the three things it touches are not ours: `~/.claude/settings.json`
   * belongs to Claude Code, and the recorded history belongs to the user. The
   * preview text comes from Rust verbatim rather than being rebuilt here — the
   * window and the terminal must not be able to describe the same irreversible
   * action differently.
   *
   * Deleting history is a separate, unticked box. Removing the tool and
   * destroying the record it collected are different decisions, and defaulting
   * the second to the first is how audit trails disappear by accident.
   */
  private uninstall(): HTMLElement {
    const details = el("details", { class: "card uninstall" });
    const body = el("div", { class: "uninstall-body" });
    const report = el("pre", { class: "report", text: "Loading…" });
    const box = el("input", { type: "checkbox", attrs: { id: "uninstall-delete-data" } });
    const label = el("label", {
      attrs: { for: "uninstall-delete-data" },
      class: "switch-label",
      text: "Also delete my recorded history",
    });
    const why = span("switch-why", "");
    const go = el("button", { class: "danger", text: "Remove toolog" });
    const status = el("div", { class: "note" });

    const refresh = (): Promise<void> =>
      uninstallPreview(box.checked)
        .then((plan) => {
          report.textContent = plan.report;
          go.disabled = !plan.any_changes;
          why.textContent =
            plan.data_bytes === 0
              ? "Nothing has been recorded yet."
              : `${bytes(plan.data_bytes)} in ${plan.data_dir}. Kept unless you tick this.`;
        })
        .catch((error: unknown) => {
          report.textContent = String(error);
        });

    // Only ask the backend once the section is actually opened: this walks the
    // settings stack and stats the store, and the status page should not pay
    // for it on every render.
    let loaded = false;
    details.addEventListener("toggle", () => {
      if (details.open && !loaded) {
        loaded = true;
        void refresh();
      }
    });
    box.addEventListener("change", () => void refresh());

    go.addEventListener("click", () => {
      go.disabled = true;
      go.textContent = "Removing…";
      void uninstallRun(box.checked)
        .then((outcome) => {
          go.textContent = "Remove toolog";
          fill(
            status,
            [
              ...outcome.done.map((line) => el("div", { text: line })),
              ...outcome.failed.map((line) => el("div", { class: "problem", text: line })),
            ].concat(
              outcome.done.length === 0 && outcome.failed.length === 0
                ? [el("div", { text: "Nothing to do." })]
                : [],
            ),
          );
          this.options.onChanged();
          return refresh();
        })
        .catch((error: unknown) => {
          go.disabled = false;
          go.textContent = "Remove toolog";
          fill(status, [el("div", { class: "problem", text: String(error) })]);
        });
    });

    body.append(
      el("p", { class: "note" }, [
        "This undoes the install: the login agent, and the six variables toolog added to ",
        "Claude Code's settings. Where a backup was taken before the first write, the file ",
        el("strong", { text: "goes back byte for byte" }),
        " rather than having keys deleted out of it.",
      ]),
      report,
      el("div", { class: "switch" }, [box, el("div", {}, [label, why])]),
      el("div", { class: "actions" }, [go]),
      status,
    );
    details.append(el("summary", { text: "What removing toolog would do" }), body);
    return details;
  }

  /**
   * Light, dark, or the machine's own setting.
   *
   * Three states rather than a switch, and **System** is the default: macOS
   * changes appearance at sunset, and a two-state toggle would make the state
   * most people want the one they cannot pick.
   *
   * Applied immediately, with no save button and no round trip — the choice
   * *is* the action. It is remembered in the window rather than in `prefs.json`
   * because nothing outside the window acts on it; see `theme.ts`.
   */
  private appearance(): HTMLElement {
    const labels: Record<Theme, string> = {
      system: "System",
      light: "Light",
      dark: "Dark",
    };

    const group = el("div", {
      class: "segmented",
      role: "radiogroup",
      attrs: { "aria-label": "Theme" },
    });

    const buttons = THEMES.map((theme) => {
      const button = el("button", {
        class: "seg",
        text: labels[theme],
        attrs: { type: "button", role: "radio", "data-theme-choice": theme },
      });
      button.addEventListener("click", () => {
        setTheme(theme);
        mark(theme);
      });
      return [theme, button] as const;
    });

    const mark = (chosen: Theme): void => {
      for (const [theme, button] of buttons) {
        const on = theme === chosen;
        button.className = on ? "seg on" : "seg";
        button.setAttribute("aria-checked", String(on));
      }
    };
    mark(currentTheme());

    group.append(...buttons.map(([, button]) => button));
    return el("div", { class: "card" }, [
      group,
      el("p", {
        class: "note",
        text: "System follows the machine's own light or dark setting, including when it changes at sunset.",
      }),
    ]);
  }

  /**
   * Task 6.12's two switches, moved here by task 9.3.
   *
   * They were on the live view, which is gone. Nothing about them belonged to
   * that view: they are preferences, they persist across restarts, and this is
   * where the other preference already lives. Same `Prefs` round-trip, same
   * defaults — both off, because a tool that starts by interrupting you is a
   * tool you turn off.
   */
  private notifications(): HTMLElement {
    const card = el("div", { class: "card" }, [
      el("p", { class: "note" }, [
        "Off until you turn them on, and each one on its own. ",
        el("strong", { text: "Nothing leaves this machine either way." }),
      ]),
    ]);

    const boxes = [
      this.toggle(
        "notify_refusals",
        "When a call is refused",
        "A denial by a person, a hook or a permission rule. This is the event the tool exists for.",
      ),
      this.toggle(
        "notify_high_risk",
        "When a call trips a high-severity rule",
        "The same rules the Risk tab runs, asked about the one call as it lands.",
      ),
    ];
    card.append(...boxes.map((b) => b.node));

    void getPrefs()
      .then((prefs) => {
        for (const box of boxes) box.load(prefs);
      })
      .catch((error: unknown) => this.options.onNotice(String(error)));

    return card;
  }

  /**
   * One notification switch. Narrowed to the boolean fields so a non-boolean
   * preference cannot be handed to a checkbox.
   *
   * The element is built before the preferences arrive and enabled when they
   * do: a checkbox that renders unchecked and then flips is a checkbox that
   * has lied to whoever was already reading it.
   */
  private toggle(
    key: "notify_refusals" | "notify_high_risk",
    label: string,
    why: string,
  ): { node: HTMLElement; load: (prefs: Prefs) => void } {
    const box = el("input", { type: "checkbox", attrs: { id: `pref-${key}` } });
    box.disabled = true;

    const load = (prefs: Prefs): void => {
      let current = prefs;
      box.checked = current[key];
      box.disabled = false;
      box.addEventListener("change", () => {
        const next: Prefs = { ...current, [key]: box.checked };
        void setPrefs(next)
          .then((saved) => {
            current = saved;
            box.checked = saved[key];
          })
          .catch((error: unknown) => {
            box.checked = current[key];
            this.options.onNotice(`That switch could not be saved: ${String(error)}`);
          });
      });
    };

    const node = el("div", { class: "switch" }, [
      box,
      el("div", {}, [
        el("label", { attrs: { for: `pref-${key}` }, class: "switch-label", text: label }),
        span("switch-why", why),
      ]),
    ]);
    return { node, load };
  }

  /**
   * Task 7.3, stated rather than defaulted.
   *
   * Secrets are always stripped from the projection — the rows the four views
   * read. Whether they are also stripped from the **evidence** is a real
   * choice with a cost in both directions, so it is a switch with the cost
   * next to it: off keeps every projection rebuildable and keeps secrets on
   * disk; on stops storing them and makes that irreversible.
   */
  private privacy(): HTMLElement {
    const card = el("div", { class: "card" }, [
      el("p", { class: "note" }, [
        "Secrets are removed from what the timeline and risk views show — commands, ",
        "arguments and results — using patterns you can extend. ",
        el("strong", { text: "That much is not optional." }),
      ]),
    ]);

    const box = el("input", { type: "checkbox", attrs: { id: "pref-redact-evidence" } });
    box.disabled = true;
    const why = span(
      "switch-why",
      "The evidence store keeps every record exactly as it arrived, which is what lets a " +
        "projection be rebuilt when a pattern turns out to be wrong — and means a secret that " +
        "went past stays on disk. Turning this on stops storing them at all, and cannot be " +
        "undone or applied backwards: records already held keep what they hold.",
    );

    void getPrefs()
      .then((prefs) => {
        box.checked = prefs.redact_evidence;
        box.disabled = false;
        box.addEventListener("change", () => {
          const next: Prefs = { ...prefs, redact_evidence: box.checked };
          void setPrefs(next)
            .then((saved) => {
              box.checked = saved.redact_evidence;
              this.options.onNotice(
                saved.redact_evidence
                  ? "New records will be redacted before they are stored. Records already held are unchanged."
                  : "New records will be stored exactly as they arrive.",
              );
            })
            .catch((error: unknown) => {
              box.checked = prefs.redact_evidence;
              this.options.onNotice(String(error));
            });
        });
      })
      .catch((error: unknown) => this.options.onNotice(String(error)));

    card.append(
      el("div", { class: "switch" }, [
        box,
        el("div", {}, [
          el("label", {
            attrs: { for: "pref-redact-evidence" },
            class: "switch-label",
            text: "Redact the evidence store too",
          }),
          why,
        ]),
      ]),
    );
    return card;
  }
}
