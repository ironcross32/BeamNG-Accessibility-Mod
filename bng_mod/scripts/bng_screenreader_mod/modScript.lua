-- modScript.lua for bng_screenreader_mod
-- This file is auto-executed by BeamNG when the mod loads.
-- It loads the vehicleScanner GE extension.

extensions.load("vehicleNaming")
extensions.load("vehicleScanner")
extensions.load("beamtelAI")
extensions.load("vehicleSlots")
extensions.load("cameraInfo")
extensions.load("obstacleDetector")
extensions.load("nodeGrabberAccessible")
extensions.load("clickspotAccessible")
extensions.load("vehicleSpawnerAccessible")
extensions.load("roadDetector")
extensions.load("uiToggle")
extensions.load("consoleAccessible")
extensions.load("bnvdaBridge")
log('I', 'bng_screenreader_mod', 'modScript.lua executed: accessibility extensions and bnvdaBridge loaded.')
