# Fuel stations (`fuelStations.lua`, `beamtelFuelPark.lua`, `fuel_stations.py`)

Finds every place on the loaded map where a car can be refuelled or recharged, routes to the one
the player picks, and on request parks the car at a free pump and hands it back. The player then
shifts into park and refuels the way the game requires (Shift+E opens the stock prompt; in career
that opens the refuelling screen).

Ports: **4486** Game→Python (rows and events), **4488** Python→Game (commands).

## Keys

- **F9 then Shift+F**: one key, three answers.
  - **Parking in progress:** cancels parking (`PARK_CANCEL`). The destination is kept.
  - **At the chosen station:** parks at a pump (`PARK`). This is the confirmation step. The player chose "announce, then confirm" over an automatic take-over.
  - **Otherwise:** opens the station list (`LIST`).
- **F9 then Ctrl+Shift+F**: always opens the list.
- The controller Functions menu has "Fuel stations", "Park at a pump" and "Cancel parking" under Waypoints, each shown only when it applies.
- **In the list:**
  - Enter on a station sends `TARGET:<id>`.
  - When a destination is set, the first line is "Stop fuel station assistance for …", which sends `CANCEL`.
  - **Choosing the station you are already at** (within 40 m) parks straight away. Choosing it is the confirmation.

## The catalogue

It is built on `onWorldReadyState(2)`, and on load into a live world. There are no threads in the GE VM, so it is a state machine stepped from `onUpdate`.

**Official stations** come from `freeroam_facilities.getFacilities(level).gasStations`. This is the data the stock refuel prompt (`gameplay/markers/gasStationMarker.lua`) runs on.
- **Always go through the game's reader.** The `*.facilities.json` files are lenient JSON, with missing and trailing commas in the stock East Coast and Utah files.
- **Names are locale keys.** An untranslated key (common on modded maps) falls back to its `…<segment>.title` segment, prettified.
- **Each pump** is `{areaName, iconName}`. The area is a `BeamNGGameplayArea`: an oriented box whose `scale` is its half-extents (`rot*axis*scale`, exactly as the stock marker computes them).

**Unofficial pumps** are pump and charger `TSStatic`s, matched by the basename of `shapeName`.
- **Speed:** `scenetree.findClassObjects('TSStatic')` returns ids for unnamed statics, so the live scene is enumerated. That is 22,809 objects on Italy and about 60 ms in all, stepped at 2000 per frame.
- **Matching:** `pump` must come with a fuel word (fuel, gas, petrol, diesel, station). `charging_station` / `ev_charger` count as chargers.
- **Excluded:** tank, terminal, pallet, airport, number, sign, jack, water, pumpkin. These are the false positives seen in the stock levels: Industrial's tank farm, gas terminals, Italy's airport refuelling props and pump number plates.
- **Grouping:**
  - Hits within 15 m of an official pump are dropped.
  - The rest are clustered single-link at 25 m.
  - Each cluster is named after the nearest raw POI within 300 m.
- **What the player hears:** they are listed as "unofficial, the game can't refuel here". The mod never refuels anything itself (a decision made with the user).
- Hirochi Raceway's stock pump and chargers are the case this exists for. Italy also has several.

**Stock level survey:**

| Maps | Stations |
|---|---|
| West Coast, East Coast, Italy, Utah, Johnson Valley, Automation Test Track | Declare stations. East Coast and West Coast have EV chargers; Italy has n2o. |
| Industrial, Jungle Rock, Small Island, Gridmap, Driver Training, Derby, Cliff | None |

## Rows and compatibility

- **Message framing:** `LIST` answers `FS_BEGIN:<n>`, then `FS:<json>` per station sorted by distance, then `FS_END`. While the catalogue is still building it answers `FS_WAIT`, and sends the list when the build finishes.
- **Distance and bearing:** distance is to the station's edge (centre minus radius). Bearing is relative to the car, **positive left**, and lateral is `up:cross(fwd)`. Python only words it, so the 180° heading offset never comes into it.
- **`compatible`** is true, false, or null (unknown).
  - It comes from `core_vehicleBridge.requestValue(veh, cb, 'energyStorage')`, compared against the station's `energyTypes`.
  - `air` and `n2o` tanks are ignored, and `kerosene` (vehicle spelling) is mapped to `kerosine` (GE spelling).
  - **The 1 s timeout matters:** a vehicle that never answers must not stall the list.

