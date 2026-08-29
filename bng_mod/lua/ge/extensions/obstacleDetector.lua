-- Predictive static-obstacle and terrain warning detector for BeamNG.drive.
-- Static obstacles are selected in the GE VM and sent as one trajectory-relevant hazard.
local M = {}

local PYTHON_HOST = "127.0.0.1"
local PYTHON_PORT_DATA = 4452
local CMD_LISTEN_PORT = 4453
local SCAN_INTERVAL = 0.1
local NUM_FAN_RAYS = 13
local NUM_CORRIDOR_RAYS = 7
local NUM_RAYS = NUM_FAN_RAYS + NUM_CORRIDOR_RAYS
local RAYS_PER_TICK = 7 -- 7 + 7 + 6 rays: one sweep in about 0.3 seconds
local LOW_RAY_OFFSET = -0.2
local HIGH_RAY_OFFSET = 0.2
-- A fixed angle is not safe at predictive ranges: 2 degrees climbs 1.96 m over a 56 m
-- low-speed lookahead and several metres at highway reach, so the probes fly over normal
-- obstacles until impact is close. Keep only a tiny fixed TOTAL rise across the whole ray.
local RAY_TOTAL_RISE_M = 0.15
local PATH_MARGIN_M = 0.75
local DRIVING_SPEED_MPS = 3.0
local STATE_EXPIRY_S = 1.0
local TARGET_MATCH_DEG = 14.0
local TARGET_SWITCH_RATIO = 1.15
local TARGET_MISSED_SWEEPS = 2
local MAX_RANGE_CAP = 600.0

local BASE_TERRAIN_DISTANCES = {5, 10, 15, 20}
local DROPOFF_THRESHOLD = 3.0
local HILL_THRESHOLD = 3.0
local PKT_DROPOFF, PKT_HILL = 2, 3

local SENSITIVITY = {
  early  = {driveAdvisory = 6.5, driveUrgent = 2.5, parkAdvisory = 4.0, parkUrgent = 2.0},
  normal = {driveAdvisory = 5.0, driveUrgent = 2.0, parkAdvisory = 3.0, parkUrgent = 1.5},
  late   = {driveAdvisory = 3.5, driveUrgent = 1.5, parkAdvisory = 2.0, parkUrgent = 1.0},
}

local udpSend, udpCmd, udpDiag = nil, nil, nil
local isActive, scanTimer, currentRayIndex = false, 0, 0
local sensitivityName = "normal"
local pushedState = {direction = "F", steering = 0, throttle = 0, brake = 0, age = math.huge}
local sweepHits, sweepContext = {}, nil
local currentHazard, missedSweeps = nil, 0
local lastDropoffSent, lastHillSent = false, false
local terrainWarned = false

local function detLog(level, msg) log(level, "ObstacleDetector", msg) end
local function clamp(v, lo, hi)
  if v < lo then return lo end
  if v > hi then return hi end
  return v
end

local function rotateVectorAroundAxis(vec, axis, angleDeg)
  local angleRad = math.rad(angleDeg)
  local cosA, sinA = math.cos(angleRad), math.sin(angleRad)
  return vec * cosA + axis:cross(vec) * sinA + axis * (axis:dot(vec) * (1 - cosA))
end

local function rayAnglesFor(speedMps, steering)
  local parking = speedMps < DRIVING_SPEED_MPS
  local step = parking and 13.0 or 7.0
  local halfSpan = parking and 78.0 or 42.0
  local maxShift = parking and 45.0 or 20.0
  local centre = clamp(steering or 0, -1, 1) * maxShift
  local angles = {}
  for i = 0, NUM_FAN_RAYS - 1 do angles[i + 1] = centre - halfSpan + i * step end
  return angles, centre, parking
end

local function stoppingDistance(closing)
  return closing * closing / (2.0 * 9.0) + 0.3
end

