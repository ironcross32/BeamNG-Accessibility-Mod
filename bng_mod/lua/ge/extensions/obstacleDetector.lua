-- =================================================================================================
--
--  Obstacle Detector for BeamNG.drive Accessibility Mod (GE Extension)
--
--  Description: Casts rays in a fan pattern around the player vehicle to detect nearby static
--               obstacles (walls, barriers, trees, buildings, terrain). Also samples terrain
--               height ahead to detect drop-offs and steep hills. Sends obstacle data to the
--               Python backend via UDP.
--
--  Loaded by: scripts/bng_screenreader_mod/modScript.lua
--
--  Version:     1.0
--  Target Game: BeamNG.drive 0.37+
--
-- =================================================================================================

local M = {}

-- Configuration
local PYTHON_HOST        = "127.0.0.1"
local PYTHON_PORT_DATA   = 4452   -- send obstacle data to Python on this port
local CMD_LISTEN_PORT    = 4453   -- receive ON/OFF commands from Python on this port
local SCAN_INTERVAL      = 0.1   -- seconds between scan ticks (10 Hz)

-- Ray Configuration
local NUM_RAYS           = 12    -- directions around the vehicle (every 30 degrees)
local RAYS_PER_TICK      = 4     -- how many rays to cast per scan tick (performance budget)
local RAY_HEIGHT_OFFSET  = 0.5   -- meters above vehicle center to cast from (bumper height)
local RAY_UPWARD_ANGLE   = 2.0   -- degrees to angle rays upward (avoid hitting flat ground)

-- Speed-Sensitive Range Configuration
local BASE_MAX_RANGE     = 30.0  -- ray cast range at 0 speed (meters)
local BASE_WARNING_RANGE = 20.0  -- warning range at 0 speed (meters)
local RANGE_PER_MPS      = 2.0   -- extra meters of range per m/s of speed (~2s lookahead)
local MAX_RANGE_CAP      = 100.0 -- absolute maximum range cap (meters)
local WARNING_RANGE_CAP  = 80.0  -- absolute maximum warning range cap

-- Terrain Sampling Configuration
local BASE_TERRAIN_DISTANCES = { 5, 10, 15, 20 }  -- base sample distances (meters ahead)
local DROPOFF_THRESHOLD  = 3.0   -- meters of drop to trigger warning
local HILL_THRESHOLD     = 3.0   -- meters of rise to trigger warning

-- Packet Types
local PKT_CLEAR          = 0
local PKT_STATIC         = 1
local PKT_DROPOFF        = 2
local PKT_HILL           = 3

-- Internal State
local udpSend            = nil
local udpCmd             = nil
local isActive           = false
local scanTimer          = 0
local currentRayIndex    = 0   -- which ray direction to start from this tick
local currentMaxRange    = BASE_MAX_RANGE
local currentWarnRange   = BASE_WARNING_RANGE

-- Obstacle state: one slot per quadrant (front-left, front-right, rear-left, rear-right)
-- Each stores the nearest obstacle in that quadrant
local quadrants = {
  { bearing = 0, distance = math.huge, type = 0 },  -- front-right (0 to 90)
  { bearing = 0, distance = math.huge, type = 0 },  -- rear-right  (90 to 180)
  { bearing = 0, distance = math.huge, type = 0 },  -- rear-left   (-180 to -90)
  { bearing = 0, distance = math.huge, type = 0 },  -- front-left  (-90 to 0)
}

-- Terrain warning state (to avoid spamming)
local lastDropoffSent    = false
local lastHillSent       = false

-- Precomputed ray directions as angle offsets (degrees from forward, 0 = forward, clockwise)
local rayAngles = {}
for i = 0, NUM_RAYS - 1 do
  rayAngles[i + 1] = (i * 360.0 / NUM_RAYS)  -- 0, 30, 60, 90, ... 330
end

-- Logging helper
local function detLog(level, msg)
  log(level, 'ObstacleDetector', msg)
end

-- =================================================================================================
--  Utility: Rotate a vector around an up axis by angle (degrees)
-- =================================================================================================

local function rotateVectorAroundAxis(vec, axis, angleDeg)
  local angleRad = math.rad(angleDeg)
  local cosA = math.cos(angleRad)
  local sinA = math.sin(angleRad)
  -- Rodrigues' rotation formula
  local dot = axis:dot(vec)
  local cross = axis:cross(vec)
  return vec * cosA + cross * sinA + axis * (dot * (1 - cosA))
