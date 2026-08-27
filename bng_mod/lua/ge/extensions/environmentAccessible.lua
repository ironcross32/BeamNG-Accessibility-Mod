-- =================================================================================================
--
--  Accessible Environment Controls for BeamNG.drive Accessibility Mod (GE Extension)
--
--  Description: The environment values the stock pause UI does not expose at all, surfaced as a
--               list Python can read and edit (F9 then N). Today that is the ambient
--               temperature; the row protocol is named-field rather than positional so adding
--               the next one is data, not plumbing.
--
--               Temperature is the reason this file exists. It is NOT a settable value in
--               BeamNG: core_environment.onUpdate interpolates the level's temperature CURVE
--               against the time of day every frame and pushes the result into the engine with
--               be:setSeaLevelTemperatureK. core_environment exports getTemperatureK but its
--               setter is a file-local, so writing a temperature directly is overwritten on the
--               very next frame. The only durable way in is to change the curve the loop reads.
--
--  Loaded by: scripts/bng_screenreader_mod/modScript.lua
--
--  Version:     1.0
--  Target Game: BeamNG.drive 0.39+
--
-- =================================================================================================

local M = {}

M.dependencies = {"core_environment"}

local PYTHON_HOST      = "127.0.0.1"
local PYTHON_PORT_DATA = 4474   -- send environment rows to Python on this port
local CMD_LISTEN_PORT  = 4475   -- receive commands from Python on this port

-- A temperature set takes a frame to show up: core_environment.onInit() refreshes the cached
-- curve, but the value itself is only recomputed by the next onUpdate. Re-sending the state
-- immediately would report the value we just replaced, so the resend is deferred by a tick.
local RESEND_DELAY_S = 0.1

-- The band offered to the browser. Wider than any stock level's curve (which run roughly -10
-- to 40) because the point of the control is to reach conditions the level does not ship with;
-- clamped at all only so a slipped keypress cannot ask for something the thermal model has
-- never been near.
local TEMP_MIN_C = -60.0
local TEMP_MAX_C = 60.0

local udpSend = nil
local udpCmd  = nil

-- The level's own curve, captured before this extension has ever written one, so RESTORE puts
-- back what the level author wrote rather than a flat guess. Captured per level and cleared on
-- level change: a curve from the last map is not a default for this one.
local originalCurve = nil
local originalLevel = nil
local resendTimer   = nil

local function envLog(level, msg)
  log(level, 'EnvAccessible', msg)
end

-- =================================================================================================
--  Level info access
-- =================================================================================================

-- Named "theLevelInfo" by the engine on every level; found by class rather than by that name so
-- a level that registers it differently still resolves.
local function levelInfoObject()
  local ok, obj = pcall(function()
    local ids = scenetree.findClassObjects("LevelInfo")
    if not ids or #ids == 0 then return nil end
    return scenetree.findObject(ids[1])
  end)
  if ok then return obj end
  return nil
end

local function currentLevelName()
  local ok, name = pcall(function()
    return getCurrentLevelIdentifier and getCurrentLevelIdentifier() or nil
  end)
  if ok and name then return tostring(name) end
  return "level"
end

-- The curve is a list of {timeOfDay, degreesC} pairs. A copy is taken rather than the engine's
-- table being held: it is rebuilt on every getTemperatureCurveC call, and holding one would
-- make "the original" whatever the last read happened to be.
local function copyCurve(curve)
  if type(curve) ~= "table" then return nil end
  local out = {}
  for i, pair in ipairs(curve) do
    if type(pair) == "table" and pair[1] and pair[2] then
      out[i] = {pair[1], pair[2]}
    end
  end
  if #out == 0 then return nil end
  return out
end

local function curveRangeC(curve)
  if not curve or #curve == 0 then return nil, nil end
  local lo, hi = curve[1][2], curve[1][2]
  for _, pair in ipairs(curve) do
    if pair[2] < lo then lo = pair[2] end
    if pair[2] > hi then hi = pair[2] end
  end
  return lo, hi
end

