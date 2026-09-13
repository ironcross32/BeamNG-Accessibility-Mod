-- build.py prepends the surface list, terrain dimensions and wrap constants.
local M = {}
local enabled = true
local wraps = 0
local lastWrap = nil
local fault = nil
local GROUP_DISTANCE = 100
local EDGE_MARGIN = 200

local function surfaceAt(x, y)
  return SURFACES[(math.floor((x + BLOCK / 2) / BLOCK)
    + math.floor((y + BLOCK / 2) / BLOCK)) % #SURFACES + 1]
end

local function axisShift(value)
  if math.abs(value) <= WRAP_LIMIT then return 0 end
  return -math.floor((value + PERIOD / 2) / PERIOD) * PERIOD
end

local function snapshot(veh)
  local p, v = veh:getPosition(), veh:getVelocity()
  return {veh = veh, id = veh:getID(), x = p.x, y = p.y, z = p.z,
    vx = v.x, vy = v.y, vz = v.z}
end

local function vehicleGroups()
  local records, byId, parent = {}, {}, {}
  for i = 0, be:getObjectCount() - 1 do
    local veh = be:getObject(i)
    if veh then
      local record = snapshot(veh)
      records[#records + 1] = record
      byId[record.id] = record
      parent[record.id] = record.id
    end
  end
  table.sort(records, function(a, b) return a.id < b.id end)
  local function root(id)
    while parent[id] ~= id do
      parent[id] = parent[parent[id]]
      id = parent[id]
    end
    return id
  end
  local function join(a, b)
    if parent[a] and parent[b] then parent[root(b)] = root(a) end
  end
  -- Read the engine's live graph, including multi-trailer chains and couplers
  -- created before this extension was loaded. Event-only tracking misses those.
  for _, c in ipairs(core_vehicles.attachedCouplers) do join(c[1], c[2]) end
  -- Loose vehicle-based cargo is not coupled. Keep nearby objects together as
  -- well, so a load resting on a trailer does not stay behind at the wrap.
  for i = 1, #records do
    local a = records[i]
    for j = i + 1, #records do
      local b = records[j]
      if (a.x-b.x)^2 + (a.y-b.y)^2 + (a.z-b.z)^2 <= GROUP_DISTANCE^2 then
        join(a.id, b.id)
      end
    end
  end
  local groups, ordered = {}, {}
  for _, record in ipairs(records) do
    local id = root(record.id)
    if not groups[id] then
      groups[id] = {}
      ordered[#ordered + 1] = groups[id]
    end
    table.insert(groups[id], record)
  end
  return ordered
end

local function moveGroup(group, dx, dy)
  -- Snapshot all targets before moving any member: a coupled object must never
  -- calculate its destination from a position already affected by this wrap.
  for _, r in ipairs(group) do
    local x, y = r.x + dx, r.y + dy
    if x < TERRAIN_MIN + EDGE_MARGIN or x > TERRAIN_MAX - EDGE_MARGIN
      or y < TERRAIN_MIN + EDGE_MARGIN or y > TERRAIN_MAX - EDGE_MARGIN then
      error("Vehicle group exceeds the buffered terrain; wrapping stopped")
    end
    if not r.veh.setClusterPosRelRot then error("Cluster translation API unavailable") end
  end
  local report = {dx = dx, dy = dy, vehicles = {},
    couplersBefore = #core_vehicles.attachedCouplers,
    maxPositionError = 0, maxVelocityChange = 0}
  for _, r in ipairs(group) do
    -- -1 addresses every cluster, including detached parts of this vehicle.
    -- Identity rotation and no applyClusterVelocityScaleAdd/reset/respawn calls:
    -- retain deformation and the existing per-node velocities. This engine
    -- behavior, particularly wheelspin and couplers, requires live verification.
    r.veh:setClusterPosRelRot(-1, r.x + dx, r.y + dy, r.z, 0, 0, 0, 1)
  end
  for _, r in ipairs(group) do
    local p, v = r.veh:getPosition(), r.veh:getVelocity()
    local positionError = math.sqrt((p.x-r.x-dx)^2 + (p.y-r.y-dy)^2 + (p.z-r.z)^2)
    local velocityChange = math.sqrt((v.x-r.vx)^2 + (v.y-r.vy)^2 + (v.z-r.vz)^2)
    report.maxPositionError = math.max(report.maxPositionError, positionError)
    report.maxVelocityChange = math.max(report.maxVelocityChange, velocityChange)
    report.vehicles[#report.vehicles + 1] = {id = r.id,
      before = {r.x, r.y, r.z}, after = {p.x, p.y, p.z},
      velocityBefore = {r.vx, r.vy, r.vz}, velocityAfter = {v.x, v.y, v.z},
      surfaceBefore = surfaceAt(r.x, r.y), surfaceAfter = surfaceAt(p.x, p.y)}
  end
  report.couplersAfter = #core_vehicles.attachedCouplers
  wraps = wraps + 1
  report.number = wraps
  lastWrap = report
  log("I", "endlessSurfaces", "Wrap " .. wraps .. ": " .. #group
    .. " vehicles, shift " .. dx .. "," .. dy
    .. ", position error " .. report.maxPositionError
    .. ", velocity change " .. report.maxVelocityChange)
  return report
end

local function step()
  if not core_vehicles or type(core_vehicles.attachedCouplers) ~= "table" then
    error("Engine coupler graph unavailable; refusing to move a partial assembly")
  end
  local playerId = be:getPlayerVehicleID(0)
  for _, group in ipairs(vehicleGroups()) do
    local anchor = group[1]
    for _, r in ipairs(group) do
      if r.id == playerId then anchor = r; break end
    end
    local dx, dy = axisShift(anchor.x), axisShift(anchor.y)
    if dx ~= 0 or dy ~= 0 then moveGroup(group, dx, dy) end
  end
end

function M.onUpdate(dtReal, dtSim)
  if not enabled or not dtSim or dtSim <= 0 then return end
  local ok, err = pcall(step)
  if not ok then
    enabled = false
    fault = tostring(err)
    -- Stop physics if a translation fails midway, before a stretched coupler
    -- can pull the assembly apart. The diagnostic remains available in status().
    be:setPhysicsRunning(false)
    log("E", "endlessSurfaces", fault)
    if ui_message then ui_message("Endless Surfaces paused: " .. fault, 15, "endlessSurfaces") end
  end
end

function M.status()
  local veh = be:getPlayerVehicle(0)
  local p = veh and veh:getPosition()
  return {enabled = enabled, wraps = wraps, fault = fault, lastWrap = lastWrap,
    period = PERIOD, blockSize = BLOCK, wrapLimit = WRAP_LIMIT,
    playerSurface = p and surfaceAt(p.x, p.y) or nil,
    playerPosition = p and {p.x, p.y, p.z} or nil}
end

function M.setEnabled(value)
  enabled = value == true
  if enabled then fault = nil end
end

function M.onExtensionLoaded()
  log("I", "endlessSurfaces", "Prototype loaded: " .. BLOCK
    .. " m surfaces; " .. PERIOD .. " m group wrapping")
end

-- Exercise the exact production path while paused, without resetting or adding
-- velocity. The live verifier samples wheels around this call in the vehicle VM.
function M.probeWrap(dx, dy)
  if dx % PERIOD ~= 0 or dy % PERIOD ~= 0 then error("Shift must be a whole period") end
  local playerId = be:getPlayerVehicleID(0)
  for _, group in ipairs(vehicleGroups()) do
    for _, r in ipairs(group) do
      if r.id == playerId then return moveGroup(group, dx, dy) end
    end
  end
  error("No player vehicle")
end

M.surfaceAt = surfaceAt
return M
