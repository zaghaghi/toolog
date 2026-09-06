//! The risk review (tasks 6.3, 6.4, and 11.7–11.13).
//!
//! A review is read worst first, and it has to survive the reader disagreeing
//! with it. Four things shape this view, three of them from living with v1.0:
//!
//! - **A dismissal hides nothing.** Setting a rule aside keeps the finding in
//!   the list, greyed and carrying the note someone wrote; what changes is the
//!   per-project posture, which is a claim about what still needs answering.
//!   A review that silently drops what was waved through is not a review.
//! - **Every finding leads to the calls.** A rule's conditions have no
//!   equivalent in the timeline's filter — "outside the session's cwd" is not
//!   a column — so the drill-through fetches the rule's own matches rather
//!   than a filter that would quietly show a *similar* set. Any one of them
//!   opens in the timeline, where the evidence is.
//! - **The summary and the table are one unit: distinct calls flagged.** They
//!   used to count different things — rules above, (rule, project) pairs below
//!   — so one rule spanning three projects appeared three times and nothing
//!   added up. Each severity column now sums to the number above it exactly.
//!   The four numbers still do not add to a grand total, and the page says so
//!   rather than letting a reader discover it (task 11.9).
//! - **Every rule is here, including the ones that matched nothing.** A rule
//!   that found nothing is a real result, and skipping it was exactly why a
//!   reader could not tell from the window which rules exist.

import type { Finding, ProjectRisk, RiskReview, Severity, ToolCall } from "./bindings";
import { dismissRule, restoreRule, revealRules, risk, ruleCalls } from "./bindings";
import { append, el, fill, orDash } from "./dom";
import { basename, clock, count, dayLabel, fullStamp, shortPath } from "./format";

/** How many calls one page of a drill-through fetches. */
const PAGE = 50;

/**
 * How many calls a finding shows before offering the rest.
 *
 * The same handful the finding used to carry on it — fetched on expand now
 * (task 11.2), because eight rows were built for twelve rules on every tab
 * activation and read for at most one.
 */
const FIRST_PAGE = 8;

/** Worst first, which is the order a review is read in. */
const SEVERITIES: Severity[] = ["high", "medium", "low", "info"];

const SEVERITY_WORDS: Record<Severity, string> = {
  high: "Worth answering for",
  medium: "Worth explaining",
  low: "Worth a look",
  info: "Worth knowing",
};

/** What a rule's scope means for the number beside it. */
function unit(finding: Finding): string {
  if (finding.scope === "session") {
    return `${count(finding.sessions)} ${finding.sessions === 1 ? "session" : "sessions"}`;
  }
  const calls = `${count(finding.calls)} ${finding.calls === 1 ? "call" : "calls"}`;
  const sessions = `${count(finding.sessions)} ${finding.sessions === 1 ? "session" : "sessions"}`;
  return `${calls} across ${sessions}`;
}

export interface RiskOptions {
  onNotice: (message: string) => void;
  /** Open one call in the timeline — the drill-through's destination. */
  onOpenCall: (toolUseId: string) => void;
  /**
   * Show every call one rule matched, in the timeline (task 12.12).
   *
   * Possible for the first time in Phase 12: `@rule:<id>` compiles the rule's
   * own conditions into the timeline's filter, so this is the same set the
   * finding counted rather than a similar one.
   */
  onOpenRule: (ruleId: string) => void;
}

export class RiskView {
  readonly node: HTMLElement;
  private readonly content = el("div", { class: "sheet" });
  private review: RiskReview | null = null;
  private readonly open = new Set<string>();
  private loading = false;

  constructor(private readonly opts: RiskOptions) {
    this.node = el("section", { class: "risk" }, [this.content]);
  }

  async refresh(): Promise<void> {
    if (this.loading) return;
    this.loading = true;
    this.content.classList.add("busy");
    try {
      this.review = await risk();
      this.draw();
    } catch (error) {
      this.opts.onNotice(`The rules could not be run: ${String(error)}`);
    } finally {
      this.loading = false;
      this.content.classList.remove("busy");
    }
  }

