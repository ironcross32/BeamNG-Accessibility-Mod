-- cannonGeometry.lua
--
-- Live barrel geometry and pre-shot launch-speed prediction for the stock Old Cannon. Node
-- names and processed JBeam properties resolve in the vehicle VM, then the GE VM samples
-- their physical positions.

local M = {}

local RESOLVE_TIMEOUT_S = 3.0
local MAX_TRIES = 3
local cache, pending, failed, notReady = {}, {}, {}, {}
local epochCounter = 0

local function cgLog(level, msg) log(level, 'cannonGeometry', msg) end

-- cannon_8 is the breech, cannon_9 the muzzle, ball_42 the launcher attachment, and
-- ball_43 the projectile centre. The speed estimate is the spring work available before the
-- support beam reaches its long bound, divided by the complete processed ball-node mass.
-- Reading processed data keeps the estimate in step with the user's Powder and Weight values.
local VEH_SCRIPT = [[
local EPOCH = %d
local function reply(breech, muzzle, ball, speed, power, mass, reason)
  obj:queueGameEngineLua(
    "if extensions.cannonGeometry then extensions.cannonGeometry.onGeometry("
    .. obj:getID() .. "," .. EPOCH .. "," .. breech .. "," .. muzzle .. ","
    .. ball .. "," .. speed .. "," .. power .. "," .. mass .. ",'"
    .. reason .. "') end")
end
local ok, err = pcall(function()
  if not (v.data and v.data.nodes) then
    return reply(-1, -1, -1, -1, -1, -1, "no node data")
  end
  local breech, muzzle, ballAttach, ball = -1, -1, -1, -1
  local mass = 0
  for _, nd in pairs(v.data.nodes) do
    if nd.name == "cannon_8" then breech = nd.cid end
    if nd.name == "cannon_9" then muzzle = nd.cid end
    if nd.name == "ball_42" then ballAttach = nd.cid end
    if nd.name == "ball_43" then ball = nd.cid end
    if type(nd.name) == "string" and nd.name:match("^ball_%%d+$") then
      mass = mass + (tonumber(nd.nodeWeight) or 0)
    end
  end
  if breech < 0 or muzzle < 0 or ballAttach < 0 or ball < 0 then
    return reply(-1, -1, -1, -1, -1, mass, "Old Cannon nodes not found")
  end

  local launcher = nil
  for _, beam in pairs(v.data.beams or {}) do
    local connectsLauncher =
      (beam.id1 == breech and beam.id2 == ballAttach)
      or (beam.id2 == breech and beam.id1 == ballAttach)
    if connectsLauncher and beam.beamType == 7
      and (tonumber(beam.beamPrecompression) or 1) > 1 then
      launcher = beam
      break
    end
  end
  if not launcher or mass <= 0 then
    return reply(-1, -1, -1, -1, -1, mass, "Old Cannon launcher data not found")
  end

  local refLength = obj:getBeamRefLength(launcher.cid)
  local power = tonumber(launcher.beamSpring) or 0
  local precompression = tonumber(launcher.beamPrecompression) or 1
  local longBound = tonumber(launcher.beamLongBound) or 1
  local springLength = refLength * precompression
  local releaseLength = math.min(springLength, refLength * (1 + longBound))
  local initialCompression = math.max(0, springLength - refLength)
  local finalCompression = math.max(0, springLength - releaseLength)
  local work = 0.5 * power
    * (initialCompression * initialCompression - finalCompression * finalCompression)
  local speed = (work > 0 and mass > 0) and math.sqrt(2 * work / mass) or -1
  if speed <= 0 then
    return reply(-1, -1, -1, -1, power, mass, "invalid Old Cannon launch model")
  end
  reply(breech, muzzle, ball, speed, power, mass, "")
end)
if not ok then
  obj:queueGameEngineLua("log('E','cannonGeometry','vehicle-side resolve failed: "
    .. tostring(err):gsub("'", " ") .. "')")
end
]]

