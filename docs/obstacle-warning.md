# Predictive obstacle warning

Ctrl+O is the sole activation authority. Obstacle detection starts off on every launch; old
`obstacle_detection_enabled`, `obstacle_max_range_m`, and `obstacle_warning_range_m` values are
tolerated in existing configuration files but no longer control the detector.

## State and wire protocol

While the mode is active, Python sends driver intent to UDP 4453 on gear changes, material
steering/pedal changes, and at least every 0.5 seconds:

```text
STATE,<F|R>,<steering>,<throttle>,<brake>
```

Steering is normalized to -1..1 and positive means left. A pushed state expires after one
second. Lua then derives forward/reverse from longitudinal velocity, sets steering to zero,
and requires actual motion before warning. Python also sends `SENSITIVITY,early|normal|late`
when the mode starts or configuration changes.

Lua sends either `0`, the unchanged terrain packet, or one selected static hazard:

```text
1,1,<bearing>,<urgency>,<gap>,<state>,<closing>,<ttc>,<stoppingMargin>
```

The first five fields are a valid one-entry legacy packet. State is 1 advisory, 2 urgent, or
3 emergency; TTC is -1 when unavailable. Gap is clear air from the player's resolved vehicle
perimeter to the static hit, not distance from the vehicle origin. Stopping margin is gap minus
`closing² / (2 × 9 m/s²) + 0.3 m`. New Python still accepts old multi-entry packets, chooses
their highest urgency, and never infers emergency from their centre distances.

## Coverage and priority

Each sweep completes in about 0.3 seconds. Thirteen low/high fan pairs provide angular coverage:
at 3 m/s and above they cover ±42 degrees around a steering-shifted path centre in 7-degree
steps; below 3 m/s they cover ±78 degrees in 13-degree steps around the selected forward or
reverse end. Seven additional parallel low/high pairs span the complete swept collision corridor.
Those guard rays prevent narrow obstacles from hiding in the widening gaps between angular rays
at long range. Steering can move the centre by 20 degrees while driving or 45 degrees while
parking. Both heights must hit along the same path lane. Their distances need not match because
parallel probes meet slopes and irregular solid surfaces at different depths; the nearer hit is
the reported surface gap. A candidate must remain inside half the vehicle width plus 0.75 m from
the predicted centreline. `castRayStatic` means moving traffic remains outside this detector.

Driving ray reach is recomputed from speed and the selected advisory TTC, with 0.6 seconds of
sweep/reaction reserve; it is also never shorter than the emergency stopping-distance reach.
This prevents highway-speed casts from pinning at a short fixed range. Ctrl+L explicitly reloads
this manually managed extension, and the next active-mode `STATE` heartbeat restores its mode.
The clearance probes rise by only 0.15 m across their complete cast regardless of range; a fixed
angle would climb metres at predictive distances and pass over ordinary-height obstacles.

Receding and non-intersecting hits are silent. Emergency is the stopping-distance crossing.
Otherwise driving states use TTC and parking states use surface gap:

| Sensitivity | Driving advisory / urgent | Parking advisory / urgent |
|---|---:|---:|
| Early | 6.5 s / 2.5 s | 4.0 m / 2.0 m |
| Normal | 5.0 s / 2.0 s | 3.0 m / 1.5 m |
| Late | 3.5 s / 1.5 s | 2.0 m / 1.0 m |

Exactly one hazard wins by state, then TTC or gap, then proximity to the path centre. The
current target is retained until another reaches a higher state or is at least 15 percent more
urgent, and is cleared after two missed sweeps.

## Cue mapping

The cue is an 80 ms, peak-normalized 413.7 Hz pulse with its 2nd, 4th, 6th, 8th, and 10th
harmonics at `1/n`. Advisory is 6 dB below the configured Obstacle cue volume and runs at
0.7–1.5 pulses/s. Urgent is at configured level and 1.5–4 pulses/s. Emergency enters with two
full-level pulses separated by 100 ms, then holds the same spectrum 9 dB lower while closing.
The emergency entry is latched until clear or one second below urgent. Bearing uses HRTF when
available and stereo panning otherwise, with a 50 ms directional glide.

Urgent and emergency reduce scanner, road, docking, coordinate, and heading guidance by 6 dB
and 12 dB. Advisory changes no other audio. Safety warnings, drop-off/hill sweeps, and terrain
sonification are never ducked.