local function maximumRayRange(speed, parking, sensitivity)
  local thresholds = SENSITIVITY[sensitivity] or SENSITIVITY.normal
  if parking then
    return math.min(MAX_RANGE_CAP, math.max(8.0, thresholds.parkAdvisory + 2.0))
  end
  -- A complete sweep takes about 0.3 s. Reserve 0.6 s so the far edge still provides the
  -- configured TTC after sweep completion and packet/audio latency. The braking term keeps
  -- the emergency boundary inside the cast even at speeds where v^2 dominates TTC reach.
  local sweepAndReaction = 0.6
  local advisoryReach = speed * (thresholds.driveAdvisory + sweepAndReaction)
  local brakingReach = stoppingDistance(speed) + speed * sweepAndReaction + 5.0
  return math.min(MAX_RANGE_CAP, math.max(10.0, advisoryReach, brakingReach))
end

local function rayUpwardAngle(maxRange)
  return math.deg(math.atan(RAY_TOTAL_RISE_M / math.max(1.0, maxRange)))
end

local function confirmedPairGap(hitLow, hitHigh, maxRange)
  -- Parallel probes can meet the same sloped or irregular surface many metres apart.
  -- Both vehicle-height probes must hit; the nearer intersection is the conservative
  -- surface clearance relevant to the vehicle envelope.
  if hitLow <= 0 or hitLow >= maxRange or hitHigh <= 0 or hitHigh >= maxRange then
    return nil
  end
  return math.min(hitLow, hitHigh)
end

local function classifyHazard(gap, closing, parking, sensitivity)
  if closing <= 0.05 then return nil end
  local thresholds = SENSITIVITY[sensitivity] or SENSITIVITY.normal
  local stop = stoppingDistance(closing)
  local ttc = gap / closing
  local state
  if gap <= stop then
    state = 3
  elseif parking then
    if gap <= thresholds.parkUrgent then state = 2
    elseif gap <= thresholds.parkAdvisory then state = 1 end
  else
    if ttc <= thresholds.driveUrgent then state = 2
    elseif ttc <= thresholds.driveAdvisory then state = 1 end
  end
  if not state then return nil end

  local urgency
  if state == 3 then
    urgency = 255
  elseif state == 2 then
    local outer = parking and thresholds.parkUrgent or thresholds.driveUrgent
    local metric = parking and gap or ttc
    local ratio = clamp(1.0 - metric / math.max(0.01, outer), 0, 1)
    urgency = math.floor(170 + 84 * ratio + 0.5)
  else
    local outer = parking and thresholds.parkAdvisory or thresholds.driveAdvisory
    local inner = parking and thresholds.parkUrgent or thresholds.driveUrgent
    local metric = parking and gap or ttc
    local ratio = clamp((outer - metric) / math.max(0.01, outer - inner), 0, 1)
    urgency = math.floor(1 + 168 * ratio + 0.5)
  end
  return state, urgency, ttc, stop, gap - stop
end

local function candidateBetter(a, b)
  if not a then return false end
  if not b then return true end
  if a.state ~= b.state then return a.state > b.state end
  local am = a.priorityMetric or ((a.ttc and a.ttc >= 0) and a.ttc or a.gap)
  local bm = b.priorityMetric or ((b.ttc and b.ttc >= 0) and b.ttc or b.gap)
  if math.abs(am - bm) > 1e-5 then return am < bm end
  return a.centreOffset < b.centreOffset
end

local function urgencyMeasure(c)
  local metric = c.priorityMetric or ((c.ttc and c.ttc >= 0) and c.ttc or c.gap)
  return 1.0 / math.max(0.01, metric)
end

