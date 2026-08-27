-- =================================================================================================
--
--  Vehicle-Specific Bindings for BeamNG.drive Accessibility Mod (GE Extension)
--
--  Description: Replicates the data behind the stock "Special Vehicle Keys" UI app
--               (ui/modules/apps/Keys), which is pure Vue and exposes no accessibility
--               tree. Joins core_input_actions.getActiveActions() against
--               core_input_bindings.bindings to produce, for the current vehicle, the
--               list of vehicle-specific actions that actually have a key or controller
--               button bound, and pushes it to Python over UDP.
--
--               The list is rebuilt silently on every vehicle load; Python speaks
--               nothing until the user opens the browser (F9 then B).
--
--  Loaded by: scripts/bng_screenreader_mod/modScript.lua
--
--  Version:     1.0
--  Target Game: BeamNG.drive 0.37+
--
-- =================================================================================================

local M = {}

M.dependencies = {"core_input_actions", "core_input_bindings"}

-- Configuration
local PYTHON_HOST      = "127.0.0.1"
local PYTHON_PORT_DATA = 4467   -- send binding data to Python on this port
local CMD_LISTEN_PORT  = 4468   -- receive commands from Python on this port

local REBUILD_RETRY_INTERVAL = 0.5  -- seconds between retry attempts
local MAX_REBUILD_RETRIES    = 10

-- Internal State
local udpSend        = nil
local udpCmd         = nil
local bindingCache   = nil   -- array of {action, title, line}
local cacheVehID     = nil
local cacheVehName   = ""
local pendingVehID   = nil
local retryTimer     = 0
local retryCount     = 0

local function vbLog(level, msg)
  log(level, 'VehicleBindings', msg)
end

-- =================================================================================================
--  Control name prettification
--
--  The game has no Lua-side equivalent: CONTROL_LABELS / controlLabel() live only in
--  ui/ui-vue/src/services/controls.js, and they compress for display ("PG^", "L(shift)",
--  "NP0") using glyphs a screen reader cannot read. This is the same mapping expanded
--  for speech instead.
-- =================================================================================================

local KEY_LABELS = {
  escape = "escape", enter = "enter", tab = "tab", capslock = "caps lock",
  backspace = "backspace", delete = "delete", insert = "insert",
  home = "home", ["end"] = "end", pageup = "page up", pagedown = "page down",
  lshift = "left shift", rshift = "right shift", shift = "shift",
  lcontrol = "left control", rcontrol = "right control", control = "control", ctrl = "control",
  lalt = "left alt", ralt = "right alt", alt = "alt",
  left = "left arrow", right = "right arrow", up = "up arrow", down = "down arrow",
  space = "space", grave = "grave", backslash = "backslash", slash = "slash",
  comma = "comma", period = "period", semicolon = "semicolon", apostrophe = "apostrophe",
  lbracket = "left bracket", rbracket = "right bracket",
  minus = "minus", equals = "equals",
  numpaddivide = "numpad divide", numpadmultiply = "numpad multiply",
  numpaddecimal = "numpad decimal", numpadenter = "numpad enter",
  -- axes
  xaxis = "X axis", yaxis = "Y axis", zaxis = "Z axis",
  rxaxis = "R X axis", ryaxis = "R Y axis", rzaxis = "R Z axis",
  thumblx = "left thumbstick X", thumbly = "left thumbstick Y",
  thumbrx = "right thumbstick X", thumbry = "right thumbstick Y",
  triggerl = "left trigger", triggerr = "right trigger",
  -- xinput buttons
  btn_a = "A button", btn_b = "B button", btn_x = "X button", btn_y = "Y button",
  btn_back = "back button", btn_start = "start button",
  btn_l = "left bumper", btn_r = "right bumper",
  btn_lt = "left stick button", btn_rt = "right stick button",
  upov = "d-pad up", dpov = "d-pad down", lpov = "d-pad left", rpov = "d-pad right",
}

