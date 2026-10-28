# Junction proximity cues

## Stop bar, 2026-10-27

The stop bar replaces the 520 Hz pips below; their target selection survives. A bar is the line before a junction boundary, a mapped stop point, the start of a gated or private road, or a dead end. The tone is D#4 (311.13 Hz) with a sub an octave down (D#3, 155.56 Hz) at half amplitude. It pulses faster towards the bar (period `clamp(min(d/30, d/speed/4), 0.18, 1.5)` s, half on), holds solid while the nose is on the bar (0.5 m before the line to 2.5 m past it), fades 18 dB over 0.6 s once stopped there, and stops when the nose leaves the bar either way.

The first driven run sounded erratic: pulses switching to solid while moving slowly. Three causes, all fixed in `audio.py`:

- The pulse clock kept seconds modulo the period. A period shrinking at every 20 Hz update cut the current pulse short or stretched it. It now counts cycles, so the tempo changes smoothly.
- The fastest pulse was 60 ms on, 60 ms off, close enough to a buzz to pass for the solid tone. The floor is now 0.18 s.
- There was no hysteresis. Front-node distance and speed jitter flipped pulse, solid and silence at the edge of the bar. Once on a bar, it now takes `STOP_BAR_HOLD_M` (1 m) of backing off to leave it, and "stopped" means below 0.2 m/s until above 0.5 m/s. A car stopped short of a bar it has not yet reached gets one 150 ms pulse every 2 s at the stopped level (`waiting`), so the bar ahead is not forgotten. Backing off a bar already reached is leaving it, and is silent. Stop signs and red lights no longer sound different; their speech carries that.

Traps:

- **The bar is held as a world line** (`stopBarAt`). Once the nose reaches it, the selectors move on to whatever is beyond, and a junction boundary's distance clamps to zero. The line is fixed from the last fix more than 0.5 m out and reported as `signedDistance` until the nose is 2.5 m past.
- **`distance` stays non-negative on the wire;** the signed value is the optional tail `signedDistance`, with `kind` (`junction`, `gate`, `deadEnd`, `stopSign`, `trafficLight`). An older receiver rejects a whole R2 packet with a negative distance, which would drop all road guidance.
- **While the assistant steers, stops come from its route** (`routeStopAhead`), not `findJunctionAhead`. The assistant drives roads manual guidance does not list, and once a turn is committed, the junction it is about to cross has left `route.junction`.
- **No second bar inside a grouped junction:** a branching node one short (35 m or less) link on from a node that branched on the route is skipped. The test is the previous node's exits on this route, not its road count: a one-way merge has three roads and one exit, and treating it as a junction hid real bars on Johnson Valley.
- **A gate gets a bar wherever the route steps onto a `gatedRoad` link**, not only where a public road becomes private. `isPrivateEdge` counts the gated flag as private, so a gate between two private roads had no public-to-private step and no bar. `edge.gated` keeps the gate flag on its own. Johnson Valley's links mark private roads with `type = "private"`, which `isPrivateEdge` does not read, so there the driveway gate already had a bar.
- **A gated exit is named in the junction prompt:** "straight, or right through a gate". `gatedAhead` flags an exit with a gated link inside the junction or on its road within 40 m, up to the next branch. On the second gated run the warning came about 2.5 s after "Turning right", at 50 mph, because the bar is at the junction itself.
- `performScan` is at LuaJIT's 60-upvalue limit. Reach new helpers through `M`, as `stopBarAt` does.
- Manual guidance still treats gated and private roads as not listed, but a road that only continues as one is now `kind = "gate"` ("Gated road in ..."), not a dead end.

Checks: `junction_proximity_sim.py` measures both oscillators and their ratio, pulse cadence, a slow 18 m approach updated every 50 ms (no pulse cut short, and tempo only rises), edge jitter on the bar, creeping-speed jitter short of it, the fade (-18 ± 0.5 dB), and silence past the bar, reversed off it, and short of it while stopped. `driving_assistant_sim.lua` (run in GE with `BEAMTEL_ASSISTANT_SOURCES`; the standalone console cannot read the `bng_mod` junction) covers gated and rough exits, route bars, the gate and dead-end kinds, the held line, and the grouped-junction rule. A read-only sweep of all 5,680 approaches on Johnson Valley found a bar for every junction within 12 m. Not yet heard while driving.

## Original pips

Implemented 2026-09-14. The optional R2 `proximity` field shares distance cues between manual road guidance and driving assistance. Old packets remain valid. The detector continues scanning while assistance owns the player vehicle, even when the road-detection toggle is off.

## Behavior

- Use the front-most live vehicle node. Prefer the applicable mapped traffic control stopping point used by stock AI, unless an earlier junction boundary lies before that controlled junction.
- Without a control point, use the forward navigation junction boundary. Search up to 400 metres; acquire at the existing speed-dependent reaction/braking warning distance (at least 60 metres). Retain an acquired target while slowing.
- Single 520 Hz pips indicate approach distance. Known stop signs, red, red-yellow, and flashing-red signals use two pips 100 ms apart. Green retains the approach cue at the same target. Yellow, unknown, disabled, flashing-yellow, and ambiguous preview states do not assert a definite stop requirement.
- Repetition ranges from 1.8 seconds to 0.30 seconds, based on distance and time to the target. This does not estimate a safe cornering speed.
- Silence for stopped/reversing vehicles, no target, off-road/dormant/loading states, disabled configuration, neither driving mode enabled, or data older than 0.75 seconds. Speech and the spatial turn-choice cues do not cancel it.
- The new default-on `road_junction_proximity_enabled` setting uses the existing intersection tone volume. The old one-shot near-junction double pip is suppressed when continuous distance beeps are enabled; the existing entry tone remains available.

## Checks

Python compilation and `junction_proximity_sim.py`, `road_audio_sim.py`, `road_guidance_sim.py`, `traffic_control_sim.py`, and `assistant_audio_sim.py` pass. Checks cover measured waveform cadence, stop/green changes, stationary silence, stale data, manual/assisted audio gates, configuration, malformed R2 fields, MCP snapshot wiring, and bounded opt-in diagnostic recording.

The production signal resolver passes `road_signals_sim.lua` inside BeamNG, including front-node geometry, stop/green target stability, previews, nearer junction selection, and retained acquisition. The game loaded the changed road detector and AI coordinator. Fresh `beamng.log` entries confirm the successful detector load (4339.22928 and 4434.34450) and the final proximity-ready marker (4434.34482). A LuaJIT upvalue-limit failure found during loading was corrected before these successful loads.

A read-only live query at the parked vehicle measured 108.36 metres from its front to junction `DR33_27`, using a supplied 20 m/s approach speed. With manual road detection enabled, a bounded stationary GE-to-Python R2 replay showed active approach audio at 37.62 metres / 0.627-second repetition and stop audio at 10 metres / 0.30-second repetition. The replay sent a final single-pip approach phase, then restored the real detector automatically. MCP confirmed the real on-road feed and silent stationary audio afterward. The vehicle was not moved; road detection was returned to its previous off state.

The player confirmed that the speeding-up single pips and stop double pips were clearly distinct. The assistant's 10 Python regressions and the MCP HTTP transport checks also pass. After the final receiver restart, MCP read back durable `road` samples from a new armed diagnostic session, with no recording error; road detection was restored off.

A fresh player-driven approach is still needed to judge notice time and how the cue mixes with the engine and speech. This stationary replay does not establish a safe turning speed or validate a driven red/green transition.
