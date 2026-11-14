# Frontend to backend contract

This is the complete list of Tauri commands the frontend (`rust/ui/`) calls and events it
listens for. The backend (`rust/src-tauri/`) implements exactly this; nothing else is
called. `js/mock-backend.js` is a working fake of every entry here and is the quickest way
to see the expected shapes.

Python references are to the reference implementation in the repository root
(`config_ui.py`, `beamtel.py`, `configurator.py`, `updater.py`, `ai_describer.py`,
`gear_report.py`, `hrtf_sets.py`, `speech.py`).

## Conventions

- Tauri 2 with `app.withGlobalTauri: true`. The frontend calls
  `window.__TAURI__.core.invoke(name)` for a command with no arguments, and
  `window.__TAURI__.core.invoke(name, { args: {...} })` for one with arguments.
- **Every command that takes arguments has exactly one parameter, named `args`**, whose
  type is a `#[derive(Deserialize)]` struct with the snake_case field names written below.
  This sidesteps Tauri's camelCase renaming of parameter names: field names inside `args`
  are deserialised by serde as written.
- Return values are serialised with serde; `null` means `()` / `Option::None`.
- "Never rejects" means the command must return `Ok`; failures are reported inside the
  value as shown. Where a command may reject, the frontend's handling of the rejection is
  stated. A rejection's error value is shown with `String(err)`, so use `Err(String)`.
- Events are emitted to the main window with `emit` and a JSON payload. The frontend
  reads `event.payload` only.
- Commands that wait on the user or the network (`message_box`, `install_mod`,
  `check_updates`, `describer_validate_key`, `logs_menu_popup`) must be `async` commands
  that do not block the main thread, so the web view keeps processing events (the update
  progress dialog is driven by events while `check_updates` is pending).
- The capability file must allow `core:event:default` (listen) for the main window.

## Startup and shutdown

### Startup order (frontend)

1. `app_info` -> sets `document.title` to `title`; chooses the mode.
2. `ui_choices`.
3. `listen("close-requested")`, `listen("update-progress")`, `listen("update-progress-close")`.
4. Main mode only: `console_history`, then `listen("log-line")`, `listen("console-line")`,
   `listen("console-contexts")`.
5. Configuration panel: `audio_list_devices`, `speech_list_backends`, `config_load`,
   `speech_list_voices` (ConfigPanel.__init__ order).
6. Main mode only: AI Describer panel: `describer_load`.
7. Initial focus is placed (main mode: the Main tab; configurator mode: the first control,
   the Speech Backend select).
8. `frontend_ready`.

The backend must not emit `log-line`, `console-line` or `console-contexts` before
`frontend_ready` has been called. It buffers them and returns them from `frontend_ready`.
After `frontend_ready` returns, it emits them as events. Main mode only: the **startup
update flow** (`updater.run_startup_flow(frame, gate)`, which releases the launch gate
exactly once) starts after `frontend_ready` has returned. That stands in for
`wx.CallAfter(_run_update_flow, frame)`, which ran once the window was up.

### Close

The window close button, Alt+F4, the **Exit** button (`exit_app`) and the updater's
`frame.Close(True)` all take the same path (`BeamTelFrame._on_close`,
`configurator.main._on_close`):

1. The backend prevents the window from closing, stops emitting `log-line` and
   `console-line` / `console-contexts` (`_shutting_down`, `console_frame = None`, log
   handler removed), and emits `close-requested`.
2. The frontend flushes the pending debounced saves (Configuration panel, then AI
   Describer panel; in configurator mode only the Configuration panel). Each flush is a
   `config_save_panel` / `describer_save` call that the frontend awaits.
3. The frontend calls `close_ready`.
4. The backend continues `_on_close`: `STOP.set()`, joins the engine thread with a 2.0 s
   timeout, destroys the window, and in main mode sends `LEARN_OFF` if learn mode is
   active, as `beamtel.main` does after `MainLoop` returns.

`close-requested` is emitted at most once per close attempt. The frontend ignores a second
one. *Recommendation, not in the Python:* if `close_ready` has not arrived within a few
seconds (the page has crashed), close anyway.

## Commands

### `app_info`

No arguments. Never rejects.

```json
{ "version": "0.2.1", "title": "BeamNG Accessibility 0.2.1", "mode": "main" }
```

