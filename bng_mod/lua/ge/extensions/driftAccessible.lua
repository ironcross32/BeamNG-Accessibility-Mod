-- Speaks the stock drift display. The game's drift system raises GE hooks for
-- everything its UI shows; this relays the ones the display presents, while it
-- presents them, to Python over bnvdaBridge. See docs/lua-drift.md.

local M = {}

-- Contexts in which the game shows a drift display (display.lua is loaded).
local SHOWN_CONTEXTS = {inFreeroamCruising = true, inFreeroamChallenge = true}

local shown = false
local context = nil
local comboWhole = 1
local lastPotential = 0
local outOfBounds = false

local function send(event, fields)
  local bridge = extensions.bnvdaBridge
  if not bridge or not bridge.sendFromGE then return false end
  local payload = {type = "drift_event", event = event, context = context}
  for key, value in pairs(fields or {}) do payload[key] = value end
  return bridge.sendFromGE(payload)
end

-- The game's own text, with its {{name}} placeholders filled. English fallback
-- if the key is missing, so a game update degrades to plain words.
local function tr(key, fallback, vars)
  local ok, text = pcall(translateLanguage, key, fallback, true)
  if not ok or type(text) ~= "string" or text == "" or text == key then text = fallback end
  for name, value in pairs(vars or {}) do
    text = text:gsub("{{%s*" .. name .. "%s*}}", function() return tostring(value) end)
  end
  return text
end

local function mine(vehId)
  if vehId == nil then return true end
  return be and tonumber(vehId) == be:getPlayerVehicleID(0)
end

local function scoreFor(vehId)
  local scoring = gameplay_drift_scoring
  if not scoring or not scoring.getScore then return nil end
  local ok, score = pcall(scoring.getScore, vehId)
  return ok and type(score) == "table" and score or nil
end

-- Hooks run inside the game's drift update; never let ours break it.
local function guarded(fn)
  return function(...)
    local ok, err = pcall(fn, ...)
    if not ok then log('E', 'driftAccessible', tostring(err)) end
  end
end

local function setContext(newContext)
  context = newContext
  local nowShown = SHOWN_CONTEXTS[newContext] == true
  if nowShown and not shown then
    shown = true
    local score = scoreFor(nil) or {}
    comboWhole = math.floor(tonumber(score.combo) or 1)
    send("shown", {points = math.floor(tonumber(score.cachedScore) or 0),
      combo = tonumber(score.combo) or 1})
  elseif not nowShown and shown then
    shown = false
    outOfBounds = false
    send("hidden")
  end
end

M.onDriftContextChanged = guarded(function(newContext)
  setContext(newContext)
end)

M.onDriftStatusChanged = guarded(function(isDrifting, direction, vehId)
  if not shown or not isDrifting or not mine(vehId) then return end
  if direction == "left" or direction == "right" then send("slide", {direction = direction}) end
end)

M.onDriftNewCombo = guarded(function(data, vehId)
  if not shown or not mine(vehId) or type(data) ~= "table" then return end
  local whole = math.floor(tonumber(data.newCombo) or 0)
  if whole > comboWhole then
    comboWhole = whole
    send("combo", {combo = whole})
  end
end)

-- Not lastPotential: on a spinout, scoring's own fail handler clears the cached
-- score first, and this reset hook runs nested inside it, before our fail hook.
-- Zeroing here made the first driven spinout say no points lost.
M.onDriftCachedScoreReset = guarded(function(vehId)
  if mine(vehId) then comboWhole = 1 end
end)

-- The fail hooks can arrive after scoring has already cleared the cached score,
-- so the points lost are the last potential score seen. Scoring re-raises
-- onDriftScoreChanged with 0 the frame after a reset, so this never goes stale.
M.onDriftScoreChanged = guarded(function(driftScore, vehId)
  if mine(vehId) and type(driftScore) == "table" then
    lastPotential = math.floor(tonumber(driftScore.potentialScore) or 0)
  end
end)

M.onNewDriftTierReached = guarded(function(tier, vehId)
  if not shown or not mine(vehId) or type(tier) ~= "table" then return end
  local name = tier.name or tr("missions.drift.tiers." .. tostring(tier.id), "Drift!")
  send("tier", {text = tostring(name)})
end)

M.onDriftQuickMessageDisplay = guarded(function(data)
  if not shown or type(data) ~= "table" or not mine(data.vehId) then return end
  if type(data.msg) == "string" and data.msg ~= "" then send("quick", {text = data.msg}) end
end)

local function failed(vehId, key, fallback)
  if not shown or not mine(vehId) then return end
  -- The display keeps "Pts: total" up through a fail, so read it with the loss.
  local score = scoreFor(vehId) or {}
  send("fail", {text = tr(key, fallback), lost = lastPotential,
    total = math.floor(tonumber(score.score) or 0)})
  lastPotential = 0
