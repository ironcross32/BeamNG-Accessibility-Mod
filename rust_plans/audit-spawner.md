# Audit: vehicle spawner (V1, slice 6)

2026-11-21. Python: `vehicle_spawner.py` (2922 lines). Rust:
`beam-engine/src/vehicle_spawner/` (`mod.rs`, `screens.rs`, `arrange.rs`, `place3d.rs`,
`pyjson.rs`). Audited by the orchestrator, which committed E8's work after the agent
died but did not write it. E8's report was lost, so this audit also looked for the
Python bugs that report would have listed.

## Method

1. Coverage of the real Python under every fixture generator (method in
   `audit-listeners.md`). The scripts now match a module by its exact path: the first
   pass attributed the spawner's coverage to its generator, which has the same file name.
2. Every literal string the Python spawner speaks (`_say_safe`, `_say_with_prefix`,
   `_say`, `say`) checked against the Rust sources.
3. Hand walk of the plumbing no generator reaches.
4. The Rust comments that describe Python behaviour, read for reproduced bugs.
5. In game (user, 2026-11-21): F11 hotkey installed with suppression, listener bound on
   4460, the 126-entry catalog received.

## Coverage of the real Python

Almost every function runs at 88-100% under E8's fixture: `_handle_packet` 98%, the
arrangement maths 99%, `_speak_current` 88%, `_on_enter` 95%, every arrow, Tab, Home,
End, Escape, Space, Delete, W, X, G, A and Info handler 84-100%, the 3D placement
78-100%. Low: `_send_cmd` 36%, `_listener_loop` 3%, `_worker_loop` 10%, the modal hook
install and removal 56-62%, `start` 8% (all plumbing).

The behaviour lines that remain unrun are messages for empty states ("No vehicles in
slots.", "Nothing to spawn.", "No configurations available."); **all 49 literal spoken
strings are in the Rust**.

## Walked by hand

| Python | Rust | State |
|---|---|---|
| `_listener_loop` (1 MB receive buffer, 127.0.0.1:4460, 64 KB reads, UTF-8 with replacement, stripped; socket errors continue; an exception out of `_handle_packet` ends the thread) | `listener_loop`, `handle_datagram` | equivalent, including the thread ending (bug 63) |
| `_worker_loop` ("vehicle_spawner: handler error: ...") | `worker_loop` | equivalent |
| `_send_cmd` (one reused socket, recreated after a failure) | `send_cmd` | equivalent |
| `_install_modal_hooks` / `_uninstall_modal_hooks` (plus the `keyboard` package aliasing clean-up) | `mod.rs` | equivalent; the aliasing clean-up is for a `keyboard` package bug the Rust listener does not have |
| `start` (listener and worker threads, F11 with suppression, `REQUEST_CATALOG`) | `start` | equivalent, seen in game |

## Python bugs

- **63 (new, found by E8, recorded here)**: a `CATALOG_ITEM:` whose JSON parses but is
  not an object raises AttributeError outside any `try`, ending the spawner listener for
  the session. Confirmed against the real module (`[]`, `5` and `"x"` all raise). E8's
  fixture `catalog_errors` pins the Rust to it. Reproduced (R).

## Open

None besides bug 63's decision.