- `version`: `APP_VERSION`.
- `title`: main mode `"BeamNG Accessibility " + APP_VERSION` (fallback `"BeamNG Accessibility"`
  if the version cannot be read); configurator mode `"BeamTel Configurator"`. The backend
  also sets the native window title to this; the frontend copies it to `document.title`.
- `mode`: `"main"`, or `"configurator"` when started with `--configurator`.

### `ui_choices`

No arguments. Never rejects. Every list a panel shows comes from here, so no table is
duplicated in JavaScript. Items are `{ "value": <config value>, "label": <text shown> }`
in display order. `value` types are exactly the config value types (`gear_octaves` values
and interval values are integers).

| Field | Shape | Source |
|---|---|---|
| `audio_test_ok` | bool | `configurator.AUDIO_TEST_OK`: whether tone playback is available. When false, **Test Shift Tone** is disabled |
| `speech_auto_label` | string `"Auto (best available)"` | `ConfigPanel._AUTO_BACKEND_LABEL` |
| `units` | `[{imperial, "Imperial (mph, °F, psi)"}, {metric, "Metric (km/h, °C, bar)"}]` | `config_ui.py` `rb_units` choices; values from `controls_to_config` |
| `telemetry_protocols` | `[{extended, "Extended"}, {outgauge, "OutGauge"}]` | `rb_proto` |
| `renderers` | `[{d3d, "Direct3D 11"}, {vulkan, "Vulkan"}]` | `choice_renderer` |
| `speed_intervals` | `{ "values": [25, 50, 75, 100], "labels": { "imperial": ["Every 25 mph", ...], "metric": ["Every 25 km/h", ...] } }` | `_update_speed_interval_labels`: `f"Every {v} {unit}"`, unit `mph` / `km/h` |
| `callout_intervals` | `[{5, "5 seconds"}, {10, "10 seconds"}, 15, 20, 30, 45, 60]` | `self._callout_intervals`, `f"{v} seconds"` |
| `gear_modes` | `[{off, "Off"}, {speech, "As speech"}, {audio, "As audio"}]` | `gear_report.MODES` zipped with the `choice_gear_mode` labels |
| `gear_number_styles` | `[{ordinal, "Ordinals (first, second)"}, {number, "Numbers (1, 2)"}]` | `gear_report.NUMBER_STYLES` zipped with `choice_gear_number_style` labels |
| `gear_roots` | `gear_report.ROOT_NOTES` as `{value, label}` | |
| `gear_octaves` | `gear_report.ROOT_OCTAVES` as `{value: int, label: str(int)}` | |
| `gear_scales` | `gear_report.SCALES` as `{value: key, label}` | |
| `hrtf_sets` | `hrtf_sets.available_sets()` as `{value: set_id, label}` | |
| `hrtf_elevation` | `{ "min": ELEVATION_MIN_DEG, "max": ELEVATION_MAX_DEG }` (-20.0, 10.0) | `hrtf_sets` |
| `obstacle_sensitivities` | `[{early, "Early"}, {normal, "Normal"}, {late, "Late"}]` | `choice_obstacle_sensitivity` |
| `console_contexts` | `[{ "index": 0, "label": "GE - Lua" }, { 1, "GE - TorqueScript" }, { 2, "CEF/UI - JS" }]` | `_build_console_section` initial choices and `_console_ctx_indices` |
| `ai_default_provider` | `"gemini"` | `ai_describer.DEFAULT_PROVIDER` |
| `ai_providers` | list, below | `ai_describer.PROVIDERS` and `_PROVIDER_INFO` |

Each `ai_providers` entry, in `PROVIDERS` order:

```json
{
  "id": "openai",
  "display": "OpenAI",
  "key_help": "Paste your OpenAI API key:",
  "key_cfg": "ai_describer_openai_api_key",
  "model_cfg": "ai_describer_openai_model",
  "default_model": "gpt-5.6-terra",
  "models": [{ "value": "gpt-5.6-terra", "label": "GPT-5.6 Terra (balanced)" }],
  "extras": [
    { "key": "ai_describer_openai_base_url", "arg": "base_url", "kind": "text",
      "label": "Base URL:", "default": "https://api.openai.com/v1", "help": "..." },
    { "key": "ai_describer_openai_reasoning_effort", "arg": "reasoning_effort",
      "kind": "choice", "label": "Reasoning effort:", "default": "low", "help": "...",
      "values": [{ "value": "none", "label": "None (fastest)" }] }
  ]
}
```

