-- modScript.lua for bng_screenreader_mod
-- This file is auto-executed by BeamNG when the mod loads.
-- It loads the vehicleScanner GE extension.

extensions.load("vehicleNaming")
-- Loaded first: vehicleScanner and implementProximity both query it, and its own onUpdate
-- only ever retries stalled resolves, so it cannot throw ahead of anything below.
extensions.load("vehicleGeometry")
extensions.load("vehicleScanner")
extensions.load("beamtelAI")
extensions.load("vehicleSlots")
extensions.load("cameraInfo")
extensions.load("obstacleDetector")
extensions.load("nodeGrabberAccessible")
extensions.load("clickspotAccessible")
extensions.load("vehicleSpawnerAccessible")
extensions.load("vehicleBindings")
extensions.load("roadDetector")
extensions.load("uiToggle")
extensions.load("consoleAccessible")
-- Loaded late on purpose: an uncaught throw in an extension's onUpdate stops every
-- extension AFTER it in this list, and this one raycasts against arbitrary scene objects.
extensions.load("implementProximity")
extensions.load("bnvdaBridge")
log('I', 'bng_screenreader_mod', 'modScript.lua executed: accessibility extensions and bnvdaBridge loaded.')
