# Map speed limit awareness

Validated on 2026-09-14 local time (2026-09-15 UTC).

## Data and behavior

BeamNG's live `tx_map` navigation graph has 3,725 cached edges, all with a speed limit. Values found were 30, 50, 60, 80, and 120 km/h. The parked player's segment (`DR33_11` / `DR33_12`) reports 8.3333333333333 m/s: 30 km/h, rounded to 19 mph in the user's imperial setting.

The installed `lua/ge/map.lua` function `createSpeedLimits` fills absent limits from road radii, drivability, and direction. Authored and generated values share `speedLimit`; provenance is not retained. [BeamNG's navigation-map documentation](https://documentation.beamng.com/modding/levels/level_formats/map/) also documents m/s and automatic calculation. Speech therefore says **Map speed limit**, without claiming to read a roadside sign or determine a safe turn speed.

The existing R2 packet gains optional positive finite `speedLimit` in m/s, only on road. Old packets remain accepted. Python uses a 0.75-second dwell for a changed limit, tolerates brief missing data, and announces sustained unknown data once after three seconds if a limit was previously known. Extended off-road travel silently clears announcement memory. Enabling announcements, reconnecting, or changing map/vehicle context rearms the current limit. Missing data on an unsupported map is quiet until queried.

Automatic speech is non-interrupting and enabled while either road detection or driving assistance is active. The default-on `road_speed_limit_speech_enabled` setting has a checkbox in Road guidance. F9, S reads current speed plus the fresh map limit regardless of the automatic speech setting; it reports unavailable if neither mode is enabled or data is missing/stale. The road-status readout and MCP road packets also include the limit. Assistant diagnostic road records retain the raw value.

## Validation

- Python compilation: `beamtel.py`, `road_guidance.py`, `assistant_diagnostics.py`, `configurator.py`, `config_ui.py`, and modified Python diagnostics.
- `diagnostic/speed_limit_sim.py`: protocol validation, imperial/metric wording, stable changes, edge flicker, brief/sustained missing data, config gating, stale reads, reconnects, and off-road reentry.
- Existing road guidance, traffic control, and junction proximity diagnostics pass.
- `diagnostic/speed_limit_sim.lua` in BeamNG: production scanner with isolated sockets and graph verifies values, changes, reverse-facing travel, missing/nonfinite/nonpositive values, and off-road suppression.
- Reloaded the production road scanner after focusing and verifying the actual BeamNG process, PID 8780. Fresh `beamng.log` records at 8684.81631 (cached edges) and 8684.81655 (`mapSpeedLimit` marker) confirm the load. Buffered log entries were flushed for inspection.
- Restarted the Python receiver. Live R2 UDP reported `speedLimit=8.3333333333333`. With road detection enabled, speech log sequence 7 recorded **Map speed limit 19 mph.** with `interrupt=false`; F9, S at sequence 8 recorded **0 mph. Map speed limit 19 mph.** Both had `spoken=true`.
- Restored road detection off and left steering assistance off. Saved the preceding reverse-drive recording and armed `20260915T005149-099241Z-map-speed-limit-drive.ndjson` for the next drive.

Actual driving across a map speed-limit boundary and the player's listening confirmation remain pending. The live parked check validates the receiver and speech path; it does not establish that the map limits match posted signs or are safe for every turn.
