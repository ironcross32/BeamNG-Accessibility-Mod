-- =================================================================================================
--
--  AI Vehicle Control for BeamNG.drive Accessibility Mod (GE Extension)
--
--  Description: Receives AI control commands via UDP from beamtel.py and applies them
--               to the player vehicle's AI system. Sends responses back via UDP.
--
--  Loaded by: scripts/bng_screenreader_mod/modScript.lua
--
--  Target Game: BeamNG.drive 0.37+
--
-- =================================================================================================

local M = {}

-- Configuration
local PYTHON_HOST     = "127.0.0.1"
local PYTHON_PORT     = 4445   -- send responses on scanner data port (with AI_ prefix)
local CMD_LISTEN_PORT = 4449   -- receive commands from Python on this port

-- Internal State
local udpSend = nil
local udpCmd  = nil

-- AI State Tracking
local currentMode       = "disabled"
local currentAggression = 1.0
local currentSpeedLimit = nil   -- nil = no limit (m/s internally)
local avoidMode         = "auto" -- "auto", "on", "off"
local laneDriving       = false

-- STATUS_ALL state: collects per-vehicle AI query callbacks with a timeout
local _statusAllPending = 0
local _statusAllResults = {}   -- vid → {mode, targetId}
local _statusAllSlotMap = {}   -- vid → slotNum
local _statusAllTimeout = 0
local STATUS_ALL_TIMEOUT = 2.0  -- give vehicle VMs this long to respond

-- Logging helper
local function aiLog(level, msg)
  log(level, 'BeamtelAI', msg)
end

-- Send a response string to Python on the scanner data port.
-- Defined up here (before _sendStatusAll) because Lua locals are only visible
-- after their declaration; the earlier definition further down had _sendStatusAll
-- resolving 'sendResponse' as a global, which was nil and crashed STATUS_ALL.
local function sendResponse(msg)
  if udpSend then
    udpSend:send(msg)
  end
end

-- Build a human-friendly display name (brand, friendly model, config, color)
-- for a vehicle ID. Commas are stripped because AI_STATUS_ALL's wire format
-- splits fields on ',' on the Python side.
local function jbeamDisplayName(vid)
  local v = scenetree.findObjectById(vid)
  if not v then return "unknown" end
  if extensions and extensions.vehicleNaming and extensions.vehicleNaming.describe then
    local ok, name = pcall(extensions.vehicleNaming.describe, v)
    if ok and type(name) == "string" and name ~= "" then
      return (name:gsub(",", ""))
    end
  end
  local f = v:getJBeamFilename() or ""
  return f:match("([^/\\]+)%.jbeam$") or f
end

-- Compile collected STATUS_ALL results and send to Python.
local function _sendStatusAll()
  local parts = {}
  -- Sort by slot number so Python receives them in order.
  local sorted = {}
  for vid, slotNum in pairs(_statusAllSlotMap) do
    table.insert(sorted, {vid = vid, slot = slotNum})
  end
  table.sort(sorted, function(a, b) return a.slot < b.slot end)

  for _, entry in ipairs(sorted) do
    local vid    = entry.vid
    local slot   = entry.slot
    local result = _statusAllResults[vid] or {mode = "no response", targetId = -1}
    local mode   = result.mode
    local tid    = result.targetId
    local tname  = (tid and tid > 0) and jbeamDisplayName(tid) or ""
    local vname  = jbeamDisplayName(vid)
    table.insert(parts, string.format("%d,%s,%s,%s", slot, vname, mode, tname))
  end

  sendResponse("AI_STATUS_ALL:" .. table.concat(parts, "|"))

  -- Reset state.
  _statusAllPending = 0
  _statusAllResults = {}
  _statusAllSlotMap = {}
  _statusAllTimeout = 0
end

-- Called from vehicle VMs via obj:queueGameEngineLua after a STATUS_ALL query.
function M.onVehicleStatus(vid, mode, targetId)
  if _statusAllPending <= 0 then return end  -- stale callback after timeout
  _statusAllResults[vid] = {mode = mode, targetId = targetId}
  _statusAllPending = _statusAllPending - 1
  if _statusAllPending <= 0 then
    _sendStatusAll()
  end
