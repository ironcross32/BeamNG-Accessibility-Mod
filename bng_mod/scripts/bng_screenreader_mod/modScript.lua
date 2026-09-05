-- modScript.lua for bng_screenreader_mod
-- This file is auto-executed by BeamNG when the mod loads.
-- It loads the vehicleScanner GE extension.

extensions.load("vehicleNaming")
-- Adds the Accessibility controls category before BeamNG validates this mod's regular
-- actions. The extension is manual so the bindings and bridge survive ordinary Lua reloads.
extensions.load("accessibilityInput")
-- Loaded first: vehicleScanner and implementProximity both query it, and its own onUpdate
-- only ever retries stalled resolves, so it cannot throw ahead of anything below.
extensions.load("vehicleGeometry")
-- Same reasoning, same slot: implementProximity queries it, and its onUpdate only retries
-- stalled resolves. It is a separate file from vehicleGeometry because it answers a different
-- question -- "where is this vehicle's ramp mouth" rather than "where is its surface" -- and
-- folding it in would put a name-matched special case inside the one file whose whole premise
-- is that it has none.
extensions.load("rampGeometry")
-- Old Cannon barrel nodes and configuration-derived pre-shot ballistics.
extensions.load("cannonGeometry")
-- What happened to the car you just fired out of the large cannon. Loaded after rampGeometry,
-- which it queries for the firing axis; it holds no scene references of its own between ticks,
-- so it does not need implementProximity's late slot.
extensions.load("cannonShot")
extensions.load("vehicleScanner")
-- The yaw between the driven vehicle and the trailer hooked to it, fed to the same tone the
-- WL-40's frame articulation drives. After vehicleGeometry, whose boxFrame it queries for both
-- bodies; it holds no scene references between ticks and reads a registry the engine maintains,
-- so it needs neither implementProximity's late slot nor a command port.
extensions.load("trailerAngle")
-- Where the big map's navigation route ends, so Python can pulse a beacon at it. Reads
-- core_groundMarkers, which the engine maintains; it holds no scene references between ticks
-- and binds no command port, so like trailerAngle it needs no late slot and nothing to be told.
extensions.load("routeBeacon")
extensions.load("beamtelAI")
extensions.load("vehicleSlots")
extensions.load("cameraInfo")
-- Manual extensions survive the engine's Ctrl+L unload pass. A plain load therefore keeps
-- the old module table and old source even though this modScript is executed again. Reload
-- this detector explicitly so Python's backwards-compatible audio cannot make an old Lua
-- packet sound like the new implementation. onExtensionUnloaded closes its command socket.
if extensions.isExtensionLoaded("obstacleDetector") then
  extensions.reload("obstacleDetector")
else
  extensions.load("obstacleDetector")
end
-- Passive until a SCAN command arrives; its heightmap tier costs nothing per frame and
-- its raycast fallback is budgeted, so it does not need implementProximity's late slot.
extensions.load("terrainScanner")
extensions.load("nodeGrabberAccessible")
extensions.load("clickspotAccessible")
extensions.load("vehicleSpawnerAccessible")
extensions.load("vehicleBindings")
-- Learn Bindings Mode. Sits beside vehicleBindings because it shares its core_input_actions
-- dependency and reuses its control-name formatter. Passive: its onUpdate drains a socket and
-- ticks a timer until Python turns the mode on. Reloaded explicitly for the reason
-- obstacleDetector is -- this one WRAPS a stock engine function, so a stale copy left behind by
-- the manual unload mode would be a wrapper whose live half no longer exists.
if extensions.isExtensionLoaded("bindingLearn") then
  extensions.reload("bindingLearn")
else
  extensions.load("bindingLearn")
end
-- roadDetector is manual and owns the challenge-capture command socket. A plain
-- load after Ctrl+L retains the old module table and source, so challenge events
-- could reach current Python while CAPTURE_ON was still handled by stale Lua.
if extensions.isExtensionLoaded("roadDetector") then
  extensions.reload("roadDetector")
else
  extensions.load("roadDetector")
end
extensions.load("uiToggle")
extensions.load("consoleAccessible")
-- The environment values the stock pause UI does not expose at all (temperature). Passive:
-- its onUpdate drains a socket and does nothing else until Python asks.
extensions.load("environmentAccessible")
-- The stock vehicle selector's details-page specifications, answered on request (F9 then
-- SPACE there, `i` in the mod's own spawner). Passive: its onUpdate drains a socket and
-- re-checks one table field, and it never speaks on its own.
extensions.load("vehicleInfo")
-- Loaded late on purpose: an uncaught throw in an extension's onUpdate stops every
-- extension AFTER it in this list, and this one raycasts against arbitrary scene objects.
extensions.load("implementProximity")
-- Both are manual extensions, so a plain load after Ctrl+L would retain the old
-- source. Reload explicitly to keep the lifecycle producer and transport in step.
if extensions.isExtensionLoaded("bnvdaBridge") then
  extensions.reload("bnvdaBridge")
else
  extensions.load("bnvdaBridge")
end
-- Loaded after the transport it calls. This remains passive outside the exact
-- generated Proving Grounds mission id.
if extensions.isExtensionLoaded("hillClimbChallenge") then
  extensions.reload("hillClimbChallenge")
else
  extensions.load("hillClimbChallenge")
end
log('I', 'bng_screenreader_mod', 'modScript.lua executed: accessibility extensions, bnvdaBridge and hill-climb challenge loaded.')
