-- cameraLineup.lua
--
-- Raw ground-plane geometry for the free-camera line-up guide (F9 then Alt+G on the Python
-- side): where the free camera is and which way it looks, and where the front centre of the
-- driven vehicle is and which way it points. Python decides what "lined up" means -- the mode
-- (head-on or crossing), the target distance and the mute all live there -- so this file has
-- nothing to be told.
--
-- Send only, port 4489. No command port, for the reason trailerAngle.lua and cannonShot.lua
-- give: this file binds no listening socket, so it is outside the contract the listening
-- extensions share and that vehicle_geometry_sim.lua scenario 12 polices.
--
-- Packets:
--   LINEUP:camX,camY,camFwdX,camFwdY,noseX,noseY,vehFwdX,vehFwdY   while the free camera is
--                                                                   active and a vehicle exists
--   LINEUP:OFF                                                     on leaving free camera,
--                                                                   or with no vehicle
-- Both forward vectors are FLATTENED to the ground plane and unit length. Positions are world
-- metres. Everything is 2D on purpose: the guide is about where a car on the ground will pass,
-- and pitch folded into the heading would invent a miss on any slope.

local M = {}

local PYTHON_HOST      = "127.0.0.1"
local PYTHON_PORT_DATA = 4489

-- The tone smooths its own parameters, and a car being driven into place does not need more.
local TICK_HZ = 20.0
-- OFF is re-sent at this interval while not in free camera, so a Python started after the
-- transition still learns it. Python also ages the feed out on its own.
local OFF_HEARTBEAT_S = 1.0

local udpSend = nil
local tickAcc = 0
local sinceT = 0
local lastOffT = -1e9
local wasLive = false
local lastGoodVehFwd = nil
local lastGoodCamFwd = nil

local function clLog(level, msg) log(level, 'cameraLineup', msg) end

local function send(line)
  if udpSend then pcall(function() udpSend:send(line) end) end
end

-- Flatten to the ground plane; nil when it collapses to residue (pointing straight up/down).
local function flat(v)
  if not v then return nil end
  local f = vec3(v.x, v.y, 0)
  if f:length() < 1e-3 then return nil end
  return f:normalized()
end

local function sendOff(force)
  if wasLive or force or (sinceT - lastOffT) >= OFF_HEARTBEAT_S then
    send("LINEUP:OFF")
    lastOffT = sinceT
  end
  wasLive = false
end

-- The front centre of the vehicle: forward to the measured nose, and laterally to the middle
-- of the measured width (a body is not always centred on its origin). Without measured extents
-- the origin stands in; the line through it along the heading is the same line, only the
-- "is the target ahead of the nose" test loses a couple of metres.
local function nosePoint(veh, frame)
  local nose = frame.c
  local entry = extensions.vehicleGeometry and extensions.vehicleGeometry.get(veh:getID())
  if entry and entry.ext then
    local midR = 0.5 * (entry.ext.minR + entry.ext.maxR)
    nose = frame.c + frame.f * entry.ext.maxF + frame.r * midR
  elseif extensions.vehicleGeometry then
    extensions.vehicleGeometry.request(veh:getID())
  end
  return nose
end

local function tick()
  local camName = (core_camera.getActiveCamName())
  if camName ~= "free" then sendOff(false) return end

  local veh = be:getPlayerVehicle(0)
  if not veh then sendOff(false) return end
  local frame = extensions.vehicleGeometry and extensions.vehicleGeometry.boxFrame(veh:getID())
  if not frame then sendOff(false) return end

  local vf = flat(frame.f) or lastGoodVehFwd
  if not vf then sendOff(false) return end
  lastGoodVehFwd = vf

  -- Looking straight down, the camera's forward flattens to nothing; the top of the screen
  -- (its up vector, flattened) is then the direction it is facing on the ground.
  local cf = flat(core_camera.getForward()) or flat(core_camera.getUp()) or lastGoodCamFwd
  if not cf then sendOff(false) return end
  lastGoodCamFwd = cf

  local cam = core_camera.getPosition()
  local nose = nosePoint(veh, frame)
  send(string.format("LINEUP:%.3f,%.3f,%.4f,%.4f,%.3f,%.3f,%.4f,%.4f",
    cam.x, cam.y, cf.x, cf.y, nose.x, nose.y, vf.x, vf.y))
  wasLive = true
end

local function setupSockets()
  if udpSend then pcall(function() udpSend:close() end); udpSend = nil end
  udpSend = socket.udp()
  if udpSend then
    udpSend:setpeername(PYTHON_HOST, PYTHON_PORT_DATA)
    udpSend:settimeout(0)
  else
    clLog('E', "Failed to create UDP send socket.")
  end
end

function M.onExtensionLoaded()
  setExtensionUnloadMode(M, "manual")
  setupSockets()
  clLog('I', "Camera line-up feed loaded.")
end

function M.onExtensionUnloaded()
  if udpSend then
    pcall(function() udpSend:send("LINEUP:OFF") end)
    pcall(function() udpSend:close() end)
    udpSend = nil
  end
end

function M.onWorldReadyState(state)
  if state == 2 then
    tickAcc = 0
    wasLive = false
    lastGoodVehFwd = nil
    lastGoodCamFwd = nil
    setupSockets()
  end
end

function M.onVehicleSwitched(oldId, newId, player)
  lastGoodVehFwd = nil
end

function M.onVehicleResetted(vehID)
  lastGoodVehFwd = nil
end

function M.onUpdate(dtReal, dtSim, dtRaw)
  -- The GE onUpdate chain is dispatched WITHOUT pcall; never let a throw escape.
  sinceT = sinceT + dtReal
  tickAcc = tickAcc + dtReal
  if tickAcc < 1.0 / TICK_HZ then return end
  tickAcc = 0
  local ok, err = pcall(tick)
  if not ok then clLog('E', "tick failed: " .. tostring(err)) end
end

return M
