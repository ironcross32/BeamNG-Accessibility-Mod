-- =================================================================================================
--
--  Learn Bindings Mode for BeamNG.drive Accessibility Mod (GE Extension)
--
--  Description: A mode you switch on, press buttons in, and hear what each one DOES -- while
--               the binding does not fire. Press the handbrake to find out it is the handbrake,
--               without the car spinning. Covers the stock controller/wheel profiles the game
--               ships as well as any custom binding the user has made.
--
--               Ports 4479 (data to Python) / 4480 (commands from Python).
--
--  Loaded by: scripts/bng_screenreader_mod/modScript.lua
--
--  Version:     1.0
--  Target Game: BeamNG.drive 0.39+
--
--  See docs/lua-binding-learn.md for the reasoning that is invisible in this code.
--
-- =================================================================================================

local M = {}

M.dependencies = {"core_input_actions", "core_input_bindings"}

-- Configuration
local PYTHON_HOST      = "127.0.0.1"
local PYTHON_PORT_DATA = 4479   -- send learn events to Python on this port
local CMD_LISTEN_PORT  = 4480   -- receive commands from Python on this port

-- The watchdog budget. Python sends KEEPALIVE once a second; this is the silence after which
-- the mode tears itself down. It has to be comfortably more than one send interval (a dropped
-- datagram must not end the mode) and comfortably less than a person's patience with a game
-- whose controls have stopped responding.
local HEARTBEAT_TIMEOUT_S = 6.0
local MAX_SESSION_S       = 1800.0  -- hard cap; nobody maps a pad for half an hour
local RESTORE_RETRY_S     = 1.0
local WRAP_CHECK_S        = 0.5

-- Axis reporting. A stick or pedal sends continuously, so the report is edge-triggered with
-- hysteresis: fire crossing MIN_DEFLECTION, repeat no faster than COOLDOWN while still held,
-- and re-arm instantly once it falls back under REARM. REARM is DERIVED rather than being its
-- own constant, so the two can never be set into a window that latches.
local AXIS_MIN_DEFLECTION = 0.35
local AXIS_COOLDOWN_S     = 4.0
local AXIS_REARM          = AXIS_MIN_DEFLECTION * 0.5
local BUTTON_REPEAT_S     = 0.6

-- One physical press is one announcement, however many actions it fires. A button routinely
-- carries several bindings at once (a stock pad has btn_a on accept, menu_item_select, shiftUp,
-- triggerAction0 and bigMapControllerSelect), and every one of them arrives here as its own hit
-- within the same frame or two. Sent one at a time they each interrupt the last, so the user
-- hears only whichever happened to arrive last -- which reads exactly like the other actions
-- being ignored. Hits are therefore buffered for this long and emitted as one grouped report.
-- Long enough to catch a whole frame's worth of dispatch, short enough not to feel like lag.
local HIT_COALESCE_S = 0.12

-- Wrapper bookkeeping. See installWrapper() for why each of these exists.
local WRAP_MARK    = "__bngScreenReaderLearnWrappedVer"
local ORIG_KEY     = "__bngScreenReaderLearnOrig"
local HANDLER_KEY  = "__bngScreenReaderLearnHandler"
local WRAP_VERSION = 1

