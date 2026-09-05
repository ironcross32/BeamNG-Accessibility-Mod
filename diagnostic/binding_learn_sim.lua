-- Replay bindingLearn.lua against a stubbed input system.
--
--     lua diagnostic/binding_learn_sim.lua
--
-- This drives the REAL module -- the wrapper, the rewrite, the watchdog and the axis gating are
-- the shipped code, not a copy -- against a fake `core_input_actions` / `core_input_bindings`
-- built to the shape the game's own files actually have. The re-push stub deliberately does what
-- sendBindingsToGE does (bindings.lua:171): call `core_input_actions.actionToCommands(action)`
-- THROUGH the module table and keep the command strings. That is the whole point; a stub that
-- called a captured reference would exercise nothing.
--
-- Why a sim at all: every failure this file guards against is a plausible wrong answer rather
-- than a crash. A wrapper that also rewrote `actionMap` would break menu gating game-wide and
-- look fine here; an exemption that silently stopped matching would suppress the pause key and
-- strand somebody in a menu; a watchdog that did not fire would leave the game unplayable after
-- beamtel died. None of those show up as an error anywhere. So every scenario also asserts what
-- the naive form answers, and no check can pass for free.
--
-- Tuning constants are parsed out of the source rather than copied, so retuning there cannot
-- silently invalidate these checks.

local SRC    = "bng_mod/lua/ge/extensions/bindingLearn.lua"
local VB_SRC = "bng_mod/lua/ge/extensions/vehicleBindings.lua"
local GEO_SIM = "diagnostic/vehicle_geometry_sim.lua"
local PY_SRC = "beamtel.py"

local function readAll(path)
  local fh = assert(io.open(path, "r"), "cannot open " .. path)
  local body = fh:read("*a")
  fh:close()
  return body
end

local function readConstFrom(path, name)
  local body = readAll(path)
  local val = body:match("\nlocal " .. name .. "%s*=%s*([%-%d%.]+)")
  assert(val, "could not find " .. name .. " in " .. path)
  return tonumber(val)
end

local function readConst(name) return readConstFrom(SRC, name) end

local HEARTBEAT_TIMEOUT_S = readConst("HEARTBEAT_TIMEOUT_S")
local AXIS_MIN_DEFLECTION = readConst("AXIS_MIN_DEFLECTION")
local AXIS_COOLDOWN_S     = readConst("AXIS_COOLDOWN_S")
local BUTTON_REPEAT_S     = readConst("BUTTON_REPEAT_S")
local WRAP_CHECK_S        = readConst("WRAP_CHECK_S")
local MAX_SESSION_S       = readConst("MAX_SESSION_S")
local HIT_COALESCE_S      = readConst("HIT_COALESCE_S")

local SRC_BODY = readAll(SRC)

