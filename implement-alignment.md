# Implement alignment — staged work

Lining the WL-40's bucket or forks up with a prop or vehicle was "close enough to get near,
not close enough to work". Two root causes, one generic and one loader-specific, worked in
order. Each stage is independently useful and independently revertible; the commit is noted
beside each.

---

## [x] Stage 1 — Scanner range is a surface gap, not centre to centre

`vehicleScanner` computed `originPos:distance(targetPos)`, and `getPosition()` returns the
object's reference node. Every distance it had ever reported was centre-to-centre: half a
vehicle of phantom range at each end, and — the part that actually mattered —
orientation-dependent, reporting the same number broadside and nose-on to the same car at the
same real gap.

- [x] New `vehicleGeometry.lua`: trimmed extents, hull cids, front/rear contact bands and a
      vertical occupancy histogram, resolved once per vehicle in that vehicle's own VM and
      cached GE-side behind an epoch guard (`e8ca7e8`)
- [x] Scanner wired onto it; `implementProximity` measures targets through the same cache
- [x] Retuned scanner half-distances, floored negative gaps at both ends, removed a
      duplicated block of scanner constants in `audio.py`

Two findings came from the sim rather than from review:

- The node tier over-reads near contact by up to half the node spacing — 0.19 m on a test
  car, i.e. clear air reported while the tines are touching. Fixed by keeping the *smaller*
  of the box and node answers, with `HULL_MAX` raised to shrink the sampling error behind it.
- Raw min/max extents get dragged out over empty air by a detached part, which stays in the
  same object. A percentile trim cannot fix it because the debris is a *dense* cluster; what
  separates it from the body is empty space, so extents cut at the first gap wider than
  `OUTLIER_GAP_M`.

## [x] Stage 2 — Reverse-aware contact end (every vehicle, not just the loader)

Nothing was gear-aware, so reversing measured from the front toward a target behind and
reported ~180°. On a loader the implement override put the origin on the bucket — the far
end — so backing up carried the whole machine length as error in the wrong direction.

- [x] Contact set moves to the rear node band, reference heading negates, `GEAR:R`/`GEAR:F`
      pushed from Python on change with a velocity fallback (`6e3f958`)
- [x] `playerLeftVec` kept on the un-negated forward vector; the sim asserts the sign *and*
      asserts that the buggy form inverts it, so the check cannot pass for free

## [x] Stage 3 — Selectable reference band and the spoken cane tap

`ABOVE`/`BELOW`/`LEVEL` was enough to know you were near something and nowhere near enough to
slide forks into a pocket.

- [x] Vertical profile collapsed into GAP/SOLID runs; one is *selected* rather than a pocket
      being derived, because lifting and ramming want different bands out of identical
      geometry — and selecting is what makes the reference announceable (`652ff55`)
- [x] Auto-select by implement type with `F9` `Shift+I` to override; `F9` `I` speaks the whole
      picture in one utterance; `F9` `Ctrl+I` gates the instrument

## [x] Stage 4 — Docking tones

- [x] Panned pulse (lateral + range) and a beat pair nulling to unison (vertical), phased by
      range so no more than three dimensions are ever live (`5164c16`)
- [x] Scanner suppressed and articulation ducked 12 dB while open — the instrument *replaces*
      rather than adds, which is the whole point on a machine this noisy
- [x] Config keys plumbed; the four `implement_*` keys missing from `configurator.py`'s
      duplicated `DEFAULT_CONFIG` synced as a drive-by (they were reaching `audio.py`
      unvalidated)

## [x] Stage 5 — Slam gate

- [x] Three discrete hysteretic states (clear / over / committed) for using the implement as
      a tool of destruction, riding on the docking toggle (`898775a`)
- [x] Committed earcon rises where the docking lock chime falls — the only two two-note
      figures in the mod, deliberately opposite

---

## Verification

`diagnostic/` is gitignored, so these sims live locally alongside the existing ones:

```bash
lua diagnostic/vehicle_geometry_sim.lua      # stages 1, 2, 5
python diagnostic/dock_readout_sim.py        # stage 3
python diagnostic/dock_tone_sim.py           # stage 4
lua diagnostic/implement_resolve_sim.lua     # regression
python diagnostic/implement_tone_sim.py      # regression
python diagnostic/implement_proximity_sim.py # regression
python diagnostic/hydro_steer_sim.py         # regression
```

## [ ] Next pass — OPEN: play-test findings, 2026-08-13

Reported from play after the beat-pair fix (`c684c58`) had landed and beamtel had been
restarted. **Not yet investigated.** The hypotheses below are untested reasoning, recorded so
the next pass does not start cold — treat them as leads, not findings.

### 1. "No implement fitted" after a Python-only restart

Restarting beamtel alone, without touching the game, produces "no implement fitted" — though
the scanner still tracks. `Ctrl+R` on the vehicle plus toggling the docking mode off and on
clears it.

*Untested hypothesis*: `_implement_word_current` is Python-side state that a restart wipes,
but the mod only re-sends the `IMPLEMENT:` line when the name **changes** — `nameEverSent`
and `lastSentName` are Lua-side and survive the Python restart, so the mod believes it has
already announced and stays quiet. If so this is the mirror image of the REBUILD gap below:
the same one-shot announcement pattern, failing from the other side of the socket. A
`settings_request`-style pull on the Python listener's startup, as `bnvdaRuntime.js` already
does over the UI bridge, would cover both.

