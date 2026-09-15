-- Steering stays in the vehicle VM. GE sends explicit paths, not steering frames.
local M = {}
local session, saved, plan
local lease, planAge, reportAge, brakeTime, cueAge = 0, 0, 0, 0, 8
local announced, locked, pathKey, suspended = nil, nil, nil, false
local retired = {}
local endpoint, recovering, hazardAge = nil, false, 8
local plannerStarts, pathExtensions = 0, 0
local lastChoice, lastRequested, choiceAutomatic, lastOffReason
local standstillTime = 0
local reversing, motionWaiting, motionRevision = false, false, 0
local reverseWheelbase, reverseBlocked, forwardSpeed = 3, false, 0
local selection = {junction = nil, direction = nil, held = nil, seconds = 0, reported = false}
local controls = {'steering', 'throttle', 'brake', 'parkingbrake', 'clutch'}

local function callback(kind, message)
  if not session then return end
  obj:queueGameEngineLua(string.format(
    'if extensions.beamtelAI then extensions.beamtelAI.onAssistantEvent(%d,%q,%q,%q) end',
    obj:getID(), session, kind, message or ''))
end

local function preserveGearboxCall(fn, ...)
  -- Switching to realistic mode can put first gear into neutral and reset a
  -- shift in progress. Block AI's synchronous setter calls instead of undoing
  -- them afterwards. The player's setter is restored before the next frame.
  local main = controller.mainController
  local original = main.setGearboxMode
  main.setGearboxMode = function() end
  local ok, err = pcall(fn, ...)
  main.setGearboxMode = original
  if not ok then error(err) end
end

local function inputDirection(steering)
  return steering < -0.25 and 'left' or (steering > 0.25 and 'right' or 'straight')
end

local function resetSelection(junction)
  selection = {junction = junction, direction = nil, held = nil, seconds = 0, reported = false}
  announced = nil
end

local function wantsReverse()
  forwardSpeed = obj:getVelocity():dot(obj:getDirectionVector())
  local gear = electrics.values.gearIndex or 0
  return gear < 0 or forwardSpeed < -0.3
    or (reversing and (forwardSpeed < -0.05 or (gear == 0 and forwardSpeed < 0.3)))
end

local function measureWheelbase()
  local low, high = math.huge, -math.huge
  pcall(function()
    for _, wheel in pairs(wheels.wheels) do
      local along = obj:getNodePosition(wheel.node1):dot(obj:getDirectionVector())
      low, high = math.min(low, along), math.max(high, along)
    end
  end)
  reverseWheelbase = high-low > 0.5 and high-low or 3
end

local function reverseSteering()
  local guide = plan.reverseGuidance
  local blocked = guide and guide.blocked
  if blocked and not motionWaiting then
    M.stop(session, (guide.reason or 'Reverse route ends. Stop') .. '. Steering assistance off')
    return
  end
  reverseBlocked = blocked == true
  local steering = 0
  if guide and not blocked and not motionWaiting and forwardSpeed <= 0.3 then
    local target = vec3(guide.target[1], guide.target[2], guide.target[3])
    local delta, facing = target-obj:getPosition(), obj:getDirectionVector()
    delta.z = 0
    local behind = -delta:dot(facing)
    if behind > 0.5 then
      local right = vec3(facing.y, -facing.x, 0):normalized()
      -- Rearward pure pursuit: steering right moves the rear to the body's
      -- right while the nose rotates left. Never chase a target in front.
      steering = math.atan(2 * reverseWheelbase * delta:dot(right)
        / math.max(1, delta:squaredLength())) / 0.6
      steering = math.max(-1, math.min(1, steering))
    end
  end
  input.event('steering', steering, 'FILTER_AI', nil, nil, nil, 'ai')
end

local function choose(choices, steering)
  local requested = selection.direction or inputDirection(steering)
  local chosen = choices[requested] and requested
  if not chosen then
    for _, name in ipairs({'straight', 'left', 'right'}) do
      if choices[name] then chosen = name; break end
    end
  end
  lastRequested, lastChoice, choiceAutomatic = requested, chosen, chosen ~= requested
  return chosen, chosen and choices[chosen]
