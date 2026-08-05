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
-- Two ray heights bracket the "drive-over vs real obstacle" boundary. Vehicle origin sits
-- ~0.5m above ground; the guardrail rail-top in BeamNG is ~0.7m above ground. So:
--   LOW (~0.3m above ground): above typical curb/island height (~0.15m), so curbs miss.
--   HIGH (~0.7m above ground): at guardrail rail height — guardrails register, but knee-high
--                              decorative props the car can plow through often don't.
local LOW_RAY_OFFSET     = -0.2  -- low probe offset above vehicle origin
local HIGH_RAY_OFFSET    = 0.2   -- high probe offset above vehicle origin
local CLEARANCE_TOLERANCE = 1.5  -- if high ray hits within this far of low hit, treat as same obstacle
local CLOSE_OBSTACLE_DIST = 4.0  -- low-only hits closer than this still warn (safety override)
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

-- Per-ray hit storage for the current sweep. Each entry: { hit, distance, bearing }.
-- After a full sweep, contiguous rays with similar distances are clustered into one
-- "obstacle" so a long wall/guardrail produces a single beep at its closest point
-- rather than 3-4 stacked beeps along its length.
local sweepHits = {}
for i = 1, NUM_RAYS do
  sweepHits[i] = { hit = false, distance = math.huge, bearing = 0 }
end

-- Cluster threshold: contiguous rays whose hit distances differ by less than this
-- are treated as the same obstacle. Tuned for typical walls/guardrails seen at
-- 30deg spacing — a flat wall in front produces ~2-3 ray hits at distances d, d/cos(30),
-- d/cos(60), with the first two within ~0.7d of each other.
local CLUSTER_DIST_THRESHOLD = 4.0

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
--  Cluster contiguous-angle ray hits with similar distances. Wall along several rays
--  becomes one obstacle, placed at the closest hit (minimum distance) in the cluster.
--  Returns a list of { distance, bearing } for each cluster, in arbitrary order.
-- =================================================================================================