local function controlPartToSpeech(part)
  local lbl = KEY_LABELS[part]
  if lbl then return lbl end
  local n = part:match("^numpad(%d)$")
  if n then return "numpad " .. n end
  n = part:match("^button(%d+)$")
  if n then return "button " .. n end
  n = part:match("^modifier(%d+)$")
  if n then return "modifier " .. n end
  if #part == 1 then return part:upper() end
  return part
end

-- Device families, keyed by devicetype with any trailing index stripped
-- (xinput0 -> xinput). Keyboard contributes no prefix: "L" reads better than
-- "keyboard L", and it is by far the common case.
local DEVICE_WORDS = {
  keyboard = "", mouse = "mouse", xinput = "gamepad", gamepad = "gamepad",
  sce_pad = "gamepad", ps5 = "gamepad", joystick = "joystick",
  wheel = "wheel", openxr = "VR controller", vinput = "touch",
}

-- Preference order for reading a single action's bindings aloud: the key first,
-- then the pad. (The stock DEVICE_ORDER puts keyboard last, which is backwards
-- for speech -- most users want the key and can stop listening there.)
local DEVICE_RANK = {
  keyboard = 1, mouse = 2, xinput = 3, gamepad = 3, sce_pad = 3, ps5 = 3,
  wheel = 4, joystick = 5, openxr = 6, vinput = 7,
}

local function deviceKey(devicetype, devname)
  local key = tostring(devicetype or ""):lower()
  if key == "" then key = tostring(devname or ""):lower() end
  return (key:gsub("%d+$", ""))
end

