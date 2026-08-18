# BEAM Accessibility Mod


### Purpose

Provide screen reader accessibility for BeamNG.drive through use of telemetry and a UI app that hooks in and fetches strings to be spoken.

### Use of AI Disclosure

Use of AI coding agents and tooling have been heavily used in this project.

### Installation Instructions

1. Unzip the contents to a directory of your choice (If you're seeing this, hopefully you have already done this)
2. Launch beamtel.exe - granting admin access (explanation in later section)
3. Navigate to the "Install Mod" button and activate it which will copy the zipped BeamNG mod to the appropriate place
4. Launch the game and enter a free roam session in the map of your choice
5. (optionally) get Xinyi OCR as it helps with the next steps
6. Press escape in the game, then scan for, route mouse to and click UI apps (normal NVDA OCR tends to say UL apps)
7. Scan for, route mouse to and click "Add app" (normal NVDA scan tends to only have one p)
8. Find nvda hook, route and click. This may be difficult due to the amount of apps available

At this point, you should have speech in game menus, as well as when changing gears.

### How to Use

The mod speaks speed announcements every 25mph or 25kph. It speaks your heading every 45 degrees and plays compass clicks every 15 degrees. The higher the pitch of the click, the closer to north you are. It also automatically announces gear changes.

You have access to most game screens, even if in some of them it's rudimentary. You can change vehicles, add or remove parts, and tune them with this mod. You can also change your map accessibly, though there is, and likely never will be, main menu reading for technical reasons. You will always have to OCR your way into the world, but once done, you'll be able to change maps with speech.

### Input Help Mode

Press ? (Shift+/) in either the F9 or F10 layer to toggle input help mode. While on, the layer stays open and every key press speaks what that command does instead of executing it. Press ? again to turn it off and exit the layer.

### F9 Layer (Vehicle Commands)

Press F9, then one of the following keys. If you press an invalid key or allow the command to time out, you'll hear "Exit".

#### Telemetry readouts:

- S: Speak speed
- R: Speak RPM
- Shift+R: Speak redline RPM
- G: Speak gear
- F: Speak fuel
- E: Speak engine temperature
- O: Speak oil temperature
- T: Speak turbo pressure
- Shift+T: Speak max turbo pressure
- P: Speak air pressure (pneumatic vehicles only)
- H: Speak heading
- A: Speak attitude (roll and pitch)
- C: Speak coordinates
- U: Switch between imperial and metric
- B: Browse the current vehicle's special controls (the "Special Vehicle Keys" list, e.g. Toggle Lightbar, Toggle Tailgate) with the keys and controller buttons they are bound to. Arrow up and down through the list, Enter to fire the highlighted action, Escape to close.

#### Message buffer:

- 1-9: Speak the 1st through 9th most recent message
- 0: Speak the 10th most recent message

#### Waypoints:

- Shift+C: Mark waypoint at current position
- Alt+C: Speak marked waypoint coordinates
- W: Distance and bearing to waypoint

#### Vehicle scanner:

- Ctrl+V: Toggle vehicle scanner
- Tab: Next scanner target
- Shift+Tab: Previous scanner target
- D: Scanner distance and orientation (approaching, angling toward, broadside, angling away, departing)
- Shift+D: Scanner relative bearing to target
- Shift+V: Align to trailer coupler — vehicle scanner must be on, target must be a trailer, couplers on source vehicle and target must be compatible
- Ctrl+Shift+D: Automatic coupler distance callouts — vehicle scanner must be on

In reverse, the scanner measures from the **back** of your vehicle and reports the bearing relative to the way you are travelling, so a target directly behind you reads straight ahead rather than straight behind. Left and right are unchanged — they stay the left and right you are sitting in, in every gear. This applies to any vehicle, not just the loader, so backing up to a wall, a trailer or a parking space now gives you a real bumper-to-surface distance.

Scanner distances are the **gap** between the two vehicles — the clear space you still have — not the distance between their centres. Earlier versions measured centre to centre, which added roughly half a vehicle at each end and, worse, changed with the target's orientation: a car sitting broadside and the same car nose-on at the same real gap reported the same number. Expect the figures to read smaller than they used to, and to reach zero when you touch.

#### Alignment and cannon readout:

- Ctrl+I: Toggle the docking instrument
- I: Speak the alignment readout — reference band, how far to raise or lower, how far left or right, range, and how square you are to the face
- Shift+I: Cycle the reference band
- In the Old Cannon, I reports live barrel elevation and the solution for the scanner target

#### Toggleable modes:

- Ctrl+S: Toggle status mode
- Ctrl+B: Toggle buffer mode
- Ctrl+C: Toggle pedal tones
- Ctrl+H: Toggle heading guidance
- Ctrl+D: Toggle drift detection
- Ctrl+Shift+L: Toggle low speed detection
- Ctrl+K: Toggle wheel slip detection (lockup / wheelspin)
- Ctrl+O: Toggle obstacle detection
- Ctrl+G: Toggle coordinate guidance
- Ctrl+Shift+C: Clickspot detection
- Ctrl+N: Accessible node grabber

#### Camera info (free camera):

- Alt+F: Toggle camera info
- Alt+H: Camera heading
- Alt+A: Camera altitude — height above whatever is directly below the camera, or, when there is nothing below it at all, height above sea level (said explicitly)
- Alt+P: Camera pitch
- Alt+V: Vehicle bearing from camera
- Alt+D: Vehicle distance from camera
- Alt+Shift+A: Camera and ground diagnostic — speaks the camera height, the ground height under it, and the difference, and writes the full detail (camera position, active camera, and every ground query the mod can make) to `bnvdahook.log`. Works whether or not camera info is switched on.

If the game stops sending camera data — which happens when the Lua side reloads — these readouts say "Camera info is not updating, reconnecting" and ask for the feed again, rather than repeating the last value they received.

#### Diagnostics:

- M: Damage report
- Shift+E: Dump electrics to log
- Shift+P: Dump powertrain to log
- Shift+H: Dump hydros to log
- Ctrl+L: DOM dump
- Ctrl+Shift+S: Toggle speech logger

#### Other:

- Space: Activate context action)
- Ctrl+Shift+Alt+C: When clickspot detection is on, open a browsable menu of all clickspots; arrows to navigate, Enter to activate
- ?: Toggle input help mode

