# Loader implement: fix the geometry, re-aim the docking instrument at blocks

## Context

The docking instrument was built to help line the WL-40's Block Handler Forks up with
something and lift it. In practice it could not be made to work, and from the seat every
failure mode sounded identical: plausible, confident audio followed by the forks not going
where they were supposed to — usually shoving the target away.

A ground-truth dump (`implementProximity.dockTruth()`, added during the investigation and
kept) was run at both tilt stops. Two poses of a rigid body determine the tilt pivot exactly:
it sits at **z 1.188 m** with the lift at its bottom stop, and `edgeL` orbits it at r = 1.63 m
(1.631 vs 1.626 across the two poses — determined, not fitted). The tine underside is a clean
straight line in the longitudinal profile at **35.8°** in the dumped pose. Rotating to level
puts the tine tips at **z ≈ 0.9 m**.

A car's liftable underbody gap is ~0.11 m. **The attachment physically cannot put level tines
under a vehicle** — the lift is already at its stop and tilting forward is the only way to
reach the ground, at which point nothing can enter. That is a machine constraint, not a bug,
and the user has decided to re-aim the feature at what the machine *can* lift: blocks and
pallets sitting on the ground.

Three real defects were found on the way, and they are why this took four rounds to diagnose.
They matter for blocks and pallets exactly as much as they did for vehicles.

### D1 — the tilt axis is a diagonal, so `0°` does not mean level

`resolveImplementNodes` picks `heelL`/`heelR` as the **lateral extremes of the rear band**,
with no constraint on height. On the forks that lands on the *top of the backplate* (z 2.138 in
the dumped pose), so the `edgeMid → heelMid` axis is a diagonal across the implement rather
than the tine plane. `_implTiltDeg` is that axis's world pitch minus its design-space pitch, so
`0°` means "as modelled", which for this attachment is ~38° away from level tines. The status
readout said 6° at the fully-racked-back stop where the tines were 47° nose-up.

This feeds `audio.py`'s quarter-tone tilt scale, where 400 Hz is nominally "level". It has been
reporting level at an angle nothing can enter.

### D2 — `edgeC` is 0.6 m off centre, biasing every lateral reading

`bandPicks` chooses `edgeC` as the node nearest the centreline *within the front band*. Forks
have nothing in the middle, so it lands on an inner node of the **left tine** — 0.6 m off
centre and 0.87 m behind the tips. The docking origin is `mean(edgeL, edgeC, edgeR)`, so it
carries a ~0.2 m lateral bias and sits short of the tips.

### D3 — band thickness is quantized to the bin height, so thin voids are unreachable

`vehicleGeometry.bands()` collapses a 24-bin histogram; on a 1.63 m target one bin is 0.068 m,
so every band thickness is a multiple of that. `BAND_MIN_HEIGHT_M = 0.10` therefore means "at
least two bins", and any genuine single-bin void is discarded regardless of whether it is real.

### D4 — "under it" and "touching it" are mutually exclusive

The lift gate in `implementProximity.scan` requires `minD > CONTACT_M` (0.12 m), so tines
resting against the underside of a load report contact and never "under". That is backwards for
the one state the operator most needs confirmed.

### Intended outcome

Tilt that means level, a docking origin actually at the tine tips, band selection that can see
a real pocket, and a new **entry gate** that says outright when the tines are too steep to go
into the selected band — the single cue that would have ended this investigation in one press.

---

## Approach

### 1. Fix the sample-node picks and the tilt axis
**`bng_mod/lua/vehicle/protocols/796F6C6F313035.lua`** (`resolveImplementNodes`, ~line 412–478)

- Add a **low-band constraint** to `bandPicks`: restrict candidates to nodes in the lower part
  of the implement's design-space vertical extent (`p.z <= minZ + IMPL_FLOOR_BAND * zSpan`,
  new constant alongside `IMPL_EDGE_BAND`). Apply to both the front and rear band picks, so
  `edgeMid → heelMid` becomes the implement's **floor plane** — the tine underside on forks,
  the bucket floor on a bucket. Fall back to the unconstrained pick if the low band yields
  fewer than two candidates, so a flat implement with no vertical spread still resolves.