local failures = {}
local function check(label, ok, detail)
  print(string.format("   %s: %s%s", label, ok and "OK" or "FAIL",
    detail and (" - " .. detail) or ""))
  if not ok then failures[#failures + 1] = label end
end

-- =================================================================================================
--  Stubbed engine globals
-- =================================================================================================

COMMAND_CONTEXT_TLUA = 1
COMMAND_CONTEXT_VLUA = 2
COMMAND_CONTEXT_ELUA = 3

local logLines = {}
function log(level, tag, msg) logLines[#logLines + 1] = level .. " " .. tag .. " " .. msg end
function setExtensionUnloadMode(_, _) end

-- One press is reported as a GROUP -- a control plus every action it fired -- so the encoder has
-- to reach one level of nesting. Keys are still sorted, so a row assertion can match a literal.
local function encodeValue(v)
  if type(v) == "number" then return tostring(v) end
  if type(v) == "boolean" then return tostring(v) end
  if type(v) ~= "table" then return '"' .. tostring(v):gsub('"', '\\"') .. '"' end
  if #v > 0 then
    local out = {}
    for _, item in ipairs(v) do out[#out + 1] = encodeValue(item) end
    return "[" .. table.concat(out, ",") .. "]"
  end
  local parts = {}
  for k, val in pairs(v) do parts[#parts + 1] = '"' .. k .. '":' .. encodeValue(val) end
  table.sort(parts)
  return "{" .. table.concat(parts, ",") .. "}"
end

function jsonEncode(t) return encodeValue(t) end

-- --- sockets ---------------------------------------------------------------------------------
local sent    = {}    -- everything the module has sent to Python
local inbox   = {}    -- commands waiting to be received

local function makeSock()
  local s = {}
  function s:setpeername() return 1 end
  function s:settimeout() return 1 end
  function s:close() return 1 end
  function s:setsockname() return 1 end
  function s:send(msg) sent[#sent + 1] = msg; return 1 end
  function s:receive() return table.remove(inbox, 1) end
  return s
end
socket = { udp = makeSock }

-- --- the action catalogue ---------------------------------------------------------------------
-- Shapes copied from the real actions/*.json. The commands are the real ones, because scenario 5
-- asserts they are GONE from the push, and a placeholder would make that assertion vacuous.
local ACTIONS = {
  parkingbrake_toggle = {
    cat = "vehicle", ctx = "vlua",
    onDown = "input.toggleEvent('parkingbrake')",
    title = "ui.inputActions.vehicle.parkingbrake_toggle.title",
    desc  = "ui.inputActions.vehicle.parkingbrake_toggle.description",
  },
  shiftUp = {
    cat = "vehicle", ctx = "vlua",
    onDown = "controller.mainController.shiftUp()",
    onUp   = "if controller.mainController.shiftUpOnUp then controller.mainController.shiftUpOnUp() end",
    title  = "ui.inputActions.vehicle.shiftUp.title",
  },
  steering = {
    cat = "vehicle", ctx = "vlua", isCentered = true,
    onChange = "input.event('steering', VALUE, FILTERTYPE, ANGLE, LOCKTYPE, OSCLOCKHP)",
    title = "ui.inputActions.vehicle.steering.title",
  },
  -- The trap door: this is the button that OPENS the pause menu, and it is cat "menu".
  toggleMenues = {
    cat = "menu", ctx = "tlua", actionMap = "First",
    onDown = "guihooks.trigger('UINavigation','menu',VALUE)",
    onUp   = "guihooks.trigger('UINavigation','menu',VALUE)",
    title  = "ui.inputActions.controllerui.menu.title",
  },
  menu_item_focus_ud = {
    cat = "menu", ctx = "tlua", actionMap = "MenuIndependent", isCentered = true,
    onChange = "guihooks.trigger('UINavigation','focus_ud',VALUE)",
    title = "ui.inputActions.controllerui.focus_ud.title",
  },
  beamtel_accessibility_activate = {
    cat = "accessibility", ctx = "tlua", actionMap = "Normal",
    onDown = "extensions.accessibilityInput.trigger('activate')",
    title = "Accessibility activate",
  },
  -- The pad's modifier is not device state the way a keyboard's ctrl is -- it is an ACTION whose
  -- onChange is the only thing that enables the modifier engine-side. Suppress it and every
  -- "modifier1 ..." binding in the game stops matching. Shape copied from actions/general.json.
  customModifier1 = {
    cat = "modifier", ctx = "tlua", actionMap = "Modifier",
    onChange = "core_input_bindings.enableCustomModifier(PLAYER, VALUE ~= 0, 1)",
    title = "ui.inputActions.general.customModifier1.title",
  },
  -- Bound TWICE on the same device, with and without the modifier -- the shape that made the
  -- modifier flaky, because pairs order decided which of the two got spoken.
  activateStarterMotor = {
    cat = "vehicle", ctx = "vlua",
    onDown = "controller.mainController.setStarter(true)",
    title = "ui.inputActions.vehicle.activateStarterMotor.title",
  },
  -- Bound only bare, on a control that has no modified binding at all: what the engine falls
  -- through to when a modifier is held over it.
  signal_right = {
    cat = "vehicle", ctx = "vlua",
    onDown = "electrics.toggle_signal_right()",
    title = "ui.inputActions.vehicle.signal_right.title",
  },
  -- A second action sharing btn_a with shiftUp: one press, two answers.
  triggerAction0 = {
    cat = "vehicle", ctx = "vlua",
    onDown = "controller.mainController.triggerAction(0)",
    title = "ui.inputActions.vehicle.triggerAction0.title",
  },
  look_back = {
    cat = "camera", ctx = "tlua",
    onChange = "if core_camera then core_camera.setLookBack(PLAYER, VALUE >= 0.5) end",
    title = "ui.inputActions.camera.look_back.title",
  },
}

-- The name a command string must never be built from.
local EVIL = "evil');os.exit(--"
ACTIONS[EVIL] = { cat = "vehicle", ctx = "tlua", onDown = "print('hi')", title = "Evil" }

-- The stock actionToCommands, reduced to the parts that matter here but keeping its exact
-- return order (actions.lua:196).
local function stockActionToCommands(actionName, actionCache)
  local c = actionCache or ACTIONS[actionName]
  if not c then return false end
  local actionMap = "Normal"
  if c.cat == "menu" then actionMap = "Menu" end
  if c.ctx == "vlua" then actionMap = "VehicleCommon" end
  if c.actionMap then actionMap = c.actionMap end
  if actionMap == "MenuIndependent" then actionMap = "MenuIndependent_" .. actionName end

  local ctx = { type = COMMAND_CONTEXT_TLUA }
  if c.ctx == "vlua" then ctx.type = COMMAND_CONTEXT_VLUA
  elseif c.ctx == "elua" then ctx.type = COMMAND_CONTEXT_ELUA end

  return true, actionMap,
         c.onChange ~= nil, c.onChange or "",
         c.onDown ~= nil,   c.onDown or "",
         c.onUp ~= nil,     c.onUp or "",
         false, ctx, c.isCentered or false
end

core_input_actions = {
  actionToCommands = stockActionToCommands,
  getActiveActions = function() return ACTIONS end,
}

-- --- devices and bindings ----------------------------------------------------------------------
local BINDINGS = {
  {
    devname = "xinput0",
    contents = {
      devicetype = "xinput",
      bindings = {
        { action = "parkingbrake_toggle",            control = "btn_b" },
        { action = "shiftUp",                        control = "btn_a" },
        { action = "steering",                       control = "thumblx" },
        { action = "toggleMenues",                   control = "btn_start" },
        { action = "menu_item_focus_ud",             control = "thumbly" },
        { action = "beamtel_accessibility_activate", control = "btn_x" },
        { action = "look_back",                      control = "btn_lt" },
        { action = "customModifier1",                control = "btn_r" },
        -- Deliberately unmodified FIRST, so "take the first binding" answers without the
        -- modifier and scenario 16 cannot pass for free.
        { action = "activateStarterMotor",           control = "dpov" },
        { action = "activateStarterMotor",           control = "modifier1 dpov" },
        { action = "signal_right",                   control = "rpov" },
        { action = "triggerAction0",                 control = "btn_a" },
        { action = EVIL,                             control = "button9" },
        { action = "shiftUp",                        control = "unused_ghost", unused = true },
      },
    },
  },
}

-- What the last simulated re-push produced, keyed by action: exactly what am:bind would receive.
local pushed = {}
local pushCount = 0

local function simulatePush()
  pushCount = pushCount + 1
  pushed = {}
  for _, device in ipairs(BINDINGS) do
    for _, b in ipairs(device.contents.bindings) do
      if not b.unused then
        -- Through the module table, exactly as sendBindingsToGE does.
        local success, actionMap, actsOnChange, onChange, actsOnDown, onDown, actsOnUp, onUp,
              isRelative, ctx, isCentered = core_input_actions.actionToCommands(b.action)
        if success then
          pushed[b.action] = {
            actionMap = actionMap, isCentered = isCentered, isRelative = isRelative,
            actsOnChange = actsOnChange, onChange = onChange,
            actsOnDown = actsOnDown, onDown = onDown,
            actsOnUp = actsOnUp, onUp = onUp,
            ctxType = ctx and ctx.type,
          }
        end
      end
    end
  end
end

local pushFails = false
core_input_bindings = {
  bindings = BINDINGS,
  devices = { xinput0 = { "{guid}", "Controller (Xbox One For Windows)", "pidvid", 0 } },
  getRecentDevices = function() return { "xinput0" } end,
  getUsedBindingsFiles = function()
    if pushFails then error("simulated refresh failure", 0) end
    simulatePush()
    return {}
  end,
  onDeviceChanged = function()
    if pushFails then error("simulated refresh failure", 0) end
    simulatePush()
  end,
}

-- The formatter bindingLearn borrows from vehicleBindings rather than copying. Stubbed here; the
-- seam itself is grepped in scenario 11, which is the part a fake cannot reach.
extensions = {
  vehicleBindings = {
    deviceKey = function(devicetype, devname) return (tostring(devicetype or devname):gsub("%d+$", "")) end,
    controlToSpeech = function(devKey, control) return devKey .. " " .. control end,
    translate = function(key) return key end,
  },
}

-- =================================================================================================
--  Load the real module and drive it
-- =================================================================================================

local M = dofile(SRC)
extensions.bindingLearn = M
M.onExtensionLoaded()

-- One tick of the real onUpdate. Every timer in the module accumulates dtReal, so the fake clock
-- is just the dt handed in -- no os.clockhp to stub, which is why the module accumulates at all.
local function tick(dt)
  M.onUpdate(dt or 0.016, dt or 0.016, dt or 0.016)
end

local function cmd(text) inbox[#inbox + 1] = text end

-- Hits are buffered for HIT_COALESCE_S and emitted as one grouped report by onUpdate, so nothing
-- is on the wire until the module has been ticked past the window.
local function flush() tick(HIT_COALESCE_S + 0.02) end

local function drainSent()
  local out = sent
  sent = {}
  return out
end

local function sentMatching(list, prefix)
  local n = 0
  for _, s in ipairs(list) do
    if s:sub(1, #prefix) == prefix then n = n + 1 end
  end
  return n
end

local function lastLearnRow(list)
  local out = nil
  for _, s in ipairs(list) do
    if s:sub(1, 6) == "LEARN:" then out = s end
  end
  return out
end

-- Snapshot the stock push once, before anything is wrapped, as the reference every restore is
-- compared against.
simulatePush()
local STOCK = {}
for k, v in pairs(pushed) do
  STOCK[k] = { actionMap = v.actionMap, isCentered = v.isCentered, ctxType = v.ctxType,
               onDown = v.onDown, onChange = v.onChange, onUp = v.onUp }
end

print(string.format(
  "tuning: heartbeat %.1f s  axis %.2f / %.1f s  button %.2f s  wrap check %.1f s  cap %.0f s",
  HEARTBEAT_TIMEOUT_S, AXIS_MIN_DEFLECTION, AXIS_COOLDOWN_S, BUTTON_REPEAT_S,
  WRAP_CHECK_S, MAX_SESSION_S))
print()

-- =================================================================================================

print("1. the source parses and loads")
do
  local chunk, err = loadfile(SRC)
  check("bindingLearn.lua parses", chunk ~= nil, err)
  check("...and returns a module with the hit entry point",
    type(M) == "table" and type(M.hit) == "function")
  check("...whose command port is the free pair, not a collision",
    SRC_BODY:find("PYTHON_PORT_DATA = 4479", 1, true) ~= nil
    and SRC_BODY:find("CMD_LISTEN_PORT  = 4480", 1, true) ~= nil)
end
print()

print("2. the wrapper survives being reloaded over itself")
do
  -- vehicleInfo.lua paid for every one of these in a live bug. A boolean mark reads "installed"
  -- for a wrapper left by an OLDER build that calls its own captured handler; a wrapper without
  -- ORIG_KEY parked gets re-wrapped instead of replaced, stacking a layer per reload.
  check("the mark is a version integer, not a boolean",
    SRC_BODY:find("local WRAP_VERSION = %d") ~= nil
    and SRC_BODY:find("act%[WRAP_MARK%] == WRAP_VERSION") ~= nil)
  check("...and the original is parked so a stale wrapper is replaced, not stacked",
    SRC_BODY:find("act%[ORIG_KEY%] or act%.actionToCommands") ~= nil)
  check("...and the dispatch is a mutable slot read through the stock module table",
    SRC_BODY:find("local h = act%[HANDLER_KEY%]") ~= nil)
  check("...and installed-means-OURS, not merely installed",
    SRC_BODY:find("act%[HANDLER_KEY%] == rewriteCommands") ~= nil)
  check("...and the handler is pcall'd so a throw cannot take the binding push down",
    SRC_BODY:find("pcall%(h, actionName") ~= nil)

  -- The negative control for the whole block: the naive forms must be absent.
  check("the naive boolean-mark form is absent",
    SRC_BODY:find("wrapInstalled = true") == nil
    and SRC_BODY:find("act%[WRAP_MARK%] == true") == nil)
end
print()

print("3. actionMap, isCentered and isRelative are structurally untouchable")
do
  -- Rewriting actionMap would move a binding to a different ActionMap and break the
  -- MenuIndependent catalogue readActionsFromDisk builds -- game-wide menu gating, from a
  -- feature that has nothing to do with menus. Passing them to the handler at all is the risk,
  -- so the check is that they are NOT passed.
  local sig = SRC_BODY:match("rewriteCommands = function%((.-)%)")
  check("the handler is never handed actionMap", sig ~= nil and not sig:find("actionMap"), sig)
  check("...nor isCentered or isRelative",
    sig ~= nil and not sig:find("isCentered") and not sig:find("isRelative"), sig)

  cmd("LEARN_ON"); tick()
  local bad = {}
  for action, stock in pairs(STOCK) do
    local now = pushed[action]
    if not now then
      bad[#bad + 1] = action .. " missing from push"
    else
      if now.actionMap ~= stock.actionMap then
        bad[#bad + 1] = action .. " actionMap " .. tostring(stock.actionMap) .. "->" .. tostring(now.actionMap)
      end
      if now.isCentered ~= stock.isCentered then bad[#bad + 1] = action .. " isCentered" end
    end
  end
  check("every action keeps its actionMap and isCentered through a learn-mode push",
    #bad == 0, table.concat(bad, ", "))
  check("...and the mode reported itself on", sentMatching(drainSent(), "LEARNMODE:ON") == 1)
end
print()

print("4. the exemption is the trap door: menu bindings still fire")
do
  -- If the pause key were suppressed there would be no way back out of a menu that opened by
  -- any other route, and the six accessibility actions are the controller's only exit.
  local pause = pushed.toggleMenues
  check("toggleMenues keeps its original command verbatim",
    pause ~= nil and pause.onDown:find(STOCK.toggleMenues.onDown, 1, true) ~= nil,
    pause and pause.onDown)
  check("...with the announcement riding in front of it, not replacing it",
    pause ~= nil
    and pause.onDown:find("extensions.bindingLearn.hit('toggleMenues'", 1, true) == 1,
    pause and pause.onDown)
  check("menu axis navigation is exempt too",
    pushed.menu_item_focus_ud.onChange:find(STOCK.menu_item_focus_ud.onChange, 1, true) ~= nil)
  check("this mod's own accessibility actions are exempt",
    pushed.beamtel_accessibility_activate.onDown:find(
      STOCK.beamtel_accessibility_activate.onDown, 1, true) ~= nil)

  -- ...and the exemption is a category test, not a name list that a new menu action falls out of.
  check("the exemption is by category, not by action name",
    SRC_BODY:find("EXEMPT_CATS = { menu = true, menuExtra = true, accessibility = true, modifier = true }", 1, true) ~= nil)
  check("...and a camera action is NOT exempt (so the rule is not exempting everything)",
    pushed.look_back.onChange:find(STOCK.look_back.onChange, 1, true) == nil,
    pushed.look_back.onChange)
end
print()

print("5. everything else is genuinely suppressed")
do
  local bad = {}
  for _, action in ipairs({"parkingbrake_toggle", "shiftUp", "steering", "look_back"}) do
    local now, stock = pushed[action], STOCK[action]
    for _, field in ipairs({"onDown", "onChange", "onUp"}) do
      if stock[field] ~= "" and now[field]:find(stock[field], 1, true) then
        bad[#bad + 1] = action .. "." .. field .. " survived"
      end
    end
    if now.ctxType ~= COMMAND_CONTEXT_TLUA then
      bad[#bad + 1] = action .. " ctx not routed to the game engine"
    end
  end
  check("no suppressed action's original command reaches the engine", #bad == 0,
    table.concat(bad, ", "))
  check("...and the handbrake now calls the mod instead",
    pushed.parkingbrake_toggle.onDown == "extensions.bindingLearn.hit('parkingbrake_toggle',VALUE,'d')",
    pushed.parkingbrake_toggle.onDown)
  check("an onUp-carrying action drops its onUp (a release is not a discovery)",
    pushed.shiftUp.actsOnUp == false and pushed.shiftUp.actsOnDown == true)
end
print()

print("6. an action name is vetted, never escaped")
do
  -- The name is concatenated into a string the engine executes. A name that cannot be vetted is
  -- left ALONE: the binding then fires normally and is simply not learnable, which is the safe
  -- direction. The alternative is a quote breaking out of the command.
  check("the unsafe name is left entirely unrewritten",
    pushed[EVIL] ~= nil and pushed[EVIL].onDown == STOCK[EVIL].onDown,
    pushed[EVIL] and pushed[EVIL].onDown)
  check("...and no command string anywhere carries it",
    (function()
      for _, v in pairs(pushed) do
        if v.onDown:find("os.exit", 1, true) or v.onChange:find("os.exit", 1, true) then
          return false
        end
      end
      return true
    end)())
  check("the vet is an allowlist pattern, not a blocklist",
    SRC_BODY:find('actionName:match%("%^%[%%w_%%%-%%.%]%+%$"%)') ~= nil)
end
print()

print("7. buttons are deduped and axes are gated with hysteresis")
do
  drainSent()
  -- A button held down repeats; a stick sends every frame. Neither should machine-gun speech.
  M.hit("parkingbrake_toggle", 1, "d")
  M.hit("parkingbrake_toggle", 1, "d")
  flush()
  check("a repeated press inside the dedupe window speaks once",
    sentMatching(sent, "LEARN:") == 1, tostring(sentMatching(sent, "LEARN:")))
  M.hit("parkingbrake_toggle", 0, "d")
  flush()
  check("...and a release says nothing", sentMatching(sent, "LEARN:") == 1)
  for _ = 1, math.ceil(BUTTON_REPEAT_S / 0.016) + 2 do tick() end
  M.hit("parkingbrake_toggle", 1, "d")
  flush()
  check("...but a fresh press after the window speaks again",
    sentMatching(sent, "LEARN:") == 2, tostring(sentMatching(sent, "LEARN:")))

  drainSent()
  M.hit("steering", AXIS_MIN_DEFLECTION * 0.5, "c")
  M.hit("steering", AXIS_MIN_DEFLECTION * 0.9, "c")
  flush()
  check("a stick short of the threshold says nothing", sentMatching(sent, "LEARN:") == 0)
  M.hit("steering", AXIS_MIN_DEFLECTION * 1.2, "c")
  flush()
  check("...crossing it speaks once", sentMatching(sent, "LEARN:") == 1)
  M.hit("steering", 1.0, "c")
  M.hit("steering", 1.0, "c")
  flush()
  check("...and holding it does not repeat inside the cooldown",
    sentMatching(sent, "LEARN:") == 1, tostring(sentMatching(sent, "LEARN:")))
  -- Re-arm is derived from the threshold rather than being its own constant, so the two can
  -- never be set into a window where releasing the stick fails to re-arm it.
  M.hit("steering", 0.0, "c")
  M.hit("steering", -1.0, "c")
  flush()
  check("...but releasing and pushing the other way speaks immediately",
    sentMatching(sent, "LEARN:") == 2, tostring(sentMatching(sent, "LEARN:")))
  check("the re-arm level is derived from the threshold, not a second constant",
    SRC_BODY:find("AXIS_REARM%s*=%s*AXIS_MIN_DEFLECTION") ~= nil)

  local row = lastLearnRow(sent)
  check("the row names the control and what it does", row ~= nil
    and row:find('"control":"xinput thumblx"', 1, true) ~= nil
    and row:find('"kind":"axis"', 1, true) ~= nil, row)
  check("...and marks a suppressed binding as suppressed",
    row ~= nil and row:find('"suppressed":1', 1, true) ~= nil, row)
end
print()

print("8. an exempt binding is reported as still active")
do
  drainSent()
  M.hit("toggleMenues", 1, "d")
  flush()
  local row = lastLearnRow(sent)
  -- "It did something anyway" has to be announced, or it reads as the mode being broken.
  check("the exempt press is reported as NOT suppressed",
    row ~= nil and row:find('"suppressed":0', 1, true) ~= nil, row)
end
print()

print("9. leaving restores the stock bindings exactly")
do
  drainSent()
  cmd("LEARN_OFF"); tick()
  local bad = {}
  for action, stock in pairs(STOCK) do
    local now = pushed[action]
    if not now then bad[#bad + 1] = action .. " missing"
    else
      for _, field in ipairs({"onDown", "onChange", "onUp"}) do
        if now[field] ~= stock[field] then bad[#bad + 1] = action .. "." .. field end
      end
      if now.ctxType ~= stock.ctxType then bad[#bad + 1] = action .. ".ctx" end
    end
  end
  check("every command string is byte-identical to the stock push", #bad == 0,
    table.concat(bad, ", "))
  check("...and the mode reported itself off", sentMatching(drainSent(), "LEARNMODE:OFF") == 1)
  check("...and a press after leaving says nothing",
    (function() M.hit("parkingbrake_toggle", 1, "d"); tick(); return sentMatching(sent, "LEARN:") == 0 end)())
end
print()

print("10. the watchdog is what stops beamtel dying from bricking the game")
do
  -- With the mode on, every binding in the game points at this extension. If beamtel goes away
  -- and nothing tears the mode down, the controls stay dead until BeamNG is restarted. This is
  -- the single most important check in the file.
  drainSent()
  cmd("LEARN_ON"); tick()
  check("armed again", pushed.parkingbrake_toggle.onDown ~= STOCK.parkingbrake_toggle.onDown)

  -- Just short of the timeout, with the keepalives stopped, it must still be on -- otherwise the
  -- check below would pass for free on any teardown at all.
  local dt = 0.1
  local steps = math.floor((HEARTBEAT_TIMEOUT_S - 0.5) / dt)
  for _ = 1, steps do tick(dt) end
  check("...still on just short of the timeout",
    pushed.parkingbrake_toggle.onDown ~= STOCK.parkingbrake_toggle.onDown)

  for _ = 1, math.ceil(1.0 / dt) + 2 do tick(dt) end
  check("...and torn itself down once the keepalive went quiet",
    pushed.parkingbrake_toggle.onDown == STOCK.parkingbrake_toggle.onDown,
    pushed.parkingbrake_toggle.onDown)
  check("...saying which of its several reasons it was",
    (function()
      for _, s in ipairs(sent) do if s:find("keepalive lost", 1, true) then return true end end
      return false
    end)())

  -- ...and a keepalive keeps it alive, or the mode would be unusable for its own timeout.
  drainSent()
  cmd("LEARN_ON"); tick()
  for _ = 1, math.floor((HEARTBEAT_TIMEOUT_S * 2) / dt) do
    cmd("KEEPALIVE")
    tick(dt)
  end
  check("a fed watchdog leaves the mode running past twice the timeout",
    pushed.parkingbrake_toggle.onDown ~= STOCK.parkingbrake_toggle.onDown)
  cmd("LEARN_OFF"); tick()
end
print()

print("11. a re-push that fails on the way OUT is retried, not swallowed")
do
  -- The wrapper is gone but the engine still holds the commands it wrote: the one genuinely
  -- dangerous state this feature has. Going quiet here is what would leave a bricked game.
  drainSent()
  cmd("LEARN_ON"); tick()
  pushFails = true
  cmd("LEARN_OFF"); tick()
  check("the failure is announced rather than reported as a clean exit",
    sentMatching(sent, "LEARNFAIL:norestore") == 1
    and sentMatching(sent, "LEARNMODE:OFF") == 0)
  pushFails = false
  drainSent()
  for _ = 1, math.ceil(readConst("RESTORE_RETRY_S") / 0.1) + 2 do tick(0.1) end
  check("...and the retry puts the stock bindings back on its own",
    pushed.parkingbrake_toggle.onDown == STOCK.parkingbrake_toggle.onDown)
  check("...and says so", sentMatching(sent, "LEARNMODE:OFF") == 1)
end
print()

print("12. a Lua reload that drops the wrapper is noticed and re-armed")
do
  -- Ctrl+L reloads core_input_actions, taking the wrapper with it. The mode would then claim to
  -- be suppressing while every binding fired normally -- a mode that lies about the one thing it
  -- promises. vehicleInfo.lua re-arms its own wrapper for the same reason.
  drainSent()
  cmd("LEARN_ON"); tick()
  core_input_actions.actionToCommands = stockActionToCommands
  core_input_actions.__bngScreenReaderLearnWrappedVer = nil
  core_input_actions.__bngScreenReaderLearnHandler = nil
  core_input_actions.__bngScreenReaderLearnOrig = nil
  simulatePush()
  check("the wrapper really is gone",
    pushed.parkingbrake_toggle.onDown == STOCK.parkingbrake_toggle.onDown)
  for _ = 1, math.ceil(WRAP_CHECK_S / 0.1) + 2 do cmd("KEEPALIVE"); tick(0.1) end
  check("...and the module put it back",
    pushed.parkingbrake_toggle.onDown ~= STOCK.parkingbrake_toggle.onDown,
    pushed.parkingbrake_toggle.onDown)
  cmd("LEARN_OFF"); tick()
end
print()

print("13. unloading restores, inverting vehicleInfo's rule on purpose")
do
  -- vehicleInfo leaves its wrapper installed on unload because a reload is about to reclaim the
  -- slot -- correct there, because that wrapper is ADDITIVE and an orphan is merely useless.
  -- This one is SUBSTITUTIVE, so an orphan points every binding at a dead extension.
  cmd("LEARN_ON"); tick()
  M.onExtensionUnloaded()
  simulatePush()
  check("onExtensionUnloaded leaves the stock bindings in place",
    pushed.parkingbrake_toggle.onDown == STOCK.parkingbrake_toggle.onDown,
    pushed.parkingbrake_toggle.onDown)
  check("...and it restores BEFORE closing the sockets, so it can still say so",
    (function()
      local body = SRC_BODY:match("function M%.onExtensionUnloaded%(%)(.-)\nend\n")
      if not body then return false end
      local stopAt = body:find("stopLearn", 1, true)
      local closeAt = body:find("udpSend:close", 1, true)
      return stopAt ~= nil and closeAt ~= nil and stopAt < closeAt
    end)())
  check("a level change ends the mode too",
    SRC_BODY:find("function M%.onClientEndMission") ~= nil
    and SRC_BODY:find('stopLearn%("level change"%)') ~= nil)
end
print()

print("14. the pad's modifier button must keep working, or nothing modified fires at all")
do
  -- The report was "modifiers are flaky and the accessibility bindings are ignored outright".
  -- Both are this: customModifier1..6 are ordinary ACTIONS (cat "modifier") whose onChange is the
  -- single call that enables the modifier engine-side. Suppressed, holding the shoulder button
  -- sets nothing, so "modifier1 dpov" can never match and the engine fires the PLAIN "dpov"
  -- bindings instead -- which is exactly a modifier that silently went missing. The keyboard's
  -- ctrl/shift/alt are native device state and keep working, which is what made it look flaky
  -- rather than broken. This mod's own accessibility bindings are all "modifier2 <button>", so
  -- for them it is not flaky at all: none of them can fire.
  M.onExtensionLoaded()
  drainSent()
  cmd("LEARN_ON"); tick()
  check("the modifier action keeps its original command",
    pushed.customModifier1.onChange:find(STOCK.customModifier1.onChange, 1, true) ~= nil,
    pushed.customModifier1.onChange)
  check("...with the announcement riding in front of it rather than replacing it",
    pushed.customModifier1.onChange:find("extensions.bindingLearn.hit('customModifier1'", 1, true) == 1,
    pushed.customModifier1.onChange)
  check("...so it is exempt, not merely left alone",
    pushed.customModifier1.onChange ~= STOCK.customModifier1.onChange)

  drainSent()
  M.hit("customModifier1", 1, "c")
  flush()
  local row = lastLearnRow(sent)
  -- Declared with onChange because it is reported through the analogue path, but it is a
  -- shoulder button. Classifying on the command slot instead of the control announced "axis".
  check("the modifier press is named as a button, not an axis",
    row ~= nil and row:find('"kind":"button"', 1, true) ~= nil, row)
  check("...and reported as still active", row ~= nil and row:find('"suppressed":0', 1, true) ~= nil, row)
end
print()

print("15. a modified binding is named by the modifier that is actually held")
do
  -- activateStarterMotor is bound on this pad BOTH ways, unmodified first. Taking the first
  -- binding therefore answered "dpov" whichever was pressed. The held modifier is knowable now
  -- precisely because scenario 14 made customModifierN report through M.hit.
  drainSent()
  M.hit("activateStarterMotor", 1, "d")   -- modifier1 still held from scenario 14
  flush()
  local held = lastLearnRow(sent)
  check("with the modifier held, the modified control is named",
    held ~= nil and held:find('"control":"xinput modifier1 dpov"', 1, true) ~= nil, held)
  check("...and the naive first-binding form would have dropped it",
    BINDINGS[1].contents.bindings[(function()
      for i, b in ipairs(BINDINGS[1].contents.bindings) do
        if b.action == "activateStarterMotor" then return i end
      end
    end)()].control == "dpov")

  drainSent()
  M.hit("customModifier1", 0, "c")        -- release: the edge that clears it
  for _ = 1, math.ceil(BUTTON_REPEAT_S / 0.05) + 2 do tick(0.05) end
  M.hit("activateStarterMotor", 1, "d")
  flush()
  local plain = lastLearnRow(sent)
  check("with it released, the unmodified control is named",
    plain ~= nil and plain:find('"control":"xinput dpov"', 1, true) ~= nil, plain)
  check("...so the two presses genuinely answer differently", held ~= plain)
  check("the release edge is recorded before the hysteresis gates return",
    (function()
      local body = SRC_BODY:match("function M%.hit%(actionName, value, kind%)(.-)lastHit%[actionName%] = now")
      if not body then return false end
      local modAt = body:find("heldModifiers%[tonumber%(modN%)%]")
      local gateAt = body:find("if kind == 'c' then")
      return modAt ~= nil and gateAt ~= nil and modAt < gateAt
    end)())
end
print()

print("16. one press is one announcement, however many bindings the button carries")
do
  -- A stock pad has btn_a on accept, menu_item_select, shiftUp, triggerAction0 and
  -- bigMapControllerSelect. Sent one packet at a time each announcement interrupts the last and
  -- only the final one is ever heard -- which is what "it doesn't count the alternate actions"
  -- describes. Grouped by CONTROL, not simply by window, so a fast two-button press stays two
  -- answers.
  drainSent()
  M.hit("shiftUp", 1, "d")
  M.hit("triggerAction0", 1, "d")
  flush()
  local rows = 0
  local row = nil
  for _, m in ipairs(sent) do if m:sub(1, 6) == "LEARN:" then rows = rows + 1; row = m end end
  check("two actions on one button produce ONE report", rows == 1, tostring(rows))
  check("...naming both of them", row ~= nil
    and row:find("shiftUp.title", 1, true) ~= nil
    and row:find("triggerAction0.title", 1, true) ~= nil, row)
  check("...under a single control", row ~= nil
    and row:find('"control":"xinput btn_a"', 1, true) ~= nil, row)

  drainSent()
  M.hit("parkingbrake_toggle", 1, "d")    -- btn_b
  M.hit("look_back", 1, "d")              -- btn_lt
  flush()
  local groups = 0
  for _, m in ipairs(sent) do if m:sub(1, 6) == "LEARN:" then groups = groups + 1 end end
  check("but two different controls stay two reports", groups == 2, tostring(groups))
  cmd("LEARN_OFF"); tick()
end
print()

print("17. a modifier held over a control that has none is unbound, not the bare binding")
do
  -- The engine does not refuse a modified press with no binding -- it falls THROUGH and fires
  -- the bare one. Naming that answers a question nobody asked: "modifier 1 plus d-pad right" is
  -- a different button from "d-pad right", and announcing the right indicator for it is a
  -- confident wrong answer about the combination actually pressed. Reported rather than dropped,
  -- because silence is indistinguishable from the mode being broken.
  drainSent()
  cmd("LEARN_ON"); tick()
  M.hit("customModifier1", 1, "c")
  flush()
  drainSent()
  M.hit("signal_right", 1, "d")           -- rpov has no modifier1 binding anywhere
  flush()
  local row = lastLearnRow(sent)
  check("the fall-through is reported as unbound",
    row ~= nil and row:find('"unbound":1', 1, true) ~= nil, row)
  check("...naming the combination that was physically pressed, not the bare control",
    row ~= nil and row:find('"control":"xinput modifier1 rpov"', 1, true) ~= nil, row)
  check("...and NOT naming what the bare control does",
    row ~= nil and row:find("signal_right.title", 1, true) == nil, row)

  -- ...and the drop is scoped to the fall-through. A control that really does carry a binding
  -- for the held modifier still answers, and the bare actions sharing it are the ones dropped.
  drainSent()
  for _ = 1, math.ceil(BUTTON_REPEAT_S / 0.05) + 2 do cmd("KEEPALIVE"); tick(0.05) end
  M.hit("activateStarterMotor", 1, "d")   -- has a modifier1 binding
  M.hit("parkingbrake_toggle", 1, "d")    -- btn_b, bare only: falls through
  flush()
  local starter, brake = nil, nil
  for _, m in ipairs(sent) do
    if m:find("activateStarterMotor", 1, true) then starter = m end
    if m:find("modifier1 btn_b", 1, true) then brake = m end
  end
  check("a genuinely modified binding still answers",
    starter ~= nil and starter:find('"unbound":0', 1, true) ~= nil, starter)
  check("...while a bare-only action under the same modifier reads unbound",
    brake ~= nil and brake:find('"unbound":1', 1, true) ~= nil
    and brake:find("parkingbrake_toggle.title", 1, true) == nil, brake)

  -- An EXEMPT fall-through is dropped too. It genuinely fires, so this is the one place the
  -- "still active" contract is deliberately not applied: keeping it would mean almost every
  -- direction under a modifier naming a menu action (bare rpov alone carries menu_item_right),
  -- which is the same wrong answer the drop exists to prevent arriving through the exemption.
  drainSent()
  M.hit("toggleMenues", 1, "d")           -- btn_start, bare only, cat "menu"
  flush()
  local menu = lastLearnRow(sent)
  check("an exempt fall-through is dropped as well, not excused by its exemption",
    menu ~= nil and menu:find('"unbound":1', 1, true) ~= nil
    and menu:find("controllerui.menu.title", 1, true) == nil, menu)
  check("...while an exempt press with nothing held is still named as active",
    (function()
      drainSent()
      M.hit("customModifier1", 0, "c")
      for _ = 1, math.ceil(BUTTON_REPEAT_S / 0.05) + 2 do cmd("KEEPALIVE"); tick(0.05) end
      M.hit("toggleMenues", 1, "d")
      flush()
      local r = lastLearnRow(sent)
      return r ~= nil and r:find('"suppressed":0', 1, true) ~= nil
         and r:find('"unbound":0', 1, true) ~= nil
    end)())
  M.hit("customModifier1", 1, "c")        -- put it back for the checks below

  -- The modifier's OWN press must not be prefixed with itself. The held set is recorded before
  -- the resolve, so its binding -- which carries no modifier token -- would otherwise read as a
  -- fall-through and announce "modifier 1 plus right bumper". Seen in a live game.
  drainSent()
  for _ = 1, math.ceil(AXIS_COOLDOWN_S / 0.05) + 2 do cmd("KEEPALIVE"); tick(0.05) end
  M.hit("customModifier1", 1, "c")
  flush()
  local mod = lastLearnRow(sent)
  check("a modifier button is never named as a combination with itself",
    mod ~= nil and mod:find('"control":"xinput btn_r"', 1, true) ~= nil
    and mod:find("modifier1 btn_r", 1, true) == nil, mod)

  drainSent()
  M.hit("customModifier1", 0, "c")
  for _ = 1, math.ceil(BUTTON_REPEAT_S / 0.05) + 2 do cmd("KEEPALIVE"); tick(0.05) end
  M.hit("signal_right", 1, "d")
  flush()
  local bare = lastLearnRow(sent)
  check("with nothing held, the same press is an ordinary answer",
    bare ~= nil and bare:find('"unbound":0', 1, true) ~= nil
    and bare:find('"control":"xinput rpov"', 1, true) ~= nil, tostring(bare))
  cmd("LEARN_OFF"); tick()
end
print()

print("18. cross-file agreements no fake can reach")
do
  local py = readAll(PY_SRC)
  local vb = readAll(VB_SRC)
  local geo = readAll(GEO_SIM)

  -- The Python keepalive must beat the Lua timeout with room for dropped datagrams. Same rule
  -- trailerAngle.lua's heartbeat rests on.
  local pyInterval = tonumber(py:match("\nBINDING_LEARN_KEEPALIVE_S = ([%d%.]+)"))
  check("Python sends a keepalive at all", pyInterval ~= nil)
  check("...well under half the mod's timeout",
    pyInterval ~= nil and pyInterval <= HEARTBEAT_TIMEOUT_S / 2,
    string.format("%.1f s against a %.1f s timeout", pyInterval or -1, HEARTBEAT_TIMEOUT_S))

  -- Ports must agree at both ends, in both directions.
  check("the data port agrees at both ends",
    py:find("BINDING_LEARN_LISTEN_PORT = %(?\n?%s*4479") ~= nil
    and SRC_BODY:find("PYTHON_PORT_DATA = 4479", 1, true) ~= nil)
  check("the command port agrees at both ends",
    py:find("BINDING_LEARN_CMD_PORT = 4480", 1, true) ~= nil
    and SRC_BODY:find("CMD_LISTEN_PORT  = 4480", 1, true) ~= nil)

  -- The speech formatting is BORROWED, not copied: bindingLearn and the bindings browser must
  -- name one binding the same way or the two readouts of it disagree.
  check("bindingLearn borrows vehicleBindings' control formatter",
    SRC_BODY:find("vb.controlToSpeech", 1, true) ~= nil
    and SRC_BODY:find("vb.deviceKey", 1, true) ~= nil
    and SRC_BODY:find("vb.translate", 1, true) ~= nil)
  check("...and vehicleBindings actually exports all three",
    vb:find("M.deviceKey%s*=%s*deviceKey") ~= nil
    and vb:find("M.controlToSpeech%s*=%s*controlToSpeech") ~= nil
    and vb:find("M.translate%s*=%s*translate") ~= nil)
  check("...rather than carrying its own copy of the key labels",
    SRC_BODY:find("KEY_LABELS", 1, true) == nil)

  -- The socket contract grep in vehicle_geometry_sim.lua only polices files it knows about.
  check("bindingLearn is on the listening-extension list scenario 12 polices",
    geo:find('"bindingLearn"', 1, true) ~= nil)

  -- Every F9 branch needs a help entry or input-help mode answers "No command".
  check("the F9 key has an input-help entry",
    py:find('%("b", False, True, False%): "Toggle learn bindings mode"') ~= nil)
  check("...and the mode is reachable from the controller as well",
    py:find('_function_item%("Learn bindings mode"') ~= nil)
end
print()

if #failures > 0 then
  print(string.format("%d FAILURE(S): %s", #failures, table.concat(failures, ", ")))
  os.exit(1)
end
print("all checks passed")