end

M.onDriftCrash = guarded(function(vehId) failed(vehId, "missions.drift.display.crashed", "Crashed!") end)
M.onDriftSpinout = guarded(function(vehId) failed(vehId, "missions.drift.display.spinout", "Spinout!") end)

M.onDriftCompletedScored = guarded(function(data)
  if not shown or type(data) ~= "table" or not mine(data.vehId) then return end
  local score = scoreFor(data.vehId) or {}
  send("scored", {points = math.floor(tonumber(data.addedScore) or 0),
    total = math.floor(tonumber(score.score) or 0)})
end)

M.onDriftScoreWrappedUp = guarded(function(points, vehId)
  if not shown or not mine(vehId) then return end
  send("wrapup", {points = math.floor(tonumber(points) or 0)})
end)

-- Drift spots: free-roam zones with a start line. The name, high score and
-- objectives come from saveLoad; the line id is the only thing the hook carries.
local function spotForLine(lineId)
  local saveLoad = gameplay_drift_saveLoad
  if not saveLoad or not saveLoad.getDriftSpotsById then return nil end
  for _, spot in pairs(saveLoad.getDriftSpotsById() or {}) do
    for _, line in pairs(type(spot.lines) == "table" and spot.lines or {}) do
      if type(line) == "table" and line.id == lineId then return spot end
    end
  end
  return nil
end

M.onDriftSpotStarted = guarded(function(data)
  local fields = {}
  local ok, spot = pcall(spotForLine, type(data) == "table" and data.lineId or nil)
  if ok and spot then
    fields.name = spot.info and spot.info.name and tr(spot.info.name, spot.info.name) or nil
    local best = spot.saveData and spot.saveData.scores and spot.saveData.scores[1]
    fields.highScore = best and math.floor(tonumber(best.score) or 0) or 0
    local goals = {}
    for _, objective in ipairs(spot.info and spot.info.objectives or {}) do
      local done = spot.saveData and spot.saveData.objectivesCompleted
        and spot.saveData.objectivesCompleted[objective.id]
      goals[#goals + 1] = {points = math.floor(tonumber(objective.score) or 0), done = done == true}
    end
    fields.goals = goals
  end
  send("spot_start", fields)
end)

M.onDriftSpotCompleted = guarded(function(data)
  if type(data) ~= "table" then return end
  local score = math.floor(tonumber(data.score) or 0)
  local text = data.newRecord
    and tr("missions.drift.display.newRecord", "New record! Score: {{score}}", {score = score})
    or tr("missions.drift.display.driftZoneFinished", "Drift zone finished! Score: {{score}}", {score = score})
  send("spot_done", {text = text})
end)

M.onDriftSpotFailed = guarded(function(data)
  local key = type(data) == "table" and type(data.reason) == "table" and data.reason.msg or nil
  send("spot_failed", {text = key and tr(key, "Drift zone ended") or "Drift zone ended"})
end)

M.onFreeroamDriftZoneNewHighscore = guarded(function()
  send("spot_highscore", {text = tr("missions.drift.display.newHighscore", "New Highscore!")})
end)

-- Out of bounds is a state the display polls, not a hook; follow its edges.
-- While shown, a silent keepalive tells Python the display is still up, since a
-- long single slide can go many seconds without another event.
local KEEPALIVE_S = 3
local keepaliveTimer = 0
M.onUpdate = guarded(function(dtReal)
  if not shown then keepaliveTimer = 0; return end
  keepaliveTimer = keepaliveTimer + (dtReal or 0)
  if keepaliveTimer >= KEEPALIVE_S then keepaliveTimer = 0; send("alive") end
  if context ~= "inFreeroamChallenge" then return end
  local bounds = gameplay_drift_bounds
  local now = bounds and bounds.getIsOutOfBounds and bounds.getIsOutOfBounds() or false
  if now and not outOfBounds then
    send("out_of_bounds", {text = tr("missions.crawl.general.outOfBounds", "Out of bounds!")})
  end
  outOfBounds = now and true or false
end)

function M.onExtensionLoaded()
  setExtensionUnloadMode(M, "manual")
  -- Adopt the current state silently: a reload mid-drift is not a new display.
  context = gameplay_drift_general and gameplay_drift_general.getContext() or nil
  shown = SHOWN_CONTEXTS[context] == true
end

function M.onExtensionUnloaded()
  if shown then send("hidden") end
  shown = false
end

function M.state()
  return {shown = shown, context = context, comboWhole = comboWhole, lastPotential = lastPotential}
end

return M