-- The exemption rule, and the trap door. Every UI navigation action is cat "menu" -- and so is
-- toggleMenues, the button that OPENS the pause menu (lua/ge/extensions/core/input/actions/
-- menu.json). Exempting the category therefore guarantees that if a menu does come up it is
-- navigable and escapable, which is the one way this mode could strand somebody. One rule,
-- two jobs. "accessibility" is this mod's own six actions: the controller route out.
--
-- Exempt actions are still ANNOUNCED (see rewriteCommands) -- they just also fire.
--
-- "modifier" is exempt for a harder reason than the other two, and it is not about menus at all:
-- suppressing it BREAKS EVERY MODIFIED BINDING IN THE GAME. On a pad, modifier1/modifier2 are
-- not a device-level state the way keyboard ctrl/shift are -- they are ordinary actions
-- (customModifier1..6 in actions/general.json, cat "modifier") whose onChange is the single call
-- that enables the modifier: core_input_bindings.enableCustomModifier(PLAYER, VALUE ~= 0, N).
-- Replace that command and holding the shoulder button sets nothing, so "modifier2 btn_a" can
-- never match and the engine fires the PLAIN "btn_a" bindings instead. Symptom from the seat:
-- modified bindings intermittently announce without their modifier (the keyboard's native
-- ctrl/shift/alt still work, the pad's do not), and this mod's own accessibility bindings --
-- all five of which are "modifier2 <button>" -- appear to be ignored outright.
local EXEMPT_CATS = { menu = true, menuExtra = true, accessibility = true, modifier = true }

-- Internal state
local udpSend        = nil
local udpCmd         = nil
local learnActive    = false
local clockS         = 0.0     -- accumulated dtReal; see onUpdate
local startedAt      = 0.0
local lastKeepalive  = 0.0
local restorePending = false
local restoreTimer   = 0.0
local wrapCheckTimer = 0.0
local resolving      = false   -- reentrancy guard around getActiveActions
local lastHit        = {}      -- action -> clockS of last announcement
local axisState      = {}      -- action -> clockS of last axis announcement, nil when re-armed
local heldModifiers  = {}      -- N -> true while customModifierN is held; see EXEMPT_CATS
local pendingHits    = nil     -- hits waiting out HIT_COALESCE_S
local pendingSince   = 0.0
local rewriteCount   = 0
local exemptCount    = 0
local lastHitText    = "none"

local function blLog(level, msg)
  log(level, 'BindingLearn', msg)
end

local function send(msg)
  if not udpSend then return end
  pcall(function() udpSend:send(msg) end)
end

-- =================================================================================================
--  The wrapper
--
--  Every binding reaches the engine as a command STRING plus a context enum, assembled in exactly
--  one place: sendBindingsToGE (core/input/bindings.lua:171) calls
--  core_input_actions.actionToCommands and hands the result straight to am:bind. A repo-wide grep
--  finds only two callers of that function and both go through the module table, so wrapping it
--  reaches every binding in the game.
--
--  Suppression is therefore not a mechanism of its own -- it is the ABSENCE of the original
--  command. Nothing is filtered (core_input_actionFilter would block silently, with no callback,
--  so it can suppress but can never tell us what was pressed), no action map is pushed, and
--  restoring is just unwrapping and asking the game to re-push.
--
--  The four-part shape below is vehicleInfo.lua's, and all four of its reasons carry over:
--   * a closure captures upvalues, so a wrapper installed by one instance writes to THAT
--     instance's locals forever; extensions.reload builds a fresh instance and every call would
--     land in the dead one. The mutable HANDLER_KEY slot on the stock module is how the newest
--     instance takes the dispatch back.
--   * the mark is a VERSION INTEGER, not a boolean: a wrapper left by an older build of this file
--     predates the slot and calls its own captured handler, so a boolean would read "installed"
--     and be ignored.
--   * ORIG_KEY parks the original so a stale wrapper is REPLACED rather than wrapped again.
--     Re-wrapping stacks a layer per reload and every layer runs on every binding.
--   * the handler is pcall'd, because a throw in here takes the game's entire binding push down.
-- =================================================================================================

local rewriteCommands  -- forward declaration; defined below, installed into HANDLER_KEY

local function installWrapper()
  local act = core_input_actions
  if type(act) ~= "table" then return false end

  -- Claim the slot first and unconditionally: this is what a reloaded instance does to take
  -- ownership of a wrapper that is already installed and current.
  act[HANDLER_KEY] = rewriteCommands

  if act[WRAP_MARK] == WRAP_VERSION then return true end

  local orig = act[ORIG_KEY] or act.actionToCommands
  if type(orig) ~= "function" then return false end
  act[ORIG_KEY] = orig

  act.actionToCommands = function(actionName, actionCache)
    local success, actionMap, actsOnChange, onChange, actsOnDown, onDown, actsOnUp, onUp,
          isRelative, ctx, isCentered = orig(actionName, actionCache)
    if not success then return success end

    local h = act[HANDLER_KEY]
    if h then
      local ok, r = pcall(h, actionName, actionCache, actsOnChange, onChange,
                          actsOnDown, onDown, actsOnUp, onUp, ctx)
      if ok and type(r) == "table" then
        -- Note what is NOT passed to the handler and therefore cannot be altered by it:
        -- actionMap, isRelative and isCentered come straight back out of the original call.
        -- actionMap in particular decides which ActionMap the binding lands on and feeds the
        -- MenuIndependent catalogue in readActionsFromDisk; rewriting it would break menu
        -- gating game-wide. Making that structurally impossible beats commenting on it.
        return true, actionMap, r.actsOnChange, r.onChange, r.actsOnDown, r.onDown,
               r.actsOnUp, r.onUp, isRelative, ctx, isCentered
      end
    end

    return success, actionMap, actsOnChange, onChange, actsOnDown, onDown, actsOnUp, onUp,
           isRelative, ctx, isCentered
  end

  act[WRAP_MARK] = WRAP_VERSION
  blLog('I', "Wrapped core_input_actions.actionToCommands (v" .. WRAP_VERSION .. ").")
  return true
end

local function wrapperInstalled()
  local act = core_input_actions
  return type(act) == "table"
     and act[WRAP_MARK] == WRAP_VERSION
     and act[HANDLER_KEY] == rewriteCommands
end

-- vehicleInfo.lua deliberately LEAVES its wrapper installed on unload, because a reload is about
-- to reclaim the slot. This one must do the opposite, and the difference is the kind of wrapper:
-- that one is ADDITIVE (record, then call through), so an orphan is merely useless. This one is
-- SUBSTITUTIVE. An orphan left behind with no live instance points every binding in the game at
-- a dead extension -- the car does not respond and the only cure is restarting BeamNG. Learn mode
-- does not need to survive a reload; the game being playable does.
local function uninstallWrapper()
  local act = core_input_actions
  if type(act) ~= "table" then return end
  local orig = act[ORIG_KEY]
  if type(orig) == "function" then act.actionToCommands = orig end
  act[ORIG_KEY]    = nil
  act[HANDLER_KEY] = nil
  act[WRAP_MARK]   = nil
end

-- =================================================================================================
--  The rewrite
-- =================================================================================================

-- An action name is about to be concatenated into a command string that the engine will execute,
-- so it is vetted rather than escaped. A name that fails is left ALONE -- the binding then fires
-- normally and is simply not learnable, which is the safe direction to fail in.
local function nameIsSafe(actionName)
  return type(actionName) == "string"
     and actionName ~= ""
     and actionName:match("^[%w_%-%.]+$") ~= nil
end

local function actionInfo(actionName, actionCache)
  if actionCache ~= nil then return actionCache end
  if resolving then return nil end
  resolving = true
  local acts = nil
  pcall(function() acts = core_input_actions.getActiveActions() end)
  resolving = false
  if type(acts) ~= "table" then return nil end
  return acts[actionName]
end

local function ctxIsGameEngine(ctx)
  local t = nil
  pcall(function() t = ctx.type end)
  return t ~= nil and t == COMMAND_CONTEXT_TLUA
end

rewriteCommands = function(actionName, actionCache, actsOnChange, onChange,
                           actsOnDown, onDown, actsOnUp, onUp, ctx)
  if not learnActive then return nil end

  -- readActionsFromDisk calls actionToCommands with a cache, purely to read actionMap while it
  -- builds the action table. That pass is not a binding push, discards everything we would
  -- write, and runs while getActiveActions is mid-build. Declining it costs nothing.
  if actionCache ~= nil then return nil end

  if not nameIsSafe(actionName) then
    blLog('W', "Refusing to rewrite unsafe action name: " .. tostring(actionName))
    return nil
  end

  local info = actionInfo(actionName, actionCache)
  local exempt = info ~= nil and EXEMPT_CATS[info.cat] == true

  local hit = "extensions.bindingLearn.hit('" .. actionName .. "',VALUE,"
  local r = {
    actsOnChange = false, onChange = "",
    actsOnDown   = false, onDown   = "",
    -- onUp is dropped on the suppressed path. A release is not a discovery, and announcing both
    -- ends of every press doubles the speech for nothing.
    actsOnUp     = false, onUp     = "",
  }

  if exempt then
    -- Exempt actions still fire, so their announcement has to ride ALONGSIDE the original
    -- command rather than replace it -- which is only possible where both run in the same
    -- context. Every menu action and all six of this mod's own are ctx tlua, so in practice
    -- this always takes the first branch; anything else is left silent rather than risked.
    if not ctxIsGameEngine(ctx) then return nil end
    if not (actsOnChange or actsOnDown) then return nil end
    exemptCount = exemptCount + 1
    r.actsOnChange = actsOnChange
    r.onChange     = actsOnChange and (hit .. "'c') " .. onChange) or ""
    r.actsOnDown   = actsOnDown
    r.onDown       = actsOnDown and (hit .. "'d') " .. onDown) or ""
    r.actsOnUp     = actsOnUp
    r.onUp         = onUp
    return r
  end

  if actsOnChange then
    r.actsOnChange = true
    r.onChange     = hit .. "'c')"
  end
  if actsOnDown or actsOnUp or not actsOnChange then
    -- An action declared with onUp only, or with nothing at all, still gets a down command, so
    -- no binding can become unlearnable through an odd declaration.
    r.actsOnDown = true
    r.onDown     = hit .. "'d')"
  end

  -- The command now has to run in the game engine Lua rather than wherever the original went.
  -- ctx is constructed fresh per call inside actionToCommands, so this is local to this binding.
  pcall(function() ctx.type = COMMAND_CONTEXT_TLUA end)
  rewriteCount = rewriteCount + 1
  return r
end

-- =================================================================================================
--  Naming the control that was pressed
-- =================================================================================================

-- The wrapper sees only the action name, and there is no device token the engine will substitute,
-- so the physical control is resolved after the fact: walk the devices most-recently-used first
-- and take that device's bindings of this action. That is the same rule bnvdaRuntime.js's
-- pickBindingVariants already follows, and pressing the button is itself what promotes its
-- device -- so it is exact for one device and a stated heuristic only when one action is bound
-- on two pads at once.
--
-- Taking the FIRST binding of the action was the other half of the flaky-modifier report, and it
-- survived the modifier fix above: an action is routinely bound twice on the SAME device with
-- different modifiers (this pad has activateStarterMotor on both "dpov" and "modifier1 dpov",
-- and toggleCamera on "modifier1 btn_b" and "modifier2 btn_b"), so pairs order decided whether
-- the modifier was spoken. Two rules replace it, in order:
--   * the held custom modifiers are KNOWN, because customModifierN is now an exempt action and
--     therefore reports through M.hit -- so a candidate whose modifier set matches what is held
--     is not a guess, it is the binding the engine just matched.
--   * where that still leaves several (a keyboard's ctrl/shift/alt are device-level state this
--     extension cannot see, so "ctrl g" and "shift g" are indistinguishable from here), every
--     survivor is named rather than one being picked. Ambiguity that is spoken is recoverable;
--     ambiguity resolved by table order is a confident wrong answer.
local function controlModifierKey(control)
  local mods = {}
  for tok in tostring(control or ""):lower():gmatch("%S+") do
    local n = tok:match("^modifier(%d+)$")
    if n then mods[#mods + 1] = tonumber(n) end
  end
  table.sort(mods)
  return table.concat(mods, ","), #mods
end

local function heldModifierKey()
  local mods = {}
  for n, held in pairs(heldModifiers) do
    if held then mods[#mods + 1] = n end
  end
  table.sort(mods)
  return table.concat(mods, ",")
end

-- An axis is named by the CONTROL, never by which command slot the engine used. customModifierN
-- is declared with onChange -- it is a shoulder button reported through the analogue path -- and
-- classifying on the command would announce "left bumper, axis".
local function controlIsAxis(control)
  local last = nil
  for tok in tostring(control or ""):lower():gmatch("%S+") do last = tok end
  if not last then return false end
  return last:match("axis$") ~= nil
      or last:match("^thumb") ~= nil
      or last:match("^trigger") ~= nil
      or last:match("^slider") ~= nil
end

-- `ignoreHeld` is set for a customModifierN press, and it is not a nicety: the held set is
-- recorded before the resolve (it has to be -- see M.hit), so the modifier's OWN binding, which
-- naturally carries no modifier token, would read as a fall-through and be announced as
-- "modifier 1 plus right bumper" -- the button prefixed with itself. A modifier button is by
-- definition pressed without itself.
local function findControl(actionName, ignoreHeld)
  local vb = extensions.vehicleBindings
  local bindings = core_input_bindings and core_input_bindings.bindings
  if type(vb) ~= "table" or type(bindings) ~= "table" then return nil, nil, false, true end

  local byDev = {}
  local order = {}
  for _, device in ipairs(bindings) do
    if device.devname then byDev[device.devname] = device end
  end
  local recent = nil
  pcall(function() recent = core_input_bindings.getRecentDevices() end)
  if type(recent) == "table" then
    for _, devname in ipairs(recent) do order[#order + 1] = devname end
  end
  -- Tail: every device again, so a pad that has not registered recent activity still answers.
  for _, device in ipairs(bindings) do order[#order + 1] = device.devname end

  local held = (not ignoreHeld) and heldModifierKey() or ""

  for _, devname in ipairs(order) do
    local device   = byDev[devname]
    local contents = device and device.contents
    local list     = contents and contents.bindings
    if type(list) == "table" then
      -- Collect every binding of this action on this device, not the first.
      local cands, exact = {}, {}
      for _, b in ipairs(list) do
        if b.action == actionName and not b.unused then
          local key = controlModifierKey(b.control)
          cands[#cands + 1] = b.control
          if key == held then exact[#exact + 1] = b.control end
        end
      end
      local matched = (#exact > 0)
      local chosen  = matched and exact or cands
      if #chosen > 0 then
        local devKey  = vb.deviceKey(contents.devicetype, devname)
        -- A hit that matched no candidate while a modifier is HELD is the engine falling through
        -- to the bare binding, so the control to name is the combination that was physically
        -- pressed -- not the bare control, which is a different button as far as the user is
        -- concerned. Built as a control string and run through the same formatter, so
        -- "modifier1 rpov" reads as "modifier 1 plus d-pad right" like any other combination.
        local prefix = ""
        if not matched and held ~= "" then
          for n in held:gmatch("[^,]+") do prefix = prefix .. "modifier" .. n .. " " end
        end
        local texts, seen, isAxis = {}, {}, false
        for _, control in ipairs(chosen) do
          local text = vb.controlToSpeech(devKey, prefix .. control)
          if text and not seen[text] then
            seen[text] = true
            texts[#texts + 1] = text
            if controlIsAxis(control) then isAxis = true end
          end
        end
        if #texts > 0 then
          local product = ""
          local info    = core_input_bindings.devices and core_input_bindings.devices[devname]
          if type(info) == "table" then product = tostring(info[2] or "") end
          return table.concat(texts, " or "), product, isAxis, matched
        end
      end
    end
  end
  return nil, nil, false, true
end

-- =================================================================================================
--  The hit handler -- called by the rewritten bindings themselves
-- =================================================================================================

-- One packet per distinct control, carrying every action that control just fired. Grouping by
-- control rather than sending one packet per action is what makes "this button does four things"
-- a single sentence instead of four that talk over each other; grouping is by control and not
-- simply "everything in the window" because a fast two-button press is genuinely two answers.
local function flushHits()
  local hits = pendingHits
  pendingHits = nil
  if not hits then return end

  local groups, order = {}, {}
  for _, h in ipairs(hits) do
    local key = h.control .. " " .. h.device
    local g = groups[key]
    if not g then
      g = { control = h.control, device = h.device, axis = false, items = {} }
      groups[key] = g
      order[#order + 1] = g
    end
    g.axis = g.axis or h.axis
    g.items[#g.items + 1] = {
      title = h.title, desc = h.desc, suppressed = h.suppressed, matched = h.matched,
    }
  end

  for _, g in ipairs(order) do
    -- Holding a modifier over a control that carries no binding for it does NOT mean the control
    -- is unbound as far as the engine is concerned: it falls through and fires the bare binding.
    -- Naming that binding answers a question nobody asked -- "modifier 1 plus d-pad right" is a
    -- different button from "d-pad right", and reporting the right indicator for it is a
    -- confident wrong answer about the combination actually pressed. So a fall-through item is
    -- dropped, and a group left with nothing is reported as unbound rather than not at all
    -- (silence is indistinguishable from the mode being broken, which is the whole failure this
    -- feature is built to avoid).
    --
    -- An EXEMPT fall-through is dropped too, and that is a deliberate call rather than an
    -- oversight. It genuinely fires -- suppression is what makes the others a non-event -- so
    -- this is the one place the "still active" contract is not applied. Keeping it would have
    -- meant almost every direction under a modifier naming a menu action (bare `rpov` alone
    -- carries `menu_item_right`), which is the same wrong answer the drop exists to prevent,
    -- arriving through the exemption. The trade is stated plainly: holding a modifier over the
    -- pause button says "nothing bound" while the pause menu opens.
    local kept = {}
    for _, it in ipairs(g.items) do
      if it.matched then
        kept[#kept + 1] = { title = it.title, desc = it.desc, suppressed = it.suppressed }
      end
    end
    local unbound = (#kept == 0)

    local ok = pcall(function()
      send("LEARN:" .. jsonEncode({
        control = g.control,
        device  = g.device,
        kind    = g.axis and "axis" or "button",
        unbound = unbound and 1 or 0,
        items   = kept,
      }))
    end)
    if not ok then blLog('E', "Failed to encode a learn report.") end
    local names = {}
    for _, it in ipairs(kept) do names[#names + 1] = it.title end
    lastHitText = (g.control ~= "" and g.control or "?") .. " -> "
               .. (unbound and "(nothing bound)" or table.concat(names, " + "))
  end
end

function M.hit(actionName, value, kind)
  -- A command queued a frame before the mode was torn down still arrives. Silence is correct.
  if not learnActive then return end
  if type(actionName) ~= "string" then return end

  local v   = tonumber(value) or 0
  local mag = math.abs(v)
  local now = clockS

  -- Recorded BEFORE the hysteresis gates below, which return early on a release -- and a release
  -- is exactly the edge that clears a modifier. This is the only view this extension has of what
  -- the engine matched, so it has to see both edges. (Keyboard ctrl/shift/alt are device-level
  -- state with no action behind them and never appear here; findControl handles that by naming
  -- every candidate rather than guessing.)
  local modN = actionName:match("^customModifier(%d+)$")
  if modN then heldModifiers[tonumber(modN)] = (mag > 0.5) or nil end

  if kind == 'c' then
    if mag < AXIS_REARM then axisState[actionName] = nil; return end
    if mag < AXIS_MIN_DEFLECTION then return end
    local last = axisState[actionName]
    if last and (now - last) < AXIS_COOLDOWN_S then return end
    axisState[actionName] = now
  else
    if v ~= 1 then return end
    local last = lastHit[actionName]
    if last and (now - last) < BUTTON_REPEAT_S then return end
  end
  lastHit[actionName] = now

  local ok = pcall(function()
    local vb   = extensions.vehicleBindings
    local info = actionInfo(actionName, nil)
    local title, desc = nil, nil
    if type(info) == "table" and type(vb) == "table" then
      title = vb.translate(info.title)
      desc  = vb.translate(info.desc)
    end
    local control, product, isAxis, matched = findControl(actionName, modN ~= nil)
    local exempt = type(info) == "table" and EXEMPT_CATS[info.cat] == true

    pendingHits = pendingHits or {}
    if #pendingHits == 0 then pendingSince = now end
    pendingHits[#pendingHits + 1] = {
      control    = control or "",
      device     = product or "",
      title      = title or actionName,
      desc       = desc or "",
      -- The command slot is not the question: see controlIsAxis.
      axis       = isAxis or (control == nil and kind == 'c'),
      suppressed = exempt and 0 or 1,
      -- False only when a modifier was held and this action has no binding carrying it: the
      -- engine fell through to the bare control. See flushHits.
      matched    = matched ~= false,
    }
  end)
  if not ok then
    blLog('E', "Failed to report hit for " .. tostring(actionName))
  end
end

-- =================================================================================================
--  Entering and leaving
-- =================================================================================================

-- notifyGE -- which clears every *ActionMap and re-pushes all bindings through
-- actionToCommands -- is local to bindings.lua. getUsedBindingsFiles is the cheapest EXPORTED
-- function that reaches it (reloadBindings -> notifyAll -> notifyGE); its own return value is a
-- debug list nobody here wants. onDeviceChanged reaches it too but re-enumerates the hardware.
local function repushBindings()
  local ok = pcall(function()
    if type(core_input_bindings) ~= "table"
       or type(core_input_bindings.getUsedBindingsFiles) ~= "function" then
      error("getUsedBindingsFiles unavailable", 0)
    end
    core_input_bindings.getUsedBindingsFiles()
  end)
  if ok then return true end
  blLog('W', "getUsedBindingsFiles failed; falling back to onDeviceChanged.")
  ok = pcall(function() core_input_bindings.onDeviceChanged() end)
  return ok
end

local function stopLearn(reason)
  -- learnActive goes down FIRST, so that any rewritten command still in flight -- or left
  -- standing by a re-push that fails below -- does nothing instead of talking about a mode
  -- that has ended.
  local wasActive = learnActive
  learnActive = false
  pendingHits = nil
  heldModifiers = {}
  uninstallWrapper()

  if repushBindings() then
    restorePending = false
    if wasActive then
      blLog('I', "Learn mode off (" .. tostring(reason) .. ").")
      send("LEARNMODE:OFF;" .. tostring(reason))
    end
  else
    -- This is the one genuinely dangerous state this feature has: the wrapper is gone but the
    -- engine still holds the commands it wrote. Retry until it takes; do not go quiet.
    restorePending = true
    restoreTimer   = 0.0
    blLog('E', "Learn mode off (" .. tostring(reason) .. ") but the binding re-push FAILED. Retrying.")
    send("LEARNFAIL:norestore;Bindings did not refresh. Retrying.")
  end
end

local function startLearn()
  if learnActive then
    send("LEARNMODE:ON;already on")
    return
  end
  if not installWrapper() then
    blLog('E', "Could not wrap core_input_actions.actionToCommands.")
    send("LEARNFAIL:nowrap;Could not hook the input system. Learn mode not started.")
    return
  end

  learnActive   = true
  startedAt     = clockS
  lastKeepalive = clockS
  lastHit       = {}
  axisState     = {}
  heldModifiers = {}
  pendingHits   = nil
  rewriteCount  = 0
  exemptCount   = 0
  lastHitText   = "none"

  if not repushBindings() then
    learnActive = false
    uninstallWrapper()
    blLog('E', "Could not refresh bindings; learn mode not started.")
    send("LEARNFAIL:norefresh;Could not refresh the bindings. Learn mode not started.")
    return
  end

  blLog('I', "Learn mode on. " .. rewriteCount .. " suppressed, " .. exemptCount .. " left active.")
  send("LEARNMODE:ON;" .. rewriteCount .. " suppressed, " .. exemptCount .. " left active")
end

-- =================================================================================================
--  Diagnostics
--
--  This feature's failure mode is a plausible wrong answer -- the control resolved to the wrong
--  pad, an exemption that did not apply, a wrapper quietly lost to a Lua reload. None of that is
--  visible from the seat, so print what it actually did. Same argument rampGeometry.diag() makes.
-- =================================================================================================

function M.diag()
  local lines = {
    "bindingLearn:",
    "  active:        " .. tostring(learnActive),
    "  wrapper:       " .. (wrapperInstalled() and "installed (ours)" or "NOT ours"),
    "  suppressed:    " .. tostring(rewriteCount),
    "  left active:   " .. tostring(exemptCount),
    "  last hit:      " .. tostring(lastHitText),
    "  held mods:     " .. (heldModifierKey() ~= "" and heldModifierKey() or "none"),
    "  keepalive age: " .. string.sub(tostring(clockS - lastKeepalive), 1, 6) .. " s",
    "  session age:   " .. string.sub(tostring(clockS - startedAt), 1, 6) .. " s",
    "  restore retry: " .. tostring(restorePending),
    "  cmd socket:    " .. (udpCmd and ("listening on " .. CMD_LISTEN_PORT) or "NOT BOUND"),
  }
  for _, l in ipairs(lines) do print(l) end
  return table.concat(lines, "\n")
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
    blLog('I', "UDP send socket targeting " .. PYTHON_HOST .. ":" .. PYTHON_PORT_DATA)
  else
    blLog('E', "Failed to create UDP send socket.")
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
    blLog('I', "UDP command socket listening on port " .. CMD_LISTEN_PORT)
  else
    blLog('E', "Failed to create UDP command socket: " .. tostring(err))
    if udpCmd then pcall(function() udpCmd:close() end) end
    udpCmd = nil
  end
end

-- A failed bind is otherwise permanent for the session, so re-arm it. See implementProximity.lua
-- for the full account of why this is the recovery path rather than a precaution.
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
    blLog('I', "UDP command socket bound on port " .. CMD_LISTEN_PORT .. " after retry.")
  end
end

function M.onExtensionUnloaded()
  -- Restore BEFORE the sockets go, and unconditionally. See uninstallWrapper for why an orphaned
  -- wrapper here is the worst outcome this file has.
  if learnActive or wrapperInstalled() then stopLearn("extension unloaded") end
  if udpSend then pcall(function() udpSend:close() end); udpSend = nil end
  if udpCmd  then pcall(function() udpCmd:close()  end); udpCmd  = nil end
end

function M.onExtensionLoaded()
  setExtensionUnloadMode(M, "manual")
  blLog('I', "Binding learn extension loaded.")
  -- Bind sockets here so a Ctrl+L Lua reload re-opens them.
  setupSockets()
end

function M.onWorldReadyState(state)
  if state == 2 then setupSockets() end
end

-- A level change is not a request to keep the mode, and the safest state across a load is stock.
function M.onClientEndMission()
  if learnActive then stopLearn("level change") end
end

function M.onUpdate(dtReal, dtSim, dtRaw)
  retryCmdBind(dtReal)

  -- One accumulated real-time clock for every timer in this file. Real time rather than
  -- simulated, because everything here -- the keepalive, the axis cooldown, the session cap --
  -- is a fact about the person and their pad, not about the physics. It also makes the sim's
  -- fake clock a plain accumulator rather than a stubbed os.clockhp.
  clockS = clockS + (dtReal or 0)

  if udpCmd then
    repeat
      local data = udpCmd:receive()
      if data then
        local cmd = data:match("^%s*(.-)%s*$"):upper()
        if cmd == "LEARN_ON" then
          lastKeepalive = clockS
          startLearn()
        elseif cmd == "LEARN_OFF" then
          stopLearn("requested")
        elseif cmd == "KEEPALIVE" then
          lastKeepalive = clockS
        elseif cmd == "DIAG" then
          M.diag()
        end
      end
    until not data
  end

  -- The re-push that failed on the way out. Nothing else in this file matters while this is set.
  if restorePending then
    restoreTimer = restoreTimer + (dtReal or 0)
    if restoreTimer >= RESTORE_RETRY_S then
      restoreTimer = 0.0
      if repushBindings() then
        restorePending = false
        blLog('I', "Bindings restored after retry.")
        send("LEARNMODE:OFF;restored after retry")
      end
    end
  end

  if not learnActive then return end

  -- One press, one announcement: see HIT_COALESCE_S.
  if pendingHits and (clockS - pendingSince) >= HIT_COALESCE_S then flushHits() end

  -- The watchdog. If beamtel dies with the mode on, every binding in the game points at a dead
  -- extension and the only cure would be restarting BeamNG. This is what makes that impossible.
  if (clockS - lastKeepalive) > HEARTBEAT_TIMEOUT_S then
    blLog('W', "Keepalive lost; restoring bindings.")
    stopLearn("keepalive lost")
    return
  end
  if (clockS - startedAt) > MAX_SESSION_S then
    stopLearn("time limit")
    return
  end

  -- A Ctrl+L reload of core_input_actions drops the wrapper silently, and the bindings then
  -- revert to stock with the mode still claiming to be on -- a mode that says it is suppressing
  -- and is not. Re-arm, the same way vehicleInfo.lua re-arms its own.
  wrapCheckTimer = wrapCheckTimer + (dtReal or 0)
  if wrapCheckTimer >= WRAP_CHECK_S then
    wrapCheckTimer = 0.0
    if not wrapperInstalled() then
      blLog('W', "Wrapper lost (Lua reload?); re-installing.")
      if installWrapper() and repushBindings() then
        send("LEARNMODE:ON;re-armed after a Lua reload")
      else
        stopLearn("could not re-arm")
      end
    end
  end
end

return M
