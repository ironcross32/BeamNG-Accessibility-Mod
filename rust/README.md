# BEAM, Rust port

The Rust and Tauri 2 port of BEAM's Python application. The plan, conventions and
workstream status are in `../rust_plans/`; read `02-porting-conventions.md` before
writing any code here. The Python in the repository root is the reference
implementation and stays untouched until the port is validated.

All commands below are PowerShell, run from this `rust/` directory unless stated.

## Prerequisites

- Rust stable, MSVC toolchain (`stable-x86_64-pc-windows-msvc`), 1.85 or newer
  (edition 2024). Developed on 1.98.
- Visual Studio Build Tools with the C++ workload (the MSVC linker and the Windows SDK's
  resource compiler, which embeds the icon and manifest).
- The WebView2 runtime (part of Windows 11).
- Node.js 18+ and npm, only for the Tauri CLI (`npm install` here once).
- For regenerating fixtures: `uv` and the project's virtualenv (`uv sync` in the
  repository root).

## Layout

| Path | What |
|---|---|
| `crates/beam-base` | paths, config, logger, secret store, `pyfmt`, `hrtf_sets`, `route_beacon`, gear report, AI registry |
| `crates/beam-reports` | pure report and phrasing modules |
| `crates/beam-road` | road guidance, driving assistant, recorders |
| `crates/beam-speech` | Prism (`prism.dll`) FFI and the speech layer |
| `crates/beam-audio` | synthesis, HRTF, audio controller, the output callback |
| `crates/beam-engine` | everything in `beamtel.py` that is not the window; no Tauri dependency |
| `crates/beam-testutil` | dev-only: locate and load golden fixtures |
| `src-tauri` | the application crate `beam-app`, binary `beamtel.exe` |
| `ui` | the frontend: plain HTML, CSS, ES modules, no build step |
| `fixtures/gen` | Python scripts that write golden cases from the reference modules |
| `fixtures/data` | the golden cases (committed) |
| `xtask` | build and packaging (port of `build.py`) |

Every module the plan assigns already exists as a file with a one-line `//!` comment,
declared in its crate's `lib.rs`; workstreams fill files, they do not edit `lib.rs`.
One target directory, `rust/target/`, is shared by the whole workspace.

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
debug or release, serves `rust/ui/` embedded in the executable. There is no dev server
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

The shipped app must run elevated, because the low-level keyboard hooks have to work
while the (elevated) game has focus. The manifest that requests it is behind the
`uac-admin` cargo feature of `beam-app`, **off by default**, so `cargo run` and the
tests work from an ordinary shell:

```powershell
cargo build --release -p beam-app --features uac-admin
npx tauri build --features uac-admin
```

`src-tauri/build.rs` picks `manifests/admin.xml` (Common Controls v6 plus
`requireAdministrator`) when the feature is on, `manifests/default.xml` (Common
Controls v6 only) otherwise. The release pipeline (`xtask`, workstream R1) must pass
`--features uac-admin`. Running an admin build raises a UAC prompt.

## Golden fixtures

A generator in `fixtures/gen/<module>.py` imports the real Python module and writes
`fixtures/data/<module>.json`; a Rust test loads it with `beam_testutil::load` and
compares. Run generators from the **repository root** with the project interpreter:

```powershell
uv run python rust/fixtures/gen/pyfmt.py
uv run python rust/fixtures/gen/route_beacon.py
uv run python rust/fixtures/gen/hrtf_sets.py
```

Rules (details in `rust_plans/02-porting-conventions.md`):

- Start every generator with `from _common import ...`; that puts the repository root
  on `sys.path`.
- Write output with `_common.write_fixture(name, obj)`: sorted keys, `\n` line endings,
  so regenerating an unchanged fixture is a no-op. Put anything whose order matters in
  a list.
- Floats that must compare exactly go through `_common.fbits(x)` (IEEE bits plus a
  human-readable repr; NaN, infinities and -0.0 survive). Read them with
  `beam_testutil::f`; compare with `beam_testutil::same_f64`.
- `_common.outcome(fn, *args)` records `{"ok": value}` or `{"err": type, "msg": str}`.
- Fixtures are committed and never edited by hand.

## Python parity helpers

`beam_base::pyfmt` holds the fixture-tested replacements for Python's number handling:
`py_round`, `py_round0`, `try_py_int`, `py_float_str` (`str(x)`), `py_g` (`:g`/`%g`),
`py_f` (`:.Nf` including `nan`/`inf`), `py_mod` (float `%`), `py_float_parse`
(`float(s)`), `py_strip` / `py_is_space` (`str.strip()`). `beam_base::pyerr::ValueError`
stands in for a raised `ValueError` and keeps its message.

## Paths

`beam_base::paths`: `config_dir()` (`%LOCALAPPDATA%\beamtel`, home fallback),
`config_path()`, `program_dir()` (the exe's directory), `find_data_file(name)` (beside
the exe, else the repository root of a source checkout), `hrtf_base_dir()`, and
`find_prism_dll()` (beside the exe, else `.venv\Lib\site-packages\prism\_native`).
