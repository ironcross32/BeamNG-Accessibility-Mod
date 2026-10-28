# What's new in BEAM 0.2.0

This release covers everything since 0.1.9. The main additions are an experimental driving assistant, support for BeamNG.drive 0.39 and 0.40, and a range of new readouts and sounds.

---

## Driving assistant (experimental)

Press **F10, then H** to turn the driving assistant on or off. It is always off when BEAM starts. The assistant steers; you keep control of the throttle, brake, handbrake, clutch and gears, and you are responsible for speed, stopping and traffic lights.

- **Following the road:** the assistant follows drivable roads using the game's own lane driving.
- **Choosing a turn:** after a junction is announced, hold the steering left or right for one second. You'll hear "left selected. Release steering", and you can then centre the controls without losing the choice. About two seconds before the junction you'll hear "Turning left" or "Turning right". If the turn you asked for isn't possible, the assistant goes straight if it can, otherwise left, otherwise right.
- **Junction tones:** when a junction is announced, a tone plays on the left, centre and right for each exit that exists. A missing exit is silent, so a T junction sounds left, silence, right. Two descending tones mean the assistant has switched off, including when the connection is lost.
- **Leaving the road:** if you drift a little way off the road, you'll hear "Rejoining the road. Slow down", then "Back on the road" once you're back. If you go too far, or the assistant can't make progress, it switches off.
- **Obstacles:** "Obstacle ahead. Brake" warns you of something in the road. The assistant never brakes for you.
- **Reversing:** select reverse, or roll backwards, and the assistant steers you slowly back along the road you came from. Select a forward gear to carry on forwards.
- **One-way roads:** if you try to switch on facing the wrong way, it tells you how far and which way to turn.
- **When it switches off by itself:** at roundabouts and junctions it can't handle, when you change or reset the vehicle, when you give a normal AI command, and when the connection is lost.
- **Gears:** the assistant no longer causes gearbox mode messages, and a manual car stays in first gear when you switch it on.
- **Still being tested:** keyboard, gamepad, wheel and real-world driving tests are not finished yet.

## Road guidance

- **Junction distance beeps:** beeps get faster as you approach a junction's stopping point. A double beep means a stop sign or red light ahead. A single beep means green or unknown, and is not permission to go. The beeps work with the driving assistant, or with road detection (**F9, then Ctrl+R**) when you drive yourself. You can turn them off in the Road guidance settings.
- **Stop signs and traffic lights:** stop signs and traffic lights that the map knows about are announced ahead of you, with a reminder when you're 12 m (about 40 ft) away. Light changes are announced, including flashing lights. Stop signs and traffic lights each have their own on/off setting. Warnings come about 8 seconds ahead, earlier at higher speeds.
- **Works in either mode:** these announcements work with road guidance or the driving assistant, and switching between them doesn't repeat an announcement.
- **Speed limits:** the map's speed limit is announced when it changes. **F9, then S** now reads your speed and the current limit. You can turn the automatic announcements off with **Speak map speed limit changes** in the Road guidance settings. These limits come from the map's road data, so they may differ from the signs at the roadside.
- **Road status:** **F9, then Ctrl+Shift+R** now includes the stop sign or traffic light ahead.
- **One-way roads:** announcements now tell you which way is legal compared with the way you're facing.

## Status readouts

- **Electric vehicles:**
  - On a fully electric car the Fuel row is now **Battery**, and there are new Motor Power, Motor Torque and Range rows.
  - Range no longer jumps when you brake. It says "unknown" when it can't be worked out yet.
  - Engine and oil temperatures are hidden on cars without an engine. They used to read 32 °F on an electric car.
- **Distance driven:** new rows for Trip, Odometer (career mode only) and Model lifetime distance.
- **Air pressure:**
  - Air Pressure now has a row in the status menu.
  - The maximum tank pressure no longer always reads 0.
  - An empty tank now reads 0 instead of the row disappearing.
- **Every wheel:**
  - Pressure and brake temperature are now reported for every wheel, including trailers and anything else hitched to your vehicle.
  - Wheels are named by where they are: which axle, which side, inner or outer, dual, or trailer.
  - Before, only four wheels were covered. On the Percheron that meant only the front axle, and trailers were never included.
- **Outside temperature:** **F9, then Ctrl+T** reads the outside temperature, and it is also a new row in the status menu.
- **Mixing versions:** a newer BEAM still works with an older copy of the in-game mod, and the other way round.

## Gear changes

- **Choose how gear changes are reported:** off, speech, or sound. Your existing setting carries over.
- **Speech options:** you can pick how gear numbers are spoken. A "say sport once" option announces "sport" when you enter sport mode and stays quiet as it shifts. The default speech hasn't changed.
- **Gear tones:** each gear plays its own note on a musical scale. You can set the starting note, octave, scale and volume, and preview them. Sport gears play the same note with a brighter sound. A quick double shift doesn't click.
- **Neutral:** neutral is still never announced on a manual gearbox.

## Wheel slip

**Ctrl+K** turns wheel slip detection on and off.

