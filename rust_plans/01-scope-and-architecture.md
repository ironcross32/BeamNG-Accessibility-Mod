# Scope and architecture

## What exists today

About 27,000 lines of Python in the shipped application:

| File | Lines | Role |
|---|---|---|
| `beamtel.py` | 11,400 | Engine: telemetry loop, ~25 UDP listeners, keyboard layers (F9, F10), status and function menus, virtual browser, all phrasing that is not in a pure module, the wx main window |
| `audio.py` | 6,700 | Procedural synthesis, 48 kHz stereo; the callback alone is ~2,900 lines |
| `vehicle_spawner.py` | 2,500 | Accessible vehicle spawner and placement |
| `config_ui.py` | 2,400 | wx Configuration and AI Describer panels, mod installer |
| `mcp_server.py` | 1,470 | MCP automation server over loopback HTTP |
| `updater.py` | 650 | GitHub updater with self-replacement |
| `configurator.py` | 500 | Config load, coerce, clamp, atomic write; standalone configurator window |
| Pure modules | ~4,500 | `road_guidance`, `road_diagnostics`, `challenge_results`, `ai_describer`, `nvda_ws_speaker`, `speech`, `wheel_slip`, `hrtf`, `gear_report`, `camera_lineup`, `wheel_report`, `assistant_route`, `secretstore`, `driving_assistant`, `scanner_hrtf_diag`, `assistant_diagnostics`, `fuel_stations`, `route_beacon`, `audio_debug`, `drift_speech`, `induction_report`, `poi_guidance`, `hrtf_sets`, `bnh_logger` |

`docs/python-components.md` and the other `docs/*.md` files record the reasoning behind
the non-obvious decisions. **They are required reading for whoever ports the matching
file**; most of what they record is a trap that is invisible in the source.

## In scope

1. The whole runtime application (`beamtel.py` and everything it imports).
2. The interface: the three-tab main window (Main, Configuration, AI Describer), the
   standalone configurator window, and every dialog and menu (updater prompts, mod
   installer prompts, API key entry, the View Logs menu).
3. The build and release pipeline (`build.py`, `scripts/release.ps1`,
   `.github/workflows/release.yml`).
4. The Python `diagnostic/*_sim.py` simulators, re-expressed as Rust tests.
5. Last: developer tooling (`tools/`, `bake_hrtf.py`, `diagnostic/map_survey`, the
   standalone UDP listeners in `diagnostic/`).

## Not in scope

- `bng_mod/` (Lua and the in-game JS runtime). Unchanged. Its `diagnostic/*_sim.lua` and
  `*_sim.js` files stay as they are.
- Any behaviour change, rewording, retuning or "improvement". A bug found in the Python is
  reported to the orchestrator and ported as it is unless the user says otherwise.

## Workspace layout

```
rust/
  Cargo.toml                 workspace; shared dependency versions; one target dir
  crates/
    beam-base/               paths, logger, config, secretstore, hrtf_sets, route_beacon,
                             gear_report, the AI provider registry, speech config migration
    beam-reports/            wheel_slip, wheel_report, induction_report, camera_lineup,
                             audio_debug, drift_speech, fuel_stations, poi_guidance
    beam-road/               road_guidance, assistant_route, driving_assistant,
                             road_diagnostics, challenge_results, assistant_diagnostics
    beam-speech/             Prism FFI (prism.dll) and the speech.py layer
    beam-audio/              hrtf, terrain scan render, waveform generators,
                             AudioController, the callback, device handling,
                             scanner_hrtf_diag
    beam-engine/             everything in beamtel.py that is not the window, plus
                             vehicle_spawner, the UI bridge, the MCP server, ai_describer,
                             the updater core and the mod installer core
  src-tauri/                 the application crate (binary name `beamtel`): Tauri setup,
                             commands, events, native menus and dialogs, the GUI side of
                             the developer console and the updater flow
  ui/                        index.html, styles.css, js/*.js (no build step)
  fixtures/
    gen/                     Python scripts that import the reference modules and write
                             golden cases
    data/                    the golden cases (JSON, small binary buffers)
  xtask/                     build, package and bundle (port of build.py)
```

Why six library crates and not one: several agents work at once, and a crate that does not
compile blocks everyone inside it. Crate boundaries are the isolation. They also keep the
pure logic testable without Tauri, WebView2, an audio device or the game.

`beam-engine` does not depend on Tauri. It talks to the window through a small trait
(`UiSink`: log line, console record, request a dialog, set launch gate) that `src-tauri`
implements, so the engine is testable headless, exactly as the Python sims drive
`beamtel` without the wx frame.

## Runtime design

The threading model is kept as it is, because a great deal of the documented behaviour
depends on which thread does what:

