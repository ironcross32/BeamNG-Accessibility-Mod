-- =================================================================================================
--
--  Accessible Console for BeamNG.drive Accessibility Mod (GE Extension)
--
--  Description: A screen-reader-accessible equivalent of the in-game developer console (the one
--               opened with the backtick '`' key). That stock console is rendered with Dear ImGui,
--               which exposes no accessibility tree, so it is invisible to NVDA. This extension
--               re-implements the console's execution model and exposes it over UDP to the Python
--               backend (beamtel.py), which presents it through native, screen-reader-friendly
--               wxPython controls on its Main tab.
--
--               It mirrors the stock console (lua/ge/extensions/ui/console.lua):
--                 * Execution "contexts" (sandboxes): GE - Lua, GE - TorqueScript, CEF/UI - JS,
--                   and one per active vehicle.
--                 * GE - Lua is run through executeLuaSandboxed() (lua/common/devUtils.lua) so the
--                   return value AND captured print() output come straight back to the caller.
--                 * Other contexts are queued; their output surfaces through the global frame log.
--                 * The global engine log (Engine.getFrameLog()) can be streamed on request so the
--                   user can review output from any "section".
--
--  Loaded by: scripts/bng_screenreader_mod/modScript.lua
--
--  Version:     1.0
--  Target Game: BeamNG.drive 0.37+
--
-- =================================================================================================

local M = {}

local socket = require("socket")

-- Configuration ----------------------------------------------------------------------------------
local PYTHON_HOST     = "127.0.0.1"
local RESP_PORT       = 4466   -- send responses / context list / log stream to Python here
local CMD_LISTEN_PORT = 4465   -- receive EXEC / CTXLIST / LOGON / LOGOFF commands from Python here

local MAX_LOG_LINES_PER_FRAME = 250   -- cap forwarded log lines per frame to avoid UDP flooding
local MAX_DATAGRAM_CONTENT    = 1400  -- max payload bytes per UDP datagram (well under the MTU)
local MAX_OUTPUT_DATAGRAMS    = 400   -- safety cap on datagrams emitted for a single command's output
local MAX_SHORT_FIELD         = 1400  -- truncate short structured fields (labels, log lines) to one datagram

-- Internal state ---------------------------------------------------------------------------------
local udpSend     = nil
local udpCmd      = nil
local logStreaming = false
local vehIds       = {}   -- ordered list of vehicle ids; vehicle context index N maps to vehIds[N-2]
local vehAccum     = nil  -- reassembly buffer for the current vehicle command's chunked result

-- Logging helper
local function caLog(level, msg)
  log(level, 'ConsoleAccessible', msg)
end

-- =================================================================================================
--  UDP helpers
-- =================================================================================================

local function sendLine(line)
  if not udpSend then return end
  -- Final safety clamp; callers are expected to keep payloads within MAX_DATAGRAM_CONTENT.
  if #line > MAX_DATAGRAM_CONTENT + 200 then
    line = line:sub(1, MAX_DATAGRAM_CONTENT + 200)
  end
  pcall(function() udpSend:send(line) end)
end

local function stringify(v)
  if type(v) == "string" then return v end
  local ok, d = pcall(dumps, v)
  return ok and d or tostring(v)
end

-- Short structured field (context label, log time/level/origin/msg): collapse to a single
-- datagram-sized line. The protocol keeps each record's content as its LAST '|'-segment and
-- rejoins it on the Python side, so embedded '|' characters are safe and need no escaping.
local function field(v)
  local s = stringify(v):gsub("[\r\n]+", " ")
  if #s > MAX_SHORT_FIELD then
    s = s:sub(1, MAX_SHORT_FIELD) .. " ...(truncated)"
  end
  return s
end

