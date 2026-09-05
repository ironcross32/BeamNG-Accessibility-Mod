# Road-guidance driving diagnostics

The road diagnostic recorder captures the decisions behind the lane-correction sounds while
the vehicle is being driven. It is deliberately opt-in: ordinary `R2` road packets remain
compact, and `DIAG_ON` adds the detailed fields only after Python has opened a recording.

Recordings are newline-delimited JSON under
`%LOCALAPPDATA%/beamtel/road_diagnostics/`. Every sample is flushed immediately, so a crash or
loss of control cannot erase the part of the attempt that led to it. Stopping normally also
writes a matching `.summary.json` file.

The MCP `road_diagnostic` tool is the control surface:

- `start` opens a recording and asks `roadDetector.lua` for its detailed feed.
- `status` reports the active path, elapsed time, and sample count.
- `mark` adds a timestamped observation without interrupting the recording.
- `stop` closes the file and returns the full computed review.
- `review` recomputes the review for the active, latest, or named session.
- `list` enumerates previous sessions.

Each 20 Hz sample pairs the Lua decision state with the closest Python telemetry snapshot.
The Lua half records the selected navigation edge, road radius, signed lateral position and
filtered lateral velocity, prediction horizon and predicted position, target lane side,
target offset/error/tolerance, heading error, correction bearing, boundary/target times, the
three individual settled predicates, correction rearm state, and the consecutive-clear
counter. During diagnostics the Lua side also samples raw/filtered steering and wheel contact
material IDs/names directly from the vehicle VM, because legacy OutGauge has no steering
channel. Python adds world position, vehicle and ground speed, throttle, brake, heading,
pitch/roll, gear, traction-control state, and the wheel/ground speed divergence used to
identify slip.

Lane correction uses perpendicular offset from the road tangent. In particular, travel past a
clamped navigation-edge endpoint is not counted as lateral departure. Settlement uses a
15-percent-radius target band and a speed-scaled lateral-velocity tolerance from 0.45 to
0.75 m/s. Three consecutive 50 ms samples are required. Once settled, correction remains
latched off until both entry predictors have stayed safely below their hysteresis thresholds
for ten samples; the audio renderer also rejects settled-tone duplicates for two seconds.

The review groups correction and sustained-slip episodes and reports:

- how often each settled predicate passed and how often all three passed together;
- the longest consecutive settled-candidate run (three 50 ms samples are currently required);
- every actual settled waveform scheduling event and any pair less than three seconds apart;
- applied-steering reversals, correction-bearing reversals, and lane-target crossings during
  recovery (raw `steeringInput` is retained separately but is not used for reversal counting);
- wheelspin/lockup episodes and whether they overlapped an active correction;
- wheel contact-material sets and every material-set transition;
- maximum target error, lateral speed, heading error, correction bearing, steering input, and
  wheel/ground-speed divergence.

This separation matters when interpreting an attempt. A low lateral-position pass rate means
the target band itself is hard to reach; a low lateral-speed pass rate with repeated target
crossings points toward driver/system oscillation; a low heading pass rate points toward the
look-ahead or road tangent; and repeated settled waveform events prove state re-entry rather
than an auditory misidentification.
