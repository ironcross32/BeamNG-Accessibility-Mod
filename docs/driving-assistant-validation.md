# Driving assistant: implementation and validation

## First driven diagnostic run and repair, 2026-09-14

The player confirmed hearing the three spatial junction tones and the two descending disengagement sweeps. The first recorded drive (`20260914T230001-624414Z-player-junction-drive.ndjson`) then exposed three additional problems before an automatic fallback could be tested:

- First gear would not hold. Stock `manualGearbox.lua:gearboxBehaviorChanged` puts first/reverse into neutral when realistic mode is set with automatic clutch disabled. Restoring mode after AI starts therefore changed the gear even though it suppressed the announcement. Assistant-owned AI calls now temporarily block the gearbox-mode setter, restore the setter immediately even on error, and never reapply the mode afterwards. Player-triggered changes remain effective.
- The startup path included the entry node of the occupied segment. Stock manual-path planning visits every supplied node, so it attempted to return to a node behind the car. The assistant now starts at the forward endpoint while retaining the full connected path for GE tracking and endpoint-connected extensions.
- Diagnostic JSON grew beyond the scanner listener's 1024-byte receive limit after straight commitment, while shorter heartbeats kept arriving. The shared listener now accepts a full UDP datagram (65535-byte buffer). Vehicle samples also include gear, RPM, clutch, and handbrake for launch diagnosis.

Stopped suspension now waits 0.3 seconds below 0.1 m/s with throttle at or below 0.05. Resume requires speed above 0.3 m/s or throttle above 0.1. The separate thresholds reduce repeated planner restarts from small stationary fluctuations. The existing two-second restart while held against a stop remains to prevent stock AI's stationary crash maneuver.

The expanded Lua regression passed in BeamNG with stock-like first-gear reset behavior, forward-only startup targets, stopped-state hysteresis, normal player gearbox changes, exception cleanup, and all earlier turn/session/restoration checks. Ten Python checks and compilation passed. A synchronous test in the actual parked vehicle confirmed gear index 1 after activation, suspension, and disengagement, then restored its original neutral gear before simulation advanced. This establishes gear preservation, not a successful driven retest. A fresh recording named `first-gear-forward-target-retest` is armed for that retest. BeamNG foreground focus was verified by process ID after the receiver restart. The runtime test exercised the repaired module, but fresh reload markers had not yet appeared in the on-disk `beamng.log`; that part of reload verification remains pending.

## Turn fallback, audio and observation update, 2026-09-14

Unavailable held directions now fall back to a legal exit in order: straight, left, right. This covers both approaching a junction and enabling inside the commitment zone. Valid player choices keep priority, and the committed exit remains locked. Dead ends and unsupported junctions still disengage because they have no supported route.

Junction events trigger three fixed audio slots at 0, 240, and 480 ms for left, centre, and right. Missing exits remain silent. A separate pair of descending tones signals disengagement, including receiver-side timeout, and works with road detection off. Stock shifter-mode messages are filtered only during synchronous assistant-owned AI/gearbox calls. Player-triggered messages and unrelated warnings pass through, and the hook is restored even when a call fails.

MCP now provides `assistant_diagnostic` and the `assistant` state section; see [observation workflow](assistant-diagnostics.md). Detailed vehicle and navigation samples accompany the existing session protocol, with durable records of announcements and off reasons.

Validation completed:

- Python compilation and ten assistant session/recording checks passed.
- `diagnostic/assistant_audio_sim.py` passed using the real KEMAR HRTF data: left/right channel direction, missing-slot silence, block-by-block playback, and the off cue with road detection disabled. Existing road-audio checks passed.
- Expanded `diagnostic/driving_assistant_sim.lua` passed inside BeamNG using isolated control mocks: invalid centred and held choices, immediate activation fallback, valid held/locked choices, dead ends, shifter suppression with unrelated/player messages preserved, exception cleanup, route extension and restoration checks.
- MCP HTTP transport checks passed. A parked live activation produced an `on` event, an actual `left,straight,right` junction event, and 286 combined diagnostic samples. A vehicle-side disengagement reached Python and the saved recording. Speech logs contained no shifter-mode announcement on activation or shutdown; gearbox mode remained `realistic`.
- The two updated Lua modules were installed through the live `bng_mod` junction and the Python receiver was restarted. `beamng.log` confirms a fresh AI extension load at 1382.85713 and an updated-coordinator marker at 1404.14671. Foreground focus was subsequently verified against the actual BeamNG process ID, not the receiver window.