  private draw(): void {
    const review = this.review;
    if (review === null) return;

    const matched = review.findings.filter((f) => f.calls > 0);

    if (matched.length === 0) {
      // Still the whole list underneath: task 11.11's point is that a rule
      // which found nothing is a result, and a reader who cannot see the rules
      // cannot tell "clean" from "not looking".
      fill(this.content, [
        el("p", { class: "empty" }, [
          el("strong", { text: "No rule matched anything in this store." }),
          ` That is a real result, not an empty screen: all ${count(review.findings.length)} rules ran and found nothing. Each is listed below with what it looks for.`,
        ]),
        el("div", { class: "findings" }, review.findings.map((f) => this.finding(f))),
        this.rulesFooter(),
      ]);
      return;
    }

    const aside = matched.filter((f) => f.dismissed !== null).length;
    const quiet = review.findings.length - matched.length;
    fill(this.content, [
      el("div", { class: "risk-summary" }, [
        el("div", { class: "risk-counts" }, review.totals.map((t) => this.severityCount(t))),
        el("p", { class: "note" }, [
          this.newness(review),
          `${count(matched.length - aside)} of ${count(review.findings.length)} rules matched something` +
            (aside === 0
              ? ". "
              : `; ${count(aside)} more were set aside with a note and still count against nothing. `) +
            (quiet === 0
              ? ""
              : `${count(quiet)} matched nothing, and are listed below with what they look for. `),
          // Task 11.9: said here rather than discovered by adding four numbers
          // that were never meant to be added.
          el("strong", { text: "The four numbers do not add up to a total" }),
          " — a call caught by a high rule and a low rule is one call at each severity — so there is not one.",
        ]),
      ]),
      this.projects(review.projects),
      el("div", { class: "findings" }, review.findings.map((f) => this.finding(f))),
      this.rulesFooter(),
    ]);
  }

  /**
   * What is new since the last review (task 12.5).
   *
   * A first review is not a review with nothing new in it — everything is new
   * the first time anyone looks — and saying "0 new" on a store nobody has read
   * would be reassurance this has not earned.
   */
  private newness(review: RiskReview): HTMLElement {
    if (review.first_review) {
      return el("strong", { class: "risk-new", text: "First review of this store. " });
    }
    const fresh = review.findings.reduce((sum, f) => sum + f.new_calls, 0);
    if (fresh === 0) {
      return el("span", { class: "risk-new none", text: "Nothing new since the last review. " });
    }
    return el("strong", {
      class: "risk-new",
      text: `${count(fresh)} ${fresh === 1 ? "call is" : "calls are"} new since the last review. `,
    });
  }

  /**
   * One hero number: distinct calls flagged, with the rule count under it.
   *
   * Both numbers are worth having and only one of them can be the total
   * (task 11.10). The calls are the total, because that is the unit the table
   * below adds up in.
   */
  private severityCount(tally: RiskReview["totals"][number]): HTMLElement {
    return el("div", { class: `risk-count ${tally.severity}` }, [
      el("span", { class: "risk-count-n", text: count(tally.calls) }),
      el("span", { class: "risk-count-label", text: tally.severity }),
      el("span", {
        class: "risk-count-rules",
        text: `${count(tally.rules)} ${tally.rules === 1 ? "rule" : "rules"}, ${count(tally.calls)} ${tally.calls === 1 ? "call" : "calls"}`,
      }),
      el("span", { class: "risk-count-note", text: SEVERITY_WORDS[tally.severity] }),
    ]);
  }