- Fix `edgeC`: pick the node nearest the **midpoint of `edgeL`/`edgeR`** rather than nearest
  the vehicle centreline. On forks with no centre structure it still lands on a tine, but on
  the *inner* face and no longer skews the mean.
- Report `_implTiltDeg` as the **world pitch of the floor plane**, dropping `_implTiltZeroDeg`
  from the live path. With a true floor axis, world pitch *is* the angle from level and needs
  no design-pose offset. Keep the design zero computed and logged as a diagnostic only, so a
  future implement whose floor plane resolves badly is visible in the log rather than silent.
- Extend the existing `implement '%s' (%s): ...` log line with the heel band's vertical spread
  and the resolved floor-plane design pitch.

Consequence to note in the commit: `audio.py`'s tilt scale is unchanged in code but changes in
*meaning* — 400 Hz now genuinely means level. The scale is already two-sided about a shared
zero (`IMPL_TILT_*`), so the forks' asymmetric −47°/+38° range is handled.

### 2. Docking origin from the tine tips
**`bng_mod/lua/ge/extensions/implementProximity.lua`** (`getImplementFrame`, ~line 158)

Build `edge` from the **midpoint of `edgeL`/`edgeR`** instead of the mean of all three sample
points. `edgeC` stays in `getImplementPoints`, the narrow-phase sweep and the clearance sets —
it is a real contact point, just not a centre. The lateral-axis heading derivation already uses
`edgeL → edgeR` only and is unaffected.

### 3. Un-quantize band thickness
**`bng_mod/lua/ge/extensions/vehicleGeometry.lua`**

Raise `HIST_BINS` 24 → 48. On a 1.63 m target that is 0.034 m per bin, so `BAND_MIN_HEIGHT_M`
(0.10) becomes a genuine ~3-bin threshold rather than a 2-bin one, and a real thin pocket stops
being rounded away. Wire format is `table.concat(hist, ",")` and both ends ship in the same
mod, so no compatibility shim is needed — but `M.onVehicleGeometry` already guards
`#hist == HIST_BINS`, and that guard must stay.

*(Deferred by decision: the second centre-column histogram that would ignore wheels and legs.
Blocks and pallets sit flat on the ground, so their pocket is already visible in the existing
profile.)*

### 4. The entry gate — the new feature
**`implementProximity.lua`** (docking readout), **`beamtel.py`**, **`audio.py`**

Computed entirely GE-side from the now-correct sample cids plus the selected band:

- Tine plane angle `θ` = world pitch of `edgeMid → heelMid` (same axis as fix 1).
- Tine length `L` = `|edgeMid − heelMid|`.
- Usable insertion depth `d = min(L, T / sin|θ|)` where `T` is the selected band's thickness;
  `d = L` when `|θ|` is below a small level threshold.
- **Enterable** when `d >= IMPL_ENTRY_MIN_DEPTH_M` (start at 0.4 m — enough tine engaged to
  carry). Hysteresis on the threshold, mirroring `SLAM_CLEAR_ENTER_M`/`SLAM_CLEAR_EXIT_M`, so
  a machine breathing on its suspension at the boundary cannot chatter.

