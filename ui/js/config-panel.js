// Configuration tab (config_ui.py ConfigPanel).
//
// Saving is debounced by two seconds after the last change (_schedule_save/_auto_save)
// and flushed on close (flush_pending_save). Programmatic fills run under `loading`, so
// they never schedule a save (_loading). Every choice list comes from ui_choices(); the
// backend turns these control values into the config file (CONTRACT.md,
// config_save_panel).

import {
  $, fillSelect, itemsOf, selectedValue, selectedLabel, selectValue, indexOfValue,
  enable, setRow, initSpin, setSpin, getSpin, serialQueue,
} from "./util.js";
import { invoke, messageBox } from "./backend.js";

const AUTO_SAVE_DELAY_MS = 2000; // config_ui._AUTO_SAVE_DELAY_MS

// Config key -> element id, by control kind. The order is controls_to_config's.
const CHECKS = [
  ["implement_tones_enabled", "chk_impl_tones"],
  ["implement_proximity_speech", "chk_impl_proximity"],
  ["dock_tones_enabled", "chk_dock_tones"],
  ["scan_tones_enabled", "chk_scan_tones"],
  ["cannon_shot_readout", "chk_cannon_shot"],
  ["binding_learn_speak_description", "chk_learn_desc"],
  ["compass_highlight_enabled", "chk_compass_highlight"],
  ["road_follow_guidance_enabled", "chk_road_follow"],
  ["road_stop_sign_speech_enabled", "chk_road_stop_signs"],
  ["road_traffic_light_speech_enabled", "chk_road_traffic_lights"],
  ["road_speed_limit_speech_enabled", "chk_road_speed_limits"],
  ["road_surface_speech_enabled", "chk_road_surface"],
  ["road_junction_speech_enabled", "chk_road_junction_speech"],
  ["road_junction_earcon_enabled", "chk_road_junction_earcon"],
  ["road_junction_proximity_enabled", "chk_road_junction_proximity"],
  ["road_include_private", "chk_road_private"],
  ["hrtf_enabled", "chk_hrtf_enabled"],
  ["hrtf_equalise", "chk_hrtf_equalise"],
  ["pitch_roll_tones_enabled", "chk_pitch_roll_enabled"],
  ["oil_chime_enabled", "chk_oil_chime_enabled"],
  ["tc_clicks_enabled", "chk_tc_clicks_enabled"],
  ["launch_beamng", "chk_launch_beamng"],
  ["update_check_enabled", "chk_update_check"],
  ["follow_default_audio_device", "chk_follow_device"],
  ["announce_turn_signals", "chk_announce_turn_signals"],
  ["announce_speed", "chk_announce_speed"],
  ["drift_speech_enabled", "chk_drift_speech"],
  ["gear_speech_sport_once", "chk_gear_sport_once"],
  ["announce_clickspot_actions", "chk_announce_clickspot_actions"],
  ["scanner_distance_callout_enabled", "chk_scanner_callout"],
  ["scanner_steer_tone_enabled", "chk_scanner_steer_tone"],
  ["ui_nav_hold_suppression", "chk_ui_nav_hold"],
  ["mcp_server_enabled", "chk_mcp_server"],
];

const SPINS = [
  ["speech_rate", "spin_rate"],
  ["speech_volume", "spin_volume"],
  ["shift_tone_frequency_hz", "spin_freq"],
  ["shift_tone_level_dbfs", "spin_level"],
  ["check_engine_buzzer_level_dbfs", "spin_buzzer_level"],
  ["oil_chime_level_dbfs", "spin_chime_level"],
  ["obstacle_buzz_volume_db", "spin_obstacle_level"],
  ["implement_ground_tone_dbfs", "spin_impl_ground"],
  ["implement_tilt_tone_dbfs", "spin_impl_tilt"],
  ["dock_tone_dbfs", "spin_dock"],
  ["scan_tone_dbfs", "spin_scan"],
  ["compass_click_interval", "spin_compass_interval"],
  ["compass_highlight_nth_click", "spin_compass_highlight_nth"],
  ["compass_click_level_dbfs", "spin_compass_click_level"],
  ["lowspeed_click_level_dbfs", "spin_lowspeed_click_level"],
  ["lowspeed_stop_tone_level_dbfs", "spin_lowspeed_stop_level"],
  ["slip_tone_level_dbfs", "spin_slip_tone_level"],
  ["placement_ping_volume_db", "spin_placement_ping_level"],
  ["road_beep_volume_db", "spin_road_beep"],
  ["road_correction_volume_db", "spin_road_correction"],
  ["road_junction_volume_db", "spin_road_junction"],
  ["hrtf_elevation_deg", "spin_hrtf_elevation"],
  ["hrtf_front_emphasis_db", "spin_hrtf_front_emphasis"],
  ["hrtf_distance_gain_db", "spin_hrtf_distance_gain"],
  ["pitch_roll_max_dbfs", "spin_pitch_roll_max"],
  ["pitch_roll_min_dbfs", "spin_pitch_roll_min"],
  ["audio_poll_interval_sec", "spin_poll"],
  ["gear_tone_level_dbfs", "spin_gear_tone_level"],
  ["scanner_base_freq_hz", "spin_scanner_base_freq"],
  ["scanner_pitch_offset_oct", "spin_scanner_offset"],
  ["mcp_server_port", "spin_mcp_port"],
];

