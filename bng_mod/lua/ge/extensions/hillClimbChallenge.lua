-- Native mission lifecycle relay for the Proving Grounds hill climb.

local M = {}

local MISSION_ID = "proving_grounds/timeTrial/hill_climb"
local sequence = 0
local attemptId = nil
local active = false
local pendingComplete = false
local checkpointIndex = 0
local recoveryCount = 0

local function foregroundMissionId()
  if gameplay_missions_missionManager
    and gameplay_missions_missionManager.getForegroundMissionId then
    return gameplay_missions_missionManager.getForegroundMissionId()
  end
  return nil
end

local function isOurMission(mission)
  local id = type(mission) == "table" and mission.id or foregroundMissionId()
  return id == MISSION_ID
end

local function makeAttemptId()
  sequence = sequence + 1
  return string.format("%d-%d", os.time(), sequence)
end

local function send(event, fields)
  local bridge = extensions.bnvdaBridge
  if not bridge or not bridge.sendFromGE then return false end
  local payload = {
    type = "challenge_event",
    challenge_id = "hill_climb",
    mission_id = MISSION_ID,
    attempt_id = attemptId,
    event = event,
  }
  for key, value in pairs(fields or {}) do payload[key] = value end
  return bridge.sendFromGE(payload)
end

local function recoveriesForRace(race)
  if not race or type(race.states) ~= "table" then return recoveryCount end
  local vehicleId = be and be:getPlayerVehicleID(0) or -1
  local state = race.states[vehicleId]
  local count = state and tonumber(state.recoveriesUsed) or nil
  if count then recoveryCount = math.max(recoveryCount, count) end
  return recoveryCount
end

function M.onExtensionLoaded()
  setExtensionUnloadMode(M, "manual")
end

function M.onRaceStarted(data)
  if not isOurMission() then return end
  -- Time trials emit once when their intro starts and again when the countdown
  -- releases the vehicle. Replace the short staging trace instead of retaining
  -- it as a separate aborted attempt in BeamTel's report history.
  local replaceActive = active
  attemptId = makeAttemptId()
  active = true
  pendingComplete = false
  checkpointIndex = 0
  recoveryCount = 0
  send("started", {
    race_time_s = tonumber(data and data.time) or 0,
    replace_active = replaceActive,
  })
end

function M.onRacePathnodeReached(data)
  if not active or not isOurMission() then return end
  checkpointIndex = checkpointIndex + 1
  local node = data and data.pathnode or nil
  send("checkpoint", {
    checkpoint_index = checkpointIndex,
    checkpoint_name = node and tostring(node.name or "") or "",
    checkpoint_id = node and tonumber(node.id) or nil,
    race_time_s = tonumber(data and data.time) or 0,
    recovery_count = recoveriesForRace(data and data.race),
  })
end

function M.onRaceComplete(data)
  if not active or not isOurMission() then return end
  pendingComplete = true
  send("race_complete", {
    race_time_s = tonumber(data and data.time) or 0,
    raw_time_s = tonumber(data and data.time) or 0,
    recovery_count = recoveriesForRace(data and data.race),
  })
end

function M.onRaceAborted(data)
  if not active then return end
  send("aborted", {
    race_time_s = tonumber(data and data.time) or 0,
    recovery_count = recoveriesForRace(data and data.race),
  })
  active = false
  pendingComplete = false
end

function M.onMissionAttemptAggregated(attempt, mission, progressKey)
  if not isOurMission(mission) then return end
  if not attemptId then attemptId = makeAttemptId() end
  local values = type(attempt) == "table" and attempt.data or nil
  values = type(values) == "table" and values or {}
  local official = tonumber(values.time)
  local penalty = tonumber(values.penalty) or 0
  local recoveries = tonumber(values.recoveryCount or values.recoveriesUsed)
  if recoveries then recoveryCount = math.max(recoveryCount, recoveries) end
  local newBest = attempt and attempt.newBestTime
  if newBest == nil then newBest = values.newBestTime end
  send("attempt_aggregated", {
    official_time_s = official,
    raw_time_s = official and math.max(0, official - penalty) or nil,
    penalty_s = penalty,
    recovery_count = recoveryCount,
    new_best = newBest,
    progress_key = tostring(progressKey or ""),
  })
  active = false
  pendingComplete = false
end

function M.onAnyMissionChanged(state, mission)
  if state ~= "stopped" or not active or pendingComplete then return end
  if isOurMission(mission) then
    send("mission_stopped", {recovery_count = recoveryCount})
    active = false
  end
end

function M.onExtensionUnloaded()
  if active then send("mission_stopped", {recovery_count = recoveryCount}) end
  active = false
  pendingComplete = false
end

return M
