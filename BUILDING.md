# BEAM, Rust

BEAM's application, in Rust and Tauri 2. It began as a port of a Python application,
which has since been removed from the repository (it is in git history before the
commit "Remove the Python application"). The plan, conventions and workstream status
are in `rust_plans/`; read `02-porting-conventions.md` before writing code.

All commands below are PowerShell, run from the repository root unless stated.

## Prerequisites

- Rust stable, MSVC toolchain (`stable-x86_64-pc-windows-msvc`), 1.85 or newer
  (edition 2024). Developed on 1.98.
- Visual Studio Build Tools with the C++ workload (the MSVC linker and the Windows SDK's
  resource compiler, which embeds the icon and manifest). The compiler must handle C++23:
  speech comes from the `prismer` crate, whose build script compiles Prism from source and
  links it into the exe.
- CMake 3.24 or newer on `PATH`, for that Prism build. The first build takes about a
  minute longer while Prism compiles; nothing is downloaded or shipped beside the exe.
- The WebView2 runtime (part of Windows 11).
- Node.js 18+ and npm, only for the Tauri CLI (`npm install` once).

## Layout

| Path | What |
|---|---|
| `crates/beam-base` | paths, config, logger, secret store, `pyfmt`, `hrtf_sets`, `route_beacon`, gear report, AI registry |
| `crates/beam-reports` | pure report and phrasing modules |
| `crates/beam-road` | road guidance, driving assistant, recorders |
| `crates/beam-speech` | Prism through the `prismer` crate (statically linked) and the speech layer |
| `crates/beam-audio` | synthesis, HRTF, audio controller, the output callback |
| `crates/beam-engine` | telemetry, listeners, keyboard layers, menus, MCP, updater: everything that is not the window; no Tauri dependency |
| `crates/beam-testutil` | dev-only: locate and load golden fixtures |
| `src-tauri` | the application crate `beam-app`, binary `beamtel.exe` |
| `ui` | the frontend: plain HTML, CSS, ES modules, no build step |
| `fixtures/data` | golden cases recorded from the Python application (committed, frozen) |
| `xtask` | build and packaging (not written yet: there is no release pipeline) |

Every module the plan assigns already exists as a file with a one-line `//!` comment,
declared in its crate's `lib.rs`; workstreams fill files, they do not edit `lib.rs`.
One target directory, `target/`, is shared by the whole workspace.

## Build, test, run

```powershell
cargo build --workspace            # debug, everything (~1 min cold)
cargo test --workspace             # all unit, fixture and sim tests
cargo test -p beam-base            # one crate
cargo clippy --workspace --all-targets
cargo fmt --all

cargo run -p beam-app              # opens the window (debug build keeps a console)
.\target\debug\beamtel.exe         # same, without cargo
```

`beam-app` enables Tauri's `custom-protocol` feature unconditionally, so every build,
debug or release, serves `ui/` embedded in the executable. There is no dev server
and no `devUrl`; after editing `ui/`, rebuild (`cargo build -p beam-app`) to see it.

### Release builds

```powershell
npm install                        # once: the Tauri CLI (@tauri-apps/cli 2.x)
npx tauri build                    # release exe; bundling is off (bundle.active = false)
# or, equivalently, without the CLI:
cargo build --release -p beam-app
```

Either writes `target\release\beamtel.exe` (~9 MB, ~1m45s cold) with the frontend
embedded; the exe runs on its own from any directory. Release builds have no console
window (`windows_subsystem = "windows"` under `not(debug_assertions)`).

### Administrator manifest (`uac-admin`)

The shipped app does **not** run elevated. A low-level keyboard hook sees and
suppresses keys going to a window at the same integrity level, and BeamNG runs
un-elevated when launched through Steam (verified in game, 2026-11-21: F9 then F read
fuel without the F reaching the game). Elevation only matters for someone who runs
Steam or BeamNG as administrator, because Windows hides an elevated window's keystrokes
from a normal process's hook. For them, the `uac-admin` cargo feature of `beam-app`
(**off by default**, releases included) embeds a manifest that requests elevation:

```powershell
cargo build --release -p beam-app --features uac-admin
npx tauri build --features uac-admin
```

`src-tauri/build.rs` picks `manifests/admin.xml` (Common Controls v6 plus
`requireAdministrator`) when the feature is on, `manifests/default.xml` (Common
Controls v6 only) otherwise. The release pipeline (`xtask`, workstream R1) builds
without it. Running an admin build raises a UAC prompt.

## Golden fixtures

`fixtures/data/<module>.json` holds cases recorded from the Python application's real
modules; a Rust test loads one with `beam_testutil::load` and compares. The generators
went with the Python, so the fixtures are now frozen: never edit one by hand. When
behaviour is meant to change, change the test's expectation in Rust (or replace the
fixture-driven test with a plain one) and say so in the commit.

Exact floats are stored as IEEE bits plus a readable repr (NaN, infinities and -0.0
survive); read them with `beam_testutil::f` and compare with `beam_testutil::same_f64`.

## Python-compatible helpers

Spoken numbers and config values still follow the Python application's formatting, so
`beam_base::pyfmt` keeps fixture-tested equivalents of Python's number handling:
`py_round`, `py_round0`, `try_py_int`, `py_float_str` (`str(x)`), `py_g` (`:g`/`%g`),
`py_f` (`:.Nf` including `nan`/`inf`), `py_mod` (float `%`), `py_float_parse`
(`float(s)`), `py_strip` / `py_is_space` (`str.strip()`). `beam_base::pyerr::ValueError`
stands in for a raised `ValueError` and keeps its message.

## Paths

`beam_base::paths`: `config_dir()` (`%LOCALAPPDATA%\beamtel`, home fallback),
`config_path()`, `program_dir()` (the exe's directory), `find_data_file(name)` (beside
the exe, else the repository root of a source checkout), and `hrtf_base_dir()`.
Speech needs no file at all: Prism is compiled into the exe.