// Selects whose items come from ui_choices and whose value is the config value. The
// fallback is what controls_to_config writes when nothing is selected.
const SELECTS = [
  // [config key, element id, ui_choices key, fallback when nothing is selected]
  ["obstacle_warning_sensitivity", "choice_obstacle_sensitivity", "obstacle_sensitivities", "first-or-normal"],
  ["hrtf_set", "choice_hrtf_set", "hrtf_sets", "first"],
  ["beamng_renderer", "choice_renderer", "renderers", "first"],
  ["gear_report_mode", "choice_gear_mode", "gear_modes", "first"],
  ["gear_speech_number_style", "choice_gear_number_style", "gear_number_styles", "first"],
  ["gear_tone_root", "choice_gear_root", "gear_roots", "first"],
  ["gear_tone_octave", "choice_gear_octave", "gear_octaves", "first"],
  ["gear_tone_scale", "choice_gear_scale", "gear_scales", "first"],
  ["scanner_distance_callout_interval", "choice_callout_interval", "callout_intervals", 10],
];

let choices;
let loading = false; // _loading
let saveTimer = null; // _save_timer
let curVoiceName = ""; // cur_cfg["speech_voice_name"]
let voices = []; // self.voices
let deviceNames = []; // the WASAPI output devices last listed
let backends = []; // self._backends
let voiceRequest = 0; // drops a stale voice listing if the backend changed meanwhile
const enqueue = serialQueue();
const el = {};

// ---------------------------------------------------------------------------
// Radio boxes (wx.RadioBox -> nested fieldset of radios)
// ---------------------------------------------------------------------------

function buildRadios(fieldset, name, items) {
  items.forEach((it, i) => {
    const wrap = document.createElement("div");
    wrap.className = "check";
    const input = document.createElement("input");
    input.type = "radio";
    input.name = name;
    input.id = `${fieldset.id}_${i}`;
    input.value = String(i);
    const label = document.createElement("label");
    label.htmlFor = input.id;
    label.textContent = it.label;
    wrap.append(input, label);
    fieldset.append(wrap);
  });
  fieldset._items = items;
}

function radioValue(fieldset) {
  const items = fieldset._items;
  const checked = fieldset.querySelector("input[type=radio]:checked");
  return checked ? items[Number(checked.value)].value : items[0].value;
}

function setRadio(fieldset, value) {
  const items = fieldset._items;
  let i = items.findIndex((it) => it.value === value);
  if (i < 0) i = 0;
  fieldset.querySelectorAll("input[type=radio]").forEach((r) => {
    r.checked = Number(r.value) === i;
  });
}

// ---------------------------------------------------------------------------
// Speech backend helpers
// ---------------------------------------------------------------------------

/** _populate_backends */
async function populateBackends() {
  backends = await invoke("speech_list_backends");
  fillSelect(el.choice_backend, [
    { value: "auto", label: choices.speech_auto_label },
    ...backends.map((nm) => ({ value: nm, label: nm })),
  ]);
  el.choice_backend.selectedIndex = 0;
}

/** _select_backend */
function selectBackend(name) {
  const want = (name || "auto").trim();
  let idx = 0;
  if (want.toLowerCase() !== "auto") {
    const i = backends.findIndex((nm) => nm.toLowerCase() === want.toLowerCase());
    if (i >= 0) idx = i + 1; // +1 for the Auto entry
  }
  el.choice_backend.selectedIndex = idx;
}

