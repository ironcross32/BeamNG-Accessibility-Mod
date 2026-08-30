-- =================================================================================================
--
--  Accessible Clickspot Detection for BeamNG.drive Accessibility Mod (GE Extension)
--
--  Description: Detects vehicle interior trigger clickspots under the mouse cursor using
--               be:triggerRaycastClosest(). Enumerates all triggers on activation and sends
--               the list + hover events to Python via UDP. Supports SNAP (cursor warp to
--               trigger position) and EXECUTE (fire a trigger action directly).
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
local PYTHON_PORT_DATA  = 4456   -- send clickspot data to Python on this port
local CMD_LISTEN_PORT   = 4457   -- receive commands from Python on this port
local MAX_TRIGGER_DIST  = 1000   -- max raycast distance for trigger detection

-- Internal State
local udpSend           = nil
local udpCmd            = nil
local isActive          = false
local triggerCache      = nil     -- array of {triggerId, name, actionName, actionStr, namespace, ...}
local triggerCacheVehID = nil
local lastHoveredId     = -1
local pendingCacheVehID = nil     -- set when we need to retry cache building
local cacheRetryTimer   = 0
local CACHE_RETRY_INTERVAL = 0.5  -- seconds between retry attempts
local MAX_CACHE_RETRIES = 10
local cacheRetryCount   = 0

-- Logging helper
local function csLog(level, msg)
  log(level, 'ClickspotAccess', msg)
end

local function sendListState(packet)
  if udpSend then udpSend:send(packet) end
end


-- =================================================================================================
--  Pick the one action string to present, and to fire, for a trigger.
--
--  A trigger's links are keyed by action string: triggerEventLinksDict[id][actionStr].
--  Those are separate interactions, not one compound action -- the game maps the
--  triggerAction0/1/2 input to a single 'action0'/'action1'/'action2' and calls
--  core_vehicleTriggers.triggerEvent with only that one (core/vehicleTriggers.lua,
--  onActionEvent). action0 is the primary click, so prefer it.
--
--  The fallback is the lowest-sorted key rather than "whatever pairs() yields
--  first": Lua leaves pairs() order unspecified, so the previous code could cache
--  and announce a different action from one session to the next.
-- =================================================================================================

local function pickActionStr(actionMap)
  if type(actionMap) ~= 'table' then return nil end
  if actionMap.action0 ~= nil then return "action0" end
  local best = nil
  for aStr in pairs(actionMap) do
    if best == nil or aStr < best then best = aStr end
  end
  return best
end

-- =================================================================================================
--  Trigger Enumeration
-- =================================================================================================

