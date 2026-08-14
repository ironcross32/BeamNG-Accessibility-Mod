-- =================================================================================================
--
--  Accessible Vehicle Spawner for BeamNG.drive Accessibility Mod (GE Extension)
--
--  Description: Builds a catalog of every available vehicle + configuration (shipped + mods +
--               props), exposes it to the Python/UI layer via UDP, and spawns vehicles at
--               positions relative to either the player vehicle, a "marked" vehicle, or the
--               camera. Intercepts F11 via a TorqueScript ActionMap so the world editor cannot
--               open while the modal is active.
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
local PYTHON_PORT_DATA  = 4460   -- send spawner data to Python on this port
local CMD_LISTEN_PORT   = 4461   -- receive commands from Python on this port

-- Internal State
local udpSend           = nil
local udpCmd            = nil
local catalog           = nil          -- array of vehicle entries
local catalogReady      = false

-- Logging helper
local function vsLog(level, msg)
  log(level, 'VehSpawnerAccess', msg)
end

-- =================================================================================================
--  Catalog Build
-- =================================================================================================

local function safeStr(v, default)
  if v == nil then return default or "" end
  if type(v) == "table" then return default or "" end
  return tostring(v)
end

local function safeNum(v)
  if type(v) == "number" then return v end
  if type(v) == "string" then return tonumber(v) end
  return nil
end

local function extractYears(info)
  if not info then return nil, nil end
  local y = info.Years
  if type(y) == "table" then
    return safeNum(y.min), safeNum(y.max)
  end
  return nil, nil
end