/** _selected_backend */
function selectedBackend() {
  const sel = el.choice_backend.selectedIndex;
  if (sel <= 0 || sel - 1 >= backends.length) return "auto";
  return backends[sel - 1];
}

/**
 * _apply_backend_capabilities: disable what the backend does not implement, and say so,
 * so the disabled controls are explained rather than just inert.
 */
function applyBackendCapabilities(feats) {
  if (!feats) {
    for (const c of [el.choice_voice, el.spin_rate, el.spin_volume]) {
      enable(c, false, el.choice_backend);
    }
    el.lbl_speech_caps.textContent = "Backend not available on this machine.";
    return;
  }
  enable(el.choice_voice, !!feats.supports_set_voice, el.choice_backend);
  enable(el.spin_rate, !!feats.supports_set_rate, el.choice_backend);
  enable(el.spin_volume, !!feats.supports_set_volume, el.choice_backend);
  const can = [
    ["voice", feats.supports_set_voice],
    ["rate", feats.supports_set_rate],
    ["volume", feats.supports_set_volume],
    ["braille", feats.supports_braille],
  ].filter(([, ok]) => ok).map(([label]) => label);
  el.lbl_speech_caps.textContent = can.length
    ? "This backend supports: " + can.join(", ")
    : "This backend manages its own voice, rate, and volume.";
}

/**
 * on_refresh_voices. `explicit` is the Refresh Voices button: it also re-detects
 * backends started since the panel opened. The voice selected afterwards is the saved
 * one (cur_cfg), not the one on screen, exactly as in wx.
 */
async function refreshVoices(explicit) {
  const ticket = ++voiceRequest;
  if (explicit) {
    const current = selectedBackend();
    await populateBackends();
    selectBackend(current);
  }
  const result = await invoke("speech_list_voices", { backend: selectedBackend() });
  if (ticket !== voiceRequest) return;
  const names = result.voices || [];
  voices = names.slice();
  if (names.length) {
    fillSelect(el.choice_voice, names.map((nm) => ({ value: nm, label: nm })));
    const want = (curVoiceName || "").trim().toLowerCase();
    let selIdx = 0;
    if (want) {
      const i = names.findIndex((nm) => nm.trim().toLowerCase() === want);
      if (i >= 0) selIdx = i;
    }
    el.choice_voice.selectedIndex = selIdx;
  } else {
    fillSelect(el.choice_voice, [{ value: "", label: "Backend chooses its own voice" }]);
    el.choice_voice.selectedIndex = 0;
  }
  applyBackendCapabilities(result.features);
}

/** on_change_backend */
async function onChangeBackend() {
  await refreshVoices(false);
  scheduleSave();
}

// ---------------------------------------------------------------------------
// Audio device
// ---------------------------------------------------------------------------

/**
 * Fill the device list and select `want`. Departure (python-bugs.md 23): wx saved the
 * selected label, so with no devices the placeholder "(no WASAPI devices found)" became
 * the preferred device, and a saved device that was merely unplugged was replaced by the
 * first one listed on the next save. A saved name that is not present now stays in the
 * list, marked, and selected; the placeholder's value is "".
 */
function fillDevices(want) {
  const items = deviceNames.map((nm) => ({ value: nm, label: nm }));
  if (want && !deviceNames.includes(want)) {
    items.push({ value: want, label: `${want} (not connected)` });
  }
  if (!items.length) items.push({ value: "", label: "(no WASAPI devices found)" });
  fillSelect(el.choice_device, items);
  const idx = indexOfValue(el.choice_device, want);
  el.choice_device.selectedIndex = idx >= 0 ? idx : 0;
}

/** on_refresh_devices */
async function refreshDevices() {
  const prev = selectedValue(el.choice_device);
  deviceNames = await invoke("audio_list_devices");
  fillDevices(prev === undefined ? "" : prev);
}

// ---------------------------------------------------------------------------
// Show/hide handlers (on_toggle_*, on_change_gear_mode)
// ---------------------------------------------------------------------------

function onToggleHighlight() {
  setRow(el.highlight_row, [el.spin_compass_highlight_nth],
    el.chk_compass_highlight.checked, el.chk_compass_highlight);
}

function onToggleHrtf() {
  setRow(el.hrtf_grid,
    [el.choice_hrtf_set, el.spin_hrtf_elevation, el.chk_hrtf_equalise,
      el.spin_hrtf_front_emphasis, el.spin_hrtf_distance_gain],
    el.chk_hrtf_enabled.checked, el.chk_hrtf_enabled);
}

