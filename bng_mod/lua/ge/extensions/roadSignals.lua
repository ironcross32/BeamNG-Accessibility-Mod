-- Read-only signal selection and front-node distance to mapped stopping points.
local M = {}

local states = {
  greenTrafficLight = "green", yellowTrafficLight = "yellow", redTrafficLight = "red",
  redYellowTrafficLight = "redYellow", greenFlashingTrafficLight = "flashingGreen",
  yellowFlashingTrafficLight = "flashingYellow", redFlashingTrafficLight = "flashingRed",
  none = "off",
}

function M.frontPosition(player)
  local origin, forward = player:getPosition(), player:getDirectionVector()
  local best, extent, cid = nil, -math.huge, nil
  -- Node positions are world-oriented offsets from the reference node. Use live
  -- geometry, including deformation and moving implements, instead of a fixed
  -- centre-to-front correction.
  pcall(function()
    for i = 0, player:getNodeCount() - 1 do
      local point = vec3(player:getNodePosition(i))
      local along = point:dot(forward)
      if along > extent then best, extent, cid = point, along, i end
    end
  end)
  return best and origin + best or origin, cid, best and extent or 0
end

function M.warningDistance(speed)
  -- Eight seconds of notice, with room for three seconds of reaction followed
  -- by moderate 3 m/s^2 braking at higher speeds.
  return math.min(400, math.max(60, speed * 8, speed * 3 + speed * speed / 6))
end

function M.proximity(signal, junction, speed, activeId)
  local target
  -- Do not guide past an earlier junction towards a more distant traffic light.
  if signal and (not junction or signal.distance <= junction.centreDistance) then
    local stop = not signal.preview and (signal.state == 'stop' or signal.state == 'red'
      or signal.state == 'redYellow' or signal.state == 'flashingRed')
    target = {id = 'signal:' .. signal.id, distance = signal.distance,
      source = 'stopPoint', action = stop and 'stop' or 'approach'}
  elseif junction then
    target = {id = 'junction:' .. junction.id, distance = junction.distance,
      source = 'junctionBoundary', action = 'approach'}
  end
  if target and (target.id == activeId or target.distance <= M.warningDistance(speed)) then
    target.speed = speed
    return target
  end
end

function M.matchRoad(position, forward, edges, project)
  local best, bestProjection, bestSign, bestScore
  for _, edge in ipairs(edges) do
    local p = project(position, edge)
    if p.rawT >= 0 and p.rawT <= 1 and math.abs(p.dz) <= 6
      and p.distance <= p.radius + 0.5 then
      local tangent = vec3(edge.outPos.x-edge.inPos.x, edge.outPos.y-edge.inPos.y, 0):normalized()
      local dot = forward:dot(tangent)
      local sign = dot >= 0 and 1 or -1
      if math.abs(dot) >= 0.5 and (not edge.oneWay or sign == 1) then
        -- An overlapping slip lane can be closer even while the car follows the
        -- main road. Prefer alignment with the nose, then lateral containment.
        local score = 1 - math.abs(dot) + 0.01 * p.distance / p.radius
        if not bestScore or score < bestScore then
          best, bestProjection, bestSign, bestScore = edge, p, sign, score
        end
      end
    end
  end
  return best, bestProjection, bestSign
end