A moving junction/curve acceptance run remains. The player subsequently confirmed both audio cues. The older sections below describe historical validation; their unavailable-choice disengagement behavior and later gearbox-mode restoration are superseded by the updates above.

## Follow-up repair after live driving feedback, 2026-09-13

The original immediate road-loss cutoff and restriction on activation inside the commitment zone are superseded by this repair. The four Lua files were installed through the restored `bng_mod` junction; their preceding versions are under `backup/assistant-repair/originals`.

The main identified defect was calling `ai.driveUsingPath` every time GE removed a passed node, including during cornering, and again when a junction choice was committed. Stock source confirms this resets the planner. The vehicle now keeps that planner and uses `ai.setPath` only to append a tail beginning at its existing endpoint. Dropping passed nodes causes no planner call. Intentional starts after stopping remain distinct from route updates.

Activation selects nearby eligible roads using facing rather than low-speed velocity, including shoulders within eight metres of the road boundary. One-way refusals provide a relative angle. A valid held exit can now be selected immediately on activation inside the commitment zone. General one-way speech in Python also gives the legal relative direction; restart BeamTel to load that Python change.

Departures over two metres beyond the road boundary enter a rejoining state; recovery clears within half a metre of the boundary. The committed connected path is retained for stock steering to rejoin. It does not invent a cross-country path or choose another exit. A departure beyond 25 metres, mismatched elevation, or 12 moving seconds without improving the lateral offset ends assistance with a brake instruction. A forward static ray adds an obstacle/brake warning independently of unresolved-endpoint slowdown suppression. This ray is a warning, not comprehensive collision prediction or automatic braking.

Expanded Lua diagnostics passed in BeamNG's runtime against the repaired sources. They verify endpoint-connected appends, no planner restart on node pruning or turn commitment, near-junction held selection, explicit wrong-way direction, rejoining/return/stalled recovery, and the original session/restoration checks. Python compilation, seven session tests, and road-guidance diagnostics including the new wording passed. A live read-only route query at the current player position accepted the road and returned both legal T-junction exits. A full driven bend/left-turn and obstacle-avoidance acceptance run remains outstanding; the collision report is not established as solved by these checks.

BeamNG was focused for the reload attempt and live source/API probes returned the repaired implementation. The current `beamng.log` did not expose fresh load entries during this pass, so full reload verification under the repository rule remains incomplete.

## Original implementation baseline

Status on 2026-09-13: implemented, with focused runtime checks passing. **Not yet verified for normal driving.** The connected game's unpacked mod does not contain these new files; no level or extension reload was performed. Runtime checks staged the workspace sources in memory, without changing the installed game or replacing the running mod.

## Components and protocol

- `beamtel.py` binds F10, H, suppresses duplicate road-junction announcements while confirmed assistance is active, and starts `driving_assistant.py`'s heartbeat worker. Existing signal and obstacle warnings remain enabled.
- `beamtelAI.lua` retains command port 4449 and response port 4445. Commands are `ASSISTANT_TOGGLE:<token>`, `ASSISTANT_OFF[:<token>]`, `ASSISTANT_STATUS:<token>`, and `ASSISTANT_HEARTBEAT:<token>`. The optional off token scopes cleanup to that activation; an unscoped off also supports receiver startup and shutdown cleanup.
- `ASSISTANT:` session responses contain JSON with activation token, vehicle ID, sequence, active flag, event kind, and message. One-shot status queries use their own request token. Python rejects obsolete/reordered packets. Vehicle callbacks carry ID and token; both Lua coordinators retire ended tokens.
- `drivingAssistant.lua` coordinates the session. `roadDetector.lua` exposes announcement-independent start/update/choice queries, and `assistantRoutes.lua` selects exits deterministically from directed paths.
- `beamtelAssistant.lua` owns the vehicle's steering filter, excludes AI from every pedal input, and uses stock `ai.driveUsingPath` with explicit nodes, lane driving, and vehicle avoidance. Gearbox mode is restored after every planner start. No stock AI implementation was copied.

