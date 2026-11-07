-- Parks the car at a fuel pump for fuelStations.lua (GE), then hands it back.
--
-- The forward driving is the game's own AI. The pump is off the navgraph, and the stock way to
-- follow arbitrary points (ai.driveUsingPath{script=}) teleports the car to the first point and
-- swaps the AI's map for a throwaway graph. Instead the points GE planned are ADDED to this
-- vehicle's own copy of the navgraph (mapmgr.mapData -- each vehicle VM holds its own) as a
-- short chain of nodes, and stock AI is handed that chain as an ordinary manual path. It then
-- drives it with its own steering, speed control and car avoidance. The chain runs past the
-- stop point and this module brakes at the exact spot. The nodes are removed afterwards.
--
-- What stock AI cannot do stays here: it only reverses as crash recovery, so when the car is
-- blocked, pointed the wrong way, or arrives off the line, this module takes the controls,
-- backs up on the opposite lock, and hands the rest of the chain back to the AI. The safety
-- layer is also here, because stock AI does not look for static obstacles off the road: a
-- look-ahead at this vehicle's own heights (vehicleGeometry.rayProfile, passed in by GE), and
-- a stop on any new body damage, which catches whatever the map has no collision for.
--
-- Inputs go through the 'ai' source with every other source shut out, saved and restored as
-- beamtelAssistant.restore does. The gearbox is put in arcade, which picks drive and reverse by
-- itself, and the player's mode is restored at the end, as beamtelAssistant.endJoin does.
local M = {}

local CONTROLS = {'steering', 'throttle', 'brake', 'parkingbrake'}
local AI_MPS = 2.2
local STOP_NODE_MPS = 0.6
local MAX_STEER_RAD = 0.6
local STUCK_S = 15
local TOTAL_S = 120
local MAX_MANOEUVRES = 6
local REVERSE_M = 3.5
local REVERSE_MPS = 1.0
local DAMAGE_STOP = 150   -- beamstate damage gained since the start that counts as a touch
local MAX_FORWARD_MPS = 3.0
local MAX_REVERSE_MPS = 1.8
local NODE_PREFIX = 'beamtelFuel_'

local session, saved
local path, total, beyond
local lease, aliveAge, elapsed, progressAge, bestRemaining = 0, 0, 0, 0, math.huge
local stopping, stoppedAge = false, 0
local geom -- {rearAlong, centreAlong, centreLeft, wheelbase}
local rays -- {front = {lowLift, highLift, step}, rear = {...}} from GE
local retired = {}
local trace, traceAge = {}, 0 -- the centre once a second, for diagnosticState
local phase, phaseAge, manoeuvres = 'ai', 0, 0
local reverseSteer, reverseFrom, reverseFull = 0, nil, false
local moveFrom, moveAge = nil, 0 -- for "pushing but not moving"
local obstacleAge = 0
local offLineBest = math.huge -- closest to the line since the AI last took over
local manoeuvreLog = {}
local damageLast, damageGain = 0, 0
local lastReason
local failing -- a failure waiting for the car to stop before control goes back
local nodes, ownMap = {}, false -- the chain added to the navgraph
local aiStarts = 0

-- What was last sent, for smoothing. Jumping straight to full lock or a hard brake made the
-- car lurch on every correction.
local sent = {steering = 0, throttle = 0, brake = 0}
local RATE = {steering = 1.5, throttle = 0.8, brake = 1.5} -- units per second, rising

local function callback(kind, message)
  if not session then return end
  obj:queueGameEngineLua(string.format(
    'if extensions.fuelStations then extensions.fuelStations.onParkEvent(%d,%q,%q,%q) end',
    obj:getID(), session, kind, message or ''))
end

local function frame()
  local fwd = obj:getDirectionVector()
  local up = obj:getDirectionVectorUp()
  fwd = vec3(fwd.x, fwd.y, 0):normalized()
  local left = vec3(0, 0, 1):cross(fwd):normalized()
  return obj:getPosition(), fwd, left, up
end