## Arrival

- Only a route to a station not yet in range is set, using `freeroam_bigMapMode.setNavFocus(centre)` (falling back to `core_groundMarkers.setPath`). The driving assistant and the route beacon then follow it with no changes on their side.
- **The range watch is independent of the route**, which stock clears on arrival.
  - It runs every 0.5 s.
  - `in_range` fires once inside radius + 40 m; `left_range` fires past radius + 70 m.
  - Python then says "Arriving at …. Press F9 then Shift F to park at a pump".

## Parking

### Where the car should stand is measured, not inferred

**The trap:** the obvious reading of the stock data is wrong.
- A pump area is a long thin strip, scale e.g. 2.2 × 0.3 × 1.
- At Italy's Apex Airport the pump itself is a thin slab lying **along the strip's long axis**. The strip runs out from it into both lanes, and a car parked alongside the pump lies **across** the strip.
- The first implementation parked along the long axis and could never overlap the area. A ray test for "which side is the island" also saw obstacles on both sides and parked behind the pump.

**The planner (`placements`) instead:**
1. Takes a height grid round each pump: 0.25 m cells, ±5 m, using downward `castRayStatic`.
2. Fits the forecourt plane to the cells that are not standing up out of it. A median alone turns a slope into a wall at one edge.
3. Slides a car-sized footprint (the player's OOBB extents plus 0.15 m) out from the pump, in both orientations and on both sides, until no cell inside it stands more than 0.2 m above the plane.
4. Keeps a placement only if a box of the car's size there passes the stock `overlapsOBB_OBB` test against the area. That is exactly what opens the refuel prompt, and the only success that counts.

On Italy every fuel pump comes out "across the strip, 1.1–2.5 m out, both sides". Some EV chargers have fewer spots or none. A search costs about 40 ms per station, once per `PARK`.

### Choosing a placement

- **Rejected candidates:** a placement is rejected if another vehicle's OOBB centre is within max(3 m, half length) of it, or if its approach is not clear.
- **Approach:** a 10 m lead-in on the placement's heading, plus a "pre" point 8 m further back when that stretch is open too.
  - The extra straight is there because a car turning onto the line needs room to straighten. One run arrived 3 m wide of a 10 m approach.
- **Reaching the lead-in:** straight if `clearLeg` (rays at 0.6 m along the centre line and both edges) is open; otherwise through one staging point on rings 8/14/20 m out from the station.
- **Direct entry:** a car already lined up a short way back drives straight in.
- **Scoring:** path length, plus 15 when the first leg starts behind the car, plus 8 when the heading opposes the car's.
- **Refusal reasons:** every pump taken; no clear way (drive closer); no room the game would accept.

### Driving: stock AI on nodes added to this vehicle's navgraph

**Why not a plain stock AI call.** The pump is off the navgraph, and every stock destination is a navgraph node. `ai.driveUsingPath{script=}`, the one stock way to follow arbitrary points, **teleports the car** to the script's first point (`vehicleSetPositionRotation`) and swaps the AI's map for a throwaway graph.

**What works instead.** Each vehicle VM holds its own copy of the navgraph (`mapmgr.mapData`), and stock AI plans on whatever that copy contains.
- `beamtelFuelPark` adds the planned points to it as an **isolated** chain of nodes, `beamtelFuel_<token>_<n>_<i>`. They are linked only to each other, with radius half the car width plus 0.3 m and speed limit 2.2 m/s.
- It then calls `ai.driveUsingPath{path = chain, avoidCars = 'on', routeSpeed = 2.2, routeSpeedMode = 'limit', wpSpeeds = {[stopNode] = 0.6}}`. That is an ordinary manual path: no teleport, and the AI does its own steering, speed control and car avoidance.
- **The nodes are deleted afterwards.** They were never linked to the road network, so the graph is left exactly as it was.
- **A map with no road graph** gets a fresh `graphpath` through `mapmgr.setCustomMap`. Then `clearCustomMap` + `requestMap` run at the end, and an empty `mapmgr.nodeAliases` is supplied, because the AI's path check indexes it unguarded.
- **Stock AI targets the car's FRONT** (`ego.pos` is `obj:getFrontPosition`). A probe stopped with its front 0.41 m short of its last node. So the chain runs 5 m past the stop, with a 0.6 m/s target at the stop node, and this module brakes when the wheel centre reaches the stop.
- **Measured on Apex Norte (BX):** stopped inside the pump area, 0.15 m off the line, heading within 3°. No damage; nodes, AI and gearbox all cleaned up.

**What stock AI cannot do stays in `beamtelFuelPark`.**
- **Reversing:** stock AI only reverses as crash recovery. Back-and-fill disables the AI, reverses under this module's own control, then hands the rest of the chain back.
- **Restarting after the AI lets go:** if it drops out of manual mode (gave up, or the map was re-sent and took the nodes), the chain is re-added and the AI restarted. That counts against the manoeuvre budget.
- **The safety layer:** stock AI does not look for static obstacles off the road. The look-ahead at this car's own heights, and the **damage stop**, stay here. Body damage rising more than 150 since the start counts as a touch: stop, then hand back. It catches anything the map has no collision mesh for, and movable props, which static raycasts never see.
- **Gearbox:** AI calls run with `setGearboxMode` blocked, as in `beamtelAssistant`, so the AI's own mode switching cannot fight the arcade mode this module set.

**Inputs and gearbox**
- Inputs are sent as `input.event(name, v, 'FILTER_AI', …, 'ai')`, with `allowedInputSources` for steering, throttle, brake and the parking brake set to AI only. They are saved and restored exactly as `beamtelAssistant.restore` does.
- **Arcade gearbox** for the duration: it selects drive and reverse by itself and auto-starts a stalled engine. The player's mode is restored at the end, as `endJoin` does.

**Steering and progress**
- **Pure pursuit** from the rear axle. Positive lateral is left; BeamNG steering is positive right, hence the minus. Wheelbase and wheel centre are measured from both hub nodes of every wheel. The reference node is not on the centre line; it was 0.25 m off on the test car.
- **Progress goes through the segments in order.** The tracker only moves to the next segment past the corner, or within 3 m of it. Taking the nearest segment let a car beside the pump skip the lead-in and arrive side-on to the island.

**Speed**
- Speed is limited to 2.2 m/s and tapers to the stop at 0.7 m/s².
- An integral term ramps the throttle when the car will not move, for kerbs and slopes.

**Back and fill.** Forward-only driving cannot recover from a car facing away from the approach, or nose against an island; both happened on the first runs. A manoeuvre starts when:
- the aim point is more than 110° off the nose;
- the car has pushed (throttle above 0.12) yet moved less than 0.25 m in 2 s (judged by distance, because pushing a kerb rocks the speed reading either side of zero);
- or it is within 3.5 m of the stop but more than 0.6 m off the line or 20° off the heading. Realigning backs the whole 3.5 m.

The car stops, reverses on the opposite lock up to 3.5 m, then drives forward. At most 6 manoeuvres.
- **Arcade reverse:** in arcade the **brake is the reverse throttle** at a standstill, and throttle is the brake while rolling backwards.
- **Holds at a standstill use the parking brake, never the brake,** or arcade would start backing up. `restore()` gives the player back their own parking brake setting.

**Heights come from the vehicle, everywhere.** `vehicleGeometry.rayProfile(vid, sign)` answers from the vehicle's own nodes, for any jbeam, mods included:
- `lowLift` / `highLift`: just under the lowest body at the travelling end, and 0.4 m above it, kept under the roof.
- `step`: the tallest sudden rise it can take, the smaller of the overhang and half a tyre radius.

The same figures are used throughout:
- GE's planner: the footprint's solid threshold is the smaller step of the two ends, and leg rays stand at the high lift.
- The vehicle VM's look-ahead: sent in `start`.
- `obstacleDetector`.

A BX comes out at lifts 0.13 / 0.53 m and a step of 0.12 m.

**Look-ahead and smoothness**
- **The look-ahead is ground-referenced and must tell a ramp from a wall.**
  - The first version started its rays at reference-node height, which on a BX is 2 to 14 cm above the road. They hit the forecourt surface, and the car stopped and reversed every metre or two until it ran out of manoeuvres. The player felt this as "very jerky".
  - A level ray also stops at the foot of a driveway ramp: Italy's forecourt entry rises 0.36 m in 1.5 m.
  - Now each of three origins (both corners and the middle) finds the ground under it. Walls and poles are a level ray at 0.8 m. Anything lower must show as a step of more than 0.18 m between ground samples 0.25 m apart.
  - A hit must be seen for 0.3 s before it counts. The look-ahead is off within 1.5 m of the stop, where the pump is beside the car by design.
  - `beamtelFuelPark.lookAhead(sign, reach)` answers it live.
- **Outputs are slewed.** Steering moves at most 1.5 lock per second. Throttle and brake ease on (0.8/s and 1.5/s) but come off at once. Braking into a reverse is scaled to speed (0.2 + 0.2 × speed, at most 0.6) instead of a flat 0.6.

**Stopping in arcade: the runaway this rule exists for**
- **The trap:** in arcade the brake pedal at a standstill is REVERSE THROTTLE, and the throttle while rolling backwards is the brake. The first "brake until stopped" step held the brake on a car that was already stopped.
- **What happened:** a car facing away from the pump (aim 140°) entered the manoeuvre at rest. It reversed away at 2, 4, 6, then 7.6 m/s on full lock, never satisfied "stopped", and crashed.
- **The rule:** every stop goes through `brakeToStop`. Brake while rolling forwards, throttle while rolling backwards, parking brake at a standstill. The pedal is held at rest only in the deliberate `reverse` phase.
- **Hard ceilings for the whole session:** 3 m/s forwards, 1.8 m/s backwards (`MAX_FORWARD_MPS`, `MAX_REVERSE_MPS`). Past either, parking stops the car and hands back.
- **Damage counts increases only.** The game's counter went DOWN by 2855 during that crash, so "damage now minus damage at the start" never rose.

**Ending**
- **Normal end:** a hold of 0.5 s at the stop, then `stopped`. GE runs the overlap test, calls `gameplay_markerInteraction.setForceReevaluateOpenPrompt()` (the stock prompt only opens on the transition into parking speed), and sends `parked` with `inZone`.
- **Failure:** brakes to a halt first, then hands back.
- **The player's brake (over 0.5)** hands back at once.
- **Failure triggers:** no progress for 15 s, 120 s in total, lease lost (3 s without GE's heartbeat), or a fault.
- GE ends the session on `PARK_CANCEL` or `CANCEL`, a vehicle switch or reset, a level change, or unload.
- The driving assistant is switched off first if it owns the car, because it holds the steering source.

Cross-VM chunks: GE→vehicle sends `start(token, {{x=,y=,z=},…}, beyond)` built with `%.3f` formatting. The resulting chunk holds no percent sign and no doubled bracket.

## Events (`FS_EVENT:{kind,…}`)

| Kind | Meaning |
|---|---|
| `scan_done` | The catalogue was rebuilt. Python forgets its target, because a new catalogue means a new level. |
| `target` | `id`, `station`, `here` |
| `in_range`, `left_range` | The range watch |
| `parking` | `pump`, `side` (pump on your left or right), `official` |
| `parked` | `inZone`, `electric`, `official` |
| `park_refused`, `park_failed`, `park_cancelled`, `cancelled` | `message` |
| `status`, `error` | Diagnostics and faults |

## Diagnostics

- `extensions.fuelStations.diag()` returns catalogue counts, the last plan (path, start pose, stop, heading), candidate rejection reasons and the last end reason.
- `extensions.beamtelFuelPark.diagnosticState()` in the vehicle VM returns the phase, manoeuvre count and a once-a-second trace of centre, steering, remaining distance, speed and aim angle.
- `extensions.fuelStations.placements(pump, halfLength, halfWidth, halfHeight)` and `.classifyShape(shapeName)` are exported for live checks through the MCP `lua_exec`.

## Verification

There is no standalone sim yet. The planner and controller need the scene (raycasts, `overlapsOBB_OBB`), and the standalone console cannot open mod files by relative path. Instead:
- check `classifyShape` live against the stock shape names (16 cases, including every false positive above);
- dry-run `placements` over every station on the loaded map;
- drive `TARGET`/`PARK` on 4488 while reading the two diagnostics.

`diagnostic/vehicle_geometry_sim.lua` scenario 12 lists `fuelStations` among the listeners.
