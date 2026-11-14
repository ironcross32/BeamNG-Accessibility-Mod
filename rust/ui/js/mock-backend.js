// In-memory stand-in for the Rust backend, used only when window.__TAURI__ is absent (the
// page opened from disk or a static server). It answers every command in CONTRACT.md with
// plausible data so the interface can be driven with a screen reader before any Rust
// exists. Nothing here is used in the real application.
//
// URL parameters:
//   ?mode=configurator   open in configurator mode
//   ?update=available    "Check for Updates" offers a fake update with a cancellable download
//
// From the browser console: __beamMock.emit("close-requested", {}) simulates closing the
// window; __beamMock.config shows what the last save wrote.

// configurator.py DEFAULT_CONFIG
const DEFAULT_CONFIG = {
  speech_backend: "auto", speech_voice_name: "", speech_rate: 50, speech_volume: 100,
  units: "imperial", shift_tone_frequency_hz: 880.0, shift_tone_level_dbfs: -12.0,
  check_engine_buzzer_level_dbfs: -12.0, oil_chime_level_dbfs: -12.0,
  oil_chime_enabled: true, tc_clicks_enabled: true, pitch_roll_tones_enabled: true,
  pitch_roll_max_dbfs: -24.0, pitch_roll_min_dbfs: -36.0, compass_click_level_dbfs: -6.0,
  lowspeed_click_level_dbfs: -14.0, lowspeed_stop_tone_level_dbfs: -16.0,
  slip_tone_level_dbfs: -18.0, placement_ping_volume_db: -12.0,
  obstacle_buzz_volume_db: -18.0, obstacle_warning_sensitivity: "normal",
  road_follow_guidance_enabled: true, road_junction_speech_enabled: true,
  road_stop_sign_speech_enabled: true, road_traffic_light_speech_enabled: true,
  road_speed_limit_speech_enabled: true, road_surface_speech_enabled: true,
  drift_speech_enabled: true, road_junction_earcon_enabled: true,
  road_junction_proximity_enabled: true, road_include_private: false,
  road_beep_volume_db: -14.0, route_beacon_volume_db: -16.0,
  road_correction_volume_db: -24.0, road_junction_volume_db: -14.0,
  telemetry_protocol: "extended", compass_click_interval: 15,
  compass_highlight_enabled: true, compass_highlight_nth_click: 6, hrtf_enabled: true,
  hrtf_set: "mit_kemar", hrtf_elevation_deg: -3.0, hrtf_equalise: true,
  hrtf_front_emphasis_db: -6.0, hrtf_distance_gain_db: 0.0,
  follow_default_audio_device: true, preferred_device_name: "",
  audio_poll_interval_sec: 2.0, launch_beamng: false, beamng_renderer: "d3d",
  update_check_enabled: true, pending_update_version: "", announce_turn_signals: true,
  announce_speed: true, speed_announce_interval: 25, gear_report_mode: "speech",
  gear_speech_sport_once: false, gear_speech_number_style: "ordinal",
  gear_tone_root: "C", gear_tone_octave: 4, gear_tone_scale: "major",
  gear_tone_level_dbfs: -3.0, announce_clickspot_actions: false,
  scanner_distance_callout_enabled: false, scanner_distance_callout_interval: 10,
  scanner_steer_tone_enabled: true, scanner_base_freq_hz: 1000.0,
  scanner_pitch_offset_oct: 1.0, ui_nav_hold_suppression: true,
  implement_tones_enabled: true, implement_ground_tone_dbfs: -19.5,
  implement_tilt_tone_dbfs: -20.0, implement_proximity_speech: true,
  dock_tones_enabled: true, dock_tone_dbfs: -18.0, scan_tones_enabled: true,
  cannon_shot_readout: true, binding_learn_speak_description: false,
  scan_tone_dbfs: -20.0, ai_describer_provider: "gemini", ai_describer_api_key: "",
  ai_describer_model: "models/gemini-3-flash-preview", ai_describer_openai_api_key: "",
  ai_describer_openai_model: "gpt-5.6-terra",
  ai_describer_openai_base_url: "https://api.openai.com/v1",
  ai_describer_openai_reasoning_effort: "low", ai_describer_openai_detail: "auto",
  ai_describer_disable_ui_toggle: false, mcp_server_enabled: false, mcp_server_port: 4481,
};

