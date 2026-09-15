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

function M.off(reason)
  if not active then return end
  lastOffReason = reason or 'Driving assistant off'
  local v = vehicle()
  if v then v:queueLuaCommand(string.format(
    'if extensions.beamtelAssistant then extensions.beamtelAssistant.stop(%q) end', active.token)) end
  emit('off', reason or 'Driving assistant off')
  retired[active.token] = true
  active = nil
end

local function packet()
  local state = active.route
  local value = {path = state.path, distance = state.distance, clear = not state.committed,
    recovering = state.recovering, hazard = state.hazard, roadMessage = state.roadMessage, diagnostic = diagnostic,
    motion = active.motion or 'forward', motionRevision = active.motionRevision or 0,
    reverseGuidance = state.reverseGuidance}
  active.options = {}
  if active.motion ~= 'reverse' and state.junction and not state.committed then
    local junction = state.junction
    value.junction = {id = junction.id, unsupported = junction.unsupported, choices = {}}
    for name in pairs(junction.choices) do
      local candidate = {records = {}, junction = junction, generation = state.generation,
        travelled = state.travelled, history = state.history}
      for _, record in ipairs(state.records) do candidate.records[#candidate.records + 1] = record end
      local chosen = extensions.roadDetector.assistantChoose(candidate, name)
      if chosen then
        local path = require('ge/extensions/assistantRoutes').nodes(chosen.records)
        if path then
          value.junction.choices[name] = path
          active.options[name] = chosen
        end
      end
    end
  end
  return value
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
    local message = active and (active.ready and 'Driving assistant on' or 'Driving assistant starting')
      or 'Driving assistant off'
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
  local state, err = rd.assistantStart(player)
  if state then state, err = rd.assistantUpdate(state, player) end
  if not state then M.off(err .. '. Steering assistance off'); return end
  local speed = player:getVelocity():length()
  active.route = state
  local value = packet()
  value.chooseNow = state.junction and state.distance <= math.max(12, math.min(35, speed * 2))
  player:queueLuaCommand(string.format(
    'extensions.load("beamtelAssistant"); extensions.beamtelAssistant.start(%q,%s)', token, serialize(value)))
end

function M.event(vid, token, kind, message)
  if not active or active.vid ~= vid or active.token ~= token then return end
  if be:getPlayerVehicleID(0) ~= vid then M.off('Vehicle changed. Steering assistance off'); return end
  active.age = 0
  if kind == 'off' then lastOffReason = message; emit('off', message); retired[active.token] = true; active = nil
  elseif kind == 'ready' then active.ready = true; emit('on', message)
  elseif kind == 'motion' and (message == 'reverse' or message == 'forward') then
    if active.motion == message then return end
    active.motion = message
    active.motionRevision = (active.motionRevision or 0) + 1
    if message == 'forward' then
      local state, err = extensions.roadDetector.assistantStart(vehicle())
      if not state then M.off(err .. '. Steering assistance off'); return end
      active.route, active.selected, active.chosen = state, nil, nil
    end
    emit('motion', '', {motion=message})
  elseif kind == 'choice' then
    if active.motion == 'reverse' then return end
    local choice = active.options[message]
    if not choice then M.off('Unresolvable junction choice. Steering assistance off'); return end
    active.route = choice
    active.chosen = message
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
  local state, err
  if active.motion == 'reverse' then
    state, err = extensions.roadDetector.assistantReverse(active.route, player)
  else
    state, err = extensions.roadDetector.assistantUpdate(active.route, player, elapsed)
  end
  if not state then M.off(err .. '. Steering assistance off'); return end
  local value = packet()
  player:queueLuaCommand(string.format(
    'if extensions.beamtelAssistant then extensions.beamtelAssistant.setPlan(%q,%s) end', active.token, serialize(value)))
end

function M.diagnosticState()
  if not active then return {active = false, diagnostic = diagnostic, lastOffReason = lastOffReason} end
  local route = active.route
  if not route then return {active = false, starting = true, vehicle = active.vid} end
  local choices = {}
  for _, name in ipairs({'left', 'straight', 'right'}) do
    if active.options and active.options[name] then choices[#choices + 1] = name end
  end
  return {active = active.ready, vehicle = active.vid, path = route.path,
    distance = route.distance, offset = route.offset, recovering = route.recovering,
    hazard = route.hazard, hazardDetail = route.hazardDetail,
    junction = route.junction and route.junction.id, selected = active.selected,
    choices = choices, chosen = active.chosen, committed = route.committed,
    motion = active.motion or 'forward', motionRevision = active.motionRevision or 0,
    reverseGuidance = route.reverseGuidance,
    diagnostic = diagnostic, leaseAge = active.lease, vehicleAge = active.age}
end

return M
