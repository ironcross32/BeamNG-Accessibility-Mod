-- Pure route choices, shared by the navigation adapter and focused diagnostics.
local M = {}

function M.direction(value)
  if value < -0.25 then return 'left' end
  if value > 0.25 then return 'right' end
  return 'straight'
end

function M.classify(bearing)
  if math.abs(bearing) > 150 then return nil end
  if math.abs(bearing) <= 30 then return 'straight' end
  return bearing > 0 and 'left' or 'right'
end

function M.choices(exits)
  local result = {}
  local targets = {left = 90, straight = 0, right = -90}
  for _, exit in ipairs(exits) do
    local name = M.classify(exit.bearing)
    if name then
      local old = result[name]
      local score = math.abs(exit.bearing - targets[name])
      if not old or score < old.score or (score == old.score and exit.id < old.id) then
        exit.score = score
        result[name] = exit
      end
    end
  end
  return result
end

function M.nodes(records)
  if #records == 0 then return nil end
  local result = {records[1].from}
  for _, record in ipairs(records) do
    if result[#result] ~= record.from then return nil end
    result[#result + 1] = record.to
  end
  return result
end

-- A* over directed road records. neighbours(record, visit, parent) calls
-- visit(nextRecord, stepCost) for each onward record; parent maps a record to the
-- one before it, for rules about the road already driven. Ties pop in insertion
-- order, so a given graph always gives the same route. Returns the records from
-- start to goal, or nil if there is none within maxExpansions.
function M.search(start, neighbours, isGoal, heuristic, maxExpansions)
  local heap, size, order = {}, 0, 0
  local function before(a, b) return a.f < b.f or (a.f == b.f and a.order < b.order) end
  local function push(f, record)
    order = order + 1
    size = size + 1
    local item, i = {f = f, order = order, record = record}, size
    while i > 1 do
      local up = math.floor(i / 2)
      if not before(item, heap[up]) then break end
      heap[i] = heap[up]
      i = up
    end
    heap[i] = item
  end
  local function pop()
    local top, item = heap[1], heap[size]
    heap[size] = nil
    size = size - 1
    if size > 0 then
      local i = 1
      while true do
        local child = i * 2
        if child > size then break end
        if child < size and before(heap[child + 1], heap[child]) then child = child + 1 end
        if not before(heap[child], item) then break end
        heap[i] = heap[child]
        i = child
      end
      heap[i] = item
    end
    return top.record
  end
  local cost, parent, closed = {[start] = 0}, {}, {}
  local expansions, limit = 0, maxExpansions or 200000
  push(heuristic(start), start)
  while size > 0 do
    local record = pop()
    if not closed[record] then
      closed[record] = true
      if isGoal(record) then
        local path, step = {}, record
        while step do
          table.insert(path, 1, step)
          step = parent[step]
        end
        return path, expansions
      end
      expansions = expansions + 1
      if expansions > limit then return nil, expansions end
      local base = cost[record]
      neighbours(record, function(nextRecord, stepCost)
        local total = base + stepCost
        if not closed[nextRecord] and (not cost[nextRecord] or total < cost[nextRecord]) then
          cost[nextRecord], parent[nextRecord] = total, record
          push(total + heuristic(nextRecord), nextRecord)
        end
      end, parent)
    end
  end
  return nil, expansions
end

-- ================================================================================================
--  Three point turn
-- ================================================================================================
-- The player has the pedals and the gears; the steering is held at full lock and
-- this decides when each leg ends. Steering is negative left. Forward on left lock
-- and backward on right lock both swing the nose left, so `side` alone fixes both.
local TURN_DONE_DEG, TURN_MAX_MOVES, TURN_STOP_MARGIN_M, TURN_LIMIT_S = 25, 9, 0.7, 180
local TURN_REPEAT_S = 2

function M.turnStopDistance(speed)
  local size = math.abs(speed)
  return TURN_STOP_MARGIN_M + 0.3 * size + size * size / 6
end

-- `err` is how far the nose has to swing, positive left. Facing almost dead away
-- either way round will do, so the roomier side wins.
function M.turnBegin(err, front, rear, left, right)
  local side = err >= 0 and 1 or -1
  if math.abs(err) >= 150 then side = left >= right and 1 or -1 end
  return {side = side, phase = front >= rear and 'forward' or 'reverse', moves = 0, waiting = true,
    time = 0, repeatAge = 0}
end

function M.turnPhrase(phase) return phase == 'forward' and 'Forward' or 'Reverse' end

local function inGear(phase, gear, speed)
  if phase == 'forward' then return gear > 0 or speed > 0.3 end
  return gear < 0 or speed < -0.3
end

-- input: err, speed (signed, forward positive), gear (index), front, rear (metres
-- of room each way), dt. Returns {steering, say, done, fail}.
function M.turnStep(turn, input)
  local result = {}
  turn.time, turn.repeatAge = turn.time + input.dt, turn.repeatAge + input.dt
  local function flip()
    turn.phase = turn.phase == 'forward' and 'reverse' or 'forward'
    turn.moves, turn.waiting, turn.repeatAge = turn.moves + 1, true, 0
    result.say = 'Stop. ' .. M.turnPhrase(turn.phase)
  end
  local finished = math.abs(input.err) <= TURN_DONE_DEG
  if turn.time > TURN_LIMIT_S then
    result.fail = 'Could not turn round'
  elseif finished and turn.phase == 'reverse' then
    flip()
  elseif finished and inGear('forward', input.gear, input.speed) then
    result.done = true
  elseif turn.waiting then
    if inGear(turn.phase, input.gear, input.speed) then
      turn.waiting = false
    elseif turn.repeatAge >= TURN_REPEAT_S and math.abs(input.speed) > 0.5 then
      -- Still rolling the old way, towards whatever ended that leg.
      turn.repeatAge = 0
      result.say = 'Stop. ' .. M.turnPhrase(turn.phase)
    end
  elseif not finished then
    local room = turn.phase == 'forward' and input.front or input.rear
    if room <= M.turnStopDistance(input.speed) then
      if turn.moves >= TURN_MAX_MOVES then
        result.fail = 'Could not turn round'
      else
        local other = turn.phase == 'forward' and input.rear or input.front
        if other <= TURN_STOP_MARGIN_M then
          result.fail = 'No room to turn round'
        else
          flip()
        end
      end
    end
  end
  result.steering = turn.side * (turn.phase == 'forward' and -1 or 1)
  return result
end

function M.obstacleHit(records, hit, project)
  local nearest
  for i = 1, math.min(#records, 8) do
    local p = project(hit, records[i].edge)
    if math.abs(p.dz) <= 6 and (not nearest or p.distance < nearest.distance) then nearest = p end
  end
  if not nearest then return false, 'outsideRoute' end
  local height = -nearest.dz
  if nearest.distance > nearest.radius then return false, 'outsideRoute', height end
  -- A nose ray can hit the pavement when the car pitches or the road rises.
  -- Graph elevation is approximate, so reserve obstacle speech for hits clearly
  -- above the road, within the connected route that the assistant will follow.
  if height <= 0.6 then return false, 'roadSurface', height end
  return true, 'obstruction', height
end

function M.reverseTarget(state, position, facing, speed, project)
  local best, index
  for i, record in ipairs(state.records) do
    local p = project(position, record.edge)
    if math.abs(p.dz) <= 6 and (not best or p.distance < best.distance - 0.1) then best, index = p, i end
  end
  if not best or best.distance > best.radius + 25 then return nil, 'No reachable reverse route. Stop' end
  local record = state.records[index]
  local function tangent(r)
    local direction = r.forward and r.edge.outPos-r.edge.inPos or r.edge.inPos-r.edge.outPos
    direction.z = 0
    return direction:normalized()
  end
  local direction = tangent(record)
  if not state.reverseSign then
    state.reverseSign = facing:dot(direction) >= 0 and -1 or 1
    local right = vec3(direction.y, -direction.x, 0)
    state.reverseLane = math.max(-best.radius * 0.5,
      math.min(best.radius * 0.5, (position-best.point):dot(right)))
  end
  local remaining = math.max(5, math.min(12, 5 + speed))
  local along = (record.forward and best.t or 1-best.t) * record.edge.length
  local exhausted = false
  while true do
    local available = state.reverseSign > 0 and record.edge.length-along or along
    if remaining <= available then along = along + state.reverseSign * remaining; break end
    remaining = remaining - available
    local nextRecord = state.records[index + state.reverseSign]
    if not nextRecord then
      along = state.reverseSign > 0 and record.edge.length or 0
      exhausted = true
      break
    end
    index, record = index + state.reverseSign, nextRecord
    along = state.reverseSign > 0 and 0 or record.edge.length
  end
  local fraction = math.max(0, math.min(1, along / math.max(0.01, record.edge.length)))
  if not record.forward then fraction = 1-fraction end
  local target = record.edge.inPos + (record.edge.outPos-record.edge.inPos) * fraction
  direction = tangent(record)
  target = target + vec3(direction.y, -direction.x, 0) * state.reverseLane
  local delta = target-position
  delta.z = 0
  local behind = -delta:dot(facing)
  local blocked = behind < 0.5 or (exhausted and delta:length() < 2)
  state.offset = math.max(0, best.distance-best.radius)
  return {target={target.x,target.y,target.z}, blocked=blocked,
    reason=blocked and 'End of reverse guidance. Stop and reposition' or nil,
    distance=delta:length(), offset=state.offset}
end

return M
