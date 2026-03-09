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

-- Logging helper
local function aiLog(level, msg)
  log(level, 'BeamtelAI', msg)
end

-- Send a response string to Python on the scanner data port
local function sendResponse(msg)
  if udpSend then
    udpSend:send(msg)
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

function M.onExtensionLoaded()
  aiLog('info', "AI control extension loaded.")
end

function M.onWorldReadyState(state)
  aiLog('info', "onWorldReadyState triggered with state: " .. tostring(state))

  if state == 2 then
    aiLog('info', "World is ready. Initializing AI control systems.")

    -- Create send socket for responses
    udpSend = socket.udp()
    if udpSend then
      udpSend:setpeername(PYTHON_HOST, PYTHON_PORT)
      udpSend:settimeout(0)
      aiLog('info', "UDP send socket created, targeting " .. PYTHON_HOST .. ":" .. PYTHON_PORT)
    else
      aiLog('error', "Failed to create UDP send socket.")
    end

    -- Create receive socket for commands from Python
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
end

function M.onUpdate(dtReal, dtSim, dtRaw)
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