end

local function announceChoices(j)
  local names = {}
  for _, name in ipairs({'left', 'straight', 'right'}) do
    if j.choices[name] then names[#names + 1] = name end
  end
  callback('junction', table.concat(names, ','))
  announced = j.id
end

local function announceChoice(name)
  callback('choice', name)
  callback('say', (choiceAutomatic and 'Choosing automatically. ' or '')
    .. (name == 'straight' and 'Continuing straight' or ('Turning ' .. name)))
end

local function restore()
  if not saved then return end
  local liveValues = {}
  for _, name in ipairs(controls) do
    -- toggleEvent (notably the handbrake) updates state directly, without
    -- lastInputs. Pedal state is authoritative while AI pedals are excluded.
    liveValues[name] = name == 'steering' and ((input.lastInputs['local'] or {}).steering or 0)
      or input.state[name].val
  end
  pcall(preserveGearboxCall, ai.setMode, 'disabled')
  for _, name in ipairs(controls) do
    input.allowedInputSources[name] = saved.sources[name]
    local state = input.state[name]
    local metadata = saved.metadata[name]
    local value = liveValues[name] or 0
    -- AI may have left its final command in state; restore current physical input
    -- even when the old source filter excluded local input.
    state.val, state.filter = value, metadata.filter
    state.angle, state.lockType = metadata.angle, metadata.lockType
    state.source, state.osClockHP = 'local', nil
    if state.smootherKBD then state.smootherKBD:set(value) end
    if state.smootherPAD then state.smootherPAD:set(value) end
    input[name] = value
    electrics.values[name .. '_input'] = value
  end
  saved = nil
end

function M.stop(token, reason)
  if token then retired[token] = true end
  if token and session ~= token then return end
  if session then retired[session] = true end
  local ok, err = pcall(restore)
  lastOffReason = ok and (reason or 'Driving assistant off') or ('Control restoration failed: ' .. tostring(err))
  callback('off', ok and (reason or 'Driving assistant off') or ('Control restoration failed: ' .. tostring(err)))
  session, plan, locked, announced, pathKey = nil, nil, nil, nil, nil
  reversing, motionWaiting, reverseBlocked = false, false, false
  resetSelection(nil)
end

local function follow(path)
  -- GE includes the entry node of the occupied road segment as a connectivity
  -- anchor. Stock manual paths visit every node, including that node behind us.
  -- Start at the segment's forward endpoint, as stock waypoint routing does.
  local ahead = {}
  for i = 2, #path do ahead[#ahead + 1] = path[i] end
  assert(#ahead > 0, 'No forward path target')
  preserveGearboxCall(ai.driveUsingPath, {path = ahead, driveInLane = 'on', avoidCars = 'on', aggression = 0.3})
  ai.setRecoverOnCrash(false)
  if ai.mode ~= 'manual' then error('AI manual path mode unavailable') end
  pathKey = table.concat(path, '\0')
  endpoint = path[#path]
  plannerStarts = plannerStarts + 1
end

local function extendPath(path)
  if path[#path] == endpoint then return end -- GE only removed passed nodes.
  local start
  for i, node in ipairs(path) do if node == endpoint then start = i; break end end
  if not start then error('Route extension is disconnected') end
  local tail = {}
  for i = start, #path do tail[#tail + 1] = path[i] end
  -- Stock setPath APPENDS only when the first node equals its current endpoint.
  -- Restarting driveUsingPath here discards the smoothed plan mid-corner.
  ai.setPath(tail)
  endpoint, pathKey = path[#path], table.concat(path, '\0')
  pathExtensions = pathExtensions + 1
end

function M.start(token, initialPlan)
  if retired[token] or session == token then return end
  if session then M.stop(nil, 'Driving assistant restarted') end
  session = token
  local ok, err = pcall(function()
    assert(type(input.allowedInputSources) == 'table' and type(input.lastInputs) == 'table',
      'Input source filters unavailable')
    assert(controller.mainController and controller.mainController.setGearboxMode
      and electrics.values.gearboxMode, 'Gearbox mode unavailable')
    assert(ai.mode == 'disabled', 'Disable ordinary AI before enabling the driving assistant')
    local snapshot = {sources = {}, metadata = {}, gearbox = electrics.values.gearboxMode}
    for _, name in ipairs(controls) do
      assert(input.state[name], 'Missing input: ' .. name)
      snapshot.sources[name] = input.allowedInputSources[name]
      local state = input.state[name]
      snapshot.metadata[name] = {filter = state.filter, angle = state.angle, lockType = state.lockType}
    end
    saved = snapshot
    for _, name in ipairs(controls) do
      input.allowedInputSources[name] = name == 'steering' and {ai = true} or {['local'] = true}
      if name ~= 'steering' then
        if input.state[name].source ~= 'local' then
          input.state[name].val = (input.lastInputs['local'] or {})[name] or 0
        end
      end
    end
    lease, planAge, reportAge, brakeTime, cueAge = 0, 0, 0, 0, 8
    suspended = false
    standstillTime, M.stoppedTime = 0, 0
    recovering, hazardAge = false, 8
    plannerStarts, pathExtensions = 0, 0
    lastChoice, lastRequested, choiceAutomatic, lastOffReason = nil, nil, false, nil
    resetSelection(nil)
    reversing, motionWaiting, motionRevision, reverseBlocked = false, false, 0, false
    measureWheelbase()
    plan = initialPlan
    reversing = wantsReverse()
    if not reversing and plan.chooseNow and plan.junction then
      local value = (input.lastInputs['local'] or {}).steering or 0
      local name, path = choose(plan.junction.choices, value)
      assert(path, 'No valid exit at this junction')
      assert(not plan.junction.unsupported, 'This junction is unsupported')
      plan.path = path
      locked = plan.junction.id
      plan.initialChoice = name
    end
    if reversing then
      suspended, motionWaiting = true, true
      input.event('steering', 0, 'FILTER_AI', nil, nil, nil, 'ai')
    else follow(plan.path) end
  end)
  if not ok then M.stop(token, 'Driving assistant unavailable. ' .. tostring(err)); return end
  callback('ready', 'Driving assistant on. You control speed and stopping'
    .. (plan.roadMessage and ('. ' .. plan.roadMessage) or ''))
  if reversing then
    callback('motion', 'reverse')
    callback('say', 'Reverse steering assistance. Go slowly')
  end
  if plan.initialChoice then
    announceChoices(plan.junction)
    announceChoice(plan.initialChoice)
  end
end

function M.heartbeat(token)
  if session == token then lease = 0 end
end

function M.setPlan(token, value)
  if session ~= token then return end
  if wantsReverse() ~= reversing then return end
  if value.motion and value.motion ~= (reversing and 'reverse' or 'forward') then return end
  if motionWaiting and (value.motionRevision or 0) <= motionRevision then return end
  if motionWaiting then
    motionWaiting = false
    motionRevision = value.motionRevision
    locked = nil
    resetSelection(nil)
    if not reversing then
      local ok, err = pcall(follow, value.path)
      if not ok then M.stop(token, 'Route control failed. ' .. tostring(err)); return end
      suspended = false
      callback('say', 'Forward steering assistance')
    end
  end
  -- Until GE acknowledges a choice, an in-flight old plan cannot overwrite it.
  if locked and value.junction and value.junction.id == locked then return end
  if value.clear then locked = nil end
  plan, planAge = value, 0
  if not plan.junction or plan.junction.id ~= selection.junction then
    resetSelection(plan.junction and plan.junction.id)
  end
  local key = table.concat(plan.path, '\0')
  if not suspended and key ~= pathKey then
    local ok, err = pcall(extendPath, plan.path)
    if not ok then M.stop(token, 'Route control failed. ' .. tostring(err)) end
  end
end

local function update(dt)
  lease, planAge, reportAge, cueAge = lease + dt, planAge + dt, reportAge + dt, cueAge + dt
  if lease > 3 or planAge > 1 then M.stop(session, 'Connection lost. Steering assistance off'); return end
  for _, name in ipairs(controls) do
    local allowed = input.allowedInputSources[name]
    local expected = name == 'steering' and 'ai' or 'local'
    if not allowed or not allowed[expected] then error('Input ownership lost') end
    for source, enabled in pairs(allowed) do
      if enabled and source ~= expected then error('Input ownership changed') end
    end
  end
  local speed = obj:getVelocity():length()
  local reverseNow = wantsReverse()
  if reverseNow ~= reversing then
    reversing, motionWaiting, reverseBlocked = reverseNow, true, false
    motionRevision = plan.motionRevision or motionRevision
    preserveGearboxCall(ai.setMode, 'disabled')
    suspended = true
    input.event('steering', 0, 'FILTER_AI', nil, nil, nil, 'ai')
    callback('motion', reversing and 'reverse' or 'forward')
    if reversing then callback('say', 'Reverse steering assistance. Go slowly') end
  end
  if reversing or motionWaiting then
    if ai.mode ~= 'disabled' then M.stop(session, 'AI control changed. Steering assistance off'); return end
    reverseSteering()
    if not session then return end
    if reportAge >= 0.2 then
      callback('alive')
      if plan.diagnostic then callback('diagnostic', jsonEncode(M.diagnosticState())) end
      reportAge = 0
    end
    return
  end
  hazardAge = hazardAge + dt
  if (plan.recovering == true) ~= recovering then
    recovering = plan.recovering == true
    callback('say', recovering and 'Rejoining the road. Slow down' or 'Back on the road')
    cueAge = 0
  end
  if plan.hazard and hazardAge >= 3 then
    callback('say', 'Obstacle ahead. Brake')
    hazardAge, cueAge = 0, 0
  end
  local localInputs = input.lastInputs['local'] or {}
  -- Stock AI interprets rejected throttle while stopped as a crash after 5 s.
  -- Suspend its planner at rest and resume on player throttle, before recovery.
  if speed < 0.1 and (localInputs.throttle or 0) <= 0.05 then
    standstillTime = standstillTime + dt
    if not suspended and standstillTime >= 0.3 then
      preserveGearboxCall(ai.setMode, 'disabled')
      suspended = true
    end
  else standstillTime = 0 end
  -- Separate thresholds keep stopped suspension from toggling with tiny
  -- velocity/pedal fluctuations while the player is selecting a gear.
  if suspended and (speed > 0.3 or (localInputs.throttle or 0) > 0.1) then
    follow(plan.path)
    suspended = false
  end
  if not suspended and ai.mode ~= 'manual' then
    M.stop(session, 'AI control changed. Steering assistance off'); return
  end
  -- A held throttle against player brake or an obstacle must not trigger recovery.
  if speed < 0.1 and not suspended then
    M.stoppedTime = (M.stoppedTime or 0) + dt
    if M.stoppedTime > 2 then follow(plan.path); M.stoppedTime = 0 end
  else M.stoppedTime = 0 end
  local j = plan.junction
  if j and not locked then
    if selection.junction ~= j.id then resetSelection(j.id) end
    local remaining = math.max(0, plan.distance - speed * planAge)
    if remaining <= math.max(30, math.min(140, speed * 7)) and announced ~= j.id then
      if j.unsupported then M.stop(session, 'Unsupported junction or roundabout. Steering assistance off'); return end
      if not next(j.choices) then M.stop(session, 'Dead end ahead. Steering assistance off'); return end
      local names = {}
      for _, name in ipairs({'left', 'straight', 'right'}) do if j.choices[name] then names[#names + 1] = name end end
      announceChoices(j)
      callback('say', 'Junction ahead. ' .. table.concat(names, ' or ') .. '. '
        .. (j.choices.straight and 'Hold a turn for one second to select.'
          or 'Turn required. Hold left or right for one second.'))
      announced, cueAge = j.id, 0
    end
    if announced == j.id then
      local direction = inputDirection(localInputs.steering or 0)
      if direction == 'straight' then
        selection.held, selection.seconds, selection.reported = nil, 0, false
      else
        if direction ~= selection.held then
          selection.held, selection.seconds, selection.reported = direction, 0, false
        end
        selection.seconds = selection.seconds + dt
        if selection.seconds >= 1 and not selection.reported then
          selection.reported = true
          if not j.choices[direction] then
            callback('say', direction .. ' is unavailable')
          elseif selection.direction ~= direction then
            selection.direction = direction
            callback('selection', direction)
            callback('say', direction .. ' selected. Release steering')
            cueAge = 0
          end
        end
      end
    end
    if remaining <= math.max(12, math.min(35, speed * 2)) then
      local value = localInputs.steering or 0
      local name, path = choose(j.choices, value)
      if not path then M.stop(session, 'Dead end ahead. Steering assistance off'); return end
      locked, cueAge = j.id, 0
      extendPath(path)
      plan.path = path
      announceChoice(name)
    end
  end
  local brake = (input.lastInputs.ai or {}).brake or 0
  -- An unresolved endpoint produces artificial braking; conservatively suppress
  -- its advisory until an outgoing route has been selected.
  if brake > 0.15 and speed > 5 and (not j or locked) then brakeTime = brakeTime + dt else brakeTime = 0 end
  if brakeTime >= 0.4 and cueAge >= 8 then callback('slow', 'Slow down'); cueAge, brakeTime = 0, 0 end
  if reportAge >= 0.2 then
    callback('alive')
    if plan.diagnostic then callback('diagnostic', jsonEncode(M.diagnosticState())) end
    reportAge = 0
  end
end

function M.updateGFX(dt)
  if not session then return end
  local ok, err = pcall(update, dt)
  if not ok then M.stop(session, 'Control failure. Steering assistance off. ' .. tostring(err)) end
end

function M.onReset() M.stop(nil, 'Vehicle reset. Steering assistance off') end
function M.diagnosticState()
  local physical = input.lastInputs['local'] or {}
  local aiInputs = input.lastInputs.ai or {}
  local choices = {}
  if plan and plan.junction then
    for _, name in ipairs({'left', 'straight', 'right'}) do
      if plan.junction.choices[name] then choices[#choices + 1] = name end
    end
  end
  local wheelCount, contactWheels, downForce = 0, 0, 0
  if wheels and wheels.wheels then
    for _, wheel in pairs(wheels.wheels) do
      wheelCount = wheelCount + 1
      local force = math.max(0, wheel.downForceRaw or 0)
      downForce = downForce + force
      if force > 1 then contactWheels = contactWheels + 1 end
    end
  end
  return {active = session ~= nil, endpoint = endpoint, recovering = recovering,
    reversing = reversing, forwardSpeed = forwardSpeed, motionWaiting = motionWaiting,
    reverseBlocked = reverseBlocked, reverseWheelbase = reverseWheelbase,
    plannerStarts = plannerStarts, pathExtensions = pathExtensions, suspended = suspended,
    aiMode = ai.mode, heldSteering = physical.steering or 0, appliedSteering = input.steering,
    throttle = input.state.throttle.val, brake = input.state.brake.val,
    clutch = input.state.clutch.val, parkingbrake = input.state.parkingbrake.val,
    gear = electrics.values.gearIndex, rpm = electrics.values.rpm,
    aiSteering = aiInputs.steering, aiBrake = aiInputs.brake,
    speed = obj:getVelocity():length(), gearbox = electrics.values.gearboxMode,
    choices = choices, requested = lastRequested, chosen = lastChoice, automatic = choiceAutomatic,
    selection = selection.direction, selectionJunction = selection.junction,
    selectionHeld = selection.held, selectionSeconds = selection.seconds,
    wheelCount = wheelCount, contactWheels = contactWheels, wheelDownForce = downForce,
    locked = locked, planAge = planAge, leaseAge = lease, lastOffReason = lastOffReason}
end
function M.onExtensionUnloaded() M.stop(nil, 'Steering assistance off') end
return M