end

-- =================================================================================================
--  Utility: Determine which quadrant a bearing falls into
--  Returns 1-4: 1=front-right(0..90), 2=rear-right(90..180),
--               3=rear-left(-180..-90), 4=front-left(-90..0)
-- =================================================================================================

local function bearingToQuadrant(bearingDeg)
  if bearingDeg >= 0 and bearingDeg < 90 then return 1
  elseif bearingDeg >= 90 then return 2
  elseif bearingDeg < -90 then return 3
  else return 4 end
end

-- =================================================================================================
--  Core Scan Logic
-- =================================================================================================

local function performScan()
  if not udpSend then return end

  local player = be:getPlayerVehicle(0)
  if not player then return end

  local playerPos   = player:getPosition()
  local playerFwd   = player:getDirectionVector()
  local playerUp    = player:getDirectionVectorUp()
  local playerRight = playerUp:cross(playerFwd)

  -- Compute vehicle speed (m/s) from velocity vector
  local vel = player:getVelocity()
  local speedMps = vel:length()

  -- Scale ranges based on speed
  currentMaxRange  = math.min(BASE_MAX_RANGE  + speedMps * RANGE_PER_MPS, MAX_RANGE_CAP)
  currentWarnRange = math.min(BASE_WARNING_RANGE + speedMps * RANGE_PER_MPS, WARNING_RANGE_CAP)

  -- Raise ray origin above vehicle center
  local rayOrigin = playerPos + playerUp * RAY_HEIGHT_OFFSET

  -- Reset quadrants for this scan cycle when we wrap around to ray 0
  if currentRayIndex == 0 then
    for i = 1, 4 do
      quadrants[i].distance = math.huge
      quadrants[i].type = 0
    end
  end

  -- Cast RAYS_PER_TICK rays starting from currentRayIndex
  for r = 1, RAYS_PER_TICK do
    local idx = (currentRayIndex % NUM_RAYS) + 1
    currentRayIndex = (currentRayIndex + 1) % NUM_RAYS

    local angleDeg = rayAngles[idx]

    -- Rotate forward vector around up axis by angleDeg
    local rayDir = rotateVectorAroundAxis(playerFwd, playerUp, angleDeg)

    -- Tilt ray slightly upward to avoid hitting flat ground
    rayDir = rotateVectorAroundAxis(rayDir, playerRight, -RAY_UPWARD_ANGLE)
    rayDir = rayDir:normalized()

    -- Cast ray
    local hitDist = castRayStatic(rayOrigin, rayDir, currentMaxRange)

    if hitDist > 0 and hitDist < currentWarnRange then
      -- Convert ray angle to signed bearing (-180 to +180, positive = right)
      local bearing = angleDeg
      if bearing > 180 then bearing = bearing - 360 end

      local quad = bearingToQuadrant(bearing)
      if hitDist < quadrants[quad].distance then
        quadrants[quad].bearing  = bearing
        quadrants[quad].distance = hitDist
        quadrants[quad].type     = PKT_STATIC
      end
    end
  end

  -- After a full sweep (all rays cast), send results as CSV text
  -- Format: "type,bearing,urgency,distance" per obstacle, or "0" for all clear
  if currentRayIndex == 0 then
    local anyObstacle = false
    for i = 1, 4 do
      if quadrants[i].type ~= 0 then
        anyObstacle = true
        local urgency = math.floor(math.max(0, math.min(255,
          (1 - quadrants[i].distance / currentWarnRange) * 255)))
        local packet = string.format("%d,%.2f,%d,%.2f",
          quadrants[i].type, quadrants[i].bearing, urgency, quadrants[i].distance)
        udpSend:send(packet)
      end
    end
    if not anyObstacle then
      udpSend:send("0")
    end
  end
end

-- =================================================================================================
--  Terrain Height Sampling (drop-offs and hills)
-- =================================================================================================

