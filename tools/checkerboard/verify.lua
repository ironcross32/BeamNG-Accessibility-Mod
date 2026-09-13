-- Run in the GE Lua console:
-- extensions.loadAtRoot('/levels/endless_surfaces/verify', 'endlessSurfaces')
-- endlessSurfaces_verify.inspect()
-- Optional MOVING test: endlessSurfaces_verify.beginWrapTest()
local M = {}
local pending = nil
local result = nil
local token = 0
local MATERIALS = {ASPHALT = 10, DIRT = 15, GRAVEL = 19, SAND = 16, ICE = 21, GRASS = 20}

function M.inspect()
  local terrain = scenetree.findObject("theTerrain")
  local data = {status = mainLevel.status(),
    terrain = terrain and terrain:getField("terrainFile", 0),
    couplers = core_vehicles.attachedCouplers,
    expectedContactIds = MATERIALS}
  log("I", "endlessSurfaces.verify", dumps(data))
  return data
end

local function sampleWheels(id, phase)
  local veh = be:getObjectByID(id)
  if not veh then error("Test vehicle disappeared") end
  -- Vehicle physics lives in a separate VM. Wait for every before-response
  -- before translating, then request after-responses with physics still paused.
  veh:queueLuaCommand(string.format([[
    local data = {wheels = {}}
    for _, w in pairs(wheels.wheels) do
      local p = obj:getPosition() + obj:getNodePosition(w.node1)
      data.wheels[#data.wheels+1] = {name = w.name,
        angularVelocity = w.coreData and w.coreData.angularVelocity,
        contact = w.contactMaterialID1,
        x = p.x, y = p.y, z = p.z}
    end
    obj:queueGameEngineLua('endlessSurfaces_verify.receive(%d,%d,%q,' .. serialize(data) .. ')')
  ]], token, id, phase))
end

function M.beginWrapTest(dx, dy)
  if pending then error("Test already running") end
  if not be:getPlayerVehicle(0) then error("No player vehicle") end
  local period = mainLevel.status().period
  dx, dy = dx or period, dy or 0
  if dx % period ~= 0 or dy % period ~= 0 then error("Use whole " .. period .. " m periods") end
  token = token + 1
  pending = {dx = dx, dy = dy, before = {}, after = {}, ids = {},
    elapsed = 0, stage = "before", wasRunning = be:getPhysicsRunning(),
    wasEnabled = mainLevel.status().enabled}
  result = nil
  mainLevel.setEnabled(false)
  be:setPhysicsRunning(false)
  for i = 0, be:getObjectCount() - 1 do
    local veh = be:getObject(i)
    pending.ids[#pending.ids+1] = veh:getID()
  end
  for _, id in ipairs(pending.ids) do sampleWheels(id, "before") end
  return "Sampling paused physics; call endlessSurfaces_verify.result() after a few frames"
end

function M.receive(testToken, id, phase, data)
  if pending and testToken == token and phase == pending.stage then
    pending[phase][id] = data
  end
end

function M.checkContacts()
  local veh = be:getPlayerVehicle(0)
  if not veh then error("No player vehicle") end
  veh:queueLuaCommand([[
    local data = {}
    for _, w in pairs(wheels.wheels) do
      local p = obj:getPosition() + obj:getNodePosition(w.node1)
      data[#data+1] = {name = w.name, x = p.x, y = p.y, contact = w.contactMaterialID1}
    end
    obj:queueGameEngineLua('endlessSurfaces_verify.receiveContacts(' .. serialize(data) .. ')')
  ]])
  return "Wheel contact results will be logged; query result() after a few frames"
end

function M.receiveContacts(data)
  for _, w in ipairs(data) do
    w.expectedSurface = mainLevel.surfaceAt(w.x, w.y)
    w.expectedId = MATERIALS[w.expectedSurface]
    w.matches = w.contact == w.expectedId
  end
  result = {contacts = data}
  log("I", "endlessSurfaces.verify", dumps(result))
end

local function finish(err)
  result = pending
  result.error = err
  pending = nil
  -- A failed move can leave an assembly split: stay paused on errors.
  mainLevel.setEnabled(not err and result.wasEnabled)
  if not err then be:setPhysicsRunning(result.wasRunning) end
  log(err and "E" or "I", "endlessSurfaces.verify", dumps(result))
end

local function advance(dtReal)
  pending.elapsed = pending.elapsed + dtReal
  if pending.elapsed > 10 then error("Timed out waiting for vehicle VM samples; left paused") end
  for _, id in ipairs(pending.ids) do
    if not pending[pending.stage][id] then return end
  end
  if pending.stage == "before" then
    pending.translation = mainLevel.probeWrap(pending.dx, pending.dy)
    pending.stage = "after"
    pending.elapsed = 0
    for _, id in ipairs(pending.ids) do sampleWheels(id, "after") end
  else
    -- Preserve raw samples for review, not just a pass/fail derived from speed.
    local report = pending.translation
    if report.maxPositionError > 0.02 then error("Position changed by more than 2 cm") end
    if report.maxVelocityChange > 0.01 then error("Velocity changed during paused wrap") end
    if report.couplersBefore ~= report.couplersAfter then error("Coupler count changed") end
    for _, moved in ipairs(report.vehicles) do
      local before, after = pending.before[moved.id], pending.after[moved.id]
      local byName = {}
      for _, w in ipairs(after.wheels) do byName[w.name] = w end
      for _, w in ipairs(before.wheels) do
        local a = byName[w.name]
        if not a then error("Wheel disappeared during wrap") end
        if not a.angularVelocity or not w.angularVelocity then error("Missing wheel speed data") end
        if math.abs(a.angularVelocity - w.angularVelocity) > 0.01 then
          error("Wheel speed changed for " .. moved.id .. "/" .. w.name)
        end
      end
    end
    finish(nil)
  end
end

function M.onUpdate(dtReal)
  if not pending then return end
  local ok, err = pcall(advance, dtReal)
  if not ok then finish(tostring(err)) end
end

function M.result() return result or pending end

function M.onExtensionUnloaded()
  if pending then finish("Verifier unloaded during test; left paused") end
end

return M
