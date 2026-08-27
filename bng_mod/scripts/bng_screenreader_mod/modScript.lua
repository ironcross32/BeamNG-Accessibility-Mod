-- modScript.lua for bng_screenreader_mod
-- This file is auto-executed by BeamNG when the mod loads.
-- It loads the vehicleScanner GE extension.

extensions.load("vehicleNaming")
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
extensions.load("beamtelAI")
extensions.load("vehicleSlots")
extensions.load("cameraInfo")
extensions.load("obstacleDetector")
-- Passive until a SCAN command arrives; its heightmap tier costs nothing per frame and
-- its raycast fallback is budgeted, so it does not need implementProximity's late slot.
extensions.load("terrainScanner")
extensions.load("nodeGrabberAccessible")
extensions.load("clickspotAccessible")
extensions.load("vehicleSpawnerAccessible")
extensions.load("vehicleBindings")
extensions.load("roadDetector")
extensions.load("uiToggle")
extensions.load("consoleAccessible")
-- The environment values the stock pause UI does not expose at all (temperature). Passive:
-- its onUpdate drains a socket and does nothing else until Python asks.
extensions.load("environmentAccessible")
-- Loaded late on purpose: an uncaught throw in an extension's onUpdate stops every
-- extension AFTER it in this list, and this one raycasts against arbitrary scene objects.
extensions.load("implementProximity")
extensions.load("bnvdaBridge")
log('I', 'bng_screenreader_mod', 'modScript.lua executed: accessibility extensions and bnvdaBridge loaded.')