local function captureOriginalCurve()
  local level = currentLevelName()
  if originalCurve and originalLevel == level then return end
  local li = levelInfoObject()
  if not li then return end
  local ok, curve = pcall(function() return li:getTemperatureCurveC() end)
  if not ok then return end
  originalCurve = copyCurve(curve)
  originalLevel = level
  if originalCurve then
    local lo, hi = curveRangeC(originalCurve)
    envLog('I', string.format(
      "captured level default temperature curve for %s: %d points, %.1f to %.1f C",
      level, #originalCurve, lo or 0, hi or 0))
  end
end

-- =================================================================================================
--  Wire format
--
--  Rows are named fields separated by ';' rather than positional CSV. bng_mod/ is a live
--  junction into the game install, so the two halves of this mod genuinely do go out of step,
--  and this project has already paid for positional tails twice (the extended telemetry struct
--  and the DOCK: line). A field added on one side is ignored by the other here instead of
--  shifting everything after it.
-- =================================================================================================

local function sanitize(s)
  -- ';' and '=' are the field separators and a newline would split the datagram; nothing else
  -- needs escaping, because Python splits fields on ';' and each field on the FIRST '='.
  return (tostring(s or ""):gsub("[;=\r\n]", " "))
end

local function row(idx, fields)
  local parts = {string.format("ENV:%d", idx)}
  for _, kv in ipairs(fields) do
    parts[#parts + 1] = kv[1] .. "=" .. sanitize(kv[2])
  end
  return table.concat(parts, ";")
end

local function sendState()
  if not udpSend then return end

  local li = levelInfoObject()
  if not li then
    -- No level loaded (main menu, or between maps). Said plainly rather than answered with an
    -- empty list, which reads from the seat as "the feature is broken".
    udpSend:send("ENV_UNAVAILABLE:no level loaded")
    return
  end

  captureOriginalCurve()

  local canChange = true
  pcall(function() canChange = core_environment.canChange() end)

  local liveC = 15.0
  pcall(function() liveC = core_environment.getTemperatureK() - 273.15 end)

  local curve = nil
  pcall(function() curve = copyCurve(li:getTemperatureCurveC()) end)
  local curveLo, curveHi = curveRangeC(curve)
  local origLo, origHi = curveRangeC(originalCurve)

  local rows = {}

  rows[#rows + 1] = row(0, {
    {"key", "temperature"},
    {"kind", "numberC"},
    {"value", string.format("%.2f", liveC)},
    {"min", string.format("%.1f", TEMP_MIN_C)},
    {"max", string.format("%.1f", TEMP_MAX_C)},
    {"step", "1"},
    {"label", "Temperature"},
    -- The live figure is a point on a curve. When the curve is not flat the number the browser
    -- shows is only true for the current time of day, and editing it FLATTENS the day's cycle
    -- -- so the row has to be able to say so rather than silently discarding the level's own
    -- weather.
    {"curveLo", string.format("%.2f", curveLo or liveC)},
    {"curveHi", string.format("%.2f", curveHi or liveC)},
    {"editable", canChange and "1" or "0"},
  })

  rows[#rows + 1] = row(1, {
    {"key", "restore"},
    {"kind", "action"},
    {"label", "Restore level default"},
    {"value", origLo and string.format("%.2f", origLo) or ""},
    {"curveLo", origLo and string.format("%.2f", origLo) or ""},
    {"curveHi", origHi and string.format("%.2f", origHi) or ""},
    {"editable", (canChange and originalCurve) and "1" or "0"},
  })

  udpSend:send(string.format("ENV_BEGIN:%d;level=%s;canChange=%s",
    #rows, sanitize(currentLevelName()), canChange and "1" or "0"))
  for _, line in ipairs(rows) do
    udpSend:send(line)
  end
  udpSend:send("ENV_END")
end

-- =================================================================================================
--  Applying a change
-- =================================================================================================

-- setEditorDirty() looks like the cheap refresh -- core_environment.onUpdate re-reads the curve
-- when the LevelInfo reports dirty -- but it is a no-op outside the editor, verified in game:
-- the flag reads back false and the temperature never moves. core_environment.onInit() (which
-- is its `reset` under the hood) re-reads the cached curve unconditionally, and is exported.
local function applyCurve(curve)
  local li = levelInfoObject()
  if not li then return false, "no level loaded" end

  local canChange = true
  pcall(function() canChange = core_environment.canChange() end)
  -- Missions and scenarios switch environment changes off. Refused rather than written,
  -- because the write would appear to succeed and be reverted by whatever holds the lock.
  if not canChange then return false, "this scenario does not allow environment changes" end

  local ok, err = pcall(function() li:setTemperatureCurveC(curve) end)
  if not ok then return false, tostring(err) end
  pcall(function() core_environment.onInit() end)
  resendTimer = RESEND_DELAY_S
  return true, nil
end

local function setFlatTemperature(celsius)
  if celsius < TEMP_MIN_C then celsius = TEMP_MIN_C end
  if celsius > TEMP_MAX_C then celsius = TEMP_MAX_C end
  -- Two points, not one: core_environment.onUpdate returns early on a curve with fewer than
  -- two points, so a single-point curve would leave the temperature frozen at whatever it was
  -- -- a set that reports success and does nothing.
  return applyCurve({{0, celsius}, {1, celsius}})
end

local function restoreOriginal()
  if not originalCurve then return false, "no level default captured" end
  return applyCurve(originalCurve)
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
    envLog('I', "UDP send socket targeting " .. PYTHON_HOST .. ":" .. PYTHON_PORT_DATA)
  else
    envLog('E', "Failed to create UDP send socket.")
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
    envLog('I', "UDP command socket listening on port " .. CMD_LISTEN_PORT)
  else
    envLog('E', "Failed to create UDP command socket: " .. tostring(err))
    if udpCmd then pcall(function() udpCmd:close() end) end
    udpCmd = nil
  end
end

-- A failed bind is otherwise permanent for the session, so re-arm it. This is the recovery
-- path, not a precaution: a Lua reload leaks the outgoing instance's port until its module
-- table is collected, and without this the extension stays deaf until the game is restarted.
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
    envLog('I', "UDP command socket bound on port " .. CMD_LISTEN_PORT .. " after retry.")
  end
end

-- setupSockets closes the sockets held by THIS module instance, and extensions.reload builds a
-- fresh instance whose locals are nil -- so it closes nothing and the outgoing instance keeps
-- the port, leaving the reloaded copy permanently deaf. Hence this hook.
function M.onExtensionUnloaded()
  if udpSend then pcall(function() udpSend:close() end); udpSend = nil end
  if udpCmd  then pcall(function() udpCmd:close()  end); udpCmd  = nil end
end

-- =================================================================================================
--  Lifecycle
-- =================================================================================================

function M.onExtensionLoaded()
  setExtensionUnloadMode(M, "manual")
  envLog('I', "Accessible environment extension loaded.")
  setupSockets()
end

function M.onWorldReadyState(state)
  if state == 2 then
    envLog('I', "World ready. Initializing accessible environment controls.")
    setupSockets()
    originalCurve = nil
    originalLevel = nil
    captureOriginalCurve()
  end
end

-- The level's own curve is a fact about the level, so both of these drop it. Capturing on
-- start rather than lazily is what makes RESTORE able to undo the FIRST edit of a session.
function M.onClientStartMission()
  originalCurve = nil
  originalLevel = nil
  captureOriginalCurve()
end

function M.onClientEndMission()
  originalCurve = nil
  originalLevel = nil
end

function M.onUpdate(dtReal, dtSim, dtRaw)
  retryCmdBind(dtReal)

  if udpCmd then
    repeat
      local data = udpCmd:receive()
      if data then
        local cmd = data:match("^%s*(.-)%s*$")
        local upper = cmd:upper()

        if upper == "REQUEST" then
          sendState()
        elseif cmd:sub(1, 4) == "SET:" then
          local body = cmd:sub(5)
          local key, value = body:match("^([^=]+)=(.+)$")
          if key == "temperature" and tonumber(value) then
            local okSet, err = setFlatTemperature(tonumber(value))
            if not okSet then
              if udpSend then udpSend:send("ENV_ERROR:" .. sanitize(err)) end
              envLog('W', "temperature set refused: " .. tostring(err))
            end
          end
        elseif upper == "RESTORE" then
          local okRestore, err = restoreOriginal()
          if not okRestore then
            if udpSend then udpSend:send("ENV_ERROR:" .. sanitize(err)) end
          end
        end
      end
    until not data
  end

  -- Deferred resend: see RESEND_DELAY_S.
  if resendTimer then
    resendTimer = resendTimer - (dtReal or 0)
    if resendTimer <= 0 then
      resendTimer = nil
      sendState()
    end
  end
end

return M