### 2. The audio is unchanged by the fix — the important one

The instrument "still stutters and still seems to act as before" after the fix, i.e. the
change that measured correctly on the bench produced no audible difference in game.

**Resolve this first, because it decides whether there is a bug at all**: the pulse voice is
a gated tone at 1.2–12 Hz *by design*, and a stutter is what that is. It is entirely possible
that "stutters" describes the intended pulse train and has been read as a defect twice now.
Before changing any audio, establish which sound is being described — e.g. by setting
`dock_tone_dbfs` very low so the pulse drops away and only the beat pair remains, or by
listening with the vertical error held at zero, where the pair should be a smooth,
unwavering hum and any remaining stutter can only be the pulse.

Other leads, in rough order of likelihood:

- The selected reference band may be far from the cutting edge, holding the error above
  `DOCK_BEAT_MAX_HZ / DOCK_BEAT_HZ_PER_M` (0.5 m) the whole time. The beat would then sit
  pinned at 12 Hz and never vary — audibly a constant flutter, not a null-seeking beat. The
  `F9` `I` readout gives the number directly and would confirm or kill this immediately.
- The pair may simply be inaudible under the pulse. `DOCK_BEAT_DB` is −22 against the pulse's
  −18, and the pulse is the more attention-grabbing texture.
- The pulse rate (1.2–12 Hz) and the beat rate (0–12 Hz) span the same range, so at some
  bucket positions the two modulate at similar speeds. Known at the time of writing and
  judged acceptable given the spectral and spatial separation; may not be.

Bench measurements for reference, all from rendered audio: beat rate tracks spec to within
0.1 Hz across 0–0.5 m of error; centre pitch holds 330.1 Hz throughout; with the target hard
left the pulse sits at 2.17 L/R while the pair holds 1.00. So the DSP does what it is meant
to — which points at the mapping, the levels, or the description, rather than the synthesis.

## [ ] Next pass — REBUILD cannot actually rebuild

The implement cid list is pushed one-shot from the vehicle VM (`_implPushed`,
`796F6C6F313035.lua:259`), re-armed only by that VM's own `reset()`. The GE-side extension
drops its cids on any reload. So a GE Lua reload with the vehicle already spawned leaves GE
with no cids and the vehicle VM convinced it has already pushed — the machine reports
`IMPLEMENT:NONE` and every implement feature goes silent until the vehicle is reset.

Adding `vehicleGeometry.lua` to `modScript.lua` forced exactly that reload and surfaced it
for the first time. It is a latent gap, not a regression from this work.

`REBUILD` on 4470 clears the GE cache but has no path to make the vehicle VM re-push, so the
one command that exists to recover from this cannot.

- [ ] `REBUILD` should `queueLuaCommand` into the player vehicle to clear `_implPushed` and
      re-run `resolveImplementNodes`, not just clear the GE side
- [ ] Consider having the GE extension request a push on `onExtensionLoaded` rather than
      waiting to be told, so a Lua reload self-heals without any user action

## [ ] Next pass — make the docking instrument on by default

Currently opt-in behind `F9` `Ctrl+I`, on the reasoning that it is a mode you enter
deliberately and that it silences the scanner while it runs. In practice that is
inconsistent with the rest of the loader features: the ground tone, the tilt scale and the
articulation tone all appear by themselves on a machine with an implement fitted and need no
keybind at all. It should behave the same way.

- [ ] `dockActive` starts true in `implementProximity.lua`; `dock_mode_active` starts true in
      `beamtel.py`, with `audio_controller.set_dock_mode(True)` at startup
- [ ] Keep `F9` `Ctrl+I` as the toggle, so it can still be switched off
- [ ] Check the interaction with scanner suppression: with the instrument live by default, an
      operator who switches the scanner on inside 5 m of something would get silence from it
      and might reasonably read that as the scanner being broken. Either the suppression
      needs to be conditional on the scanner having been turned on *after* the instrument, or
      the scanner toggle should speak something when it is being suppressed.

That third point is the reason this is worth its own pass rather than a one-line default
flip — the suppression was designed on the assumption that the instrument is a deliberate,
short-lived mode.

## Not yet done — in-game

None of this has been run in BeamNG. The sims cover the geometry, the sign conventions and
the state machines, and the audio was exercised through the real callback, but the following
need the game:

1. **Generic distance** — park a Pickup at a known gap and compare `F9` `D` nose-on against
   broadside. They must now agree; today they do not.
2. **Overlap** — drive into the target until the gap goes negative. Speech must say zero,
   never a negative, and the beep rate must saturate rather than glitch.
3. **Fallback** — a vehicle whose geometry resolve fails must still report a distance; check
   `bnvdahook.log` for which tier answered.
4. **Reverse** — back an ordinary car toward a target: bearing near 0°, distance
   rear-bumper-to-surface, and **left must still be left**.
5. **Coupler regression** — `F9` `Shift+V` align and `F9` `Ctrl+Shift+D` coupler distance must
   be unchanged. Neither path was touched, but both share the file.
6. **Loader** — WL-40 with forks: cane tap against a pallet and against a car, cycle bands,
   then drive the tines into a pocket on tones alone. Then a bucket: raise over a car and
   confirm clear → over → committed fires in that order.
7. **Silence on ordinary vehicles** — every loader feature must stay completely inert in a
   car. That is the invariant the whole implement block is built around and the easiest thing
   to have broken.
