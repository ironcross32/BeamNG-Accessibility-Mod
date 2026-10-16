-- =================================================================================================
--
--  Accessible Audio Debug for BeamNG.drive Accessibility Mod (GE Extension)
--
--  Description: The World Editor's Window > Audio tools, as lists Python renders in a virtual
--               browser (F9 then J). Those windows are ImGui and have no accessibility tree, so
--               this reads and writes what is behind them instead of the windows themselves:
--
--                 * Engine Audio Debug -- the live parameters of every engine sound on the player
--                   vehicle (gain, muffling, load gains, shelves, EQ, fundamental). The values
--                   live in the vehicle VM; beamtelAudioDebug.lua there reads and writes them and
--                   answers through onVehicleData below.
--                 * SFX Previewer -- every sound event in the game, as folders, played on Enter.
--
--               The Suspension Audio Debug window is deliberately not covered: it is a read-only
--               set of graphs with nothing to act on.
--
--               None of this needs the editor open. Sim.getSFXTrackSet() and Engine.Audio.playOnce
--               both work with it closed (verified in 0.39).
--
--  Loaded by: scripts/bng_screenreader_mod/modScript.lua
--
--  Version:     1.0
--  Target Game: BeamNG.drive 0.39+
--
-- =================================================================================================

local M = {}

local PYTHON_HOST      = "127.0.0.1"
local PYTHON_PORT_DATA = 4483   -- send rows to Python on this port
local CMD_LISTEN_PORT  = 4484   -- receive commands from Python on this port

-- How long to wait for the vehicle VM before answering anyway. A vehicle that never answers
-- (its VM is reloading, or an older copy of the vehicle half) would otherwise leave the F9 key
-- looking dead.
local VEHICLE_TIMEOUT_S = 1.0

local udpSend = nil
local udpCmd  = nil

local sounds       = {}   -- last report from the vehicle VM
local selectedId   = nil  -- sound id ("device:ref") the browser is editing
local pendingTimer = nil  -- counting down while a vehicle request is outstanding
local vehicleNote  = nil  -- why there are no sounds, when there are none

local sfxTree      = nil  -- root folder, rebuilt on every root listing
local sfxById      = {}   -- track-set index -> event name, from the same build
local sfxPlayingId = nil

local function audLog(level, msg)
  log(level, 'AudioDebugAccessible', msg)
end

-- =================================================================================================
--  Wire format: named fields, as environmentAccessible. See docs/lua-audio-debug.md.
-- =================================================================================================

local function sanitize(s)
  return (tostring(s or ""):gsub("[;=\r\n]", " "))
end

local function fmtNum(v)
  if type(v) ~= "number" then return "" end
  if math.abs(v) < 0.00005 then return "0" end
  local s = string.format("%.4f", v):gsub("0+$", ""):gsub("%.$", "")
  return s
end

