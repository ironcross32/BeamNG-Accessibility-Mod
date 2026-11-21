-- Session coordinator. All vehicle callbacks carry vehicle ID and an opaque
-- Python activation token, preventing old VMs from reviving an ended session.
local M = {}
local active, send
local diagnostic = false
local lastOffReason
local retired = {}
local function vehicle() return active and scenetree.findObjectById(active.vid) end
local function emit(kind, message, data)
  if active and send then
    active.sequence = (active.sequence or 0) + 1
    send('ASSISTANT:' .. jsonEncode({token = active.token, vehicle = active.vid,
      sequence = active.sequence, active = kind ~= 'off' and active.ready == true,
      kind = kind, message = message or '', data = data}))
  end
end

function M.owns(vid) return active and (not vid or vid == active.vid) or false end
function M.status() return active and active.ready and 'driving assistant' or 'disabled' end
-- The forward route the stop bars are read from; reverse guidance has none ahead.
function M.route()
  return active and active.ready and active.motion ~= 'reverse' and active.route or nil
end

function M.off(reason, data)
  if not active then return end
  lastOffReason = reason or 'Driving assistant off'
  local v = vehicle()
  if v then v:queueLuaCommand(string.format(
    'if extensions.beamtelAssistant then extensions.beamtelAssistant.stop(%q) end', active.token)) end
  emit('off', reason or 'Driving assistant off', data)
  retired[active.token] = true
  active = nil
end

-- A fault is spoken as `spoken` only. The raw error goes to the game log and,
-- as an unspoken `error` event, to bnvdahook.log.
local function fault(raw)
  log('E', 'drivingAssistant', tostring(raw))
  emit('error', '', {detail = tostring(raw)})
end

function M.fail(spoken, err)
  if not active then log('E', 'drivingAssistant', tostring(err)); return end
  fault(err)
  M.off(spoken)
end

