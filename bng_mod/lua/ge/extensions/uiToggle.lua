-- =================================================================================================
--
--  UI Visibility Toggle for BeamNG.drive Accessibility Mod (GE Extension)
--
--  Description: Lets the Python side hide and show the game's UI (the same thing ALT+U toggles in
--               game, which maps to ui_visibility.toggle()). The AI Describer feature hides the UI,
--               takes a screenshot, then shows it again, so HUD/menu elements don't pollute what the
--               vision model sees.
--
--               Commands (received as plain UDP strings on CMD_LISTEN_PORT):
--                 HIDE    -> ui_visibility.set(false)
--                 SHOW    -> ui_visibility.set(true)
--                 TOGGLE  -> ui_visibility.toggle()
--
--  Loaded by:   scripts/bng_screenreader_mod/modScript.lua
--
--  Version:     1.0
--  Target Game: BeamNG.drive 0.37+
--
-- =================================================================================================

local M = {}

local CMD_LISTEN_PORT = 4464    -- receive HIDE/SHOW/TOGGLE commands from Python on this port
local udpCmd = nil

local function utLog(level, msg) log(level, 'UiToggle', msg) end

local function setupSockets()
  if udpCmd then pcall(function() udpCmd:close() end); udpCmd = nil end

  local ok, err = pcall(function()
    udpCmd = socket.udp()
    udpCmd:setsockname("127.0.0.1", CMD_LISTEN_PORT)
    udpCmd:settimeout(0)
  end)
  if ok and udpCmd then
    utLog('info', "UDP command socket listening on port " .. CMD_LISTEN_PORT)
  else
    utLog('error', "Failed to create UDP command socket: " .. tostring(err))
    udpCmd = nil
  end
end

local function applyCommand(cmd)
  if not ui_visibility then
    utLog('error', "ui_visibility module unavailable; cannot toggle UI.")
    return
  end
  if cmd == "HIDE" then
    ui_visibility.set(false)
  elseif cmd == "SHOW" then
    ui_visibility.set(true)
  elseif cmd == "TOGGLE" then
    ui_visibility.toggle()
  end
end

function M.onExtensionLoaded()
  utLog('info', "UI toggle extension loaded.")
  setupSockets()
end

function M.onWorldReadyState(state)
  if state == 2 then
    setupSockets()
  end
end

function M.onUpdate(dtReal, dtSim, dtRaw)
  if not udpCmd then return end
  local data = udpCmd:receive()
  if data then
    local cmd = data:match("^%s*(.-)%s*$"):upper()
    applyCommand(cmd)
  end
end

return M