const PANEL_KEYS = [
  "speech_backend", "units", "telemetry_protocol", "speech_rate", "speech_volume",
  "shift_tone_frequency_hz", "shift_tone_level_dbfs", "check_engine_buzzer_level_dbfs",
  "oil_chime_level_dbfs", "obstacle_buzz_volume_db", "obstacle_warning_sensitivity",
  "implement_tones_enabled", "implement_proximity_speech", "implement_ground_tone_dbfs",
  "implement_tilt_tone_dbfs", "dock_tones_enabled", "scan_tones_enabled",
  "cannon_shot_readout", "binding_learn_speak_description", "dock_tone_dbfs",
  "scan_tone_dbfs", "compass_click_interval", "compass_highlight_enabled",
  "compass_highlight_nth_click", "compass_click_level_dbfs", "lowspeed_click_level_dbfs",
  "lowspeed_stop_tone_level_dbfs", "slip_tone_level_dbfs", "placement_ping_volume_db",
  "road_follow_guidance_enabled", "road_stop_sign_speech_enabled",
  "road_traffic_light_speech_enabled", "road_speed_limit_speech_enabled",
  "road_surface_speech_enabled", "road_junction_speech_enabled",
  "road_junction_earcon_enabled", "road_junction_proximity_enabled",
  "road_include_private", "road_beep_volume_db", "road_correction_volume_db",
  "road_junction_volume_db", "hrtf_enabled", "hrtf_set", "hrtf_elevation_deg",
  "hrtf_equalise", "hrtf_front_emphasis_db", "hrtf_distance_gain_db",
  "pitch_roll_tones_enabled", "pitch_roll_max_dbfs", "pitch_roll_min_dbfs",
  "oil_chime_enabled", "tc_clicks_enabled", "launch_beamng", "update_check_enabled",
  "beamng_renderer", "follow_default_audio_device", "audio_poll_interval_sec",
  "announce_turn_signals", "announce_speed", "drift_speech_enabled", "gear_report_mode",
  "gear_speech_sport_once", "gear_speech_number_style", "gear_tone_root",
  "gear_tone_octave", "gear_tone_scale", "gear_tone_level_dbfs",
  "announce_clickspot_actions", "scanner_distance_callout_enabled",
  "scanner_distance_callout_interval", "scanner_steer_tone_enabled",
  "scanner_base_freq_hz", "scanner_pitch_offset_oct", "ui_nav_hold_suppression",
  "mcp_server_enabled", "mcp_server_port", "speed_announce_interval",
  "preferred_device_name", "speech_voice_name",
];

const vl = (pairs) => pairs.map(([value, label]) => ({ value, label }));

