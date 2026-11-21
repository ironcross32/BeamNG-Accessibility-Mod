# Driving assistant: following a map route

Added 2026-11-01. Files: `drivingAssistant.lua` (`syncRoute`, `arrive`), `roadDetector.lua` (`assistantSetDestination`, `assistantClearDestination`, `assistantGuidance`, `routeStopAhead`), `assistantRoutes.lua` (`search`), `beamtelAI.lua` (`onReachedTargetPos`), vehicle `beamtelAssistant.lua` (`replanRevision`), `beam-road` `assistant_route.rs`, `beam-road` `driving_assistant.rs`, `beam-engine` (`ui_listener`, `assistant_arrival`). Sims: `diagnostic/driving_assistant_sim.lua`, `assistant_route_sim` (`beam-road`).

With a route set on the big map, before switch-on or during the session, the AI chooses every turn and held steering selects nothing. The player keeps the pedals, as always. **Not yet driven:** the sims and a read-only search on the real map pass, but see "Still needs a driven run" below.

## What the game does on arrival

`gameplay/markerInteraction.lua` clears a route only when all of these hold: under 7 m of path remains, the route has `clearPathOnReachingTarget` (the big map sets it), the destination is a vec3, the big map is closed, no mission is in the foreground, and the state is freeroam or career. It then fires `onReachedTargetPos` and shows "You have reached your destination."

The 7 m **includes the final leg off the road**, because `route.lua` appends the destination after the last road node. So a destination more than 7 m from the road is never cleared by driving on the road. Left set, it would meet the assistant again on the next switch-on. The mod therefore clears it (`freeroam_bigMapMode.setNavFocus(nil)`, the same call stock arrival makes).

Only routes with `clearPathOnReachingTarget` are cleared. Career and delivery routes set it false, and they belong to those systems.

## The stock route supplies only the destination

The destination is `routePlanner.path[#path].pos`, for the reason `lua-route-beacon.md` gives. An empty path while the target is set means an async route is still being computed: that is neither a route nor a removal, so `liveDestination` reports it as pending.

The road to drive is searched here, because the stock node list can begin with a U-turn behind the car and can use the median gaps stock AI gives up on (see the 2026-10-27 run in `driving-assistant-validation.md`).

`assistantRoutes.search` is A* over directed road records. `routeNeighbours` keeps the assistant's rules: only one-way direction binds, no same-edge U-turn, nothing past 150 degrees, and at a branching node an exit whose `settledHeading` has turned round is rejected. Private and rough roads cost three times their length, and a link the map flags where private meets public (`edge.boundary`, not a gate: see [road-privacy-and-gates.md](road-privacy-and-gates.md)) 300 m, so they are used when needed rather than as shortcuts.

**Forward only is deliberate.** A car facing away from its destination goes round the block. The stock "Please make a U-turn when possible" toast is therefore wrong while following a route, and `suppressed_toast` drops it.

The search start is a **copy** of the last committed record, because the search closes records by identity and a route round the block has to be able to re-enter that same edge. Measured on Johnson Valley (7035 nodes): 22 of 22 searches succeeded, up to 5.8 km, slowest 54 ms.

## How the vehicle is kept out of it

`extendAssistantRoute` draws from `nav.records` up to the usual 800 m horizon and never sets `state.junction`. No junction reaches the vehicle, so its whole selection path stays idle. That is the mechanism, rather than a flag the vehicle has to honour. Two things follow:

- The turn is already in the AI's path, so its braking demand and the "Slow down" cue start earlier than at a player-chosen junction.
- A roundabout, refused when the player would have to name an exit, is simply driven as nodes.

`committedRecords` keeps the road within `max(60, speed * 4)` m when a route changes, so a turn the car can no longer make stays out of the new route. Stock `setPath` can only append, and a changed route would raise "Route extension is disconnected", so a change bumps `replanRevision` and the vehicle restarts its planner once. The first path needs no replan, which is what `syncRoute`'s `starting` argument is for.

## What is spoken