function M.onGeometry(vehID, epoch, breech, muzzle, ball, speed, power, mass, reason)
  local p = pending[vehID]
  if not p or p.epoch ~= epoch then return end
  pending[vehID] = nil
  if breech < 0 or muzzle < 0 or ball < 0 or speed <= 0 then
    local why = tostring(reason or "")
    if why == "no node data" then
      local n = (notReady[vehID] or 0) + 1
      notReady[vehID] = n
      if n < MAX_TRIES then return end
    end
    failed[vehID] = true
    return
  end
  cache[vehID] = {
    breech = breech, muzzle = muzzle, ball = ball,
    speed = speed, power = power, mass = mass,
  }
  notReady[vehID] = nil
  cgLog('I', string.format(
    "Old Cannon geometry on vehicle %d: breech %d, muzzle %d, ball %d, "
      .. "predicted speed %.2f m/s (power %.0f, mass %.1f kg)",
    vehID, breech, muzzle, ball, speed, power, mass))
end

function M.request(vehID)
  if not vehID or cache[vehID] or pending[vehID] or failed[vehID] then return end
  local veh = scenetree.findObjectById(vehID)
  if not veh then return end
  epochCounter = epochCounter + 1
  pending[vehID] = {epoch = epochCounter, timer = 0, tries = 1}
  pcall(function() veh:queueLuaCommand(string.format(VEH_SCRIPT, epochCounter)) end)
end

function M.has(vehID)
  M.request(vehID)
  return cache[vehID] ~= nil
end

function M.frame(vehID)
  local entry = cache[vehID]
  local veh = entry and scenetree.findObjectById(vehID) or nil
  if not veh then return nil end
  local ok, res = pcall(function()
    local base = vec3(veh:getPosition())
    local breech = base + vec3(veh:getNodePosition(entry.breech))
    local muzzle = base + vec3(veh:getNodePosition(entry.muzzle))
    local bore = muzzle - breech
    local len = bore:length()
    if len < 0.5 then return nil end
    bore = bore / len
    local elevation = math.deg(math.asin(math.max(-1, math.min(1, bore.z))))
    return {breech = breech, muzzle = muzzle, bore = bore, elevation = elevation}
  end)
  return ok and res or nil
end

function M.getLaunchSpeed(vehID)
  local entry = cache[vehID]
  return entry and entry.speed or nil
end

local function dropVehicle(vehID)
  cache[vehID], pending[vehID], failed[vehID], notReady[vehID] = nil, nil, nil, nil
end

function M.invalidate(vehID)
  if vehID == nil then
    cache, pending, failed, notReady = {}, {}, {}, {}
  else
    dropVehicle(vehID)
  end
end

function M.debugVehScript()
  return string.format(VEH_SCRIPT, 0)
end

function M.onExtensionLoaded()
  setExtensionUnloadMode(M, "manual")
  cgLog('I', "Cannon geometry extension loaded.")
end

function M.onWorldReadyState(state)
  if state == 2 then M.invalidate(nil) end
end

function M.onVehicleSwitched(oldId, newId, player)
  if player == nil or player == 0 then dropVehicle(oldId) end
end

function M.onVehicleResetted(vehID) dropVehicle(vehID) end
function M.onVehicleDestroyed(vehID) dropVehicle(vehID) end

function M.onUpdate(dtReal, dtSim, dtRaw)
  for vehID, p in pairs(pending) do
    p.timer = p.timer + dtReal
    if p.timer >= RESOLVE_TIMEOUT_S then
      if p.tries >= MAX_TRIES then
        failed[vehID], pending[vehID] = true, nil
      else
        local veh = scenetree.findObjectById(vehID)
        if veh then
          epochCounter = epochCounter + 1
          p.epoch, p.timer, p.tries = epochCounter, 0, p.tries + 1
          pcall(function() veh:queueLuaCommand(string.format(VEH_SCRIPT, epochCounter)) end)
        else
          pending[vehID] = nil
        end
      end
    end
  end
end

return M