end

-- Apply AI mode to the player vehicle
local function applyMode(mode, targetID)
  local player = be:getPlayerVehicle(0)
  if not player then
    sendResponse("AI_ERR:No player vehicle")
    return
  end

  currentMode = mode

  if mode == "disabled" then
    player:queueLuaCommand('ai.setMode("disabled")')
    sendResponse("AI_OK:AI disabled")
    return
  end

  if mode == "stop" then
    player:queueLuaCommand('ai.setMode("stop")')
    sendResponse("AI_OK:AI stopped")
    return
  end

  -- For chase/follow/flee, set target first
  if (mode == "chase" or mode == "follow" or mode == "flee") and targetID then
    player:queueLuaCommand('ai.setTargetObjectID(' .. targetID .. ')')
  end

  player:queueLuaCommand('ai.setMode("' .. mode .. '")')
  player:queueLuaCommand('ai.setAggression(' .. currentAggression .. ')')

  if currentSpeedLimit then
    player:queueLuaCommand('ai.setSpeedMode("limit")')
    player:queueLuaCommand('ai.setSpeed(' .. currentSpeedLimit .. ')')
  end

  local desc = mode
  if targetID then desc = desc .. " (target " .. targetID .. ")" end
  sendResponse("AI_OK:" .. desc)
end

-- =================================================================================================
--  GE Extension Hooks
-- =================================================================================================

local function setupSockets()
  if udpSend then pcall(function() udpSend:close() end); udpSend = nil end
  if udpCmd  then pcall(function() udpCmd:close()  end); udpCmd  = nil end

  udpSend = socket.udp()
  if udpSend then
    udpSend:setpeername(PYTHON_HOST, PYTHON_PORT)
    udpSend:settimeout(0)
    aiLog('info', "UDP send socket created, targeting " .. PYTHON_HOST .. ":" .. PYTHON_PORT)
  else
    aiLog('error', "Failed to create UDP send socket.")
  end

  local ok, err = pcall(function()
    udpCmd = socket.udp()
    udpCmd:setsockname("127.0.0.1", CMD_LISTEN_PORT)
    udpCmd:settimeout(0)
  end)
  if ok and udpCmd then
    aiLog('info', "UDP command socket listening on port " .. CMD_LISTEN_PORT)
  else
    aiLog('error', "Failed to create UDP command socket: " .. tostring(err))
    udpCmd = nil
  end
end

function M.onExtensionLoaded()
  setExtensionUnloadMode(M, "manual")
  aiLog('info', "AI control extension loaded.")
  -- Bind sockets here so Ctrl+L Lua reload re-opens them.
  setupSockets()
end

function M.onWorldReadyState(state)
  aiLog('info', "onWorldReadyState triggered with state: " .. tostring(state))

  if state == 2 then
    aiLog('info', "World is ready. Initializing AI control systems.")
    setupSockets()
  end
end

