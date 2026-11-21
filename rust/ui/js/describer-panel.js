// AI Describer tab (config_ui.py AIDescriberPanel).
//
// The provider select drives everything below it: the API key group heading and the
// intro text name the provider, the model list is repopulated, and only that provider's
// extra request parameters are shown (the Advanced group disappears when it has none).
// Every change saves at once except the text extras, which save two seconds after the
// last keystroke. The backend merges each save onto a fresh read of the config file
// (CONTRACT.md, describer_save).

import { $, fillSelect, itemsOf, setRow, ownsFocus, serialQueue } from "./util.js";
import { invoke, messageBox } from "./backend.js";
import { askApiKey } from "./dialogs.js";

const AUTO_SAVE_DELAY_MS = 2000; // config_ui._AUTO_SAVE_DELAY_MS

let providers = []; // ui_choices().ai_providers
let defaultProvider = "";
let provider = ""; // self._provider
let state = null; // the backend's view of this panel's cur_cfg (describer_load shape)
let saveTimer = null; // debounce for text extras
const extraRows = new Map(); // key -> {row, ctrl, ex}
const enqueue = serialQueue();
const el = {};

function info(pid) {
  return providers.find((p) => p.id === pid) || providers.find((p) => p.id === defaultProvider);
}

function providerState(pid) {
  return (state && state.providers && state.providers[pid]) || { has_key: false, extras: {} };
}

// ---- Extras ----

/** _build_extra_row: one labelled control per extra, built once for every provider. */
function buildExtraRow(ex) {
  const row = document.createElement("div");
  row.className = "row";
  row.id = `extra_row_${ex.key}`;
  const label = document.createElement("label");
  label.htmlFor = `extra_${ex.key}`;
  label.textContent = ex.label;
  label.title = ex.help;
  const cap = document.createElement("span");
  cap.className = "cap";
  cap.append(label);
  let ctrl;
  if (ex.kind === "choice") {
    ctrl = document.createElement("select");
    fillSelect(ctrl, ex.values);
    ctrl.addEventListener("change", onChange);
  } else {
    ctrl = document.createElement("input");
    ctrl.type = "text";
    ctrl.className = "grow";
    ctrl.autocomplete = "off";
    ctrl.spellcheck = false;
    // Saving per keystroke would rewrite the config file on every character typed.
    ctrl.addEventListener("input", onExtraText);
  }
  ctrl.id = `extra_${ex.key}`;
  row.append(cap, ctrl);
  el.sb_extra.append(row);
  extraRows.set(ex.key, { row, ctrl, ex });
}

/** _extra_value: one extra control's value, or its default when unusable. */
function extraValue(ex, ctrl) {
  if (ex.kind === "choice") {
    const sel = ctrl.selectedIndex;
    if (sel >= 0 && sel < ex.values.length) return ex.values[sel].value;
    return ex.default;
  }
  return (ctrl.value || "").trim() || ex.default;
}

/** _show_extras_for_provider */
function showExtrasForProvider() {
  const active = new Set(info(provider).extras.map((ex) => ex.key));
  for (const [key, { row, ctrl }] of extraRows) {
    setRow(row, [ctrl], active.has(key), el.choice_provider);
  }
  // Drop the whole group when the provider declares no extras.
  const hasAny = active.size > 0;
  if (!hasAny && ownsFocus(el.sb_extra)) el.choice_provider.focus();
  el.sb_extra.hidden = !hasAny;
}

// ---- Persistence ----

function selectedProvider() {
  const sel = el.choice_provider.selectedIndex;
  if (sel >= 0 && sel < providers.length) return providers[sel].id;
  return defaultProvider;
}

function selectedModelName() {
  const items = itemsOf(el.choice_model);
  const sel = el.choice_model.selectedIndex;
  if (sel >= 0 && sel < items.length) return items[sel].value;
  return info(provider).default_model;
}

/**
 * _save. Only the active provider's model and extras are sent; the backend writes them
 * onto a fresh disk read. keyAction is "keep", "clear" or "set" (with the plaintext key,
 * which the backend seals before writing).
 */
function save(keyAction = "keep", key = undefined) {
  const p = provider;
  const extras = {};
  for (const ex of info(p).extras) extras[ex.key] = extraValue(ex, extraRows.get(ex.key).ctrl);
  const args = {
    provider: p,
    model: selectedModelName(),
    extras,
    disable_ui_toggle: el.chk_disable_ui_toggle.checked,
    key_action: keyAction,
  };
  if (keyAction === "set") args.key = key;
  return enqueue(async () => {
    try {
      const r = await invoke("describer_save", args);
      if (r && r.state) state = r.state;
    } catch (err) {
      console.error("describer_save failed", err);
    }
  });
}

function onChange() {
  return save();
}

function onExtraText() {
  if (saveTimer !== null) clearTimeout(saveTimer);
  saveTimer = setTimeout(() => {
    saveTimer = null;
    save();
  }, AUTO_SAVE_DELAY_MS);
}

/** flush_pending_save */
export async function flushPendingSave() {
  if (saveTimer !== null) {
    clearTimeout(saveTimer);
    saveTimer = null;
    await save();
  }
}

/** on_change_provider: repoint every provider-specific control at the new service. */
async function onChangeProvider() {
  // Flush a half-typed base URL against the OLD provider before the keys change.
  await flushPendingSave();
  provider = selectedProvider();
  retitleForProvider();
  loadProviderControls();
  showExtrasForProvider();
  refreshKeyButton();
  await save();
}