local function packet()
  local state = active.route
  local value = {path = state.path, distance = state.distance, clear = not state.committed,
    recovering = state.recovering, hazard = state.hazard, roadMessage = state.roadMessage, diagnostic = diagnostic,
    motion = active.motion or 'forward', motionRevision = active.motionRevision or 0,
    reverseGuidance = state.reverseGuidance, replanRevision = active.replanRevision or 0}
  active.options = {}
  if active.motion ~= 'reverse' and state.junction and not state.committed then
    local junction = state.junction
    value.junction = {id = junction.id, unsupported = junction.unsupported, choices = {}, access = {}}
    for name in pairs(junction.choices) do
      local candidate = {records = {}, junction = junction, generation = state.generation,
        travelled = state.travelled, history = state.history}
      for _, record in ipairs(state.records) do candidate.records[#candidate.records + 1] = record end
      local chosen = extensions.roadDetector.assistantChoose(candidate, name)
      if chosen then
        local path = require('ge/extensions/assistantRoutes').nodes(chosen.records)
        if path then
          value.junction.choices[name] = path
          value.junction.access[name] = junction.choices[name].access
          active.options[name] = chosen
        end
      end
    end
  end
  return value
end

-- The join stage: stock AI drives at no more than JOIN_SPEED_MPS, and hands
-- over once the car is within JOIN_MARGIN_M of the road edge and within
-- JOIN_ANGLE of its legal direction. It gives up after JOIN_STALL_S stopped
-- or JOIN_LIMIT_S in all.
local JOIN_SPEED_MPS, JOIN_MARGIN_M, JOIN_ANGLE, JOIN_STALL_S, JOIN_LIMIT_S = 10, 1, 30, 15, 180

-- ================================================================================================
--  Following the map's route
-- ================================================================================================
-- Stock clears a route when under 7 m of its path remains, so that is arrival
-- here too. Stock's own 7 m includes the final leg off the road, which is why a
-- destination away from the road is never cleared by the game.
local ARRIVE_M = 7
local ROUTE_REPORT_S = 0.5
local ROUTE_MOVED_M = 1
-- A route that keeps moving (a mission's) must not search on every tick.
local ROUTE_SEARCH_MIN_S = 2

-- The destination, whether a route is still being computed, and the last road
-- node the stock route names. The last path node is always a real position, for
-- the reason routeBeacon.lua gives.
local function liveDestination()
  local gm = core_groundMarkers
  if not gm or not gm.currentlyHasTarget or not gm.currentlyHasTarget() then return nil, false end
  local path = gm.routePlanner and gm.routePlanner.path
  local count = type(path) == 'table' and #path or 0
  if count < 1 or not path[count].pos then return nil, true end
  local hint
  for i = count, 1, -1 do
    if path[i].wp then hint = path[i].wp; break end
  end
  return path[count].pos, false, hint
end

local function flatDistance(a, b)
  local dx, dy = a.x - b.x, a.y - b.y
  return math.sqrt(dx * dx + dy * dy)
end

-- Only a route the big map set is ours to clear. One a career or delivery system
-- set belongs to that system, and it asked not to be cleared on arrival.
local function clearOwnedRoute()
  local gm = core_groundMarkers
  if not gm or not gm.currentlyHasTarget or not gm.currentlyHasTarget() then return true end
  if not gm.clearPathOnReachingTarget then return false end
  if freeroam_bigMapMode and freeroam_bigMapMode.setNavFocus then
    freeroam_bigMapMode.setNavFocus(nil)
  else
    gm.setPath(nil)
  end
  return true
end

local function guidanceData(player, guidance)
  guidance.speed = player:getVelocity():length()
  return guidance
end

local function arrive(player, reachedByGame)
  local nav = active.route.nav
  local arrival = {offRoad = nav.offRoad and not reachedByGame or false}
  if arrival.offRoad then
    local position, facing, destination = player:getPosition(), player:getDirectionVector(), nav.destination
    local dx, dy = destination.x - position.x, destination.y - position.y
    arrival.distance = math.sqrt(dx * dx + dy * dy)
    -- Positive is left, the same reading as the join stage's bearing.
    arrival.bearing = math.deg(math.atan2(facing.x * dy - facing.y * dx, facing.x * dx + facing.y * dy))
    arrival.x, arrival.y = destination.x, destination.y
  end
  local ok, err = pcall(clearOwnedRoute)
  if not ok then log('E', 'drivingAssistant', tostring(err)) end
  M.off('Destination reached. Assistant off', {arrival = arrival})
end

-- Speech before the `on` event would be interrupted by it, so it waits.
local function announce(kind, message, data)
  if active.ready then emit(kind, message, data); return end
  active.pending = active.pending or {}
  active.pending[#active.pending + 1] = {kind, message, data}
end

-- Compares the live route to the one being followed. Returns false once the
-- session has ended. `starting` means the vehicle has not been given a path yet,
-- so it starts on this one; every other change has to restart its planner.
local function syncRoute(player, starting)
  local function replan()
    if not starting then active.replanRevision = (active.replanRevision or 0) + 1 end
  end
  local rd = extensions.roadDetector
  if not rd.assistantSetDestination then return true end
  local state = active.route
  local nav = state.nav
  local destination, pending, hint = liveDestination()
  if pending then return true end
  local speed = player:getVelocity():length()
  if not destination then
    active.ignoredDestination, active.failedDestination, active.routeSearchAge = nil, nil, nil
    if nav then
      local ok, err = rd.assistantClearDestination(state, speed)
      if not ok then M.off(err .. '. Steering assistance off'); return false end
      replan()
      active.selected, active.chosen = nil, nil
      announce('route_clear', 'Route cleared. Turns are yours')
    end
    return true
  end
  local function same(other) return other and flatDistance(destination, other) <= ROUTE_MOVED_M end
  if same(nav and nav.destination) or same(active.ignoredDestination) then return true end
  if same(active.failedDestination) and not active.retryRoute then return true end
  if (active.routeSearchAge or math.huge) < ROUTE_SEARCH_MIN_S then return true end
  active.routeSearchAge, active.retryRoute = 0, false
  -- Already there: nothing to follow, and the assistant must still engage.
  local function alreadyThere()
    if state.nav then
      local ok, err = rd.assistantClearDestination(state, speed)
      if not ok then M.off(err .. '. Steering assistance off'); return false end
      replan()
    end
    local ok, cleared = pcall(clearOwnedRoute)
    if not ok or not cleared then
      active.ignoredDestination = vec3(destination.x, destination.y, destination.z)
    end
    announce('say', 'Already at the destination')
    return true
  end
  if flatDistance(destination, player:getPosition()) <= ARRIVE_M then return alreadyThere() end
  local ok, err = rd.assistantSetDestination(state, destination, speed, hint)
  if not ok then
    if err == 'Disconnected navigation route' then M.off(err .. '. Steering assistance off'); return false end
    active.failedDestination = vec3(destination.x, destination.y, destination.z)
    announce('say', err .. '. Turns are yours')
    return true
  end
  active.failedDestination = nil
  local guidance = rd.assistantGuidance(state)
  if guidance.destDistance <= ARRIVE_M then return alreadyThere() end
  replan()
  announce('route_set', '', guidanceData(player, guidance))
  active.routeReportAge = 0
  return true
end

local function startAssisted(player, state, roadMessage)
  local speed = player:getVelocity():length()
  if roadMessage then state.roadMessage = roadMessage end
  active.route = state
  if not syncRoute(player, true) then return end
  local value = packet()
  value.chooseNow = state.junction and state.distance <= math.max(12, math.min(35, speed * 2))
  player:queueLuaCommand(string.format(
    'extensions.load("beamtelAssistant"); extensions.beamtelAssistant.start(%q,%s)', active.token, serialize(value)))
end

-- ================================================================================================
--  Turning round before the route
-- ================================================================================================
-- Only at switch-on and nearly stopped: the player has to be ready to work the
-- gears, and a car already rolling down the road should simply carry on.
local TURN_MAX_START_MPS = 2

local function maybeTurnRound(player, state)
  local rd = extensions.roadDetector
  if not rd.assistantTurnChoice or player:getVelocity():length() >= TURN_MAX_START_MPS then return false end
  local destination, _, hint = liveDestination()
  if not destination or flatDistance(destination, player:getPosition()) <= ARRIVE_M then return false end
  local choice = rd.assistantTurnChoice(state, destination, hint, player)
  if not choice or not choice.turn then return false end
  local record = state.records[1]
  local reading = rd.assistantTurnReading(player, record)
  local routes = require('ge/extensions/assistantRoutes')
  local turn = routes.turnBegin(reading.err, reading.front, reading.rear, choice.left, choice.right)
  active.turning = {turn = turn, record = record, gear = 0}
  emit('say', 'Three point turn. ' .. routes.turnPhrase(turn.phase), {protect = true})
  player:queueLuaCommand(string.format(
    'extensions.load("beamtelAssistant"); extensions.beamtelAssistant.turn(%q)', active.token))
  return true
end

local function finishTurn(player)
  active.turning = nil
  local rd = extensions.roadDetector
  local state, err = rd.assistantStart(player)
  if state then state, err = rd.assistantUpdate(state, player) end
  if not state then M.off(err .. '. Steering assistance off'); return end
  startAssisted(player, state)
end

local function updateTurn(player, dt)
  local turning = active.turning
  local routes = require('ge/extensions/assistantRoutes')
  local input = extensions.roadDetector.assistantTurnReading(player, turning.record)
  input.gear, input.dt = turning.gear, dt
  local step = routes.turnStep(turning.turn, input)
  if step.say then emit('say', step.say, {protect = true}) end
  if step.fail then
    -- The forward route is still there: the turn was only ever the shorter way.
    emit('say', step.fail, {protect = true})
    finishTurn(player)
  elseif step.done then
    finishTurn(player)
  else
    player:queueLuaCommand(string.format(
      'if extensions.beamtelAssistant then extensions.beamtelAssistant.setTurn(%q,%d) end',
      active.token, step.steering))
  end
end

local function updateJoin(player, dt)
  local joining = active.joining
  joining.seconds = joining.seconds + dt
  joining.stalled = player:getVelocity():length() < 0.5 and joining.stalled + dt or 0
  local rd = extensions.roadDetector
  local state = rd.assistantStart(player, JOIN_MARGIN_M, JOIN_ANGLE)
  if state then
    local err
    state, err = rd.assistantUpdate(state, player)
    if not state then M.off(err .. '. Brake now. Driving assistant off'); return end
    active.joining = nil
    startAssisted(player, state, 'On the road. Throttle and brake are yours')
  elseif joining.stalled > JOIN_STALL_S or joining.seconds > JOIN_LIMIT_S then
    M.off('Could not reach a road. Brake now. Driving assistant off')
  end
end

function M.command(command, token, responder)
  send = responder
  if command == 'ASSISTANT_DIAG_ON' or command == 'ASSISTANT_DIAG_OFF' then
    diagnostic = command == 'ASSISTANT_DIAG_ON'
    return
  end
  if command == 'ASSISTANT_HEARTBEAT' then
    if active and active.token == token then
      active.lease = 0
      emit('alive')
      local v = vehicle()
      if v then v:queueLuaCommand(string.format(
        'if extensions.beamtelAssistant then extensions.beamtelAssistant.heartbeat(%q) end', token)) end
    end
    return
  end
  if command == 'ASSISTANT_OFF' then
    if not token or (active and active.token == token) then M.off() end
    return
  end
  if command == 'ASSISTANT_STATUS' then
    local message = active and (active.ready and 'Driving assistant on'
      or active.joining and 'Driving assistant joining the road. AI has full control'
      or active.turning and 'Driving assistant turning round'
      or 'Driving assistant starting') or 'Driving assistant off'
    if not token and active then emit('status', message)
    else responder('ASSISTANT:' .. jsonEncode({token = token, kind = 'status', message = message})) end
    return
  end
  if command ~= 'ASSISTANT_TOGGLE' then return end
  if not token or not token:match('^[%w%-]+$') or #token > 80 then return end
  if retired[token] then return end
  if active then
    if active.token == token then M.off() end
    return
  end
  local player = be:getPlayerVehicle(0)
  active = {vid = player and player:getID() or -1, token = token, lease = 0, age = 0, tick = 0}
  lastOffReason = nil
  if not player then M.off('No player vehicle. Driving assistant off'); return end
  local rd = extensions.roadDetector
  if not rd or not rd.assistantStart then M.off('Navigation extension unavailable. Driving assistant off'); return end
  -- Stage one: on the road means inside its edge. The verge counts only when
  -- the join stage is unavailable.
  local state, err = rd.assistantStart(player, rd.assistantJoinTarget and JOIN_MARGIN_M or nil)
  if state then
    state, err = rd.assistantUpdate(state, player)
    if not state then M.off(err .. '. Steering assistance off'); return end
    if not maybeTurnRound(player, state) then startAssisted(player, state) end
    return
  end
  if err == 'Navigation is unavailable' or not rd.assistantJoinTarget then
    M.off(err .. '. Steering assistance off'); return
  end
  -- Not on a road, or facing off it: stock AI drives the whole car there first.
  local join, joinErr = rd.assistantJoinTarget(player)
  if not join then M.off(joinErr .. '. Steering assistance off'); return end
  active.joining = {seconds = 0, stalled = 0}
  local where = join.distance < 2 and 'Turning onto the road'
    or string.format('Driving to the road, %d metres %s', math.floor(join.distance + 0.5),
      math.abs(join.bearing) <= 30 and 'ahead' or math.abs(join.bearing) >= 150 and 'behind'
      or join.bearing > 0 and 'to your left' or 'to your right')
  emit('say', 'Off road. AI has full control. ' .. where)
  player:queueLuaCommand(string.format(
    'extensions.load("beamtelAssistant"); extensions.beamtelAssistant.join(%q,%q,%s)',
    token, join.target, JOIN_SPEED_MPS))
end

function M.event(vid, token, kind, message)
  if not active or active.vid ~= vid or active.token ~= token then return end
  if be:getPlayerVehicleID(0) ~= vid then M.off('Vehicle changed. Steering assistance off'); return end
  active.age = 0
  if kind == 'error' then fault(message)
  elseif kind == 'off' then lastOffReason = message; emit('off', message); retired[active.token] = true; active = nil
  elseif kind == 'gear' then
    if active.turning then active.turning.gear = tonumber(message) or 0 end
  elseif kind == 'ready' then
    active.ready = true
    emit('on', message)
    for _, item in ipairs(active.pending or {}) do emit(item[1], item[2], item[3]) end
    active.pending = nil
  elseif kind == 'motion' and (message == 'reverse' or message == 'forward') then
    if active.motion == message then return end
    active.motion = message
    active.motionRevision = (active.motionRevision or 0) + 1
    if message == 'forward' then
      local state, err = extensions.roadDetector.assistantStart(vehicle())
      if not state then M.off(err .. '. Steering assistance off'); return end
      active.route, active.selected, active.chosen = state, nil, nil
      -- A fresh route from where the car now faces: the old one may not fit it.
      active.failedDestination, active.routeSearchAge = nil, nil
    end
    emit('motion', '', {motion=message})
  elseif kind == 'choice' then
    if active.motion == 'reverse' then return end
    local choice = active.options[message]
    if not choice then M.off('Unresolvable junction choice. Steering assistance off'); return end
    active.route = choice
    active.chosen = message
    active.retryRoute = true
    emit('choice', '', {chosen = message})
  elseif kind == 'junction' then
    active.selected = nil
    if active.ready then emit('junction', message) end
  elseif kind == 'selection' then
    if active.ready and active.options[message] then
      active.selected = message
      emit('selection', '', {selected = message})
    end
  elseif kind == 'diagnostic' and diagnostic then
    local ok, state = pcall(jsonDecode, message)
    if ok and type(state) == 'table' then
      emit('diagnostic', '', {vehicle = state, navigation = M.diagnosticState()})
    end
  elseif kind == 'say' or kind == 'slow' then
    if active.ready then emit(kind, message) end
  end
end

function M.update(dt)
  if not active then return end
  active.lease, active.age, active.tick = active.lease + dt, active.age + dt, active.tick + dt
  if active.lease > 3 or active.age > 2 then M.off('Connection lost. Steering assistance off'); return end
  if be:getPlayerVehicleID(0) ~= active.vid then M.off('Vehicle changed. Steering assistance off'); return end
  if active.tick < 0.05 then return end
  local elapsed = active.tick
  active.tick = 0
  local player = vehicle()
  if not player then M.off('Vehicle unavailable. Steering assistance off'); return end
  if active.joining then updateJoin(player, elapsed); return end
  if active.turning then updateTurn(player, elapsed); return end
  local state, err
  if active.motion == 'reverse' then
    state, err = extensions.roadDetector.assistantReverse(active.route, player)
  else
    state, err = extensions.roadDetector.assistantUpdate(active.route, player, elapsed)
  end
  if not state then M.off(err .. '. Steering assistance off'); return end
  if active.motion ~= 'reverse' then
    active.routeSearchAge = active.routeSearchAge and active.routeSearchAge + elapsed
    if not syncRoute(player) then return end
    local guidance = extensions.roadDetector.assistantGuidance
      and extensions.roadDetector.assistantGuidance(active.route)
    if guidance then
      if guidance.destDistance <= ARRIVE_M then arrive(player); return end
      active.routeReportAge = (active.routeReportAge or 0) + elapsed
      if active.ready and active.routeReportAge >= ROUTE_REPORT_S then
        active.routeReportAge = 0
        emit('route', '', guidanceData(player, guidance))
      end
    end
  end
  local value = packet()
  player:queueLuaCommand(string.format(
    'if extensions.beamtelAssistant then extensions.beamtelAssistant.setPlan(%q,%s) end', active.token, serialize(value)))
end

-- The game's own arrival (stock onReachedTargetPos), which also tells it apart
-- from the player removing the route: both leave no route on the next tick.
function M.reached()
  if not active or not active.route or not active.route.nav or active.motion == 'reverse' then return end
  local player = vehicle()
  if player then arrive(player, true) end
end

function M.diagnosticState()
  if not active then return {active = false, diagnostic = diagnostic, lastOffReason = lastOffReason} end
  local route = active.route
  if active.turning then
    local turn = active.turning.turn
    return {active = false, turning = true, vehicle = active.vid, phase = turn.phase, moves = turn.moves,
      waiting = turn.waiting, gear = active.turning.gear, leaseAge = active.lease, vehicleAge = active.age}
  end
  if active.joining then
    return {active = false, joining = true, vehicle = active.vid, joinSeconds = active.joining.seconds,
      joinStalled = active.joining.stalled, leaseAge = active.lease, vehicleAge = active.age}
  end
  if not route then return {active = false, starting = true, vehicle = active.vid} end
  local choices = {}
  for _, name in ipairs({'left', 'straight', 'right'}) do
    if active.options and active.options[name] then choices[#choices + 1] = name end
  end
  local nav = route.nav
  local guidance = nav and extensions.roadDetector.assistantGuidance(route)
  return {active = active.ready, vehicle = active.vid, path = route.path,
    routeFollowing = nav ~= nil, routeSearchSeconds = nav and nav.searchSeconds,
    routeNodes = nav and #nav.records, routeTurns = nav and #nav.turns,
    routeGuidance = guidance, replanRevision = active.replanRevision or 0,
    distance = route.distance, offset = route.offset, recovering = route.recovering,
    hazard = route.hazard, hazardDetail = route.hazardDetail,
    junction = route.junction and route.junction.id, selected = active.selected,
    choices = choices, chosen = active.chosen, committed = route.committed,
    motion = active.motion or 'forward', motionRevision = active.motionRevision or 0,
    reverseGuidance = route.reverseGuidance,
    diagnostic = diagnostic, leaseAge = active.lease, vehicleAge = active.age}
end

return M
