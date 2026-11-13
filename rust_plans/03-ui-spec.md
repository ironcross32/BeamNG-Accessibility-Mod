# Interface specification

The Python interface is wxPython: `BeamTelFrame` in `beamtel.py` (from line 11515),
`ConfigPanel` and `AIDescriberPanel` in `config_ui.py`, the standalone window in
`configurator.py`, and the dialogs in `updater.py` and `config_ui.install_mod_interactive`.
The new interface is one HTML document in a Tauri window.

The frontend agent reads all of those sources in full. This file states the structure and
the rules; the sources are the authority for every label, tooltip, range, default,
enable/disable rule and show/hide rule. **Every control in the Python interface exists in
the new one, with the same label, in the same order.**

## Accessibility rules

1. **Semantic HTML first.** A native element is used wherever one exists:
   `<button>`, `<select>`, `<input type="checkbox|radio|number|text">`, `<label for>`,
   `<fieldset>`/`<legend>`, `<textarea readonly>`, `<dialog>`, `<progress>`, `<h1>`-`<h3>`.
2. **ARIA only where HTML has no element.** The known case is the tab strip: HTML has no
   tab element, so it uses `role="tablist"`, `role="tab"`, `role="tabpanel"`,
   `aria-selected`, `aria-controls`, `aria-labelledby`. Any other ARIA attribute needs a
   one-line comment in the HTML saying why no native element does the job. No `aria-label`
   on a control that a `<label>` can name. No `role` that repeats an element's own role.
   No `tabindex` above 0.
3. **Every grouping is a heading.** Each wx `StaticBox` group becomes
   `<fieldset><legend><h2>Group name</h2></legend> ... </fieldset>`. The fieldset gives the
   "entering group" announcement the StaticBox gave; the heading inside the legend (valid
   HTML) gives heading navigation. Each wx `RadioBox` is a nested
   `<fieldset><legend><h3>...</h3></legend>` of radio inputs. Each tab panel starts with an
   `<h1>` carrying the tab's name. Only the selected panel is rendered (`hidden` on the
   others), so the document shows one `<h1>` at a time.
4. **Every control has a visible `<label>`** with the wx label text. Where wx gave a
   control a `SetName` that differs from its visible label (for example the spin control
   named "Highlight every N clicks" beside a checkbox labelled "Highlight Every:"), the
   accessible name must come out as the wx name. Prefer restructuring the visible label
   text to achieve that; a visually hidden `<span>` inside the `<label>` is the fallback.
5. **Hidden means gone.** wx hides and disables conditional rows together. Use the
   `hidden` attribute on the row so it leaves both the tab order and the accessibility
   tree. If the control that has focus is about to be hidden or disabled, move focus to
   the control the Python names as the fallback first (`_enable`, `_set_row`).
6. **Nothing speaks on its own that did not before.** No live regions on the log, the
   console output, or status text. The wx controls were silent until read, and the user
   is usually in the game listening to the mod's own speech. (`<output>` carries an
   implicit live region, so it is not used for these.)
7. **Tooltips.** wx tooltips were hover-only and not announced. Each tooltip's text goes
   in a `title` attribute on the control's `<label>` (or on the `<button>` itself, which
   has no label), not on the input, so hover behaviour is kept and the screen reader's
   announcement of the control does not grow. *Open question for the user: whether the
   help text should instead be exposed to the screen reader as a description.*
8. **Keyboard.** Everything is reachable with Tab and operable with the keys the native
   element already handles. Additions, all taken from the wx behaviour:
   - Tab strip: Left/Right move between tabs and select; Home/End; Ctrl+Tab and
     Ctrl+Shift+Tab switch tabs from anywhere in the window, as `wx.Notebook` does.
   - Decimal spin controls: PageUp/PageDown step by ten increments
     (`_bind_spin_double_page_keys`).
   - Console command field: Enter runs; Up/Down walk the history with the prefix-search
     rules in `_on_console_input_key`.
9. **Focus is never lost.** After a dialog closes, focus returns to the control that
   opened it (the Python does this deliberately for the API key button). After a tab
   switch by keyboard, focus stays on the tab.
10. **Text areas are reviewed with the screen reader's own keys.** The log and the console
    output are `<textarea readonly wrap="off">`. Append with `setRangeText(text, end, end,
    "preserve")` so the review cursor does not jump while the user is reading.
11. **Appearance.** System font, system colours via `color-scheme: light dark`, visible
    focus outlines, no fixed pixel heights that clip at 200% zoom, respects
    `forced-colors` (Windows high contrast). Layout is one column of groups, like the wx
    panels.
12. **Language and title.** `<html lang="en">`. The window title is set by the backend:
    `BeamNG Accessibility <version>` or `BeamTel Configurator`.

## Structure

