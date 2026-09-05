//! The live view (tasks 6.10–6.12).
//!
//! Two halves that answer different questions. The **lanes** answer "what is
//! running right now": one per session, with the tool in flight, a running
//! cost meter and whether it has gone quiet. The **feed** answers "what just
//! happened": every call as it lands, newest at the bottom, scrolling itself
//! until you touch it.
//!
//! Three things are deliberate:
//!
//! - **Nothing here says a session ended.** The store has no such record —
//!   Claude Code does not announce one — so a lane goes *idle* after a
//!   threshold this view states, and drops off the list when it leaves the
//!   window. "Idle" is an observation; "finished" would be a guess.
//! - **Auto-scroll yields to the reader.** Scrolling up pins the feed and says
//!   so, with a button to rejoin. A log that yanks itself out from under you
//!   while you read a line is unusable, and this is a tool for reading lines.
//! - **Notifications are off.** Both switches start off and are individually
//!   toggleable (task 6.12); the resident process keeps them, so they survive
//!   a restart the way the login agent does.

import type { LiveSession, Prefs, ToolCall } from "./bindings";
import { getPrefs, liveSessions, setPrefs } from "./bindings";
import { statTile } from "./chart";
import { append, el, fill, orDash, span } from "./dom";
import { basename, clock, cost, count, duration, elapsed } from "./format";

/** How far back counts as a live session. */
const WINDOW_MS = 30 * 60_000;
/** No call for this long and a lane reads as idle rather than working. */
const IDLE_AFTER_MS = 2 * 60_000;
/** How often the lanes are re-read. The feed is pushed, not polled. */
const LANE_REFRESH_MS = 5_000;
/** How many calls the feed keeps. Older ones are in the timeline. */
const FEED_MAX = 300;
/** Within this many pixels of the bottom still counts as "at the bottom". */
const STICK_SLACK = 24;

/** Whether a session has gone quiet, and for how long. */
function idleFor(session: LiveSession, now: number): number | null {
  if (session.last_call_at === null) return null;
  const quiet = now - session.last_call_at;
  return quiet >= IDLE_AFTER_MS ? quiet : null;
}

export interface LiveOptions {
  onNotice: (message: string) => void;
  /** Open one call in the timeline. */
  onOpenCall: (toolUseId: string) => void;
}

export class LiveView {
  readonly node: HTMLElement;

  private readonly lanes = el("div", { class: "lanes" });
  private readonly feed = el("div", { class: "feed", role: "log", attrs: { "aria-live": "polite" } });
  private readonly feedEmpty = el("p", {
    class: "chart-empty feed-empty",
    text: "Nothing since this view opened. Calls appear here the moment either lane stores them.",
  });
  private readonly resume = el("button", { class: "newpill feed-resume", hidden: true, text: "Jump to the newest" });
  private readonly switches = el("div", { class: "card" });
  private readonly summary = el("div", { class: "tiles" });

  private sessions: LiveSession[] = [];
  /** Newest last, which is the order a log is read in. */
  private calls: ToolCall[] = [];
  private readonly seen = new Map<string, number>();
  private prefs: Prefs = {
    notify_refusals: false,
    notify_high_risk: false,
    redact_evidence: false,
    excluded_projects: [],
  };
  private stuck = true;
  private timer = 0;

  constructor(private readonly opts: LiveOptions) {
    // An empty box with a border says nothing; the sentence says what to
    // expect. Both swap rather than the box holding placeholder rows.
    this.feed.hidden = true;
    this.feed.addEventListener("scroll", () => {
      const bottom = this.feed.scrollHeight - this.feed.scrollTop - this.feed.clientHeight;
      // Pause on interaction: the reader scrolling up wins over the stream.
      this.stuck = bottom <= STICK_SLACK;
      this.resume.hidden = this.stuck;
    });
    this.resume.addEventListener("click", () => {
      this.stuck = true;
      this.resume.hidden = true;
      this.feed.scrollTop = this.feed.scrollHeight;
    });

    this.node = el("section", { class: "live" }, [
      el("div", { class: "sheet" }, [
        this.summary,
        el("div", { class: "live-head" }, [
          span("chart-title", "Sessions"),
          span("chart-caption", `Active in the last ${Math.round(WINDOW_MS / 60_000)} minutes.`),
        ]),
        this.lanes,
        el("div", { class: "live-head" }, [
          span("chart-title", "As it happens"),
          span("chart-caption", "Every call either lane stores, newest last."),
        ]),
        el("div", { class: "feed-wrap" }, [this.feed, this.feedEmpty, this.resume]),
        this.switches,
      ]),
    ]);
  }

