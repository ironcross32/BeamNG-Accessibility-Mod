# Audit: keyboard, F9/F10 layers, menus, virtual browser (V1, slice 5)

2026-11-21. Python: the keyboard plumbing in `beamtel.py` (command worker, the shared
F9/F10 layer hook, suppressed hooks, arrow ownership, status mode, `install_hotkeys`),
`_on_next_key_press`, `_on_ai_key_press`, the accessibility menu and the virtual browser.
Rust: `beam-engine/src/keyboard/`, `f9_commands.rs`, `ai_layer.rs`, `menus.rs`,
`virtual_browser.rs`. Audited by the orchestrator, which did not write E5-E7.

## Method

1. Coverage of the real Python under every fixture generator (method in
   `audit-listeners.md`), per function.
2. Hand walk of the plumbing no generator reaches, and of every behaviour line the
   generators miss in the handlers.
3. In game (user, 2026-11-21, un-elevated): F9 and F10 layers, key suppression and AI
   commands all worked.

## Coverage of the real Python handlers

| Function | Statements run |
|---|---|
| `_on_next_key_press` (F9, 588 statements) | 97% |
| `_on_ai_key_press` (F10, 133) | 94% |
| `navigate_accessibility_menu` | 96% |
| `open_virtual_browser`, `close_virtual_browser`, `_drive_virtual_browser`, `_on_vbrowser_nav` | 90-100% |
| `_on_vbrowser_adjust` / `_escape` / `_enter` | 60-80% (callback error logging, and the Shift step) |

## Walked by hand

| Python | Rust | State |
|---|---|---|
| `_command_worker_loop` (a raising handler is logged, the worker lives) | `keyboard/mod.rs` `command_worker_loop`, `run_job` | equivalent |
| `_kb_layer_press`: key-ups pass; no layer passes; modifiers noted and eaten; the opening F9/F10 repeat eaten; anything else eaten and dispatched with a modifier snapshot | `classify_layer_key`, `kb_layer_press` | equivalent |
| `_kb_layer_release`, `_kb_run_layer_command` (drop a command whose layer has closed or switched) | same names | equivalent |
| `_kb_open_layer` (retarget an installed hook; reset live modifiers) | `kb_open_layer` | equivalent. Python's "failed to install the hook" branch has no counterpart: the in-process listener cannot fail to add a hook |
| `_kb_close_layer`, `_clear_next_key_hook` | same names | equivalent |
| `_unhook_suppressed` (plus Python's clean-up of the `keyboard` package's aliased blocking lists) | `unhook_suppressed` | equivalent; the aliasing bug it works around belongs to the `keyboard` package, which the Rust listener does not reproduce |
| `release_arrow_owners` (browser, line-up layer, status mode, orphaned status hooks) | same name | equivalent, same log text |
| `on_status_arrow_press` (no arrows while a browser holds them), `toggle_status_mode` | `menus.rs` | equivalent |
| `install_hotkeys`, `on_f9`, `on_f10` (focus check, command context, prompt, speech preloaded before the hooks) | `keyboard/mod.rs` | equivalent |
| `_on_vbrowser_adjust`: Shift steps by ten | `virtual_browser.rs:193` | equivalent |
| `open_virtual_browser` closes the spawner's modal first | `virtual_browser.rs:250` | equivalent |
| F9 powertrain dump: "dumped, N values logged" / "timed out — is a vehicle spawned?" | `f9_commands.rs` `run_log_dump` | equivalent (one function for the three dumps) |
| F10 input help: "Input help off" closes the layer; an unbound key says "No command" | `ai_layer.rs:818, 837` | equivalent |

## Open

None.