  /** Task 6.4: the posture glance, one row per project. */
  private projects(projects: ProjectRisk[]): HTMLElement | null {
    if (projects.length === 0) return null;
    return el("div", { class: "card" }, [
      el("div", { class: "chart-titles" }, [
        el("span", { class: "chart-title", text: "By project" }),
        el("span", {
          class: "chart-caption",
          text: "Distinct calls flagged, so each column adds up to the number above it. Live findings only — a rule set aside stops counting against the project that tripped it.",
        }),
      ]),
      el("table", { class: "chart-table risk-projects" }, [
        el("thead", {}, [
          el("tr", {}, [
            el("th", { text: "Project" }),
            ...SEVERITIES.map((s) => el("th", { class: "num", text: s })),
          ]),
        ]),
        el(
          "tbody",
          {},
          projects.map((p) =>
            el("tr", { class: p.project_path === null ? "unattributed" : "" }, [
              // Task 11.8: these calls were dropped from this table and counted
              // in the summary above it, which is half of why the two could not
              // be made to agree.
              p.project_path === null
                ? el("th", {
                    attrs: { scope: "row" },
                    text: "No project recorded",
                    title: "Calls whose session the store never learned a project path for",
                  })
                : el("th", {
                    attrs: { scope: "row", title: p.project_path },
                    text: basename(p.project_path),
                  }),
              ...p.by_severity.map((n, i) =>
                el("td", {
                  class: n === 0 ? "num none" : `num sev ${SEVERITIES[i] ?? ""}`,
                  text: n === 0 ? "—" : count(n),
                }),
              ),
            ]),
          ),
        ),
      ]),
    ]);
  }

  private finding(f: Finding): HTMLElement {
    const dismissed = f.dismissed !== null;
    const body = el("div", { class: "finding-body", hidden: !this.open.has(f.rule_id) });
    if (this.open.has(f.rule_id)) this.fillBody(f, body);

    const head = el("button", { class: "finding-head", attrs: { type: "button", "aria-expanded": String(this.open.has(f.rule_id)) } }, [
      el("span", { class: `sev-chip ${f.severity}`, text: f.severity }),
      el("span", { class: "finding-title", text: f.title }),
      el("span", { class: "finding-count", text: unit(f) }),
      el("span", { class: "finding-caret", text: this.open.has(f.rule_id) ? "▾" : "▸" }),
    ]);
    head.addEventListener("click", () => {
      const showing = !this.open.has(f.rule_id);
      if (showing) this.open.add(f.rule_id);
      else this.open.delete(f.rule_id);
      body.hidden = !showing;
      head.setAttribute("aria-expanded", String(showing));
      if (showing) this.fillBody(f, body);
    });

    return el("div", { class: dismissed ? "finding set-aside" : "finding" }, [
      head,
      dismissed && f.dismissed !== null
        ? el("p", { class: "finding-dismissal" }, [
            el("strong", { text: "Set aside. " }),
            f.dismissed.note,
            el("span", { class: "dot", text: "·" }),
            fullStamp(f.dismissed.at),
          ])
        : null,
      body,
    ]);
  }

