-- =================================================================================================
--
--  Camera Info for BeamNG.drive Accessibility Mod (GE Extension)
--
--  Description: Sends camera spatial data (yaw, pitch, AGL, vehicle bearing/distance)
--               to the Python backend via UDP as a CSV text string.
--               Receives ON/OFF commands via UDP.
--
--  Loaded by: scripts/bng_screenreader_mod/modScript.lua
--
--  Version:     1.0
--  Target Game: BeamNG.drive 0.37+
--
-- =================================================================================================

local M = {}

-- Configuration
local PYTHON_HOST       = "127.0.0.1"
local PYTHON_PORT_DATA  = 4450   -- send camera data to Python on this port
local CMD_LISTEN_PORT   = 4451   -- receive ON/OFF commands from Python on this port
local UPDATE_INTERVAL   = 0.1    -- 10 Hz

-- Internal State
local udpSend   = nil
local udpCmd    = nil
local isActive  = false
local timer     = 0

-- Logging helper
local function camLog(level, msg)
  log(level, 'CameraInfo', msg)
end

-- =================================================================================================
--  Ground height
-- =================================================================================================

-- Height of the ground under `pos`, or nil when there is none to find.
--
-- This deliberately does NOT use core_terrain.getTerrainHeight: that only knows about a
-- TerrainBlock, and several stock maps have none at all. smallgrid is built from a
-- GroundPlane, gridmap from static meshes -- on those, terrain queries return nil for
-- every point on the map, and this used to report the camera's absolute altitude as its
-- height above ground. be:getSurfaceHeightBelow sees terrain, static meshes and ground
-- planes alike.
--
-- Two quirks of that call shape the code: it reports failure as -1e20 rather than nil, and
-- it only looks downwards -- so a camera sitting level with the ground can miss. Hence the
-- retries, the same pattern the game's own common/tech/techUtils.lua uses.
local function groundHeightBelow(pos)
  local function probe(z)
    local ok, h = pcall(function() return be:getSurfaceHeightBelow(vec3(pos.x, pos.y, z)) end)
    if ok and type(h) == "number" and h > -1e10 then return h end
    return nil
  end
  -- From the point itself first: it is the only probe that stays correct when the camera
  -- is under a bridge or inside a tunnel, where a probe from above would find the deck.
  return probe(pos.z) or probe(pos.z + 2) or probe(1e5)
end

-- =================================================================================================
--  Diagnostic
-- =================================================================================================

-- One-shot dump of every value the camera readouts are built from, so a wrong altitude can
-- be traced to the query that produced it without pasting Lua into the console.
-- Answers "DIAG" on the command port with "DIAG:<spoken summary>|<raw detail>".
--
-- Note the parentheses around the core_terrain / core_camera calls: both return NO values
-- (not nil) on some paths -- getTerrainHeight does it on every map with no terrain block,
-- getActiveCamName when the player is in no vehicle -- and tostring() of nothing at all is
-- an error, not "nil". Wrapping a call in parentheses forces it to exactly one value.
local function sendDiag()
  if not udpSend then return end

  local ok, speech, raw = pcall(function()
    local c = core_camera.getPosition()
    local p = be:getPlayerVehicle(0)
    local v = p and p:getPosition()
    local camName = (core_camera.getActiveCamName())
    local th = (core_terrain.getTerrainHeight(c))

    local function surf(z)
      local o, h = pcall(function() return be:getSurfaceHeightBelow(vec3(c.x, c.y, z)) end)
      if o and type(h) == "number" then return h end
      return nil
    end
    local ray = nil
    do
      local o, d = pcall(castRayStatic, vec3(c.x, c.y, c.z), vec3(0, 0, -1), 2000)
      if o and type(d) == "number" then ray = d end
    end

    local ground = groundHeightBelow(c)
    -- Numbers in metres, not a finished sentence: Python owns the phrasing so this speaks
    -- the same units as every other readout. Speaking bare metres here while Alt+A spoke
    -- feet made one height sound like two different ones.
    local spoken = string.format("%.3f,%s",
      c.z, ground and string.format("%.3f", ground) or "nan")
    local detail = string.format(
      "cam=%.3f,%.3f,%.3f mode=%s veh=%s terrainBlock=%s terrainH=%s surf=%s surf+2=%s surfHigh=%s rayDown=%s ground=%s",
      c.x, c.y, c.z, tostring(camName),
      v and string.format("%.3f,%.3f,%.3f", v.x, v.y, v.z) or "none",
      tostring(core_terrain.getTerrain() ~= nil), tostring(th),
      tostring(surf(c.z)), tostring(surf(c.z + 2)), tostring(surf(1e5)),
      tostring(ray), tostring(ground))
    return spoken, detail
  end)

  if ok then
    udpSend:send("DIAG:" .. speech .. "|" .. raw)
  else
    -- `speech` holds the error message when pcall fails. ERR is a sentinel the Python
    -- side recognises, so it can't be mistaken for a height.
    udpSend:send("DIAG:ERR|error " .. tostring(speech))
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
    camLog('info', "UDP send socket created, targeting " .. PYTHON_HOST .. ":" .. PYTHON_PORT_DATA)
  else
    camLog('error', "Failed to create UDP send socket.")
  end

  local ok, err = pcall(function()
    udpCmd = socket.udp()
    -- setsockname RETURNS nil plus a message; it does not THROW. A pcall around it reports
    -- success on a socket bound to nothing, and the extension then goes deaf with nothing in
    -- the log -- it still sends normally, because a UDP sender needs no bind.
    local bound, berr = udpCmd:setsockname("127.0.0.1", CMD_LISTEN_PORT)
    if not bound then error(tostring(berr), 0) end
    udpCmd:settimeout(0)
  end)
  if ok and udpCmd then
    camLog('info', "UDP command socket listening on port " .. CMD_LISTEN_PORT)
  else
    camLog('error', "Failed to create UDP command socket: " .. tostring(err))
    if udpCmd then pcall(function() udpCmd:close() end) end
    udpCmd = nil
  end