-- Emit arbitrarily large / multi-line content as a series of datagrams, each carrying the
-- given prefix (e.g. "OUT|"). Long text is split into <= MAX_DATAGRAM_CONTENT chunks, preferring
-- to break on newline boundaries so lines stay intact. `budget` is a shared {n, truncated} table
-- enforcing a per-command cap so a runaway dump cannot flood the socket.
local function emitChunks(prefix, text, budget)
  text = stringify(text)
  local pos, len = 1, #text
  if len == 0 then
    if budget.n > 0 then sendLine(prefix); budget.n = budget.n - 1 end
    return
  end
  while pos <= len do
    if budget.n <= 0 then
      if not budget.truncated then
        sendLine(prefix .. "...(output truncated; raise MAX_OUTPUT_DATAGRAMS)...")
        budget.truncated = true
      end
      return
    end
    local stop = math.min(pos + MAX_DATAGRAM_CONTENT - 1, len)
    if stop < len then
      -- Back up to the last newline within this chunk, if any, for cleaner line breaks.
      local lastNl, searchFrom = nil, pos
      while true do
        local nl = text:find("\n", searchFrom, true)
        if not nl or nl > stop then break end
        lastNl = nl
        searchFrom = nl + 1
      end
      if lastNl and lastNl >= pos then stop = lastNl end
    end
    sendLine(prefix .. text:sub(pos, stop))
    budget.n = budget.n - 1
    pos = stop + 1
  end
end

local function newBudget()
  return { n = MAX_OUTPUT_DATAGRAMS, truncated = false }
end

-- =================================================================================================
--  Context (sandbox) enumeration -- mirrors refreshCombo()/vehStrInfo() in ui/console.lua
-- =================================================================================================

local function vehStrInfo(vobj)
  local strI = tostring(vobj:getId())
  local okName, jbeam = pcall(function() return vobj:getJBeamFilename() end)
  if okName and jbeam then strI = strI .. ", " .. tostring(jbeam) end
  return strI
end