local function measure()
  local _, fwd, left = frame()
  local minA, maxA, lat, n = math.huge, -math.huge, 0, 0
  for _, wheel in pairs(wheels.wheels or {}) do
    -- Both hub nodes: node1 alone sits on one side of the hub. The reference node is not on
    -- the centre line either (0.25 m off on a BX), so the centre is measured, not assumed.
    local rel = (obj:getNodePosition(wheel.node1) + obj:getNodePosition(wheel.node2)) * 0.5
    local along = rel:dot(fwd)
    minA, maxA = math.min(minA, along), math.max(maxA, along)
    lat, n = lat + rel:dot(left), n + 1
  end
  if n == 0 or maxA - minA < 0.5 then
    return {rearAlong = -1.4, centreAlong = 0, centreLeft = 0, wheelbase = 2.7}
  end
  return {rearAlong = minA, centreAlong = (minA + maxA) / 2, centreLeft = lat / n,
    wheelbase = maxA - minA}
end

local function points()
  local pos, fwd, left = frame()
  local centre = pos + fwd * geom.centreAlong + left * geom.centreLeft
  local rear = pos + fwd * geom.rearAlong + left * geom.centreLeft
  return centre, rear, fwd, left
end

-- Progress along the planned polyline, taken segment by segment in order. Taking the nearest
-- segment of all let a car that started beside the pump skip the lead-in.
local segment = 1
local function project(i, p)
  local a, b = path[i], path[i + 1]
  local seg = b - a
  local len = math.max(1e-3, seg:length())
  local t = math.max(0, math.min(1, (p - a):dot(seg) / (len * len)))
  local s = 0
  for k = 1, i - 1 do s = s + (path[k + 1] - path[k]):length() end
  return (p - (a + seg * t)):length(), s + t * len, t
end

