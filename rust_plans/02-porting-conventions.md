# Porting conventions

Every porting agent follows this file. It exists so that twenty agents produce one
codebase, and so that "parity" means the same thing to each of them.

## The rule

Port what the Python does, not what it looks like it meant. If the Python and its doc
disagree, the Python wins and the disagreement goes in your report. If you find a bug,
port it and report it. Do not fix, retune, reword, reorder or simplify behaviour.

## Reading before writing

1. Read the whole Python source you were assigned, not excerpts.
2. Read the matching entry in `docs/python-components.md` and any `docs/*.md` file the
   root `CLAUDE.md` table maps to your area.
3. Read the matching `diagnostic/*_sim.py`, if there is one. It is your test suite.

## Structure and naming

- One Rust module per Python module, same name. Functions keep their names in snake_case
  (a leading underscore is dropped; `_dock_phrase_ramp` becomes `dock_phrase_ramp`).
  Constants keep their exact names and values (`HYDRO_STEER_MAX_AZIMUTH_DEG`). Someone
  holding the Python doc must be able to grep the Rust for the same identifier.
- Keep a function's shape close to the original so the two read side by side. Idiomatic
  restructuring is welcome only where it cannot change behaviour.
- Carry the *why* comments across, shortened to what a maintainer needs. Do not carry
  narration. Do not write comments about the port itself ("ported from line 123").
- Spoken strings are byte-identical: wording, punctuation, capitalisation, spacing.
- Config keys, wire formats, file names, log file names and ports are byte-identical.
- No `unwrap()` on anything that comes from the network, the disk or the game. Python's
  listeners swallow malformed packets (`except Exception: continue`); the Rust must be as
  hard to kill. Reproduce what the Python does on bad input, which is usually "skip it".
- No `unsafe` outside FFI and Win32 calls. Each `unsafe` block is small and says what
  makes it sound.
- `cargo fmt` your files. `cargo clippy` clean where practical; do not contort code to
  satisfy a lint.

## Python-to-Rust parity traps

These produce code that compiles, looks right and speaks the wrong number.

| Python | What it does | Rust |
|---|---|---|
| `round(x)` | Rounds half to **even** | `x.round_ties_even()`, never `x.round()` |
| `round(x, n)` | Half to even on the scaled value, with float artefacts | Port via the helper `py_round(x, n)` in `beam_base::pyfmt`, which is fixture-tested |
| `int(x)` | Truncates toward zero | `x.trunc() as i64` (and mind NaN/inf: Python raises) |
| `x // y` | Floors | `(x / y).floor()` for floats, `div_euclid` for ints with positive divisor |
| `x % y` | Sign of the divisor | `rem_euclid` for a positive divisor; write it out otherwise |
| `f"{x:.1f}"` | Correctly rounded decimal | `format!("{:.1}", x)` agrees for finite values; `-0.0` prints `-0.0` in both |
| `str(x)` / `f"{x}"` on a float | Shortest repr, always with `.0` for integral values, `1e-05` style exponents | `beam_base::pyfmt::py_float_str(x)`. Rust's `{}` prints `2` for `2.0` |
| `f"{x:g}"`, `%g`, `%d`, `%s` | printf rules | Use the helpers in `pyfmt`; add one (with fixtures) if yours is missing |
| `str.split()` (no argument) | Splits on runs of whitespace, drops empties | `split_whitespace()` |
| `str.split(",")` | Keeps empties | `split(',')` |
| `s.strip()` | Unicode whitespace | `trim()` |
| `text.split("|", n)` | At most `n` splits | `splitn(n + 1, '|')` |
| `float("nan")`, comparisons | NaN propagates quietly through `min`/`max` in ways that depend on argument order | Reproduce the order: Python `max(a, b)` returns `a` when the comparison is false |
| `min`/`max`/`sorted` on ties | First wins; sort is stable | `sort_by` is stable; write `min`/`max` folds that keep the first |
| dict iteration | Insertion order | `IndexMap` or `Vec` of pairs wherever order is observable (menus, JSON output) |
| `json.dump(cfg, indent=2, ensure_ascii=False)` | Two-space indent, `": "`, floats keep `.0` | `serde_json` pretty printer with `preserve_order`; fixture-test the bytes |
| `bool(x)` on config values | Truthiness (`"false"` is true, `0` is false, `""` is false) | Port the coercion, not a typed parse |
| `time.time()` vs `time.monotonic()` | Different clocks, used deliberately | Keep the distinction: `SystemTime` vs `Instant` |
| numpy `float32` vs Python `float` | Different precision; the dtype of a buffer is decided by how it was built | Match the dtype of each array. Phrase and state logic is `f64`. Audio buffers follow the Python's dtype at that point (check, do not assume) |
| `np.linspace(a, b, n)` | Endpoint included | Write the helper once in `beam-audio`, test it |
| `np.convolve(x, h)` mode | `full` by default | Match the mode at each call site |
| numpy RNG (`default_rng(seed)`) | PCG64 stream | Not reproduced. See "Noise" below |
| Integer overflow | None | Use `i64`/`u64`, or checked maths where a wire value is untrusted |

