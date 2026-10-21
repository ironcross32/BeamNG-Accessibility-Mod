-- wheelReport.lua
--
-- Every wheel on the driven vehicle and on whatever is coupled to it: where it sits, its tire
-- pressure and its brake temperature. Python turns the positions into the status rows
-- ("front left", "axle 2 left dual", "trailer rear right") -- wheel_report.py.
--
-- The extended telemetry struct cannot carry this. It has six fixed wheel slots matched by jbeam
-- NAME (FL FR RL RR F R), so the 0.40 Percheron -- wheels FL FR R2LL R2RR TLL TLR -- showed its
-- front axle and nothing else, and a trailer's wheels live in a different vehicle VM entirely.
-- So this file asks each vehicle VM for its raw wheel list, and labels are derived from
-- geometry rather than names: names differ per vehicle, positions do not lie.
--
-- Raw on purpose. The vehicle VM reports longitudinal and lateral position, tire width,
-- pressure and brake temperature, and nothing about axles or sides; the grouping is pure Python
-- so diagnostic/wheel_report_sim.py can assert it without a game.
--
-- Send only, port 4485, like trailerAngle.lua: nothing to be told, so no listening socket and
-- none of the bind contract the listening extensions share.

local M = {}

local PYTHON_HOST      = "127.0.0.1"
local PYTHON_PORT_DATA = 4485

-- Pressures and brake temperatures change over seconds, and the status screen reads on demand.
-- Two requests a second keeps a row current to the moment it is read.
local TICK_HZ = 2.0
-- A reply older than this belongs to a VM that stopped answering (reloading, destroyed).
local REPLY_STALE_S = 1.5

local udpSend = nil
local tickAcc = 0
local sinceT = 0
local replies = {}   -- vehID -> {t = sinceT, payload = string}

local function wrLog(level, msg) log(level, 'wheelReport', msg) end

local function send(text)
  if udpSend then pcall(function() udpSend:send(text) end) end
end

local function cleanName(s)
  s = tostring(s or "")
  s = s:gsub("[,|;\r\n]", " ")
  s = s:gsub("^%s*(.-)%s*$", "%1")
  if s == "" or s:find("^table: ") then return "unknown" end
  return s
end

local function nameOf(veh)
  local ok, n = pcall(function()
    if extensions and extensions.vehicleNaming and extensions.vehicleNaming.describe then
      return extensions.vehicleNaming.describe(veh)
    end
    return nil
  end)
  if ok and type(n) == "string" and n ~= "" then return cleanName(n) end
  local f = veh:getJBeamFilename() or "unknown"
  return cleanName(f:match("([^/\\]+)%.jbeam$") or f)
end