  private fillBody(f: Finding, body: HTMLElement): void {
    const projects = f.projects.map((p) => basename(p));
    if (f.unattributed_calls > 0) {
      projects.push(`${count(f.unattributed_calls)} with no project recorded`);
    }

    const calls = el("div", { class: "finding-calls" });
    fill(body, [
      el("p", { class: "finding-why", text: f.explanation }),
      this.conditions(f),
      el("dl", { class: "kv" }, [
        el("dt", { text: "First" }),
        el("dd", { text: f.first_at === null ? "—" : fullStamp(f.first_at) }),
        el("dt", { text: "Last" }),
        el("dd", { text: f.last_at === null ? "—" : fullStamp(f.last_at) }),
        el("dt", { text: "Projects" }),
        el("dd", { text: projects.length === 0 ? "—" : projects.join(", ") }),
        el("dt", { text: "First seen" }),
        el("dd", {
          // Not `first_at`: that is when the call ran. This is when a review
          // first noticed, which is the only one of the two nothing else can
          // reconstruct (task 12.4).
          text:
            f.first_seen === null
              ? "not yet recorded"
              : `${dayLabel(f.first_seen)}${f.new_calls > 0 ? ` · ${count(f.new_calls)} new` : ""}`,
        }),
        el("dt", { text: "Rule" }),
        el("dd", { class: "mono", text: `${f.rule_id}${f.from_user ? " · from your rules file" : ""}` }),
      ]),
      f.calls === 0
        ? el("p", { class: "note", text: "This rule matched nothing in this store." })
        : calls,
      f.calls === 0 ? null : this.moreCalls(f),
      f.calls === 0 ? null : this.openInTimeline(f),
      this.judgement(f),
    ]);

    // Task 11.2: the calls arrive when a finding is opened, not for all twelve
    // rules on every tab activation.
    if (f.calls > 0) {
      fill(calls, [el("div", { class: "empty small", text: "Loading…" })]);
      void ruleCalls(f.rule_id, { limit: FIRST_PAGE, offset: 0 })
        .then((rows) => fill(calls, rows.map((c) => this.callRow(c))))
        .catch((error: unknown) => {
          fill(calls, [el("p", { class: "problem", text: String(error) })]);
        });
    }
  }

  /**
   * What the rule looks for, in words (task 11.12).
   *
   * Rendered in Rust from the `Match` struct and carried here, rather than
   * written by hand beside the rule: a description a person maintains is a
   * description that eventually describes a different rule. `first_line` and
   * `outside_cwd` in particular are not columns and not guessable from a title.
   */
  private conditions(f: Finding): HTMLElement {
    return el("div", { class: "finding-conditions" }, [
      el("span", { class: "cond-label", text: "Matches when" }),
      el("ul", {}, f.conditions.map((c) => el("li", { text: c }))),
    ]);
  }

  /**
   * One matching call, as a line that opens it in the timeline.
   *
   * Deliberately the same three facts the timeline's own row leads with —
   * time, tool, what it did — so following the link does not feel like
   * arriving somewhere else.
   */
  private callRow(call: ToolCall): HTMLElement {
    const row = el("button", { class: "finding-call", attrs: { type: "button" } }, [
      el("span", { class: "fc-time", text: clock(call.called_at) }),
      el("span", { class: "fc-tool", text: orDash(call.tool_name) }),
      el("span", { class: "fc-what mono", text: firstLine(call) }),
      call.decision === "reject" ? el("span", { class: "fc-flag refused", text: "refused" }) : null,
      call.success === false ? el("span", { class: "fc-flag failed", text: "failed" }) : null,
    ]);
    row.addEventListener("click", () => this.opts.onOpenCall(call.tool_use_id));
    return row;
  }

  /**
   * The whole rule, in the timeline (task 12.12).
   *
   * A route, not a replacement: the inline page stays, because reading a
   * finding and leaving it are different things. What is new is that the
   * timeline can now express the question at all.
   */
  private openInTimeline(f: Finding): HTMLElement {
    const button = el("button", {
      class: "link",
      text: `Show all ${count(f.calls)} in the timeline`,
      attrs: { type: "button" },
    });
    button.addEventListener("click", () => this.opts.onOpenRule(f.rule_id));
    return el("div", { class: "finding-open" }, [button]);
  }

  /** "Show the rest" — the drill-through past the first page. */
  private moreCalls(f: Finding): HTMLElement | null {
    const shown = Math.min(f.calls, FIRST_PAGE);
    if (f.calls <= shown) return null;

    const list = el("div", { class: "finding-calls" });
    let offset = shown;
    const button = el("button", {
      class: "finding-more",
      text: `Show the other ${count(f.calls - shown)}`,
      attrs: { type: "button" },
    });
    button.addEventListener("click", () => {
      button.disabled = true;
      button.textContent = "Loading…";
      void ruleCalls(f.rule_id, { limit: PAGE, offset })
        .then((calls) => {
          for (const call of calls) append(list, this.callRow(call));
          offset += calls.length;
          const left = f.calls - offset;
          button.disabled = false;
          button.textContent = left > 0 ? `Show ${count(Math.min(left, PAGE))} more` : "That is all of them";
          button.disabled = left <= 0;
        })
        .catch((error: unknown) => {
          button.disabled = false;
          button.textContent = "Try again";
          this.opts.onNotice(`Those calls could not be fetched: ${String(error)}`);
        });
    });
    return el("div", {}, [list, button]);
  }