local function clusterSweepHits()
  local clusters = {}
  local visited = {}

  -- Walk circularly so a cluster spanning the 0-degree wraparound is captured.
  local startIdx = 1
  -- Find a non-hit gap to start from (so we don't begin in the middle of a cluster
  -- that wraps around). If every ray hit, just start at 1.
  for i = 1, NUM_RAYS do
    if not sweepHits[i].hit then startIdx = i; break end
  end

  local i = 0
  while i < NUM_RAYS do
    local idx = ((startIdx - 1 + i) % NUM_RAYS) + 1
    if sweepHits[idx].hit and not visited[idx] then
      local minDist = sweepHits[idx].distance
      local minBearing = sweepHits[idx].bearing
      local prevDist = sweepHits[idx].distance
      visited[idx] = true
      -- Walk forward as long as the next ray hit and within threshold of the previous
      local j = i + 1
      while j < NUM_RAYS do
        local jdx = ((startIdx - 1 + j) % NUM_RAYS) + 1
        if not sweepHits[jdx].hit then break end
        if math.abs(sweepHits[jdx].distance - prevDist) >= CLUSTER_DIST_THRESHOLD then break end
        if sweepHits[jdx].distance < minDist then
          minDist = sweepHits[jdx].distance
          minBearing = sweepHits[jdx].bearing
        end
        prevDist = sweepHits[jdx].distance
        visited[jdx] = true
        j = j + 1
      end
      table.insert(clusters, { distance = minDist, bearing = minBearing })
      i = j
    else
      i = i + 1
    end
  end
  return clusters
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

  -- Two ray origins for clearance probe. Low ray catches anything in the way; high ray
  -- confirms the obstacle is tall enough to actually matter (vs a curb the car drives over).
  local rayOriginLow  = playerPos + playerUp * LOW_RAY_OFFSET
  local rayOriginHigh = playerPos + playerUp * HIGH_RAY_OFFSET

  -- Reset sweep hits when we wrap around to ray 0
  if currentRayIndex == 0 then
    for i = 1, NUM_RAYS do
      sweepHits[i].hit = false
      sweepHits[i].distance = math.huge
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

    -- Cast low ray first — cheap rejection if nothing is there at all.
    local hitLow = castRayStatic(rayOriginLow, rayDir, currentMaxRange)

    if hitLow > 0 and hitLow < currentWarnRange then
      -- Confirm with high ray. Real obstacle only if high ray also hits at a similar
      -- distance, OR the obstacle is dangerously close (low-only at <CLOSE_OBSTACLE_DIST
      -- biases toward false positives over false negatives at imminent-collision range).
      local hitHigh = castRayStatic(rayOriginHigh, rayDir, currentMaxRange)
      local highConfirms = (hitHigh > 0 and hitHigh < (hitLow + CLEARANCE_TOLERANCE))
      local closeOverride = (hitLow < CLOSE_OBSTACLE_DIST)
      if highConfirms or closeOverride then
        -- Convert ray angle to signed bearing (-180 to +180, positive = right)
        local bearing = angleDeg
        if bearing > 180 then bearing = bearing - 360 end

        sweepHits[idx].hit      = true
        sweepHits[idx].distance = hitLow
        sweepHits[idx].bearing  = bearing
      end
    end
  end

  -- After a full sweep, cluster contiguous hits and send everything in one packet.
  -- Format:
  --   "0"                                            -- no static obstacles
  --   "1,n,b1,u1,d1,b2,u2,d2,...,bn,un,dn"           -- n clustered static obstacles
  if currentRayIndex == 0 then
    local clusters = clusterSweepHits()
    if #clusters == 0 then
      udpSend:send("0")
    else
      local parts = { "1", tostring(#clusters) }
      for _, c in ipairs(clusters) do
        local urgency = math.floor(math.max(0, math.min(255,
          (1 - c.distance / currentWarnRange) * 255)))
        table.insert(parts, string.format("%.2f", c.bearing))
        table.insert(parts, tostring(urgency))
        table.insert(parts, string.format("%.2f", c.distance))
      end
      udpSend:send(table.concat(parts, ","))
    end
  end
end

-- =================================================================================================
--  Terrain Height Sampling (drop-offs and hills)
-- =================================================================================================

-- Latch so the "this map has no terrain" note reaches the log once, not at scan rate forever.
local terrainWarned = false

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

  -- Get terrain height at vehicle position.
  -- Terrain queries need a TerrainBlock in the scene. Maps built on a flat ground plane
  -- (smallgrid and friends) have none, and core_terrain.getTerrainHeight returns nil there.
  -- The arithmetic below would then throw, and the GE onUpdate hook chain is dispatched
  -- WITHOUT pcall (lua/common/extensions.lua, hookProfiled), so one throw here silently
  -- stops every extension loaded after this one in modScript.lua.
  if not core_terrain or not core_terrain.getTerrainHeight then return end
  local baseHeight = core_terrain.getTerrainHeight(playerPos)
  if not baseHeight then
    if not terrainWarned then
      terrainWarned = true
      detLog('info', "No terrain on this map - skipping drop-off/hill sampling. Ray obstacles still active.")
    end
    return
  end
  terrainWarned = false

  local worstDrop = 0
  local worstRise = 0
  local worstDropDist = 0
  local worstRiseDist = 0

  for _, baseDist in ipairs(BASE_TERRAIN_DISTANCES) do
    local sampleDist = baseDist * speedScale
    local samplePos = playerPos + playerFwd * sampleDist
    local sampleHeight = core_terrain.getTerrainHeight(samplePos)
    -- A sample point past the edge of the terrain block reads nil; skip it, don't throw.
    if sampleHeight then
      local diff = sampleHeight - baseHeight

      if diff < -DROPOFF_THRESHOLD and diff < worstDrop then
        worstDrop = diff
        worstDropDist = sampleDist
      elseif diff > HILL_THRESHOLD and diff > worstRise then
        worstRise = diff
        worstRiseDist = sampleDist
      end
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
  setExtensionUnloadMode(M, "manual")
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
    for i = 1, NUM_RAYS do
      sweepHits[i].hit = false
      sweepHits[i].distance = math.huge
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