  /** Start polling the lanes. Called when the tab is shown. */
  start(): void {
    if (this.timer !== 0) return;
    void this.refresh();
    this.timer = window.setInterval(() => void this.loadSessions(), LANE_REFRESH_MS);
  }

  /** Stop polling. A hidden tab has no business querying every five seconds. */
  stop(): void {
    if (this.timer === 0) return;
    clearInterval(this.timer);
    this.timer = 0;
  }

  async refresh(): Promise<void> {
    try {
      this.prefs = await getPrefs();
      this.drawSwitches();
    } catch (error) {
      this.opts.onNotice(`Preferences could not be read: ${String(error)}`);
    }
    await this.loadSessions();
  }

  private async loadSessions(): Promise<void> {
    try {
      this.sessions = await liveSessions(WINDOW_MS);
      this.drawLanes();
      this.drawSummary();
    } catch (error) {
      this.opts.onNotice(`Live sessions could not be read: ${String(error)}`);
      this.stop();
    }
  }

  /**
   * A call has just been stored (task 6.9's channel).
   *
   * The same call arrives more than once by design — the transcript creates
   * the row and OTEL completes it with a duration and a decision — so this
   * replaces the entry rather than appending a second one.
   */
  noteCall(call: ToolCall): void {
    const at = this.seen.get(call.tool_use_id);
    if (at !== undefined) {
      this.calls[at] = call;
      const row = this.feed.children[at];
      if (row !== undefined) this.feed.replaceChild(this.feedRow(call), row);
      return;
    }

    this.seen.set(call.tool_use_id, this.calls.length);
    this.calls.push(call);
    append(this.feed, this.feedRow(call));
    this.feedEmpty.hidden = true;
    this.feed.hidden = false;

    if (this.calls.length > FEED_MAX) {
      // Drop the oldest and reindex: the feed is a window on the stream, and
      // everything that leaves it is still in the timeline.
      const dropped = this.calls.shift();
      this.feed.firstChild?.remove();
      if (dropped !== undefined) this.seen.delete(dropped.tool_use_id);
      for (const [i, c] of this.calls.entries()) this.seen.set(c.tool_use_id, i);
    }

    if (this.stuck) this.feed.scrollTop = this.feed.scrollHeight;
  }

  // ------------------------------------------------------------------ draw

  private drawSummary(): void {
    const now = Date.now();
    const active = this.sessions.filter((s) => idleFor(s, now) === null);
    const priced = this.sessions.filter((s) => s.priced);
    const spend = priced.reduce((sum, s) => sum + s.cost_usd_micros, 0);

    fill(this.summary, [
      statTile({
        label: "Sessions in flight",
        value: count(active.length),
        note:
          this.sessions.length === active.length
            ? undefined
            : `${count(this.sessions.length - active.length)} idle for over ${Math.round(IDLE_AFTER_MS / 60_000)} minutes`,
      }),
      statTile({
        label: "Calls seen here",
        value: count(this.calls.length),
        note: this.calls.length >= FEED_MAX ? `the feed keeps the last ${count(FEED_MAX)}` : undefined,
      }),
      statTile({
        label: "Spend in flight",
        value: priced.length === 0 ? "not captured" : cost(spend),
        note:
          priced.length === 0
            ? "no live session is reporting cost"
            : `${count(priced.length)} of ${count(this.sessions.length)} sessions priced`,
      }),
    ]);
  }

