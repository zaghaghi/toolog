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

import type {
  Finding,
  LlmReport,
  ProjectRisk,
  RiskReview,
  Scored,
  Severity,
  ToolCall,
} from "./bindings";
import { dismissRule, llmReport, restoreRule, revealRules, risk, ruleCalls } from "./bindings";
import { append, el, fill, orDash } from "./dom";
import { basename, clock, count, dayLabel, fullStamp } from "./format";

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

/**
 * What each severity is asking of the reader.
 *
 * A tooltip rather than a line under every number. Four captions under four
 * cards is a paragraph the reader has to skip past to reach the numbers, and it
 * says the same thing every time they look.
 */
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
  /** Open a raw query in the timeline — how the model's section drills through. */
  onOpenQuery: (query: string) => void;
}

export class RiskView {
  readonly node: HTMLElement;
  private readonly content = el("div", { class: "sheet" });
  private review: RiskReview | null = null;
  private llm: LlmReport | null = null;
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
      // Two reads, not one, and deliberately: the rules and the model are
      // separate claims, and a model that cannot be reported on must not stop
      // the review being shown. That is the same separation the section itself
      // is about (task 13.16).
      const [review, llm] = await Promise.all([
        risk(),
        llmReport().catch(() => null),
      ]);
      this.review = review;
      this.llm = llm;
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
        ...this.secondOpinion(),
      ]);
      return;
    }

    fill(this.content, [
      el("div", { class: "risk-summary" }, [
        el(
          "div",
          { class: "risk-counts" },
          review.totals.map((t) => this.severityCount(t, review)),
        ),
      ]),
      this.projects(review.projects),
      el("div", { class: "findings" }, review.findings.map((f) => this.finding(f))),
      this.rulesFooter(),
      ...this.secondOpinion(),
    ]);
  }

  /**
   * What a local model said about the calls no rule matched (task 13.16).
   *
   * **Explicitly not the rules.** Everything above this point is deterministic:
   * a rule with an id, conditions anyone can read, and a severity that means
   * the same thing every time it is computed. This is a 4.6B model's opinion,
   * it is not reproducible, and it is wrong sometimes.
   *
   * So the section is separated by more than a heading. It never uses the word
   * "severity" or any of the four severity words; its scores are numbers on
   * their own scale, drawn in their own colour. It states the model and prompt
   * it came from in words, because a number whose author cannot be named is not
   * evidence. And it never contributes to the counts above — those are the
   * rules', and they do not move because a model was pointed at the store.
   *
   * Absent entirely when no model is configured, which is the exit criterion:
   * with none, the risk view is exactly as it was.
   */
  private secondOpinion(): HTMLElement[] {
    const llm = this.llm;
    if (llm === null) return [];
    const progress = llm.progress;
    // No model, or one that has never answered: nothing to report. A card
    // saying "0 examined" for a store nobody pointed a model at would be an
    // answer to a question nobody asked.
    if (progress === null || llm.model.path === null) return [];

    const done = progress.examined + progress.failed;
    const share =
      progress.eligible > 0 ? Math.round((done / progress.eligible) * 100) : 0;

    const head = el("div", { class: "llm-head" }, [
      el("h2", { text: "A second opinion" }),
      // The claim that has to survive any trimming: not a rule, and wrong
      // sometimes (ADR-0013). Everything else it used to say — what the model
      // was pointed at, that nothing here moves the numbers above — is either
      // visible in the line below or true of the whole section.
      el("p", { class: "llm-caveat" }, [
        "A local model on the ",
        el("strong", { text: count(progress.eligible) }),
        " commands no rule matched. Advisory, not a rule, and wrong sometimes.",
      ]),
    ]);

    const bar = el("div", { class: "llm-progress" }, [
      el("div", {
        class: "llm-progress-fill",
        attrs: { style: `width: ${String(share)}%` },
      }),
    ]);

    const state = el("p", { class: "llm-state" }, [
      `${count(progress.examined)} examined`,
      progress.failed > 0 ? `, ${count(progress.failed)} the model could not answer for` : "",
      `, ${count(progress.queued)} still queued`,
      progress.mean_ms === null ? "" : ` · ${count(progress.mean_ms)} ms each`,
      llm.analysis?.paused === true ? " · paused" : "",
    ]);

    const provenance = el("p", {
      class: "llm-provenance",
      title: "Change the model or the prompt and these answers are kept — a fresh set starts",
    }, [
      "model ",
      el("code", { text: llm.pair ?? "unknown" }),
      " · prompt ",
      el("code", { text: llm.prompt_fingerprint }),
    ]);

    const body: HTMLElement[] = [head, bar, state, provenance];
    if (llm.worst.length > 0) {
      body.push(
        el("h3", { class: "llm-worst-head", text: "Highest-scoring unmatched commands" }),
        el("div", { class: "llm-worst" }, llm.worst.map((w) => this.scored(w))),
        this.openScored(),
      );
    } else if (progress.examined > 0) {
      body.push(
        el("p", { class: "llm-none" }, [
          "Nothing it examined scored 4 or above.",
        ]),
      );
    }

    return [el("section", { class: "llm" }, body)];
  }

  /** One command the model scored, with what it said about it. */
  private scored(w: Scored): HTMLElement {
    const open = el("button", {
      class: "llm-row",
      attrs: { type: "button" },
    });
    append(open, [
      // The score, never in a severity column and never given a severity word.
      el("span", { class: `llm-score llm-score-${String(w.risk_score)}`, text: String(w.risk_score) }),
      el("span", { class: "llm-row-body" }, [
        el("span", { class: "llm-intent", text: w.intent_summary }),
        el("code", { class: "llm-command", text: firstLineOf(w.command) }),
        el("span", { class: "llm-meta" }, [
          w.project_path === null ? "" : basename(w.project_path),
          w.is_destructive ? " · destructive" : "",
          w.violates_sandbox ? " · outside the project" : "",
          w.called_at === null ? "" : ` · ${dayLabel(w.called_at)} ${clock(w.called_at)}`,
        ]),
      ]),
    ]);
    open.title = w.called_at === null ? "" : fullStamp(w.called_at);
    open.addEventListener("click", () => {
      this.opts.onOpenCall(w.tool_use_id);
    });
    return open;
  }

  /** Every call the model scored 4 or above, in the timeline (task 13.15). */
  private openScored(): HTMLElement {
    const open = el("button", {
      class: "toggle",
      text: "Open all of these in the timeline",
      attrs: { type: "button" },
    });
    open.addEventListener("click", () => {
      this.opts.onOpenQuery("@model-risk:>=4");
    });
    return el("div", { class: "rules-note" }, [open]);
  }

  /**
   * One hero number: distinct calls flagged, with the rule count under it.
   *
   * Both numbers are worth having and only one of them can be the total
   * (task 11.10). The calls are the total, because that is the unit the table
   * below adds up in — each of its severity columns sums to the number here.
   *
   * The paragraph that used to sit under these boxes is gone. It explained the
   * unit, the set-aside count and why the four numbers do not add to a grand
   * total — true, and three sentences of prose read once under every review.
   * What survives is on the boxes themselves: the rule count is the line under
   * each number, and there is no grand total anywhere to be tempted by.
   */
  private severityCount(tally: RiskReview["totals"][number], review: RiskReview): HTMLElement {
    // Task 12.5's "new since the last review", as a number on the box it
    // describes rather than as a sentence under all four. Per rule rather than
    // distinct, which can overcount a call two rules newly caught — the exact
    // thing `tally.calls` exists to avoid — so it is a "+" and not a total.
    const fresh = review.findings
      .filter((f) => f.severity === tally.severity && f.dismissed === null)
      .reduce((sum, f) => sum + f.new_calls, 0);

    const card = el("div", { class: `risk-count ${tally.severity}` }, [
      el("span", { class: "risk-count-n", text: count(tally.calls) }),
      el("span", { class: "risk-count-label", text: tally.severity }),
      fresh === 0
        ? null
        : el("span", {
            class: "risk-count-new",
            text: review.first_review ? `${count(fresh)} first seen` : `+${count(fresh)} new`,
            title: review.first_review
              ? "The first review of this store: everything it found is new to it"
              : "Recorded for the first time by this review",
          }),
      el("span", {
        class: "risk-count-rules",
        text: `${count(tally.rules)} ${tally.rules === 1 ? "rule" : "rules"}, ${count(tally.calls)} ${tally.calls === 1 ? "call" : "calls"}`,
      }),
    ]);
    card.title = SEVERITY_WORDS[tally.severity];
    return card;
  }

  /** Task 6.4: the posture glance, one row per project. */
  private projects(projects: ProjectRisk[]): HTMLElement | null {
    if (projects.length === 0) return null;
    return el("div", { class: "card" }, [
      el("div", { class: "chart-titles" }, [
        el("span", {
          class: "chart-title",
          text: "By project",
          // How the columns add up is worth being able to find and not worth
          // two lines above the table every time it is read.
          title:
            "Distinct calls flagged, so each column adds up to the number above it. " +
            "Live findings only — a rule set aside stops counting against the project that tripped it.",
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
   * The way into the rules file (task 11.13).
   *
   * A paragraph explaining that rules are data, where the file goes and what
   * an id collision does used to sit here. It was three sentences of prose
   * under every review, read once. The file itself now carries all of it in
   * comments, and is created the first time this button is pressed — so the
   * explanation is where someone acts on it rather than where they don't.
   */
  private rulesFooter(): HTMLElement {
    const open = el("button", {
      class: "toggle",
      text: "Edit rules…",
      attrs: { type: "button" },
    });
    open.addEventListener("click", () => {
      void revealRules().catch((error: unknown) =>
        this.opts.onNotice(`That could not be opened: ${String(error)}`),
      );
    });
    return el("div", { class: "rules-note" }, [open]);
  }
}

/** The first line of what a call did — a heredoc body is not the command. */
function firstLine(call: ToolCall): string {
  return firstLineOf(call.input_summary ?? call.target_path);
}

/** The same, for a row that carries the command rather than the call. */
function firstLineOf(summary: string | null): string {
  const line = (summary ?? "").split("\n", 1)[0] ?? "";
  return line.length > 160 ? `${line.slice(0, 159)}…` : line;
}