```
<body>
  <div role="tablist" aria-label="BEAM Tabs">        (wx: notebook.SetName("BEAM Tabs"))
    <button role="tab">Main</button>
    <button role="tab">Configuration</button>
    <button role="tab">AI Describer</button>
  </div>
  <main>
    <section role="tabpanel"> <h1>Main</h1> ... </section>
    <section role="tabpanel" hidden> <h1>Configuration</h1> ... </section>
    <section role="tabpanel" hidden> <h1>AI Describer</h1> ... </section>
  </main>
  <dialog> ... </dialog>   (API key entry, update progress)
</body>
```

In configurator mode (`beamtel.exe --configurator`) the tab strip is absent and the
document is the Configuration panel alone under its `<h1>`.

### Tab: Main

In tab order:

1. **Application Log**: read-only multi-line text, labelled "Application Log". Receives
   every `bnvdahook` log record formatted `%Y-%m-%d %H:%M:%S [LEVEL] message`.
2. **View Logs...** button. Opens a **native** popup menu anchored under the button (a
   real Windows menu, as wx used, because a screen reader already announces one
   correctly). Entries and rules are `_log_menu_entries` and `_on_view_logs`: Application
   Log (a submenu "Current / Previous 1..3" only when rotated backups exist), Speech Log,
   AI Description Log, DOM Dump, Scanner and HRTF Diagnostic, Update Helper Log,
   separator, Open Log Folder. A file that does not exist yet is listed, disabled, with
   " (not created yet)" appended.
