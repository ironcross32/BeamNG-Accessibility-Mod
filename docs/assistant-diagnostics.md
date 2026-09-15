# Driving assistant diagnostics through MCP

Start with MCP `health`. A live world and responsive console are required for an observation session.

1. Call `assistant_diagnostic` with `{"action":"start","label":"junction-drive"}`.
2. The player enables assistance with F10, H and drives. Recording itself never changes steering, pedals, or assist activation.
3. Poll `get_state` with `{"sections":["assistant","telemetry","position"]}` or `assistant_diagnostic` with `{"action":"status"}`. `assistant.active` is the receiver's current confirmed state. `latest` is the last recorded sample; check `sample_age_s` before interpreting it as current, especially after disengagement.
4. Read events with `{"action":"read","since_seq":0,"limit":100}`. Use the returned `next_seq` on subsequent reads. `truncated_before` indicates that the bounded in-memory history has dropped older records; the file retains them.
5. Add observations using `{"action":"mark","note":"Unexpected steering at the T junction"}`.
6. Finish with `{"action":"stop"}`, then `{"action":"review"}`. Use `list` and a bare `session` file name to review an earlier run.

Sessions are flushed to NDJSON files in `%LOCALAPPDATA%/beamtel/assistant_diagnostics`. Recording begins even while assistance is off and stays armed across activations. Detailed vehicle samples arrive about five times per simulation second while assistance is active. Pauses can increase their wall-clock age. Regular heartbeats and session events are also recorded. `error` reports a recording failure.

For junction-distance cues, request `get_state` with `{"sections":["road","assistant","telemetry"]}`. `road.packet.proximity` gives the front-to-target distance in metres, forward speed in m/s, target ID, source (`stopPoint` or `junctionBoundary`), and action (`approach` or `stop`). `road.proximity_audio` gives the cue's enabled/active state, data age, and repeat interval in seconds. Audio `active` describes rendering eligibility; it cannot confirm what the player heard.

While a recording is armed, `road` records save the proximity target, signal, road state, map `speedLimit` in m/s (null when unavailable), and audio timing up to five times per wall-clock second. These also work during manual driving with road detection enabled. `review` reports their count as `road_samples`; `read` or the saved NDJSON retains the individual samples. No road samples arrive while both road detection and assistance are off.

For a read-only map query without moving the car, GE Lua exposes `extensions.roadDetector.proximityAt(position, forward, speed)`. Supply the front position and speed in m/s. The live detector uses this same selector. Signal colours are only assigned to the resolved approach; previews and unknown states never establish a stop requirement.

Vehicle samples include physical held steering, applied steering, AI steering/brake demand, effective throttle/brake/clutch/handbrake, gear index, RPM, speed, gearbox mode, planner starts, path extensions, suspension, connection ages, available exits, requested/selected direction, automatic fallback, and the locked junction. Navigation samples include the path, junction ID/distance, offset, recovery/hazard state, available exits, and commitment state. Junction, choice, speech, and off events retain ordering and reasons. These are control and protocol observations; audio rendering checks and the player's listening feedback establish what the cues sound like.

Turn latching adds `selection`, `selectionJunction`, `selectionHeld`, and `selectionSeconds` to vehicle samples. A `selection` event records the early latch; `choice` still means the route has committed near the junction. Releasing steering preserves a latch. A different valid left/right hold lasting one second can replace it before commitment.

Vehicle samples also include `wheelCount`, `contactWheels` (wheels with more than 1 N of measured downforce), and `wheelDownForce`. Zero loaded wheels is evidence of lost wheel loading, not by itself proof of a jump. Navigation `hazardDetail` records the ray range/distance, vehicle position/facing, hit position, height above the mapped road, rejection reason (`roadSurface` or `outsideRoute`), and persistence time. This helps distinguish pitching into the road or a roadside ray hit from a persistent obstruction on the planned route. These fields were not present in the earlier junction-proximity run.

Use `lua_exec` for direct checks if needed:

```lua
-- Game-engine context
return extensions.beamtelAI.assistantState()
```

```lua
-- veh:current context, once the vehicle extension has loaded
return extensions.beamtelAssistant.diagnosticState()
```

The new MCP tool is available after restarting the Python receiver. Clients that cache tool listings may need to refresh their connection. Commands `ASSISTANT_DIAG_ON` and `ASSISTANT_DIAG_OFF` use the existing AI command port; `ASSISTANT:` packets add `junction`, `choice`, and `diagnostic` kinds and an optional structured `data` field. Existing session tokens and sequence validation still apply.

Reverse guidance adds `motion` events (`reverse`/`forward`). Vehicle samples report signed `forwardSpeed`, `reversing`, `motionWaiting`, and measured `reverseWheelbase`. During reverse the stock `aiMode` is `disabled` while the assist remains active and owns steering. `aiSteering` then records the custom rearward steering demand. Navigation samples report `motion`, `motionRevision`, and `reverseGuidance` (rearward target, distance, offset, and a blocked reason). Direction changes use a revision acknowledgement so delayed forward plans cannot restart stock AI while reversing. A new forward route clears old turn selections. Forward obstacle-ray details are not a rear obstacle scan.
