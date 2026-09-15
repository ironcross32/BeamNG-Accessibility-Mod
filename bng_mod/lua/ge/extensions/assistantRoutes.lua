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
