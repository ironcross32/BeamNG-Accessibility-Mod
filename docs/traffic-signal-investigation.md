# Traffic signal notification investigation

Investigated 2026-09-13 against the running game through BeamTel's GE Lua console.
The investigation below led to the implemented road-guidance notifications.
Implementation and validation notes follow the original findings.

## Live findings

The loaded level was `/levels/italy/main.level.json`. The console and vehicle
telemetry were responsive. `core_trafficSignals.getData()` reported loaded and
active. All 57 instances had controller type `signStop`, state `basicStop`, and
action `briefStop`; Italy's signals file contained no sequences. The closest two
signal points were approximately 9.7 and 12.3 metres away. Both had heading dot
products near 0.057 and positive relative distance, so proximity alone would
incorrectly treat these as upcoming controls for the player's approach.

A supplemental scan of TSStatic names/shape paths for `trafficlight`,
`traffic_light`, and `semafor`, plus nonempty `signalInstance` fields, found no
matches. This naming heuristic does not prove that decorative light geometry is
absent, but there are no registered functioning traffic lights in this session.

Read-only inspection of other installed levels' `signals.json` through the game's
virtual filesystem found:

| Level | Traffic-light instances (`lightsBasic`) | Stop signs | Sequences |
| --- | ---: | ---: | ---: |
| Italy | 0 | 57 | 0 |
| East Coast USA | 9 | 60 | 3 |
| West Coast USA | 149 | 186 | 7 |

During the initial investigation these other maps were not loaded and no vehicle
movement, map switch, signal state override, or reload was performed. Subsequent
implementation testing is recorded below.

## Verified game interface

Read the running game's `lua/ge/extensions/core/trafficSignals.lua` using
`readFile`, since the installation path documented in AGENTS.md was unavailable
to the local shell.

- `core_trafficSignals.getSignals()` returns signal instances.
- `instance:getController().type` distinguishes signs from light controllers.
- `instance:getState()` returns a semantic state name and definition table,
  including human-readable name, action, lights, and optional flashing data.
- `instance.pos`, `dir`, and `road` describe its position and approach. `road`
  includes directed navigation nodes `n1`/`n2`, road position, direction, and radius.
- `instance:getVehPlacement(vehicleId)` returns `dot`, `dist`, and `relDist`.
  A negative `relDist` means before the signal point; positive means past it.
  `dot` measures agreement with the signal's travel direction. It requires the
  vehicle to be present in `map.objects`.
- `getMapNodeSignals()` indexes signal data by directed road-node pair.
- `onTrafficSignalUpdate(results)` is an extension hook keyed by signal name,
  with state, linked objects, action, and light data. Initial state must still be
  queried when a target is acquired; changes may have occurred before acquisition.
- `onTrafficSignalsReset`, mission lifecycle, and navgraph reloads provide cache
  invalidation opportunities. Track vehicle changes and missing telemetry too.

Standard states include red, yellow, green, red/yellow, flashing red/yellow/green,
and `none`. Classify by controller and semantic state, not the first lit color:
`basicStop` has a red display color even though it represents a stop sign.
Do not equate action zero/none with green; flashing yellow and inactive signals
can have no action. Use the semantic flashing state, not each dark flash frame.
Unknown/custom definitions should produce an explicit unknown state rather than
a guessed color. The API reports logical state; manual visual light overrides can
change the mesh without changing logical state.

`getBestSignal(pos, dirVec, resetCache)` is only a candidate helper. Its installed
implementation scores distance and directional alignment over all valid signals;
it does not enforce path connectivity, a hard range, or active/light-only filters.
It caches the result until the player's road-node pair changes unless reset.
Do not use its result alone for spoken guidance.

## Integration design

1. Resolve relevant controls in GE Lua, using the existing road detector's
   directed road and forward graph traversal. Match signal road edges along that
   approach; reject opposing/cross traffic, passed signals, excessive lateral or
   height separation, invalid instances, and unsupported data. On ambiguous
   branches, avoid claiming a light controls the player without sufficient route
   evidence. The engine source explicitly says lane-specific signal support is
   currently unsupported, so turn-arrow/lane selection needs separate validation.