local function buildCatalog()
  vsLog('info', "Building vehicle catalog...")
  catalog = {}
  catalogReady = false

  local list = nil
  local ok, err = pcall(function()
    list = core_vehicles.getVehicleList()
  end)
  if not ok or not list or not list.vehicles then
    vsLog('error', "core_vehicles.getVehicleList() failed: " .. tostring(err))
    return
  end

  -- Sort vehicle entries by brand+name for stable presentation
  local entries = {}
  for _, v in pairs(list.vehicles) do
    entries[#entries + 1] = v
  end
  table.sort(entries, function(a, b)
    local an = (a.model and a.model.Name) or a.modelName or ""
    local bn = (b.model and b.model.Name) or b.modelName or ""
    return an:lower() < bn:lower()
  end)

  for _, entry in ipairs(entries) do
    local modelName = entry.modelName or (entry.model and entry.model.key)
    local info = entry.model or {}
    if modelName and modelName ~= "" then
      local yMin, yMax = extractYears(info)

      local vehEntry = {
        model       = modelName,
        name        = safeStr(info.Name, modelName),
        brand       = safeStr(info.Brand),
        type        = safeStr(info.Type, "Car"),
        bodyStyle   = safeStr(info.BodyStyle),
        drivetrain  = safeStr(info.Drivetrain),
        propulsion  = safeStr(info.Propulsion),
        country     = safeStr(info.Country),
        yearMin     = yMin,
        yearMax     = yMax,
        configs     = {},
      }

      -- Configurations: entry.configs is a dict keyed by config name
      local hasPolice = false
      if entry.configs then
        local cfgList = {}
        for cfgKey, cfg in pairs(entry.configs) do
          local cfgName = (cfg and (cfg.Name or cfg.name)) or cfgKey
          local isDefault = false
          if info.default_pc and info.default_pc == cfgKey then
            isDefault = true
          end
          -- "Config Type" comes straight from info_<config>.json (read by
          -- core_vehicles into configData). Stock values: Factory, Custom,
          -- Police, Race, Rally, Drift, Service, Powerglow. Used by the
          -- in-game radial vehicle filter, so it's the canonical signal.
          local cfgType = safeStr(cfg and cfg["Config Type"])
          if cfgType == "Police" then
            hasPolice = true
          end
          cfgList[#cfgList + 1] = {
            key        = cfgKey,
            name       = safeStr(cfgName, cfgKey),
            isDefault  = isDefault,
            configType = cfgType,
          }
        end
        table.sort(cfgList, function(a, b)
          if a.isDefault ~= b.isDefault then return a.isDefault end
          return a.name:lower() < b.name:lower()
        end)
        vehEntry.configs = cfgList
      end
      vehEntry.hasPolice = hasPolice

      -- If a vehicle has zero configs, synthesize a "default" entry
      if #vehEntry.configs == 0 then
        vehEntry.configs = { { key = "", name = "(default)", isDefault = true } }
      end

      catalog[#catalog + 1] = vehEntry
    end
  end

  catalogReady = true
  local policeCount = 0
  for _, v in ipairs(catalog) do
    if v.hasPolice then policeCount = policeCount + 1 end
  end
  vsLog('info', string.format("Catalog ready: %d vehicles (%d flagged as having police configs)", #catalog, policeCount))
end

-- =================================================================================================
--  Catalog Transmission (chunked JSON)
-- =================================================================================================

local function sendCatalog()
  if not udpSend then return end
  if not catalogReady or not catalog then
    udpSend:send("CATALOG_ERR:not ready")
    return
  end

  udpSend:send(string.format("CATALOG_BEGIN:%d", #catalog))

  -- Send each vehicle as its own packet (JSON). Configs are inlined.
  for i, v in ipairs(catalog) do
    local payload = {
      i          = i - 1,        -- 0-indexed for the UI
      model      = v.model,
      name       = v.name,
      brand      = v.brand,
      type       = v.type,
      bodyStyle  = v.bodyStyle,
      drivetrain = v.drivetrain,
      propulsion = v.propulsion,
      country    = v.country,
      yearMin    = v.yearMin,
      yearMax    = v.yearMax,
      hasPolice  = v.hasPolice or false,
      configs    = v.configs,
    }
    local ok, encoded = pcall(jsonEncode, payload)
    if ok and encoded then
      udpSend:send("CATALOG_ITEM:" .. encoded)
    else
      vsLog('warn', "Failed to encode catalog entry: " .. tostring(v.model))
    end
  end

  udpSend:send("CATALOG_END")
  vsLog('info', string.format("Catalog sent (%d entries).", #catalog))
end

-- =================================================================================================
--  Spawn Reference Frames
-- =================================================================================================

-- Height of the ground under `pos`, or nil when there is none to find.
--
-- Deliberately not core_terrain.getTerrainHeight: that only knows about a TerrainBlock,
-- and several stock maps have none at all (smallgrid is a GroundPlane, gridmap is static
-- meshes), so terrain queries return nil for every point on them.
-- be:getSurfaceHeightBelow sees terrain, static meshes and ground planes alike. It reports
-- failure as -1e20 rather than nil and only looks downwards, hence the retries -- the same
-- pattern the game's own common/tech/techUtils.lua uses. Vehicles are soft bodies and
-- invisible to it, which is what we want: the anchor should be the ground, not the roof of
-- a car parked below. (cameraInfo.lua carries its own copy; these extensions are
-- standalone by design.)
local function groundHeightBelow(pos)
  local function probe(z)
    local ok, h = pcall(function() return be:getSurfaceHeightBelow(vec3(pos.x, pos.y, z)) end)
    if ok and type(h) == "number" and h > -1e10 then return h end
    return nil
  end
  return probe(pos.z) or probe(pos.z + 2) or probe(1e5)
end

local function frameFromVehicle(veh)
  if not veh then return nil end
  local pos = vec3(veh:getPosition())
  local fwd = vec3(veh:getDirectionVector()):normalized()
  local up  = vec3(veh:getDirectionVectorUp()):normalized()
  local right = fwd:cross(up):normalized()
  return { pos = pos, fwd = fwd, up = up, right = right }
end

local function frameFromCameraGroundSnap()
  local cp = core_camera.getPosition()
  local cf = core_camera.getForward()
  if not cp or not cf then return nil end

  local fwd = vec3(cf.x, cf.y, 0)
  if fwd:length() < 0.01 then
    fwd = vec3(1, 0, 0)
  else
    fwd = fwd:normalized()
  end
  local up = vec3(0, 0, 1)
  local right = fwd:cross(up):normalized()

  -- Ground height under the camera. See groundHeightBelow: core_terrain.getTerrainHeight
  -- used to be the query here, and it returns nil for every point on maps with no terrain
  -- block (smallgrid, gridmap), which left this falling back to a hardcoded Z=1.0 -- the
  -- anchor landed a metre off the ground, or nowhere near it.
  local groundZ = groundHeightBelow(cp)
  if not groundZ then
    -- Genuinely nothing below: over open water, or off the edge of the world. The camera's
    -- own height is at least a spot the user can reason about (the vehicle drops from where
    -- they are looking), unlike an arbitrary constant.
    vsLog('warn', "No ground found below the camera; anchoring at camera height instead.")
  end
  local z = groundZ or cp.z
  return { pos = vec3(cp.x, cp.y, z), fwd = fwd, up = up, right = right }
end

local function resolveReferenceFrame(refMode, refVehId, prevFrame)
  -- "auto" -> chain off prevFrame, else player, else camera+ground
  -- "vehicle" -> refVehId or player, fallback camera+ground
  -- "camera" or anything else -> camera+ground
  if refMode == "auto" then
    if prevFrame then return prevFrame end
    local p = be:getPlayerVehicle(0)
    if p then return frameFromVehicle(p) end
    return frameFromCameraGroundSnap()
  end

  if refMode == "vehicle" then
    local target = nil
    if refVehId and refVehId >= 0 then
      target = be:getObjectByID(refVehId)
    end
    if not target then
      target = be:getPlayerVehicle(0)
    end
    if target then
      return frameFromVehicle(target)
    end
    return frameFromCameraGroundSnap()
  end

  return frameFromCameraGroundSnap()
end

-- Rodrigues' rotation formula. Pure vec3 math so it does not depend on a quat
-- axis-angle helper being present in retail LuaJIT.
local function rotateAboutAxis(v, axis, angleRad)
  local c, s = math.cos(angleRad), math.sin(angleRad)
  return v * c + axis:cross(v) * s + axis * (axis:dot(v) * (1 - c))
end

local function rotateBasis(fwd, up, right, axis, angleRad)
  if angleRad == 0 then return fwd, up, right end
  return rotateAboutAxis(fwd, axis, angleRad):normalized(),
         rotateAboutAxis(up, axis, angleRad):normalized(),
         rotateAboutAxis(right, axis, angleRad):normalized()
end

-- Place a vehicle relative to `frame`. `off` is {fwd, right, up} in metres and
-- `rotDeg` is {yaw, pitch, roll} in degrees, all in the reference frame's own axes.
-- Sign convention (right-handed, matching the Python editor): +yaw swings the nose
-- left, +pitch lifts the nose, +roll drops the right side.
local function buildSpawnTransform(frame, off, rotDeg)
  if not frame then return nil, nil, nil end
  off = off or {}
  rotDeg = rotDeg or {}

  local spawnPos = frame.pos
    + frame.fwd   * (off.fwd   or 0)
    + frame.right * (off.right or 0)
    + frame.up    * (off.up    or 0)

  -- Intrinsic yaw -> pitch -> roll: each rotation is about the axis as it stands
  -- after the previous one, which is what "turn it, then tip it" means to the user.
  local fwd, up, right = frame.fwd, frame.up, frame.right
  fwd, up, right = rotateBasis(fwd, up, right, up,    math.rad(rotDeg.yaw   or 0))
  fwd, up, right = rotateBasis(fwd, up, right, right, math.rad(rotDeg.pitch or 0))
  fwd, up, right = rotateBasis(fwd, up, right, fwd,   math.rad(rotDeg.roll  or 0))

  local rot = quatFromDir(fwd, up)
  -- Return spawn pos/rot and the new reference frame for chaining. The rotated
  -- basis is used so "previous in queue" follows the vehicle as it actually sits.
  local newFrame = { pos = spawnPos, fwd = fwd, up = up, right = right }
  return spawnPos, rot, newFrame
end

-- =================================================================================================
--  Arrangement geometry
-- =================================================================================================

local function computeArrangementPositions(anchorFrame, arrType, variant, spacingM, nOthers)
  local s = spacingM
  local N = nOthers
  local offsets = {}  -- array of {fwdM, rightM}

  if arrType == "line" then
    if variant == "start" then
      for k = 1, N do offsets[#offsets+1] = {fwdM = -k*s, rightM = 0} end
    elseif variant == "end" then
      for k = 1, N do offsets[#offsets+1] = {fwdM = k*s, rightM = 0} end
    else  -- "middle"
      local nF = math.floor(N / 2); local nB = N - nF
      for k = 1, nF do offsets[#offsets+1] = {fwdM = k*s,  rightM = 0} end
      for k = 1, nB do offsets[#offsets+1] = {fwdM = -k*s, rightM = 0} end
    end

  elseif arrType == "side_by_side" then
    if variant == "left" then
      for k = 1, N do offsets[#offsets+1] = {fwdM = 0, rightM = k*s} end
    elseif variant == "right" then
      for k = 1, N do offsets[#offsets+1] = {fwdM = 0, rightM = -k*s} end
    else  -- "middle"
      local nR = math.floor(N / 2); local nL = N - nR
      for k = 1, nR do offsets[#offsets+1] = {fwdM = 0, rightM = k*s}  end
      for k = 1, nL do offsets[#offsets+1] = {fwdM = 0, rightM = -k*s} end
    end

  elseif arrType == "two_columns" then
    local nTotal = N + 1; local nRows = math.ceil(nTotal / 2)
    local aRow
    if     variant == "front"  then aRow = 0
    elseif variant == "back"   then aRow = nRows - 1
    else                            aRow = math.floor(nRows / 2) end
    for row = 0, nRows - 1 do
      for col = 0, 1 do
        if row ~= aRow or col ~= 0 then  -- col 0 is anchor column
          offsets[#offsets+1] = {fwdM = (aRow - row)*s, rightM = (col*2 - 1)*(s/2)}
          if #offsets >= N then break end
        end
      end
      if #offsets >= N then break end
    end

  elseif arrType == "three_columns" then
    local nTotal = N + 1; local nRows = math.ceil(nTotal / 3)
    local aRow
    if     variant == "front"  then aRow = 0
    elseif variant == "back"   then aRow = nRows - 1
    else                            aRow = math.floor(nRows / 2) end
    for row = 0, nRows - 1 do
      for col = 0, 2 do
        if row ~= aRow or col ~= 1 then  -- col 1 is anchor column
          offsets[#offsets+1] = {fwdM = (aRow - row)*s, rightM = (col - 1)*s}
          if #offsets >= N then break end
        end
      end
      if #offsets >= N then break end
    end

  elseif arrType == "boxed_in" then
    local ring = 1
    while #offsets < N do
      local r = ring * s
      local ringSlots = {
        {fwdM = r,  rightM = 0},  {fwdM = -r, rightM = 0},
        {fwdM = 0,  rightM = -r}, {fwdM = 0,  rightM = r},
        {fwdM = r,  rightM = r},  {fwdM = r,  rightM = -r},
        {fwdM = -r, rightM = r},  {fwdM = -r, rightM = -r},
      }
      for _, sl in ipairs(ringSlots) do
        offsets[#offsets+1] = sl
        if #offsets >= N then break end
      end
      ring = ring + 1
      if ring > 20 then break end
    end
  end

  local rot = quatFromDir(anchorFrame.fwd, anchorFrame.up)
  local results = {}
  for _, off in ipairs(offsets) do
    results[#results+1] = {
      pos = anchorFrame.pos + anchorFrame.fwd * off.fwdM + anchorFrame.right * off.rightM,
      rot = rot,
    }
  end
  return results
end

local function handleSpawnBatchArranged(items, arrangement)
  local anchorFrame = nil
  local anchorVehId = arrangement.anchorVehId
  if anchorVehId and type(anchorVehId) == "number" and anchorVehId >= 0 then
    local av = be:getObjectByID(math.floor(anchorVehId))
    if av then anchorFrame = frameFromVehicle(av) end
  end
  if not anchorFrame then
    local p = be:getPlayerVehicle(0)
    if p then anchorFrame = frameFromVehicle(p) end
  end
  if not anchorFrame then anchorFrame = frameFromCameraGroundSnap() end
  if not anchorFrame then
    if udpSend then udpSend:send("ARRANGE_ERR:no anchor frame") end
    return
  end

  local rot = quatFromDir(anchorFrame.fwd, anchorFrame.up)
  local nOthers = math.max(0, #items - 1)
  local positions = computeArrangementPositions(
    anchorFrame,
    arrangement.type or "line",
    arrangement.variant or "middle",
    arrangement.spacingM or 7.62,
    nOthers
  )

  local successes, failures = 0, {}

  for i, item in ipairs(items) do
    local pos, irot
    if i == 1 then
      pos  = anchorFrame.pos
      irot = rot
    else
      local pd = positions[i - 1]
      if not pd then
        failures[#failures+1] = tostring(item.model or "?") .. ": no position"
        goto continueArrangeSpawn
      end
      pos  = pd.pos
      irot = pd.rot
    end

    local replaceId = item.replaceVehId
    if replaceId and type(replaceId) == "number" and replaceId >= 0 then
      -- First parameter is the MODEL NAME, not a vehicle id -- the vehicle being
      -- replaced goes in the third slot. Passing the id made prepareConfigData
      -- fail, so replaceVehicle returned nil and this reported success for a
      -- replacement that never occurred.
      local oldVeh = be:getObjectByID(math.floor(replaceId))
      if not oldVeh then
        failures[#failures+1] = tostring(item.model or "?") .. ": replacement target not found"
      else
        local model = item.model or oldVeh.JBeam
        local ok, newVeh = pcall(core_vehicles.replaceVehicle, model,
          {config = item.config or ""}, oldVeh)
        if not ok then
          failures[#failures+1] = tostring(model or "?") .. ": " .. tostring(newVeh)
        elseif not newVeh then
          -- Declined rather than raised; see the note above.
          failures[#failures+1] = tostring(model or "?") .. ": replaceVehicle declined"
        else
          successes = successes + 1
        end
      end
    else
      local opts = {pos = pos, rot = irot, cling = true, autoEnterVehicle = false}
      if item.config and item.config ~= "" then opts.config = item.config end
      local ok, newVeh = pcall(core_vehicles.spawnNewVehicle, item.model, opts)
      if not ok then
        failures[#failures+1] = tostring(item.model or "?") .. ": " .. tostring(newVeh)
      elseif not newVeh then
        failures[#failures+1] = tostring(item.model or "?") .. ": spawn declined"
      else
        successes = successes + 1
      end
    end

    ::continueArrangeSpawn::
  end

  if udpSend then
    if #failures == 0 then
      udpSend:send(string.format("ARRANGE_OK:%d", successes))
    else
      local msg = string.format("ARRANGE_ERR:%d failed: %s", #failures, table.concat(failures, " | "))
      udpSend:send(msg:sub(1, 1200))
    end
  end
end

local function handleTeleportArrange(jsonStr)
  local ok, payload = pcall(jsonDecode, jsonStr)
  if not ok or not payload or not payload.arrangement or not payload.vehicleIds then
    if udpSend then udpSend:send("ARRANGE_ERR:bad teleport payload") end
    return
  end

  local vehIds     = payload.vehicleIds
  local arrangement = payload.arrangement

  local anchorVeh = nil
  local anchorVehId = arrangement.anchorVehId
  if anchorVehId and type(anchorVehId) == "number" and anchorVehId >= 0 then
    anchorVeh = be:getObjectByID(math.floor(anchorVehId))
  end
  if not anchorVeh then anchorVeh = be:getPlayerVehicle(0) end
  if not anchorVeh and #vehIds >= 1 then
    anchorVeh = be:getObjectByID(math.floor(vehIds[1]))
  end
  if not anchorVeh then
    if udpSend then udpSend:send("ARRANGE_ERR:no anchor vehicle") end
    return
  end

  local anchorId    = anchorVeh:getID()
  local anchorFrame = frameFromVehicle(anchorVeh)
  if not anchorFrame then
    if udpSend then udpSend:send("ARRANGE_ERR:anchor frame failed") end
    return
  end

  local otherIds = {}
  for _, id in ipairs(vehIds) do
    if id ~= anchorId then otherIds[#otherIds+1] = id end
  end

  local positions = computeArrangementPositions(
    anchorFrame,
    arrangement.type or "line",
    arrangement.variant or "middle",
    arrangement.spacingM or 7.62,
    #otherIds
  )

  local successes = 1  -- anchor stays in place
  local failures  = {}

  for i, id in ipairs(otherIds) do
    local veh = be:getObjectByID(math.floor(id))
    local pd  = positions[i]
    if not veh then
      failures[#failures+1] = tostring(id) .. ": not found"
    elseif not pd then
      failures[#failures+1] = tostring(id) .. ": no position"
    else
      -- No 5th argument: that slot is visibilityPoint and must be a vec3, so the
      -- boolean that used to be here threw inside spawn.lua. pcall swallowed it,
      -- so every arrange silently failed to move anything.
      local r, err = pcall(spawn.safeTeleport, veh, pd.pos, pd.rot)
      if r then successes = successes + 1
      else failures[#failures+1] = tostring(id) .. ": " .. tostring(err) end
    end
  end

  if udpSend then
    if #failures == 0 then
      udpSend:send(string.format("ARRANGE_OK:%d", successes))
    else
      local msg = string.format("ARRANGE_ERR:%d failed: %s", #failures, table.concat(failures, " | "))
      udpSend:send(msg:sub(1, 1200))
    end
  end
end

-- =================================================================================================
--  Teleport to an edited placement (manage page "W")
-- =================================================================================================

-- Move an already-spawned vehicle to pos/rot, carrying its damage along. Anything that
-- resets the vehicle -- safeTeleport's default resetVehicle, setPosRot, setPositionRotation
-- -- respawns it from its initial node positions, which repairs every dent. Both paths
-- below go through the cluster API instead, which moves the deformed body as it stands.
--
-- allowSettle picks between safeTeleport, which finds a non-intersecting spot and sets the
-- vehicle down on the ground, and a direct cluster move. The ground search would throw
-- away a height offset, so a raised placement takes the direct path.
local function teleportVehicleTo(veh, pos, rot, allowSettle)
  if allowSettle then
    -- 7th arg is centeredPosition, 8th is resetVehicle -- false is what keeps the damage.
    -- safeTeleport applies the 180-degree flip and zeroes the velocity itself.
    return pcall(spawn.safeTeleport, veh, pos, rot, nil, nil, nil, false, false)
  end
  return pcall(function()
    local refNode = veh:getRefNodeId()
    -- Vehicle rotations carry the same 180-degree flip spawn.lua applies on the spawn
    -- path ("vehicles' forward is inverted"), so rot has to be flipped before it can be
    -- compared against the vehicle's current rotation.
    local target  = quat(0, 0, 1, 0) * rot
    local current = quat(veh:getClusterRotationSlow(refNode))
    -- setClusterPosRelRot takes an absolute refnode position but a *relative* rotation.
    local diffRot = current:inversed() * target
    veh:setClusterPosRelRot(refNode, pos.x, pos.y, pos.z, diffRot.x, diffRot.y, diffRot.z, diffRot.w)
    -- Land it stationary rather than carrying the speed it had before the move.
    veh:applyClusterVelocityScaleAdd(refNode, 0, 0, 0, 0)
    veh:setOriginalTransform(pos.x, pos.y, pos.z, target.x, target.y, target.z, target.w)
  end)
end

-- Fastest launch we will ever ask for, in m/s (~560 mph). Past this the vehicle is
-- leaving the map rather than being thrown across it, so a target that needs more
-- gets thrown at the cap and lands short -- announced, never silently retargeted.
local MAX_LAUNCH_MPS = 250

-- Set while a launched vehicle is in the air, so onUpdate can watch it down and report
-- where it really ended up. A ballistic solve is a point-mass approximation of a
-- soft-body car: air drag, tumbling and the bounce on landing all pull the real landing
-- point in, and none of them can be solved for up front. Measuring the miss is both the
-- honest thing to announce and the only way to tell a physics shortfall from an aiming
-- bug -- the numbers go to the game log alongside the solve that produced them.
local launchTrack = nil
local LAUNCH_TRACK_TIMEOUT = 30   -- seconds; give up rather than track forever
local LAUNCH_SETTLE_SPEED  = 2.0  -- m/s below which we call it landed
local LAUNCH_SETTLE_TIME   = 0.4  -- seconds it must stay that slow

-- Throw a vehicle at targetPos instead of placing it there, the way the game's own
-- "fling"/"boost" cheats do (core/funstuff.lua): one impulse via
-- applyClusterVelocityScaleAdd, then gravity does the rest. Returns ok, speed, flightTime,
-- wasCapped.
local function launchVehicleTo(veh, targetPos)
  local refNode = veh:getRefNodeId()
  local d    = targetPos - vec3(veh:getPosition())
  local flat = vec3(d.x, d.y, 0)
  local L    = flat:length()
  local dz   = d.z

  local g = -9.81
  if core_environment and core_environment.getGravity then
    local okG, gv = pcall(core_environment.getGravity)
    if okG and type(gv) == "number" then g = gv end
  end
  g = math.abs(g)

  local vel, zeroG
  if g < 0.05 then
    -- Zero-gravity level: nothing to arc over, so just cross the gap at a steady clip.
    zeroG = true
    vel = d / 2.0
  else
    -- Apex height above the start. Tying it to a quarter of the horizontal distance is
    -- what makes this a 45-degree launch on flat ground (tan(theta) = 4h/L = 1), which
    -- is high enough to clear fences and parked cars on the way. The 1.5 m floor keeps
    -- a short hop -- or a straight-up move, where L is 0 -- from degenerating into a
    -- flat shove. Targets above the vehicle raise the apex so the arc still peaks over
    -- the landing point rather than under it.
    local h  = math.max(0, dz) + math.max(1.5, L * 0.25)
    local vz = math.sqrt(2 * g * h)
    local T  = vz / g + math.sqrt(2 * math.max(0, h - dz) / g)
    vel = vec3(0, 0, vz)
    if L > 0 then vel = vel + flat:normalized() * (L / T) end
  end

  local speed  = vel:length()
  local capped = false
  if speed > MAX_LAUNCH_MPS then
    vel = vel * (MAX_LAUNCH_MPS / speed)
    speed = MAX_LAUNCH_MPS
    capped = true
  end

  -- Flight time is derived from the velocity we actually apply, not the one we solved
  -- for, so a capped launch reports the shorter hop it will really make. For an
  -- uncapped launch this returns the solved time exactly.
  local flightTime
  if zeroG then
    flightTime = (speed > 0) and (d:length() / speed) or 0
  else
    -- Descending crossing of the target height: dz = vz*t - g*t^2/2. When the arc never
    -- climbs that high, report the round trip back to the starting height instead.
    local disc = vel.z * vel.z - 2 * g * dz
    if disc >= 0 then
      flightTime = (vel.z + math.sqrt(disc)) / g
    else
      flightTime = 2 * vel.z / g
    end
  end

  -- Scale 0 rather than the 1 the fling/boost cheats use: it wipes whatever the vehicle
  -- was already doing, so a rolling car arcs from a known state instead of adding the
  -- launch to its current motion. Nothing here resets the vehicle, so damage survives.
  local ok, err = pcall(function()
    veh:applyClusterVelocityScaleAdd(refNode, 0, vel.x, vel.y, vel.z)
  end)

  if ok then
    local start = vec3(veh:getPosition())
    vsLog('info', string.format(
      "launch solve: from %.2f,%.2f,%.2f to %.2f,%.2f,%.2f | L=%.2f dz=%.2f g=%.2f " ..
      "vel=%.2f,%.2f,%.2f speed=%.2f flight=%.2fs capped=%s",
      start.x, start.y, start.z, targetPos.x, targetPos.y, targetPos.z,
      L, dz, g, vel.x, vel.y, vel.z, speed, flightTime, tostring(capped)))
    launchTrack = {
      vehId    = veh:getId(),
      target   = vec3(targetPos),
      start    = start,
      -- Kept so the miss can be split into along-track (drag: always short) and
      -- cross-track (aiming) components, which say different things about the cause.
      dir      = (L > 0) and flat:normalized() or vec3(0, 0, 0),
      plannedL = L,
      solved   = flightTime,
      t        = 0,
      slowFor  = 0,
    }
  end

  return ok, speed, flightTime, capped, err
end

-- Watch a launched vehicle until it settles, then report the miss. Called every frame
-- while launchTrack is set.
local function updateLaunchTrack(dt)
  local tr = launchTrack
  if not tr then return end

  tr.t = tr.t + dt

  local veh = be:getObjectByID(tr.vehId)
  if not veh then launchTrack = nil; return end

  local pos = vec3(veh:getPosition())
  local spd = 0
  local okV, v = pcall(function() return vec3(veh:getVelocity()) end)
  if okV and v then spd = v:length() end

  -- Only start counting once it is actually moving, so the frame or two before the
  -- impulse takes effect cannot be mistaken for a landing.
  if tr.t > 0.5 and spd < LAUNCH_SETTLE_SPEED then
    tr.slowFor = tr.slowFor + dt
  else
    tr.slowFor = 0
  end

  local settled  = tr.slowFor >= LAUNCH_SETTLE_TIME
  local timedOut = tr.t >= LAUNCH_TRACK_TIMEOUT
  if not settled and not timedOut then return end

  launchTrack = nil

  local miss    = pos - tr.target
  local missFlat = vec3(miss.x, miss.y, 0)
  local along, cross = 0, missFlat:length()
  if tr.dir:length() > 0.5 then
    along = missFlat:dot(tr.dir)                       -- +over, -short
    cross = missFlat:dot(vec3(0, 0, 1):cross(tr.dir))  -- + is left of the launch line
  end

  vsLog('info', string.format(
    "launch result: landed %.2f,%.2f,%.2f | target %.2f,%.2f,%.2f | miss=%.2fm " ..
    "along=%.2f cross=%.2f | plannedL=%.2f actualL=%.2f (%.0f%%) | t=%.2fs solved=%.2fs%s",
    pos.x, pos.y, pos.z, tr.target.x, tr.target.y, tr.target.z, missFlat:length(),
    along, cross, tr.plannedL,
    vec3(pos.x - tr.start.x, pos.y - tr.start.y, 0):length(),
    (tr.plannedL > 0.01) and (vec3(pos.x - tr.start.x, pos.y - tr.start.y, 0):length() / tr.plannedL * 100) or 0,
    tr.t, tr.solved, timedOut and " TIMEOUT" or ""))

  if udpSend then
    -- Metres and seconds; Python owns the units and the phrasing.
    udpSend:send(string.format("TELEPORT_LANDED:%.2f,%.2f,%.2f,%.1f",
      missFlat:length(), along, cross, tr.t))
  end
end

local function handleTeleportPlace(jsonStr)
  local ok, payload = pcall(jsonDecode, jsonStr)
  if not ok or not payload or type(payload.vehId) ~= "number" then
    if udpSend then udpSend:send("TELEPORT_ERR:bad payload") end
    return
  end

  local veh = be:getObjectByID(math.floor(payload.vehId))
  if not veh then
    if udpSend then udpSend:send("TELEPORT_ERR:vehicle not found") end
    return
  end

  -- refVehId nil means "the player vehicle"; resolveReferenceFrame already handles
  -- that fallback, including the camera+ground last resort.
  local frame = resolveReferenceFrame(payload.refMode or "vehicle", payload.refVehId, nil)
  if not frame then
    if udpSend then udpSend:send("TELEPORT_ERR:no reference frame") end
    return
  end

  local FT_TO_M = 0.3048
  local off = {
    fwd   = (payload.offFwdFt   or 0) * FT_TO_M,
    right = (payload.offRightFt or 0) * FT_TO_M,
    up    = (payload.offUpFt    or 0) * FT_TO_M,
  }
  local rotDeg = {
    yaw   = payload.rotYawDeg   or 0,
    pitch = payload.rotPitchDeg or 0,
    roll  = payload.rotRollDeg  or 0,
  }
  local pos, rot = buildSpawnTransform(frame, off, rotDeg)
  if not pos or not rot then
    if udpSend then udpSend:send("TELEPORT_ERR:no transform") end
    return
  end

  -- Force mode aims at the same world point the standard teleport would have used --
  -- everything above is shared -- but throws the vehicle there instead of placing it.
  -- The rotation is dropped: physics decides how it lands.
  if payload.mode == "force" then
    local ok, speed, flightTime, capped, err = launchVehicleTo(veh, pos)
    if udpSend then
      if ok then
        udpSend:send(string.format("TELEPORT_LAUNCHED:%.0f,%.1f,%d",
          speed * 2.23694, flightTime, capped and 1 or 0))
      else
        udpSend:send(("TELEPORT_ERR:" .. tostring(err)):sub(1, 400))
      end
    end
    return
  end

  local moved, err = teleportVehicleTo(veh, pos, rot, off.up == 0)
  if udpSend then
    if moved then
      udpSend:send("TELEPORT_OK")
    else
      udpSend:send(("TELEPORT_ERR:" .. tostring(err)):sub(1, 400))
    end
  end
end

-- =================================================================================================
--  Spawn
-- =================================================================================================

local function handleSpawnBatch(jsonStr)
  local ok, payload = pcall(jsonDecode, jsonStr)
  if not ok or not payload or not payload.items then
    if udpSend then udpSend:send("SPAWN_ERR:bad payload") end
    return
  end

  -- Arrangement mode: geometry computed from anchor rather than per-item side/dist/ref.
  if payload.arrangement and type(payload.arrangement) == "table" then
    handleSpawnBatchArranged(payload.items, payload.arrangement)
    return
  end

  local items = payload.items
  local n = #items
  local successes, failures = 0, {}
  local frames = {}      -- frames[i] = newFrame after spawning queue item i
  local attempted = {}   -- attempted[i] = true once we've tried to spawn item i
  local inProgress = {}  -- cycle guard for prev/next resolution
  local prevFrame = nil  -- chain anchor for refMode == "auto" (in actual spawn order)

  local function trySpawn(i)
    if attempted[i] then return frames[i] ~= nil end
    if inProgress[i] then return false end  -- cycle: bail and let caller fall back
    inProgress[i] = true

    local item = items[i]

    -- Replacement path: swap an existing in-world vehicle with a new model at the same transform.
    local replaceId = item.replaceVehId
    if replaceId and type(replaceId) == "number" and replaceId >= 0 then
      local oldVeh = be:getObjectByID(math.floor(replaceId))
      attempted[i] = true
      inProgress[i] = nil
      if not oldVeh then
        failures[#failures + 1] = tostring(item.model) .. ": replacement target not found"
        return false
      end
      -- Capture frame before the vehicle is replaced so add-items can chain off this position.
      local frame = frameFromVehicle(oldVeh)
      local options = {}
      if item.config and item.config ~= "" then options.config = item.config end
      -- replaceVehicle returns nil on failure without raising, so pcall succeeding
      -- is not enough to call this a success.
      local ok, newVeh = pcall(core_vehicles.replaceVehicle, item.model, options, oldVeh)
      if not ok then
        failures[#failures + 1] = tostring(item.model) .. ": " .. tostring(newVeh)
        return false
      end
      if not newVeh then
        failures[#failures + 1] = tostring(item.model) .. ": replaceVehicle declined"
        return false
      end
      successes = successes + 1
      frames[i] = frame
      if frame then prevFrame = frame end
      return true
    end

    local mode = item.refMode

    -- For "prev"/"next", recursively ensure the dependency is spawned first so we have a frame.
    local depFrame = nil
    local depIdx = nil
    if mode == "prev" then depIdx = i - 1
    elseif mode == "next" then depIdx = i + 1 end
    if depIdx and depIdx >= 1 and depIdx <= n then
      trySpawn(depIdx)
      depFrame = frames[depIdx]
    end

    local frame
    if mode == "prev" or mode == "next" then
      if depFrame then
        frame = depFrame
      else
        -- Out of range or cycle/failure: fall back to player vehicle, then camera+ground.
        local p = be:getPlayerVehicle(0)
        if p then frame = frameFromVehicle(p)
        else frame = frameFromCameraGroundSnap() end
      end
    else
      frame = resolveReferenceFrame(mode or "auto", item.refVehId, prevFrame)
    end

    local FT_TO_M = 0.3048
    local off = {
      fwd   = (item.offFwdFt   or 0) * FT_TO_M,
      right = (item.offRightFt or 0) * FT_TO_M,
      up    = (item.offUpFt    or 0) * FT_TO_M,
    }
    local rotDeg = {
      yaw   = item.rotYawDeg   or 0,
      pitch = item.rotPitchDeg or 0,
      roll  = item.rotRollDeg  or 0,
    }
    local pos, rot, newFrame = buildSpawnTransform(frame, off, rotDeg)
    attempted[i] = true
    inProgress[i] = nil
    if not pos or not rot then
      failures[#failures + 1] = tostring(item.model) .. ": no reference frame"
      return false
    end

    -- cling ground-snaps the vehicle, which would silently throw away a height
    -- offset — so it is only enabled when the user asked for ground level.
    local options = {
      pos = pos, rot = rot,
      cling = (off.up == 0),
      autoEnterVehicle = false,
    }
    if item.config and item.config ~= "" then options.config = item.config end

    local okSp, vehOrErr = pcall(core_vehicles.spawnNewVehicle, item.model, options)
    if not okSp or not vehOrErr then
      failures[#failures + 1] = tostring(item.model) .. ": " .. tostring(vehOrErr)
      return false
    end

    successes = successes + 1
    frames[i] = newFrame
    if newFrame then prevFrame = newFrame end
    return true
  end

  for i = 1, n do
    trySpawn(i)
  end

  if udpSend then
    if #failures == 0 then
      udpSend:send(string.format("SPAWN_OK:%d", successes))
    else
      local msg = string.format("SPAWN_PARTIAL:%d,%d,%s", successes, #failures, table.concat(failures, " | "))
      -- Cap length to avoid UDP fragmentation
      if #msg > 1200 then msg = msg:sub(1, 1200) end
      udpSend:send(msg)
    end
  end
end

-- =================================================================================================
--  In-world Vehicle List (for "mark a vehicle")
-- =================================================================================================

local function sendActiveVehicleList()
  if not udpSend then return end
  local entries = {}
  local seen = {}

  -- Per-vehicle pcall so a single failing object can't abort the whole
  -- enumeration (some scene objects don't expose getJBeamFilename or getID).
  local function add(obj)
    if not obj then return end
    local ok, id = pcall(function() return obj:getID() end)
    if not ok or not id or seen[id] then return end
    seen[id] = true
    local desc = nil
    if extensions and extensions.vehicleNaming and extensions.vehicleNaming.describe then
      pcall(function() desc = extensions.vehicleNaming.describe(obj) end)
    end
    if not desc or desc == "" then
      local jbeam = ""
      pcall(function() jbeam = obj:getJBeamFilename() or "" end)
      desc = jbeam:match("([^/\\]+)$") or jbeam
      desc = desc:gsub("%.jbeam$", "")
      if desc == "" then desc = "vehicle" end
    end
    entries[#entries + 1] = string.format("%d:%s", id, desc)
  end

  -- Preferred: getAllVehicles() returns the canonical list of spawned vehicles.
  local ok, all = pcall(getAllVehicles)
  if ok and type(all) == "table" then
    for _, v in pairs(all) do add(v) end
  end

  -- Fallback / supplement: enumerate via be:getObject so we catch any
  -- vehicle the global helper missed (and so this still works on game builds
  -- where getAllVehicles isn't present).
  local count = 0
  pcall(function() count = be:getObjectCount() end)
  for i = 0, count - 1 do
    local obj = nil
    pcall(function() obj = be:getObject(i) end)
    add(obj)
  end

  udpSend:send("ACTIVE_VEHICLES:" .. table.concat(entries, ";"))
  vsLog('debug', string.format("ACTIVE_VEHICLES sent: %d entries", #entries))
end

-- =================================================================================================
--  Command Dispatch
-- =================================================================================================

local function parseIdList(s)
  local ids = {}
  for tok in string.gmatch(s or "", "([^,]+)") do
    local n = tonumber(tok)
    if n then ids[#ids + 1] = math.floor(n) end
  end
  return ids
end

local function handleReload(idsStr)
  local ids = parseIdList(idsStr)
  local ok, fail = 0, 0
  for _, id in ipairs(ids) do
    local veh = be:getObjectByID(id)
    if veh then
      local model = veh.JBeam
      local config = veh.partConfig
      -- Same here: a nil return means it declined, and pcall reports that as true.
      local r, newVeh = pcall(core_vehicles.replaceVehicle, model, { config = config }, veh)
      if r and newVeh then
        ok = ok + 1
      else
        fail = fail + 1
        vsLog('warn', "reload failed: " .. (r and "replaceVehicle declined" or tostring(newVeh)))
      end
    else
      fail = fail + 1
    end
  end
  if udpSend then udpSend:send(string.format("RELOAD_DONE:%d,%d", ok, fail)) end
end

local function handleRemove(idsStr)
  local ids = parseIdList(idsStr)
  local ok, fail = 0, 0
  for _, id in ipairs(ids) do
    local veh = be:getObjectByID(id)
    if veh then
      local r = pcall(function() veh:delete() end)
      if r then ok = ok + 1 else fail = fail + 1 end
    else
      fail = fail + 1
    end
  end
  if udpSend then udpSend:send(string.format("REMOVE_DONE:%d,%d", ok, fail)) end
end

local function handleIgnitionOff()
  local count = be:getObjectCount()
  local n = 0
  for i = 0, count - 1 do
    local obj = be:getObject(i)
    if obj then
      pcall(function() obj:queueLuaCommand("electrics.setIgnitionLevel(0)") end)
      n = n + 1
    end
  end
  if udpSend then udpSend:send(string.format("IGNITION_OFF_DONE:%d", n)) end
end

local function handleCommand(cmd)
  local upper = cmd:upper()

  if upper == "REQUEST_CATALOG" then
    if not catalogReady then buildCatalog() end
    sendCatalog()
  elseif upper == "REBUILD_CATALOG" then
    buildCatalog()
    sendCatalog()
  elseif upper == "REQUEST_ACTIVE_VEHICLES" then
    sendActiveVehicleList()
  elseif upper == "IGNITION_OFF" then
    handleIgnitionOff()
  elseif cmd:sub(1, 6) == "SPAWN:" then
    handleSpawnBatch(cmd:sub(7))
  elseif cmd:sub(1, 7) == "RELOAD:" then
    handleReload(cmd:sub(8))
  elseif cmd:sub(1, 7) == "REMOVE:" then
    handleRemove(cmd:sub(8))
  elseif cmd:sub(1, 17) == "TELEPORT_ARRANGE:" then
    handleTeleportArrange(cmd:sub(18))
  elseif cmd:sub(1, 15) == "TELEPORT_PLACE:" then
    handleTeleportPlace(cmd:sub(16))
  elseif upper == "REQUEST_PLAYER_VEH_ID" then
    local p = be:getPlayerVehicle(0)
    local id = p and p:getID() or -1
    if udpSend then udpSend:send("PLAYER_VEH_ID:" .. tostring(id)) end
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
    vsLog('info', "UDP send socket targeting " .. PYTHON_HOST .. ":" .. PYTHON_PORT_DATA)
  else
    vsLog('error', "Failed to create UDP send socket.")
  end

  local ok, err = pcall(function()
    udpCmd = socket.udp()
    udpCmd:setsockname("127.0.0.1", CMD_LISTEN_PORT)
    udpCmd:settimeout(0)
  end)
  if ok and udpCmd then
    vsLog('info', "UDP command socket listening on port " .. CMD_LISTEN_PORT)
  else
    vsLog('error', "Failed to create UDP command socket: " .. tostring(err))
    udpCmd = nil
  end
end

function M.onExtensionLoaded()
  setExtensionUnloadMode(M, "manual")
  vsLog('info', "Accessible vehicle spawner extension loaded.")
  -- Bind sockets here so Ctrl+L Lua reload re-opens them.
  setupSockets()
end

function M.onModManagerReady()
  -- Mods changed: invalidate catalog so it rebuilds on next request.
  catalogReady = false
  catalog = nil
  vsLog('info', "Mod manager ready; catalog invalidated.")
end

function M.onClientStartMission()
  catalogReady = false
  catalog = nil
end

function M.onWorldReadyState(state)
  if state == 2 then
    vsLog('info', "World ready. Initializing vehicle spawner.")

    setupSockets()

    -- Pre-build catalog and push it to Python immediately. This ensures Python
    -- has the catalog well before the user presses F11, avoiding the "not ready"
    -- message that would otherwise appear on the very first invocation (because
    -- Python's startup REQUEST_CATALOG is sent before the Lua socket exists and
    -- the packet is silently dropped).
    buildCatalog()
    sendCatalog()
  end
end

function M.onUpdate(dtReal, dtSim, dtRaw)
  -- Ahead of the socket guard: a vehicle already in the air still has to be watched down
  -- even if the command socket failed to bind.
  if launchTrack then
    local ok, err = pcall(updateLaunchTrack, dtReal)
    if not ok then
      vsLog('warn', "launch tracking failed: " .. tostring(err))
      launchTrack = nil
    end
  end

  if not udpCmd then return end
  repeat
    local data = udpCmd:receive()
    if data then
      local cmd = data:match("^%s*(.-)%s*$")
      if cmd and cmd ~= "" then
        handleCommand(cmd)
      end
    end
  until not data
end

return M