Junction guidance begins at `clamp(speed * 7, 30, 140)` metres, and the vehicle samples physical held input at `clamp(speed * 2, 12, 35)`. Between GE samples it estimates remaining distance using speed and elapsed simulation time. Exits within ±30 degrees count as straight; those through ±150 degrees count as left/right. Ties use node IDs. Committed outgoing paths remain locked until tracking reaches the outgoing road beyond the junction radius.

The receiver sends heartbeats every 0.5 seconds. GE and vehicle leases expire after three simulation seconds; vehicle plan freshness expires after one simulation second. Pausing does not consume those leases. Python also detects a silent command bridge after five wall-clock seconds and sends explicit off; normal paused GE updates still acknowledge heartbeats.

## Checks completed

- `uv run --no-sync --cache-dir .uv-cache python -m compileall -q beamtel.py driving_assistant.py diagnostic/driving_assistant_sim.py` passed. A workspace cache was needed because the default uv cache was inaccessible.
- `uv run --no-sync --cache-dir .uv-cache python diagnostic/driving_assistant_sim.py` passed seven session checks: confirmation gating, old tokens, duplicate/reordered packets, scoped off, malformed responses, lost-confirmation recovery, and disabled-status reporting.
- All five new/edited production Lua modules compiled with BeamNG's `loadstring`.
- `diagnostic/driving_assistant_sim.lua` ran against the actual workspace module sources in an isolated BeamNG Lua environment. It checks thresholds, deterministic ties, disconnected paths, grouped junction connecting edges, outgoing-road clearance, held-input locking, invalid centred choices, stale callbacks/commands, timeout cleanup, exact source-table restoration (including nil/empty filters), and wheel metadata restoration. It uses mocks for control actuation. Supply `BEAMTEL_ASSISTANT_SOURCES`, a table mapping repository-relative paths to source strings, then execute the diagnostic with `loadstring` in an isolated environment; it needs BeamNG's `vec3`.
- A read-only query of the real West Coast USA navigation graph returned the current player's junction boundary about 2.13 m ahead. The coordinator correctly refused activation with “Clear the junction before enabling the driving assistant.”
- A synchronous test in the actual Lansdale vehicle VM confirmed AI-only steering, separately retained physical steering history, rejection of AI throttle/brake/handbrake/clutch, effective local pedal values, preservation of `realistic` gearbox mode, and exact input-source restoration. The test started and stopped the control extension within one console evaluation, then restored the original inputs and smoothing values. It did not advance steering/driving frames or test gear changes.

Source inspection of the connected game's `input.lua` verified negative steering is left (`kbdSteerRight - kbdSteerLeft`) and suppressed requests remain in `input.lastInputs`. Physical devices have not been exercised. Stock `ai.lua` confirmed explicit manual paths and revealed a five-second stationary crash maneuver even with recovery disabled. The implementation suspends the planner when the player stops without throttle, and refreshes its path before that timer expires when throttle is held at rest. Stop/restart driving behavior still needs live validation.

## Required live acceptance checks

Install the edited mod and run the edited Python receiver. Focus BeamNG before any reload and confirm a fresh load in `beamng.log`; a console command acknowledgement does not establish that a reload ran.

1. On a clear road, verify AI steering through bends with effective player throttle, braking, handbrake, clutch, and manual shifts. Stop for more than five seconds both with and without throttle, then restart. Check for unwanted reversing, recovery, or throttle-model assumptions. Repeat with an automatic vehicle.
2. Exercise keyboard, gamepad, and wheel steering. Verify the dead zone, stationary activation, early announcements, commitment using the latest held input, and locked choices after commitment.
3. Exercise straight roads, curves, crossroads, T-junctions, grouped/one-way/consecutive junctions, dead ends, and unavailable turns. Verify unavailable choices take the deterministic legal fallback and remain assisted; dead ends and unsupported routes still release control with the off cue.
4. Check roundabout rejection before entry. Detection is conservative: directed cycles in the grouped area and compact one-way circuits are rejected. Map-specific roundabout topology, especially larger or unusually tagged circuits, still needs validation.
5. Verify F10 help, slot independence, status, actual speech, both UDP directions, AI-mode takeover, tuning rejection, reset/switch/unload/level cleanup, and killing the receiver. Pause longer than the timeout, then resume.
6. Check slowdown speech on bends/obstacles: brake demand above 0.15 for 0.4 seconds above 5 m/s, at most once per eight seconds, with junction/disengagement priority. For this initial implementation, all advisory braking is conservatively suppressed while a junction endpoint remains unresolved, so some genuine slowdown cues in that interval may be omitted.

