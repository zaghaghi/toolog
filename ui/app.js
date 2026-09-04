// The Phase 4 shell: a setup flow and a status page.
//
// Deliberately plain: no framework and no bundler yet, so the window has
// something honest to show while the timeline is built in Phase 5. The typed
// wrappers in src/bindings.ts are generated from the Rust and become the only
// call path once Vite arrives; until then this file speaks to the same commands
// through the global bridge.

const invoke = window.__TAURI__.core.invoke;
const listen = window.__TAURI__.event.listen;
const app = document.getElementById("app");

const el = (tag, attrs = {}, children = []) => {
  const node = document.createElement(tag);
  for (const [key, value] of Object.entries(attrs)) {
    if (key === "class") node.className = value;
    else if (key === "text") node.textContent = value;
    else if (key.startsWith("on")) node.addEventListener(key.slice(2), value);
    else node.setAttribute(key, value);
  }
  for (const child of [].concat(children)) {
    node.append(typeof child === "string" ? document.createTextNode(child) : child);
  }
  return node;
};

const number = (n) => (n ?? 0).toLocaleString();

async function render() {
  const [setup, status] = await Promise.all([
    invoke("doctor_status"),
    invoke("collector_status").catch(() => null),
  ]);
  app.className = "";
  app.replaceChildren(setup.configured ? home(setup, status) : wizard(setup));
}

// ---------------------------------------------------------------- first run

function wizard(setup) {
  const view = el("div");

  view.append(
    el("h1", { text: "toolog" }),
    el("p", { class: "lede", text:
      "A local record of every tool call Claude Code makes on this machine." }),
  );

  view.append(el("h2", { text: "What is captured" }));
  view.append(el("ul", { class: "plain" }, [
    el("li", {}, [el("strong", { text: "From your transcripts: " }),
      "the full command or file each tool call ran, and its result."]),
    el("li", {}, [el("strong", { text: "From Claude Code's telemetry: " }),
      "who approved each call, how long it took, what it cost — and the calls you refused, which nothing else records."]),
  ]));

  view.append(el("h2", { text: "What leaves this machine" }));
  view.append(el("p", {}, [
    el("strong", { text: "Nothing. " }),
    "The receiver binds to 127.0.0.1 and the database is a file in your Library folder. " +
    "Prompts and assistant replies are not captured at all.",
  ]));

  view.append(el("h2", { text: "To switch it on" }));
  view.append(el("p", {}, [
    "toolog will add six environment variables to ",
    el("span", { class: "mono", text: setup.settings_path }),
    ". Your existing settings are kept, and a timestamped backup is written first.",
  ]));

  const enable = el("button", { class: "primary", text: "Enable capture" });
  enable.addEventListener("click", async () => {
    enable.disabled = true;
    enable.textContent = "Writing…";
    try {
      await invoke("apply_doctor_fix");
      await render();
    } catch (e) {
      enable.disabled = false;
      enable.textContent = "Enable capture";
      view.append(el("div", { class: "problem", text: String(e) }));
    }
  });

  view.append(el("div", { class: "actions" }, [enable]));

  if (setup.problems.length) {
    view.append(el("h2", { text: "Worth knowing first" }));
    for (const problem of setup.problems) {
      view.append(el("div", { class: "problem", text: problem }));
    }
  }

  return view;
}

// ------------------------------------------------------------------- home

function home(setup, status) {
  const view = el("div");
  const live = status && !status.paused;

  view.append(
    el("h1", { text: "toolog" }),
    el("span", {
      class: `pill ${status ? (live ? "on" : "idle") : "off"}`,
      text: status
        ? (live ? `Capturing on ${status.endpoint.replace("http://", "")}` : "Capture paused")
        : "Not capturing",
    }),
  );

  if (status) {
    view.append(el("h2", { text: "Today" }));
    view.append(el("div", { class: "card" }, [
      statRow("Events stored today", number(status.events_today)),
      statRow("Tool calls in the store", number(status.tool_calls)),
      statRow("OTLP batches received", number(status.counters.batches)),
      statRow("Batches dropped", number(status.counters.dropped)),
    ]));
  }

  view.append(el("h2", { text: "History" }));
  view.append(el("div", { class: "card" }, [
    statRow("Transcripts on disk", number(setup.transcript_files)),
    statRow("Already imported", number(setup.ingested_files)),
  ]));

  const backfill = el("button", { text: "Import history" });
  backfill.addEventListener("click", async () => {
    backfill.disabled = true;
    backfill.textContent = "Importing…";
    try {
      const summary = await invoke("run_backfill");
      backfill.textContent = `Imported ${number(summary.stored)} new records`;
    } catch (e) {
      backfill.textContent = String(e);
    }
    setTimeout(render, 2500);
  });

  const pause = el("button", { text: live ? "Pause capture" : "Resume capture" });
  pause.addEventListener("click", async () => {
    pause.disabled = true;
    await invoke("set_paused", { paused: live });
    await render();
  });

  const agent = el("button", {
    text: setup.agent_installed ? "Stop starting at login" : "Start at login",
  });
  agent.disabled = !setup.agent_supported;
  agent.addEventListener("click", async () => {
    agent.disabled = true;
    await invoke("set_login_agent", { install: !setup.agent_installed });
    await render();
  });

  view.append(el("div", { class: "actions" }, [backfill, pause, agent]));

  if (setup.problems.length) {
    view.append(el("h2", { text: "Needs attention" }));
    for (const problem of setup.problems) {
      view.append(el("div", { class: "problem", text: problem }));
    }
  }

  view.append(el("h2", { text: "Live" }));
  const feed = el("div", { class: "card mono", id: "feed" }, [
    el("div", { class: "note", text: "Waiting for the next tool call…" }),
  ]);
  view.append(feed);

  view.append(el("h2", { text: "Diagnostics" }));
  view.append(el("pre", { class: "report", text: setup.report }));

  view.append(el("footer", {}, [
    el("div", { class: "note", text:
      "The timeline, search, risk review and analytics arrive in the next phases. " +
      "Everything above is already being recorded." }),
  ]));

  return view;
}

function statRow(label, value) {
  return el("div", { class: "row" }, [
    el("span", { class: "label", text: label }),
    el("span", { class: "value", text: value }),
  ]);
}

// Live tool calls, straight from the capture pipeline.
listen("live_tool_call", (event) => {
  const feed = document.getElementById("feed");
  if (!feed) return;
  const call = event.payload;
  const line = el("div", {
    text: `${call.tool_name ?? "?"}  ${call.input_summary ?? call.target_path ?? ""}`.slice(0, 110),
  });
  if (feed.firstChild && feed.firstChild.className === "note") feed.replaceChildren();
  feed.prepend(line);
  while (feed.childElementCount > 8) feed.lastChild.remove();
});

render().catch((e) => {
  app.className = "";
  app.replaceChildren(el("div", { class: "problem", text: String(e) }));
});
