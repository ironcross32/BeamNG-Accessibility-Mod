-- =================================================================================================
--
--  Road Detector for BeamNG.drive Accessibility Mod (GE Extension)
--
--  Description: Uses the game's nav graph (the same one the AI drives on) to detect whether the
--               player vehicle is currently on a road. When off-road, sends the bearing and
--               distance to the closest road segment so Python can play an HRTF-spatialized beep
--               pointing the player back to the road.
--
--               If no roads exist on this map (off-road playgrounds, gridmaps, etc.), the detector
--               goes dormant after a brief search and stops ticking until the world is reloaded
--               (map change, Ctrl+L Lua reload, or toggling the feature off and back on).
--
--  Loaded by:   scripts/bng_screenreader_mod/modScript.lua
--
--  Version:     1.0
--  Target Game: BeamNG.drive 0.37+
--
-- =================================================================================================

local M = {}

-- Configuration
local PYTHON_HOST       = "127.0.0.1"
local PYTHON_PORT_DATA  = 4462    -- send road status to Python on this port
local CMD_LISTEN_PORT   = 4463    -- receive ON/OFF commands from Python on this port
local SCAN_INTERVAL     = 0.2     -- 5 Hz ticks
local SEARCH_RADIUS_M   = 500.0   -- how far to search for the closest road segment
local DORMANT_AFTER_MISSES = 5    -- consecutive empty searches (~1s) before going dormant
local OVERPASS_Z_TOLERANCE = 6.0  -- |Δz| above which a "closest" segment is treated as overhead
local ROAD_RADIUS_SLACK = 0.5     -- meters added to lane radius when deciding "on road"
local ON_ROAD_STABLE_TICKS = 2    -- require this many consecutive on-road ticks before firing the
                                  -- orientation chime (filters out grazing crossings)

-- Alignment criteria: the chime keeps repeating until the player is moving along the road in
-- one of its travel directions for ALIGNED_REQUIRED_TICKS consecutive frames. This prevents
-- the chime from cutting off while the driver is still figuring out which way to turn, and
-- makes sure they've actually pointed the car the right way and started rolling.
local ALIGNED_THRESHOLD_DEG = 25.0   -- velocity must be within this angle of segment direction
local ALIGNED_MIN_SPEED_MPS = 3.0    -- and speed must exceed this (~6.7 mph)
local ALIGNED_REQUIRED_TICKS = 5     -- and it must hold for this many ticks (~1s @ 5Hz)

-- Internal state
local udpSend           = nil
local udpCmd            = nil
local isActive          = false   -- user toggle
local isDormant         = false   -- gave up: no roads on this map
local missCount         = 0
local scanTimer         = 0
local lastSentState     = nil     -- "ON_ROAD" / "OFF_ROAD" / "DORMANT" — to debounce send
local consecutiveOnRoad = 0       -- ticks the player has been continuously on a road
local chimeActive       = false   -- chime is currently playing/repeating; cleared when aligned
local alignedTicks      = 0       -- ticks of sustained correct-direction driving

local function rdLog(level, msg) log(level, 'RoadDetector', msg) end

-- =================================================================================================
--  Closest-point-on-segment in 2D (xy plane). Returns vec3 with z carried from the segment.
-- =================================================================================================

local function closestPointOnSegment2D(p, a, b)
  local abx, aby = b.x - a.x, b.y - a.y
  local apx, apy = p.x - a.x, p.y - a.y
  local lenSq = abx * abx + aby * aby
  if lenSq < 1e-9 then return vec3(a.x, a.y, a.z), 0.0 end
  local t = (apx * abx + apy * aby) / lenSq
  if t < 0 then t = 0 elseif t > 1 then t = 1 end
  local cx = a.x + t * abx
  local cy = a.y + t * aby
  local cz = a.z + t * (b.z - a.z)
  return vec3(cx, cy, cz), t
end

-- =================================================================================================
--  Compute the bearing (deg, signed; +right) from playerFwd to a target direction in the xy plane.
--  Same sign convention as obstacleDetector: positive = right of the vehicle when viewed from above.
-- =================================================================================================

