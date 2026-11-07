# Camera line-up guide

`lua/ge/extensions/cameraLineup.lua` + `camera_lineup.py` + the `_lineup_*` blocks in `beamtel.py` and `audio.py`. Port 4489, game to Python, **send only**. Sim: `diagnostic/camera_lineup_sim.py`.

A free-camera aid for setting up shots: a car passing the camera head-on, or crossing the view left to right. Silent when the car is lined up; the further off, the louder and dirtier the tone, placed around the head on the side of the error. The user drives the car into place with the camera parked.

## Keys

F9 then **Alt+G** is a toggle: off, it arms the guide and opens its key layer; on, it turns the guide off entirely and releases the keys (the session values are kept). Only these keys are suppressed, and only while the layer is open:

| Key | Action |
|---|---|
| X | Switch head-on / crossing (speaks the mode and target) |
| Up / Right | Increase the distance |
| Down / Left | Decrease the distance |
| T | Speak the target, plus "muted" / "not in free camera" when true |
| M | Mute / unmute the guide |
| Escape | Close the layer. The guide **keeps sounding**; keys go back to the game. `keyboard` hooks it as `escape` but reports `event.name` as **`esc`**; matching only `escape` left the keys held for good |

Mode, offset, depth and mute last for the session and are never written to the config. Before the first Alt+G of a session the guide is off. The tone only plays in free camera, whatever the mute.

The layer is a set of `_hook_suppressed` key hooks, the same mechanism as the virtual browser. It is closed by `release_arrow_owners`, `open_virtual_browser` and turning status mode on, and opening it calls `release_arrow_owners`, so two owners never hold the arrows at once (see `_hook_suppressed`'s docstring for why that kills the keys).

## Geometry

Both modes ask one question: does the ray from the **front centre** of the car, along its flattened heading, pass through a target point P?

- **Head-on:** `P = camera + camera_left * offset`. Offset is signed, positive = the car passes on the camera's LEFT (mod-wide positive-is-left; `camera_left = up x forward`). Up/Right make it more positive, so stepping right moves the pass point toward the camera's left; speech always says the side. 0 = "through the camera".
- **Crossing:** `P = camera + camera_forward * depth`. Only the depth point counts; a diagonal pass through it is lined up. There is deliberately no squareness term.

The error is the signed **perpendicular** miss of the path at P, in the car's frame, positive LEFT. One number carries both heading and position: a heading error grows with distance (5 degrees at 50 m is 4.4 m), so it is heard while the car is still far out.

P not ahead of the nose (car pointed away, or past it) has no miss at all and is **full error from behind** (azimuth 180). A degenerate frame is silent, never a guess.

The camera's forward is flattened; looking straight down it collapses, and the camera's up vector, flattened, stands in (it is where the top of the screen faces). Last good vectors are held after that. The nose is `boxFrame.c + f * ext.maxF + r * midR` from `vehicleGeometry`; before extents resolve the origin stands in, which is the same line and only moves the "is P ahead" test.

## Tone

Computed in `camera_lineup.tone_for`, smoothed (120 ms) and synthesised in `audio._render_lineup`:

- Deadzone 0.5 m silent, full scale at 8 m, linear between.
- FM carrier 440 Hz, index 1.0 rising to 1.3 with the metres.
- Ratio 1.0 when lined up, drifting below 1.0 for a LEFT miss and above for RIGHT, **in proportion to the bearing angle**: the full 0.92 / 1.08 only at 180 degrees (target straight behind). It first followed the metres and saturated at 8 m, which far out is a few degrees of turn, so most of the turn carried no change. At exactly 180 left and right meet, so the ratio crosses from 0.92 to 1.08 there; the 120 ms smoothing glides it.
- Peak -12 dB. **Loudness and grit come from the miss in metres; position comes from the bearing** to P, mirrored onto the side of the error (a target 20 degrees right means a left miss, so the sound sits 20 degrees left: steer away from it). The first build placed it at `+/- 90 * t` from the metres, and from the seat it never went behind and jumped constantly: far out, a fraction of a degree of steering is metres of miss. The bearing is continuous all the way round, so a target abeam sounds at the side and a target behind sounds behind, with no jump into the pointed-away case.
- Azimuth glides as a unit vector (250 ms, slower than loudness) so a swing through behind takes the short way.
- **No zipper:** amplitude and index are ramped across each block, never stepped, and when the azimuth moves the block is convolved with both the previous and the new HRIR and crossfaded. Swapping the filter at the block edge was the zipper heard while steering; the crossfade cuts the broadband artefact energy by about 14 dB on a fast swing. (`audio.py`'s coordinate-guidance tone still swaps at the edge.)
- Front/back is the HRTF's weak axis: a 440 Hz FM tone at index 1.3 has little energy above 2 kHz, where the pinna cues live. If behind still reads poorly, the fix is spectral content (brighter carrier or a noise layer), not more angle.
- Not mixed under the obstacle duck. The amplitude glides to zero on mute, close, leaving free camera or a stale feed (0.5 s, aged out in the audio callback since nothing on the listener notices silence).

## Wire

`LINEUP:camX,camY,camFwdX,camFwdY,noseX,noseY,vehFwdX,vehFwdY` at 20 Hz in free camera with a player vehicle; `LINEUP:OFF` on leaving, re-sent every second while out. Forwards flattened and unit. Extra trailing fields are ignored, per the append-only rule. Python owns every decision, which is why the mod needs no command port.

## Later

A part-framing mode (camera facing a chosen part of the car for AI descriptions or screenshots) is meant to reuse this core with P on the car.