function M.onUpdate(dtReal, dtSim, dtRaw)
  -- Drive the STATUS_ALL timeout so unresponsive vehicles don't stall the report.
  if _statusAllTimeout > 0 then
    _statusAllTimeout = _statusAllTimeout - dtReal
    if _statusAllTimeout <= 0 and _statusAllPending > 0 then
      aiLog('warn', "STATUS_ALL timed out; " .. _statusAllPending .. " vehicle(s) did not respond")
      -- Fill missing entries so the report still goes out.
      for vid, _ in pairs(_statusAllSlotMap) do
        if not _statusAllResults[vid] then
          _statusAllResults[vid] = {mode = "no response", targetId = -1}
        end
      end
      _statusAllPending = 0
      _sendStatusAll()
    end
  end

  if not udpCmd then return end

  local data = udpCmd:receive()
  if not data then return end

  local cmd = data:match("^%s*(.-)%s*$")
  aiLog('info', "Received command: " .. cmd)

  -- Parse "TYPE:arg" format
  local cmdType, cmdArg = cmd:match("^([%u_]+):(.*)$")
  if not cmdType then
    cmdType = cmd
    cmdArg = nil
  end

  if cmdType == "MODE" then
    local mode = cmdArg
    local targetID = nil

    -- For chase/follow/flee, get target from vehicle scanner
    if mode == "chase" or mode == "follow" or mode == "flee" then
      targetID = extensions.vehicleScanner.getCurrentTargetID()
      if not targetID then
        sendResponse("AI_ERR:No scanner target. Enable vehicle scanner first (F9+CTRL+V)")
        return
      end
    end

    applyMode(mode, targetID)

  elseif cmdType == "AGGR" then
    local val = tonumber(cmdArg)
    if val then
      currentAggression = math.max(0.1, math.min(2.0, val))
      local player = be:getPlayerVehicle(0)
      if player then
        player:queueLuaCommand('ai.setAggression(' .. currentAggression .. ')')
        sendResponse("AI_OK:aggression " .. string.format("%.1f", currentAggression))
      else
        sendResponse("AI_ERR:No player vehicle")
      end
    end

  elseif cmdType == "SPEED" then
    local val = tonumber(cmdArg)
    if val then
      currentSpeedLimit = val
      local player = be:getPlayerVehicle(0)
      if player then
        player:queueLuaCommand('ai.setSpeedMode("limit")')
        player:queueLuaCommand('ai.setSpeed(' .. val .. ')')
        local kmh = math.floor(val * 3.6 + 0.5)
        sendResponse("AI_OK:speed limit " .. kmh .. " km/h")
      else
        sendResponse("AI_ERR:No player vehicle")
      end
    end

  elseif cmdType == "CLEARSPEED" then
    currentSpeedLimit = nil
    local player = be:getPlayerVehicle(0)
    if player then
      player:queueLuaCommand('ai.setSpeedMode("off")')
      sendResponse("AI_OK:speed limit cleared")
    else
      sendResponse("AI_ERR:No player vehicle")
    end

  elseif cmdType == "AVOID" then
    local player = be:getPlayerVehicle(0)
    if not player then
      sendResponse("AI_ERR:No player vehicle")
      return
    end

    if cmdArg == "auto" then
      avoidMode = "auto"
      player:queueLuaCommand('ai.setAvoidCars("auto")')
    elseif cmdArg == "on" then
      avoidMode = "on"
      player:queueLuaCommand('ai.setAvoidCars("on")')
    elseif cmdArg == "off" then
      avoidMode = "off"
      player:queueLuaCommand('ai.setAvoidCars("off")')
    end
    sendResponse("AI_OK:avoid cars " .. avoidMode)

  elseif cmdType == "LANE" then
    local player = be:getPlayerVehicle(0)
    if not player then
      sendResponse("AI_ERR:No player vehicle")
      return
    end

    if cmdArg == "on" then
      laneDriving = true
    elseif cmdArg == "off" then
      laneDriving = false
    end
    player:queueLuaCommand('ai.setParameters({driveInLane="' .. (laneDriving and "on" or "off") .. '"})')
    sendResponse("AI_OK:lane driving " .. (laneDriving and "on" or "off"))

  elseif cmdType == "MULTI_MODE" then
    -- Format: MULTI_MODE:mode:targetID:id1,id2,...
    -- targetID is a vehicle ID number or "none" for non-targeted modes.
    local mode, targetRaw, idList = cmdArg:match("^([^:]+):([^:]+):(.+)$")
    if not mode then
      sendResponse("AI_ERR:Invalid MULTI_MODE format")
      return
    end

    local targetID = tonumber(targetRaw)  -- nil if "none"

    local vehicleIDs = {}
    for idStr in idList:gmatch("[^,]+") do
      local vid = tonumber(idStr)
      if vid then table.insert(vehicleIDs, vid) end
    end

    if #vehicleIDs == 0 then
      sendResponse("AI_ERR:No vehicle IDs in MULTI_MODE command")
      return
    end

    local successCount = 0
    for _, vid in ipairs(vehicleIDs) do
      local v = scenetree.findObjectById(vid)
      if v then
        if targetID and (mode == "chase" or mode == "follow" or mode == "flee") then
          v:queueLuaCommand('ai.setTargetObjectID(' .. targetID .. ')')
        end
        v:queueLuaCommand('ai.setMode("' .. mode .. '")')
        v:queueLuaCommand('ai.setAggression(' .. currentAggression .. ')')
        if currentSpeedLimit then
          v:queueLuaCommand('ai.setSpeedMode("limit")')
          v:queueLuaCommand('ai.setSpeed(' .. currentSpeedLimit .. ')')
        end
        successCount = successCount + 1
      else
        aiLog('warn', "MULTI_MODE: vehicle id=" .. vid .. " not found in scene")
      end
    end

    currentMode = mode
    sendResponse("AI_OK:MULTI_MODE " .. mode .. " applied to " .. successCount .. " vehicle(s)")

  elseif cmdType == "LIGHTBAR" then
    -- Format: LIGHTBAR:<state>:<id1>,<id2>,...
    -- state: 0=off, 1=lights only, 2=lights+siren (game wraps modulo 2 or 3
    -- depending on whether the vehicle has a sirenSound).
    local stateRaw, idList = cmdArg:match("^([^:]+):(.+)$")
    local stateNum = tonumber(stateRaw)
    if not stateNum or not idList then
      sendResponse("AI_ERR:Invalid LIGHTBAR format")
      return
    end

    local count = 0
    for idStr in idList:gmatch("[^,]+") do
      local vid = tonumber(idStr)
      if vid then
        local v = scenetree.findObjectById(vid)
        if v then
          v:queueLuaCommand('electrics.set_lightbar_signal(' .. stateNum .. ')')
          count = count + 1
        end
      end
    end
    sendResponse("AI_OK:LIGHTBAR " .. stateNum .. " applied to " .. count .. " vehicle(s)")

  elseif cmdType == "STATUS_ALL" then
    -- Query every tracked vehicle's actual AI state from its own vehicle VM.
    _statusAllResults = {}
    _statusAllSlotMap = {}
    local count = 0

    for slotNum = 1, 10 do
      local vid = extensions.vehicleSlots and extensions.vehicleSlots.getSlotVehicleID(slotNum) or nil
      if vid then
        local v = scenetree.findObjectById(vid)
        if v then
          _statusAllSlotMap[vid] = slotNum
          count = count + 1
          -- Queue a query in the vehicle VM. It reads ai.getMode() and
          -- ai.getTargetObjectID(), then calls back into this GE extension.
          -- Building the GE Lua call with string.format("%q",...) in the
          -- vehicle VM auto-escapes the mode string and avoids the long-bracket
          -- concatenation that previously dropped a closing quote.
          local queryCmd = string.format(
            'local ok,m=pcall(function()return ai.getMode()end);' ..
            'if not ok or type(m)~="string" then m="unknown" end;' ..
            'local ok2,t=pcall(function()return ai.getTargetObjectID()end);' ..
            'local tn=(ok2 and tonumber(t)) or -1;' ..
            'obj:queueGameEngineLua(string.format("extensions.beamtelAI.onVehicleStatus(%%d,%%q,%%d)",%d,m,tn))',
            vid
          )
          v:queueLuaCommand(queryCmd)
        end
      end
    end

    if count == 0 then
      sendResponse("AI_STATUS_ALL:no vehicles")
    else
      _statusAllPending = count
      _statusAllTimeout = STATUS_ALL_TIMEOUT
    end

  elseif cmdType == "STATUS" then
    local speedStr = "off"
    if currentSpeedLimit then
      speedStr = tostring(math.floor(currentSpeedLimit * 3.6 + 0.5))
    end
    local status = string.format("%s,%.1f,%s,%s,%s",
      currentMode, currentAggression, speedStr, avoidMode, laneDriving and "on" or "off")
    sendResponse("AI_STATUS:" .. status)
  end
end

return M
