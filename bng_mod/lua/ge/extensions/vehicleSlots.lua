-- =================================================================================================
--
--  Vehicle Slot Tracker for BeamNG.drive Accessibility Mod (GE Extension)
--
--  Maintains a stable numbered list of in-world vehicles (slots 1-10, key "0" = slot 10).
--  Sends slot data to Python on port 4458, receives commands on port 4459.
--  Exported API: M.getSlotVehicleID(slotNum) for use by beamtelAI.lua.
--
--  Loaded by: scripts/bng_screenreader_mod/modScript.lua
--
-- =================================================================================================

local M = {}

local PYTHON_HOST    = "127.0.0.1"
local SLOT_SEND_PORT = 4458   -- Game→Python
local SLOT_CMD_PORT  = 4459   -- Python→Game

local udpSend = nil
local udpCmd  = nil

-- slotList[slotNum] = {id=vehicleID, model=jbeamFilename, name=displayName,
--                      pcKey=lastSeenPartConfig, colorKey=lastSeenColorString}
-- Slots 1..10; slot 10 corresponds to keyboard key "0".
local slotList = {}
local MAX_SLOTS = 10

local worldReady      = false
local validateTimer   = 0
local VALIDATE_INTERVAL = 2.0  -- seconds between cheap stale-slot checks

local function slotLog(level, msg)
  log(level, 'VehicleSlots', msg)
end

-- Build a rich display name (brand, friendly model, config, color) via the
-- vehicleNaming helper. Falls back to the JBeam basename if the helper is
-- unavailable (e.g. during very early startup).
local function getDisplayName(vehicle)
  if not vehicle then return "Unknown" end
  if extensions and extensions.vehicleNaming and extensions.vehicleNaming.describe then
    local ok, name = pcall(extensions.vehicleNaming.describe, vehicle)
    if ok and type(name) == "string" and name ~= "" then return name end
  end
  local filename = vehicle:getJBeamFilename() or ""
  return filename:match("([^/\\]+)%.jbeam$") or filename
end

-- Cheap fingerprint of the inputs vehicleNaming.describe consumes. If the
-- fingerprint hasn't changed since last describe, we can reuse the cached
-- name and skip the per-tick string work that was causing parts-screen
-- hitches when mods stuff malformed values into vehicle.partConfig.
local function describeFingerprint(vehicle)
  if not vehicle then return "", "" end
  local pc = nil
  pcall(function() pc = vehicle.partConfig end)
  local pcStr = type(pc) == "string" and pc or ""

  local c = nil
  pcall(function() c = vehicle.color end)
  if not c then pcall(function() c = vehicle:getColor() end) end
  local colorStr = ""
  if c then
    local r = c.r or c.x or c[1]
    local g = c.g or c.y or c[2]
    local b = c.b or c.z or c[3]
    if type(r) == "number" and type(g) == "number" and type(b) == "number" then
      colorStr = string.format("%.2f,%.2f,%.2f", r, g, b)
    end
  end
  return pcStr, colorStr
end

-- Compute display name once and stash the fingerprint on the slot entry.
local function refreshSlotName(entry, vehicle)
  entry.name = getDisplayName(vehicle)
  entry.pcKey, entry.colorKey = describeFingerprint(vehicle)
end

-- Encode the current slot list and send it to Python.
local function sendSlotData()
  if not udpSend then return end
  local parts = {}
  for slot = 1, MAX_SLOTS do
    local entry = slotList[slot]
    if entry then
      table.insert(parts, slot .. "," .. entry.id .. "," .. entry.name)
    end
  end
  local msg = "SLOTS:" .. table.concat(parts, "|")
  udpSend:send(msg)
end

-- Find the lowest-numbered empty slot (1-10), or nil if full.
local function nextFreeSlot()
  for i = 1, MAX_SLOTS do
    if not slotList[i] then return i end
  end
  return nil
end