const UI_CHOICES = {
  audio_test_ok: true,
  speech_auto_label: "Auto (best available)",
  units: vl([["imperial", "Imperial (mph, °F, psi)"], ["metric", "Metric (km/h, °C, bar)"]]),
  telemetry_protocols: vl([["extended", "Extended"], ["outgauge", "OutGauge"]]),
  renderers: vl([["d3d", "Direct3D 11"], ["vulkan", "Vulkan"]]),
  speed_intervals: {
    values: [25, 50, 75, 100],
    labels: {
      imperial: [25, 50, 75, 100].map((v) => `Every ${v} mph`),
      metric: [25, 50, 75, 100].map((v) => `Every ${v} km/h`),
    },
  },
  callout_intervals: [5, 10, 15, 20, 30, 45, 60].map((v) => ({ value: v, label: `${v} seconds` })),
  gear_modes: vl([["off", "Off"], ["speech", "As speech"], ["audio", "As audio"]]),
  gear_number_styles: vl([["ordinal", "Ordinals (first, second)"], ["number", "Numbers (1, 2)"]]),
  gear_roots: vl([
    ["C", "C"], ["C#", "C sharp / D flat"], ["D", "D"], ["D#", "D sharp / E flat"],
    ["E", "E"], ["F", "F"], ["F#", "F sharp / G flat"], ["G", "G"],
    ["G#", "G sharp / A flat"], ["A", "A"], ["A#", "A sharp / B flat"], ["B", "B"],
  ]),
  gear_octaves: [2, 3, 4, 5, 6].map((o) => ({ value: o, label: String(o) })),
  gear_scales: vl([
    ["major", "Major"], ["natural_minor", "Natural minor"],
    ["harmonic_minor", "Harmonic minor"], ["melodic_minor", "Melodic minor"],
    ["dorian", "Dorian"], ["phrygian", "Phrygian"], ["lydian", "Lydian"],
    ["mixolydian", "Mixolydian"], ["major_pentatonic", "Major pentatonic"],
    ["minor_pentatonic", "Minor pentatonic"], ["blues", "Blues"],
    ["whole_tone", "Whole tone"], ["chromatic", "Chromatic"],
    ["quarter_tone", "Quarter-tone chromatic"], ["rast", "Maqam Rast (quarter tones)"],
    ["bayati", "Maqam Bayati (quarter tones)"],
  ]),
  hrtf_sets: vl([
    ["mit_kemar", "MIT KEMAR"], ["thk_ku100", "Neumann KU100 (TH Köln)"],
    ["sadie_kemar", "KEMAR (SADIE II, University of York)"],
  ]),
  hrtf_elevation: { min: -20.0, max: 10.0 },
  obstacle_sensitivities: vl([["early", "Early"], ["normal", "Normal"], ["late", "Late"]]),
  console_contexts: [
    { index: 0, label: "GE - Lua" }, { index: 1, label: "GE - TorqueScript" },
    { index: 2, label: "CEF/UI - JS" },
  ],
  ai_default_provider: "gemini",
  ai_providers: [
    {
      id: "gemini", display: "Google Gemini",
      key_help: "Paste your Google Gemini API key:",
      key_cfg: "ai_describer_api_key", model_cfg: "ai_describer_model",
      default_model: "models/gemini-3-flash-preview",
      models: vl([
        ["models/gemini-3-flash-preview", "Gemini 3 Flash Preview"],
        ["models/gemini-3-pro-preview", "Gemini 3 Pro Preview"],
        ["models/gemini-2.5-flash", "Gemini 2.5 Flash"],
        ["models/gemini-2.5-pro", "Gemini 2.5 Pro"],
      ]),
      extras: [],
    },
    {
      id: "openai", display: "OpenAI",
      key_help: "Paste your OpenAI API key:",
      key_cfg: "ai_describer_openai_api_key", model_cfg: "ai_describer_openai_model",
      default_model: "gpt-5.6-terra",
      models: vl([
        ["gpt-5.6-terra", "GPT-5.6 Terra (balanced)"],
        ["gpt-5.6-sol", "GPT-5.6 Sol (frontier)"],
        ["gpt-5.6-luna", "GPT-5.6 Luna (fast, low cost)"],
        ["gpt-4o", "GPT-4o"],
      ]),
      extras: [
        {
          key: "ai_describer_openai_base_url", arg: "base_url", kind: "text",
          label: "Base URL:", default: "https://api.openai.com/v1",
          help: "The API endpoint. Change this only to reach a proxy or an OpenAI-compatible server.",
        },
        {
          key: "ai_describer_openai_reasoning_effort", arg: "reasoning_effort", kind: "choice",
          label: "Reasoning effort:", default: "low",
          values: vl([["none", "None (fastest)"], ["minimal", "Minimal"], ["low", "Low"],
            ["medium", "Medium"], ["high", "High"]]),
          help: "How long the model thinks before answering. Higher settings are slower and cost more. Ignored by models without reasoning.",
        },
        {
          key: "ai_describer_openai_detail", arg: "detail", kind: "choice",
          label: "Image detail:", default: "auto",
          values: vl([["auto", "Auto (full resolution)"], ["high", "High"], ["low", "Low"]]),
          help: "How much image resolution is sent. Low costs less but loses on-screen text.",
        },
      ],
    },
  ],
};