local function controlToSpeech(devKey, control)
  local norm = tostring(control or ""):lower():gsub("%-", " ")
  local parts = {}
  for p in norm:gmatch("%S+") do
    parts[#parts + 1] = controlPartToSpeech(p)
  end
  if #parts == 0 then return nil end
  -- Modifiers are space-separated prefixes on the control string ("alt d").
  local text = table.concat(parts, " plus ")
  local word = DEVICE_WORDS[devKey]
  if word == nil then word = devKey end
  if word ~= "" then text = word .. " " .. text end
  return text
end

local function translate(key)
  if not key or key == "" then return nil end
  local ok, out = pcall(function() return translateLanguage(key, key, true) end)
  if ok and out and out ~= "" then return out end
  return key
end

-- =================================================================================================
--  Binding enumeration
-- =================================================================================================

local function currentVehicleName(vehId)
  local veh = be:getObjectByID(vehId)
  if not veh then veh = be:getPlayerVehicle(0) end
  if not veh then return "Vehicle" end
  local ok, name = pcall(function() return extensions.vehicleNaming.describe(veh) end)
  if ok and name and name ~= "" then return name end
  local f = ""
  pcall(function() f = veh:getJBeamFilename() or "" end)
  return f:match("([^/\\]+)%.jbeam$") or "Vehicle"
end

-- Mirrors getVehicleSpecificActions() in lua/ge/extensions/ui/bindingsLegend.lua, but
-- keeps binding.control (which that function discards) so we can speak the key.
--
-- Filtering on cat == "vehicle_specific" is also the "can this part config do it"
-- check: core/input/actions.lua resolves usability against vdata.actionsEnabled and
-- demotes anything unusable to "vehicle_specific_unusable".
local function buildBindingList()
  local vehId = be:getPlayerVehicleID(0)
  if not vehId or vehId < 0 then
    bindingCache = {}
    cacheVehID = nil
    cacheVehName = ""
    pendingVehID = nil
    return false
  end

  local actions = nil
  local ok = pcall(function() actions = core_input_actions.getActiveActions() end)
  if not ok or type(actions) ~= 'table' then
    -- Vehicle data (vdata.inputActions) is not populated yet -- retry.
    pendingVehID = vehId
    return false
  end

  local devices = core_input_bindings and core_input_bindings.bindings
  if type(devices) ~= 'table' then
    pendingVehID = vehId
    return false
  end

  local byAction = {}
  local order = {}

  for _, device in ipairs(devices) do
    local contents = device.contents
    local devBindings = contents and contents.bindings
    if type(devBindings) == 'table' then
      local devKey = deviceKey(contents.devicetype, device.devname)
      for _, binding in ipairs(devBindings) do
        local info = binding.action and actions[binding.action]
        if info and info.cat == "vehicle_specific" and not binding.unused then
          local blocked = false
          if core_input_actionFilter and core_input_actionFilter.isActionBlocked then
            local okB, res = pcall(core_input_actionFilter.isActionBlocked, binding.action)
            blocked = okB and res or false
          end
          if not blocked then
            local entry = byAction[binding.action]
            if not entry then
              entry = {
                action = binding.action,
                title = translate(info.title) or binding.action,
                order = info.order or 999,
                controls = {},
                seen = {},
              }
              byAction[binding.action] = entry
              order[#order + 1] = entry
            end
            local text = controlToSpeech(devKey, binding.control)
            if text and not entry.seen[text] then
              entry.seen[text] = true
              entry.controls[#entry.controls + 1] = {
                text = text,
                rank = DEVICE_RANK[devKey] or 9,
              }
            end
          end
        end
      end
    end
  end

  -- Sort the controls within each action, then the actions themselves.
  for _, entry in ipairs(order) do
    table.sort(entry.controls, function(a, b)
      if a.rank ~= b.rank then return a.rank < b.rank end
      return a.text < b.text
    end)
    local texts = {}
    for _, c in ipairs(entry.controls) do texts[#texts + 1] = c.text end
    entry.line = entry.title .. ": " .. table.concat(texts, ", ")
    entry.seen = nil
  end

  table.sort(order, function(a, b)
    if a.order ~= b.order then return a.order < b.order end
    return a.title < b.title
  end)

  -- An empty result right after a vehicle switch usually means vdata is still
  -- loading rather than "this vehicle has no special controls", so keep retrying
  -- until the budget runs out before believing it.
  if #order == 0 and retryCount < MAX_REBUILD_RETRIES then
    pendingVehID = vehId
    bindingCache = {}
    cacheVehID = nil
    return false
  end

  bindingCache = order
  cacheVehID = vehId
  cacheVehName = currentVehicleName(vehId)
  pendingVehID = nil
  retryCount = 0
  vbLog('I', "Binding list ready for vehicle " .. vehId .. ": " .. #order .. " actions")
  return true
end

local function sendBindingList()
  if not udpSend then return end
  local list = bindingCache or {}
  -- Name is sanitized of ',' because the header is split on the first comma;
  -- the rows are not, since Python splits those with maxsplit 1.
  local name = tostring(cacheVehName or ""):gsub(",", " ")
  if name == "" then name = "Vehicle" end
  udpSend:send(string.format("BINDINGS_BEGIN:%d,%s", #list, name))
  for i, entry in ipairs(list) do
    udpSend:send(string.format("BINDING:%d,%s", i - 1, entry.line))
  end
  udpSend:send("BINDINGS_END")
end

local function rebuildAndSend()
  if buildBindingList() then
    sendBindingList()
  end
end

local function invalidate(vehId)
  bindingCache = {}
  cacheVehID = nil
  pendingVehID = vehId or be:getPlayerVehicleID(0)
  retryTimer = 0
  retryCount = 0
end

-- =================================================================================================
--  Action execution
-- =================================================================================================

local function handleExecute(cacheIndex)
  local list = bindingCache
  if not list or cacheIndex < 0 or cacheIndex >= #list then return end
  local entry = list[cacheIndex + 1]
  if not entry then return end

  -- triggerDownUp drives ActionMap by action name, i.e. exactly the path a real
  -- key press takes, so onDown/onUp pairs and held-state bookkeeping stay correct.
  local ok = pcall(function() core_input_actions.triggerDownUp(entry.action) end)
  if not ok then
    -- Fallback: run the action's commands directly (what ui_bindingsLegend does).
    pcall(function()
      local actions = core_input_actions.getActiveActions()
      local info = actions and actions[entry.action]
      if info then
        local vehId = be:getPlayerVehicleID(0)
        core_input_actions.executeCommand(info, 1, vehId)
        core_input_actions.executeCommand(info, 0, vehId)
      end
    end)
  end
  vbLog('I', "Executed vehicle action " .. tostring(entry.action))
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
    vbLog('I', "UDP send socket targeting " .. PYTHON_HOST .. ":" .. PYTHON_PORT_DATA)
  else
    vbLog('E', "Failed to create UDP send socket.")
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
    vbLog('I', "UDP command socket listening on port " .. CMD_LISTEN_PORT)
  else
    vbLog('E', "Failed to create UDP command socket: " .. tostring(err))
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
    vbLog('I', "UDP command socket bound on port " .. CMD_LISTEN_PORT .. " after retry.")
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
  vbLog('I', "Vehicle bindings extension loaded.")
  -- Bind sockets here so Ctrl+L Lua reload re-opens them.
  setupSockets()
end

function M.onWorldReadyState(state)
  if state == 2 then
    vbLog('I', "World ready. Initializing vehicle bindings.")
    setupSockets()
    invalidate(nil)
  end
end

-- onVehicleSwitched fires before vdata is populated, so never read here -- just
-- arm the retry, exactly as clickspotAccessible.lua does for the same reason.
function M.onVehicleSwitched(oldId, newId, player)
  if player ~= nil and player ~= 0 then return end
  invalidate(newId)
end

function M.onVehicleResetted(vehId)
  if vehId == cacheVehID then invalidate(vehId) end
end

-- Re-binding a key in Options -> Controls, or plugging in a pad, changes the
-- answer without any vehicle change.
function M.onInputBindingsChanged()
  invalidate(nil)
end

function M.onDeviceChanged()
  invalidate(nil)
end

function M.onActionFilterUpdated()
  invalidate(nil)
end

function M.onUpdate(dtReal, dtSim, dtRaw)
  retryCmdBind(dtReal)
  -- 1. Drain commands from Python. Always drained, never gated, so a REQUEST is
  --    never lost while a rebuild is pending.
  if udpCmd then
    repeat
      local data = udpCmd:receive()
      if data then
        local cmd = data:match("^%s*(.-)%s*$")
        local cmdUpper = cmd:upper()

        if cmdUpper == "REQUEST" then
          if bindingCache and cacheVehID then
            sendBindingList()
          else
            invalidate(nil)
          end
        elseif cmdUpper == "REBUILD" then
          invalidate(nil)
        elseif cmd:sub(1, 5) == "EXEC:" then
          local idx = tonumber(cmd:sub(6))
          if idx then handleExecute(idx) end
        end
      end
    until not data
  end

  -- 2. Vehicle change backstop: the hook can be missed on the initial spawn.
  local currentVehId = be:getPlayerVehicleID(0)
  if currentVehId and currentVehId >= 0
     and currentVehId ~= cacheVehID and currentVehId ~= pendingVehID then
    invalidate(currentVehId)
  end

  -- 3. Retry the build until vdata is populated.
  if pendingVehID then
    retryTimer = retryTimer + dtReal
    if retryTimer >= REBUILD_RETRY_INTERVAL then
      retryTimer = 0
      retryCount = retryCount + 1
      if retryCount > MAX_REBUILD_RETRIES then
        -- Give up: this vehicle genuinely has no bound special controls.
        bindingCache = {}
        cacheVehID = pendingVehID
        cacheVehName = currentVehicleName(pendingVehID)
        pendingVehID = nil
        retryCount = 0
        sendBindingList()
      else
        rebuildAndSend()
      end
    end
  end
end

return M
