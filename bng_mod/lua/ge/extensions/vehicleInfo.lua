-- =================================================================================================
--
--  Vehicle Information Readout for BeamNG.drive Accessibility Mod (GE Extension)
--
--  Description: The vehicle specifications the stock vehicle selector shows on its details page,
--               as a list Python can browse on request (F9 then SPACE on the selector, `i` in the
--               mod's own spawner). Nothing here speaks on its own -- it answers when asked.
--
--               The data is NOT scraped from the UI. The details page is built GE-side by
--               ui_vehicleSelector_vehicleSpecifications.getDetails({model=, config=}), which
--               returns the whole payload already unit-converted and localized, for any
--               model/config pair. So one code path answers both callers, and the readout cannot
--               disagree with what a sighted player is looking at.
--
--  Loaded by: scripts/bng_screenreader_mod/modScript.lua
--
--  Version:     1.0
--  Target Game: BeamNG.drive 0.39+
--
-- =================================================================================================

local M = {}

local PYTHON_HOST      = "127.0.0.1"
local PYTHON_PORT_DATA = 4477   -- send info rows to Python on this port
local CMD_LISTEN_PORT  = 4478   -- receive commands from Python on this port

-- The stock selector clears nothing when it closes, so a record left standing would answer
-- confidently about the last car looked at hours ago. The route test below is the real guard;
-- this is the backstop for the case where the route read itself fails.
local RECORD_MAX_AGE_S = 600.0

-- extensions.reload builds a fresh module table for the stock selector, silently dropping our
-- wrapper. Re-checking is cheap (one table field), so it rides a slow tick rather than a hook.
local WRAP_CHECK_S = 2.0

-- Route names that mean "a vehicle selector is on screen". Substring matched against the
-- resolved route name, because the selector ships under ten route names (pause, garage,
-- mission control, and a .vehicle child of most of them) and they all mount the same view.
local SELECTOR_ROUTE_WORDS = { "vehicleselector", "garage.vehicles", "garage.mycars" }

local udpSend = nil
local udpCmd  = nil

-- What the details page is currently showing, as recorded by our wrapper.
local lastModel  = nil
local lastConfig = nil
local lastTs     = 0.0

local lastReason  = "nothing asked yet"
local wrapChecked = 0.0

local function viLog(level, msg)
  log(level, 'VehicleInfo', msg)
end

-- =================================================================================================
--  Tracking what the stock selector's details page is showing
-- =================================================================================================

-- There is no hook for this. ui_vehicleSelector_general.requestDetails ends in a
-- guihooks.trigger, which goes to the UI and never to an extension -- but it is a plain module
-- function, and EVERY focus and selection in the selector routes through it
-- (useVehicleSelector.focusItem -> lua.ui_gridSelector.requestDetails -> dispatch -> this).
-- So we wrap it. The marker makes the wrap idempotent; without it a re-arm would stack
-- wrappers on every check.
--
-- The wrapper dispatches through a MUTABLE SLOT on the stock module rather than calling our
-- recorder directly, and that indirection is not style. A Lua closure captures upvalues, so a
-- wrapper installed by one instance of THIS file writes to that instance's locals forever.
-- extensions.reload("vehicleInfo") builds a fresh instance with fresh locals while the old
-- wrapper stays installed on the stock module -- so the mark would read "installed", no
-- re-arm would fire, and every recording would land in the dead instance while the live one
-- reported "no vehicle selected on this screen yet" for the rest of the session. Owning the
-- slot is how the newest instance takes the recordings back.
-- ...and the mark is a VERSION, not a boolean, with the original function parked beside it.
-- A boolean says "some wrapper is installed", which is not the question: a wrapper left by an
-- OLDER build of this file predates the slot and calls its own captured recorder, so the slot
-- would be claimed and ignored. That is the ordinary case of updating the mod and reloading,
-- not a corner. Keeping the original lets a stale wrapper be REPLACED rather than wrapped
-- again -- re-wrapping stacks a layer per reload and every layer runs on every keystroke.
local WRAP_MARK    = "__bngScreenReaderInfoWrappedVer"
local RECORDER_KEY = "__bngScreenReaderInfoRecorder"
local ORIG_KEY     = "__bngScreenReaderInfoOrig"
local WRAP_VERSION = 2

local function recordFromItem(item)
  if type(item) ~= "table" then return end
  -- requestDetails unwraps item.showDetails itself, so accept either shape -- a tile carries
  -- the pair under showDetails, a direct details request carries it at the top level.
  local src = item
  if type(item.showDetails) == "table" then src = item.showDetails end
  local model  = src.model
  local config = src.config
  if type(model) ~= "string" or model == "" then return end
  lastModel  = model
  lastConfig = (type(config) == "string") and config or ""
  lastTs     = os.clock()
end

local function installWrapper()
  local gen = ui_vehicleSelector_general
  if type(gen) ~= "table" then return false end

  -- Claim the slot first and unconditionally: this is what a reloaded instance does to take
  -- ownership of a wrapper that is already installed and current.
  gen[RECORDER_KEY] = recordFromItem

  if gen[WRAP_MARK] == WRAP_VERSION then return true end

  -- Unwrap anything older before wrapping, so layers cannot accumulate. If no wrapper of
  -- ours is present (first install, or the stock module itself was reloaded and took our
  -- keys with it) this is just the stock function.
  local orig = gen[ORIG_KEY] or gen.requestDetails
  if type(orig) ~= "function" then return false end
  gen[ORIG_KEY] = orig

  gen.requestDetails = function(item, requestId)
    -- Record first and defensively: a throw in here would take the stock selector's own
    -- details flow down with it, which is a far worse failure than a missing readout.
    local rec = gen[RECORDER_KEY]
    if rec then pcall(rec, item) end
    return orig(item, requestId)
  end
  gen[WRAP_MARK] = WRAP_VERSION
  viLog('I', "Wrapped ui_vehicleSelector_general.requestDetails (v" .. WRAP_VERSION .. ").")
  return true
end

-- Installed AND ours. The identity test is the half that catches a reload of this file: the
-- mark alone would say yes while the recordings went to a dead instance.
local function wrapperInstalled()
  local gen = ui_vehicleSelector_general
  return type(gen) == "table"
     and gen[WRAP_MARK] == WRAP_VERSION
     and gen[RECORDER_KEY] == recordFromItem
end

-- =================================================================================================
--  Is a vehicle selector actually on screen?
-- =================================================================================================

-- Read every tick rather than hooked to a close event, the argument trailerAngle.lua already
-- makes for the coupler registry: what is wanted is not an event but a continuous fact, and the
-- selector can be left by a route push, a back, a pause dismiss or a level change -- the router
-- is the one place all of them agree.
local function currentRouteName()
  local ok, st = pcall(function() return extensions.ui_router.getState() end)
  if not ok or type(st) ~= "table" then return nil end
  local cur = st.currentRoute
  if type(cur) ~= "table" then return nil end
  -- toRoute is where the router is going and is what is on screen once the move completes;
  -- resolved is the fully-resolved node for the same move. Prefer toRoute, fall back.
  local name = (type(cur.toRoute) == "table" and cur.toRoute.name)
            or (type(cur.resolved) == "table" and cur.resolved.name)
  if type(name) ~= "string" then return nil end
  return name
end

local function selectorIsOpen()
  local name = currentRouteName()
  if not name then return false, nil end
  local lower = name:lower()
  for _, word in ipairs(SELECTOR_ROUTE_WORDS) do
    if lower:find(word, 1, true) then return true, name end
  end
  return false, name
end

-- =================================================================================================
--  Flattening getDetails into speakable rows
-- =================================================================================================

-- A spec value is NOT always a string. Power and Torque arrive as an array of segments --
-- {text = "334 bhp"}, {text = "@ 5700 - 6500 rpm", italic = true} -- because the page renders
-- the second one in italics. Joining the text fields is the whole conversion; a naive tostring
-- puts a table address into speech, and dropping the row loses the headline number on the page.
local function valueToText(v)
  if type(v) == "string" then return v end
  if type(v) == "number" then return tostring(v) end
  if type(v) ~= "table" then return nil end
  local parts = {}
  for _, seg in ipairs(v) do
    if type(seg) == "table" and type(seg.text) == "string" and seg.text ~= "" then
      parts[#parts + 1] = seg.text
    elseif type(seg) == "string" and seg ~= "" then
      parts[#parts + 1] = seg
    end
  end
  if #parts == 0 then return nil end
  return table.concat(parts, " ")
end

local function addRow(rows, kind, label, value)
  if value ~= nil and value ~= "" then
    rows[#rows + 1] = { kind = kind, label = label or "", value = value }
  elseif kind == "group" and label and label ~= "" then
    rows[#rows + 1] = { kind = kind, label = label, value = "" }
  end
end

-- Called against vehicleSpecifications.getDetails directly, never
-- ui_vehicleSelector_detailsInteraction.getDetails: that one additionally builds spawn buttons
-- and can run M.customDetailsButtons callbacks, which is a side effect a readout must not have.
local function buildRows(model, config)
  if type(model) ~= "string" or model == "" then
    return nil, "no model given", "nomodel"
  end

  pcall(function() extensions.load("ui_vehicleSelector_vehicleSpecifications") end)
  local specs = ui_vehicleSelector_vehicleSpecifications
  if type(specs) ~= "table" or type(specs.getDetails) ~= "function" then
    return nil, "vehicle specifications module unavailable", "nomodule"
  end

  -- An empty config means the model's default, which the model record calls default_pc.
  local cfg = config
  if type(cfg) ~= "string" or cfg == "" then
    cfg = nil
    pcall(function()
      local m = core_vehicles.getModel(model)
      if type(m) == "table" and type(m.model) == "table" then
        cfg = m.model.default_pc
      end
    end)
    if type(cfg) ~= "string" then cfg = "" end
  end

  local ok, det = pcall(function() return specs.getDetails({ model = model, config = cfg }) end)
  if not ok or type(det) ~= "table" then
    return nil, "no such vehicle or configuration", "nomodel"
  end

  local rows = {}

  addRow(rows, "head", "", det.headerTitle)
  addRow(rows, "head", "Brand", det.brand)

  local cd = det.configDetails
  if type(cd) == "table" then
    addRow(rows, "spec", "Description", cd.Description)
  end

  -- The summary strip at the top of the page. Deliberately kept even though several of its
  -- entries repeat inside specificationsList below: the page shows both, and this is the half
  -- a reader most often wants first.
  if type(det.generalSpecs) == "table" and #det.generalSpecs > 0 then
    addRow(rows, "group", "Summary", nil)
    for _, s in ipairs(det.generalSpecs) do
      if type(s) == "table" then
        addRow(rows, "spec", s.key, valueToText(s.value))
      end
    end
  end

  -- The icon strip. Its labels are already full sentences ("Drivetrain: All Wheel Drive"),
  -- which is exactly why it is worth reading: the icons carry no text on screen at all.
  if type(det.iconTags) == "table" and #det.iconTags > 0 then
    addRow(rows, "group", "Features", nil)
    for _, t in ipairs(det.iconTags) do
      if type(t) == "table" and type(t.label) == "string" then
        addRow(rows, "spec", "", t.label)
      end
    end
  end

  if type(det.specificationsList) == "table" then
    for _, grp in ipairs(det.specificationsList) do
      if type(grp) == "table" then
        addRow(rows, "group", grp.label, nil)
        if type(grp.specifications) == "table" then
          for _, s in ipairs(grp.specifications) do
            if type(s) == "table" then
              addRow(rows, "spec", s.key, valueToText(s.value))
            end
          end
        end
      end
    end
  end

  if type(det.tags) == "table" and #det.tags > 0 then
    addRow(rows, "group", "Source", nil)
    for _, t in ipairs(det.tags) do
      if type(t) == "table" and type(t.label) == "string" then
        addRow(rows, "spec", "", t.label)
      end
    end
  end

  if #rows == 0 then
    return nil, "no specifications for this vehicle", "norows"
  end
  return rows, nil, nil
end

-- =================================================================================================
--  Transmission (chunked JSON)
-- =================================================================================================

-- JSON rather than a positional tail for the reason environmentAccessible.lua records:
-- bng_mod/ is a live junction into the game install, so the two halves genuinely do go out of
-- step, and this project has already paid for positional tails twice. It also sidesteps
-- sanitising separators out of free text -- a config Description contains commas and quotes.
-- A failure carries a machine-readable CODE as well as its sentence, and the distinction is
-- load-bearing rather than tidiness: "not on a selector screen" is the answer on every screen
-- in the game, including the one where F9 SPACE means "scan the terrain". Python has to be able
-- to fall through on that one silently while still speaking the others, and it cannot tell them
-- apart from prose. Every other code is a real refusal the user asked for and wants to hear.
local function sendFail(code, reason)
  lastReason = code .. ": " .. reason
  if udpSend then
    pcall(function() udpSend:send("INFOFAIL:" .. code .. ";" .. tostring(reason)) end)
  end
end

local function sendRows(rows)
  if not udpSend then return end
  udpSend:send(string.format("INFO_BEGIN:%d", #rows))
  for i, r in ipairs(rows) do
    local ok, encoded = pcall(jsonEncode, {
      i     = i - 1,
      kind  = r.kind,
      label = r.label,
      value = r.value,
    })
    if ok and encoded then
      udpSend:send("INFO_ROW:" .. encoded)
    else
      viLog('W', "Failed to encode info row " .. tostring(i))
    end
  end
  udpSend:send("INFO_END")
end

local function answerFor(model, config, how)
  local rows, err, errCode = buildRows(model, config)
  if not rows then
    sendFail(errCode or "nomodel", err or "could not build specifications")
    return
  end
  lastReason = string.format("%s: %s / %s, %d rows", how, tostring(model),
    (config ~= nil and config ~= "") and config or "default", #rows)
  sendRows(rows)
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
    viLog('I', "UDP send socket targeting " .. PYTHON_HOST .. ":" .. PYTHON_PORT_DATA)
  else
    viLog('E', "Failed to create UDP send socket.")
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
    viLog('I', "UDP command socket listening on port " .. CMD_LISTEN_PORT)
  else
    viLog('E', "Failed to create UDP command socket: " .. tostring(err))
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
    viLog('I', "UDP command socket bound on port " .. CMD_LISTEN_PORT .. " after retry.")
  end
end

-- setupSockets closes the sockets held by THIS module instance, and extensions.reload builds a
-- fresh instance whose locals are nil -- so it closes nothing and the outgoing instance keeps
-- the port, leaving the reloaded copy permanently deaf. Hence this hook.
function M.onExtensionUnloaded()
  if udpSend then pcall(function() udpSend:close() end); udpSend = nil end
  if udpCmd  then pcall(function() udpCmd:close()  end); udpCmd  = nil end
  -- Drop the recorder if it is still ours, so an unloaded mod leaves the stock selector
  -- calling nothing rather than calling into a dead instance. The wrapper itself stays: a
  -- reload is about to re-claim the slot, and restoring requestDetails here would race a
  -- newer instance that has already wrapped.
  local gen = ui_vehicleSelector_general
  if type(gen) == "table" and gen[RECORDER_KEY] == recordFromItem then
    gen[RECORDER_KEY] = nil
  end
end

-- =================================================================================================
--  Commands
-- =================================================================================================

-- "There is no information" has half a dozen causes that sound identical from the seat and want
-- different actions -- not on the selector at all, on it but nothing focused yet, the specs
-- module missing, a bad model key. Each is named, the same argument rampGeometry.shortStateOf
-- makes for its own failure line.
local function handleSelectorRequest()
  local open = selectorIsOpen()
  if not open then
    sendFail("notselector", "not on a vehicle selector screen")
    return
  end
  if not wrapperInstalled() then
    -- Arm it now rather than reporting a fault: the user is on the selector, and the very next
    -- tile they move to will record.
    installWrapper()
    sendFail("nofocus", "selector just opened, move to a vehicle first")
    return
  end
  if not lastModel or (os.clock() - lastTs) > RECORD_MAX_AGE_S then
    sendFail("nofocus", "no vehicle selected on this screen yet")
    return
  end
  answerFor(lastModel, lastConfig, "selector")
end

local function handleCommand(cmd)
  local upper = cmd:upper()
  if upper == "INFO_SELECTOR" then
    handleSelectorRequest()
  elseif cmd:sub(1, 5) == "INFO:" then
    local body = cmd:sub(6)
    local model, config = body:match("^([^,]*),?(.*)$")
    answerFor(model, config, "explicit")
  elseif upper == "DIAG" then
    M.diag()
  end
end

-- =================================================================================================
--  Lifecycle
-- =================================================================================================

function M.onExtensionLoaded()
  setExtensionUnloadMode(M, "manual")
  viLog('I', "Vehicle info extension loaded.")
  setupSockets()
  installWrapper()
end

function M.onWorldReadyState(state)
  if state == 2 then
    setupSockets()
    installWrapper()
  end
end

function M.onUpdate(dtReal, dtSim, dtRaw)
  retryCmdBind(dtReal)

  -- Re-arm the wrapper. extensions.reload of the stock selector replaces its module table and
  -- drops ours with no error anywhere; the only symptom would be a readout that answers about
  -- the wrong car, weeks later. Same class of silent loss M.onExtensionUnloaded exists for.
  wrapChecked = wrapChecked + (dtReal or 0)
  if wrapChecked >= WRAP_CHECK_S then
    wrapChecked = 0
    if not wrapperInstalled() then pcall(installWrapper) end
  end

  if udpCmd then
    repeat
      local data = udpCmd:receive()
      if data then
        handleCommand(data:match("^%s*(.-)%s*$"))
      end
    until not data
  end
end

-- =================================================================================================
--  Diagnostics
-- =================================================================================================

-- This resolve's failure mode is a confident answer about the wrong vehicle, which is invisible
-- to every kind of inspection except printing what it chose.
function M.diag()
  local open, routeName = selectorIsOpen()
  viLog('I', "---- vehicleInfo diag ----")
  viLog('I', "  wrapper installed: " .. tostring(wrapperInstalled()))
  viLog('I', "  route: " .. tostring(routeName) .. "  (selector open: " .. tostring(open) .. ")")
  viLog('I', "  last recorded: " .. tostring(lastModel) .. " / " ..
    ((lastConfig ~= nil and lastConfig ~= "") and lastConfig or "<default>") ..
    string.format("  (%.1f s ago)", lastModel and (os.clock() - lastTs) or -1))
  viLog('I', "  cmd socket bound: " .. tostring(udpCmd ~= nil) .. " on " .. CMD_LISTEN_PORT)
  viLog('I', "  last outcome: " .. tostring(lastReason))
  return {
    wrapped = wrapperInstalled(),
    route   = routeName,
    open    = open,
    model   = lastModel,
    config  = lastConfig,
    ageS    = lastModel and (os.clock() - lastTs) or nil,
    bound   = udpCmd ~= nil,
    reason  = lastReason,
  }
end

-- Exposed so diagnostic/vehicle_info_sim.py can drive the real flattener rather than a copy:
-- this area's failure mode is a readout that is merely wrong rather than broken, so a sim
-- carrying its own copy would keep passing across exactly the edit that breaks the mod.
M.valueToText    = valueToText
M.buildRows      = buildRows
M.selectorIsOpen = selectorIsOpen

return M