3. Group heading **Developer Console**:
   - "Context:" select (initially `GE - Lua`, `GE - TorqueScript`, `CEF/UI - JS`;
     replaced by the mod's `CTX` list, keeping the selection by context index) and a
     **Refresh Contexts** button.
   - "Command:" text field and a **Run** button.
   - **Console Output**: read-only multi-line text.
   - "Filter:" text field (substring, case-insensitive, re-renders the backlog),
     **Stream game log** checkbox, **Clear Output** button.
4. **Install Mod**, **Check for Updates**, **Exit** buttons.

### Tab: Configuration

Group headings, in order. The control lists name what is known from the source outline;
the frontend agent completes them from `config_ui.py` and must not omit any.

| # | Heading | Controls |
|---|---|---|
| 1 | Speech | Speech Backend (select), Voice (select), Rate 0-100 (integer spin), Volume 0-100 (integer spin), backend capabilities text, Refresh Voices, Test Voice |
| 2 | General | Display Units (radio group: Imperial / Metric), Telemetry Protocol (radio group: Extended / OutGauge), Launch BeamNG.drive on startup, Graphics API (select; shown only when launch is on), Check for updates on startup |
| 3 | Automatic announcements | Announce turn signals, Speed announcements, Announce every (select; labels follow the unit system), Speak drift scoring, Report gear changes (Off / As speech / As audio), Report sport mode only once (shown for speech or audio), Forward gears (speech only), Root note / Octave / Scale / Gear Tone Volume / Preview gear tones (audio only), Announce clickspot actions, Periodic scanner distance callouts, Callout every (select), Quiet menu navigation while holding a direction |
| 4 | Road guidance | Lane correction guidance, Speak approaching intersections, Speak approaching stop signs, Speak traffic lights and state changes, Speak map speed limit changes, Speak road surface changes, Stop bar tone, Intersection proximity and entry tones, Include private roads, Off-road Beacon Volume, Lane Correction Volume, Intersection Tone Volume |
| 5 | Compass Clicks | Click Interval, Highlight Every (checkbox) + N clicks (spin), Compass Click Volume, Low Speed Click Volume, Low Speed Stop Tone Volume, Wheel Slip Tone Max Volume, Placement Ping Volume, HRTF Binaural Processing, HRTF Dataset, HRTF Elevation, Equalise HRTF Tone, HRTF Front Emphasis, HRTF Distance Gain |
| 6 | Vehicle Scanner | Steering tone checkbox, Base Frequency, Alignment Pitch Offset |
| 7 | Pitch & Roll (accessible name "Pitch and Roll") | Pitch and Roll Tones, Maximum Volume, Stabilized Volume |
| 8 | Shift Tone | Shift Tone Frequency, Shift Tone Level, Test Shift Tone |
| 9 | Warning Sounds | Oil Overheating Alert, traction-control clicks checkbox, Check Engine Buzzer Level, Oil Warning Chime Level, Obstacle Cue Volume, Obstacle Warning Sensitivity |
| 10 | Loader Implement | implement tones checkbox, implement proximity speech checkbox, Docking Instrument Tones, Ground Proximity Tone Level, Tilt Tone Level, Docking Tone Level |
| 11 | Terrain Scanner | Terrain Scan Tones, Terrain Scan Level |
| 12 | Large Cannon | Announce Shot Outcome |
| 13 | Learn Bindings | Speak Full Action Description |
| 14 | Audio Device | Follow Default Audio Device, Fixed Output Device (select), Refresh Devices, Device Poll Interval |
| 15 | Developer | MCP server checkbox, MCP Server Port |

After the groups: **Reset to Defaults** button.

Behaviour to carry over exactly: the two-second debounced auto-save (`_schedule_save`,
`_auto_save`), `flush_pending_save` on window close, the `_loading` guard, every
`on_toggle_*` and `on_change_*` handler, the unit-dependent interval labels, backend
capability gating of Voice/Rate/Volume, and what `on_reset` restores and what it leaves.

### Tab: AI Describer

1. Intro paragraph (text names the selected provider; see `_retitle_for_provider`).
2. Heading **Provider**: Service (select).
3. Heading **`<Provider>` API Key** (the heading text changes with the provider): one
   button whose label flips between "Set API key" and "Clear API key", and a status line
   ("No API key configured." / "An API key is configured." / "Validating API key...").
4. Heading **Vision Model**: Model (select, repopulated per provider).
5. Heading **Advanced**: one row per provider extra from the registry (select or text);
   only the active provider's rows are shown, and the whole group is absent when the
   provider declares none.
6. Heading **Capture**: Disable automatic UI hiding during capture.

Behaviour to carry over: eager save on every change except text extras (debounced), the
flush-before-provider-switch, the merge-onto-fresh-disk-read save, validation in the
background with the button disabled and focus restored afterwards.

## Dialogs and menus

| Python | New |
|---|---|
| `wx.MessageBox` (information, warning, error, yes/no with a default) | The Win32 `MessageBoxW` itself, called from Rust with the window as owner and the same title, text, icon, buttons and default button. This is what wx called, so the screen reader experience is identical. Not a web dialog |
| `wx.TextEntryDialog` (API key) | `<dialog>` opened with `showModal()`: the provider's help text, a labelled text field, OK and Cancel. Enter is OK, Escape is Cancel, focus returns to the API key button |
| Update download progress (see `updater.py`) | `<dialog>` with a `<progress>` element, a text line and Cancel, driven by backend events. The frontend agent reads `updater.py` for the exact stages and wording |
| `wx.Menu` popup (View Logs) | Native Tauri popup menu with a submenu, built in Rust |
| File/folder open (`os.startfile`) | `ShellExecuteW` from Rust |

## Frontend to backend contract

The frontend agent (U1) owns `rust/ui/CONTRACT.md` and writes the final, exact list there
(names, argument and return shapes). The backend agent (U2) implements what that file
says. The starting list:

Commands:

- `app_info` -> `{ version, title, mode: "main" | "configurator" }`
- `ui_choices` -> every table the panels need so no table is duplicated in JS: gear root
  notes, octaves and scales; HRTF sets and elevation range; speed and callout intervals;
  obstacle sensitivities; AI providers with their models, extras, key help and display
  names
- `config_load` -> the merged, validated config (secrets masked to "set"/"unset")
- `config_save_panel(values)`, `config_reset` -> the Configuration panel's save and reset
- `describer_save(values)`, `describer_validate_and_set_key(provider, key, model, extras)`
  -> `{ ok, error }`, `describer_clear_key(provider)`
- `speech_list_backends`, `speech_list_voices(backend)` -> `{ voices, features }`,
  `speech_test(backend, voice, rate, volume)`
- `audio_list_devices`, `audio_test_tone(freq_hz, level_dbfs)`,
  `audio_preview_gear_tones(root, octave, scale, level_dbfs)`
- `logs_menu_popup` (anchor rectangle of the button), `install_mod`, `check_updates`,
  `exit_app`
- `console_run(selection_index, command)`, `console_refresh_contexts`,
  `console_set_log_stream(enabled)`, `console_history` -> list
- `message_box(title, text, kind, buttons, default)` -> answer, for the few prompts the
  frontend itself raises
- `close_ready` (the frontend has flushed its pending saves)

Events from the backend:

- `log-line` `{ text }`
- `console-line` `{ text }`, `console-contexts` `{ items: [{ index, label }] }`
- `update-progress` `{ stage, text, fraction }`, `update-progress-close`
- `close-requested` (flush debounced saves, then call `close_ready`)

The console record parsing (`on_console_message`: CTX, CTXEND, RESP, OUT, EXECEND, LOG,
and speaking a one-line result) lives in Rust; the frontend only appends lines and keeps
the backlog and filter.

## Review checklist (used by the UI audit in wave 3)

- Every control in `config_ui.py` and `BeamTelFrame` has a counterpart; the audit produces
  a two-column table and it has no empty cells.
- Heading outline: one `<h1>` per panel, an `<h2>` per group, an `<h3>` per radio group.
- `grep` for `aria-` and `role=` in `rust/ui/` finds only the tab pattern and commented,
  justified exceptions.
- No `<div>` or `<span>` with a click handler.
- Every input has a `<label for>`; every `id` is unique.
- Tab order equals source order equals the wx order.
- Works with the keyboard alone; works at 200% zoom; works in forced-colours mode.