end

-- A failed bind is otherwise permanent for the session, so re-arm it. This is the recovery
-- path, not a precaution, and it has been watched doing the job: the first reload of the
-- patched files leaked eight ports, because the OUTGOING code had no unload hook yet. The
-- retry could not take them while the old module tables were still referenced -- a socket held
-- that way is not one the collector is about to free -- and ticked uselessly for two minutes.
-- The Ctrl+L that followed did NOT re-load these extensions (no load line for any of them in
-- the log at that timestamp, so setupSockets never ran again); all thirteen ports came back
-- through THIS function instead, within one frame of each other, the moment those tables went
-- away. Without it the mod would have stayed deaf until the game was restarted.
local CMD_BIND_RETRY_S = 3.0
local cmdBindRetry = 0

local function retryCmdBind(dtReal)
  if udpCmd then return end
  cmdBindRetry = cmdBindRetry + (dtReal or 0)
  if cmdBindRetry < CMD_BIND_RETRY_S then return end
  cmdBindRetry = 0
  local ok = pcall(function()
    local sk = socket.udp()
    local bound = sk:setsockname("127.0.0.1", CMD_LISTEN_PORT)
    if not bound then sk:close(); error("still in use", 0) end
    sk:settimeout(0)
    udpCmd = sk
  end)
  if ok and udpCmd then
    camLog('info', "UDP command socket bound on port " .. CMD_LISTEN_PORT .. " after retry.")
  end
end

-- setupSockets closes the sockets held by THIS module instance, and extensions.reload builds a
-- fresh instance whose locals are nil -- so it closes nothing and the outgoing instance keeps
-- the port, leaving the reloaded copy permanently deaf. Hence this hook.
function M.onExtensionUnloaded()
  if udpSend then pcall(function() udpSend:close() end); udpSend = nil end
  if udpCmd  then pcall(function() udpCmd:close()  end); udpCmd  = nil end
end

function M.onExtensionLoaded()
  setExtensionUnloadMode(M, "manual")
  camLog('info', "Camera info extension loaded.")
  -- Bind sockets here so Ctrl+L Lua reload re-opens them.
  setupSockets()
end

function M.onWorldReadyState(state)
  camLog('info', "onWorldReadyState triggered with state: " .. tostring(state))

  if state == 2 then
    camLog('info', "World is ready. Initializing camera info systems.")

    -- isActive deliberately survives a map load. Clearing it here stopped the feed while
    -- Python still believed it was running, so the readouts went on speaking the last
    -- packet they had received -- a frozen altitude that sounds exactly like a live one.
    -- (A Lua reload still resets it, since the whole module is rebuilt; Python notices the
    -- silence and asks for the feed again.)
    timer = 0

    setupSockets()
  end