function onToggleAnnounceSpeed() {
  setRow(el.speed_interval_row, [el.choice_speed_interval],
    el.chk_announce_speed.checked, el.chk_announce_speed);
}

/** Show the speech options or the tone options, whichever the mode uses. */
function onChangeGearMode() {
  const mode = selectedValue(el.choice_gear_mode);
  setRow(el.gear_sport_row, [el.chk_gear_sport_once],
    mode === "speech" || mode === "audio", el.choice_gear_mode);
  setRow(el.gear_speech_row, [el.choice_gear_number_style],
    mode === "speech", el.choice_gear_mode);
  setRow(el.gear_audio_grid,
    [el.choice_gear_root, el.choice_gear_octave, el.choice_gear_scale,
      el.spin_gear_tone_level, el.btn_gear_preview],
    mode === "audio", el.choice_gear_mode);
}

function onToggleLaunchBeamng() {
  setRow(el.renderer_row, [el.choice_renderer],
    el.chk_launch_beamng.checked, el.chk_launch_beamng);
}

function onToggleScannerCallout() {
  setRow(el.callout_interval_row, [el.choice_callout_interval],
    el.chk_scanner_callout.checked, el.chk_scanner_callout);
}

/** The fixed-device row shows only while "Follow Default" is unchecked. */
function onToggleFollowDevice() {
  setRow(el.device_row, [el.choice_device],
    !el.chk_follow_device.checked, el.chk_follow_device);
}

function applyAllToggles() {
  onToggleHighlight();
  onToggleHrtf();
  onToggleFollowDevice();
  onToggleLaunchBeamng();
  onToggleAnnounceSpeed();
  onChangeGearMode();
  onToggleScannerCallout();
}

/** _update_speed_interval_labels: labels follow the unit system; the index is kept. */
function updateSpeedIntervalLabels() {
  const unitKey = radioValue(el.rb_units) === "metric" ? "metric" : "imperial";
  const si = choices.speed_intervals;
  const sel = el.choice_speed_interval.selectedIndex;
  fillSelect(el.choice_speed_interval,
    si.values.map((v, i) => ({ value: v, label: si.labels[unitKey][i] })));
  el.choice_speed_interval.selectedIndex = Math.max(0, sel >= 0 ? sel : 0);
}

// ---------------------------------------------------------------------------
// Auto-save
// ---------------------------------------------------------------------------

/** _schedule_save: restart the two-second countdown on any UI change. */
function scheduleSave() {
  if (loading) return;
  if (saveTimer !== null) clearTimeout(saveTimer);
  saveTimer = setTimeout(() => {
    saveTimer = null;
    autoSave();
  }, AUTO_SAVE_DELAY_MS);
}

/** _auto_save. Failures are silent, as in wx. */
function autoSave() {
  const values = controlsToValues();
  return enqueue(async () => {
    try {
      const r = await invoke("config_save_panel", values);
      // cur_cfg is replaced only by a write that succeeded.
      if (r && r.ok) curVoiceName = values.speech_voice_name;
    } catch (err) {
      console.error("config_save_panel failed", err);
    }
  });
}

/** flush_pending_save: write now if the debounce is still counting down. */
export async function flushPendingSave() {
  if (saveTimer !== null) {
    clearTimeout(saveTimer);
    saveTimer = null;
    await autoSave();
  }
}

// ---------------------------------------------------------------------------
// Config <-> controls
// ---------------------------------------------------------------------------

/** load_into_controls. `loaded` is config_load()'s or config_reset()'s answer. */
function loadIntoControls(loaded) {
  const v = loaded.values;
  loading = true;
  try {
    curVoiceName = v.speech_voice_name || "";
    selectBackend(v.speech_backend);
    setRadio(el.rb_units, v.units);
    setRadio(el.rb_proto, v.telemetry_protocol);
    for (const [key, id] of SPINS) setSpin(el[id], v[key]);
    for (const [key, id] of CHECKS) el[id].checked = !!v[key];
    for (const [key, id] of SELECTS) selectValue(el[id], v[key]);
    updateSpeedIntervalLabels();
    selectValue(el.choice_speed_interval, v.speed_announce_interval);
    // Select the saved fixed device, kept in the list even when it is not connected.
    fillDevices(String(v.preferred_device_name || "").trim());
  } finally {
    loading = false;
  }
}

