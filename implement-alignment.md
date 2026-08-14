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