Lua sends metres, and `beam-road` `assistant_route.rs` decides wording and units, so `RouteCallouts` can be tested alone. `route` events arrive twice a second, and only a crossed threshold speaks.

- **Turns:** see "Rules from the map survey". Short links (35 m or less) that branch are one grouped junction and one turn. A one-way ring is one word, "roundabout", with no exit number.
- **Callouts:** imperial 2 miles, 1 mile, half a mile, 1000 feet, 500 feet. Metric 2 km, 1 km, 500 m, 200 m. A threshold within 20 percent of the distance just spoken is skipped, because it would only repeat it. "Turn left now" comes at `max(40 m, speed * 3 s)`.
- **Start-up speech waits for `on`**, which is spoken with interrupt and would cut it off. See `announce`.

**Stop bars on a route** mark a spoken turn, a real crossing road, a gate, a dead end, or the destination (`kind = 'destination'`). Signals are a separate target and always count in. Without a route every junction keeps its bar, since the player has to choose there.

## Turning round first

At switch-on, under 2 m/s, on a two-way road with a route set, `assistantTurnChoice` searches both ways. A three point turn runs if going forward costs at least 150 m more, or has no route at all, and there is at least 7 m across the car. So facing the wrong way at the end of a long block turns round, while Italy's port (71 m further going forward) just drives. It is not offered mid-session, on a one-way road, or at the join-stage hand-over.

**Stock AI cannot do it.** Its only reverse is a straight back-up after being stuck for 5 seconds (`crash.maneuver`). So the mod holds full lock and the player drives the legs, in `beamtelAssistant.turn`, which runs no AI at all. Forward on left lock and backward on right lock both swing the nose left, so `turn.side` alone fixes both.

`assistantRoutes.turnStep` is the pure state machine. A leg ends when the room runs out at `0.7 + 0.3 * speed + speed^2 / 6` m. It says "Stop. Reverse" or "Stop. Forward", repeats that every 2 s while the car still rolls the old way, and waits for the gear (or for motion that way, which covers arcade gearboxes). It ends within 25 degrees of the road's direction, in a forward gear, and hands to an ordinary start. "No room to turn round" and "Could not turn round" (9 legs or 180 s) fall back to the forward route.

**Room** (`roomFrom`) is the nearest of three things from each bumper: three static rays, a drop, and the road edge plus a 1.5 m verge. The drop check is downward rays, because Italy's port is a pier 17 m above the terrain and no forward ray sees its edge. Static rays do not see vehicles, so traffic is the player's to hear.

These instructions carry `data.protect`, which opens the speech-protection window. The heading changes constantly during a turn, and the compass would otherwise cut them off. They are plain `say` events, so an older receiver still speaks them.

## Rules from the map survey

The first driven run said "keep left/right" every 300 feet and never stopped counting in. Rather than tune to that one map, every stock map was surveyed. **No rule may use an absolute road class or name a map.** The survey scripts (`diagnostic/map_survey/`: `extract_roads.py`, `analyse_roads.py`, `route_rules.py`, `turnaround_survey.py`) were Python and went with the Python application; to rerun the survey on new maps, including mod maps, recover them from git history before "Remove the Python application".

The survey rebuilds each graph from the `DecalRoad` objects in the level zips, which is why it needs no level loaded. Against the live Johnson Valley graph it gave 85 km, 73.1 km private and 0.9 km one-way; the game has 85.1, 73.5 and 0.89. It uses control points rather than the game's subdivided edges, so node counts run lower and rates are for comparing rules, not exact. 13 of the 21 archives have a road network worth routing on. Measured with the real rule on the live Johnson Valley graph, over 30 routes and 68 km: 0.6 keeps, 2.1 turns and 3.6 bars per km, slowest search 72 ms.

What it found, and the rule each finding led to (`routeTurns`):