-- Rebuild vehIds and return the ordered list of {index, label} contexts.
local function buildContexts()
  local contexts = {
    { index = 0, label = "GE - Lua" },
    { index = 1, label = "GE - TorqueScript" },
    { index = 2, label = "CEF/UI - JS" },
  }
  vehIds = {}

  local okCount, vehCount = pcall(function() return be:getObjectCount() end)
  if not okCount or not vehCount then vehCount = 0 end

  if vehCount > 0 then
    local playerVehicle = getPlayerVehicle(0)
    if playerVehicle and playerVehicle:getActive() then
      table.insert(vehIds, be:getPlayerVehicleID(0))
      table.insert(contexts, { index = #vehIds + 2,
        label = "Vehicle (current) " .. vehStrInfo(playerVehicle) })
    end
    for i = 0, vehCount - 1 do
      local v = be:getObject(i)
      if v and v:getActive() then
        -- Skip the player vehicle if it was already added first.
        local vid = v:getID()
        local already = false
        for _, existing in ipairs(vehIds) do
          if existing == vid then already = true; break end
        end
        if not already then
          table.insert(vehIds, vid)
          table.insert(contexts, { index = #vehIds + 2, label = "Vehicle " .. vehStrInfo(v) })
        end
      end
    end
  end

  return contexts
end

local function sendContextList()
  local contexts = buildContexts()
  for _, ctx in ipairs(contexts) do
    sendLine("CTX|" .. ctx.index .. "|" .. field(ctx.label))
  end
  sendLine("CTXEND")
end

-- =================================================================================================
--  Command execution -- mirrors the dispatch block in ui/console.lua (lines ~994-1062)
-- =================================================================================================

-- Vehicle-VM wrapper, built by concatenation around string.format("%q", cmd). It must NOT be
-- assembled with :format() because the body itself contains literal %q calls. The captured
-- return value and print() output are streamed back to GE in small bounded chunks via
-- queueGameEngineLua -- a single cross-VM command has a length cap, so a large serialized
-- payload would be silently truncated and corrupt the callback. onVehBegin/onVehData/onVehEnd
-- reassemble the pieces on the GE side.
local VEH_WRAP_PRE = [==[
local __cap = {}
local __oldprint = print
print = function(...)
  local n = select('#', ...)
  local t = {}
  for i = 1, n do t[i] = tostring(select(i, ...)) end
  __cap[#__cap + 1] = table.concat(t, '\t')
end
local __src = ]==]

local VEH_WRAP_POST = [==[

-- Try as an expression first (so a bare 'electrics.values' returns its value), then as a
-- statement chunk -- mirrors the stock console's "return " prefix behaviour.
local __f, __e = load('return ' .. __src, 'AccessibleConsoleVeh', 't')
if not __f then __f, __e = load(__src, 'AccessibleConsoleVeh', 't') end
local __ok, __res
if __f then __ok, __res = pcall(__f) else __ok, __res = false, __e end
print = __oldprint
local __resStr
if __ok then
  if __res ~= nil then
    local __okd, __rs = pcall(dumps, __res)
    __resStr = __okd and __rs or tostring(__res)
  else
    __resStr = ''
  end
else
  __resStr = tostring(__res)
end
local __outStr = table.concat(__cap, '\n')
local __CH = 700
local function __ge(s) obj:queueGameEngineLua(s) end
__ge('extensions.consoleAccessible.onVehBegin(' .. tostring(__ok) .. ')')
local function __stream(kind, s)
  local p, n = 1, #s
  while p <= n do
    local e = math.min(p + __CH - 1, n)
    __ge('extensions.consoleAccessible.onVehData(' .. string.format('%q', kind) .. ',' .. string.format('%q', s:sub(p, e)) .. ')')
    p = e + 1
  end
end
__stream('res', __resStr)
__stream('out', __outStr)
__ge('extensions.consoleAccessible.onVehEnd()')
]==]

local function executeCommand(ctxIndex, cmd)
  if ctxIndex == 0 then
    -- GE - Lua, sandboxed so we capture the return value and print() output directly.
    local okExec, res, out = pcall(executeLuaSandboxed, cmd, 'AccessibleConsole')
    if not okExec then
      sendLine("RESP|error|" .. field(res))
      sendLine("EXECEND")
      return
    end
    local budget = newBudget()
    if type(res) == "string" and res:sub(1, 7) == "Error: " then
      sendLine("RESP|error|" .. field(res))
    else
      sendLine("RESP|ok")
      if res ~= nil then emitChunks("OUT|= ", stringify(res), budget) end
    end
    if out and #out > 0 then
      for _, line in ipairs(out) do
        emitChunks("OUT|", line, budget)
      end
    end
    sendLine("EXECEND")

  elseif ctxIndex == 1 then
    -- GE - TorqueScript
    if not cmd:find(";[\n\r\t ]-$") then cmd = cmd .. ";" end
    local ok, err = pcall(function() TorqueScript.eval(cmd) end)
    sendLine(ok and "RESP|queued|" or ("RESP|error|" .. field(err)))
    sendLine("EXECEND")

  elseif ctxIndex == 2 then
    -- CEF/UI - JavaScript
    local ok, err = pcall(function() be:queueJS(cmd) end)
    sendLine(ok and "RESP|queued|" or ("RESP|error|" .. field(err)))
    sendLine("EXECEND")

  else
    -- Vehicle Lua: context index N (>=3) maps to vehIds[N-2]. The vehicle runs in a
    -- separate VM, so we wrap the command to capture its print()/return output and ship
    -- it back to GE in chunks via obj:queueGameEngineLua -> onVehBegin/onVehData/onVehEnd.
    local vid = vehIds[ctxIndex - 2]
    if not vid then
      sendLine("RESP|error|Selected vehicle invalid; refresh contexts")
      sendLine("EXECEND")
      sendContextList()
      return
    end
    local v = getObjectByID(vid)
    if not v then
      sendLine("RESP|error|Selected vehicle no longer exists")
      sendLine("EXECEND")
      sendContextList()
      return
    end
    -- %q renders the user command as a safely-escaped Lua string literal for load().
    vehAccum = nil  -- discard any stale partial result from a previous command
    local wrapped = VEH_WRAP_PRE .. string.format("%q", cmd) .. VEH_WRAP_POST
    local ok, err = pcall(function() v:queueLuaCommand(wrapped) end)
    if not ok then
      sendLine("RESP|error|" .. field(err))
      sendLine("EXECEND")
    end
    -- On success, RESP/OUT/EXECEND are emitted later from onVehBegin/onVehData/onVehEnd.
  end
end

-- Vehicle-context results arrive in three phases from the vehicle VM (via queueGameEngineLua),
-- streamed in small bounded chunks to survive the cross-VM command length cap. We reassemble the
-- 'res' and 'out' streams here, then emit them over UDP once complete.
function M.onVehBegin(ok)
  vehAccum = { ok = ok and true or false, res = {}, out = {} }
end

function M.onVehData(kind, chunk)
  if not vehAccum then vehAccum = { ok = true, res = {}, out = {} } end
  local t = vehAccum[kind]
  if t then t[#t + 1] = chunk end
end

function M.onVehEnd()
  local acc = vehAccum
  vehAccum = nil
  if not acc then
    sendLine("RESP|error|vehicle result missing")
    sendLine("EXECEND")
    return
  end
  local budget = newBudget()
  local resStr = table.concat(acc.res)
  if acc.ok then
    sendLine("RESP|ok")
    if #resStr > 0 then emitChunks("OUT|= ", resStr, budget) end
  else
    sendLine("RESP|error|" .. field(resStr ~= "" and resStr or "unknown error"))
  end
  local outStr = table.concat(acc.out)
  if #outStr > 0 then
    for line in (outStr .. "\n"):gmatch("(.-)\n") do
      emitChunks("OUT|", line, budget)
    end
  end
  sendLine("EXECEND")
end

-- =================================================================================================
--  Global log streaming -- same source the stock console reads (Engine.getFrameLog())
-- =================================================================================================

local function streamFrameLog()
  if not logStreaming then return end
  local okLog, flog = pcall(function() return Engine.getFrameLog() end)
  if not okLog or not flog then return end
  local count = 0
  for _, l in ipairs(flog) do
    if count >= MAX_LOG_LINES_PER_FRAME then
      sendLine("LOG||I|ConsoleAccessible|...log truncated this frame...")
      break
    end
    -- l = { time, level, origin, message }
    sendLine("LOG|" .. field(l[1]) .. "|" .. field(l[2]) .. "|" .. field(l[3]) .. "|" .. field(l[4]))
    count = count + 1
  end
end

-- =================================================================================================
--  Command handling
-- =================================================================================================

local function handleCommand(data)
  -- EXEC carries arbitrary user text (which may contain '|'), so split off only the
  -- command keyword and context index, keeping the remainder verbatim as the Lua/JS/TS source.
  local ctxIndex, cmd = data:match("^EXEC|(%-?%d+)|(.*)$")
  if ctxIndex then
    executeCommand(tonumber(ctxIndex), cmd)
    return
  end

  local keyword = data:match("^(%u+)")
  if keyword == "CTXLIST" then
    sendContextList()
  elseif keyword == "LOGON" then
    logStreaming = true
    caLog('info', "Log streaming enabled.")
    sendLine("RESP|ok|log streaming on")
    sendLine("EXECEND")
  elseif keyword == "LOGOFF" then
    logStreaming = false
    caLog('info', "Log streaming disabled.")
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
    udpSend:setpeername(PYTHON_HOST, RESP_PORT)
    udpSend:settimeout(0)
    caLog('info', "UDP send socket created, targeting " .. PYTHON_HOST .. ":" .. RESP_PORT)
  else
    caLog('error', "Failed to create UDP send socket.")
  end

  local ok, err = pcall(function()
    udpCmd = socket.udp()
    udpCmd:setsockname("127.0.0.1", CMD_LISTEN_PORT)
    udpCmd:settimeout(0)
  end)
  if ok and udpCmd then
    caLog('info', "UDP command socket listening on port " .. CMD_LISTEN_PORT)
  else
    caLog('error', "Failed to create UDP command socket: " .. tostring(err))
    udpCmd = nil
  end
end

function M.onExtensionLoaded()
  caLog('info', "Accessible console extension loaded.")
  -- Bind sockets here so a Ctrl+L Lua reload re-opens them.
  setupSockets()
end

function M.onWorldReadyState(state)
  if state == 2 then
    caLog('info', "World ready. Re-binding accessible console sockets.")
    setupSockets()
  end
end

function M.onUpdate(dtReal, dtSim, dtRaw)
  -- Drain all pending commands this frame (there may be several queued up).
  if udpCmd then
    for _ = 1, 16 do
      local data = udpCmd:receive()
      if not data then break end
      local ok, err = pcall(handleCommand, data)
      if not ok then caLog('error', "Command handling failed: " .. tostring(err)) end
    end
  end

  streamFrameLog()
end

-- Keep the context list fresh on the Python side as vehicles come and go.
M.onVehicleDestroyed     = sendContextList
M.onVehicleSwitched      = sendContextList
M.onVehicleSpawned       = sendContextList
M.onVehicleActiveChanged = sendContextList

return M