local function track(p, ahead)
  local bestD, bestS, t = project(segment, p)
  if segment < #path - 1 then
    local nextD, nextS = project(segment + 1, p)
    -- Only past the corner, or within reach of it: a car beside the pump is always nearer the
    -- final approach than the lead-in.
    if t >= 0.999 or (nextD < bestD and (p - path[segment + 1]):length() < 3) then
      segment = segment + 1
      bestD, bestS = nextD, nextS
    end
  end
  local want = bestS + ahead
  local s = 0
  for i = 1, #path - 1 do
    local a, b = path[i], path[i + 1]
    local len = (b - a):length()
    if s + len >= want and len > 1e-3 then return bestS, a + (b - a) * ((want - s) / len), bestD end
    s = s + len
  end
  local last = path[#path]
  local dir = beyond - last
  if dir:length() < 1e-3 then return bestS, last, bestD end
  return bestS, last + dir:normalized() * (want - s), bestD
end

-- Look-ahead, at THIS vehicle's heights. Every origin finds the ground under it first. Walls
-- and poles are a level ray at the body's high lift; anything lower must show as a sudden rise
-- in the ground profile taller than this vehicle can take. A level ray at a fixed height cannot
-- tell a ramp from a wall (Italy's forecourt entry rises 0.36 m in 1.5 m and stopped the car at
-- its foot), and one started from the reference node sat 2 to 14 cm above the road on a BX.
local function groundAt(p)
  local top = vec3(p.x, p.y, p.z + 1.5)
  local drop = obj:castRayStatic(top, vec3(0, 0, -1), 4)
  if drop and drop < 4 then return top.z - drop end
  return nil
end

local DEFAULT_RAYS = {lowLift = 0.35, highLift = 0.8, step = 0.15}

local function obstacle(centre, fwd, left, sign, reach)
  local r = rays and (sign < 0 and rays.rear or rays.front) or DEFAULT_RAYS
  local halfL = (obj:getInitialLength() or 4.5) * 0.5
  local halfW = (obj:getInitialWidth() or 1.9) * 0.5 - 0.1
  local dir = fwd * sign
  for _, side in ipairs({-1, 0, 1}) do
    local start = centre + dir * (halfL - 0.2) + left * (side * halfW)
    local g0 = groundAt(start)
    if g0 then
      local origin = vec3(start.x, start.y, g0 + r.highLift)
      local hit = obj:castRayStatic(origin, dir, reach + 0.2)
      if hit and hit < reach + 0.2 then
        return true, string.format('wall %.1fm side %d', hit, side)
      end
      local prev = g0
      for k = 1, math.ceil((reach + 0.2) / 0.25) do
        local g = groundAt(start + dir * (k * 0.25))
        if g then
          if g - prev > r.step then
            return true, string.format('step %.2fm at %.2fm side %d', g - prev, k * 0.25, side)
          end
          prev = g
        end
      end
    end
  end
  return false
end

local frameDt = 0.016
local function send(name, value)
  local rate = RATE[name]
  if rate then
    local last = sent[name] or 0
    -- Steering slews both ways; pedals ease on but come off at once, so a stop is never slowed.
    local step = rate * frameDt
    if name == 'steering' or value > last then
      value = math.max(last - step, math.min(last + step, value))
    end
    sent[name] = value
  end
  input.event(name, value, 'FILTER_AI', nil, nil, nil, 'ai')
end

-- Bringing the car to a halt, the right way round for ARCADE, which this module drives in.
-- In arcade the brake pedal at a standstill is reverse throttle, and the throttle while rolling
-- backwards is the brake. Holding "the brake until stopped" on a car that is already stopped
-- therefore drives it backwards, harder the faster it goes -- which is exactly what happened:
-- a car facing away from the pump entered the manoeuvre at rest, reversed away at 7.6 m/s on
-- full lock and crashed. So: brake while rolling forwards, throttle while rolling backwards,
-- and the PARKING brake, never the pedal, at a standstill. Every stop in this file uses this.
local function brakeToStop(forwardSpeed, strength)
  if forwardSpeed > 0.05 then
    send('throttle', 0); send('brake', strength); send('parkingbrake', 0)
  elseif forwardSpeed < -0.05 then
    send('brake', 0); send('throttle', strength); send('parkingbrake', 0)
  else
    send('throttle', 0); send('brake', 0); send('parkingbrake', 1)
  end
end

-- Taking over from the AI mid-move: start the smoothing from what the car is doing now.
local function syncSent()
  sent = {steering = input.state.steering and input.state.steering.val or 0,
    throttle = input.state.throttle and input.state.throttle.val or 0,
    brake = input.state.brake and input.state.brake.val or 0}
end

-- Stock AI's own setMode calls can switch the gearbox; this module owns it for the duration.
local function aiCall(fn, ...)
  local main = controller.mainController
  local original = main and main.setGearboxMode
  if original then main.setGearboxMode = function() end end
  local ok, err = pcall(fn, ...)
  if original then main.setGearboxMode = original end
  if not ok then error(err) end
end

-- ---------------------------------------------------------------------------------------------
-- The chain in the navgraph
-- ---------------------------------------------------------------------------------------------

local function removeNodes()
  local md = mapmgr.mapData
  if md then
    for _, n in ipairs(nodes) do
      if md.graph then md.graph[n] = nil end
      if md.positions then md.positions[n] = nil end
      if md.radius then md.radius[n] = nil end
    end
  end
  nodes = {}
  -- A map with no road graph got a graph of our own; give the vehicle back its real (absent)
  -- one and let mapmgr ask GE again.
  if ownMap then
    ownMap = false
    pcall(mapmgr.clearCustomMap)
    pcall(mapmgr.requestMap)
  end
end

-- Nodes for the chain from path index `from` to the end, plus the steering-only point beyond.
-- Isolated from the road network on purpose: nothing connects to them, so no other route in
-- this vehicle can wander onto the forecourt, and removing them leaves the graph as it was.
local function addNodes(from)
  removeNodes()
  local md = mapmgr.mapData
  if not md then
    md = require('graphpath').newGraphpath()
    mapmgr.setCustomMap(md)
    ownMap = true
    -- Stock AI checks a manual path against the alias table and indexes it unguarded.
    mapmgr.nodeAliases = mapmgr.nodeAliases or {}
  end
  local width = math.max(2.5, (obj:getInitialWidth() or 1.9) + 0.6)
  local pts = {}
  for i = from, #path do pts[#pts + 1] = path[i] end
  pts[#pts + 1] = beyond
  for i, p in ipairs(pts) do
    local name = NODE_PREFIX .. session .. '_' .. aiStarts .. '_' .. i
    md:setPointPositionRadius(name, vec3(p), width * 0.5)
    nodes[i] = name
  end
  for i = 1, #nodes - 1 do
    local a, b = nodes[i], nodes[i + 1]
    md:uniEdge(a, b, md.positions[a]:distance(md.positions[b]), 1, AI_MPS, nil, true)
  end
  return nodes
end

local function startAi()
  aiStarts = aiStarts + 1
  -- Chain from the next corner on: the current segment's start is behind the car.
  local chain = addNodes(math.min(#path, segment + 1))
  -- The chain runs past the stop, so on its own the AI arrives at full cruise and this module
  -- has to brake hard. Ask it to be at walking pace by the stop node instead.
  local wpSpeeds = {}
  if #chain >= 2 then wpSpeeds[chain[#chain - 1]] = STOP_NODE_MPS end
  aiCall(ai.driveUsingPath, {path = chain, avoidCars = 'on', routeSpeed = AI_MPS,
    routeSpeedMode = 'limit', aggression = 0.3, driveInLane = 'off', wpSpeeds = wpSpeeds})
  aiCall(ai.setRecoverOnCrash, false)
  if ai.mode ~= 'manual' then error('stock AI did not take the path') end
  phase, phaseAge = 'ai', 0
  offLineBest, moveFrom, moveAge, obstacleAge = math.huge, nil, 0, 0
end

local function stopAi()
  if ai.mode ~= 'disabled' then aiCall(ai.setMode, 'disabled') end
  syncSent()
end

-- ---------------------------------------------------------------------------------------------
-- Taking and giving back the controls
-- ---------------------------------------------------------------------------------------------

local function take()
  local snapshot = {sources = {}, metadata = {}, gearbox = electrics.values.gearboxMode,
    parkingbrake = input.state.parkingbrake and input.state.parkingbrake.val or 0}
  for _, name in ipairs(CONTROLS) do
    assert(input.state[name], 'Missing input: ' .. name)
    snapshot.sources[name] = input.allowedInputSources[name]
    local state = input.state[name]
    snapshot.metadata[name] = {filter = state.filter, angle = state.angle, lockType = state.lockType}
  end
  saved = snapshot
  for _, name in ipairs(CONTROLS) do input.allowedInputSources[name] = {ai = true} end
  local main = controller.mainController
  if main and main.setGearboxMode and electrics.values.gearboxMode ~= 'arcade' then
    main.setGearboxMode('arcade')
  end
end

-- As beamtelAssistant.restore: physical input goes back into state, so nothing the AI last
-- commanded lingers. The parking brake goes back to where the player left it.
local function restore()
  if not saved then return end
  local snapshot = saved
  saved = nil
  local physical = input.lastInputs['local'] or {}
  for _, name in ipairs(CONTROLS) do
    input.allowedInputSources[name] = snapshot.sources[name]
    local state = input.state[name]
    local metadata = snapshot.metadata[name]
    local value = name == 'parkingbrake' and snapshot.parkingbrake or (physical[name] or 0)
    state.val, state.filter = value, metadata.filter
    state.angle, state.lockType = metadata.angle, metadata.lockType
    state.source, state.osClockHP = 'local', nil
    if state.smootherKBD then state.smootherKBD:set(value) end
    if state.smootherPAD then state.smootherPAD:set(value) end
    input[name] = value
    electrics.values[name .. '_input'] = value
  end
  local main = controller.mainController
  if snapshot.gearbox and main and main.setGearboxMode and electrics.values.gearboxMode ~= snapshot.gearbox then
    main.setGearboxMode(snapshot.gearbox)
  end
end

local function release()
  pcall(stopAi)
  pcall(removeNodes)
  return pcall(restore)
end

local function finish(kind, message)
  local ok, err = release()
  if not ok then
    log('E', 'beamtelFuelPark', tostring(err))
    kind, message = 'failed', 'Controls could not be fully restored. Reset the vehicle if they do not respond'
  end
  lastReason = kind .. (message and (': ' .. message) or '')
  log('I', 'beamtelFuelPark', lastReason)
  failing = nil
  callback(kind, message)
  if session then retired[session] = true end
  session, path, beyond, geom = nil, nil, nil, nil
end

local function damageNow()
  return beamstate and tonumber(beamstate.damage) or 0
end

function M.start(token, pts, beyondPoint, rayProfile)
  if retired[token] or session == token then return end
  if session then finish('failed', 'Parking restarted') end
  session = token
  local ok, err = pcall(function()
    geom = measure()
    rays = rayProfile
    local centre = points()
    path = {vec3(centre)}
    for _, p in ipairs(pts) do path[#path + 1] = vec3(p.x, p.y, p.z) end
    beyond = vec3(beyondPoint.x, beyondPoint.y, beyondPoint.z)
    total = 0
    for i = 1, #path - 1 do total = total + (path[i + 1] - path[i]):length() end
    segment, aiStarts = 1, 0
    take()
    syncSent()
    if ai.mode ~= 'disabled' then stopAi() end
    startAi()
  end)
  if not ok then
    log('E', 'beamtelFuelPark', tostring(err))
    finish('failed', 'Parking could not take the car')
    return
  end
  lease, aliveAge, elapsed, progressAge, bestRemaining = 0, 0, 0, 0, math.huge
  stopping, stoppedAge, failing = false, 0, nil
  trace, traceAge = {}, 1
  manoeuvres, moveFrom, moveAge, obstacleAge = 0, nil, 0, 0
  manoeuvreLog = {}
  damageLast, damageGain = damageNow(), 0
end

function M.heartbeat(token)
  if session == token then lease = 0 end
end

function M.stop(token)
  if token then retired[token] = true end
  if not session or (token and token ~= session) then return end
  -- GE ended it (cancel, vehicle change): no event back, GE has already spoken.
  release()
  retired[session] = true
  session, path, beyond, geom = nil, nil, nil, nil
end

-- A failure does not drop a moving car: it brakes to a halt first, then hands back.
local function fail(message)
  if not failing then
    failing = message
    pcall(stopAi)
  end
end

local function addTrace(centre, text)
  traceAge = traceAge + frameDt
  if traceAge >= 1 and #trace < 120 then
    traceAge = 0
    trace[#trace + 1] = string.format('%.1f,%.1f %s', centre.x, centre.y, text)
  end
end

local function drive(dt)
  frameDt = dt
  lease, aliveAge, elapsed, progressAge = lease + dt, aliveAge + dt, elapsed + dt, progressAge + dt
  phaseAge = phaseAge + dt
  local speed = obj:getVelocity():length()
  local physical = input.lastInputs['local'] or {}
  -- The player's brake is the one exit that hands back at once: they are already stopping.
  if (physical.brake or 0) > 0.5 then finish('failed', 'Parking cancelled by the brake'); return end
  local forwardNow = obj:getVelocity():dot(obj:getDirectionVector())
  if failing then
    -- restore() gives the player back their own parking brake setting afterwards.
    brakeToStop(forwardNow, 0.7)
    if speed < 0.05 then finish('failed', failing) end
    if elapsed > TOTAL_S + 10 then finish('failed', failing .. '. Brake now') end
    return
  end
  if lease > 3 then fail('Connection lost'); return end
  if elapsed > TOTAL_S then fail('Could not reach the pump in time'); return end
  -- A hard ceiling, whatever any phase thinks it is doing. Nothing in parking needs more than
  -- walking pace; anything faster is a fault, and the car is stopped before it goes further.
  if forwardNow > MAX_FORWARD_MPS or forwardNow < -MAX_REVERSE_MPS then
    fail('Parking was going too fast and has stopped')
    return
  end
  -- Any new body damage is a touch: stop. This is what catches whatever the map has no
  -- collision mesh for, and movable props, which static raycasts cannot see at all. Counted as
  -- increases only: the game's counter went DOWN by 2855 during the crash above, so a figure
  -- taken against the starting value never rose.
  local damage = damageNow()
  if damage > damageLast then damageGain = damageGain + (damage - damageLast) end
  damageLast = damage
  if damageGain > DAMAGE_STOP then
    fail('The car touched something. Check around it before driving on')
    return
  end

  local centre, rear, fwd, left = points()
  local progress, aim, offLine = track(centre, math.max(3, math.min(6, 3 + speed)))
  local remaining = total - progress
  local forwardSpeed = obj:getVelocity():dot(fwd)
  if remaining < bestRemaining - 0.3 then bestRemaining, progressAge = remaining, 0 end
  if progressAge > STUCK_S then fail('The car is not getting any closer to the pump. Something may be in the way'); return end

  local v = aim - rear
  local x, y = v:dot(fwd), v:dot(left)
  local aimAngle = math.deg(math.abs(math.atan2(y, x)))

  -- Arrived: the centre is at the stop. The chain runs on past it, so the AI is still rolling;
  -- this is where it is told to stop, to within centimetres rather than its own half metre.
  if remaining <= 0.15 or stopping then
    if not stopping then stopping = true; stopAi() end
    brakeToStop(forwardSpeed, math.min(0.7, 0.25 + speed * 0.25))
    if speed < 0.05 then stoppedAge = stoppedAge + dt else stoppedAge = 0 end
    if stoppedAge >= 0.5 then finish('stopped'); return end
    if aliveAge >= 0.2 then aliveAge = 0; callback('alive') end
    return
  end

  if phase == 'ai' then
    -- The AI let go by itself (gave up on the path, or the map was re-sent and took our nodes
    -- with it): give it the chain again from where the car is, once per manoeuvre budget.
    if ai.mode ~= 'manual' then
      manoeuvres = manoeuvres + 1
      if manoeuvres > MAX_MANOEUVRES then fail('Could not manoeuvre to the pump'); return end
      local ok = pcall(startAi)
      if not ok then fail('The game AI would not take the car'); return end
    end
    -- Pushing against a kerb rocks the car a few centimetres either way, so "stopped" is
    -- judged by distance covered, not by the speed reading.
    local blocked, why = false, nil
    moveAge = moveAge + dt
    if not moveFrom then moveFrom = vec3(centre) end
    if moveAge >= 2 then
      blocked = (centre - moveFrom):length() < 0.25 and (input.throttle or 0) > 0.12
      moveFrom, moveAge = vec3(centre), 0
    end
    -- Seen for a moment, not one frame. Off near the stop, where the pump is beside the car by
    -- design and the placement was measured clear.
    if blocked then why = 'not moving' end
    local seen, what = false, nil
    if remaining > 1.5 then
      seen, what = obstacle(centre, fwd, left, 1, 0.6 + math.max(0, forwardSpeed) * 0.6)
    end
    if seen then obstacleAge = obstacleAge + dt else obstacleAge = 0 end
    if obstacleAge >= 0.3 then blocked = true; obstacleAge = 0; why = 'ahead: ' .. tostring(what) end
    -- Close to the pump but off the line or angled: back out and come in again.
    local misaligned = false
    if segment == #path - 1 and remaining < 3.5 then
      local a, b = path[#path - 1], path[#path]
      local dir = vec3(b.x - a.x, b.y - a.y, 0):normalized()
      local off = vec3(centre.x - b.x, centre.y - b.y, 0)
      local lateralErr = math.abs(off:dot(vec3(0, 0, 1):cross(dir)))
      local headingErr = math.deg(math.acos(math.max(-1, math.min(1, fwd:dot(dir)))))
      misaligned = lateralErr > 0.6 or headingErr > 20
      if misaligned then why = string.format('misaligned %.1fm %.0fdeg', lateralErr, headingErr) end
    end
    -- Drifting AWAY from the line is the fault this catches (a corner cut wide). Being off it
    -- is not: a back-and-fill leaves the car off the line by design, and an absolute "more
    -- than 2 m off" sent it straight back into reverse every time the AI set off to rejoin --
    -- with a wall behind, six times in one spot until the budget ran out. So: 2 m further off
    -- than the closest it has come since the AI last took over.
    if offLine then
      offLineBest = math.min(offLineBest, offLine)
      if offLine > offLineBest + 2.0 and speed > 0.3 then
        misaligned = true
        why = string.format('drifting off the line %.1fm', offLine)
      end
    end
    if aimAngle > 110 then why = string.format('aim behind %.0fdeg', aimAngle) end
    if aimAngle > 110 or blocked or misaligned then
      -- Why each manoeuvre began: a spurious one is otherwise indistinguishable in the trace.
      manoeuvreLog[#manoeuvreLog + 1] = string.format('%.1f,%.1f %s', centre.x, centre.y,
        tostring(why))
      moveFrom, moveAge = nil, 0
      manoeuvres = manoeuvres + 1
      if manoeuvres > MAX_MANOEUVRES then fail('Could not manoeuvre to the pump'); return end
      stopAi()
      phase, phaseAge = 'toReverse', 0
      -- Reversing on the opposite lock swings the nose the way the aim point wants it.
      reverseSteer = y >= 0 and 1 or -1
      -- Realigning needs the whole run back, or the car arrives misaligned again.
      reverseFull = misaligned
      progressAge = 0
    end
  end

  if phase == 'toReverse' then
    send('steering', reverseSteer)
    -- Firm enough to stop within a metre from walking pace, no more; held on the parking brake
    -- once stopped while the wheels turn to the reversing lock.
    brakeToStop(forwardSpeed, math.min(0.6, 0.2 + speed * 0.2))
    if speed < 0.05 and phaseAge > 0.5 then
      phase, phaseAge, reverseFrom = 'reverse', 0, vec3(centre)
    end
  elseif phase == 'reverse' then
    -- Arcade gearbox: held brake at a standstill engages reverse and drives backwards, and
    -- throttle is the brake while rolling backwards.
    local back = -forwardSpeed
    local backed = (centre - reverseFrom):length()
    send('steering', reverseSteer)
    -- Reverse power only below the target, and the (arcade) brake once over it by a little;
    -- MAX_REVERSE_MPS stops the whole session well before this could run away.
    send('brake', back < REVERSE_MPS - 0.2 and (phaseAge < 1.5 and 0.3 or 0.2) or 0)
    send('throttle', back > REVERSE_MPS + 0.1 and 0.3 or 0)
    send('parkingbrake', 0)
    local stuckBehind = (phaseAge > 2.5 and back < 0.05)
      or obstacle(centre, fwd, left, -1, 0.5 + math.max(0, back) * 0.6)
    if backed >= REVERSE_M or (not reverseFull and aimAngle < 50 and backed > 1) or stuckBehind
        or phaseAge > 10 then
      phase, phaseAge = 'toForward', 0
      progressAge = 0
    end
  elseif phase == 'toForward' then
    -- Stop the backwards roll, then the AI takes over again.
    brakeToStop(forwardSpeed, 0.4)
    if math.abs(forwardSpeed) <= 0.05 and phaseAge > 0.3 then
      send('parkingbrake', 0)
      local ok = pcall(startAi)
      if not ok then fail('The game AI would not take the car'); return end
    end
  end

  if aliveAge >= 0.2 then aliveAge = 0; callback('alive') end
  addTrace(centre, string.format('%s r%.1f v%.1f a%.0f m%d', phase, remaining, forwardSpeed,
    aimAngle, manoeuvres))
end

function M.updateGFX(dt)
  if not session then return end
  local ok, err = pcall(drive, dt)
  if not ok then
    log('E', 'beamtelFuelPark', tostring(err))
    if failing then finish('failed', 'Parking fault. Brake now') else fail('Parking fault') end
  end
end

-- For live checks: would the look-ahead stop the car here, ahead (1) or behind (-1)?
function M.lookAhead(sign, reach, profile)
  local _, fwd, left = frame()
  local saveRays = rays
  if profile then rays = profile end
  local centre = geom and select(1, points()) or obj:getPosition()
  local hit = obstacle(centre, fwd, left, sign or 1, reach or 1.8)
  rays = saveRays
  return hit
end

function M.onReset()
  if session then finish('failed', 'Vehicle reset') end
end

function M.diagnosticState()
  return {active = session ~= nil, stopping = stopping, failing = failing, lastReason = lastReason,
    trace = trace, phase = phase, manoeuvres = manoeuvres, why = manoeuvreLog, aiMode = ai.mode, aiStarts = aiStarts,
    nodes = #nodes, ownMap = ownMap, total = total, bestRemaining = bestRemaining,
    elapsed = elapsed, gearbox = electrics.values.gearboxMode, geom = geom, rays = rays,
    damage = damageGain}
end

function M.onExtensionUnloaded()
  if session then finish('failed', 'Parking stopped') end
end

return M
