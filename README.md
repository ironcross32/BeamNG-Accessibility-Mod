# BEAM Accessibility Mod

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

**Telemetry readouts:**

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

**Message buffer:**

- 1-9: Speak the 1st through 9th most recent message
- 0: Speak the 10th most recent message

**Waypoints:**

- Shift+C: Mark waypoint at current position
- Alt+C: Speak marked waypoint coordinates
- W: Distance and bearing to waypoint

**Vehicle scanner:**

- Ctrl+V: Toggle vehicle scanner
- Tab: Next scanner target
- Shift+Tab: Previous scanner target
- D: Scanner distance and orientation (approaching, angling toward, broadside, angling away, departing)
- Shift+D: Scanner relative bearing to target
- Shift+V: Align to trailer coupler — vehicle scanner must be on, target must be a trailer, couplers on source vehicle and target must be compatible
- Ctrl+Shift+D: Automatic coupler distance callouts — vehicle scanner must be on

**Toggleable modes:**

- Ctrl+S: Toggle status mode
- Ctrl+B: Toggle buffer mode
- Ctrl+C: Toggle pedal tones
- Ctrl+H: Toggle heading guidance
- Ctrl+D: Toggle drift detection
- Ctrl+Shift+L: Toggle low speed detection
- Ctrl+O: Toggle obstacle detection
- Ctrl+G: Toggle coordinate guidance
- Ctrl+Shift+C: Clickspot detection
- Ctrl+N: Accessible node grabber

**Camera info (free camera):**

- Alt+F: Toggle camera info
- Alt+H: Camera heading
- Alt+A: Camera altitude
- Alt+P: Camera pitch
- Alt+V: Vehicle bearing from camera
- Alt+D: Vehicle distance from camera

**Diagnostics:**

- M: Damage report
- Shift+E: Dump electrics to log
- Shift+P: Dump powertrain to log
- Shift+H: Dump hydros to log
- Ctrl+L: DOM dump
- Ctrl+Shift+S: Toggle speech logger

**Other:**

- Space: Activate context action
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
- W: Open the placement wizard for the selected item — choose which side to spawn it relative to a reference vehicle (front, back, left, right), then set the distance in 5-foot steps, then pick whether to use the current player vehicle or mark a specific vehicle as the reference point
- Space: Spawn all queued vehicles immediately

#### Manage page (in-world vehicles)

- Enter: Toggle selection of the highlighted vehicle
- Ctrl+A: Select all in-world vehicles
- Delete: Delete all selected vehicles
- R: Reload all selected vehicles (replaces each with a fresh copy at the same position)
- V: Cut ignition on every vehicle in the world

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

**Accessible node grabber** announces node names as the mouse pointer enters them and jumps the pointer to the closest node relative to the camera's center. Holding Ctrl allows free mouse movement. Scrolling the mouse wheel announces the grabber's strength — higher strength can lift a vehicle by a node, or tear a part free if it cannot bear the weight. Middle-click pins or unpins a node; a pinned node is fixed in place, which may prevent driving or tear off the associated part. To exert force on a node: hold Ctrl, move to the desired node, then click and hold the left mouse button — Ctrl can then be released. Middle-click while holding the left button to pin or unpin.

### Why Admin?

The mod asks for elevated privileges because the layered commands need to suppress their keystrokes from reaching the game. Without this, some commands would have adverse effects — for example, pressing F9 then F to check fuel would also exit the vehicle if it were stationary.

### Known Issues

- Some metrics in status mode never update (clutch temp, tire temps, brake temps)
- Device following may or may not work for some users
- Automatic trailer alignment does not work for fifth wheel-equipped vehicles and their associated trailers

### Configuration

When you launch the mod for the first time, it will write a configuration file with default values to `%localappdata%\beamtel\beamtel_config.json`. A SAPI engine cache will also be written there after the first run. The configuration file can be edited manually, or via the GUI configuration tool on the Configuration tab of the user interface.