  /** Dismiss with a note, or take the dismissal back. */
  private judgement(f: Finding): HTMLElement {
    if (f.dismissed !== null) {
      const restore = el("button", { text: "Bring it back", attrs: { type: "button" } });
      restore.addEventListener("click", () => {
        void restoreRule(f.rule_id)
          .then((review) => {
            this.review = review;
            this.draw();
          })
          .catch((error: unknown) => this.opts.onNotice(`That failed: ${String(error)}`));
      });
      return el("div", { class: "actions" }, [restore]);
    }

    const note = el("input", {
      class: "search finding-note",
      placeholder: "Why is this one fine? (kept with the rule)",
      attrs: { "aria-label": "Reason for setting this rule aside" },
    });
    const save = el("button", { text: "Set aside", attrs: { type: "button" } });
    const commit = (): void => {
      const text = note.value.trim();
      if (text === "") {
        // A dismissal without a reason is just a hidden finding, which is the
        // thing this view exists not to have.
        this.opts.onNotice("Setting a rule aside needs a reason — it is kept with the rule.");
        note.focus();
        return;
      }
      save.disabled = true;
      void dismissRule(f.rule_id, text)
        .then((review) => {
          this.review = review;
          this.draw();
        })
        .catch((error: unknown) => {
          save.disabled = false;
          this.opts.onNotice(`That failed: ${String(error)}`);
        });
    };
    save.addEventListener("click", commit);
    note.addEventListener("keydown", (event) => {
      if (event.key === "Enter") commit();
    });
    return el("div", { class: "actions" }, [note, save]);
  }

  /**
   * Where the rules live and how to change them (task 11.13).
   *
   * "Rules are data you can edit" was true in v1.0 and unusable: the only
   * handle was a path in a footnote. The path is still stated — you cannot
   * paste a button into a terminal — but there is now a button beside it, and
   * the file's format is named rather than left to be guessed.
   */
  private rulesFooter(): HTMLElement {
    const review = this.review;
    const path = review?.rules_path ?? null;

    const reveal = el("button", {
      class: "link",
      text: review?.rules_customized === true ? "Show the file" : "Show the folder",
      attrs: { type: "button" },
    });
    reveal.addEventListener("click", () => {
      void revealRules().catch((error: unknown) =>
        this.opts.onNotice(`That could not be opened: ${String(error)}`),
      );
    });

    return el("div", { class: "rules-note" }, [
      el("p", { class: "footnote" }, [
        el("strong", { text: "Rules are data, not code. " }),
        path === null
          ? "A rules file can sit beside the database and add to or replace any rule above."
          : review?.rules_customized === true
            ? `Your own rules are read from ${shortPath(path)}, on top of the built-in set.`
            : `Put a TOML file at ${shortPath(path)} to add a rule, retune one, or switch one off — an id that matches a built-in replaces it.`,
        " Every rule above is listed with what it looks for, whether or not it matched. ",
        "Findings are recomputed whenever the store or the rules change; nothing here is stored except the notes.",
      ]),
      el("div", { class: "actions" }, [reveal]),
    ]);
  }
}

/** The first line of what a call did — a heredoc body is not the command. */
function firstLine(call: ToolCall): string {
  const summary = call.input_summary ?? call.target_path ?? "";
  const line = summary.split("\n", 1)[0] ?? "";
  return line.length > 160 ? `${line.slice(0, 159)}…` : line;
}
