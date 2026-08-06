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
- [x] `stop_speech()` calls `inst.stop()`, but the wrapper's method is
      `stop_speech()` (`sral.py:38`) and there is no `stop`. Swallowed by
      `except Exception: pass`, so speech is never actually interrupted for
      loading screens. **Confirmed live.** Fixed by the Prism migration —
      `speech.stop()` maps onto Prism's `Backend.stop()`, which is the real
      method name.
- [ ] `ai.getMode()` / `getTargetObjectID()` — AI status queries against
      methods that do not exist on the vehicle AI object
- [ ] Hover token bug in `bnvdaRuntime.js` suppressing announcements it should
      make
- [x] SAPI fallback is documented and configurable but never wired up —
      implement it or drop the config keys. Implemented: the SAPI-only panel
      is now a Prism backend picker (`speech_backend`/`speech_voice_name`/
      `speech_rate`/`speech_volume`), beamtel applies those settings through
      `speech.py`, and controls grey out per the backend's capability bits.
      Old `force_sapi`/`sapi_*` keys migrate on load.
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

## [x] Phase 6 — wx accessibility

- [x] `StaticBox` parenting — 13 groups, not 9: the 9 in `ConfigPanel`, 3 in
      `AIDescriberPanel` and the Developer Console in `beamtel.py`. Every one
      parented its children to the panel, leaving the box a *sibling* of the
      controls, so MSAA/UIA never nested them and no group name was ever
      announced. New `_group()` helper in `config_ui.py`; the `AIDescriberPanel`
      and console groups also had to move off the one-step `StaticBoxSizer`
      form, which keeps no box handle. Tab order is unchanged — verified 37
      leaves on `ConfigPanel` and 14 on the Main tab, in the original order,
      because `_focusable_leaves` recurses and descends into the box.
- [x] 5 `SpinCtrl`s missing the composite-control fix — `_label_spin_double`
      generalised to `_label_spin` (its body was always type-agnostic) and
      applied to `spin_rate`, `spin_volume`, `spin_compass_interval`,
      `spin_compass_highlight_nth`, `spin_scanner_base_freq`. The highlight
      spin's unit lived in an orphaned StaticText, so it is now folded into the
      name ("Highlight every N clicks").
- [x] `Show` → `ShowItems` — `on_toggle_highlight` called `sizer.Show(bool)`,
      which binds to the `Show(index, …)` overload: `Show(True)` meant "show
      item 1" and `Show(False)` "show item 0", so **neither branch ever hid
      anything**. The row was permanently visible with only `Enable()` taking
      effect. Now `ShowItems`, against a stored `self._highlight_row` rather
      than a `GetContainingSizer()` lookup. Verified hiding both ways.
- [~] Save feedback — **deliberately skipped.** Both panels save on every
      parameter change rather than behind a button, so there is no save action
      for the user to have confirmed.
- [x] Focus handling when a control is disabled — new `_enable()` helper moves
      focus to the governing checkbox before disabling a control that holds it.
      Applied to all six conditional-disable sites. The live bug was
      `AIDescriberPanel._set_api_key`, which disabled the button the user had
      just pressed for the duration of an async validation and never restored
      focus; `_on_validated` now calls `SetFocus()`, so the new "Clear API key"
      label is what gets announced.
- [x] Reset confirmation — `on_reset` wiped every setting with no prompt, and
      the wipe reached disk 2 s later on its own. Now confirms with `NO_DEFAULT`
      and acknowledges on success, matching `_clear_api_key`.
- [x] Duplicated model name — both cases. Two buttons both labelled "Re&fresh"
      (speech and audio device) are now "Refresh Voices" / "Refresh Devices".
      The AI Describer combo had three overlapping name sources (group box
      "Model" + label "&Vision model:" + `SetName`); the group is now "Vision
      Model", the label "Model:", and the `SetName` is gone.
- [x] Mnemonics — removed entirely. ~37 accelerators over ~20 letters (O and T
      collided 5 ways, F 4) meant Alt+key cycled rather than activated. Every
      control is still reachable by Tab, and `wrap_nav_key` / `_on_notebook_nav`
      already keep the notebook tab bar in the cycle. The one surviving `&&` is
      the escaped literal in the "Pitch && Roll" box label, which keeps a
      `SetName` so it is spoken as "Pitch and Roll".
- [x] Redundant `SetName` pruned — ~20 calls on buttons, checkboxes and
      radioboxes restated the visible label, *replacing* the label-derived
      accessible name; where the two differed (checkbox "HRTF Binaural
      Processing" named "Enable HRTF", and four others) the spoken name no
      longer matched the screen. Names are kept on `Choice`/`SpinCtrl`, which
      have no intrinsic label.
- [x] Pending-save loss on close — not feedback but data loss, so fixed here.
      The 2 s debounce was never flushed: `BeamTelFrame._on_close` destroyed the
      window and `configurator.py` had no close handler at all, so closing
      within 2 s of an edit discarded it silently. New
      `ConfigPanel.flush_pending_save()`, called from both.

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
- [ ] Phase 6 — with a screen reader actually running: each group announces its
      name on entry, all 17 spin controls announce their own, and no toggle
      strands focus. Construction, tab order, group nesting and the
      hide/show fix were verified programmatically; MSAA announcement was not.
- [ ] `c8743ac` — scanner left/right and approach side against a deliberately
      placed target
