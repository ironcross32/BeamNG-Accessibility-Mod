-- Run in BeamNG's GE Lua console:
--   dofile('lua/ge/extensions/obstacleDetector.lua') is normally loaded as an extension;
-- this diagnostic exercises its exported pure calculations without casting scene rays.
local detector = extensions and extensions.obstacleDetector
if not detector then
  local detectorPath = (arg and arg[1]) or "bng_mod/lua/ge/extensions/obstacleDetector.lua"
  detector = dofile(detectorPath)
end

local passed = 0
local function check(condition, message)
  assert(condition, message)
  passed = passed + 1
end
local function near(a, b, eps) return math.abs(a - b) <= (eps or 1e-6) end

local driving, driveCentre, isParking = detector.debugRayAngles(10, 0)
check(#driving == 13 and near(driving[1], -42) and near(driving[13], 42),
  "driving fan must be thirteen 7-degree rays across +/-42")
check(not isParking and near(driveCentre, 0), "10 m/s must use driving coverage")
local fanCount, corridorCount, totalCount = detector.debugRayCounts()
check(fanCount == 13 and corridorCount == 7 and totalCount == 20,
  "each sweep must retain the 13-ray fan and add seven swept-corridor guard rays")
local corridor = detector.debugCorridorOffsets(1.0)
check(#corridor == 7 and near(corridor[1], -1.75) and near(corridor[7], 1.75),
  "corridor guards must span half vehicle width plus the 0.75 m path margin")
check(near(corridor[4], 0), "corridor guards must include the exact predicted centreline")
local turning, shifted = detector.debugRayAngles(10, 1)
check(near(shifted, 20) and near(turning[1], -22) and near(turning[13], 62),
  "driving steering shift must cap at 20 degrees")
local parking, parkCentre, parkingMode = detector.debugRayAngles(2, -1)
check(parkingMode and near(parkCentre, -45), "low speed must use parking frame")
check(near(parking[1], -123) and near(parking[13], 33),
  "parking fan must span +/-78 around its shifted centre")

local ext = {minF = -1.5, maxF = 2.5, minR = -1.0, maxR = 1.0}
check(near(detector.debugPerimeterDistance(ext, 1, 0), 2.5),
  "forward gap must start at front perimeter")
check(near(detector.debugPerimeterDistance(ext, -1, 0), 1.5),
  "reverse gap must start at rear perimeter")
check(near(detector.debugPerimeterDistance(nil, 1, 0), 0),
  "unresolved geometry must retain origin fallback")
check(detector.debugPathRelevant(1.74, 1.0), "vehicle half-width plus 0.75 m is relevant")
check(not detector.debugPathRelevant(1.76, 1.0), "outside path corridor must be silent")

local state, urgency, ttc, stop = detector.debugClassify(20, 5, false, "normal")
check(state == 1 and near(ttc, 4), "normal driving advisory must begin at five seconds")
state = detector.debugClassify(8, 5, false, "normal")
check(state == 2, "normal driving urgent must begin at two seconds")
stop = detector.debugStoppingDistance(18)
state = detector.debugClassify(stop, 18, false, "normal")
check(state == 3, "stopping-distance crossing must be emergency")
check(detector.debugClassify(1, -1, true, "normal") == nil,
  "receding obstacles must remain silent")
check(detector.debugClassify(2.5, 1, true, "normal") == 1,
  "parking advisory must use surface gap")
check(detector.debugClassify(1.4, 1, true, "normal") == 2,
  "parking urgent must use surface gap")
check(detector.debugClassify(3.5, 1, true, "early") == 1,
  "early sensitivity must extend parking range")
check(detector.debugMaximumRayRange(10, false, "normal") >= 56,
  "driving reach must include normal advisory TTC plus sweep latency")
check(detector.debugMaximumRayRange(30, false, "normal") >= 168,
  "highway reach must grow linearly with speed instead of pinning at 120 m")
check(detector.debugMaximumRayRange(50, false, "early") >= 354.9,
  "early high-speed coverage must preserve its full 6.5 second advisory")
check(detector.debugMaximumRayRange(2, true, "normal") >= 5,
  "parking reach must cover its surface-gap advisory with margin")
local lowSpeedRange = detector.debugMaximumRayRange(10, false, "normal")
local lowSpeedRise = math.tan(math.rad(detector.debugRayUpwardAngle(lowSpeedRange)))
  * lowSpeedRange
check(near(lowSpeedRise, 0.15, 1e-6),
  "a long low-speed ray must rise only 15 cm, not nearly two metres")
local highwayRange = detector.debugMaximumRayRange(50, false, "normal")
local highwayRise = math.tan(math.rad(detector.debugRayUpwardAngle(highwayRange)))
  * highwayRange
check(near(highwayRise, 0.15, 1e-6),
  "highway reach must not increase ray height above ordinary obstacles")

check(near(detector.debugConfirmedPairGap(28, 36, 100), 28, 1e-6),
  "paired probes on a slope must confirm even when hit distances differ")
check(detector.debugConfirmedPairGap(28, 0, 100) == nil,
  "a low-only hit must not confirm an obstacle")
check(detector.debugConfirmedPairGap(0, 36, 100) == nil,
  "a high-only hit must not confirm an obstacle")

detector.debugResetSelection()
local first = {bearing = 0, gap = 5, state = 2, urgency = 180, closing = 2,
  ttc = 2.5, stoppingMargin = 4, centreOffset = 0}
local onlySlightlyBetter = {bearing = 30, gap = 4.6, state = 2, urgency = 185,
  closing = 2, ttc = 2.3, stoppingMargin = 4, centreOffset = 30}
check(detector.debugSelect({first}) == first, "first actionable hazard must be selected")
check(detector.debugSelect({first, onlySlightlyBetter}) == first,
  "less than fifteen percent improvement must not chatter targets")
check(detector.debugSelect({}) == first, "one missed sweep must retain the target")
check(detector.debugSelect({}) == nil, "two missed sweeps must clear the target")
detector.debugResetSelection()
local emergency = {bearing = 20, gap = 0.2, state = 3, urgency = 255, closing = 3,
  ttc = 0.07, stoppingMargin = -0.6, centreOffset = 20}
check(detector.debugSelect({first}) == first, "selection reset failed")
check(detector.debugSelect({first, emergency}) == emergency,
  "a higher state must replace the retained target immediately")

local pushed, sensitivity = detector.debugHandleCommand("STATE,R,0.25,0.4,0.1")
check(pushed.direction == "R" and near(pushed.steering, 0.25),
  "STATE command must preserve reverse and steering")
check(detector.debugIsActive(), "active Python STATE lease must restore mode after Lua reload")
check(not detector.debugStateExpired(), "fresh pushed state must be valid")
detector.debugAdvanceStateAge(1.01)
check(detector.debugStateExpired(), "pushed state must expire after one second")
_, sensitivity = detector.debugHandleCommand("SENSITIVITY,late")
check(sensitivity == "late", "sensitivity command must update thresholds")

local packet = detector.debugFormatPacket(emergency)
check(packet:match("^1,1,20%.00,255,0%.20,") ~= nil,
  "extended packet must begin with one legacy bearing/urgency/gap triple")
local commas = select(2, packet:gsub(",", ""))
check(commas == 8, "extended packet must contain nine fields")

print(string.format("obstacle detector simulation: %d checks passed", passed))