function M.find(api, edge, projection, directionSign, forward, outgoing, project, maxDistance)
  if not api or not api.getData or not api.getSignals then return nil, "unsupported" end
  local data = api.getData()
  if not data.loaded then return nil, "unsupported" end
  if not data.active then return nil, "inactive" end
  local byRoad = {}
  for _, signal in ipairs(api.getSignals()) do
    if signal.road and not signal._invalid then
      local key = tostring(signal.road.n1) .. ':' .. tostring(signal.road.n2)
      byRoad[key] = byRoad[key] or {}
      table.insert(byRoad[key], signal)
    end
  end
  local offset = directionSign > 0 and projection.rawT * edge.length
    or (1 - projection.rawT) * edge.length
  local budget = 256
  local function walk(edge, directionSign, travelled, depth)
    budget = budget - 1
    if budget < 0 or depth > 128 or travelled - offset > maxDistance then return nil end
    local from = directionSign > 0 and edge.inNode or edge.outNode
    local to = directionSign > 0 and edge.outNode or edge.inNode
    local a = directionSign > 0 and edge.inPos or edge.outPos
    local b = directionSign > 0 and edge.outPos or edge.inPos
    local tangent = vec3(b.x - a.x, b.y - a.y, 0):normalized()
    -- A one-way road's legal direction must not mask the player facing backwards.
    if depth == 1 and forward:dot(tangent) < 0.5 then return nil end
    local best
    for _, signal in ipairs(byRoad[tostring(from) .. ':' .. tostring(to)] or {}) do
      local road = signal.road
      if not signal._invalid and road and road.n1 == from and road.n2 == to then
        local controller = signal:getController()
        local kind = controller and (controller.type == "signStop" and "stopSign"
          or tostring(controller.type):match("^lights") and "trafficLight")
        if kind and (signal.active or kind == "trafficLight") then
          local p = project(signal.pos, edge)
          local along = directionSign > 0 and p.rawT * edge.length
            or (1 - p.rawT) * edge.length
          local distance = travelled + along - offset
          if distance >= 0 and distance <= maxDistance and signal.dir:dot(tangent) >= 0.5
            and math.abs(p.dz) <= 6 and p.distance <= p.radius + 3
            and (not best or distance < best.distance) then
            local state = signal:getState()
            best = {id = tostring(signal.id), name = signal.name, kind = kind,
              groupId = kind .. ':' .. tostring(signal.intersectionId or signal.id),
              state = kind == "stopSign" and "stop" or (states[state] or "unknown"),
              distance = distance, phase = distance <= 12 and "near" or "approach"}
          end
        end
      end
    end
    if best then return best end
    travelled = travelled + edge.length
    local choices = outgoing(to, edge, tangent)
    if #choices == 0 then return nil end
    if #choices == 1 then
      return walk(choices[1].edge, choices[1].forward and 1 or -1, travelled, depth + 1)
    end
    local ranked = {}
    for _, choice in ipairs(choices) do
      local d = choice.forward and choice.edge.outPos-choice.edge.inPos or choice.edge.inPos-choice.edge.outPos
      d.z = 0
      local angle = math.deg(math.acos(math.max(-1, math.min(1, tangent:dot(d:normalized())))))
      ranked[#ranked + 1] = {choice = choice, angle = angle}
    end
    table.sort(ranked, function(a, b) return a.angle < b.angle end)
    -- Look past side streets along a clear continuation. At a close fork require
    -- agreement from both arms instead of guessing the driver's intended turn.
    if ranked[1].angle <= 30 and ranked[2].angle - ranked[1].angle >= 25 then
      local choice = ranked[1].choice
      local result = walk(choice.edge, choice.forward and 1 or -1, travelled, depth + 1)
      if result and ranked[2].angle <= 60 then
        -- A shallow slip road can bypass the light. Give advance notice on the
        -- road ahead without claiming its colour controls the unchosen arm.
        result.preview = true
        result.id = 'preview:' .. result.groupId
        if result.kind == 'trafficLight' then result.state = 'unknown' end
      end
      return result
    end
    local common
    for _, item in ipairs(ranked) do
      local choice = item.choice
      local result = walk(choice.edge, choice.forward and 1 or -1, travelled, depth + 1)
      if not result or (common and result.groupId ~= common.groupId) then return nil end
      if not common or result.distance < common.distance then common = result end
    end
    -- Both arms reach the same controlled junction. Warn now, but withhold light
    -- colour until the vehicle commits to a particular approach.
    common.preview = true
    common.id = 'preview:' .. common.groupId
    if common.kind == 'trafficLight' then common.state = 'unknown' end
    return common
  end
  local result = walk(edge, directionSign, 0, 1)
  return result, result and "available" or "none"
end

return M