-- Compress gaps after a deletion, preserving relative order.
local function renumber()
  local entries = {}
  for i = 1, MAX_SLOTS do
    if slotList[i] then
      table.insert(entries, slotList[i])
    end
  end
  slotList = {}
  for i, entry in ipairs(entries) do
    slotList[i] = entry
  end
  sendSlotData()
end

-- Enumerate all vehicles in world and assign slots.
-- Player vehicle always gets slot 1; others fill 2..MAX_SLOTS in index order.
local function buildInitialList()
  slotList = {}
  local playerVeh = be:getPlayerVehicle(0)
  local playerID  = playerVeh and playerVeh:getID() or nil

  if playerVeh and playerID then
    local entry = {
      id    = playerID,
      model = playerVeh:getJBeamFilename() or "",
    }
    refreshSlotName(entry, playerVeh)
    slotList[1] = entry
    slotLog('info', "Slot 1 = " .. entry.name .. " (player, id=" .. tostring(playerID) .. ")")
  end

  local nextSlot = 2
  -- be:getObjectCount() is the correct BeamNG GE API for vehicle count.
  for i = 0, be:getObjectCount() - 1 do
    local v = be:getObject(i)
    if v then
      local vid = v:getID()
      if vid ~= playerID and nextSlot <= MAX_SLOTS then
        local entry = {
          id    = vid,
          model = v:getJBeamFilename() or "",
        }
        refreshSlotName(entry, v)
        slotList[nextSlot] = entry
        slotLog('info', "Slot " .. nextSlot .. " = " .. entry.name .. " (id=" .. tostring(vid) .. ")")
        nextSlot = nextSlot + 1
      end
    end
  end

  sendSlotData()
end

-- Re-check every occupied slot against the live scene. Drops slots whose
-- vehicle ID no longer resolves (e.g. removed via the spawner without
-- onVehicleDestroyed firing on our extension). Only re-runs the expensive
-- describe pipeline when the vehicle's partConfig or color actually
-- changed, so steady-state ticks are essentially free.
local function validateSlots()
  local removed = false
  local renamed = false
  for i = 1, MAX_SLOTS do
    local entry = slotList[i]
    if entry then
      local v = scenetree.findObjectById(entry.id)
      if not v then
        slotLog('info', "Slot " .. i .. " stale (id=" .. tostring(entry.id) .. " gone): removing")
        slotList[i] = nil
        removed = true
      else
        local pcKey, colorKey = describeFingerprint(v)
        if pcKey ~= entry.pcKey or colorKey ~= entry.colorKey then
          local oldName = entry.name
          refreshSlotName(entry, v)
          if entry.name ~= oldName then
            slotLog('info', "Slot " .. i .. " name updated: " .. entry.name)
            renamed = true
          end
        end
        local currentModel = v:getJBeamFilename() or ""
        if currentModel ~= "" and currentModel ~= entry.model then
          entry.model = currentModel
        end
      end
    end
  end
  if removed then
    renumber()  -- compresses gaps and calls sendSlotData()
  elseif renamed then
    sendSlotData()
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
    udpSend:setpeername(PYTHON_HOST, SLOT_SEND_PORT)
    udpSend:settimeout(0)
    slotLog('info', "UDP send socket ready (port " .. SLOT_SEND_PORT .. ")")
  else
    slotLog('error', "Failed to create UDP send socket")
  end

  local ok, err = pcall(function()
    udpCmd = socket.udp()
    -- setsockname RETURNS nil plus a message; it does not THROW. A pcall around it reports
    -- success on a socket bound to nothing, and the extension then goes deaf with nothing in
    -- the log -- it still sends normally, because a UDP sender needs no bind.
    local bound, berr = udpCmd:setsockname("127.0.0.1", SLOT_CMD_PORT)
    if not bound then error(tostring(berr), 0) end
    udpCmd:settimeout(0)
  end)
  if ok and udpCmd then
    slotLog('info', "UDP command socket listening on port " .. SLOT_CMD_PORT)
  else
    slotLog('error', "Failed to create UDP command socket: " .. tostring(err))
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
    local bound = sk:setsockname("127.0.0.1", SLOT_CMD_PORT)
    if not bound then sk:close(); error("still in use", 0) end
    sk:settimeout(0)
    udpCmd = sk
  end)
  if ok and udpCmd then
    slotLog('info', "UDP command socket bound on port " .. SLOT_CMD_PORT .. " after retry.")
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
  slotLog('info', "Vehicle slot tracker loaded.")
  -- Bind sockets here so Ctrl+L Lua reload re-opens them.
  setupSockets()