/** _retitle_for_provider */
function retitleForProvider() {
  const display = info(provider).display;
  el.sb_key_heading.textContent = `${display} API Key`;
  el.lbl_model.title = `The ${display} model used to describe the scene.`;
  el.lbl_intro.textContent =
    "Press F10 then Space in-game to describe the current scene using " +
    `${display}. The description is spoken and added to the speech buffer.`;
}

/** _load_provider_controls: model list and extras from the active provider's config. */
function loadProviderControls() {
  const pi = info(provider);
  const ps = providerState(provider);
  fillSelect(el.choice_model, pi.models);
  const want = ps.model === undefined ? pi.default_model : ps.model;
  const idx = pi.models.findIndex((m) => m.value === want);
  el.choice_model.selectedIndex = idx >= 0 ? idx : 0;

  for (const ex of pi.extras) {
    const { ctrl } = extraRows.get(ex.key);
    const raw = ps.extras ? ps.extras[ex.key] : undefined;
    const val = raw === undefined ? ex.default : raw;
    if (ex.kind === "choice") {
      const values = ex.values.map((v) => v.value);
      ctrl.selectedIndex = values.includes(val) ? values.indexOf(val) : values.indexOf(ex.default);
    } else {
      ctrl.value = String(val || "").trim() || ex.default;
    }
  }
}

/** load_into_controls */
function loadIntoControls(s) {
  state = s;
  let p = s.provider;
  if (!providers.some((x) => x.id === p)) p = defaultProvider;
  provider = p;
  el.choice_provider.selectedIndex = providers.findIndex((x) => x.id === p);
  retitleForProvider();
  loadProviderControls();
  showExtrasForProvider();
  el.chk_disable_ui_toggle.checked = !!s.disable_ui_toggle;
  refreshKeyButton();
}

// ---- API key ----

function hasKey() {
  return !!providerState(provider).has_key;
}

/** _refresh_key_button */
function refreshKeyButton() {
  if (hasKey()) {
    el.btn_api_key.textContent = "Clear API key";
    el.lbl_key_status.textContent = "An API key is configured.";
  } else {
    el.btn_api_key.textContent = "Set API key";
    el.lbl_key_status.textContent = "No API key configured.";
  }
}

async function onApiKeyButton() {
  if (hasKey()) await clearApiKey();
  else await setApiKey();
}

/** _clear_api_key */
async function clearApiKey() {
  const display = info(provider).display;
  const ans = await messageBox(
    "Clear API Key",
    `Remove your stored ${display} API key?`,
    "question", "yes_no", "no",
  );
  if (ans !== "yes") return;
  await save("clear");
  refreshKeyButton();
}

/** _set_api_key: ask, then validate in the background with the button disabled. */
async function setApiKey() {
  const entered = await askApiKey(info(provider).key_help);
  if (entered === null) return;
  const key = entered.trim();
  if (!key) return;

  el.btn_api_key.disabled = true;
  // Departure (python-bugs.md 24): wx left the provider choice live, and a switch during
  // validation saved the key under the new provider, overwriting that provider's key.
  // The key belongs to the provider it was entered for, so the choice waits.
  el.choice_provider.disabled = true;
  el.lbl_key_status.textContent = "Validating API key...";

  const p = provider;
  // Validate against the endpoint the describer will call, so a mistyped base URL or an
  // out-of-reach model surfaces here rather than in game.
  const extras = {};
  for (const ex of info(p).extras) extras[ex.arg] = extraValue(ex, extraRows.get(ex.key).ctrl);
  const model = selectedModelName();

  let result;
  try {
    result = await invoke("describer_validate_key", { provider: p, key, model, extras });
  } catch (err) {
    result = { ok: false, error: String(err) };
  }
  await onValidated(key, result.ok, result.error);
}

/** _on_validated. Focus goes back to the button so its new label is announced. */
async function onValidated(key, ok, err) {
  el.btn_api_key.disabled = false;
  el.choice_provider.disabled = false;
  el.btn_api_key.focus();
  if (ok) {
    // The provider cannot change while validating, so this is the one it was entered for.
    await save("set", key);
    refreshKeyButton();
    await messageBox("API Key", "API key validated and saved.", "info");
  } else {
    refreshKeyButton();
    await messageBox("Invalid API Key", `The API key could not be validated:\n\n${err}`, "error");
  }
}

// ---- Setup ----

/** Wire the AI Describer panel (AIDescriberPanel.__init__). */
export async function initDescriberPanel(choices) {
  for (const id of [
    "lbl_intro", "choice_provider", "sb_key_heading", "btn_api_key", "lbl_key_status",
    "lbl_model", "choice_model", "sb_extra", "chk_disable_ui_toggle",
  ]) el[id] = $(id);

  providers = choices.ai_providers;
  defaultProvider = choices.ai_default_provider;
  fillSelect(el.choice_provider, providers.map((p) => ({ value: p.id, label: p.display })));
  for (const p of providers) for (const ex of p.extras) buildExtraRow(ex);

  el.choice_provider.addEventListener("change", onChangeProvider);
  el.choice_model.addEventListener("change", onChange);
  el.chk_disable_ui_toggle.addEventListener("change", onChange);
  el.btn_api_key.addEventListener("click", onApiKeyButton);

  loadIntoControls(await invoke("describer_load"));
}
