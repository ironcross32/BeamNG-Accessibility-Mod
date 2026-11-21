# Audit: UI bridge and MCP server (V1, slice 7)

2026-11-21. Python: `nvda_ws_speaker.py`, `mcp_server.py`. Rust:
`beam-engine/src/ui_bridge.rs`, `beam-engine/src/mcp_server/`. Audited by the
orchestrator, which wrote `mcp_server/camera.rs` and E9's tests after the agent died, but
not the rest. E9's report was lost, so this audit also recorded the Python bugs E9 had
found.

## Why this slice needed new fixtures

Under every existing generator, both Python modules ran at 0-12% (the camera chunks
aside). The Rust tests checked behaviour the Python sim describes, not the Python's
answers to the same inputs. Two fixtures were added, both driving the real modules.

### `bridge` (`fixtures/gen/bridge.py`, `tests/bridge_fixture.rs`)

- **Messages**: 6 sequences, about 50 messages, through the real `handle_ws_message`,
  with a recorder for every callback and the speech function. Compared after each
  message: the calls (with their coerced arguments), every log record (level and text),
  the exception raised, and the debug flag. Covers speak (dedup window, interrupt,
  non-strings, CSS-like text), log levels, debug state, every callback type, the
  six-action allowlist, unknown and non-dict messages.
- **TCP relay**: 4 runs of the real `tcp_handler` over an asyncio StreamReader with a
  recording writer: the transport pong, skipped bad lines with their warning text, and
  the exceptions that end the connection.
- **Result: equivalent.** One kept difference, removed from the fixture: a `log` message
  at level `critical` is written as `[CRITICAL]` by Python and `[ERROR]` by Rust (the
  `log` crate has no critical level). `bnvdaRuntime.js` only ever sends info, warn and
  error.

### `mcp_dispatch` (`fixtures/gen/mcp_dispatch.py`, `tests/mcp_dispatch_fixture.rs`)

- **68 JSON-RPC messages** through the real `_dispatch` with stub deps: the handshake
  (protocol echo and fallback), ping, the full `tools/list` (every description and
  input schema), notifications, unknown methods and tools, malformed requests, and
  calls to every tool except `screenshot`, with valid and invalid arguments, with and
  without a live world.
- The console sender answers synchronously through the real `console_tap` (the capture
  is armed before the command is sent), so `lua_exec`, `lua_contexts`, `health`, `diag`,
  `vehicle_control`, `reset_test_state` and every `camera_control` action are covered
  with scripted replies and no waiting.
- Compared: the whole response (tool text byte-exact after zeroing `elapsed_ms`,
  `mcp_uptime_s` and log timestamps), or the exception raised, and every call made on
  the deps.
- `screenshot` is left out: Python's "injected no capture function" state cannot exist
  in Rust, whose Deps require a capture function. `mcp_server_sim` covers the real path.
- **Result: equivalent after one fix** (below).

## Fixed

- **`speak` with a non-string `text`** (`mcp_server/tools.rs`). The Python hands the raw
  value to `say()`, which normalises it: a number is spoken as its text, a one-item list
  or dict as its item, anything else not at all. The Rust stringified it, so `["a"]` was
  spoken as `['a']`. `t_speak` now runs the value through
  `speech_out::normalize_speech_value` first. The fixture's say stub uses beamtel's own
  normaliser, and five `speak` shapes were added (float, bool, one-item list, longer
  list, dict).

## Walked by hand

| Python | Rust | State |
|---|---|---|
| `hover_on`, `hover_cancel` (scheduled on the bridge loop) | `ui_bridge.rs` | equivalent, including bug 65 |
| `_origin_ok` (no or empty Origin allowed; otherwise `http://127.0.0.1` / `http://localhost` prefix) | `origin_ok` | equivalent, including bug 66 |
| an exception out of `_dispatch` | `internal_error`: aiohttp's 500 and its body text | equivalent |
| HTTP transport (202 for a notification, 405 for GET, batch, health) | `mcp_server_sim` over real HTTP | equivalent |

## Python bugs

- **64 (found by E9, confirmed by the `bridge` fixture)**: the TCP relay ends its
  connection on a JSON line that is not an object, on an unhashable action, or on a log
  level naming a non-callable logger attribute. Dormant: the mod never sends these.
- **65 (found by E9)**: `hover_on` clears its own token, so hover speech never fires.
  Dormant: the mod only sends `hover_cancel`.
- **66 (new, security)**: the MCP origin guard is a prefix match, so
  `http://localhost.example.com` passes, which opens it to DNS rebinding whenever the
  server is enabled. **Recommended fix**: compare the parsed host exactly. Raised with
  the user.

## Open

The decisions on bugs 64-66, 66 above all.