### What the scaffold established (read `rust/README.md` too)

- `beam_base::pyfmt` already has `py_round`, `py_round0`, `try_py_int`, `py_float_str`,
  `py_g`, `py_f`, `py_mod`, `py_float_parse`, `py_strip`, each with `try_` variants where
  Python raises. Use them; add to them (with fixtures) rather than writing a local copy.
- Float `%` is `py_mod`, not `rem_euclid`: Python gives `-0.0 % 360 == 0.0`.
- `float(text)` on a wire field is `py_float_parse` (underscores, inf/nan, Python's
  whitespace rules).
- Where Python raises `ValueError`, return `beam_base::pyerr::ValueError` with the same
  message, and let the caller do what the Python's `except` did.
- Fixtures store floats as `{"bits", "repr"}` (`_common.fbits`); read with
  `beam_testutil::f`, compare with `same_f64`. `beam_testutil::repo_root()` finds the
  Python and Lua sources for sims that read them.
- `beam-base` cannot depend on `beam-road`; `route_beacon` keeps a private, fixture-checked
  copy of `direction_label`. `beam_road::road_guidance::direction_label` is canonical.
- The app package is `beam-app` (binary `beamtel`); `ui/` is embedded at build time, so a
  UI change needs a rebuild. Tauri is pinned to 2.x (crates.io also lists a 3.0 alpha).
- Crate versions are in `rust/Cargo.toml`; note `ureq` is 3.x and `zip` is deflate-only.

### Noise

**Superseded:** `beam_audio::dsp::default_rng` reproduces numpy's `default_rng` stream bit for bit (PCG64, uniform, integers, standard_normal). Use it with the Python's seed and compare sample for sample. The paragraph below applies only where the Python generator is unseeded.


A few waveforms are built from seeded numpy noise (the articulation centre click, the
slide voice). numpy's generator is not reproduced. The Rust uses its own fixed-seed
generator and the tests assert what the Python docs say matters about the sound (band
limits, RMS and peak level, length, envelope) rather than sample equality. Everything
deterministic is compared sample by sample within a stated tolerance.

### Text encodings on the wire

Python decodes most datagrams with `errors="ignore"` or `"replace"`. Check the call site
and use `String::from_utf8_lossy` or a strict decode accordingly. The difference decides
whether a packet with one bad byte is used or dropped.

## Golden fixtures

- Generator: `rust/fixtures/gen/<module>.py`. It imports the real Python module from the
  repository root, runs a table of inputs, and writes `rust/fixtures/data/<module>.json`.
  Run it with the project's interpreter: `uv run python rust/fixtures/gen/<module>.py`.
- Fixtures are committed. A generator must be deterministic so regenerating is a no-op.
- Choose inputs that cross every branch: both unit systems, sentinels (`-1`, `-999`,
  `~`, `NA`), empty and malformed packets, boundary values either side of every threshold
  constant, negative zero, very large values.
- The Rust test names the fixture case in its failure message.
- Never edit a fixture by hand to make a test pass.

## Simulators

Each `diagnostic/<name>_sim.py` becomes `rust/crates/<crate>/tests/<name>_sim.rs` with the
same scenario numbers and names in the same order, so a doc that says "scenario 17
asserts X" still points at something. Where a sim reads constants out of the Lua mod
(`terrain_scan_sim.py` reads the ring count), the Rust test reads the same file.

## Working alongside other agents

- You own the files listed in your brief. Do not edit a file you do not own. If you need
  something from another agent's module, call the function by the name this document's
  naming rule gives it and say so in your report.
- Do not edit any Python, Lua or JS source, any `docs/` file, or anything outside `rust/`
  unless your brief says so.
- **Never run a git command that changes anything** (no commit, add, stash, checkout,
  reset, branch, merge, push, worktree). `git status`, `git diff` and `git log` are fine.
- Shell commands are PowerShell. No `&&`.
- If `cargo check` fails in a file that is not yours, that is another agent mid-edit.
  Check your own crate or module, wait and retry; do not "fix" their file.
- Do not add a workspace dependency without need. If you do, add it to your own crate's
  `Cargo.toml` and name it in your report.

## What your final report must contain

1. The files you wrote.
2. What you ran and its real result (test counts, failures quoted).
3. Every place the Rust knowingly differs from the Python, and why.
4. Everything you did not finish, as a list of concrete items. A stub is not finished.
5. Anything in the Python that looked like a bug, or that contradicted its doc.

An honest "these three functions are stubbed" is worth more than a report that says done.