local function clusterCandidates()
  local clusters, active = {}, nil
  for i = 1, NUM_RAYS do
    local hit = sweepHits[i]
    if hit then
      if not active or i ~= active.lastIndex + 1 or math.abs(hit.gap - active.lastGap) >= 4.0 then
        active = {best = hit, lastIndex = i, lastGap = hit.gap}
        clusters[#clusters + 1] = active
      else
        if candidateBetter(hit, active.best) then active.best = hit end
        active.lastIndex, active.lastGap = i, hit.gap
      end
    else
      active = nil
    end
  end
  local out = {}
  for _, cluster in ipairs(clusters) do out[#out + 1] = cluster.best end
  return out
end

local function selectHazard(candidates)
  local best = nil
  for _, candidate in ipairs(candidates) do
    if candidateBetter(candidate, best) then best = candidate end
  end
  if not currentHazard then
    currentHazard, missedSweeps = best, 0
    return currentHazard
  end

  local match = nil
  for _, candidate in ipairs(candidates) do
    if math.abs(candidate.bearing - currentHazard.bearing) <= TARGET_MATCH_DEG
        and (not match or math.abs(candidate.bearing - currentHazard.bearing)
          < math.abs(match.bearing - currentHazard.bearing)) then
      match = candidate
    end
  end
  if match then
    missedSweeps = 0
    if best and best ~= match and (best.state > match.state
        or (best.state == match.state
          and urgencyMeasure(best) >= urgencyMeasure(match) * TARGET_SWITCH_RATIO)) then
      currentHazard = best
    else
      currentHazard = match
    end
    return currentHazard
  end

  if best and best.state > currentHazard.state then
    currentHazard, missedSweeps = best, 0
    return currentHazard
  end
  missedSweeps = missedSweeps + 1
  if missedSweeps >= TARGET_MISSED_SWEEPS then
    currentHazard, missedSweeps = best, 0
  end
  return currentHazard
end

local function formatSelectedHazard(hazard)
  if not hazard then return "0" end
  return string.format("1,1,%.2f,%d,%.2f,%d,%.2f,%.2f,%.2f",
    hazard.bearing, hazard.urgency, hazard.gap, hazard.state,
    hazard.closing, hazard.ttc or -1, hazard.stoppingMargin)
end

local function sendSelectedHazard(hazard)
  if not udpSend then return end
  udpSend:send(formatSelectedHazard(hazard))
end

local function resolvedIntent(player, fwd, velocity)
  local speed = velocity:length()
  if pushedState.age <= STATE_EXPIRY_S then
    return pushedState.direction == "R" and -1 or 1, pushedState.steering, speed,
      (pushedState.throttle > pushedState.brake + 0.02) or speed > 0.15
  end
  local longitudinal = velocity:dot(fwd)
  return longitudinal < -0.1 and -1 or 1, 0, speed, speed > 0.15
end

local function perimeterDistance(ext, directionSign, angleDeg)
  if not ext then return 0 end
  local a = math.rad(angleDeg)
  local along, lateral = math.cos(a), math.sin(a)
  local localForward = directionSign * along
  local fExtent = localForward >= 0 and math.max(0, ext.maxF) or math.max(0, -ext.minF)
  local rExtent = math.max(math.abs(ext.minR), math.abs(ext.maxR))
  local fDist = math.abs(along) > 1e-4 and fExtent / math.abs(along) or math.huge
  local rDist = math.abs(lateral) > 1e-4 and rExtent / math.abs(lateral) or math.huge
  return math.min(fDist, rDist)
end

local function corridorOffsets(halfWidth)
  local offsets = {}
  local corridorHalf = halfWidth + PATH_MARGIN_M
  for i = 0, NUM_CORRIDOR_RAYS - 1 do
    offsets[#offsets + 1] = -corridorHalf
      + 2 * corridorHalf * i / (NUM_CORRIDOR_RAYS - 1)
  end
  return offsets
end

local function beginSweep(player, playerFwd, playerUp, velocity)
  local directionSign, steering, speed, hasIntent = resolvedIntent(player, playerFwd, velocity)
  local angles, centre, parking = rayAnglesFor(speed, steering)
  local geom = extensions.vehicleGeometry
  local entry = nil
  if geom then geom.request(player:getID()); entry = geom.get(player:getID()) end
  local travel = directionSign < 0 and -playerFwd or playerFwd
  local pathCentre = rotateVectorAroundAxis(travel, playerUp, centre):normalized()
  local halfWidth = entry and math.max(math.abs(entry.ext.minR), math.abs(entry.ext.maxR)) or 1.0
  local raySpecs = {}
  for _, bearing in ipairs(angles) do
    raySpecs[#raySpecs + 1] = {bearing = bearing, lateralOrigin = 0, kind = "fan"}
  end
  for _, lateralOrigin in ipairs(corridorOffsets(halfWidth)) do
    raySpecs[#raySpecs + 1] = {
      bearing = centre,
      lateralOrigin = lateralOrigin,
      kind = "corridor",
    }
  end
  sweepContext = {
    directionSign = directionSign, angles = angles, rays = raySpecs,
    centre = centre, parking = parking,
    speed = speed, hasIntent = hasIntent, ext = entry and entry.ext or nil,
    travel = travel, pathCentre = pathCentre,
    pathLeft = playerUp:cross(pathCentre):normalized(),
    halfWidth = halfWidth,
    maxRange = maximumRayRange(speed, parking, sensitivityName),
    lowHits = 0, confirmedHits = 0, pathHits = 0, classifiedHits = 0,
  }
  for i = 1, NUM_RAYS do sweepHits[i] = nil end
end

local function performScan()
  if not udpSend then return end
  local player = be:getPlayerVehicle(0)
  if not player then return end
  local playerPos = player:getPosition()
  local playerFwd = player:getDirectionVector():normalized()
  local playerUp = player:getDirectionVectorUp():normalized()
  local velocity = player:getVelocity()
  if currentRayIndex == 0 then beginSweep(player, playerFwd, playerUp, velocity) end
  local ctx = sweepContext
  if not ctx then return end

  for _ = 1, RAYS_PER_TICK do
    if currentRayIndex >= NUM_RAYS then break end
    local idx = currentRayIndex + 1
    currentRayIndex = currentRayIndex + 1
    local raySpec = ctx.rays[idx]
    local bearing = raySpec.bearing
    local rayHorizontal = rotateVectorAroundAxis(ctx.travel, playerUp, bearing):normalized()
    local rayLeft = playerUp:cross(rayHorizontal):normalized()
    local rayDir = rotateVectorAroundAxis(
      rayHorizontal, rayLeft, -rayUpwardAngle(ctx.maxRange)):normalized()
    local perimeter = perimeterDistance(ctx.ext, ctx.directionSign, bearing)
    local surfaceOrigin = playerPos + rayHorizontal * perimeter
      + ctx.pathLeft * raySpec.lateralOrigin
    local lowOrigin = surfaceOrigin + playerUp * LOW_RAY_OFFSET
    local highOrigin = surfaceOrigin + playerUp * HIGH_RAY_OFFSET
    local hitLow = castRayStatic(lowOrigin, rayDir, ctx.maxRange)
    if hitLow > 0 and hitLow < ctx.maxRange then
      ctx.lowHits = ctx.lowHits + 1
      local hitHigh = castRayStatic(highOrigin, rayDir, ctx.maxRange)
      local confirmedGap = confirmedPairGap(hitLow, hitHigh, ctx.maxRange)
      if confirmedGap then
        ctx.confirmedHits = ctx.confirmedHits + 1
        local hitOrigin = hitLow <= hitHigh and lowOrigin or highOrigin
        local hitPoint = hitOrigin + rayDir * confirmedGap
        local lateral = math.abs(ctx.pathLeft:dot(hitPoint - playerPos))
        local closing = velocity:dot((hitPoint - playerPos):normalized())
        if ctx.hasIntent and lateral <= ctx.halfWidth + PATH_MARGIN_M then
          ctx.pathHits = ctx.pathHits + 1
          local state, urgency, ttc, _, margin = classifyHazard(
            confirmedGap, closing, ctx.parking, sensitivityName)
          if state then
            ctx.classifiedHits = ctx.classifiedHits + 1
            sweepHits[idx] = {bearing = bearing, gap = confirmedGap, state = state,
              urgency = urgency, closing = closing, ttc = ttc,
              stoppingMargin = margin,
              centreOffset = math.abs(bearing - ctx.centre)
                + math.abs(raySpec.lateralOrigin) * 0.01,
              source = raySpec.kind,
              priorityMetric = ctx.parking and hitLow or ttc}
          end
        end
      end
    end
  end

  if currentRayIndex >= NUM_RAYS then
    currentRayIndex = 0
    local selected = selectHazard(clusterCandidates())
    if selected then
      detLog("D", string.format(
        "hazard state=%d gap=%.2fm closing=%.2fm/s ttc=%.2fs margin=%.2fm bearing=%.1f range=%.1fm",
        selected.state, selected.gap, selected.closing, selected.ttc or -1,
        selected.stoppingMargin, selected.bearing, ctx.maxRange))
    end
    if udpDiag then
      udpDiag:send(string.format("D,%.2f,%.2f,%d,%d,%d,%d,%s",
        ctx.speed, ctx.maxRange, ctx.lowHits, ctx.confirmedHits, ctx.pathHits,
        ctx.classifiedHits, formatSelectedHazard(selected)))
    end
    sendSelectedHazard(selected)
  end
end

local function sampleTerrain()
  if not udpSend then return end
  local player = be:getPlayerVehicle(0)
  if not player then return end
  local playerPos, playerFwd = player:getPosition(), player:getDirectionVector()
  local speedScale = math.max(1.0, player:getVelocity():length() / 10.0)
  if not core_terrain or not core_terrain.getTerrainHeight then return end
  local baseHeight = core_terrain.getTerrainHeight(playerPos)
  if not baseHeight then
    if not terrainWarned then terrainWarned = true; detLog("I", "No terrain; obstacle rays remain active.") end
    return
  end
  terrainWarned = false
  local worstDrop, worstRise, worstDropDist, worstRiseDist = 0, 0, 0, 0
  for _, baseDist in ipairs(BASE_TERRAIN_DISTANCES) do
    local sampleDist = baseDist * speedScale
    local sampleHeight = core_terrain.getTerrainHeight(playerPos + playerFwd * sampleDist)
    if sampleHeight then
      local diff = sampleHeight - baseHeight
      if diff < -DROPOFF_THRESHOLD and diff < worstDrop then
        worstDrop, worstDropDist = diff, sampleDist
      elseif diff > HILL_THRESHOLD and diff > worstRise then
        worstRise, worstRiseDist = diff, sampleDist
      end
    end
  end
  if worstDrop < -DROPOFF_THRESHOLD then
    if not lastDropoffSent then
      local urgency = math.floor(math.min(255, math.abs(worstDrop) / 10 * 255))
      udpSend:send(string.format("%d,%.2f,%d,%.2f", PKT_DROPOFF, 0, urgency, worstDropDist))
      lastDropoffSent = true
    end
  else lastDropoffSent = false end
  if worstRise > HILL_THRESHOLD then
    if not lastHillSent then
      local urgency = math.floor(math.min(255, worstRise / 10 * 255))
      udpSend:send(string.format("%d,%.2f,%d,%.2f", PKT_HILL, 0, urgency, worstRiseDist))
      lastHillSent = true
    end
  else lastHillSent = false end
end

local function setupSockets()
  if udpSend then pcall(function() udpSend:close() end); udpSend = nil end
  if udpCmd then pcall(function() udpCmd:close() end); udpCmd = nil end
  udpSend = socket.udp()
  if udpSend then udpSend:setpeername(PYTHON_HOST, PYTHON_PORT_DATA); udpSend:settimeout(0) end
  local ok, err = pcall(function()
    udpCmd = socket.udp()
    local bound, bindErr = udpCmd:setsockname("127.0.0.1", CMD_LISTEN_PORT)
    if not bound then error(tostring(bindErr), 0) end
    udpCmd:settimeout(0)
  end)
  if not ok then
    detLog("E", "Failed to bind obstacle commands: " .. tostring(err))
    if udpCmd then pcall(function() udpCmd:close() end) end
    udpCmd = nil
  end
end

local CMD_BIND_RETRY_S, cmdBindRetry = 3.0, 0
local function retryCmdBind(dtReal)
  if udpCmd then return end
  cmdBindRetry = cmdBindRetry + (dtReal or 0)
  if cmdBindRetry < CMD_BIND_RETRY_S then return end
  cmdBindRetry = 0
  local ok = pcall(function()
    local sk = socket.udp()
    local bound = sk:setsockname("127.0.0.1", CMD_LISTEN_PORT)
    if not bound then sk:close(); error("still in use", 0) end
    sk:settimeout(0); udpCmd = sk
  end)
  if ok and udpCmd then detLog("I", "Obstacle command socket rebound.") end
end

local function handleCommand(data)
  local command = tostring(data or ""):match("^%s*(.-)%s*$")
  local upper = command:upper()
  if upper == "ON" then
    isActive, currentRayIndex, currentHazard, missedSweeps = true, 0, nil, 0
  elseif upper == "OFF" then
    isActive, currentHazard, missedSweeps = false, nil, 0
    if udpSend then udpSend:send("0") end
  else
    local diagPort = command:match("^[Dd][Ii][Aa][Gg],(%d+)$")
    if diagPort then
      if udpDiag then pcall(function() udpDiag:close() end) end
      udpDiag = socket.udp()
      udpDiag:setpeername(PYTHON_HOST, tonumber(diagPort))
      udpDiag:settimeout(0)
      return
    elseif upper == "DIAG,OFF" then
      if udpDiag then pcall(function() udpDiag:close() end); udpDiag = nil end
      return
    end
    local direction, steering, throttle, brake = command:match(
      "^[Ss][Tt][Aa][Tt][Ee],([FfRr]),([^,]+),([^,]+),([^,]+)$")
    if direction then
      -- STATE is emitted only while Python's Ctrl+O mode is active. Treat it as the
      -- activation lease too, so a Ctrl+L extension reload resumes without requiring the
      -- user to toggle the mode off and on again.
      isActive = true
      pushedState.direction = direction:upper()
      pushedState.steering = clamp(tonumber(steering) or 0, -1, 1)
      pushedState.throttle = clamp(tonumber(throttle) or 0, 0, 1)
      pushedState.brake = clamp(tonumber(brake) or 0, 0, 1)
      pushedState.age = 0
      return
    end
    local sensitivity = command:match("^[Ss][Ee][Nn][Ss][Ii][Tt][Ii][Vv][Ii][Tt][Yy],([%a]+)$")
    if sensitivity and SENSITIVITY[sensitivity:lower()] then sensitivityName = sensitivity:lower() end
  end
end

function M.onExtensionUnloaded()
  if udpSend then pcall(function() udpSend:close() end); udpSend = nil end
  if udpCmd then pcall(function() udpCmd:close() end); udpCmd = nil end
  if udpDiag then pcall(function() udpDiag:close() end); udpDiag = nil end
end
function M.onExtensionLoaded()
  setExtensionUnloadMode(M, "manual")
  detLog("I", "Predictive obstacle detector v2 loaded (surface gap, TTC, speed-scaled reach).")
  setupSockets()
end
function M.onWorldReadyState(state)
  if state == 2 then
    isActive, scanTimer, currentRayIndex = false, 0, 0
    currentHazard, missedSweeps, pushedState.age = nil, 0, math.huge
    lastDropoffSent, lastHillSent = false, false
    setupSockets()
  end
end
function M.onUpdate(dtReal, dtSim, dtRaw)
  retryCmdBind(dtReal)
  pushedState.age = pushedState.age + (dtReal or 0)
  if udpCmd then
    while true do local data = udpCmd:receive(); if not data then break end; handleCommand(data) end
  end
  if not isActive then return end
  scanTimer = scanTimer + dtReal
  if scanTimer >= SCAN_INTERVAL then
    scanTimer = scanTimer - SCAN_INTERVAL
    performScan(); sampleTerrain()
  end
end

function M.debugRayAngles(speed, steering) return rayAnglesFor(speed, steering) end
function M.debugClassify(gap, closing, parking, sensitivity)
  return classifyHazard(gap, closing, parking, sensitivity or "normal")
end
function M.debugStoppingDistance(closing) return stoppingDistance(closing) end
function M.debugMaximumRayRange(speed, parking, sensitivity)
  return maximumRayRange(speed, parking, sensitivity or "normal")
end
function M.debugRayUpwardAngle(maxRange) return rayUpwardAngle(maxRange) end
function M.debugConfirmedPairGap(hitLow, hitHigh, maxRange)
  return confirmedPairGap(hitLow, hitHigh, maxRange)
end
function M.debugRayCounts() return NUM_FAN_RAYS, NUM_CORRIDOR_RAYS, NUM_RAYS end
function M.debugCorridorOffsets(halfWidth) return corridorOffsets(halfWidth) end
function M.debugPerimeterDistance(ext, direction, bearing)
  return perimeterDistance(ext, direction, bearing)
end
function M.debugPathRelevant(lateral, halfWidth)
  return math.abs(lateral) <= halfWidth + PATH_MARGIN_M
end
function M.debugFormatPacket(hazard) return formatSelectedHazard(hazard) end
function M.debugHandleCommand(command) handleCommand(command); return pushedState, sensitivityName end
function M.debugIsActive() return isActive end
function M.debugStateExpired() return pushedState.age > STATE_EXPIRY_S end
function M.debugAdvanceStateAge(dt) pushedState.age = pushedState.age + dt end
function M.debugResetSelection() currentHazard, missedSweeps = nil, 0 end
function M.debugSelect(candidates) return selectHazard(candidates) end

return M
