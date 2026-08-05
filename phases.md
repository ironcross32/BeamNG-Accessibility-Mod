# BEAM adversarial review — remediation phases

Findings from the full-codebase adversarial review (Python side plus the Lua
mod reached through the `bng_mod/` junction), grouped into phases and worked in
order. Checked items have landed on `master`; the commit is noted beside each.

---

## [x] Phase 1 — Delete verified-dead AngularJS

BeamNG 0.39 ported much of the interface to Vue, stranding whole regions of
`bnvdaRuntime.js`. Every deletion had zero callers, confirmed by grep.

- [x] `bnvdaRuntime.js` Angular options-screen speech (`optionsObserverAttached`,
      `optionsInterval`, `extractLabelFromElement`, `speakOptionRow`,
      `attachOptionsObserver`)
- [x] Remaining verified-dead Angular regions and their helpers

---

## [x] Phase 2 — Radial menu

- [x] Rewrite radial speech against `RadialCenterCanvas.prototype.setState` —
      the centre label is painted to a `<canvas>`, so the old `foreignObject`
      DOM scrape could never have worked (`2a69d19`)
- [x] Suppress the stalled-engine message spam and throttle repeated game
      messages by category, mirroring `messagesStore.js` (`37a1830`)

---

## [x] Phase 3 — Crashes, hangs and data loss

- [x] Guard `core_terrain.getTerrainHeight` returning nil in
      `obstacleDetector.lua`; the GE `onUpdate` chain is dispatched without
      `pcall`, so one throw kills every extension loaded after it (`a26c05d`)
- [x] `pcall` `map.findBestRoad` in `roadDetector.lua`, which indexes the edge
      kd-tree with no nil check (`a26c05d`)
- [x] Move keyboard command handling off the low-level hook callback, so
      handlers cannot trip Windows' ~300 ms `LowLevelHooksTimeout` (`04ac928`)
- [x] Fix the audio device-switch deadlock: `Pa_StopStream` blocks on the
      callback, which was waiting on the same lock (`2beee0d`)
- [x] Make config writes atomic and stop treating a transient read failure as
      corruption, which was resetting user settings to defaults (`2beee0d`)

---

## [ ] Phase 4 — Silently dead features

Code that looks implemented but never runs.

- [ ] `_vehicle_selector_open` NameError — read at `beamtel.py:2118`, never
      defined anywhere; every arrow key in status mode raises before reaching
      the metric cycling. **Confirmed live.**
- [ ] `stop_speech()` calls `inst.stop()`, but the wrapper's method is
      `stop_speech()` (`sral.py:38`) and there is no `stop`. Swallowed by
      `except Exception: pass`, so speech is never actually interrupted for
      loading screens. **Confirmed live.**
- [ ] `ai.getMode()` / `getTargetObjectID()` — AI status queries against
      methods that do not exist on the vehicle AI object
- [ ] Hover token bug in `bnvdaRuntime.js` suppressing announcements it should
      make
- [ ] SAPI fallback is documented and configurable but never wired up —
      implement it or drop the config keys
- [ ] `translateLanguage` → `_tr` rename not followed at the call site
      (not located by a first grep; needs a proper search)
- [ ] Vehicle-spawner filter fields moved into `info.aggregates`, so the
      filters match nothing

---

## [ ] Phase 5 — Wrong behaviour

Features that run, and do the wrong thing.

- [x] Approach-side callout inverted — `approachDeg > 0` means the player is
      off the target's *left* (`c8743ac`)
- [x] The misleading "positive = right" bearing comments and the
      `...RightVec` locals that are actually left vectors (`c8743ac`)
- [x] `safeTeleport` 5th argument — that slot is `visibilityPoint` and must be
      a vec3; the boolean threw inside `spawn.lua` (`5d39c29`)
- [x] `replaceVehicle` signature, and treating `pcall` success as call success
      — it returns nil on failure without raising (`5d39c29`)
- [x] Clickspot firing *all* actions rather than the selected one, plus
      non-deterministic action selection via `pairs()` (`5ee836b`)
- [x] SNAP viewport / DPI coordinates — client-area origin and thread-scoped
      DPI awareness; window lookup now matches the render window class
      (`a892f71`)
- [ ] **Audio gain structure and the convolutions performed in the callback —
      deliberately not changed; see below.**

### Why the audio item was left alone

Measured rather than assumed. Worst case is 8 × 512-tap `np.convolve` per
callback (4 HRTF sources × 2 ears): **0.72 ms at 256 frames, 1.49 ms at 512,
3.02 ms at 1024 — a flat ~14% of the callback deadline** at every block size.
Real, but no cliff, and it only reaches that with four simultaneous spatial
sources.

The gain structure is a genuine weakness: every source does `buf += signal *
gain` and the only protection is one hard `np.clip` at the very end, so enough
simultaneous cues will clip and distort. Fixing it properly means a master gain
or limiter, which changes the loudness of every cue relative to the levels
already tuned in the config.

Both changes land in the audio callback, the most timing-critical code in the
project, and neither is a correctness bug. Worth doing as a considered piece of
work with the levels decided deliberately — not folded into a bug-fix pass.

---

## [ ] Phase 6 — wx accessibility

- [ ] `StaticBox` parenting across 9 groups
- [ ] 5 `SpinCtrl`s missing the composite-control fix
- [ ] `Show` → `ShowItems`
- [ ] Save feedback
- [ ] Focus handling when a control is disabled
- [ ] Reset confirmation
- [ ] Duplicated model name
- [ ] Mnemonics

---

## [ ] Phase 7 — Hygiene

- [ ] Drop the unused `asyncio` dependency
- [ ] Build freshness / staleness checking
- [ ] `SO_REUSEADDR` on the UDP sockets
- [ ] CSV escaping in the Lua text protocols
- [ ] CLAUDE.md corrections — Extended telemetry is 164 bytes
      (`<H4sBx9fII28f`), not 132; OutGauge is 96; the obstacle detector does
      distance clustering rather than "4 quadrant slots", and casts 8 rays per
      tick, not 4; the UI section is stale

---

## Not assigned to a phase

- [ ] Missing BeamNG UI click sound — game-side `bng-sound-class`, not driven
      by the mod
- [ ] Possible sluggishness from `startVueFocusWatcher`'s 120 ms poll doing 8+
      multi-selector subtree scans per tick

## Awaiting in-game verification

- [ ] `2beee0d` — audio returns after an output device change; settings survive
      a save from `configurator.py` while `beamtel.py` is running
- [ ] `c8743ac` — scanner left/right and approach side against a deliberately
      placed target