-- ================================================================================================
--  The vehicle-VM half
-- ================================================================================================
-- A cross-VM chunk: no percent sign and no doubled close-square-bracket anywhere in it, code or
-- comment, or it fails to compile in the vehicle VM with nothing logged here. Hence the rounding
-- by math.floor rather than string.format. wheel_report_sim.py checks both.
--
-- Payload: "<dual>|lon,lat,width,psi,brakeC,broken;..." with psi and brakeC EMPTY when unknown --
-- a wheel with no pressure group (a solid tire), or brakes with no thermal model. Zero is a real
-- reading for both, so it can never stand in for "unknown". Lateral is up:cross(fwd), positive
-- LEFT, the mod-wide convention. dual is the jbeam's simplifiedDuallies switch: with it on, a
-- pair of rear duals is ONE physics wheel twice as wide, and only the width says so.
local VEHICLE_CHUNK = [[
local function r1(x) return math.floor(x * 10 + 0.5) / 10 end
local ok, res = pcall(function()
  if not (wheels and wheels.wheels and v and v.data) then return "" end
  local fwd = obj:getDirectionVector():normalized()
  local up = obj:getDirectionVectorUp():normalized()
  local left = up:cross(fwd)
  local env = (powertrain and powertrain.currentEnvPressure) or 101325
  local groups = v.data.pressureGroups or {}
  local comps = v.data.components or {}
  local out = {}
  for _, wd in pairs(wheels.wheels) do
    if wd.node1 and wd.node2 then
      local p = (obj:getNodePosition(wd.node1) + obj:getNodePosition(wd.node2)) * 0.5
      local psi = ""
      local g = wd.pressureGroup and groups[wd.pressureGroup]
      if g then
        psi = tostring(r1(math.max(0, obj:getGroupPressure(g) - env) * 0.000145038))
      end
      local brake = ""
      if wd.enableBrakeThermals and wd.brakeSurfaceTemperature then
        brake = tostring(r1(wd.brakeSurfaceTemperature))
      end
      out[#out + 1] = tostring(r1(p:dot(fwd))) .. "," .. tostring(r1(p:dot(left))) .. ","
        .. tostring(math.floor((wd.tireWidth or 0) * 100 + 0.5) / 100) .. "," .. psi .. "," .. brake .. ","
        .. (wd.isBroken and "1" or "0")
    end
  end
  return (comps.simplifiedDuallies == true and "1" or "0") .. "|" .. table.concat(out, ";")
end)
if ok and res ~= "" then
  obj:queueGameEngineLua("if extensions.wheelReport then extensions.wheelReport.onWheels("
    .. tostring(obj:getID()) .. ", '" .. res .. "') end")
end
]]

-- Called back from each vehicle VM.
function M.onWheels(vehID, payload)
  if type(vehID) ~= "number" or type(payload) ~= "string" then return end
  replies[vehID] = {t = sinceT, payload = payload}
end

-- ================================================================================================
--  What is coupled
-- ================================================================================================
-- Everything reachable from the player through core_vehicles.attachedCouplers, nearest hop
-- first -- a dolly before the trailer on it. The same registry trailerAngle.lua reads, for the
-- same reason: it is maintained on attach, detach and destroy and survives a save reload, so a
-- trailer coupled any way at all is in it.
local function coupledChain(playerID)
  local order, seen = {}, {[playerID] = true}
  local frontier = {playerID}
  local ok = pcall(function()
    local pairsList = core_vehicles and core_vehicles.attachedCouplers
    if type(pairsList) ~= "table" then return end
    while #frontier > 0 do
      local nextFrontier = {}
      for _, id in ipairs(frontier) do
        for _, c in ipairs(pairsList) do
          if type(c) == "table" then
            local other = nil
            if c[1] == id then other = c[2] elseif c[2] == id then other = c[1] end
            if other and not seen[other] then
              seen[other] = true
              order[#order + 1] = other
              nextFrontier[#nextFrontier + 1] = other
            end
          end
        end
      end
      frontier = nextFrontier
    end
  end)
  if not ok then return {} end
  return order
end

-- ================================================================================================
--  Tick
-- ================================================================================================
-- Packet, newline separated:
--   WHEELS
--   V,<role>,<vehID>,<name>        role 0 is the driven vehicle, 1.. the coupled ones in order
--   <the vehicle VM's payload>
-- A bare "WHEELS" is a real message: no vehicle, so Python drops its rows now rather than
-- waiting out its own age-out.
local function tick()
  local player = be:getPlayerVehicle(0)
  if not player then send("WHEELS") return end
  local playerID = player:getID()

  local ids = {playerID}
  for _, id in ipairs(coupledChain(playerID)) do ids[#ids + 1] = id end

  local lines = {"WHEELS"}
  for role, id in ipairs(ids) do
    local veh = (role == 1) and player or be:getObjectByID(id)
    if veh then
      veh:queueLuaCommand(VEHICLE_CHUNK)
      local r = replies[id]
      if r and (sinceT - r.t) <= REPLY_STALE_S then
        lines[#lines + 1] = string.format("V,%d,%d,%s", role - 1, id, nameOf(veh))
        lines[#lines + 1] = r.payload
      end
    end
  end
  send(table.concat(lines, "\n"))
end

-- ================================================================================================
--  Lifecycle
-- ================================================================================================
local function setupSockets()
  if udpSend then pcall(function() udpSend:close() end); udpSend = nil end
  udpSend = socket.udp()
  if udpSend then
    udpSend:setpeername(PYTHON_HOST, PYTHON_PORT_DATA)
    udpSend:settimeout(0)
  else
    wrLog('E', "Failed to create UDP send socket.")
  end
end

function M.onExtensionLoaded()
  setExtensionUnloadMode(M, "manual")
  setupSockets()
  wrLog('I', "Wheel report loaded.")
end

function M.onExtensionUnloaded()
  if udpSend then pcall(function() udpSend:close() end); udpSend = nil end
end

function M.onWorldReadyState(state)
  if state == 2 then
    replies = {}
    tickAcc = 0
    setupSockets()
  end
end

function M.onVehicleDestroyed(vehID)
  replies[vehID] = nil
end

function M.onUpdate(dtReal, dtSim, dtRaw)
  -- The GE onUpdate chain is dispatched WITHOUT pcall; a throw here would stop every
  -- extension loaded after this one.
  sinceT = sinceT + dtReal
  tickAcc = tickAcc + dtReal
  if tickAcc < 1.0 / TICK_HZ then return end
  tickAcc = 0
  local ok, err = pcall(tick)
  if not ok then wrLog('E', "tick failed: " .. tostring(err)) end
end

-- What each vehicle last answered, raw.
function M.diag()
  local out = {}
  local player = be:getPlayerVehicle(0)
  if not player then
    out[#out + 1] = "no player vehicle"
  else
    local ids = {player:getID()}
    for _, id in ipairs(coupledChain(ids[1])) do ids[#ids + 1] = id end
    for role, id in ipairs(ids) do
      local r = replies[id]
      out[#out + 1] = string.format("role %d id %d: %s", role - 1, id,
        r and string.format("%.1fs old  %s", sinceT - r.t, r.payload) or "no reply")
    end
  end
  local text = table.concat(out, "\n")
  print(text)
  return text
end

return M