  private drawLanes(): void {
    if (this.sessions.length === 0) {
      fill(this.lanes, [
        el("p", { class: "chart-empty" }, [
          "No session has run a tool in the last half hour. Start Claude Code anywhere and its calls appear here as they are stored.",
        ]),
      ]);
      return;
    }

    const now = Date.now();
    // Cost is the only measure with a scale worth comparing across lanes; the
    // meters share it so a wide bar means more money, not a wider card.
    const mostCost = Math.max(1, ...this.sessions.map((s) => s.cost_usd_micros));

    fill(
      this.lanes,
      this.sessions.map((s) => {
        const quiet = idleFor(s, now);
        return el("div", { class: quiet === null ? "lane" : "lane idle" }, [
          el("div", { class: "lane-head" }, [
            el("span", { class: "lane-project", text: basename(s.project_path), title: s.project_path ?? "" }),
            s.git_branch === null ? null : span("lane-branch mono", s.git_branch),
            el("span", { class: "grow" }),
            el("span", {
              class: quiet === null ? "pill on" : "pill idle",
              text: quiet === null ? "active" : `idle ${elapsed(quiet)}`,
            }),
          ]),
          el("div", { class: "lane-now" }, [
            span("lane-tool", orDash(s.current_tool)),
            s.current_success === null
              ? span("lane-state none", "no outcome yet")
              : s.current_success
                ? span("lane-state ok", "finished")
                : span("lane-state bad", "failed"),
          ]),
          // The running cost meter (task 6.10), and it says when there is none.
          el("div", { class: "lane-cost" }, [
            el("div", { class: "meter-head" }, [
              span("meter-label", s.priced ? "Spend so far" : "Spend"),
              span("meter-value", s.priced ? cost(s.cost_usd_micros) : "not captured"),
            ]),
            el("div", { class: s.priced ? "meter-track" : "meter-track empty" }, [
              s.priced
                ? el("div", {
                    class: "meter-fill",
                    style: { width: `${((s.cost_usd_micros / mostCost) * 100).toFixed(1)}%` },
                  })
                : null,
            ]),
          ]),
          el("dl", { class: "kv lane-facts" }, [
            el("dt", { text: "Calls" }),
            el("dd", { text: count(s.calls) }),
            el("dt", { text: "Failed" }),
            el("dd", { text: count(s.failures) }),
            el("dt", { text: "Refused" }),
            el("dd", { text: count(s.refused) }),
            el("dt", { text: "Mode" }),
            el("dd", { text: orDash(s.permission_mode) }),
            el("dt", { text: "Since" }),
            el("dd", { text: clock(s.first_call_at) }),
          ]),
          statTile({ label: "Calls per minute", value: count(s.recent.at(-1) ?? 0), trend: s.recent }),
        ]);
      }),
    );
  }

  private feedRow(call: ToolCall): HTMLElement {
    const row = el("button", { class: "feed-row", attrs: { type: "button" } }, [
      span("fc-time", clock(call.called_at)),
      span("fc-tool", orDash(call.tool_name)),
      span("fc-what mono", firstLine(call)),
      call.duration_ms === null ? null : span("fc-dur", duration(call.duration_ms)),
      call.decision === "reject" ? span("fc-flag refused", "refused") : null,
      call.success === false ? span("fc-flag failed", "failed") : null,
    ]);
    row.addEventListener("click", () => this.opts.onOpenCall(call.tool_use_id));
    return row;
  }

  /** Task 6.12: two switches, both off until someone turns them on. */
  private drawSwitches(): void {
    fill(this.switches, [
      el("div", { class: "chart-titles" }, [
        span("chart-title", "Notifications"),
        span(
          "chart-caption",
          "Off until you turn them on, and each one on its own. Nothing leaves this machine either way.",
        ),
      ]),
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
    ]);
  }

  /** The switches this view owns. Narrowed so a non-boolean field cannot be
   * passed to a checkbox. */
  private toggle(
    key: "notify_refusals" | "notify_high_risk",
    label: string,
    why: string,
  ): HTMLElement {
    const box = el("input", { type: "checkbox", attrs: { id: `pref-${key}` } });
    box.checked = this.prefs[key];
    box.addEventListener("change", () => {
      const next: Prefs = { ...this.prefs, [key]: box.checked };
      void setPrefs(next)
        .then((saved) => {
          this.prefs = saved;
          box.checked = saved[key];
        })
        .catch((error: unknown) => {
          box.checked = this.prefs[key];
          this.opts.onNotice(`That switch could not be saved: ${String(error)}`);
        });
    });

    return el("div", { class: "switch" }, [
      box,
      el("div", {}, [
        el("label", { attrs: { for: `pref-${key}` }, class: "switch-label", text: label }),
        span("switch-why", why),
      ]),
    ]);
  }
}

/** The first line of what a call did — a heredoc body is not the command. */
function firstLine(call: ToolCall): string {
  const summary = call.input_summary ?? call.target_path ?? "";
  const line = summary.split("\n", 1)[0] ?? "";
  return line.length > 160 ? `${line.slice(0, 159)}…` : line;
}