local function buildTriggerCache()
  local vehId = be:getPlayerVehicleID(0)
  if not vehId or vehId < 0 then
    csLog('warn', "No player vehicle for trigger cache.")
    triggerCache = {}
    triggerCacheVehID = nil
    pendingCacheVehID = nil
    return
  end

  if triggerCacheVehID == vehId and triggerCache and #triggerCache > 0 then
    pendingCacheVehID = nil
    return  -- already cached with data
  end

  csLog('info', "Building trigger cache for vehicle ID " .. vehId)

  local vData = extensions.core_vehicle_manager.getVehicleData(vehId)
  if not vData or not vData.vdata or not vData.vdata.triggers then
    -- Data not ready yet — schedule a retry
    csLog('info', "Vehicle data not ready yet, will retry...")
    pendingCacheVehID = vehId
    cacheRetryTimer = 0
    cacheRetryCount = 0
    triggerCache = {}
    triggerCacheVehID = nil
    return
  end

  -- Check if triggers table is actually populated
  local triggerCount = 0
  for _ in pairs(vData.vdata.triggers) do triggerCount = triggerCount + 1 end

  if triggerCount == 0 then
    -- Might be loading, or vehicle genuinely has no triggers
    if cacheRetryCount < MAX_CACHE_RETRIES then
      csLog('info', "No triggers yet (attempt " .. cacheRetryCount .. "), will retry...")
      pendingCacheVehID = vehId
      triggerCache = {}
      triggerCacheVehID = nil
      return
    else
      -- Give up — vehicle genuinely has no triggers
      csLog('info', "No triggers found for this vehicle after retries.")
      triggerCache = {}
      triggerCacheVehID = vehId
      pendingCacheVehID = nil
      if udpSend then
        udpSend:send("TRIGGER_LIST:0")
      end
      return
    end
  end

  -- Success — clear retry state
  pendingCacheVehID = nil
  cacheRetryCount = 0
  triggerCache = {}
  triggerCacheVehID = vehId

  local triggers = vData.vdata.triggers
  local linksDict = vData.vdata.triggerEventLinksDict or {}
  local inputActions = vData.vdata.inputActions or {}

  for triggerId, trg in pairs(triggers) do
    -- Get the trigger's friendly name
    local trgName = trg.name or ("trigger_" .. tostring(triggerId))
    -- Try to translate localization keys
    local ok, translated = pcall(function()
      return translateLanguage(trgName, trgName, true)
    end)
    if ok and translated and translated ~= "" then
      trgName = translated
    end

    -- Find linked action name for extra context
    local actionName = ""
    local actionStr = "action0"
    local namespace = "vehicle"
    local hasLinks = false

    local chosenActionStr = pickActionStr(linksDict[triggerId])
    if chosenActionStr then
      for _, lnk in pairs(linksDict[triggerId][chosenActionStr]) do
        hasLinks = true
        actionStr = chosenActionStr
        if lnk.inputAction then
          actionName = lnk.inputAction
          namespace = lnk.namespace or "vehicle"

          -- Try to get a friendlier title from the inputAction
          local act = inputActions[lnk.inputAction]
          if act and act.title then
            local ok2, actTitle = pcall(function()
              return translateLanguage(act.title, act.title, true)
            end)
            if ok2 and actTitle and actTitle ~= "" and actTitle ~= act.title then
              actionName = actTitle
            end
          end
        elseif lnk.targetEvent and lnk.targetEvent.name then
          actionName = lnk.targetEvent.name
        end
        break  -- take first link
      end
    end

    triggerCache[#triggerCache + 1] = {
      triggerId = triggerId,
      name = trgName,
      actionName = actionName,
      actionStr = actionStr,
      namespace = namespace,
      hasLinks = hasLinks,
    }
  end

  -- Sort by name for consistent ordering
  table.sort(triggerCache, function(a, b) return a.name < b.name end)

  csLog('info', "Trigger cache ready: " .. #triggerCache .. " triggers")

  -- Send the full trigger list to Python
  if udpSend then
    -- First send count
    udpSend:send("TRIGGER_LIST:" .. #triggerCache)

    -- Then send each trigger as a separate packet
    for i, t in ipairs(triggerCache) do
      local displayName = t.name
      if t.actionName ~= "" and t.actionName ~= t.name then
        displayName = t.name .. " (" .. t.actionName .. ")"
      end
      local packet = string.format("TRIGGER:%d,%d,%s",
        i - 1, t.triggerId, displayName)
      udpSend:send(packet)
    end
  end
end

-- =================================================================================================
--  Hover Detection
-- =================================================================================================

local function checkHover()
  if not udpSend then return end

  local hit = be:triggerRaycastClosest(MAX_TRIGGER_DIST, true)

  if hit then
    local triggerId = hit.t
    if triggerId ~= lastHoveredId then
      lastHoveredId = triggerId

      -- Find this trigger in our cache
      local trgInfo = nil
      for _, t in ipairs(triggerCache) do
        if t.triggerId == triggerId then
          trgInfo = t
          break
        end
      end

      if trgInfo then
        local displayName = trgInfo.name
        if trgInfo.actionName ~= "" and trgInfo.actionName ~= trgInfo.name then
          displayName = trgInfo.name .. ", " .. trgInfo.actionName
        end
        udpSend:send("HOVER:" .. tostring(triggerId) .. "," .. displayName)
      else
        -- Trigger not in our cache (might be from another vehicle)
        udpSend:send("HOVER:" .. tostring(triggerId) .. ",unknown trigger")
      end
    end
  else
    if lastHoveredId ~= -1 then
      lastHoveredId = -1
      udpSend:send("HOVER_CLEAR")
    end
  end
end

-- =================================================================================================
--  SNAP: Project trigger world position to screen coordinates
-- =================================================================================================

local function handleSnap(cacheIndex)
  if not triggerCache or #triggerCache == 0 then return end
  if cacheIndex < 0 or cacheIndex >= #triggerCache then return end

  local trgInfo = triggerCache[cacheIndex + 1]  -- Lua 1-indexed
  if not trgInfo then return end

  local vehId = be:getPlayerVehicleID(0)
  if not vehId or vehId < 0 then return end

  local vData = extensions.core_vehicle_manager.getVehicleData(vehId)
  if not vData or not vData.vdata or not vData.vdata.triggers then return end

  local trg = vData.vdata.triggers[trgInfo.triggerId]
  if not trg then return end

  -- Get the trigger's world position
  -- Triggers are attached to vehicle nodes; we need to get the world position
  local player = be:getPlayerVehicle(0)
  if not player then return end

  local objPos = player:getPosition()

  -- The trigger has a translation relative to its reference node
  -- We need to compute the world position: vehicle position + node position + trigger offset
  -- For a reasonable approximation, use the reference node position + translation
  local triggerWorldPos = nil

  if trg.idRef then
    local nodePos = vec3(player:getNodePosition(trg.idRef))
    local offset = vec3(0, 0, 0)
    if trg.translation then
      offset = vec3(trg.translation.x or trg.translation[1] or 0,
                     trg.translation.y or trg.translation[2] or 0,
                     trg.translation.z or trg.translation[3] or 0)
    end
    triggerWorldPos = objPos + nodePos + offset
  else
    -- Fallback: use vehicle center
    triggerWorldPos = objPos
  end

  -- Project to screen coordinates
  local camPos = core_camera.getPosition()
  local camFwd = core_camera.getForward()
  local camRight = core_camera.getRight()
  local camUp = core_camera.getUp()
  if not camPos or not camFwd or not camRight or not camUp then return end

  local camPosV = vec3(camPos.x, camPos.y, camPos.z)
  local camFwdN = vec3(camFwd.x, camFwd.y, camFwd.z):normalized()

  local toTrigger = triggerWorldPos - camPosV
  local fwdDot = toTrigger:dot(camFwdN)

  if fwdDot <= 0.01 then
    -- Trigger is behind camera
    if udpSend then
      udpSend:send("SNAP_FAIL:behind camera")
    end
    return
  end

  local rightDot = toTrigger:dot(vec3(camRight.x, camRight.y, camRight.z):normalized())
  local upDot = toTrigger:dot(vec3(camUp.x, camUp.y, camUp.z):normalized())

  local fovDeg = core_camera.getFovDeg()
  local vm = GFXDevice.getVideoMode()
  local screenW = vm.width
  local screenH = vm.height

  local fovRad = math.rad(fovDeg)
  local tanHalfFov = math.tan(fovRad / 2.0)
  local aspect = screenW / screenH

  local ndcX = (rightDot / fwdDot) / (tanHalfFov * aspect)
  local ndcY = (upDot / fwdDot) / tanHalfFov

  -- Check if on screen
  if math.abs(ndcX) > 1.1 or math.abs(ndcY) > 1.1 then
    if udpSend then
      udpSend:send("SNAP_FAIL:off screen")
    end
    return
  end

  local screenX = math.floor((ndcX + 1.0) * 0.5 * screenW)
  local screenY = math.floor((1.0 - ndcY) * 0.5 * screenH)

  screenX = math.max(0, math.min(screenW - 1, screenX))
  screenY = math.max(0, math.min(screenH - 1, screenY))

  if udpSend then
    local displayName = trgInfo.name
    if trgInfo.actionName ~= "" and trgInfo.actionName ~= trgInfo.name then
      displayName = trgInfo.name .. ", " .. trgInfo.actionName
    end
    -- Viewport size travels with the point: these are coordinates inside the
    -- render area, and Python has to place that area on the desktop.
    udpSend:send(string.format("SNAP_OK:%d,%d,%d,%d,%d,%s",
      screenX, screenY, screenW, screenH, trgInfo.triggerId, displayName))
    csLog('info', string.format("SNAP to trigger %d (%s) at screen %d,%d",
      trgInfo.triggerId, trgInfo.name, screenX, screenY))
  end
end

-- =================================================================================================
--  EXECUTE: Fire a trigger's linked action directly
-- =================================================================================================

local function handleExecute(cacheIndex, actionValue)
  if not triggerCache or #triggerCache == 0 then return end
  if cacheIndex < 0 or cacheIndex >= #triggerCache then return end

  local trgInfo = triggerCache[cacheIndex + 1]
  if not trgInfo or not trgInfo.hasLinks then
    csLog('warn', "Trigger has no linked actions")
    return
  end

  local vehId = be:getPlayerVehicleID(0)
  if not vehId or vehId < 0 then return end

  local vData = extensions.core_vehicle_manager.getVehicleData(vehId)
  if not vData or not vData.vdata then return end

  local actionStr = trgInfo.actionStr or "action0"

  -- Fire exactly ONE action, the way the game does.
  --
  -- This used to iterate every key of triggerEventLinksDict[triggerId] and fire
  -- all of them, so activating a single clickspot triggered every interaction
  -- the trigger had. The game never does that: onActionEvent in
  -- core/vehicleTriggers.lua resolves the input to one 'actionN' string and
  -- passes just that to triggerEvent.
  --
  -- Delegate to that same triggerEvent rather than keeping our own copy of the
  -- v2 / common / legacy link branches, which can only drift from the original.
  local gameTriggers = core_vehicleTriggers
  if gameTriggers and gameTriggers.triggerEvent then
    gameTriggers.triggerEvent(actionStr, actionValue, trgInfo.triggerId, vehId, vData.vdata)
  else
    -- Fallback if the stock extension is not loaded: same logic, but still
    -- restricted to the single selected action string.
    csLog('warn', "core_vehicleTriggers unavailable; using local link execution")
    local linksDict = vData.vdata.triggerEventLinksDict or {}
    local links = linksDict[trgInfo.triggerId]
    local linkTable = links and links[actionStr]
    if not linkTable then return end

    for _, lnk in pairs(linkTable) do
      local value = actionValue
      if lnk.isInverted then value = -value end

      if lnk.version and lnk.version == 2 then
        if lnk.namespace == 'vehicle' then
          local act = vData.vdata.inputActions[lnk.inputAction]
          if act then
            core_input_actions.executeCommand(act, value, vehId)
          end
        elseif lnk.namespace == 'common' then
          if lnk.commonLua then
            local act = vData.vdata.inputActions[lnk.inputAction]
            if act then
              core_input_actions.executeCommand(act, value, vehId)
            end
          else
            ActionMap.triggerBindingByNameDigital(lnk.inputAction, value > 0.9, os.clockhp(), vehId)
          end
        end
      else
        -- old format
        if lnk.targetEvent then
          core_input_actions.executeCommand(lnk.targetEvent, value, vehId)
        end
      end
    end
  end

  csLog('info', string.format("Executed trigger %d (%s) action %s with value %s",
    trgInfo.triggerId, trgInfo.name, actionStr, tostring(actionValue)))
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
    csLog('info', "UDP send socket targeting " .. PYTHON_HOST .. ":" .. PYTHON_PORT_DATA)
  else
    csLog('error', "Failed to create UDP send socket.")
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
    csLog('info', "UDP command socket listening on port " .. CMD_LISTEN_PORT)
  else
    csLog('error', "Failed to create UDP command socket: " .. tostring(err))
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
    csLog('info', "UDP command socket bound on port " .. CMD_LISTEN_PORT .. " after retry.")
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
  csLog('info', "Accessible clickspot detection extension loaded.")
  -- Bind sockets here so Ctrl+L Lua reload re-opens them.
  setupSockets()
  sendListState("TRIGGER_LIST_OFF")
end

function M.onVehicleSwitched(oldId, newId, player)
  if not isActive then return end
  if player ~= 0 then return end  -- only care about player 0
  csLog('info', "Vehicle switched from " .. tostring(oldId) .. " to " .. tostring(newId))
  -- Force a cache rebuild with retry
  sendListState("TRIGGER_LIST_PENDING")
  triggerCache = {}
  triggerCacheVehID = nil
  lastHoveredId = -1
  pendingCacheVehID = newId
  cacheRetryTimer = 0
  cacheRetryCount = 0
end

function M.onWorldReadyState(state)
  if state == 2 then
    csLog('info', "World ready. Initializing clickspot detection.")

    -- Reset state
    isActive = false
    triggerCache = nil
    triggerCacheVehID = nil
    lastHoveredId = -1

    setupSockets()
    sendListState("TRIGGER_LIST_OFF")
  end
end

function M.onUpdate(dtReal, dtSim, dtRaw)
  retryCmdBind(dtReal)
  -- 1. Poll for commands from Python (drain all queued packets)
  if udpCmd then
    repeat
      local data = udpCmd:receive()
      if data then
        local cmd = data:match("^%s*(.-)%s*$")
        local cmdUpper = cmd:upper()

        if cmdUpper == "ON" and not isActive then
          isActive = true
          lastHoveredId = -1
          triggerCacheVehID = nil
          pendingCacheVehID = nil
          cacheRetryCount = 0
          cacheRetryTimer = 0
          csLog('info', "Clickspot detection activated.")
          sendListState("TRIGGER_LIST_PENDING")
          buildTriggerCache()
        elseif cmdUpper == "OFF" and isActive then
          isActive = false
          lastHoveredId = -1
          csLog('info', "Clickspot detection deactivated.")
          sendListState("TRIGGER_LIST_OFF")
        elseif cmdUpper == "RECACHE" and isActive then
          sendListState("TRIGGER_LIST_PENDING")
          triggerCacheVehID = nil
          buildTriggerCache()
        elseif cmd:sub(1, 5) == "SNAP:" then
          local idx = tonumber(cmd:sub(6))
          if idx and isActive then
            handleSnap(idx)
          end
        elseif cmd:sub(1, 5) == "EXEC:" then
          -- EXEC:<cacheIndex>,<value> (value: 1=press, 0=release)
          local idxStr, valStr = cmd:sub(6):match("^(%d+),(%d+)$")
          if idxStr and valStr and isActive then
            handleExecute(tonumber(idxStr), tonumber(valStr))
          end
        end
      end
    until not data
  end

  if not isActive then return end

  -- 2. Check if vehicle changed
  local currentVehId = be:getPlayerVehicleID(0)
  if currentVehId and currentVehId >= 0 and currentVehId ~= triggerCacheVehID and currentVehId ~= pendingCacheVehID then
    sendListState("TRIGGER_LIST_PENDING")
    pendingCacheVehID = currentVehId
    cacheRetryTimer = 0
    cacheRetryCount = 0
    triggerCache = {}
    triggerCacheVehID = nil
    lastHoveredId = -1
  end

  -- 3. Retry cache building if pending
  if pendingCacheVehID then
    cacheRetryTimer = cacheRetryTimer + dtReal
    if cacheRetryTimer >= CACHE_RETRY_INTERVAL then
      cacheRetryTimer = 0
      cacheRetryCount = cacheRetryCount + 1
      buildTriggerCache()
    end
  end

  -- 4. Detect hovered clickspot
  checkHover()
end

return M