local function line(prefix, idx, fields)
  local parts = {string.format("%s:%d", prefix, idx)}
  for _, kv in ipairs(fields) do
    parts[#parts + 1] = kv[1] .. "=" .. sanitize(kv[2])
  end
  return table.concat(parts, ";")
end

local function send(text)
  if udpSend then udpSend:send(text) end
end

-- =================================================================================================
--  FMOD frequency scale. Engine sound EQ frequencies are stored on FMOD's 0-1000 scale, a
--  piecewise-linear map over these Hz points (100 units per segment). Same table and inverse as
--  the stock editor window and sounds.hzToFMODHz.
-- =================================================================================================

local FMOD_HZ = {20.0, 40.0, 80.0, 160.0, 330.0, 660.0, 1300.0, 2700.0, 5400.0, 11000.0, 22000.0}

local function fmodToHz(f)
  f = math.max(0, math.min(f, 1000))
  local seg = math.min(math.floor(f / 100), 9)
  local frac = (f - seg * 100) / 100
  return FMOD_HZ[seg + 1] + frac * (FMOD_HZ[seg + 2] - FMOD_HZ[seg + 1])
end
M.fmodToHz = fmodToHz

-- =================================================================================================
--  Engine sound rows
-- =================================================================================================

-- What each control is, and the step the browser moves it by. Ranges are the stock window's.
-- Frequencies step on the FMOD scale: 10 units is a tenth of a band (roughly a tenth of an
-- octave), and shift makes it a whole band.
local CONTROLS = {
  {key = "mainGain",          label = "Main gain",          unit = "dB",    min = -30, max = 10, step = 0.5},
  {key = "muffling",          label = "Muffling",           unit = "ratio", min = 0,   max = 1,  step = 0.01},
  {key = "onLoadGain",        label = "On-load gain",       unit = "ratio", min = 0,   max = 1,  step = 0.01},
  {key = "offLoadGain",       label = "Off-load gain",      unit = "ratio", min = 0,   max = 1,  step = 0.01},
  {key = "lowShelfFreq",      label = "Low shelf frequency",  evLabel = "Low cut frequency",  unit = "Hz", min = 0, max = 1000, step = 10},
  {key = "lowShelfGain",      label = "Low shelf gain",     unit = "dB",    min = -30, max = 30, step = 0.5},
  {key = "highShelfFreq",     label = "High shelf frequency", evLabel = "High cut frequency", unit = "Hz", min = 0, max = 1000, step = 10},
  {key = "highShelfGain",     label = "High shelf gain",    unit = "dB",    min = -30, max = 30, step = 0.5},
  {key = "eqLowFreq",         label = "Low EQ frequency",   unit = "Hz",    min = 0,   max = 1000, step = 10},
  {key = "eqLowGain",         label = "Low EQ gain",        unit = "dB",    min = -30, max = 30, step = 0.5},
  {key = "eqLowWidth",        label = "Low EQ width",       unit = "ratio", min = 0,   max = 1,  step = 0.01},
  {key = "eqHighFreq",        label = "High EQ frequency",  unit = "Hz",    min = 0,   max = 1000, step = 10},
  {key = "eqHighGain",        label = "High EQ gain",       unit = "dB",    min = -30, max = 30, step = 0.5},
  {key = "eqHighWidth",       label = "High EQ width",      unit = "ratio", min = 0,   max = 1,  step = 0.01},
  {key = "eqFundamentalGain", label = "Fundamental gain",   unit = "dB",    min = -30, max = 30, step = 0.5},
  {key = "cylinders",         label = "Fundamental cylinder count", unit = "count", min = 0, max = 32, step = 1},
}

local function selectedSound()
  for i, s in ipairs(sounds) do
    if s.id == selectedId then return s, i end
  end
  return nil, nil
end

local function isElectric(s)
  return s.deviceType == "electricMotor"
end

local function sendState()
  local rows = {}
  local s, index = selectedSound()

  if not s then
    rows[#rows + 1] = line("AUD", 0, {
      {"key", "none"}, {"kind", "info"},
      {"label", vehicleNote or "No engine sounds on this vehicle"},
    })
  else
    rows[#rows + 1] = line("AUD", #rows, {
      {"key", "sound"}, {"kind", "select"},
      {"index", index - 1}, {"count", #sounds},
      {"ref", s.ref}, {"device", s.device}, {"blend", s.blend},
      {"deviceType", s.deviceType},
      {"modified", s.modified and "1" or "0"},
    })
    local v = s.values or {}
    for _, c in ipairs(CONTROLS) do
      local value = v[c.key]
      if type(value) == "number" then
        local fields = {
          {"key", c.key},
          {"kind", "number"},
          {"label", (isElectric(s) and c.evLabel) or c.label},
          {"unit", c.unit},
          {"value", fmtNum(value)},
          {"min", fmtNum(c.min)}, {"max", fmtNum(c.max)}, {"step", fmtNum(c.step)},
        }
        if c.unit == "Hz" then fields[#fields + 1] = {"hz", fmtNum(fmodToHz(value))} end
        rows[#rows + 1] = line("AUD", #rows, fields)
      end
    end
    rows[#rows + 1] = line("AUD", #rows, {{"key", "copy"}, {"kind", "action"}, {"label", "Copy as jbeam"}})
    local anyModified = false
    for _, o in ipairs(sounds) do if o.modified then anyModified = true end end
    rows[#rows + 1] = line("AUD", #rows, {
      {"key", "restore"}, {"kind", "action"}, {"label", "Restore vehicle defaults"},
      {"editable", anyModified and "1" or "0"},
    })
  end
  rows[#rows + 1] = line("AUD", #rows, {{"key", "library"}, {"kind", "action"}, {"label", "Sound library"}})

  send(string.format("AUD_BEGIN:%d;soundCount=%d", #rows, #sounds))
  for _, r in ipairs(rows) do send(r) end
  send("AUD_END")
end

-- One-line calls only: a cross-VM chunk may carry no percent sign, and these never do -- the
-- format below runs HERE, and its arguments are an id, a key and a number.
local function askVehicle(call)
  local veh = getPlayerVehicle(0)
  if not veh then
    sounds = {}
    vehicleNote = "No vehicle"
    sendState()
    return
  end
  pendingTimer = VEHICLE_TIMEOUT_S
  veh:queueLuaCommand('extensions.load("beamtelAudioDebug") if extensions.beamtelAudioDebug then extensions.beamtelAudioDebug.'
    .. call .. ' end')
end

-- Called from the vehicle VM. Every request is answered here, including a refused SET, so this
-- is the single place a reply turns into rows.
function M.onVehicleData(payload)
  if type(payload) ~= "table" then return end
  local veh = getPlayerVehicle(0)
  if veh and payload.vehId and payload.vehId ~= veh:getID() then return end
  pendingTimer = nil
  sounds = payload.sounds or {}
  vehicleNote = nil
  if not selectedSound() then
    selectedId = sounds[1] and sounds[1].id or nil
  end
  if payload.error then send("AUD_ERROR:" .. sanitize(payload.error)) end
  sendState()
end

-- The stock window's "Copy to Clipboard", but in jbeam's own key names, so it can be pasted into
-- a soundConfig block. The stock one exports "muffling", which no jbeam reads; the engine's key
-- is intakeMuffling, and the exhaust's muffling is derived from the fitted exhaust part, so it has
-- no key to export at all.
local function copyJbeam()
  local s = selectedSound()
  if not s then send("AUD_ERROR:No engine sound to copy"); return end
  local v = s.values or {}
  local out = {}
  local function put(name, value, isFreq)
    if type(value) ~= "number" then return end
    if isFreq then
      out[name] = math.floor(fmodToHz(value) + 0.5)
    else
      out[name] = math.floor(value * 1000 + 0.5) / 1000
    end
  end
  local electric = isElectric(s)
  local mainGain = v.mainGain
  if s.ref == "exhaust" and type(mainGain) == "number" and s.exhaustPartGain then
    mainGain = mainGain - s.exhaustPartGain
  end
  put("mainGain", mainGain)
  if s.ref == "engine" then put("intakeMuffling", v.muffling) end
  put("onLoadGain", v.onLoadGain)
  put("offLoadGain", v.offLoadGain)
  if electric then
    put("lowCutFreq", v.lowShelfFreq, true)
    put("highCutFreq", v.highShelfFreq, true)
  else
    put("lowShelfFreq", v.lowShelfFreq, true)
    put("lowShelfGain", v.lowShelfGain)
    put("highShelfFreq", v.highShelfFreq, true)
    put("highShelfGain", v.highShelfGain)
  end
  put("eqLowFreq", v.eqLowFreq, true)
  put("eqLowGain", v.eqLowGain)
  put("eqLowWidth", v.eqLowWidth)
  put("eqHighFreq", v.eqHighFreq, true)
  put("eqHighGain", v.eqHighGain)
  put("eqHighWidth", v.eqHighWidth)
  put("eqFundamentalGain", v.eqFundamentalGain)
  if v.cylinders then out.fundamentalFrequencyCylinderCount = v.cylinders end

  local text = jsonEncodePretty(out)
  -- Outer braces dropped, as the stock window does, so the fields paste into an existing block.
  text = text:gsub("^%s*{%s*\n?", ""):gsub("\n?%s*}%s*$", "")
  local count = 0
  for _ in pairs(out) do count = count + 1 end
  -- ImGui's clipboard first. The global setClipboard the stock window uses returned false and
  -- left the clipboard untouched when measured with the game in the background -- which is
  -- exactly where it is when the user alt-tabs to paste -- while this one wrote it.
  local ok = pcall(function() ui_imgui.SetClipboardText(text) end)
  if not ok then
    local okSet, wrote = pcall(setClipboard, text)
    ok = okSet and wrote ~= false
  end
  if ok then
    send("AUD_DONE:" .. sanitize(string.format("Copied %d %s settings", count, s.ref)))
  else
    send("AUD_ERROR:Could not reach the clipboard")
  end
end

-- =================================================================================================
--  Sound library (the SFX Previewer)
-- =================================================================================================

local function newDir(name)
  return {kind = "dir", name = name, byName = {}, children = {}, count = 0}
end

local function sortDir(dir)
  local list = {}
  for _, child in pairs(dir.byName) do list[#list + 1] = child end
  table.sort(list, function(a, b)
    if a.kind ~= b.kind then return a.kind == "dir" end
    return a.name:lower() < b.name:lower()
  end)
  dir.children = list
  dir.byName = nil
  for _, child in ipairs(list) do
    if child.kind == "dir" then
      sortDir(child)
      dir.count = dir.count + child.count
    else
      dir.count = dir.count + 1
    end
  end
end

-- Same source as the stock previewer, and its name rule: the object's name when it is an FMOD
-- event, else its sound filename. That filename is often an FMOD event path too (measured: a
-- few dozen, e.g. "event:>UI>Generic>Select"), so the split is chosen by the resolved name's
-- form, not by which accessor produced it -- otherwise those land at the root as one long name.
-- The "event:" prefix is dropped so the root reads as the categories. A name seen twice is
-- listed once.
local function buildSfxTree()
  local root = newDir("")
  local byId, seen = {}, {}
  local set = Sim.getSFXTrackSet()
  local n = set and set:getCount() or 0
  for i = 0, n - 1 do
    local o = Sim.upcast(set:getObject(i))
    local name = o and o:getName()
    if not (name and string.startswith(name, "event:>")) then
      name = (o and o.getSoundFilename) and o:getSoundFilename() or nil
    end
    local parts = {}
    if name and string.startswith(name, "event:>") then
      for p in name:sub(8):gmatch("[^>]+") do parts[#parts + 1] = p end
    elseif name then
      for p in name:gmatch("[^/]+") do parts[#parts + 1] = p end
    end
    if name and #parts > 0 and not seen[name] then
      seen[name] = true
      local dir = root
      for j = 1, #parts - 1 do
        local sub = dir.byName[parts[j]]
        if not sub or sub.kind ~= "dir" then
          sub = newDir(parts[j])
          dir.byName[parts[j]] = sub
        end
        dir = sub
      end
      local leaf = parts[#parts]
      -- Two events can share a leaf name in one folder only if the names differ in a part we
      -- split away; keep both rather than silently dropping one.
      local slot = leaf
      if dir.byName[slot] then slot = leaf .. "#" .. i end
      dir.byName[slot] = {kind = "event", name = leaf, id = i}
      byId[i] = name
    end
  end
  sortDir(root)
  sfxTree, sfxById = root, byId
end

-- path is a ">"-separated list of 0-based child indices, "" for the root. Indices rather than
-- names because names go through sanitize on the way out and would not round-trip.
local function sendSfxList(path)
  if path == "" or not sfxTree then buildSfxTree() end
  local dir, names = sfxTree, {}
  for part in path:gmatch("[^>]+") do
    local child = dir.children[(tonumber(part) or -1) + 1]
    if not child or child.kind ~= "dir" then
      send("AUD_ERROR:That folder no longer exists")
      return
    end
    dir = child
    names[#names + 1] = child.name
  end
  send(string.format("AUD_SFX_BEGIN:%d;path=%s;total=%d", #dir.children,
    sanitize(table.concat(names, " > ")), dir.count))
  for i, child in ipairs(dir.children) do
    if child.kind == "dir" then
      send(line("AUD_SFX", i - 1, {{"kind", "dir"}, {"name", child.name}, {"count", child.count}}))
    else
      send(line("AUD_SFX", i - 1, {{"kind", "event"}, {"name", child.name}, {"id", child.id}}))
    end
  end
  send("AUD_SFX_END")
end

local function stopSfx()
  if sfxPlayingId then
    local o = scenetree.findObjectById(sfxPlayingId)
    if o then
      pcall(function() o:stop(-1) end)
      pcall(function() o:delete() end)
    end
    sfxPlayingId = nil
  end
end

-- Played at the camera rather than at the vehicle as the stock previewer does: the camera is the
-- listener, so the sound arrives centred and at full level instead of panned and attenuated by
-- wherever the car happens to be relative to the view.
local function playSfx(id)
  local name = sfxById[id]
  if not name then send("AUD_ERROR:That sound no longer exists"); return end
  stopSfx()
  local pos = vec3(0, 0, 0)
  pcall(function() pos = core_camera.getPosition() end)
  local ok, src = pcall(function() return Engine.Audio.playOnce('AudioGui', name, {position = pos}) end)
  if ok and src then
    sfxPlayingId = src.sourceId
  else
    audLog('W', "playOnce failed for " .. tostring(name))
    send("AUD_ERROR:Could not play that sound")
  end
end

-- =================================================================================================
--  Sockets
-- =================================================================================================

local function setupSockets()
  if udpSend then pcall(function() udpSend:close() end); udpSend = nil end
  if udpCmd  then pcall(function() udpCmd:close()  end); udpCmd  = nil end

  udpSend = socket.udp()
  if udpSend then
    udpSend:setpeername(PYTHON_HOST, PYTHON_PORT_DATA)
    udpSend:settimeout(0)
  else
    audLog('E', "Failed to create UDP send socket.")
  end

  local ok, err = pcall(function()
    udpCmd = socket.udp()
    -- setsockname RETURNS nil plus a message; it does not THROW. See environmentAccessible.
    local bound, berr = udpCmd:setsockname("127.0.0.1", CMD_LISTEN_PORT)
    if not bound then error(tostring(berr), 0) end
    udpCmd:settimeout(0)
  end)
  if ok and udpCmd then
    audLog('I', "UDP command socket listening on port " .. CMD_LISTEN_PORT)
  else
    audLog('E', "Failed to create UDP command socket: " .. tostring(err))
    if udpCmd then pcall(function() udpCmd:close() end) end
    udpCmd = nil
  end
end

-- A failed bind is otherwise permanent for the session; a Lua reload leaks the outgoing
-- instance's port until its module table is collected.
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
    audLog('I', "UDP command socket bound on port " .. CMD_LISTEN_PORT .. " after retry.")
  end
end

function M.onExtensionUnloaded()
  stopSfx()
  if udpSend then pcall(function() udpSend:close() end); udpSend = nil end
  if udpCmd  then pcall(function() udpCmd:close()  end); udpCmd  = nil end
end

-- =================================================================================================
--  Lifecycle
-- =================================================================================================

function M.onExtensionLoaded()
  setExtensionUnloadMode(M, "manual")
  setupSockets()
end

function M.onVehicleSwitched()
  selectedId = nil
end

-- A level can bring its own sound events; the next root listing rebuilds the tree anyway, and
-- this drops the old one so a stale sub-folder listing cannot outlive the level.
function M.onClientStartMission()
  stopSfx()
  sfxTree, sfxById = nil, {}
end

local function handle(cmd)
  local upper = cmd:upper()
  if upper == "REQUEST" then
    askVehicle("report()")
  elseif cmd:sub(1, 7) == "SELECT:" then
    local i = tonumber(cmd:sub(8))
    if i and sounds[i + 1] then selectedId = sounds[i + 1].id end
    sendState()
  elseif cmd:sub(1, 4) == "SET:" then
    local key, value = cmd:sub(5):match("^([%w_]+)=(.+)$")
    value = tonumber(value)
    if not (key and value) then return end
    if not selectedId then send("AUD_ERROR:No engine sound selected"); return end
    askVehicle(string.format("set(%q,%q,%s)", selectedId, key, string.format("%.6g", value)))
  elseif upper == "RESTORE" then
    askVehicle("restore()")
  elseif upper == "COPY" then
    copyJbeam()
  elseif cmd:sub(1, 9) == "SFX_LIST:" then
    sendSfxList(cmd:sub(10))
  elseif cmd:sub(1, 9) == "SFX_PLAY:" then
    local id = tonumber(cmd:sub(10))
    if id then playSfx(id) end
  elseif upper == "SFX_STOP" then
    stopSfx()
  end
end

function M.onUpdate(dtReal, dtSim, dtRaw)
  retryCmdBind(dtReal)

  if udpCmd then
    repeat
      local data = udpCmd:receive()
      if data then
        local ok, err = pcall(handle, data:match("^%s*(.-)%s*$"))
        if not ok then
          audLog('E', "command failed: " .. tostring(err))
          send("AUD_ERROR:" .. sanitize(err))
        end
      end
    until not data
  end

  if pendingTimer then
    pendingTimer = pendingTimer - (dtReal or 0)
    if pendingTimer <= 0 then
      pendingTimer = nil
      sounds = {}
      vehicleNote = "The vehicle did not answer"
      sendState()
    end
  end
end

return M
