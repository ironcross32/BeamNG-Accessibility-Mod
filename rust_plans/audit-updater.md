# Audit: updater and mod installer (V1, slice 8)

2026-11-21. Python: `updater.py`, and the installer half of `config_ui.py`
(`install_mod_interactive`, the layout clean-up, the stamped version comparison). Rust:
`beam-engine/src/updater.rs`, `beam-engine/src/mod_install.rs`. Audited by the
orchestrator, which committed E10's work after the agent died but did not write it.
E10's report was lost, so this audit also recorded the Python bugs E10 had found.

## Method

1. Coverage of the real Python under every fixture generator (method in
   `audit-listeners.md`).
2. Hand walk of what the fixture does not run, and every message on an unrun path
   checked word for word.
3. The Rust comments describing exceptions the Python does not catch, read for
   reproduced bugs.
4. In game (user, 2026-11-21): the startup check ran ("Up to date: running 0.2.1,
   latest release is v0.2.1") and released the launch gate.

## Coverage of the real Python

| Function | Statements run |
|---|---|
| `check_latest`, `download`, `stage`, `write_helper` | 100% |
| `run_startup_flow`, `_phase_two`, `_check_and_offer`, `apply_and_restart` | 86-92% |
| `_run_with_progress` | 3% (the wx progress dialog; walked by hand) |
| `install_mod_interactive` | 90% |
| `_load_bnvda_layout`, `_remove_bnvda_layout_entries`, `_mod_zip_version` | 100% |
| `_write_layout_with_backup`, `_same_bytes` | 82-85% |

## Walked by hand

| Python | Rust | State |
|---|---|---|
| `_run_with_progress`: work on a thread; every 50 ms an update with "N of M MB" and `min(100, int(done*100/total))` once the total is known, otherwise a pulse; Cancel sets the flag; "cancelled" when nothing worse was reported | `run_with_progress`, through `UiSink::progress_update` / `progress_close` (the `update-progress` events) | equivalent. Python's 2 s join timeout is never reached (its loop only ends when the thread has) |
| `apply_and_restart` "Could not write the update helper: ..." | `updater.rs:1266` | equivalent |
| `_check_and_offer` "Could not record the pending update: ..." | `updater.rs:1651` | equivalent |
| installer "Failed to copy mod file" box; "Layout Cleanup Failed" box | `mod_install.rs:416, 491` | equivalent, word for word |
| `program_dir` (frozen vs source) | `beam_base::paths::program_dir` | equivalent for the built exe |

## Python bugs

- **67 (found by E10, recorded here)**: the update helper `.cmd` is written as ASCII,
  and its paths include the user profile. Any non-ASCII character in the user name or
  install path raises an exception `apply_and_restart` does not catch. By then the
  pending flag is written and the game launch denied, so the user is left with no game,
  no message and an update that never applies. Reproduced; **recommended for fixing**.
- Not recorded as a bug: an `assets` value in the GitHub response that cannot be
  iterated raises out of `check_latest`. GitHub always sends a list, and beamtel's outer
  handler would catch it and release the launch.

## Open

None. **67 is fixed in the Rust** (user, 2026-11-21): the helper reads its paths from the environment, verified with a real `cmd` on a `Zoë 漢字 50%` folder; `e10_updater` `non_ascii_paths_reach_the_helper`.