end

function M.onWorldReadyState(state)
  if state ~= 2 then return end

  setupSockets()

  worldReady = true

  -- Wrap in pcall so any enumeration error is logged rather than fatal.
  local ok2, err2 = pcall(buildInitialList)
  if not ok2 then
    slotLog('error', "buildInitialList failed: " .. tostring(err2))
  end
end

function M.onVehicleSpawned(vid)
  if not worldReady then return end

  -- Skip if already tracked (avoids duplicating initial vehicles).
  for i = 1, MAX_SLOTS do
    if slotList[i] and slotList[i].id == vid then return end
  end

  -- Look up the vehicle object by its numeric ID via the scene tree.
  local v = scenetree.findObjectById(vid)
  if not v then
    slotLog('warn', "onVehicleSpawned: could not find vehicle id=" .. tostring(vid))
    return
  end

  local slot = nextFreeSlot()
  if not slot then
    slotLog('warn', "All 10 slots full; cannot track new vehicle id=" .. tostring(vid))
    return
  end
  local entry = {
    id    = vid,
    model = v:getJBeamFilename() or "",
  }
  refreshSlotName(entry, v)
  slotList[slot] = entry
  slotLog('info', "Vehicle spawned: slot " .. slot .. " = " .. entry.name)
  sendSlotData()
end

-- When the player swaps active vehicles (e.g. via vehicle selector or our
-- scanner), re-send the slot list so Python's view stays in sync. Cheap —
-- doesn't touch describe.
function M.onVehicleSwitched(oldId, newId, player)
  if player ~= 0 then return end
  if not worldReady then return end
  sendSlotData()
end

function M.onVehicleDestroyed(vid)
  for i = 1, MAX_SLOTS do
    if slotList[i] and slotList[i].id == vid then
      slotLog('info', "Vehicle destroyed: removing slot " .. i .. " (" .. slotList[i].name .. ")")
      slotList[i] = nil
      renumber()
      return
    end
  end
end

function M.onUpdate(dtReal, dtSim, dtRaw)
  retryCmdBind(dtReal)
  -- Throttled stale-slot validation. validateSlots short-circuits on a
  -- per-slot fingerprint check, so steady-state ticks don't re-run the
  -- describe pipeline that previously caused parts-screen hitches with
  -- mods that ship malformed partConfig values.
  validateTimer = validateTimer + dtReal
  if validateTimer >= VALIDATE_INTERVAL then
    validateTimer = 0
    if worldReady then
      pcall(validateSlots)
    end
  end

  -- Command polling.
  if not udpCmd then return end
  local data = udpCmd:receive()
  if not data then return end
  local cmd = data:match("^%s*(.-)%s*$")
  if cmd == "SLOT_STATUS" then
    sendSlotData()
  elseif cmd == "SLOT_RESET" then
    local ok, err = pcall(buildInitialList)
    if not ok then
      slotLog('error', "SLOT_RESET buildInitialList failed: " .. tostring(err))
    end
  elseif cmd == "SLOT_VALIDATE" then
    local ok, err = pcall(validateSlots)
    if not ok then
      slotLog('error', "SLOT_VALIDATE failed: " .. tostring(err))
    end
  end
end

-- Returns the vehicle ID for a given slot number (1-10), or nil if the slot is empty.
function M.getSlotVehicleID(slotNum)
  local entry = slotList[slotNum]
  return entry and entry.id or nil
end

return M