2. Read state on target acquisition and at roughly 5-10 Hz, or use the update hook
   plus periodic reconciliation. Retain the selected instance while stopped and
   release it after crossing the control point with hysteresis. Measure approach
   distance along the road to the control point, rather than to the lamp mesh.
3. Add optional signal data to the existing `R2|` JSON packet on port 4462:
   availability, identity, kind, semantic state, distance, and approach phase.
   Extend `road_guidance.py` validation and feed state; preserve old-packet behavior
   when signal fields are absent. Distinguish no relevant signal, unsupported map,
   inactive signal, and stale telemetry. A separate resolver helper can keep
   `roadDetector.lua` manageable without allocating new UDP ports.
4. In `beamtel.py`, announce acquisition once, for example "Traffic light ahead,
   red, 200 feet", then relevant state changes such as "Light green" while
   approaching/waiting. Include an on-demand status readout. Deduplicate by target
   and semantic state, expire queued stale announcements, honor configured units,
   and reset on mode/vehicle/map changes. A green announcement describes the signal;
   it should not assert that the intersection is clear.
5. Provide a notification setting in the existing configuration interface. Stop
   sign announcements can share detection infrastructure but should be a distinct
   option and phrase, not be reported as traffic lights.

## Validation needed before shipping

Use East Coast USA or West Coast USA for an actual light cycle. Check initial red,
yellow, and green acquisition; red-to-green while stationary; passing the line;
opposite/cross-road rejection; curved approaches; junction branches; and duplicate
heads sharing an approach. Check flashing/disabled/custom states, pause/resume,
switching vehicles/maps, telemetry loss, and notification toggling. Verify Lua
selection, transmitted packets, and actual speech together. Add meaningful feed
tests for transitions, deduplication, expiry, and backward compatibility; syntax
check edited Python files as required by AGENTS.md.

## External reference

BeamNG's official [signals.json documentation](https://documentation.beamng.com/modding/levels/level_formats/signals/)
describes level instances, controllers, sequences, navigation association, and
visual object linkage. The API details and counts above were checked against the
installed source and live session rather than inferred from that documentation.

## Initial implementation and validation (2026-09-13)

`roadSignals.lua` now selects registered signs and lights on directed road edges,
following only unambiguous continuations. `roadDetector.lua` includes optional
`signal`, `signalStatus`, and `signalContext` fields in R2 packets on the existing
4462 port. It scans up to 180 m, announces acquisition within a speed-dependent
45-180 m range, retains acquired controls during deceleration, and marks the near
phase at 12 m. It uses semantic state polling rather than installing engine hooks.

`road_guidance.py` validates the fields and deduplicates approach, near, and state
change events. Context changes and stale feeds re-arm notifications; short missing
target gaps do not repeat them. `beamtel.py` speaks events through the existing
speech path, with separate default-on `road_stop_sign_speech_enabled` and
`road_traffic_light_speech_enabled` settings in both configuration entry points.
Road guidance must be enabled (F9, Ctrl+R); F9, Ctrl+Shift+R includes controls in
the status readout even with automatic control speech disabled. Loading/settling
suppresses event acquisition so a suppressed initial callout is not consumed.
When a control and intersection approach arrive together, they share one utterance
so the control is heard first without discarding the available exits.

Validation performed:

- Python compile checks and `diagnostic/road_guidance_sim.py` plus
  `diagnostic/traffic_control_sim.py` passed. Tests cover older packets, settings,
  state changes, repeated waiting samples, near reminders, reacquisition, map
  context, stale feeds, and malformed values.
- `diagnostic/road_signals_sim.lua` passed inside GE using the production helper:
  opposing/cross-traffic rejection, passed control, range, a curved continuation,
  ambiguous branch rejection, lateral/height mismatch, unknown/flashing/off states,
  and inactive-system reporting.
- Real wx configuration controls loaded both defaults and round-tripped disabled
  values, then restored defaults without saving user settings.
- Italy `stop 1`: actual spoken calls at 95 feet and 29 feet; no repeats while
  stationary; no target facing backwards or after passing. Lua snapshots and the
  road-status readout agreed with the speech feed.
