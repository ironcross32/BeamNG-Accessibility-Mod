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
--  GE Extension Hooks
-- =================================================================================================

function M.onExtensionLoaded()
  camLog('info', "Camera info extension loaded.")
end

function M.onWorldReadyState(state)
  camLog('info', "onWorldReadyState triggered with state: " .. tostring(state))

  if state == 2 then
    camLog('info', "World is ready. Initializing camera info systems.")

    -- Close existing sockets before re-creating (handles map reload)
    if udpSend then pcall(function() udpSend:close() end); udpSend = nil end
    if udpCmd  then pcall(function() udpCmd:close()  end); udpCmd  = nil end

    -- Reset state for new map
    isActive = false
    timer    = 0

    -- Create send socket for camera data
    udpSend = socket.udp()
    if udpSend then
      udpSend:setpeername(PYTHON_HOST, PYTHON_PORT_DATA)
      udpSend:settimeout(0)
      camLog('info', "UDP send socket created, targeting " .. PYTHON_HOST .. ":" .. PYTHON_PORT_DATA)
    else
      camLog('error', "Failed to create UDP send socket.")
    end

    -- Create receive socket for ON/OFF commands from Python
    local ok, err = pcall(function()
      udpCmd = socket.udp()
      udpCmd:setsockname("127.0.0.1", CMD_LISTEN_PORT)
      udpCmd:settimeout(0)
    end)
    if ok and udpCmd then
      camLog('info', "UDP command socket listening on port " .. CMD_LISTEN_PORT)
    else
      camLog('error', "Failed to create UDP command socket: " .. tostring(err))
      udpCmd = nil
    end
  end
end

function M.onUpdate(dtReal, dtSim, dtRaw)
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
    local terrainHeight = core_terrain.getTerrainHeight(camPos)
    if terrainHeight then
      camAGL = camPos.z - terrainHeight
    else
      camAGL = camPos.z  -- fallback: absolute height when no terrain
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

  -- 4. Send CSV packet: "yaw,pitch,agl,bearing,distance,isFreeCam"
  local packet = string.format("%.2f,%.2f,%.2f,%.2f,%.2f,%d", camYaw, camPitch, camAGL, vehBearing, vehDist, isFreeCam)
  udpSend:send(packet)
end

return M
