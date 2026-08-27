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
    -- setsockname RETURNS nil plus a message; it does not THROW. A pcall around it reports
    -- success on a socket bound to nothing, and the extension then goes deaf with nothing in
    -- the log -- it still sends normally, because a UDP sender needs no bind.
    local bound, berr = udpCmd:setsockname("127.0.0.1", CMD_LISTEN_PORT)
    if not bound then error(tostring(berr), 0) end
    udpCmd:settimeout(0)
  end)
  if ok and udpCmd then
    utLog('info', "UDP command socket listening on port " .. CMD_LISTEN_PORT)
  else
    utLog('error', "Failed to create UDP command socket: " .. tostring(err))
    if udpCmd then pcall(function() udpCmd:close() end) end
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
    utLog('info', "UDP command socket bound on port " .. CMD_LISTEN_PORT .. " after retry.")
  end
end

-- setupSockets closes the sockets held by THIS module instance, and extensions.reload builds a
-- fresh instance whose locals are nil -- so it closes nothing and the outgoing instance keeps
-- the port, leaving the reloaded copy permanently deaf. Hence this hook.
function M.onExtensionUnloaded()
  if udpCmd  then pcall(function() udpCmd:close()  end); udpCmd  = nil end
end

function M.onExtensionLoaded()
  setExtensionUnloadMode(M, "manual")
  utLog('info', "UI toggle extension loaded.")
  setupSockets()
end

function M.onWorldReadyState(state)
  if state == 2 then
    setupSockets()
  end
end

function M.onUpdate(dtReal, dtSim, dtRaw)
  retryCmdBind(dtReal)
  if not udpCmd then return end
  local data = udpCmd:receive()
  if data then
    local cmd = data:match("^%s*(.-)%s*$"):upper()
    applyCommand(cmd)
  end
end

return M