local function sampleTerrain()
  if not udpSend then return end

  local player = be:getPlayerVehicle(0)
  if not player then return end

  local playerPos = player:getPosition()
  local playerFwd = player:getDirectionVector()

  -- Compute speed for terrain lookahead scaling
  local vel = player:getVelocity()
  local speedMps = vel:length()
  -- Scale terrain sample distances: at higher speed, sample farther ahead
  local speedScale = math.max(1.0, speedMps / 10.0)  -- 1x at <=10m/s, 2x at 20m/s, etc.

  -- Get terrain height at vehicle position
  local baseHeight = core_terrain.getTerrainHeight(playerPos)

  local worstDrop = 0
  local worstRise = 0
  local worstDropDist = 0
  local worstRiseDist = 0

  for _, baseDist in ipairs(BASE_TERRAIN_DISTANCES) do
    local sampleDist = baseDist * speedScale
    local samplePos = playerPos + playerFwd * sampleDist
    local sampleHeight = core_terrain.getTerrainHeight(samplePos)
    local diff = sampleHeight - baseHeight

    if diff < -DROPOFF_THRESHOLD and diff < worstDrop then
      worstDrop = diff
      worstDropDist = sampleDist
    elseif diff > HILL_THRESHOLD and diff > worstRise then
      worstRise = diff
      worstRiseDist = sampleDist
    end
  end

  if worstDrop < -DROPOFF_THRESHOLD then
    if not lastDropoffSent then
      local urgency = math.floor(math.min(255, math.abs(worstDrop) / 10.0 * 255))
      udpSend:send(string.format("%d,%.2f,%d,%.2f", PKT_DROPOFF, 0, urgency, worstDropDist))
      lastDropoffSent = true
    end
  else
    lastDropoffSent = false
  end

  if worstRise > HILL_THRESHOLD then
    if not lastHillSent then
      local urgency = math.floor(math.min(255, worstRise / 10.0 * 255))
      udpSend:send(string.format("%d,%.2f,%d,%.2f", PKT_HILL, 0, urgency, worstRiseDist))
      lastHillSent = true
    end
  else
    lastHillSent = false
  end
end

-- =================================================================================================
--  GE Extension Hooks
-- =================================================================================================

local function setupSockets()
  if udpSend then pcall(function() udpSend:close() end); udpSend = nil end
  if udpCmd  then pcall(function() udpCmd:close()  end); udpCmd  = nil end

  udpSend = socket.udp()
  if udpSend then
    udpSend:setpeername(PYTHON_HOST, PYTHON_PORT_DATA)
    udpSend:settimeout(0)
    detLog('info', "UDP send socket created, targeting " .. PYTHON_HOST .. ":" .. PYTHON_PORT_DATA)
  else
    detLog('error', "Failed to create UDP send socket.")
  end

  local ok, err = pcall(function()
    udpCmd = socket.udp()
    udpCmd:setsockname("127.0.0.1", CMD_LISTEN_PORT)
    udpCmd:settimeout(0)
  end)
  if ok and udpCmd then
    detLog('info', "UDP command socket listening on port " .. CMD_LISTEN_PORT)
  else
    detLog('error', "Failed to create UDP command socket: " .. tostring(err))
    udpCmd = nil
  end
end

function M.onExtensionLoaded()
  detLog('info', "Obstacle detector extension loaded.")
  -- Bind sockets here so Ctrl+L Lua reload re-opens them.
  setupSockets()
end

function M.onWorldReadyState(state)
  if state == 2 then
    detLog('info', "World ready. Initializing obstacle detector.")

    -- Reset state for new map
    isActive        = false
    scanTimer       = 0
    currentRayIndex = 0
    currentMaxRange = BASE_MAX_RANGE
    currentWarnRange = BASE_WARNING_RANGE
    lastDropoffSent = false
    lastHillSent    = false
    for i = 1, 4 do
      quadrants[i].distance = math.huge
      quadrants[i].type = 0
    end

    setupSockets()
  end
end

function M.onUpdate(dtReal, dtSim, dtRaw)
  -- Poll for commands
  if udpCmd then
    local data = udpCmd:receive()
    if data then
      local cmd = data:match("^%s*(.-)%s*$"):upper()
      if cmd == "ON" and not isActive then
        isActive = true
        currentRayIndex = 0
        detLog('info', "Obstacle detection activated.")
      elseif cmd == "OFF" and isActive then
        isActive = false
        detLog('info', "Obstacle detection deactivated.")
      end
    end
  end

  if not isActive then return end

  -- Rate-limit scans
  scanTimer = scanTimer + dtReal
  if scanTimer >= SCAN_INTERVAL then
    scanTimer = 0
    performScan()
    sampleTerrain()
  end
end

return M