- West Coast loaded with a fresh `beamng.log` load and navgraph/road-detector
  records after focusing the game. `stop 29` announced at 92 feet and appeared in
  the on-demand road readout. `trafficLight 1` produced natural yellow, red, and
  green changes while stationary, matching the engine's state. After restarting
  the final Python build, acquisition at that light announced red at 30 feet.
- Repeated the West Coast load with the loading fix: the initial green light at
  7 feet was spoken after the loading-ready event, followed by yellow and red;
  the initial notification was no longer lost to loading suppression.
- Final combined-event check at West Coast `stop 29` spoke "Stop sign in 62 feet.
  T-junction in 81 feet: left or right." The running app uses the final source,
  and both installed Lua files were byte-compared with the workspace.

The running Python application was restarted from source. Only the two affected
Lua files were replaced in the installed mod ZIP; the original ZIP is backed up
locally under `build/traffic-control/`. Test placements used the game teleport
helper; they do not constitute a full driving-route test. Lane-specific arrows,
decorative/unregistered objects, and custom controller semantics remain outside
the resolved-state guarantee. Flashing/off/custom behavior was checked with the
production resolver and synthetic data, not a live custom intersection.

## Advance-warning correction after AI driving feedback (2026-09-13)

The initial stationary checks missed a moving-vehicle problem. A 30-second AI
recording confirmed that stopping at every graph fork delayed acquisition, and
the lane-guidance detector's retained edge could select a neighboring slip road.
The correct `stop 18` was first acquired with 50.4 m remaining at 29.4 m/s (1.7 s
at that speed). The preceding scan briefly reported `stop 19` on the other arm.

Changes:

- Read the front-most live vehicle node, instead of `getPosition()`, for traffic
  control distance. The tested Lansdale's front was about 1.919 m ahead of its
  reference position. Geometry failure falls back to the reference position;
  `signalSnapshot().frontNode` exposes whether a live node was available.
- Resolve the current signal approach independently from the retained
  lane-correction edge, favoring heading among overlapping road footprints.
- Follow clear continuations past side streets. At a close fork, preview a
  control only when the candidate arms lead to the same controlled junction.
  A shallow slip-road alternative gets a general warning on the continuation;
  its color is withheld until the approach is selected.
- Add optional R2 `signal.groupId` and `signal.preview` fields. Group-based event
  history prevents the preview-to-specific-stop transition from repeating the
  advance callout. A preview never asserts a light color.
- Scan 400 m and use `min(400, max(60, speed*8, speed*3 + speed^2/6))` metres for
  acquisition: eight seconds, or three seconds of reaction plus braking at
  3 m/s^2, whichever is larger, within the bounded range.

BeamNG's installed `lua/vehicle/ai.lua` reads `bestSignal.pos` as its controlled
intersection stopping position and uses `obj:getFrontPosition()` for its ego
position. There is no separate painted-stop-line mesh used by this code. BeamTel
projects that same mapped point onto the directed road; map authoring determines
whether it coincides with the paint.

Replayed the recorded poses with the measured front offset through the revised
production resolver. The stop-junction preview arrives at 158.2 m / 20.6 m/s
(7.7 s), 4.47 seconds earlier than the original correct-sign acquisition. The
subsequent traffic-light junction gets a preview at 236.2 m / 29.5 m/s (8.0 s).
This second case reproduced a shallow 37-degree slip-road split that had hidden
the light until the immediate approach. Replay uses recorded motion rather than
claiming an identical second AI route. Raw recordings are local generated JSON
under `build/traffic-control/`.

The expanded production-helper Lua checks pass for front-node geometry,
heading-based road choice, side streets, shallow slip roads, common-junction
forks, unrelated branches, and reaction/braking warning distances. Python checks
also cover preview-to-specific deduplication and unknown-to-red transitions.
Measured front-node lookup plus signal resolution at approximately 0.68 ms per
scan for this vehicle and location. The AI was observed in bounded recordings;
the recorder pauses automatically, without changing its route or mode.
