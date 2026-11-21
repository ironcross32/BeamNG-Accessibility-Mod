# Private roads and gates

Covers `roadDetector.lua`'s private-road handling, `roadGates.lua`, and the speech built on them (`beam-engine`, `beam-road` `road_guidance.rs`, `beamtelAssistant.lua`). Sims: `driving_assistant_sim.lua`, `road_detector_sim.lua`, `junction_proximity_sim` (`beam-audio`, `beam-road`), `road_guidance_sim` (`beam-road`).

## What the game's "gated" flag means, 2026-11-13

The one authored field is the DecalRoad / `map.json` checkbox `gatedRoad`. It is **not a gate**:

- `map.lua` turns it into `type = 'private'` on every link of that road (`roadType = road.gatedRoad and 'private' or road.type`). Traffic does not spawn there (`trafficUtils.lua`, unless `usePrivateRoads`).
- `processPrivateRoads` then sets a link-level `gatedRoad = true` where private meets public. When a node has exactly one public link, the flag goes on that **public** link. Routing adds 10000 to it: a cost, not a block.
- Traffic exclusion zones (`map.setTrafficExclusionZones`, rally and mission zones) set `type = 'private'` inside and `gated = true` on the boundary. `originalType` keeps the old type but is nil when the old type was nil, so it does not reliably tell a zone from an authored road; they are treated alike.
- The editor's own tooltip for the field reads "Road is open to the public.", the opposite of what it does.

An audit of all 21 stock levels (`reports/BeamNG gated road audit.md`, per-road CSVs in `research_notes/BeamNG gated road audit/`) found 2,053 marked navgraph roads: 105 (5 percent) with a gate, boom or closure near them, 1,360 private ground with no gate, 518 open roads or trails, 70 unsettled. Johnson Valley marks 521 of its 533 roads. Real gates also stand on unmarked roads (Hirochi's boom gates, Industrial's waste station, Italy's airport). The third driven Johnson Valley run had already found none of its 166 gated links blocked.

## How the mod reads it

- **`edge.private`** is `type == 'private'` (plus the legacy keys `private`, `isPrivate`, `privateRoad` for modded maps). It no longer reads `gatedRoad`/`gated`: before, it did, and missed `type`, so it marked exactly the boundary links and nothing else.
- **`edge.boundary`** is the link-level `gatedRoad`/`gated`/`isGated`. It is used only for the route search's `GATE_COST_M`, mirroring the game. It is never spoken.
- **`edge.gates`** comes from `roadGates.lua`: a gate mesh within 6 m of the link's centreline. This is the only thing called a gate.

## The gate index (`roadGates.lua`)

A plain module required by `roadDetector`. `rebuildNavigationModel` calls `roadGates.begin()`; `onUpdate` steps the scan 2000 TSStatics a tick (the `fuelStations.lua` pattern) and, when it ends, `attach` puts each gate on the one nearest link within `ROAD_M` (6 m) horizontally and `DZ_M` (4 m) vertically, as `{along, id}`. Gates within `MERGE_M` (8 m) of each other on a link are one gate: a left and right leaf, or a boom and its partner.

- **Vocabulary** (`M.WORDS`): `boomgate`, `boom_gate`, `pole_boom`, `metal_gate`, `fence_gate`, `swing_gate`, `sliding_gate`, `weight_station_barrier`. Excluded: `pillar`, `sign`, `corrugated`, `boom_lift`. Guardhouses and control booths stand beside the lane; police barriers and closure signs are not gates. None of them count.
- **Fence runs are dropped**: more than `FENCE_RUN_MAX` (3) of one gate mesh within 15 m. Johnson Valley's settlement fence is built of `ind_mesh_fence_gate_3m` panels.
- **Limits.** Gate meshes are never opened, so open or closed is unknown; the wording is "Gate ahead", never "closed". Orientation is not read: a gate origin 6 m off a narrow link may stand beside it. Farm gates placed as forest items are not TSStatics and are missed; the audit found them all beside the road. Not yet checked in game: whether 6 m fires on Italy's kerbside gates or on the Johnson Valley gas-station panels (5-8 m).

## Speech and bars

| Where | Gate mesh | Public road onto private |
|---|---|---|
| Assistant route (`routeStopAhead`) | Bar **at the gate itself**, `kind = 'gate'`: "Gate ahead. Stop at the bar" | No bar. "Private road ahead", once per entry (`privateEntry` tail) |
| Assistant junction prompt (`exitAccess`) | "right through a gate" | "right onto a private road" |
| Manual guidance (`findJunctionAhead`) | "Gate in N" (mid-link gate, or one within 15 m past a dead end) | "Private road in N" where the public road only goes on as private |

- **The private entry rides on the next stop** when that stop is within `PRIVATE_ENTRY_CARRY_M` (40 m) past it: the stop keeps its own kind and bar and carries `privateEntry`. Otherwise the entry is its own target, `kind = 'privateEntry'`, which `update_junction_proximity` drops, so it sounds no bar. `privateEntry` is an optional appended wire tail; an older receiver ignores it.
- **Private-to-private is never named.** The old "a gate between two private roads gets its own bar" rule fired on boundary artefacts (Johnson Valley's DR155) and is gone. Inside Johnson Valley no exit is labelled, which is what the report's "say *leaving public roads* once" rule wanted, without a level-wide share.
- **The bar no longer lands one link early.** It used to sit wherever the route stepped onto a boundary link, which was sometimes the public road before the entrance.

## Manual guidance legality

`legalRecord(record, permissive, fromPrivate)` hides a private exit only when the car comes from a public road (and "Include private roads" is off). Once on private ground every way on is listed: with `type` now read, a strict rule would make every Johnson Valley junction a dead end. `signalRoadCandidates` no longer filters private links, because it matches the road the car is on.

"Include private roads" (`road_include_private`, was "Include private and gated roads") now hides what it says. Before, it hid only boundary links.

## Not yet driven

None of this has been heard in the car. Worth driving first: the West Coast USA paved false positives (`520c3833`, `d2daa66a`, `cd053bbf`, `7613e0a8`), which should now say only "Private road ahead"; a Hirochi boom-gate road, which should get the gate bar although unmarked; and a Johnson Valley highway exit into the valley.