local function bearingToDir(playerFwd, playerUp, dirVec)
  local crossV = playerFwd:cross(dirVec)
  local dotV   = playerFwd:dot(dirVec)
  local rad    = math.atan2(playerUp:dot(crossV), dotV)
  return math.deg(rad)
end

-- =================================================================================================
--  Send a status string, but only if it differs from the last one we sent (or if it's OFF_ROAD,
--  which carries changing distance/bearing data).
-- =================================================================================================

local function sendStatus(payload, force)
  if not udpSend then return end
  if not force and payload == lastSentState then return end
  udpSend:send(payload)
  lastSentState = payload
end

-- =================================================================================================
--  Core: probe the nav graph and figure out where the road is relative to us.
-- =================================================================================================

local function performScan()
  if not udpSend then return end
  if not map or not map.findBestRoad then return end

  local player = be:getPlayerVehicle(0)
  if not player then return end

  local playerPos = player:getPosition()
  local playerFwd = player:getDirectionVector()
  local playerUp  = player:getDirectionVectorUp()

  local n1id, n2id, dist2D = map.findBestRoad(playerPos, playerFwd)
  if not n1id then
    -- Fallback: try a wider radius search
    n1id, n2id, dist2D = map.findClosestRoad(playerPos, SEARCH_RADIUS_M)
  end

  if not n1id then
    missCount = missCount + 1
    if missCount >= DORMANT_AFTER_MISSES then
      isDormant = true
      sendStatus("DORMANT", false)
      rdLog('info', "No roads found on this map — going dormant.")
    end
    return
  end

  -- Got a hit; reset miss counter
  missCount = 0

  local mapData = map.getMap()
  if not mapData or not mapData.nodes then return end
  local n1 = mapData.nodes[n1id]
  local n2 = mapData.nodes[n2id]
  if not n1 or not n2 then return end

  local cp = closestPointOnSegment2D(playerPos, n1.pos, n2.pos)
  local laneRadius = math.max(n1.radius or 4.0, n2.radius or 4.0)

  -- Reject overhead matches (overpass above us, tunnel road below) — those aren't actually
  -- "the road we're trying to be on." If the closest segment is far in z, treat as off-road
  -- toward the planar projection.
  local dz = cp.z - playerPos.z
  local horizDx = cp.x - playerPos.x
  local horizDy = cp.y - playerPos.y
  local horizDist = math.sqrt(horizDx * horizDx + horizDy * horizDy)

  local onRoad = (horizDist <= (laneRadius + ROAD_RADIUS_SLACK)) and (math.abs(dz) <= OVERPASS_Z_TOLERANCE)

  if onRoad then
    consecutiveOnRoad = consecutiveOnRoad + 1

    if consecutiveOnRoad < ON_ROAD_STABLE_TICKS then
      -- Warmup tick: silence the off-road beep but don't chime yet (filters grazing crossings).
      sendStatus("ON_ROAD", false)
      return
    end

    -- Compute segment travel directions and player forward in xy.
    local segDir = vec3(n2.pos.x - n1.pos.x, n2.pos.y - n1.pos.y, 0)
    if segDir:length() < 1e-6 then
      sendStatus("ON_ROAD", false)
      return
    end
    segDir = segDir:normalized()
    local segDirRev = vec3(-segDir.x, -segDir.y, 0)

    local fwdFlat = vec3(playerFwd.x, playerFwd.y, 0)
    if fwdFlat:length() < 1e-6 then
      sendStatus("ON_ROAD", false)
      return
    end
    fwdFlat = fwdFlat:normalized()

    -- First on-road tick after warmup: arm the chime.
    if not chimeActive then
      chimeActive = true
      alignedTicks = 0
    end

    -- Check whether the driver is now moving along the road in the right direction.
    local vel = player:getVelocity()
    local velFlat = vec3(vel.x, vel.y, 0)
    local speed = velFlat:length()
    local cosThresh = math.cos(math.rad(ALIGNED_THRESHOLD_DEG))
    if speed >= ALIGNED_MIN_SPEED_MPS then
      local velDir = velFlat:normalized()
      local bestDot = math.max(velDir:dot(segDir), velDir:dot(segDirRev))
      if bestDot >= cosThresh then
        alignedTicks = alignedTicks + 1
      else
        alignedTicks = 0
      end
    else
      alignedTicks = 0
    end

    if alignedTicks >= ALIGNED_REQUIRED_TICKS then
      -- Driver has lined up and committed to the road — silence the chime.
      chimeActive = false
      sendStatus("ON_ROAD", false)
      return
    end

    -- Chime still active: emit the chime payload with fresh bearings each tick. Python paces
    -- the actual playback so successive payloads don't pile up on top of each other.
    local upRef = vec3(0, 0, 1)
    local b1 = bearingToDir(fwdFlat, upRef, segDir)
    local b2 = bearingToDir(fwdFlat, upRef, segDirRev)
    local bFirst, bSecond
    if math.abs(b1) <= math.abs(b2) then
      bFirst, bSecond = b1, b2
    else
      bFirst, bSecond = b2, b1
    end
    local payload = string.format("ON_ROAD,%.2f,%.2f", bFirst, bSecond)
    udpSend:send(payload)
    lastSentState = payload
    return
  end

  -- Off-road: reset chime state so re-entering the road triggers a fresh chime
  consecutiveOnRoad = 0
  chimeActive = false
  alignedTicks = 0

  -- Off-road: bearing from forward to the closest point, in the xy plane
  local toRoad = vec3(horizDx, horizDy, 0)
  if toRoad:length() < 1e-6 then
    sendStatus("ON_ROAD", false)
    return
  end
  toRoad = toRoad:normalized()

  local fwdFlat = vec3(playerFwd.x, playerFwd.y, 0)
  if fwdFlat:length() < 1e-6 then return end
  fwdFlat = fwdFlat:normalized()

  local upRef = vec3(0, 0, 1)
  local bearing = bearingToDir(fwdFlat, upRef, toRoad)

  -- Always force-send OFF_ROAD; bearing/distance change continuously.
  local payload = string.format("OFF_ROAD,%.2f,%.2f", bearing, horizDist)
  sendStatus(payload, true)