- **Road class only means something relatively.** `private` is 86 percent of Johnson Valley's road and 1 percent of Utah's; drivability is 1.0 everywhere on Gridmap and Driver Training and tops out at 0.9 on Johnson Valley. `lesserRoad` only ever compares two roads at one junction: private against public, or drivability or width under 70 percent.
- **A slip link beside a straight road is on every map,** outnumbering true forks by 3:1 to 8:1. Going straight past one is no instruction.
- **A bend is not a turn.** 15 to 40 percent of the spoken turns under the first rule were the road swinging round with nothing else near it, up to 2.6 per km. `roadGoesOn`: the route takes the best-aligned exit, within 45 degrees, at least 20 degrees better than any rival, and does not leave a bigger road. Then nothing is said, whatever the heading does afterwards.
- **"Keep"** needs the route itself to bend more than 10 degrees with a rival inside 45, or to be leaving a bigger road that bends away. Rates fell from 0.5-4.7 per km to about 0.2-0.85 on driving maps. Derby and Driver Training stay higher (about 2-3) because they are genuinely tight little networks.
- **A way round has to be big enough to drive.** Facing away from the destination at Italy's port, the search turned round on a 40 m two-way loop of 120 degree corners, then came back down the same road. Stock AI left the road every time at any speed, ending in "Steering lost". `tightLoop` rejects returning to a node left under 80 m ago, unless every edge of the loop is one-way, because a true roundabout is built to be driven. A corner-radius test was tried first and rejected: `turnaround_survey.py` shows control points make nearly every route look undrivable. The same survey shows up to 14 percent of road on tight maps has a tiny loop as its shortest way round, and on Italy 41 percent has no way round within 2 km, so "No forward route" is a real outcome. A route that does revisit a node, round a real block, is extended from the visit reached the same way (`endpointFrom`).
- **A crossroads gets a bar without the side links coming back:** a side road at 50-130 degrees, not a lesser road, running at least 60 m. That costs 0-1.2 bars per km against 0.8-5.5 for every side road. Gridmap is highest because it really is a grid.

## Arrival

At 7 m along the route, or on stock's hook. The hook is also what separates arrival from the player removing the route, since both leave no route on the next tick. A hook arrival is never spoken as off-road: the game has measured it as under 7 m away.

The `off` event carries `data.arrival` and keeps its plain message as the fallback for an older receiver. Off the road the sentence gives distance and bearing, positive left, and `assistant_arrival` copies the position to the marked waypoint so `F9 W` and `F9 Ctrl+G` carry on. This replaces any waypoint already marked. The route beacon cannot help here because it needs a route.

Steering is released on arrival even if the car is moving. The callouts and pips are the warning, and the sentence does not add "Brake now": stopping at a destination is what the driver is doing anyway. The same goes for "Go slowly" in reverse. "Brake now" stays on faults such as "Steering lost", where the release is a surprise. The disengage cue opens the existing speech-protection window (`assistant_cue`), because a compass heading landing 6 ms after the arrival sentence cut it off on the first driven arrival.

**No trap on the next switch-on.** If the car is within 7 m of a route's destination, the assistant says "Already at the destination", clears the route if it owns it and otherwise ignores that destination for the session, and carries on with ordinary junctions.

## When there is no route

A forward search can fail, for instance on a dead-end stretch or facing the wrong way on a one-way road. The assistant says so once and falls back to ordinary junctions. It retries after each committed choice and on returning to forward from reverse, so dead-end reverse guidance gets the car out and the route resumes.

The two-second throttle on searches exists for routes that keep moving, such as a mission's. Removing a route resets it, so a new route set straight afterwards is not delayed.

## Signals

Stop signs and traffic lights already report with the assistant on and road guidance off: `roadDetector.onUpdate` scans while the assistant is active, and `beam-engine` gates speech and stop-bar audio on `road_mode_active or driving_assistant.active`. The stop sign and traffic light speech settings still apply. Nothing needed changing.

## Still needs a driven run

- A POI route set before switch-on, with held steering ignored at junctions.
- A route set and cleared mid-drive, and how the planner restart feels at speed.
- A stop sign and a traffic light with road guidance off.
- Arrival on and off the road, then switching on again.
- A roundabout.