end

function M.onUpdate(dtReal, dtSim, dtRaw)
  retryCmdBind(dtReal)
  -- 1. Poll UDP for ON/OFF commands from Python
  if udpCmd then
    local data = udpCmd:receive()
    if data then
      local cmd = data:match("^%s*(.-)%s*$"):upper()
      if cmd == "ON" and not isActive then
        isActive = true
        camLog('info', "Camera info activated via UDP.")
      elseif cmd == "OFF" and isActive then
        isActive = false
        camLog('info', "Camera info deactivated via UDP.")
      elseif cmd == "DIAG" then
        -- Answers whether or not the feed is active: it is a one-shot query.
        sendDiag()
      end
    end
  end

  if not isActive then return end
  if not udpSend then return end

  -- 2. Rate-limit to UPDATE_INTERVAL seconds
  timer = timer + dtReal
  if timer < UPDATE_INTERVAL then return end
  timer = 0

  -- 3. Gather camera data (all wrapped in pcall for robustness)
  local camYaw, camPitch, camAGL, vehBearing, vehDist, isFreeCam = 0, 0, 0, 0, -1, 0
  local aglValid = 0

  local ok, err = pcall(function()
    -- Camera forward and position
    local fwd = core_camera.getForward()
    local camPos = core_camera.getPosition()
    -- Camera yaw: negate atan2 args to match MotionSim heading convention
    camYaw = math.deg(math.atan2(-fwd.x, -fwd.y)) % 360.0

    -- Camera pitch: asin(forward.z) -> degrees (positive = looking up)
    local fwdLen = math.sqrt(fwd.x * fwd.x + fwd.y * fwd.y + fwd.z * fwd.z)
    if fwdLen > 0.001 then
      camPitch = math.deg(math.asin(math.max(-1, math.min(1, fwd.z / fwdLen))))
    end

    -- Camera AGL (above ground level)
    local groundZ = groundHeightBelow(camPos)
    if groundZ then
      camAGL = camPos.z - groundZ
      aglValid = 1
    else
      -- Nothing under the camera at all: over open water, or off the edge of the world.
      -- Send the absolute height with the flag clear so Python can say which one it is,
      -- rather than passing sea level off as ground level.
      camAGL = camPos.z
      aglValid = 0
    end

    -- Is free camera?
    local camName = core_camera.getActiveCamName()
    isFreeCam = (camName == "free") and 1 or 0

    -- Vehicle bearing and distance
    -- Bearing = degrees the vehicle must turn to face the camera (0 = on course)
    local player = be:getPlayerVehicle(0)
    if player then
      local vehPos = player:getPosition()
      vehDist = camPos:distance(vehPos)

      -- Vehicle forward and direction from vehicle to camera
      local vehFwd = player:getDirectionVector()
      local toCam = (camPos - vehPos):normalized()

      local vehFwdFlat = vec3(vehFwd.x, vehFwd.y, 0):normalized()
      local toCamFlat = vec3(toCam.x, toCam.y, 0):normalized()
      local cosAngle = vehFwdFlat:dot(toCamFlat)
      local angle = math.deg(math.acos(math.max(-1, math.min(1, cosAngle))))
      local up = vec3(0, 0, 1)
      local vehLeft = up:cross(vehFwdFlat)
      local signDot = vehLeft:dot(toCamFlat)
      vehBearing = angle * (signDot < 0 and -1 or 1)
    end
  end)

  if not ok then
    camLog('warn', "Error gathering camera data: " .. tostring(err))
    return
  end

  -- 4. Send CSV packet: "yaw,pitch,agl,bearing,distance,isFreeCam,aglValid"
  -- aglValid is appended rather than replacing a field, so a Python side that still
  -- expects the original six keeps working.
  local packet = string.format("%.2f,%.2f,%.2f,%.2f,%.2f,%d,%d",
    camYaw, camPitch, camAGL, vehBearing, vehDist, isFreeCam, aglValid)
  udpSend:send(packet)
end

return M