/** controls_to_config, for this panel's keys only (the backend merges the rest). */
function controlsToValues() {
  const out = {};
  out.speech_backend = selectedBackend();
  out.units = radioValue(el.rb_units);
  out.telemetry_protocol = radioValue(el.rb_proto);
  for (const [key, id] of SPINS) out[key] = getSpin(el[id]);
  for (const [key, id] of CHECKS) out[key] = el[id].checked;
  for (const [key, id, , fallback] of SELECTS) {
    const items = itemsOf(el[id]);
    const sel = el[id].selectedIndex;
    if (sel >= 0 && sel < items.length) out[key] = items[sel].value;
    else if (fallback === "first-or-normal") out[key] = "normal";
    else if (fallback === "first") out[key] = items.length ? items[0].value : undefined;
    else out[key] = fallback;
  }
  {
    const items = itemsOf(el.choice_speed_interval);
    const sel = el.choice_speed_interval.selectedIndex;
    out.speed_announce_interval = sel >= 0 && sel < items.length ? items[sel].value : 25;
  }
  out.preferred_device_name = selectedValue(el.choice_device) ?? "";
  out.speech_voice_name =
    el.choice_voice.selectedIndex >= 0 && voices.length ? selectedLabel(el.choice_voice) : "";
  return out;
}

// ---------------------------------------------------------------------------
// Buttons
// ---------------------------------------------------------------------------

async function onTestVoice() {
  const cfg = controlsToValues();
  const r = await invoke("speech_test", {
    backend: cfg.speech_backend,
    voice: cfg.speech_voice_name,
    rate: cfg.speech_rate,
    volume: cfg.speech_volume,
  });
  if (!r.ok && r.error) await messageBox("Speech Test", r.error, "error");
}

async function onTestTone() {
  const r = await invoke("audio_test_tone", {
    freq_hz: getSpin(el.spin_freq),
    level_dbfs: getSpin(el.spin_level),
  });
  if (!r.ok && r.error) await messageBox("Audio Error", r.error, "error");
}

async function onPreviewGearTones() {
  const cfg = controlsToValues();
  const r = await invoke("audio_preview_gear_tones", {
    root: cfg.gear_tone_root,
    octave: cfg.gear_tone_octave,
    scale: cfg.gear_tone_scale,
    level_dbfs: cfg.gear_tone_level_dbfs,
  });
  if (!r.ok && r.error) await messageBox("Audio Error", r.error, "error");
}

/** on_reset. Asks first: the wipe reaches disk on its own two seconds later. */
async function onReset() {
  const ans = await messageBox(
    "Reset to Defaults",
    "Reset every setting on this tab to its default value?",
    "question", "yes_no", "no",
  );
  if (ans !== "yes") return;
  try {
    loadIntoControls(await invoke("config_reset"));
    await refreshVoices(false);
    await refreshDevices();
    applyAllToggles();
    scheduleSave();
  } catch (err) {
    await messageBox("Error", `Failed to reset:\n${err}`, "error");
    return;
  }
  await messageBox(
    "Reset to Defaults",
    "All settings on this tab have been reset to their defaults.",
    "info",
  );
}

// ---------------------------------------------------------------------------
// Setup (ConfigPanel.__init__)
// ---------------------------------------------------------------------------

