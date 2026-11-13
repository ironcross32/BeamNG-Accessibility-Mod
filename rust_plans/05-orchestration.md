# Orchestration

## Roles

- **Orchestrator** (the main session): owns these plans, briefs agents, reads their
  reports, updates the status table in `README.md`, commits. Writes little code itself.
- **Porting agents**: Opus subagents, one per workstream in `04-workstreams.md`.

## Waves

| Wave | Runs | Gate to the next wave |
|---|---|---|
| 0 | W0 alone | `cargo build --workspace` and `cargo test --workspace` pass; the Tauri window opens |
| 1 | B1, B2, B3, S1, AH, AA, EA, U1 in parallel | Each crate's own tests pass; `audio-callback-split.md`, `engine-map.md`, `CONTRACT.md` and `CONTROL-INVENTORY.md` exist |
| 2 | A1-A3, E1-E10, U2 in parallel | Each agent's own tests pass; reports list their stubs |
| 3 | I1, then the V1 audits in parallel, then fix-up agents for what the audits list | Workspace builds, all tests pass, audit lists are empty or accepted |
| 4 | R1, D1, T1 | A release bundle builds locally |
| - | UV | The user signs off |

Within a wave, agents share one working tree and one cargo target directory. They are
kept apart by file ownership (each brief lists the files) and by crate boundaries. No git
worktrees: creating one creates a branch, which is not allowed.

`beam-audio` and `beam-engine` each have several agents in wave 2. That is why wave 1
puts an architect in each (AA, EA) who stubs every module and signature first: wave 2
agents then fill bodies in their own files and never need to edit a shared one.

## Briefing an agent

Each brief is self-contained, because the agent starts with none of this context:

1. One paragraph on the project and the port, and the instruction to read
   `rust_plans/01-scope-and-architecture.md` and `rust_plans/02-porting-conventions.md`
   first, then the root `CLAUDE.md`.
2. The workstream entry from `04-workstreams.md`, verbatim, plus the Python files and
   `docs/` files to read in full.
3. The exact Rust files it owns.
4. What "done" is and how to check it (the commands to run).
5. The constraints: PowerShell syntax; no git commands that change anything; no edits
   outside its files; no behaviour changes.
6. The report format from the conventions file.

## Commits

The orchestrator commits on `rust-port` at the end of each wave, and after any large
standalone piece. Only `rust/` and `rust_plans/` are staged, by explicit path. The
working tree carries unrelated uncommitted Python and Lua changes that predate the port;
they are never staged by the port's commits. Nothing is pushed.

## Resuming after a model or session switch

1. Read `rust_plans/README.md` and its status table.
2. `git log --oneline -15` on `rust-port` to see which waves are committed.
3. Read whichever of `audio-callback-split.md`, `engine-map.md`, `integration-report.md`
   and `audit-*.md` exist; they hold the detail that was decided after this plan.
4. In `rust/`: `cargo build --workspace`, then `cargo test --workspace`. What fails is
   where the work stopped.
5. Continue with the first workstream in the status table that is not "done".

## What needs the user

Automated checks cannot reach these. They are collected for one validation pass rather
than asked one at a time:

1. **Screen reader pass over the interface** (NVDA and, if available, JAWS): tab order,
   group and heading announcements, the tab strip, spin controls, the View Logs menu, the
   API key dialog, message boxes, the update prompt.
2. **In game**: speech through the chosen backend; every F9 and F10 command; controller
   menu; the audio cues by ear against the Python build with the same config (compass,
   scanner, obstacle, road guidance, implement and docking tones, terrain scan, gear
   tones, slip voices); the vehicle spawner; the AI describer.
3. **Elevation and hooks**: the keyboard layers work with the game focused; arrow capture
   releases properly.
4. **Updater**: a Python build updating into a Rust build, and a Rust build updating
   itself. This needs a real release, so it happens after the user decides to publish.
5. **Decisions held for the user**:
   - Should control help text (the wx tooltips) be announced by the screen reader, or
     stay hover-only as it was? Default: hover-only.
   - Cutover: when the Python sources are retired, and whether the developer tooling
     ports (T1) are wanted at all.
   - Any Python bug the agents reported: port as is (the default) or fix.

## Risks being watched

| Risk | Mitigation |
|---|---|
| The audio callback misses its deadline | No allocation or locking beyond what the Python did; pre-rendered buffers stay pre-rendered; measured in a release build |
| WebView2 behaviour under an elevated process | Checked in wave 0 with the admin feature on |
| A low-level keyboard hook that blocks stalls all input system-wide | The hook only classifies and enqueues; the timeout reasoning at `beamtel.py:5066` is carried over; tested with the game focused during UV |
| Prism's C API differs from what the wheel's cffi layer suggests | S1 derives it from the pinned wheel and tests on this machine |
| Silent drift between twenty agents' ideas of the shared state | EA writes the state struct and every signature before wave 2 starts |
| An agent reports "done" over stubs | The report format demands the stub list; I1 greps for `todo!`/`unimplemented!`; the V1 audits are done by non-authors |
| Rounding and float formatting differences change spoken numbers | `pyfmt` helpers with Python-generated fixtures; conventions table |