| Python | Rust |
|---|---|
| wx main thread | Tauri main thread and the WebView |
| `beamtel-engine` daemon thread | `std::thread` started from Tauri `setup` |
| One daemon thread per UDP listener, blocking socket with timeout | The same: `std::thread` + `std::net::UdpSocket` with a read timeout and a shared stop flag |
| `state_lock` guarding module globals | One `State` struct behind a `parking_lot::Mutex`, plus the same handful of separate locks Python has (`_slots_lock`, the capture lock, `_exec_lock`) |
| `command-dispatch` serial worker fed by a queue | A worker thread fed by an `mpsc` channel |
| `sounddevice` callback | `cpal` WASAPI output stream callback |
| `keyboard` library hooks with `suppress=True` | A `WH_KEYBOARD_LL` hook on a dedicated message-loop thread (`windows` crate); the hook callback classifies and enqueues only, as the Python comment at `beamtel.py:5066` requires |
| aiohttp WebSocket 8765, TCP relay 8766, MCP HTTP | A tokio runtime on its own thread: `axum` for HTTP/WebSocket, tokio TCP for the relay |
| `threading.Timer` | A small timer helper on the engine side (thread + sleep, cancellable) |
| `STOP` event | `Arc<AtomicBool>` plus a condvar where Python waits on it |

Async is used only where Python used asyncio. The synchronous parts stay synchronous so
the ported control flow reads line for line against the original.

### Libraries

| Need | Python | Rust |
|---|---|---|
| Window and web view | wxPython | `tauri` 2, WebView2 |
| Audio output and device enumeration | sounddevice (PortAudio, WASAPI) | `cpal` (WASAPI) |
| Arrays and FFT | numpy | plain `Vec<f32>`/`Vec<f64>` loops, `rustfft`/`realfft` |
| HRTF `.npz` | numpy | `zip` + a small `.npy` reader in `beam-audio` |
| Speech | prismatoid (cffi over `prism.dll`) | direct FFI to the same `prism.dll`, loaded with `libloading` from beside the exe |
| Keyboard and mouse hooks, window lookup, DPAPI, registry | keyboard, mouse, ctypes, winreg | `windows` crate |
| Screen capture | mss | `windows` crate GDI capture (BitBlt), `png` to encode |
| HTTP client (AI providers, GitHub) | urllib | `ureq` (blocking, matches the call sites) |
| HTTP/WebSocket servers | aiohttp | `axum` + `tokio` |
| JSON | json | `serde_json` with `preserve_order` |
| Zip (mod install, update bundle) | zipfile | `zip` |
| Logging | logging + RotatingFileHandler | `log` facade with a rotating file sink written in `beam-base` to the same file name, size and backup count |

The Tauri and cpal APIs move between releases. An agent that is not certain of a call
looks it up rather than writing it from memory.

### The window

- One Tauri window, title `BeamNG Accessibility <APP_VERSION>`, minimum 600x500, default
  700x700. The version in the title is a documented feature (a screen reader announces it
  on focus) and `_find_game_window`'s prefix match must not collide with it.
- `beamtel.exe --configurator` opens the standalone configurator window (title
  `BeamTel Configurator`, the Configuration panel only, no engine), which replaces
  `python configurator.py`.
- **The process does not require administrator rights** (decided 2026-11-21, a change
  from the Python build). A `WH_KEYBOARD_LL` hook needs no elevation to see and suppress
  keys going to a window at the same integrity level, and BeamNG runs un-elevated: it is
  launched through Steam (`steam.exe -applaunch`), which hands off to the running Steam.
  Verified in game on 2026-11-21 with an un-elevated debug build: F9 then F read fuel and
  did not exit the stationary car; the browse dialogs kept their arrows. Requiring admin
  was in fact self-defeating: an elevated beamtel that starts Steam itself makes the game
  elevated too. The one case it cannot handle is a user who runs Steam or BeamNG as
  administrator (Windows hides an elevated window's keystrokes from a normal process's
  hook); the `uac-admin` cargo feature still builds a `requireAdministrator` exe for that.
  The release manifest is `asInvoker` (`manifests/default.xml`).
- No console window in release builds.

### Frontend to backend contract

Tauri commands (`invoke`) for everything the wx panels called directly, and Tauri events
for everything that was a `wx.CallAfter` from another thread (log lines, console records,
"config changed on disk", update prompts). The command and event names are listed in
`03-ui-spec.md` and are the contract between the frontend agent (U1) and the backend
agent (U2).

### Data files and distribution

- The HRTF sets stay as the committed `hrtf_*.npz` files and ship beside the executable,
  with `HRTF_DATA_LICENSES.md`, as now.
- `prism.dll` ships beside the executable. The build takes it from the `prismatoid` wheel
  at the pinned version (0.18.2) so both builds speak through the identical library.
- The release bundle keeps its name and its two required members (`beamtel.exe`,
  `bng_screenreader_mod.zip`), so `updater.py`'s validation accepts a Rust release and the
  Python build can update into it.
- The Nuitka `.appdata` cache directory is still removed by the update helper: on the
  first update from a Python build it is the old build's leftovers.

## Parity strategy in one paragraph

Every pure function is checked against the Python by golden fixtures: a script in
`rust/fixtures/gen/` imports the real Python module, runs it over a table of inputs and
writes the outputs; a Rust test loads them and compares. Every Python simulator in
`diagnostic/` is ported as a Rust test with the same scenarios and the same assertions.
What neither can reach (the hooks, the audio device, the web view, the game) is covered
by a second agent auditing the Rust against the Python function by function, and finally
by the user driving it. Details in `04-workstreams.md` and `05-orchestration.md`.
