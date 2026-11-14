// Main tab: application log, View Logs menu, the accessible developer console, and the
// Install Mod / Check for Updates / Exit buttons (beamtel.py BeamTelFrame).
//
// The console record parser (CTX, CTXEND, RESP, OUT, EXECEND, LOG) and the spoken
// one-line result live in the backend; this side keeps the output backlog, the filter,
// the context list and the command history keys.

import { $, fillSelect, itemsOf, appendPreserving } from "./util.js";
import { invoke, listen, messageBox } from "./backend.js";

const CONSOLE_HISTORY_MAX = 50; // beamtel.CONSOLE_HISTORY_MAX

let logText, ctxChoice, input, output, filter, logCheck;

// Accessible-console state (BeamTelFrame.__init__)
let history = [];
let historyPos = 0; // one past the end
let searchPrefix = null; // locked prefix during history navigation
let ctxIndices = []; // context index per select item
let lines = []; // full output backlog (the filter shows a subset)

// ---- Application Log (WxLogHandler) ----

function appendLog(text) {
  appendPreserving(logText, text + "\n");
}

// ---- Console output, backlog and filter ----

function matchesFilter(line) {
  const f = filter.value.trim().toLowerCase();
  return !f || line.toLowerCase().includes(f);
}

/** _console_append */
function consoleAppend(line) {
  line = line.replace(/\n+$/, "");
  lines.push(line);
  if (matchesFilter(line)) appendPreserving(output, line + "\n");
}

/** _console_rerender: rebuild the output from the backlog under the current filter. */
function consoleRerender() {
  const visible = lines.filter(matchesFilter);
  output.value = visible.join("\n") + (visible.length ? "\n" : "");
  // SetInsertionPointEnd
  const end = output.value.length;
  output.setSelectionRange(end, end);
}

/** _on_console_clear */
function consoleClear() {
  lines = [];
  output.value = "";
}

// ---- Context list ----

function setContexts(items, keepPrevious) {
  let prevIdx = null;
  const sel = ctxChoice.selectedIndex;
  if (keepPrevious && sel >= 0 && sel < ctxIndices.length) prevIdx = ctxIndices[sel];
  fillSelect(ctxChoice, items.map((it) => ({ value: it.index, label: it.label })));
  ctxIndices = items.map((it) => it.index);
  let newSel = 0;
  if (prevIdx !== null && ctxIndices.includes(prevIdx)) newSel = ctxIndices.indexOf(prevIdx);
  ctxChoice.selectedIndex = items.length ? newSel : -1;
}

/** _rebuild_context_choice: the backend sends the accumulated CTX list on CTXEND. */
function onContexts(payload) {
  if (!payload.items || !payload.items.length) return;
  setContexts(payload.items, true);
}

// ---- Run and history ----

/** _on_console_run */
async function consoleRun() {
  const cmd = input.value.trim();
  if (!cmd) return;
  let sel = ctxChoice.selectedIndex;
  if (sel < 0) sel = 0;
  const ctxIndex = sel < ctxIndices.length ? ctxIndices[sel] : 0;
  const items = itemsOf(ctxChoice);
  const ctxLabel = sel < items.length ? items[sel].label : "";

  // History (skip consecutive duplicates); the backend persists the same list.
  if (!history.length || history[history.length - 1] !== cmd) {
    history.push(cmd);
    history = history.slice(-CONSOLE_HISTORY_MAX);
  }
  historyPos = history.length;
  searchPrefix = null;

  // Echo before sending, so it cannot land after the response.
  consoleAppend(`[${ctxLabel}] > ${cmd}`);
  input.value = "";
  input.focus();
  try {
    const result = await invoke("console_run", { index: ctxIndex, command: cmd });
    if (result && Array.isArray(result.history)) {
      history = result.history.slice(-CONSOLE_HISTORY_MAX);
      // Only reset the walk if the user has not started a new one meanwhile.
      if (searchPrefix === null) historyPos = history.length;
    }
  } catch (err) {
    console.error("console_run failed", err);
  }
}

function setInput(text) {
  input.value = text;
  const end = text.length;
  input.setSelectionRange(end, end); // SetInsertionPointEnd
}