### F10 Layer (AI Control)

Press F10, then one of the following keys.

- D: Disable AI
- T: Traffic mode
- R: Random mode
- S: Stop AI
- C: Chase mode
- F: Follow mode
- E: Flee mode
- 1-9: Set aggression level (0.2 to 2.0)
- +: Increase speed limit by 10 km/h
- -: Decrease speed limit by 10 km/h
- 0: Clear speed limit
- A: Cycle avoid cars mode (auto/on/off)
- L: Toggle lane driving
- ?: Toggle input help mode

### F11 Layer (Vehicle Spawner)

Press F11 to open the accessible vehicle spawner. F11 is suppressed while the spawner is open so the game's world editor cannot activate. The spawner has four pages: the main vehicle browser, the to-be-spawned list, the manage screen, and the arrangement presets screen. Tab cycles forward through pages and Shift+Tab cycles backward.

#### Navigation (all pages)

- Up/Down: Move between items
- Home/End: Jump to first or last item
- Page Up/Page Down: Jump 20 items
- Escape: Back up one level or close the spawner
- F11: Toggle the spawner open or closed, remembering position

#### Main page (vehicle browser)

- Left/Right: Drill into or back out of a vehicle's configuration list
- Enter: Queue the selected vehicle or configuration for spawning
- Shift+Enter: On a configuration, queue it as a replacement for an existing in-world vehicle (opens a slot picker)
- R: Add a random vehicle to the queue
- F: Open the filter dialog — Space toggles a filter on or off, Enter confirms, Escape reverts
- C: Clear all active filters
- Space: Spawn all queued vehicles immediately

#### To-be-spawned list