// Fake speech backends: a screen reader, SAPI, and one that is not installed.
const SPEECH = {
  NVDA: { voices: [], features: { supports_set_voice: false, supports_set_rate: false, supports_set_volume: false, supports_braille: true } },
  SAPI: { voices: ["Microsoft Zira Desktop", "Microsoft David Desktop", "microsoft hazel"], features: { supports_set_voice: true, supports_set_rate: true, supports_set_volume: true, supports_braille: false } },
  OneCore: { voices: ["Microsoft Mark", "Microsoft Aria"], features: { supports_set_voice: true, supports_set_rate: true, supports_set_volume: true, supports_braille: false } },
  JAWS: null,
};

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const now = () => {
  const d = new Date();
  const p = (n) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
};

export function createMockBackend() {
  const params = new URLSearchParams(location.search);
  const mode = params.get("mode") === "configurator" ? "configurator" : "main";
  const listeners = new Map();
  let config = structuredClone(DEFAULT_CONFIG);
  let panelSnapshot = structuredClone(config);
  let describerSnapshot = structuredClone(config);
  let history = ["print(1+1)", "dump(be:getPlayerVehicle(0))"];
  let logStreamTimer = null;
  let updateCancel = false;
  let attached = false;
  const pendingLog = [
    `${now()} [INFO] Mock backend: no Tauri runtime, using in-memory data.`,
    `${now()} [INFO] Console listener started on port 4466`,
    `${now()} [WARNING] Mock: telemetry socket idle`,
  ];

  function emit(event, payload) {
    for (const h of listeners.get(event) || []) {
      try {
        h(payload);
      } catch (err) {
        console.error(err);
      }
    }
  }

  function log(level, msg) {
    const line = `${now()} [${level}] ${msg}`;
    if (attached) emit("log-line", { text: line });
    else pendingLog.push(line);
  }

  function panelValues(cfg) {
    const values = {};
    for (const k of PANEL_KEYS) values[k] = cfg[k];
    values.scanner_base_freq_hz = Math.round(cfg.scanner_base_freq_hz);
    return { values };
  }

  function describerState(cfg) {
    const providers = {};
    for (const p of UI_CHOICES.ai_providers) {
      const extras = {};
      for (const ex of p.extras) extras[ex.key] = cfg[ex.key];
      providers[p.id] = {
        has_key: !!String(cfg[p.key_cfg] || "").trim(),
        model: cfg[p.model_cfg],
        extras,
      };
    }
    return {
      provider: cfg.ai_describer_provider,
      disable_ui_toggle: !!cfg.ai_describer_disable_ui_toggle,
      providers,
    };
  }

  function aiKeys() {
    const keys = ["ai_describer_provider", "ai_describer_disable_ui_toggle"];
    for (const p of UI_CHOICES.ai_providers) {
      keys.push(p.key_cfg, p.model_cfg, ...p.extras.map((ex) => ex.key));
    }
    return keys;
  }

  async function messageBox({ title, text, buttons }) {
    await sleep(0);
    if (buttons === "yes_no") return window.confirm(`${title}\n\n${text}`) ? "yes" : "no";
    window.alert(`${title}\n\n${text}`);
    return "ok";
  }

  async function checkUpdates() {
    emit("update-progress", { title: "Checking for updates", text: "Contacting GitHub...", fraction: null, cancellable: false });
    await sleep(1200);
    emit("update-progress-close", {});
    if (params.get("update") !== "available") {
      await messageBox({ title: "No Update Available", text: "You are running the latest version (0.2.1).", buttons: "ok" });
      return;
    }
    const ans = await messageBox({ title: "Update Available", text: "You have version 0.2.1. Version 0.3.0 is available.\n\nInstall it now?", buttons: "yes_no" });
    if (ans !== "yes") return;
    updateCancel = false;
    const total = 24 * 1048576;
    const message = "Downloading BeamNG_accessibility_mod.zip...";
    for (let done = 0; done <= total; done += 1048576) {
      if (updateCancel) {
        emit("update-progress-close", {});
        log("INFO", "User cancelled the download of v0.3.0");
        return;
      }
      emit("update-progress", {
        title: "Downloading update",
        text: `${message}\n${(done / 1048576).toFixed(1)} of ${(total / 1048576).toFixed(1)} MB`,
        fraction: done / total,
        cancellable: true,
      });
      await sleep(250);
    }
    emit("update-progress-close", {});
    await messageBox({ title: "Restarting to Update", text: "BEAM will now close and restart on the new version.", buttons: "ok" });
  }

  const commands = {
    app_info: () => ({
      version: "0.2.1",
      title: mode === "configurator" ? "BeamTel Configurator" : "BeamNG Accessibility 0.2.1",
      mode,
    }),
    ui_choices: () => structuredClone(UI_CHOICES),

    config_load: () => {
      panelSnapshot = structuredClone(config);
      return panelValues(panelSnapshot);
    },
    config_save_panel: (v) => {
      const cfg = { ...structuredClone(panelSnapshot), ...v };
      cfg.scanner_base_freq_hz = Number(cfg.scanner_base_freq_hz);
      cfg.scanner_pitch_offset_oct = Math.round(cfg.scanner_pitch_offset_oct * 12) / 12;
      for (const k of aiKeys()) if (k in config) cfg[k] = config[k];
      cfg.pending_update_version = config.pending_update_version;
      config = cfg;
      panelSnapshot = structuredClone(cfg);
      console.info("[mock] config_save_panel wrote", cfg);
      return { ok: true };
    },
    config_reset: () => {
      panelSnapshot = structuredClone(DEFAULT_CONFIG);
      return panelValues(panelSnapshot);
    },

    describer_load: () => {
      describerSnapshot = structuredClone(config);
      return describerState(describerSnapshot);
    },
    describer_save: (a) => {
      const p = UI_CHOICES.ai_providers.find((x) => x.id === a.provider);
      if (a.key_action === "clear") describerSnapshot[p.key_cfg] = "";
      if (a.key_action === "set") describerSnapshot[p.key_cfg] = `sealed:${a.key.length}`;
      const cfg = structuredClone(config);
      cfg.ai_describer_provider = a.provider;
      cfg[p.model_cfg] = a.model;
      cfg[p.key_cfg] = describerSnapshot[p.key_cfg] || "";
      for (const ex of p.extras) cfg[ex.key] = a.extras[ex.key];
      cfg.ai_describer_disable_ui_toggle = a.disable_ui_toggle;
      config = cfg;
      describerSnapshot = structuredClone(cfg);
      console.info("[mock] describer_save wrote", cfg);
      return { ok: true, state: describerState(describerSnapshot) };
    },
    describer_validate_key: async (a) => {
      await sleep(1500);
      if (a.key.toLowerCase().includes("bad")) {
        return { ok: false, error: "API key not valid. Please pass a valid API key." };
      }
      return { ok: true, error: null };
    },

    speech_list_backends: () => Object.keys(SPEECH),
    speech_list_voices: ({ backend }) => {
      const name = backend === "auto" ? "NVDA" : backend;
      const b = SPEECH[name];
      if (!b) return { voices: [], features: null };
      return {
        voices: b.voices.slice().sort((x, y) => x.toLowerCase().localeCompare(y.toLowerCase())),
        features: b.features,
      };
    },
    speech_test: ({ backend }) => {
      const name = backend === "auto" ? "NVDA" : backend;
      if (!SPEECH[name]) return { ok: false, error: "Speech test failed: backend not available" };
      log("INFO", `[mock] Speech test on ${name}`);
      return { ok: true, error: null };
    },

    audio_list_devices: () => ["Headphones (USB Audio)", "Speakers (Realtek(R) Audio)"],
    audio_test_tone: ({ freq_hz, level_dbfs }) => {
      log("INFO", `[mock] Test tone ${freq_hz} Hz at ${level_dbfs} dBFS`);
      return { ok: true, error: null };
    },
    audio_preview_gear_tones: (a) => {
      log("INFO", `[mock] Gear tone preview ${a.root}${a.octave} ${a.scale} ${a.level_dbfs} dBFS`);
      return { ok: true, error: null };
    },

    logs_menu_popup: ({ x, y, scale }) => {
      log("INFO", `[mock] The native View Logs menu would open at (${x}, ${y}) x${scale}`);
      return null;
    },
    install_mod: async () => {
      await messageBox({ title: "Installation Complete", text: "The mod installation is complete.\n\nMod file: the installed file is already identical.\nFreeroam layout: layout file was not found; no cleanup was needed.\n\nC:\\mock\\mods\\bng_screenreader_mod.zip", buttons: "ok" });
      return true;
    },
    check_updates: checkUpdates,
    update_cancel: () => {
      updateCancel = true;
      return null;
    },
    exit_app: () => {
      setTimeout(() => emit("close-requested", {}), 0);
      return null;
    },

    console_history: () => history.slice(),
    console_run: ({ index, command }) => {
      if (!history.length || history[history.length - 1] !== command) {
        history.push(command);
        history = history.slice(-50);
      }
      setTimeout(() => {
        emit("console-line", { text: `  = mock result of context ${index}: ${command}` });
      }, 120);
      return { history: history.slice() };
    },
    console_refresh_contexts: () => {
      setTimeout(() => emit("console-contexts", {
        items: [
          { index: 0, label: "GE - Lua" }, { index: 1, label: "GE - TorqueScript" },
          { index: 2, label: "CEF/UI - JS" }, { index: 3, label: "Vehicle 1 - Lua (pickup)" },
        ],
      }), 150);
      return null;
    },
    console_set_log_stream: ({ enabled }) => {
      clearInterval(logStreamTimer);
      logStreamTimer = null;
      if (enabled) {
        let n = 0;
        logStreamTimer = setInterval(() => {
          n += 1;
          emit("console-line", { text: `  [I GE] mock game log line ${n}` });
        }, 3000);
      }
      return null;
    },

    message_box: messageBox,
    frontend_ready: () => {
      attached = true;
      const ready = {
        log_lines: pendingLog.splice(0),
        console_lines: [],
        console_contexts: null,
      };
      return ready;
    },
    close_ready: () => {
      console.info("[mock] close_ready: the window would close now.");
      window.alert("Mock: saves flushed; the window would close now.");
      return null;
    },
  };

  const api = {
    isMock: true,
    async invoke(command, args) {
      const fn = commands[command];
      if (!fn) throw new Error(`mock backend: unknown command ${command}`);
      await sleep(10);
      return fn(args === undefined ? {} : structuredClone(args));
    },
    async listen(event, handler) {
      if (!listeners.has(event)) listeners.set(event, new Set());
      listeners.get(event).add(handler);
      return () => listeners.get(event).delete(handler);
    },
  };

  window.__beamMock = {
    emit,
    get config() {
      return config;
    },
  };
  return api;
}
