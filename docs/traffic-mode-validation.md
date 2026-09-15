# Shared traffic notifications

Validated 2026-09-14 local time (2026-09-15 UTC).

Stop-sign and traffic-light speech now uses the shared enabled state: road guidance OR confirmed driving assistance, with loading suppression and each notification's own configuration preference. The shared road feed continues while either mode is active. Switching road guidance while assistance is active preserves signal announcement history; enabling manual guidance rearms orientation separately. The Lua scanner also retains its tracking context when manual guidance joins an already active assistant. Turning both modes off stops notifications. The road-status command can read traffic information with either mode active.

## Checks

- Python compilation passed for the edited receiver, guidance helper, settings UI, and diagnostic.
- `diagnostic/traffic_mode_sim.py` executes the actual receiver listener and road-mode toggle, using scripted packets. It verifies assistance only, both modes, both handoff directions, no duplicate announcement on handoff, stop-sign near reminders, light changes, both modes off, independent preferences, and loading suppression.
- Existing traffic-control, road-guidance, and map-speed-limit diagnostics passed.
- `diagnostic/traffic_mode_sim.lua` runs the production scanner in BeamNG with isolated sockets and graph. It confirms scanning and unchanged signal context through both handoff directions, and silence when both modes are off.
- Installed the scanner, restarted the receiver, and focused the actual BeamNG window (verified PID 8780). Fresh `beamng.log` marker `trafficModes` at 9210.26063 confirms the scanner reload.
- A stationary UDP replay on the restarted receiver recorded speech sequence 8 **Stop sign in 26 feet.**, sequence 9 **Traffic light red, in 26 feet.**, and sequence 10 **Traffic light green.**, all with `spoken=true`. Real road scanning resumed afterward and MCP returned the actual road packet and map speed limit.
- Restored road guidance and assistance off and rearmed assistant diagnostics for the next drive.

Mode handoffs were verified with the production code under simulation; the stationary live replay verified receiver speech delivery. Player-driven handoffs at a registered traffic control remain to be exercised. The current `tx_map` reports traffic-control support as unavailable, so the replay used synthetic registered-control packets; this change does not add recognition of decorative or unregistered signs and lights.