/**
 * _on_console_input_key: Up/Down history recall with prefix search. Whatever is in the
 * field when navigation begins is locked as the prefix; Up walks back through entries
 * that start with it (a failed match leaves the field alone); Down walks forward and,
 * past the newest match, restores the typed prefix and ends the session. Any other key
 * ends the session.
 */
function onInputKey(e) {
  if (e.key === "Enter") {
    // TE_PROCESS_ENTER -> EVT_TEXT_ENTER -> _on_console_run. Enter is not Up/Down, so it
    // also ends any history session, as in wx.
    searchPrefix = null;
    e.preventDefault();
    consoleRun();
    return;
  }
  if (e.key !== "ArrowUp" && e.key !== "ArrowDown") {
    searchPrefix = null;
    return;
  }
  // wx consumed Up/Down in this field whether or not there was any history.
  e.preventDefault();
  if (!history.length) return;

  if (searchPrefix === null) {
    searchPrefix = input.value;
    historyPos = history.length;
  }
  const prefix = searchPrefix;

  if (e.key === "ArrowUp") {
    for (let idx = historyPos - 1; idx >= 0; idx--) {
      if (history[idx].startsWith(prefix)) {
        historyPos = idx;
        setInput(history[idx]);
        return;
      }
    }
    return; // no older match: leave the field unchanged
  }

  for (let idx = historyPos + 1; idx < history.length; idx++) {
    if (history[idx].startsWith(prefix)) {
      historyPos = idx;
      setInput(history[idx]);
      return;
    }
  }
  // Past the newest match: restore the originally typed prefix and end the session.
  historyPos = history.length;
  setInput(prefix);
}

// ---- Buttons ----

function onViewLogs() {
  // Anchored under the button, not at the mouse, so it opens where keyboard focus is.
  const r = $("btn_view_logs").getBoundingClientRect();
  invoke("logs_menu_popup", { x: r.left, y: r.bottom, scale: window.devicePixelRatio })
    .catch((err) => console.error("logs_menu_popup failed", err));
}

async function onCheckUpdates() {
  try {
    await invoke("check_updates");
  } catch (err) {
    // BeamTelFrame._on_check_updates' own except branch.
    await messageBox(
      "Update Check Failed",
      `The update check could not run.\n\n${err}`,
      "error",
    );
  }
}

// ---- Setup ----

/**
 * Wire the Main tab. `choices` is ui_choices(); `history0` is the persisted console
 * history. Event listeners are registered here; backlogs arrive later via
 * applyBacklog() once frontend_ready has answered.
 */
export async function initMainTab(choices, history0) {
  logText = $("log_text");
  ctxChoice = $("console_ctx_choice");
  input = $("console_input");
  output = $("console_output");
  filter = $("console_filter");
  logCheck = $("console_log_check");

  history = (history0 || []).slice(-CONSOLE_HISTORY_MAX);
  historyPos = history.length;
  setContexts(choices.console_contexts, false);

  $("btn_view_logs").addEventListener("click", onViewLogs);
  $("console_btn_refresh").addEventListener("click", () =>
    invoke("console_refresh_contexts").catch((err) => console.error(err)),
  );
  $("console_btn_run").addEventListener("click", consoleRun);
  input.addEventListener("keydown", onInputKey);
  $("console_btn_clear").addEventListener("click", consoleClear);
  filter.addEventListener("input", consoleRerender);
  logCheck.addEventListener("change", () =>
    invoke("console_set_log_stream", { enabled: logCheck.checked }).catch((err) =>
      console.error(err),
    ),
  );
  $("btn_install_mod").addEventListener("click", () =>
    invoke("install_mod").catch((err) => console.error("install_mod failed", err)),
  );
  $("btn_check_updates").addEventListener("click", onCheckUpdates);
  $("btn_exit").addEventListener("click", () =>
    invoke("exit_app").catch((err) => console.error("exit_app failed", err)),
  );

  await listen("log-line", (p) => appendLog(p.text));
  await listen("console-line", (p) => consoleAppend(p.text));
  await listen("console-contexts", onContexts);
}

/** Lines produced before the page was listening (frontend_ready's answer). */
export function applyBacklog(ready) {
  for (const text of ready.log_lines || []) appendLog(text);
  for (const text of ready.console_lines || []) consoleAppend(text);
  if (ready.console_contexts && ready.console_contexts.length) {
    setContexts(ready.console_contexts, true);
  }
}