There is no automatic braking or traffic-light stopping, lane-change request, destination routing, or persistent activation. Exact lane retention is not guaranteed by stock lane driving. File checks and synchronous VM tests do not replace this acceptance pass.

## One-second selection latch and obstacle filtering, 2026-09-14

The player reported a substantially better junction-proximity run and clearly distinguishable approach/stop pips. That recording contains nine obstacle speech events, but lacks wheel-loading and ray-hit detail; it cannot establish whether a reported brief jump occurred.

After the junction announcement, a continuous one-second valid left/right hold now latches an early selection. Speech confirms it and tells the player to release steering. Centre preserves it; another full valid turn hold can change it before the existing near-junction commitment. Short holds do not accumulate across release, invalid directions preserve a prior valid selection, and the latch/announcement state clears for each new junction and session. `selection` events precede the existing final `choice` events.

Static obstacle hits now require at least 0.2 seconds of persistence, must fall inside the connected planned road corridor, and must be more than 0.6 metres above its mapped surface. This rejects transient rays, nose-down pavement hits, and roadside hits beyond a bend. The filter is approximate: inaccurate graph elevation and low obstructions can evade it. New ray-hit diagnostics and measured wheel loading support evaluation during the next drive.

All four edited Lua modules compiled in BeamNG. The expanded production-source Lua harness passes, including one-second timing, interrupted holds, release retention, replacement, invalid selection, delayed route commitment, next-junction cleanup, a persistent barrier, road-surface hits, and off-route hits. Existing route, control ownership, first-gear, and session tests also pass. The game window was verified by process ID before reload; `beamng.log` confirms the GE latch/obstacle load at 5622.14747. The actual vehicle VM returned the new selection fields and four loaded wheels while parked. These checks do not replace a driven latch/obstacle acceptance pass.

## Reverse steering, 2026-09-15 UTC

The recorded run confirms reverse gear from t=691.641 to 696.150 seconds, with AI steering rising to almost full right lock. It remained there after the road-recovery state cleared. The prior controller used unsigned speed and retained a forward route. The same run confirms a right selection latch at t=682.273 and subsequent commitment at t=683.476.

The vehicle VM now recognizes reverse gear and signed backward motion. Stock AI is suspended before reversing, retaining AI-only steering ownership and local-only pedals. GE retains up to 16 passed route segments and supplies a rearward lookahead point with a bounded lane offset. A low-speed rearward pursuit controller uses that target and the live wheelbase. It does not chase targets in front of the car. Route exhaustion or an unusable rearward target triggers the existing off event and audible cue. Returning to forward driving requests a newly positioned route and waits for its motion revision before restarting stock AI. Old turn latches are cleared on that restart.

This is guidance for low-speed manoeuvres on a known road path, not general reverse navigation or rear obstacle avoidance. The steering calculation uses a 0.6-radian full-lock approximation and a 3-metre wheelbase fallback when wheel geometry is unavailable. Other steering geometries, trailers, and actual reverse cornering still need driven validation.

All four production modules compile in BeamNG. The expanded isolated Lua harness passes the previous forward regressions plus reverse-gear entry before movement, backward rolling in neutral, stale-plan rejection, proportional steering, centring for a straight rearward target, refusing a forward target, continued reverse control while rolling backward in a forward gear, a single fresh-route restart, reverse path traversal through retained segments, and route-limit disengagement. A 15-second ideal bicycle-model check at 2 m/s moved from a shoulder offset of 7 m to within 0.109 m of the 2 m lane target, ending at 0.016 radians heading error with peak normalized steering 0.642. This is a control-sign/convergence check, not a BeamNG driving trial.

Live reloads are confirmed in `beamng.log` at 7357.74857 (GE) and 7357.75180 (vehicle). A read-only map query supplied a valid target 5.00 m behind the parked car. A synchronous test in the actual vehicle VM suspended stock AI in reverse, produced steering demand -0.066 using its measured 2.496 m wheelbase, then resumed forward AI exactly once. Gear, gearbox mode, pedal values, and input-source ownership were restored before any driving frame advanced. A player-driven reversing check is still required.