- Delete: Remove the selected item from the queue
- X: Toggle the selected item between spawning as a new vehicle and replacing an existing one in place
- W: Open the placement wizard for the selected item — first pick the anchor (the current player vehicle, a marked vehicle, or the previous/next item in the queue), then position the vehicle in the 3D editor described below
- Space: Spawn all queued vehicles immediately

#### Manage page (in-world vehicles)

- Enter: Toggle selection of the highlighted vehicle
- Ctrl+A: Select all in-world vehicles
- Delete: Delete all selected vehicles
- R: Reload all selected vehicles (replaces each with a fresh copy at the same position)
- V: Cut ignition on every vehicle in the world
- W: Open the placement wizard for the highlighted vehicle and move it there — first pick the anchor, then position it in the 3D editor described below. The anchor can be the vehicle itself (offsets then read as "move it this far from where it is now"), the ground below the camera (the spot you are looking at, facing the way the camera faces — useful for dropping a car exactly where you are), or any marked vehicle. The editor starts dead on the anchor, so accepting it without moving anything puts the vehicle exactly on target. Damage is preserved either way; the vehicle is moved, not respawned.

#### Placement editor (opened with W from the to-be-spawned or manage page)

The editor works in the anchor's own frame, so "forward" means the way the anchor is facing.

- Up/Down: Move forward or back; Left/Right: move left or right; Page Up/Page Down: raise or lower
- W/S: Pitch the nose down or up; A/D: roll left or right; Q/E: yaw left or right
- Space: Speak the current position; R: Reset to sit right on the anchor
- X: On a teleport, switch between standard and force mode (see below)
- Enter: Accept (queues the placement, or moves the vehicle); Escape: Discard

When moving an in-world vehicle, X chooses how it gets there. **Standard** sets it down on the spot exactly. **Force** throws it instead: the game works out the launch velocity that lands it on the mark and flings it, the same way the fling and boost cheats do. It arcs high enough to clear fences and parked cars, announces its launch speed and flight time, and lands approximately — not exactly — on target, taking whatever damage the landing does and ending up facing whichever way it tumbles. Rotation is ignored in force mode for that reason. A target too far away to reach is launched at full speed and announced as landing short. Once the vehicle comes to rest, the mod announces where it actually ended up — "landed on target", or how far short, long, left or right of the mark it finished.

A quick tap moves exactly one foot or one degree. Holding a key escalates the step the longer it is held — 1, 10, 100, 1000, 10000 feet, or 1, 10, 45 degrees. Each step plays a ping whose timbre identifies the magnitude, spatialized in the direction of travel; height and pitch changes are pitch-shifted up or down instead. Holding two arrows moves along the diagonal between them, and after a short pause the full position is spoken automatically.

#### Arrangement presets screen (opened with G from any page)

- Up/Down: Move between rows — Type, Variant, Spacing, Apply Queue, Arrange Active
- Left/Right: Change the value of the current row
- Enter: Activate Apply Queue (spawns queued vehicles in the chosen arrangement) or Arrange Active (teleports in-world vehicles into the arrangement)
- Escape: Return to the previous page

Arrangement types are: line (all vehicles in a row), side by side (abreast), two columns, three columns, and boxed in (surrounding a center vehicle). Each type has a variant that controls where the reference vehicle sits within the arrangement (front, middle, or back/left/right for side-by-side). Spacing is set in feet.

### Special Modes

**Pedal tone mode** allows you to hear tones that identify how much input the game is receiving from you. You'll hear three tones: one near the center for brake, one at the right for gas, and one at the left for clutch. If the vehicle is automatic or DCT, the clutch tone will never play, even if the input is actuated. The tones also require input of each type before they will begin.

**Status mode** commandeers the arrow keys for itself. Up and down step through each telemetry item; left or right give an up-to-date readout of the currently selected item. This mode must be turned off in order to use a screen reader effectively. It must also not be used if you play with a keyboard, because you will not be able to accelerate, brake, or steer.

**Buffer mode** commandeers the bracket keys for itself. These step through the 100 most recent messages. Left bracket navigates backwards through the buffer (older messages), and right bracket moves forward (newer messages).