- **Faster:** wheelspin is now reported in about 0.1 seconds instead of about a second. A clutch dump on ice is heard straight away, and a locked wheel is reported while it is still locked.
- **Pitch:** the pitch now shows how badly the wheels are slipping compared with how fast the car is moving.
- **Sliding sideways:** a new hiss-like sound tells you when the car is sliding sideways.
- **Direction:** the slip sounds now come from the direction the car is travelling, and you can tell front from back.

## Vehicle scanner

- **Wrecked cars:** parts that have broken off a car are no longer counted as part of it. Before, a wrecked car 400 ft away could read as 0 ft away.
- **Steering tone:** small steering corrections are now easier to hear.
- **Percheron in 0.40:**
  - Steering the tag and pusher axles no longer counts as the truck bending.
  - Tilting the cab no longer makes a diesel truck show electric readouts.

## Node grabber

- **Controller support (0.39):** BEAM now announces the mode of the game's controller node grabber. With the accessible grabber on, the controller grabber picks the node nearest the centre of the screen and announces grabbing, releasing and pinning.
- **Pins:** pinning is now reported reliably.
- **Release all pins:** **Ctrl+Space** releases every pinned node on every vehicle.
- **Strength:** BEAM no longer announces the grabber strength itself, because the game already does.

## Audio debug browser (new)

Press **F9, then J** to open an accessible version of the World Editor's audio tools.

- **Engine sounds:** read and adjust every engine and exhaust sound setting while you drive. Frequencies are spoken in hertz.
- **Restore and copy:** Restore puts the original values back. "Copy as jbeam" copies your settings in a form you can paste into a vehicle file.
- **Sound library:** browse the game's sounds and play them. Every folder starts with a "Stop sound" item.
- **Problems in the original tool that this one avoids:**
  - Changing a setting no longer wipes out the volume change from the fitted exhaust.
  - The main volume and muffling settings now work on electric cars.
  - Electric cars no longer get a cylinder setting that does nothing.

## Game menus (0.39 and 0.40)

- **Holding a direction:**
  - Single presses are spoken straight away.
  - Holding a direction to move through a list speaks the first item and then where you stop.
  - Controller hints wait until you stop moving.
- **Radial menu:**
  - In 0.40 the radial menu clicked but said nothing. It speaks again.
  - Each item is spoken as soon as the stick reaches it.
  - Letting go of the stick no longer reads the item next to the one you chose.
  - Menu names are no longer repeated, and button names are no longer read after every item.
- **Tabs:**
  - Pause menu tabs and sub-tabs are now announced, including tabs that only show an icon.
  - The tab name is no longer cut off by the item you land on.
- **Controls:**
  - Bindings and hints now name only the controller you're holding, and are no longer read twice.
  - In the edit-binding window, the Reassign and Delete buttons can now be told apart.
  - Conflicting bindings are announced.
- **0.40 controls screen:**
  - The **+** button reads "Add binding, button".
  - How to leave a row is spoken once, when you enter it.
  - Rows are no longer read twice when you enter them.
  - Categories say whether they are expanded or collapsed.
  - Key combinations are read in full. Alt+Shift+N used to be read as "n".
- **0.40 messages:**
  - Camera, drive mode and gearbox mode messages are spoken properly.
  - Camera names are spoken, including the seat for the new passenger cameras.
  - Stray formatting no longer gets read out.
- **0.40 main menu:** each tile in the top row is read correctly. They all used to read "Freeroam".
- **Vehicle selector:** filter buttons say on, off or locked, filtered categories are announced, and option selectors read their value.
- **Tuning sliders:** BEAM's workaround for tuning sliders is switched off, because 0.40 fixed the problem in the game.

## Vehicle spawner placement editor

- **Step size:** press **1, 2, 3 or 4** to move in steps of 1, 10, 100 or 1000 feet. The step only changes when you change it, and goes back to 1 foot each time the editor opens.
- **Holding a key:** holding a key repeats the step faster, from about 8 to about 30 times a second, rather than making the steps bigger.
- **Rotation:** each press turns 45 degrees, or 1 degree with Ctrl held, or 90 degrees with Shift held.
- **Position sounds:** the ping deepens whenever you pass 10, 100, 1000 or 10,000 feet, or any multiple of them. Count them to judge how far you've gone.

## For modders and developers

- **New test maps** (built from the source code, not included in the release):
  - **Ice Pool:** an oval ice rink with curved walls, a strip of shallow water and an ice track.
  - **Endless Surfaces (prototype):** a repeating grid of asphalt, dirt, gravel, sand, ice and grass squares, each 500 m across.
  - **Rolling Road (prototype):** a drive-on dyno for testing engine sound under load.
- **Map editor research:** `accessible_map_editor.md` looks at whether a separate screen-reader-friendly map editor could be built.
- **Driving assistant recordings:** a new MCP tool, `assistant_diagnostic`, records drives for review.
- **Docs and tests:** developer documentation has been reorganised by topic, and new automated checks cover most of the features above.