const IDS = [
  "choice_backend", "choice_voice", "spin_rate", "spin_volume", "lbl_speech_caps",
  "btn_refresh_voices", "btn_test_voice", "rb_units", "rb_proto", "chk_launch_beamng",
  "renderer_row", "choice_renderer", "chk_update_check", "chk_announce_turn_signals",
  "chk_announce_speed", "speed_interval_row", "choice_speed_interval", "chk_drift_speech",
  "choice_gear_mode", "gear_sport_row", "chk_gear_sport_once", "gear_speech_row",
  "choice_gear_number_style", "gear_audio_grid", "choice_gear_root", "choice_gear_octave",
  "choice_gear_scale", "spin_gear_tone_level", "btn_gear_preview",
  "chk_announce_clickspot_actions", "chk_scanner_callout", "callout_interval_row",
  "choice_callout_interval", "chk_ui_nav_hold", "chk_road_follow",
  "chk_road_junction_speech", "chk_road_stop_signs", "chk_road_traffic_lights",
  "chk_road_speed_limits", "chk_road_surface", "chk_road_junction_proximity",
  "chk_road_junction_earcon", "chk_road_private", "spin_road_beep",
  "spin_road_correction", "spin_road_junction", "spin_compass_interval",
  "chk_compass_highlight", "highlight_row", "spin_compass_highlight_nth",
  "spin_compass_click_level", "spin_lowspeed_click_level", "spin_lowspeed_stop_level",
  "spin_slip_tone_level", "spin_placement_ping_level", "chk_hrtf_enabled", "hrtf_grid",
  "choice_hrtf_set", "spin_hrtf_elevation", "chk_hrtf_equalise",
  "spin_hrtf_front_emphasis", "spin_hrtf_distance_gain", "chk_scanner_steer_tone",
  "spin_scanner_base_freq", "spin_scanner_offset", "chk_pitch_roll_enabled",
  "spin_pitch_roll_max", "spin_pitch_roll_min", "spin_freq", "spin_level",
  "btn_test_tone", "chk_oil_chime_enabled", "chk_tc_clicks_enabled", "spin_buzzer_level",
  "spin_chime_level", "spin_obstacle_level", "choice_obstacle_sensitivity",
  "chk_impl_tones", "chk_impl_proximity", "chk_dock_tones", "spin_impl_ground",
  "spin_impl_tilt", "spin_dock", "chk_scan_tones", "spin_scan", "chk_cannon_shot",
  "chk_learn_desc", "chk_follow_device", "device_row", "choice_device",
  "btn_refresh_device", "spin_poll", "chk_mcp_server", "spin_mcp_port", "btn_reset",
];

/** Wire the Configuration panel and fill it from the config file. */
export async function initConfigPanel(uiChoices) {
  choices = uiChoices;
  for (const id of IDS) el[id] = $(id);

  // Choices fixed at construction in wx.
  buildRadios(el.rb_units, "units", choices.units);
  buildRadios(el.rb_proto, "telemetry_protocol", choices.telemetry_protocols);
  for (const [, id, choicesKey] of SELECTS) fillSelect(el[id], choices[choicesKey]);
  el.spin_hrtf_elevation.min = String(choices.hrtf_elevation.min);
  el.spin_hrtf_elevation.max = String(choices.hrtf_elevation.max);
  // wx: btn_test_tone.Enable(AUDIO_TEST_OK)
  el.btn_test_tone.disabled = !choices.audio_test_ok;

  // Show/hide handlers, then auto-save on every data control.
  el.chk_compass_highlight.addEventListener("change", onToggleHighlight);
  el.chk_hrtf_enabled.addEventListener("change", onToggleHrtf);
  el.chk_follow_device.addEventListener("change", onToggleFollowDevice);
  el.chk_announce_speed.addEventListener("change", onToggleAnnounceSpeed);
  el.chk_launch_beamng.addEventListener("change", onToggleLaunchBeamng);
  el.chk_scanner_callout.addEventListener("change", onToggleScannerCallout);
  el.choice_gear_mode.addEventListener("change", onChangeGearMode);
  for (const [, id] of CHECKS) el[id].addEventListener("change", scheduleSave);
  for (const [, id] of SELECTS) el[id].addEventListener("change", scheduleSave);
  el.choice_speed_interval.addEventListener("change", scheduleSave);
  el.choice_device.addEventListener("change", scheduleSave);
  el.choice_voice.addEventListener("change", scheduleSave);
  el.choice_backend.addEventListener("change", onChangeBackend);
  for (const fs of [el.rb_units, el.rb_proto]) fs.addEventListener("change", scheduleSave);
  el.rb_units.addEventListener("change", updateSpeedIntervalLabels);
  for (const [, id] of SPINS) initSpin(el[id], scheduleSave);

  el.btn_refresh_voices.addEventListener("click", () => refreshVoices(true));
  el.btn_refresh_device.addEventListener("click", () => refreshDevices());
  el.btn_test_voice.addEventListener("click", onTestVoice);
  el.btn_test_tone.addEventListener("click", onTestTone);
  el.btn_gear_preview.addEventListener("click", onPreviewGearTones);
  el.btn_reset.addEventListener("click", onReset);

  // Populate, in ConfigPanel.__init__'s order.
  await refreshDevices(); // fill the device list before load_into_controls
  await populateBackends(); // same, for the speech backend list
  loadIntoControls(await invoke("config_load"));
  onToggleHighlight();
  onToggleHrtf();
  onToggleFollowDevice();
  onToggleAnnounceSpeed();
  onChangeGearMode();
  onToggleLaunchBeamng();
  onToggleScannerCallout();
  await refreshVoices(false);
}