**Clickspot detection** looks for interactables on the interior or exterior of a vehicle — buttons, switches, levers, latches, etc. Press F9 then Ctrl+Shift+C to enable it; this will announce how many clickspots were found. Clickspots are then discoverable with the mouse: a beep sounds when the pointer enters one, and a reverse beep when it leaves. Click the mouse to perform the associated action. A menu-driven approach can also be activated with F9 then Ctrl+Shift+Alt+C, opening a virtual browser where you can move through all clickspots with the arrow keys and press Enter to activate one.

**Loader implement awareness** covers the bucket and forks on machines like the WL-40 wheel loader. Nothing needs turning on and there is no keybind: it appears by itself on a machine that has an implement, and is completely inert on every ordinary vehicle.

Status mode gains three extra items on such a machine — implement height above the ground beneath it, implement tilt in degrees from level, and the frame articulation angle (the same bend that drives the articulation tone).

Two tones report the implement continuously. A rough FM tone reports the ground: it is clean and quiet a couple of metres up, and grows progressively grittier the closer the bucket or forks get to the ground. If you curl into the ground hard enough to lever the machine up off its front wheels, that tone cleans up again and rises in pitch with how much lift you are getting. A second tone reports tilt on a quarter-tone scale — 400 Hz is dead level, falling to 200 Hz at the full forward dump and rising to 800 Hz at the full curl back, in countable quarter-tone steps. Both tones fade away about a second after you stop moving the implement and snap straight back when you touch the controls again; the ground tone also comes back on its own if you are driving toward something rather than lowering onto it.

Speech announces vehicles and props the implement is approaching, by name, along with whether the bucket or forks are above, below or level with them. It announces again when the relationship changes — for instance when raising the forks clears the roofline — when the tines slide underneath something ready to lift it, and when you leave the area. Parts that have broken off a vehicle are detected in their own right, so a bumper lying in the dirt is not invisible.

**The docking instrument** is for the last couple of metres, when you are lining the tines up with a pallet pocket or squaring the bucket against a car's flank. Turn it on with F9 then Ctrl+I; it is off by default because it is a mode you enter deliberately to line something up, not ambient awareness.

It works from a **reference band**. The mod reads the vertical profile of whatever you are closest to and splits it into alternating solid runs and voids — the air under a car's rocker, the sill, the door, the glass, the roofline — then picks one for you: the lowest real void if you are carrying forks, the tallest solid face if you are carrying a bucket. F9 then Shift+I steps to a different one and says which it is, so the readout always tells you what it is measuring against. It is never a guess you have to reverse-engineer.

Two tones run while it is on, and it is quieter overall than having it off: the vehicle scanner is silenced and the articulation tone is ducked hard for as long as it is open. That is deliberate. The articulation tone tells you what your steering input did; the docking instrument tells you where you need to be, and on a final approach at walking pace those are the same thing said twice. The articulation centre click still fires, so you have not lost the frame reference.

A pulse carries two of the three axes: it sits left or right in the stereo image according to which way the target is off your centreline, and it speeds up as you close. Inside two metres a second voice fades in — a pair of tones that beat against each other. The further the implement is from the reference band, the rougher the beating; as you close on it the beating slows, and at alignment the two tones fall into unison and the sound goes smooth. A short descending chime marks the moment you arrive. A slow tremolo tells you which way you are out — faster means you are above the band and should come down, slower means you are below it and should come up — and it stops at the null, so a steady, smooth unison means stop.

Nothing plays past five metres. From five to two you get only the pulse, because at four metres the height does not matter yet and you are still steering.

F9 then I speaks the whole picture in one go: which band, how tall it is, how far to raise or lower, how far left or right, the range, and — only when it is bad enough to jam the tines — how far off square you are to the face. Think of it as a cane tap rather than a continuous field: one deliberate press, one complete answer, silence in between.

The same toggle automatically switches to **ramp alignment** when an ordinary vehicle is approaching a drive-in ramp. While you are outside the approach corridor, the pulse is a beacon: its position is the true bearing to the ramp mouth and its rate reports straight-line distance. The approach handoff begins within 6 m (about 20 ft) of the centreline, with a 9 m exit boundary to prevent chatter. Crossing onto the approach line sounds a centred double pip; that handoff means the continuous cues now describe corrections to the line rather than where to find the mouth. Leaving the corridor or losing the target resets the handoff, so each new approach gets one cue.