end

-- =================================================================================================
--  Sockets / lifecycle
-- =================================================================================================

local function setupSockets()
  if udpSend then pcall(function() udpSend:close() end); udpSend = nil end
  if udpCmd  then pcall(function() udpCmd:close()  end); udpCmd  = nil end

  udpSend = socket.udp()
  if udpSend then
    udpSend:setpeername(PYTHON_HOST, PYTHON_PORT_DATA)
    udpSend:settimeout(0)
    rdLog('info', "UDP send socket created, targeting " .. PYTHON_HOST .. ":" .. PYTHON_PORT_DATA)
  else
    rdLog('error', "Failed to create UDP send socket.")
  end

  local ok, err = pcall(function()
    udpCmd = socket.udp()
    udpCmd:setsockname("127.0.0.1", CMD_LISTEN_PORT)
    udpCmd:settimeout(0)
  end)
  if ok and udpCmd then
    rdLog('info', "UDP command socket listening on port " .. CMD_LISTEN_PORT)
  else
    rdLog('error', "Failed to create UDP command socket: " .. tostring(err))
    udpCmd = nil
  end
end

local function rearm(reason)
  isDormant         = false
  missCount         = 0
  scanTimer         = 0
  lastSentState     = nil
  consecutiveOnRoad = 0
  chimeActive       = false
  alignedTicks      = 0
  if reason then rdLog('info', "Re-arming road detector: " .. reason) end
end

function M.onExtensionLoaded()
  rdLog('info', "Road detector extension loaded.")
  setupSockets()
  rearm("extension load")
end

function M.onWorldReadyState(state)
  if state == 2 then
    rdLog('info', "World ready. Resetting road detector state.")
    setupSockets()
    rearm("world ready")
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
        rearm("toggle on")
        rdLog('info', "Road detection activated.")
      elseif cmd == "OFF" and isActive then
        isActive = false
        rdLog('info', "Road detection deactivated.")
      end
    end
  end

  if not isActive then return end
  if isDormant then return end

  scanTimer = scanTimer + dtReal
  if scanTimer >= SCAN_INTERVAL then
    scanTimer = 0
    performScan()
  end
end

return M