Carry `θ` and `d` as **two new fields appended to the end of the `DOCK:` line**, and parse them
defensively in `beamtel.py` (positional parse with a length guard, the same contract the scanner
packet's optional fourth field already uses — an older mod simply omits them).

Presentation, per decision:
- **Speech**: `_dock_phrase` gains a clause — "tines enter 0.15 metres, too steep" versus
  staying silent when enterable, so the gate costs nothing when there is nothing to say.
- **One-shot earcon** on the transition *into* enterable, so "you can go in now" arrives without
  polling. Mirror `trigger_slam_cue` / `SLAM_CUE_WAVEFORMS` / `_generate_slam_cue`
  (`audio.py:977, 1248, 1813`). It must be distinguishable from the existing two-note figures:
  the dock lock chime **descends**, the slam commit **rises**, so this one should be a single
  short tone at a pitch not already claimed (compass/cam clicks are FM near 900 Hz, scanner and
  coupler are single sines near 1 kHz).
  **Regenerate the waveform at BOTH build sites** (`audio.py:1248` and `audio.py:1639`) — the
  same rule `DOCK_LOCK_WAVEFORM` and `HYDRO_CENTER_CLICK_WAVEFORM` already carry.

### 5. Let "under it" and "touching it" coexist
**`implementProximity.lua`** (`scan`, the `inside` computation)

Drop `minD > CONTACT_M` from the `inside` predicate. Keep `nInside >= INSIDE_MIN_PTS` and the
`implCentre.z < boxMidZ` clause. `contact` is already reported as its own field on the `NEAR:`
line, so the two states become independent rather than mutually exclusive.

---

## Files

| File | Change |
|---|---|
| `bng_mod/lua/vehicle/protocols/796F6C6F313035.lua` | Low-band `bandPicks`, `edgeC` re-pick, floor-plane tilt, richer log |
| `bng_mod/lua/ge/extensions/implementProximity.lua` | Origin from `edgeL`/`edgeR`, entry gate, `DOCK:` fields, `inside` fix |
| `bng_mod/lua/ge/extensions/vehicleGeometry.lua` | `HIST_BINS` 24 → 48 |
| `beamtel.py` | Parse two new `DOCK:` fields, entry clause in `_dock_phrase`, fire the earcon on transition |
| `audio.py` | Entry-gate earcon: constants, `_generate_*`, trigger, **both** build sites |
| `diagnostic/implement_resolve_sim.lua` | New scenarios (below) |
| `diagnostic/dock_readout_sim.py` | Assert the new fields and the entry-depth maths |
| `CLAUDE.md` | Document the floor-plane tilt, the entry gate, and why `0°` changed meaning |

## Tests to add

`diagnostic/implement_resolve_sim.lua` — the harness already has 12 scenarios and the fork case
is #12, so extend it rather than starting a file:

1. **Tall-backplate forks**: heel picks must land in the low band, not on the backplate top.
   Assert the resolved `edgeMid → heelMid` axis is within a degree of the tine plane, and
   assert the *old* unconstrained pick would have been >30° off — a check that cannot pass for
   free is the only kind worth having here.
2. **`edgeC` on a centre-less implement**: assert it lands near the `edgeL`/`edgeR` midpoint and
   that `mean(edgeL, edgeC, edgeR)` is within a few centimetres of the true centreline.
3. **Entry depth**: level tines enter fully; 36° tines into a 0.11 m band enter ~0.19 m and are
   rejected; hysteresis holds across 200 ticks of threshold wobble (mirror
   `vehicle_geometry_sim.lua` scenario 9).

## Verification

1. `lua -e "assert(loadfile('<each changed .lua>'))"` for syntax, then run all three sims:
   `lua diagnostic/implement_resolve_sim.lua`, `lua diagnostic/vehicle_geometry_sim.lua`,
   `python diagnostic/dock_readout_sim.py`, plus `python diagnostic/implement_tone_sim.py` and
   `dock_tone_sim.py` since the tilt meaning moved under them.
2. In game, Ctrl+L, then `return extensions.implementProximity.dockTruth()` at both tilt stops.
   **Expect `TILT` ≈ −47° racked back and ≈ +38° fully dumped**, i.e. straddling zero, where it
   previously read −11° and +74°. The status readout should agree, and should read ~0 with the
   tines visually level (`dockCam("side")` + screenshot to confirm).
3. Park in front of a pallet or block on the ground, F9 Ctrl+I. The reference band should be a
   **GAP** at pocket height, not a SOLID face, and the phrase should not carry the entry
   warning. Tilt forward past ~20° and it should start warning, with the earcon firing once when
   you come back within range.
4. Drive the tines under a block until they touch: `NEAR:` should report **both** `inside` and
   `contact`, which today is impossible.
5. Regression: get into an ordinary car. Every implement metric and tone must stay silent —
   `IMPL_FLAG_PRESENT` never sets and the cylinder walk finds nothing, so this is a check that
   the low-band fallback did not accidentally make resolution succeed on a car.