On the approach line, every continuous cue has one meaning. Pulse rate still reports distance. Pulse position reports lateral offset only, with offsets up to 15 cm rendered in the centre to agree with the F9+I “centred” readout; the remaining 0.15–3.0 m range is spread across 0–75 degrees. The beat pair reports heading error at 0.5 Hz per degree, reaching its 12 Hz ceiling at 24 degrees. Its tremolo gives the turn direction: slow means turn left and fast means turn right. F9+I always states heading on the approach, including the explicit phrase “heading zero degrees” below half a degree.

The descending alignment chime is a composite ramp lock, not a single-axis null. It sounds only after both lateral offset is at most 15 cm and heading error is at most 3 degrees. It re-arms only after lateral offset reaches 50 cm or heading error reaches 8 degrees. The lock latch is disabled during mouth acquisition, so entering the corridor already aligned produces the handoff double pip but no misleading second chime.

**Using the implement as a tool of destruction** works from the same toggle, because it is the same act approached from the other end — dropping a bucket on a car, or driving forks through a window, is still putting the implement somewhere specific relative to an object. But lining up to *lift* means driving a height error to zero, and lining up to *drop* means the opposite: getting decisively above the thing before you commit. So instead of a continuous tone you get three short cues as you pass through three states. A high tick means the underside of the implement is clear above the target. A lower tick means you are over its footprint looking straight down. When both are true you get a short rising two-note figure and the words "over it, clear" — that is the point at which dropping will land on it. The docking alignment chime falls where this one rises, so the two can never be mistaken for one another.

These states are coarse on purpose. Ramming is forgiving in a way that threading tines into a pallet pocket is not, and they hold once claimed, so idling with a raised bucket will not set them flickering.

**Old Cannon aiming** uses Page Up to raise the barrel and Page Down to lower it. F9 then I
reports the physical barrel elevation in degrees, measured from the live barrel rather than
from the input value. Turn on the vehicle scanner and select a vehicle or prop; its ordinary
spatial beep remains the horizontal cue.

The cannonball drops under gravity, so pointing directly at the target is not a firing
solution. Before firing, the mod predicts launch speed from the cannon's current Powder and
Weight configuration and calculates the low ballistic angle. A low centred pulse means raise
the barrel, a high centred pulse means lower it, and the pulses accelerate as the error grows.
Silence on that channel means the elevation is within half a degree; the scanner's bright
aligned beep then requires both bearing and elevation to be aligned. F9 then I also states the
calculated angle and correction. No calibration shot or reset is required.

**Accessible node grabber** announces node names as the mouse pointer enters them and jumps the pointer to the closest node relative to the camera's center. Holding Ctrl allows free mouse movement. Scrolling the mouse wheel announces the grabber's strength — higher strength can lift a vehicle by a node, or tear a part free if it cannot bear the weight. Middle-click pins or unpins a node; a pinned node is fixed in place, which may prevent driving or tear off the associated part. To exert force on a node: hold Ctrl, move to the desired node, then click and hold the left mouse button — Ctrl can then be released. Middle-click while holding the left button to pin or unpin.

### Why Admin?

The mod asks for elevated privileges because the layered commands need to suppress their keystrokes from reaching the game. Without this, some commands would have adverse effects — for example, pressing F9 then F to check fuel would also exit the vehicle if it were stationary.

### Known Issues

- Some metrics in status mode never update (clutch temp, tire temps, brake temps)
- Device following may or may not work for some users
- Automatic trailer alignment does not work for fifth wheel-equipped vehicles and their associated trailers

### Configuration

When you launch the mod for the first time, it will write a configuration file with default values to `%localappdata%\beamtel\beamtel_config.json`. A SAPI engine cache will also be written there after the first run. The configuration file can be edited manually, or via the GUI configuration tool on the Configuration tab of the user interface.