`models` is `vision_models_for(id)` as `{value: name, label: display}`; `extras` is
`extras_for(id)` with `values` (choice extras only) as `{value, label}`. All strings
verbatim from the registry.

### Configuration panel

The Configuration panel owns these config keys (every key `controls_to_config` writes).
Type is the JSON type the backend writes to disk:

| Key | Type | Control |
|---|---|---|
| `speech_backend` | string | Speech Backend (`"auto"` or a Prism backend name) |
| `speech_voice_name` | string | Voice (`""` when the backend lists no voices) |
| `speech_rate` | int | Rate |
| `speech_volume` | int | Volume |
| `units` | string `imperial` / `metric` | Display Units |
| `telemetry_protocol` | string `extended` / `outgauge` | Telemetry Protocol |
| `launch_beamng` | bool | Launch BeamNG.drive on startup |
| `beamng_renderer` | string `d3d` / `vulkan` | Graphics API |
| `update_check_enabled` | bool | Check for updates on startup |
| `announce_turn_signals` | bool | |
| `announce_speed` | bool | |
| `speed_announce_interval` | int | Announce every |
| `drift_speech_enabled` | bool | |
| `gear_report_mode` | string | Report gear changes |
| `gear_speech_sport_once` | bool | |
| `gear_speech_number_style` | string | Forward gears |
| `gear_tone_root` | string | Root note |
| `gear_tone_octave` | int | Octave |
| `gear_tone_scale` | string | Scale |
| `gear_tone_level_dbfs` | float | Gear Tone Volume |
| `announce_clickspot_actions` | bool | |
| `scanner_distance_callout_enabled` | bool | |
| `scanner_distance_callout_interval` | int | Callout every |
| `ui_nav_hold_suppression` | bool | |
| `road_follow_guidance_enabled` | bool | |
| `road_junction_speech_enabled` | bool | |
| `road_stop_sign_speech_enabled` | bool | |
| `road_traffic_light_speech_enabled` | bool | |
| `road_speed_limit_speech_enabled` | bool | |
| `road_surface_speech_enabled` | bool | |
| `road_junction_proximity_enabled` | bool | |
| `road_junction_earcon_enabled` | bool | |
| `road_include_private` | bool | |
| `road_beep_volume_db` | float | |
| `road_correction_volume_db` | float | |
| `road_junction_volume_db` | float | |
| `compass_click_interval` | int | |
| `compass_highlight_enabled` | bool | |
| `compass_highlight_nth_click` | int | |
| `compass_click_level_dbfs` | float | |
| `lowspeed_click_level_dbfs` | float | |
| `lowspeed_stop_tone_level_dbfs` | float | |
| `slip_tone_level_dbfs` | float | |
| `placement_ping_volume_db` | float | |
| `hrtf_enabled` | bool | |
| `hrtf_set` | string (set id) | |
| `hrtf_elevation_deg` | float | |
| `hrtf_equalise` | bool | |
| `hrtf_front_emphasis_db` | float | |
| `hrtf_distance_gain_db` | float | |
| `scanner_steer_tone_enabled` | bool | |
| `scanner_base_freq_hz` | float (control is an integer spin) | |
| `scanner_pitch_offset_oct` | float | |
| `pitch_roll_tones_enabled` | bool | |
| `pitch_roll_max_dbfs` | float | |
| `pitch_roll_min_dbfs` | float | |
| `shift_tone_frequency_hz` | float | |
| `shift_tone_level_dbfs` | float | |
| `oil_chime_enabled` | bool | |
| `tc_clicks_enabled` | bool | |
| `check_engine_buzzer_level_dbfs` | float | |
| `oil_chime_level_dbfs` | float | |
| `obstacle_buzz_volume_db` | float | |
| `obstacle_warning_sensitivity` | string `early` / `normal` / `late` | |
| `implement_tones_enabled` | bool | |
| `implement_proximity_speech` | bool | |
| `dock_tones_enabled` | bool | |
| `implement_ground_tone_dbfs` | float | |
| `implement_tilt_tone_dbfs` | float | |
| `dock_tone_dbfs` | float | |
| `scan_tones_enabled` | bool | |
| `scan_tone_dbfs` | float | |
| `cannon_shot_readout` | bool | |
| `binding_learn_speak_description` | bool | |
| `follow_default_audio_device` | bool | |
| `preferred_device_name` | string | Fixed Output Device (the selected entry's text) |
| `audio_poll_interval_sec` | float | |
| `mcp_server_enabled` | bool | |
| `mcp_server_port` | int | |

That is 79 keys. Every other key in the file (`route_beacon_volume_db`,
`pending_update_version`, the AI Describer keys, anything unknown) is never sent by the
panel.

**The panel snapshot (`ConfigPanel.cur_cfg`).** The backend keeps one config dict for the
Configuration panel. `config_load` sets it to `load_config()`. `config_reset` replaces it
with a copy of `DEFAULT_CONFIG`. A successful `config_save_panel` replaces it with what was
written. Keys the panel does not own are written from this snapshot, which is why a reset
returns them to their defaults as well (see `config_reset`).

#### `config_load`

No arguments. Rejects only if the config cannot be produced at all; the frontend then
logs the error and leaves the controls at their HTML defaults.

`load_config()` (merge, coerce, clamp, migrate, seal plaintext secrets, write back as the
Python does), store it as the panel snapshot, and return:

```json
{ "values": { "<each of the 79 keys above>": <value> } }
```

Each value is what `load_into_controls` would put in its control:

- `units`: `"metric"` if `str(v).lower().startswith("m")` else `"imperial"`.
- `telemetry_protocol`: `"outgauge"` if `str(v).lower() == "outgauge"` else `"extended"`.
- `beamng_renderer`: `"vulkan"` if `str(v).lower() == "vulkan"` else `"d3d"`.
- `obstacle_warning_sensitivity`: `str(v).lower()`, or `"normal"` if not one of the three.
- `hrtf_set`: `hrtf_sets.resolve(v).set_id`. `hrtf_elevation_deg`: `clamp_elevation(v)`.
- `hrtf_equalise`: `bool(v)`.
- Gear keys: from `gear_report.settings_from_config(cfg)` (`mode`, `collapse_sport`,
  `number_style`, `root`, `octave`, `scale`, `level_db`).
- `scanner_distance_callout_interval`: `v` if in `[5, 10, 15, 20, 30, 45, 60]` else `10`.
- `speed_announce_interval`: `v` if in `[25, 50, 75, 100]` else `25`.
- `scanner_base_freq_hz`: `int(round(v))` (**round half to even**).
- `mcp_server_port`: `int(v)`.
- `preferred_device_name`, `speech_voice_name`, `speech_backend`: the stored strings
  unchanged. The frontend strips and matches them.
- Everything else: the `load_config()` value.

`load_config` has already validated these, so the extra steps are no-ops on any config it
returns. They are listed so nothing is assumed. The frontend clamps every spin value to
its control's range, as `wx.SpinCtrl.SetValue` did, and selects the first entry of a list
when a value is not in it.

#### `config_save_panel`

`args`: an object with exactly the 79 keys above, holding the control values. Numbers
arrive as JSON numbers, which JavaScript cannot mark as int or float, so the backend
coerces each key to the type in the table: ints to `i64`, floats to `f64` (so `880` is
written `880.0`), bools as bools, strings as strings. The frontend has already applied
`controls_to_config`'s fallbacks for empty selections.

Never rejects. Returns `{ "ok": true }`, or `{ "ok": false, "error": "<message>" }` when
the write failed. The frontend shows nothing on failure, as `_auto_save` swallowed
exceptions. It updates its copy of `cur_cfg["speech_voice_name"]` only when `ok`.

The backend does, in this order (`_auto_save` plus the rest of `controls_to_config`):

1. `cfg = panel_snapshot.copy()`, then `cfg[k] = args[k]` for every key.
2. `cfg["scanner_base_freq_hz"] = float(...)`.
3. `cfg["scanner_pitch_offset_oct"] = round(v * 12.0) / 12.0`, with **round half to even**.
4. Inside one try that ignores every error (steps 4 and 5 only): `disk = load_config()`.
   For each `k` in `ai_describer.all_config_keys()` that is in `disk`, set `cfg[k] = disk[k]`.
5. If `"pending_update_version"` is in `disk`, set `cfg["pending_update_version"] = disk[...]`.
6. `_write_config(CONFIG_PATH, cfg)` (atomic). On success the snapshot becomes `cfg`.

#### `config_reset`

No arguments. If it rejects, the frontend shows the `"Error"` box below and stops.

Sets the panel snapshot to a copy of `DEFAULT_CONFIG` and returns `{ "values": {...} }`
built from it exactly as `config_load` builds them. **Writes nothing.** The frontend fills
the controls, refreshes the voices (selecting by the default `speech_voice_name`, `""`)
and the devices, re-applies every show/hide rule, and starts the two-second save. When that
save runs, every key the panel does not own is written from `DEFAULT_CONFIG`, except the AI
Describer keys and `pending_update_version`, which steps 4 and 5 re-read from disk. So
**Reset to Defaults preserves only the AI Describer settings and the pending update flag;
everything else returns to `DEFAULT_CONFIG`**, `route_beacon_volume_db` included.

#### `speech_list_backends`

No arguments. Never rejects (empty list on failure).
Returns `string[]`: `configurator.list_speech_backends()`, Prism backend names in
priority order. The frontend puts `ui_choices.speech_auto_label` in front of them.

#### `speech_list_voices`

`args`: `{ "backend": "auto" | "<backend name>" }`. Never rejects.

Returns `configurator.list_speech_voices(backend)`:

```json
{ "voices": ["Microsoft David Desktop", "Microsoft Zira Desktop"],
  "features": { "supports_set_voice": true, "supports_set_rate": true,
                "supports_set_volume": true, "supports_braille": false } }
```

`voices` sorted with `key=str.lower` (stable). `features` is `null` when the backend
cannot be acquired. The frontend disables Voice, Rate and Volume and shows "Backend not
available on this machine." in that case.

#### `speech_test`

`args`: `{ "backend": string, "voice": string, "rate": int, "volume": int }`.
Never rejects. Returns `{ "ok": bool, "error": string | null }` from
`configurator.test_speech_voice` (the error already reads `"Speech test failed: ..."`).

#### `audio_list_devices`

No arguments. Never rejects. Returns `string[]`: `list_wasapi_output_devices()`, sorted,
empty on failure.

#### `audio_test_tone`

`args`: `{ "freq_hz": float, "level_dbfs": float }`. Never rejects. Returns
`{ "ok": bool, "error": string | null }` from `play_test_tone` (non-blocking playback).

#### `audio_preview_gear_tones`

`args`: `{ "root": string, "octave": int, "scale": string, "level_dbfs": float }`.
Never rejects. Returns `{ "ok": bool, "error": string | null }` from
`play_gear_tone_preview`.

### AI Describer panel (main mode only)

The AI Describer panel owns `ai_describer.all_config_keys()`: `ai_describer_provider`,
`ai_describer_disable_ui_toggle`, and for each provider its `key_cfg`, `model_cfg` and
extras' `key`s.

**The describer snapshot (`AIDescriberPanel.cur_cfg`).** The backend keeps a second config
dict for this panel, separate from the Configuration panel's. Secrets never leave the
backend: the frontend sees only `has_key`.

**State** (returned by `describer_load` and `describer_save`), built from the snapshot:

```json
{
  "provider": "gemini",
  "disable_ui_toggle": false,
  "providers": {
    "gemini": { "has_key": false, "model": "models/gemini-3-flash-preview", "extras": {} },
    "openai": { "has_key": true, "model": "gpt-5.6-terra",
                "extras": { "ai_describer_openai_base_url": "https://api.openai.com/v1",
                            "ai_describer_openai_reasoning_effort": "low",
                            "ai_describer_openai_detail": "auto" } }
  }
}
```

- `provider`: `snapshot[PROVIDER_CFG_KEY]` as stored. The frontend falls back to
  `ai_default_provider` if it is not a known id.
- `disable_ui_toggle`: `bool(snapshot["ai_describer_disable_ui_toggle"])`.
- `has_key`: `bool((snapshot.get(key_cfg, "") or "").strip())`.
- `model`: `snapshot.get(model_cfg)`; omit the field (or send `null`) when absent. The
  frontend selects it if listed, otherwise the first model.
- `extras[key]`: `snapshot.get(key)`; omit when absent. The frontend applies
  `_load_provider_controls`' fallbacks.

#### `describer_load`

No arguments. Rejects only if no config can be produced. Sets the describer snapshot to
`load_config()` and returns the state.

#### `describer_save`

`args`:

```json
{ "provider": "openai", "model": "gpt-5.6-terra",
  "extras": { "ai_describer_openai_base_url": "...", "...": "..." },
  "disable_ui_toggle": false,
  "key_action": "keep" | "clear" | "set",
  "key": "<plaintext, only when key_action is set>" }
```

`provider` is the provider whose values these are. During a provider switch it is the
**old** provider: the frontend flushes a pending text save before switching
(`on_change_provider`). `extras` holds that provider's extras only, already through
`_extra_value`: a choice's selected value or its default, a text field stripped or its
default.

Never rejects. Returns `{ "ok": bool, "error": string | null, "state": <state> }`, with
`state` always present. The backend does `_save`, plus the key handling `_clear_api_key`
and `_on_validated` did before calling it:

1. `key_cfg, model_cfg = config_keys_for(provider)`.
   `key_action == "clear"`: `snapshot[key_cfg] = ""`.
   `key_action == "set"`: `snapshot[key_cfg] = secretstore.protect(key)`.
   These change the snapshot even if the write in step 4 then fails, as `cur_cfg` was
   mutated before `_save` in the Python.
2. `cfg = load_config()`; if that raises, `cfg = snapshot.copy()`.
3. `cfg[PROVIDER_CFG_KEY] = provider`; `cfg[model_cfg] = model`;
   `cfg[key_cfg] = snapshot.get(key_cfg, "")`; for each extra of `provider`,
   `cfg[ex.key] = extras[ex.key]`; `cfg["ai_describer_disable_ui_toggle"] = bool`.
4. `_write_config(CONFIG_PATH, cfg)`. On success the snapshot becomes `cfg` and `ok` is
   true; on failure `ok` is false (the frontend shows nothing, as `_save` swallowed it).
5. Return the state built from the snapshot.

The frontend sends saves one at a time, in order.

#### `describer_validate_key`

`args`: `{ "provider": string, "key": string, "model": string, "extras": { "<arg>": value } }`.
`extras` is keyed by each extra's **`arg`** (for example `base_url`), not its config key.

Runs `ai_describer.validate_api_key(key, provider=provider, model=model, **extras)` off the
main thread (timeout `VALIDATE_TIMEOUT_SEC`). Never rejects. Returns
`{ "ok": bool, "error": string | null }`. It does **not** store anything. On `ok` the
frontend calls `describer_save` with `key_action: "set"`, `key` and the **then-current**
provider, as `_on_validated` used `self._provider` at callback time.

### Main tab (main mode only)

#### `console_history`

No arguments. Never rejects. Returns `string[]`: `_load_console_history()` (the last
`CONSOLE_HISTORY_MAX` = 50 entries of `console_history.json`, oldest first, `[]` on any
error).

#### `console_run`

`args`: `{ "index": int, "command": string }`. `command` is already stripped and non-empty.
`index` is the selected context's index (`_console_ctx_indices[sel]`, or 0).

The frontend has already appended the echo line `"[<context label>] > <command>"` to its
output; the backend does not emit it. The backend does the rest of `_on_console_run`:

1. History: if it is empty or its last entry `!= command`, append, keep the last 50, and
   `_save_console_history` (log `Failed to save console history: {e}` on error).
2. `send_console_command(f"EXEC|{index}|{command}")` (UDP to 127.0.0.1:4465).

Never rejects. Returns `{ "history": string[] }`, the history after step 1. The frontend
mirrors the same append-and-cap locally for its Up/Down navigation.

#### `console_refresh_contexts`

No arguments. Never rejects. Sends `CTXLIST`. Returns `null`.

#### `console_set_log_stream`

`args`: `{ "enabled": bool }`. Never rejects. Sends `LOGON` when true, `LOGOFF` when false.
Returns `null`.

#### `logs_menu_popup`

`args`: `{ "x": float, "y": float, "scale": float }`: the bottom-left corner of the
**View Logs...** button in CSS pixels relative to the web view's client area, and
`window.devicePixelRatio`. The menu's top-left is at physical client position
`(x * scale, y * scale)`, which is under the button, where keyboard focus is
(`btn_view_logs.PopupMenu(menu, (0, height))`).

The backend builds a **native** popup menu (`_on_view_logs` / `_log_menu_entries`) at
call time:

- "Application Log": a plain item for `LOG_FILENAME`, or, when any of
  `LOG_FILENAME.1` / `.2` / `.3` exists, a submenu "Application Log" holding "Current" plus
  "Previous N" for each existing backup.
- "Speech Log" (`SPEECH_LOG_PATH`), "AI Description Log" (`ai_describer.LOG_PATH`),
  "DOM Dump" (`DOM_DUMP_PATH`), "Scanner and HRTF Diagnostic"
  (`scanner_hrtf_diag.DEFAULT_PATH`), "Update Helper Log"
  (`UPDATE_DIR\apply_update.log`). An entry whose path cannot be resolved is left out
  and logged, as in the Python.
- A file that does not exist is listed **disabled**, labelled `"<label> (not created yet)"`.
  The rotated backups are only ever listed when they exist.
- Separator, then "Open Log Folder".
- Choosing a file opens it with the shell (`os.startfile`). If it has vanished meanwhile,
  the backend shows a message box: text `"File not found:\n{path}"`, title `"Not Found"`,
  warning icon, OK. "Open Log Folder" opens `CONFIG_DIR`. On failure it logs
  `Could not open the log folder: {e}` and shows text
  `"The log folder could not be opened.\n\n{e}"`, title `"Open Failed"`, error icon, OK.

Resolves (with `null`) once the menu has been dismissed or an item handled. Never rejects.
Keyboard focus stays on the button in the page.

#### `install_mod`

No arguments. Runs `install_mod_interactive(frame)` with every one of its message boxes
native and owned by the window. Resolves with its boolean when the flow ends. The frontend
ignores the value and logs a rejection.

#### `check_updates`

No arguments. Runs `updater.run_startup_flow(frame, NullGate(), manual=True)` and resolves
(`null`) when it ends. It shows all its own message boxes natively, and its progress
through the `update-progress` events. It rejects only where `_on_check_updates`' `except`
would fire (the flow could not be started). The frontend then logs nothing itself and
shows text `"The update check could not run.\n\n{error}"`, title `"Update Check Failed"`,
error icon, OK. *The backend should log `Manual update check failed: {e}` before rejecting.*

#### `update_cancel`

No arguments. Never rejects. Sets the cancel flag of the progress loop that is running
(`state["cancel"] = True` in `_run_with_progress`). The frontend calls it only while a
`cancellable` progress dialog is showing.

#### `exit_app`

No arguments. Never rejects. Starts the close path above (wx: `self.Close()`), so the
backend emits `close-requested`.

### Shared

#### `message_box`

`args`:

```json
{ "title": string, "text": string,
  "kind": "info" | "warning" | "error" | "question",
  "buttons": "ok" | "yes_no",
  "default": "ok" | "yes" | "no" }
```

Shows Win32 `MessageBoxW` owned by the main window (modal to it), with
`MB_ICONINFORMATION` / `MB_ICONWARNING` / `MB_ICONERROR` / `MB_ICONQUESTION`, `MB_OK` or
`MB_YESNO`, and `MB_DEFBUTTON2` when `default` is `"no"`. Resolves with `"ok"`, `"yes"` or
`"no"`. Never rejects.

The boxes the frontend raises, verbatim from the Python:

| Where | title | text | kind | buttons | default |
|---|---|---|---|---|---|
| `on_test_voice` | `Speech Test` | the `error` from `speech_test` | error | ok | ok |
| `on_test_tone` | `Audio Error` | the `error` from `audio_test_tone` | error | ok | ok |
| `on_preview_gear_tones` | `Audio Error` | the `error` from `audio_preview_gear_tones` | error | ok | ok |
| `on_reset` (ask) | `Reset to Defaults` | `Reset every setting on this tab to its default value?` | question | yes_no | no |
| `on_reset` (failed) | `Error` | `Failed to reset:\n{e}` | error | ok | ok |
| `on_reset` (done) | `Reset to Defaults` | `All settings on this tab have been reset to their defaults.` | info | ok | ok |
| `_clear_api_key` | `Clear API Key` | `Remove your stored {display} API key?` | question | yes_no | no |
| `_on_validated` (ok) | `API Key` | `API key validated and saved.` | info | ok | ok |
| `_on_validated` (failed) | `Invalid API Key` | `The API key could not be validated:\n\n{err}` | error | ok | ok |
| `_on_check_updates` (failed) | `Update Check Failed` | `The update check could not run.\n\n{e}` | error | ok | ok |

The backend raises every other box itself: all of `install_mod_interactive`, all of
`updater.py` (`_phase_two`, `_check_and_offer`, `_fail`), and those of `logs_menu_popup`.
They use the same mapping: `wx.YES_NO | wx.NO_DEFAULT` -> `MB_YESNO | MB_DEFBUTTON2`;
`wx.YES_DEFAULT` -> default button 1.

#### `frontend_ready`

No arguments. Never rejects. Called once, after every listener is registered and the
panels are filled. Returns what was buffered before it:

```json
{ "log_lines": [string], "console_lines": [string], "console_contexts": [{ "index": int, "label": string }] | null }
```

`log_lines` and `console_lines` have the same format as the `log-line` and
`console-line` payload texts. `console_contexts` is the last completed CTX list, if a
`CTXEND` arrived before the page was ready. From this call on, those arrive as events. In
main mode this is also the signal to start the startup update flow. In configurator mode
all three are empty / `null` and no flow starts.

#### `close_ready`

No arguments. Never rejects. The frontend has flushed its saves; continue closing (see
**Close**).

## Events

| Event | Payload | When |
|---|---|---|
| `log-line` | `{ "text": string }` | Each record on the `bnvdahook` logger that `WxLogHandler` would have received, formatted `"%(asctime)s [%(levelname)s] %(message)s"` with `datefmt="%Y-%m-%d %H:%M:%S"`, **no trailing newline**. Not emitted before `frontend_ready` (buffered) or after close starts. Main mode only |
| `console-line` | `{ "text": string }` | Each line `on_console_message` passes to `_console_append`, already formatted: `"  error: {body}"`, `"  = {body}"`, `"  (queued)"`, `"  " + content` (OUT), `"  [{lvl} {origin}] {msg}"` (LOG). Records the MCP console tap consumes are not emitted. Same buffering rules as `log-line` |
| `console-contexts` | `{ "items": [{ "index": int, "label": string }] }` | On `CTXEND` when CTX lines were accumulated (`_rebuild_context_choice`); the pending list is then cleared. The frontend keeps the previous selection by context index |
| `update-progress` | `{ "title": string, "text": string, "fraction": float \| null, "cancellable": bool }` | While `_run_with_progress` runs, at each tick of its loop (every ~50 ms) or whenever the values change. The first event opens the progress dialog |
| `update-progress-close` | `{}` | When `_run_with_progress` ends (its `dlg.Destroy()`), before any message box that follows |
| `close-requested` | `{}` | See **Close** |

`update-progress` values, from `_run_with_progress`:

- `title`: the `title` argument: `"Checking for updates"` or `"Downloading update"`.
- `text`: `message` while the total is unknown (`"Contacting GitHub..."`,
  `"Downloading BeamNG_accessibility_mod.zip..."`). Once `total > 0`,
  `"%s\n%.1f of %.1f MB" % (message, done / 1048576.0, total / 1048576.0)`.
- `fraction`: `null` while the total is unknown (wx `Pulse`, an indeterminate bar). Once
  `total > 0`, `min(100, int(done * 100 / total)) / 100`.
- `cancellable`: the `cancellable` argument (true only for the download).

The frontend's Cancel button and Escape call `update_cancel`. The dialog stays open until
`update-progress-close`.

## Commands by mode

| Command | main | configurator |
|---|---|---|
| `app_info`, `ui_choices`, `message_box`, `frontend_ready`, `close_ready` | yes | yes |
| `config_load`, `config_save_panel`, `config_reset` | yes | yes |
| `speech_list_backends`, `speech_list_voices`, `speech_test` | yes | yes |
| `audio_list_devices`, `audio_test_tone`, `audio_preview_gear_tones` | yes | yes |
| `describer_load`, `describer_save`, `describer_validate_key` | yes | no |
| `console_history`, `console_run`, `console_refresh_contexts`, `console_set_log_stream` | yes | no |
| `logs_menu_popup`, `install_mod`, `check_updates`, `update_cancel`, `exit_app` | yes | no |

Events in configurator mode: `close-requested` only. The progress listeners are
registered in both modes, but nothing emits to them in configurator mode.
