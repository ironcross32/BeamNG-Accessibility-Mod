# Junction proximity cues

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
