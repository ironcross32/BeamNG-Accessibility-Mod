# Audit: config and UI inventory (V1, slice 1)

2026-11-21. Python: `configurator.py`, `beamtel.py`'s config half, `config_ui.py`
(`ConfigPanel`, `AIDescriberPanel`), `BeamTelFrame`'s non-console controls and
`_on_console_run`. Rust: `beam_base::config`, `src-tauri` (`panel.rs`, `describer.rs`,
`choices.rs`, `commands.rs`, `tones.rs`), and `rust/ui/js/util.js` where the frontend
owns a behaviour. Audited by the orchestrator, which wrote U2 (the plan wanted a
non-author; the user chose sequential audits here instead).

## Method

1. **Every fixture regenerated against today's Python.** All generators in
   `rust/fixtures/gen/` were rerun on 2026-11-21. Every data file came out byte-identical
   except `config.json`, whose one difference is a DPAPI-sealed secret: sealing is
   randomised, so that blob changes on every run. So the modules finished before the
   Python working tree moved (AH's HRTF, B3's road guidance, A1-A3) still match it
   wherever a fixture reaches. This holds for every later slice too.
2. **A new fixture drives the real wx `ConfigPanel`** (`fixtures/gen/config_panel.py`,
   `src-tauri/tests/panel_fixture.rs`). The generator builds the panel off-screen with
   `LOCALAPPDATA` pointed at a scratch folder. For each case it feeds a config through
   `load_into_controls` and records what `controls_to_config` writes, with types. The
   Rust test puts the same config through `control_values` and `build_saved_config` and
   requires every key to match, type included. It also requires `PANEL_KEYS` to equal
   the keys `controls_to_config` assigns (79 both sides).
3. Hand walk of the rest.

## Results

| Python | Rust | State |
|---|---|---|
| `DEFAULT_CONFIG` (both), `load_config` (both), `_write_config`, `_read_config_raw`, corruption backups | `beam_base::config` | equivalent: B1's fixtures, regenerated today, unchanged |
| `ConfigPanel.load_into_controls` | `panel::control_values` | equivalent: new fixture |
| `ConfigPanel.controls_to_config` (backend half) | `panel::build_saved_config` | equivalent: new fixture |
| `_auto_save` disk re-read of AI keys and `pending_update_version` | `build_saved_config(.., disk)` | equivalent: unit test `save_coerces_and_keeps_foreign_keys` |
| `on_reset` (snapshot swap, nothing written) | `config_reset` | equivalent |
| wx spin `SetValue` / `GetValue`: range clamp and digit rounding | `util.js` `clampRound` / `roundTo` | **was different, fixed**: see below |
| `_select_backend`, voice and device selection by text | frontend (`config-panel.js`) | out of the backend's scope; U1 inventory |
| `list_wasapi_output_devices` | `beam_audio::device` | equivalent (sorted names of WASAPI outputs) |
| `test_speech_voice` | `speech_test` | equivalent, same text and `"Speech test failed: "` prefix |
| `play_test_tone` | `tones::test_tone` | equivalent waveform (0.75 s triangle, 50 ms linear fade, f64 then f32) |
| `play_gear_tone_preview` | `tones::gear_preview` | difference, kept: a tone that overruns the buffer is clipped where numpy raised |
| `AUDIO_TEST_OK` | `ui_choices.audio_test_ok` | difference, kept: always true (the audio stack is compiled in) |
| `AIDescriberPanel._save`, `_clear_api_key`, `_on_validated` key handling | `describer::save_at` | equivalent (contract steps 1-5, unit tests) |
| `AIDescriberPanel` validation worker | `describer::validate` | difference, kept: an escaped exception reports a failed validation instead of killing the worker and leaving the button disabled |
| `validate_api_key(..., timeout=VALIDATE_TIMEOUT_SEC, model, **extras)` | `try_validate_api_key(.., VALIDATE_TIMEOUT_SEC, Opts{model, extras})` | equivalent (30 s both) |
| `configurator.main` (title, 600x940, min 580x940, close flushes) | `lib.rs` setup, `Shell` close | equivalent |
| `BeamTelFrame` title, log pane handler, View Logs menu, open log folder | `Mode::title`, `PaneLog`, `logs_menu` | equivalent |
| `_on_console_run` (history skip-duplicate, cap 50, save, `EXEC|idx|cmd`) | `console_run` | equivalent; the echo line is the frontend's |

## Fixed

- **Spin rounding on a tie** (`rust/ui/js/util.js`, U1's code). wx formats a spin value
  with printf, and the Windows CRT rounds an exact tie to even: -12.5 at 0 digits is -12,
  0.125 at 2 digits is 0.12. The page used `toFixed`, which rounds both away from zero.
  So a tie, loaded or typed, differed from the wx build. `roundTo` now finds a true tie
  on the exact binary value and rounds it to even; anything else is still `toFixed`.
  Checked against Python's `"%.*f"` on 15 values, including 2.675 and 1.005, which only
  look like ties.

## Open

None in this slice. The kept differences above are deliberate and recorded in
`README.md` (U2 row).

## Note for later

- `config.json`'s generator seals a secret with DPAPI, so regenerating it always shows a
  diff. It is not a behaviour change; restore the file with `git checkout`.
