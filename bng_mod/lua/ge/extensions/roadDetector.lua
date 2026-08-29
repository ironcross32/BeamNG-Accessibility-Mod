-- BeamTel road awareness (GE extension).
-- R2 packets precede legacy packets so new and old executables can share this mod.

local M = {}

local PYTHON_HOST = "127.0.0.1"
local PYTHON_PORT_DATA = 4462
local CMD_LISTEN_PORT = 4463
local SCAN_INTERVAL = 0.2
local SEARCH_RADIUS_M = 500.0
local BUCKET_SIZE_M = 50.0
local OVERPASS_Z_TOLERANCE = 6.0
local DORMANT_AFTER_MISSES = 5
local ENTER_SLACK_M = 0.5
local EXIT_SLACK_M = 2.0
local ENTER_TICKS = 2
local EXIT_TICKS = 3
local MIN_DRIVABILITY = 0.25
local JUNCTION_SEARCH_M = 240.0
local CORRECTION_HEADING_ENTER_DEG = 6.0
local CORRECTION_HEADING_EXIT_DEG = 3.0
-- BeamNG's radius is the whole road half-width, not a lane half-width. A
-- correctly driven two-way lane commonly sits around ratio 0.5, so centreline
-- thresholds below that turn ordinary lane keeping into a permanent warning.
local CORRECTION_LATERAL_ENTER_RATIO = 0.75
local CORRECTION_LATERAL_EXIT_RATIO = 0.60
local DRIFT_MIN_OUTWARD_SPEED_MPS = 0.20
local DRIFT_WARNING_SECONDS = 3.0
local DRIFT_SPEED_SMOOTH_ALPHA = 0.35
local OFFROAD_MERGE_ANGLE_DEG = 30.0
local OFFROAD_INTERCEPT_MIN_M = 20.0
local OFFROAD_INTERCEPT_MAX_M = 180.0
local JUNCTION_ENTRY_COMPENSATION_S = SCAN_INTERVAL * 0.5

local udpSend, udpCmd = nil, nil
local isActive, isDormant = false, false
local includePrivate = false
local scanTimer, missCount = 0, 0
local edges, adjacency, buckets = {}, {}, {}
local currentEdge, onRoad = nil, false
local enterTicks, exitTicks = 0, 0
local orientationArmed = true
local correctionActive, correctionClearTicks = false, 0
local activeJunctionId = nil
local lastVehicleId = nil
local lateralTrackEdgeId, lateralTrackDirection = nil, nil
local lastSignedLateral, smoothedLateralSpeed = nil, 0

local function rdLog(level, message) log(level, 'RoadDetector', message) end
local function clamp(value, lo, hi) return math.max(lo, math.min(hi, value)) end
local function keyForBucket(x, y)
  return tostring(math.floor(x / BUCKET_SIZE_M)) .. ":" .. tostring(math.floor(y / BUCKET_SIZE_M))
end
local function flatLength(v) return math.sqrt(v.x * v.x + v.y * v.y) end
local function flatDirection(a, b)
  local result = vec3(b.x - a.x, b.y - a.y, 0)
  local length = result:length()
  if length < 1e-6 then return nil end
  return result / length
end
local function edgeLength(edge)
  local dx, dy = edge.outPos.x - edge.inPos.x, edge.outPos.y - edge.inPos.y
  return math.sqrt(dx * dx + dy * dy)
end
local function horizontalDistance(a, b)
  local dx, dy = a.x - b.x, a.y - b.y
  return math.sqrt(dx * dx + dy * dy)
end
local function truthy(value)
  return value == true or value == 1 or value == "1" or value == "true"
end
local function isPrivateEdge(data)
  return truthy(data.private) or truthy(data.isPrivate) or truthy(data.gated)
    or truthy(data.isGated) or truthy(data.gatedRoad) or truthy(data.privateRoad)
end

local function bearingToDir(reference, direction)
  local crossZ = reference.x * direction.y - reference.y * direction.x
  local dot = reference.x * direction.x + reference.y * direction.y
  return math.deg(math.atan2(crossZ, dot))
end

local function projectOnEdge(position, edge)
  local a, b = edge.inPos, edge.outPos
  local abx, aby = b.x - a.x, b.y - a.y
  local apx, apy = position.x - a.x, position.y - a.y
  local lengthSq = abx * abx + aby * aby
  local rawT = 0
  if lengthSq > 1e-9 then rawT = (apx * abx + apy * aby) / lengthSq end
  local t = clamp(rawT, 0, 1)
  local point = vec3(a.x + abx * t, a.y + aby * t, a.z + (b.z - a.z) * t)
  local radius = edge.inRadius + (edge.outRadius - edge.inRadius) * t
  return {
    point = point,
    t = t,
    rawT = rawT,
    radius = math.max(0.5, radius),
    distance = horizontalDistance(position, point),
    dz = point.z - position.z,
  }
end

local function addAdjacency(nodeId, record)
  adjacency[nodeId] = adjacency[nodeId] or {}
  table.insert(adjacency[nodeId], record)
end

local function addEdgeToBuckets(edge)
  local radius = math.max(edge.inRadius, edge.outRadius)
  local minX = math.floor((math.min(edge.inPos.x, edge.outPos.x) - radius) / BUCKET_SIZE_M)
  local maxX = math.floor((math.max(edge.inPos.x, edge.outPos.x) + radius) / BUCKET_SIZE_M)
  local minY = math.floor((math.min(edge.inPos.y, edge.outPos.y) - radius) / BUCKET_SIZE_M)
  local maxY = math.floor((math.max(edge.inPos.y, edge.outPos.y) + radius) / BUCKET_SIZE_M)
  for bx = minX, maxX do
    for by = minY, maxY do
      local key = tostring(bx) .. ":" .. tostring(by)
      buckets[key] = buckets[key] or {}
      table.insert(buckets[key], edge)
    end
  end
end

local function rebuildNavigationModel(reason)
  edges, adjacency, buckets = {}, {}, {}
  local mapData = map and map.getMap and map.getMap() or nil
  local nodes = mapData and mapData.nodes or nil
  if not nodes then
    rdLog('info', "Navigation graph is not ready during " .. tostring(reason) .. ".")
    return false
  end

  local seen = {}
  for sourceId, sourceNode in pairs(nodes) do
    local neighbours = sourceNode.links
    if type(neighbours) == "table" and nodes[sourceId] then
      for targetId, data in pairs(neighbours) do
        if type(data) == "table" and nodes[targetId] then
          local left, right = tostring(sourceId), tostring(targetId)
          local pairKey = left < right and (left .. "|" .. right) or (right .. "|" .. left)
          if not seen[pairKey] then
            seen[pairKey] = true
            local inId = data.inNode or sourceId
            local outId = data.outNode or targetId
            if not nodes[inId] or not nodes[outId] then
              inId, outId = sourceId, targetId
            end
            local inNode, outNode = nodes[inId], nodes[outId]
            local inPos = data.inPos or inNode.pos
            local outPos = data.outPos or outNode.pos
            if inPos and outPos then
              local edge = {
                id = pairKey,
                inNode = inId,
                outNode = outId,
                inPos = vec3(inPos.x, inPos.y, inPos.z),
                outPos = vec3(outPos.x, outPos.y, outPos.z),
                inRadius = tonumber(data.inRadius) or tonumber(inNode.radius) or 4.0,
                outRadius = tonumber(data.outRadius) or tonumber(outNode.radius) or 4.0,
                oneWay = truthy(data.oneWay),
                drivability = tonumber(data.drivability) or 1.0,
                private = isPrivateEdge(data),
                raw = data,
              }
              edge.length = edgeLength(edge)
              if edge.length > 0.05 then
                table.insert(edges, edge)
                addAdjacency(inId, {edge = edge, from = inId, to = outId, forward = true})
                addAdjacency(outId, {edge = edge, from = outId, to = inId, forward = false})
                addEdgeToBuckets(edge)
              end
            end
          end
        end
      end
    end
  end
  rdLog('info', string.format("Cached %d navigation edges (%s).", #edges, tostring(reason)))
  return #edges > 0
end

local function edgeDirection(edge, directionSign)
  local direction = flatDirection(edge.inPos, edge.outPos)
  if not direction then return nil end
  if directionSign < 0 then return vec3(-direction.x, -direction.y, 0) end
  return direction
end

local function legalRecord(record)
  if record.edge.drivability < MIN_DRIVABILITY then return false end
  if record.edge.private and not includePrivate then return false end
  if record.edge.oneWay and not record.forward then return false end
  return true
end

local function outgoing(nodeId, incomingEdge, incomingDirection)
  local result = {}
  for _, record in ipairs(adjacency[nodeId] or {}) do
    if record.edge ~= incomingEdge and legalRecord(record) then
      local direction = edgeDirection(record.edge, record.forward and 1 or -1)
      if direction and (not incomingDirection or math.abs(bearingToDir(incomingDirection, direction)) <= 150) then
        record.direction = direction
        table.insert(result, record)
      end
    end
  end
  return result
end

local function candidateEdges(position)
  local result, seen = {}, {}
  local radiusBuckets = math.ceil(SEARCH_RADIUS_M / BUCKET_SIZE_M)
  local centerX = math.floor(position.x / BUCKET_SIZE_M)
  local centerY = math.floor(position.y / BUCKET_SIZE_M)
  for bx = centerX - radiusBuckets, centerX + radiusBuckets do
    for by = centerY - radiusBuckets, centerY + radiusBuckets do
      for _, edge in ipairs(buckets[tostring(bx) .. ":" .. tostring(by)] or {}) do
        if not seen[edge.id] then
          seen[edge.id] = true
          table.insert(result, edge)
        end
      end
    end
  end
  return result
end

local function nearestCompatibleEdge(position)
  local bestEdge, bestProjection = nil, nil
  for _, edge in ipairs(candidateEdges(position)) do
    local projection = projectOnEdge(position, edge)
    if math.abs(projection.dz) <= OVERPASS_Z_TOLERANCE
      and projection.distance <= SEARCH_RADIUS_M
      and (not bestProjection or projection.distance < bestProjection.distance) then
      bestEdge, bestProjection = edge, projection
    end
  end
  return bestEdge, bestProjection
end

local function projectionInsideExitBoundary(projection, edge)
  local extension = (projection.radius + EXIT_SLACK_M) / math.max(1.0, edge.length)
  return math.abs(projection.dz) <= OVERPASS_Z_TOLERANCE
    and projection.distance <= projection.radius + EXIT_SLACK_M
    and projection.rawT >= -extension and projection.rawT <= 1 + extension
end

local function resolvedTravel(player, vehicleForward, edge)
  local velocity = player:getVelocity()
  local reference = vec3(velocity.x, velocity.y, 0)
  if reference:length() <= 1.0 then reference = vec3(vehicleForward.x, vehicleForward.y, 0) end
  if reference:length() < 1e-6 then reference = vec3(1, 0, 0) end
  reference = reference:normalized()
  local forward = edgeDirection(edge, 1)
  if not forward then return reference, 1 end
  if edge.oneWay then return forward, 1 end
  if reference:dot(forward) >= 0 then return reference, 1 end
  return reference, -1
end

local function pointAlong(edge, directionSign, distanceFromStart)
  local t
  if directionSign > 0 then t = distanceFromStart / edge.length
  else t = 1 - distanceFromStart / edge.length end
  t = clamp(t, 0, 1)
  return vec3(
    edge.inPos.x + (edge.outPos.x - edge.inPos.x) * t,
    edge.inPos.y + (edge.outPos.y - edge.inPos.y) * t,
    edge.inPos.z + (edge.outPos.z - edge.inPos.z) * t
  )
end

local function followLookahead(edge, projection, directionSign, lookahead)
  local remaining = directionSign > 0 and edge.length * (1 - projection.t) or edge.length * projection.t
  if lookahead <= remaining then
    local fromStart = directionSign > 0 and edge.length * projection.t + lookahead
      or edge.length * (1 - projection.t) + lookahead
    return pointAlong(edge, directionSign, fromStart), false
  end
  local distanceLeft = lookahead - remaining
  local nodeId = directionSign > 0 and edge.outNode or edge.inNode
  local incoming, incomingDir = edge, edgeDirection(edge, directionSign)
  for _ = 1, 24 do
    local choices = outgoing(nodeId, incoming, incomingDir)
    if #choices == 0 then
      return directionSign > 0 and edge.outPos or edge.inPos, false
    end
    if #choices > 1 then return nil, true end
    local choice = choices[1]
    local sign = choice.forward and 1 or -1
    if distanceLeft <= choice.edge.length then
      return pointAlong(choice.edge, sign, distanceLeft), false
    end
    distanceLeft = distanceLeft - choice.edge.length
    incoming, incomingDir = choice.edge, choice.direction
    nodeId = choice.to
    edge, directionSign = choice.edge, sign
  end
  return nil, false
end

local function offRoadIntercept(edge, projection, position, vehicleForward, speed)
  local forward = vec3(vehicleForward.x, vehicleForward.y, 0)
  if forward:length() < 1e-6 then forward = vec3(1, 0, 0) else forward = forward:normalized() end
  local edgeForward = edgeDirection(edge, 1)
  local preferredSign = 1
  if not edge.oneWay and edgeForward and forward:dot(edgeForward) < 0 then preferredSign = -1 end
  local lead = clamp(
    projection.distance / math.tan(math.rad(OFFROAD_MERGE_ANGLE_DEG)) + speed * 1.5,
    OFFROAD_INTERCEPT_MIN_M, OFFROAD_INTERCEPT_MAX_M)

  local function candidate(directionSign)
    local target = followLookahead(edge, projection, directionSign, lead)
    if not target then target = directionSign > 0 and edge.outPos or edge.inPos end
    return target, horizontalDistance(projection.point, target)
  end

  local target, alongDistance = candidate(preferredSign)
  if not edge.oneWay and alongDistance < lead * 0.5 then
    local alternate, alternateAlong = candidate(-preferredSign)
    if alternateAlong > alongDistance + 5 then target = alternate end
  end
  return target, horizontalDistance(position, target)
end

local function clusterBearings(values)
  table.sort(values)
  local clusters = {}
  for _, value in ipairs(values) do
    local last = clusters[#clusters]
    if not last or math.abs(value - last.mean) > 15 then
      table.insert(clusters, {sum = value, count = 1, mean = value})
    else
      last.sum = last.sum + value
      last.count = last.count + 1
      last.mean = last.sum / last.count
    end
  end
  local result = {}
  for _, cluster in ipairs(clusters) do table.insert(result, cluster.mean) end
  table.sort(result, function(a, b) return a > b end)
  return result
end

local function classifyJunction(exits, complex)
  if #exits == 0 then return "deadEnd" end
  if complex then return "complex" end
  local hasLeft, hasRight, hasStraight = false, false, false
  local allForward = true
  for _, bearing in ipairs(exits) do
    local magnitude = math.abs(bearing)
    if magnitude <= 15 then hasStraight = true
    elseif bearing > 0 then hasLeft = true else hasRight = true end
    if magnitude > 45 then allForward = false end
  end
  if #exits == 2 and allForward and hasLeft and hasRight then return "fork" end
  if #exits == 2 and hasLeft and hasRight and not hasStraight then return "tJunction" end
  if #exits >= 3 and hasLeft and hasRight and hasStraight then return "crossroads" end
  return "intersection"
end

local function junctionZone(startNode, incomingEdge, incomingDirection, initialChoices)
  local zone, queue = {[startNode] = true}, {startNode}
  local startRecords = adjacency[startNode] or {}
  local startPos = nil
  if #startRecords > 0 then
    local record = startRecords[1]
    startPos = record.from == record.edge.inNode and record.edge.inPos or record.edge.outPos
  end
  local head = 1
  while head <= #queue do
    local nodeId = queue[head]
    head = head + 1
    for _, record in ipairs(adjacency[nodeId] or {}) do
      if legalRecord(record) and record.edge ~= incomingEdge and not zone[record.to]
        and record.edge.length <= 35 then
        local targetPos = record.to == record.edge.inNode and record.edge.inPos or record.edge.outPos
        local close = not startPos or horizontalDistance(startPos, targetPos) <= 40
        local targetChoices = outgoing(record.to, record.edge, record.direction)
        if close and #targetChoices >= 2 then
          zone[record.to] = true
          table.insert(queue, record.to)
        end
      end
    end
  end

  local exitBearings = {}
  for nodeId, _ in pairs(zone) do
    for _, record in ipairs(adjacency[nodeId] or {}) do
      if legalRecord(record) and record.edge ~= incomingEdge and not zone[record.to] then
        local direction = edgeDirection(record.edge, record.forward and 1 or -1)
        if direction then
          local bearing = bearingToDir(incomingDirection, direction)
          if math.abs(bearing) <= 150 then table.insert(exitBearings, bearing) end
        end
      end
    end
  end
  if #exitBearings == 0 then
    for _, choice in ipairs(initialChoices or {}) do
      table.insert(exitBearings, bearingToDir(incomingDirection, choice.direction))
    end
  end
  local ids = {}
  for nodeId, _ in pairs(zone) do table.insert(ids, tostring(nodeId)) end
  table.sort(ids)
  return clusterBearings(exitBearings), ids
end

local function findJunctionAhead(edge, projection, directionSign)
  local distance = directionSign > 0 and edge.length * (1 - projection.t) or edge.length * projection.t
  local nodeId = directionSign > 0 and edge.outNode or edge.inNode
  local incoming, incomingDir = edge, edgeDirection(edge, directionSign)
  for _ = 1, 48 do
    if distance > JUNCTION_SEARCH_M or not incomingDir then return nil end
    local choices = outgoing(nodeId, incoming, incomingDir)
    if #choices ~= 1 then
      local exits, ids = junctionZone(nodeId, incoming, incomingDir, choices)
      local nodeRadius = 0
      for _, record in ipairs(adjacency[nodeId] or {}) do
        local atIn = record.from == record.edge.inNode
        nodeRadius = math.max(nodeRadius, atIn and record.edge.inRadius or record.edge.outRadius)
      end
      return {
        id = table.concat(ids, "+"),
        kind = classifyJunction(exits, #ids > 1),
        distance = math.max(0, distance - nodeRadius),
        exits = exits,
      }
    end
    local choice = choices[1]
    distance = distance + choice.edge.length
    nodeId, incoming, incomingDir = choice.to, choice.edge, choice.direction
  end
  return nil
end

local function makeRoadDirections(vehicleForward, edge)
  local flat = vec3(vehicleForward.x, vehicleForward.y, 0)
  if flat:length() < 1e-6 then flat = vec3(1, 0, 0) else flat = flat:normalized() end
  local forward = edgeDirection(edge, 1)
  if not forward then return {} end
  local result = {bearingToDir(flat, forward)}
  if not edge.oneWay then table.insert(result, bearingToDir(flat, vec3(-forward.x, -forward.y, 0))) end
  table.sort(result, function(a, b) return math.abs(a) < math.abs(b) end)
  return result
end

local function sendPacket(packet, legacy)
  if not udpSend then return end
  local ok, encoded = pcall(jsonEncode, packet)
  if ok and encoded then udpSend:send("R2|" .. encoded) end
  udpSend:send(legacy)
end

local function dormantPacket()
  return {state = "dormant", oneWay = false, roadDirections = {}, offRoad = nil,
    correction = nil, junction = nil}
end

local function resetLateralTracking()
  lateralTrackEdgeId, lateralTrackDirection = nil, nil
  lastSignedLateral, smoothedLateralSpeed = nil, 0
end

local function resetTracking(reason)
  isDormant, missCount, scanTimer = false, 0, 0
  currentEdge, onRoad = nil, false
  enterTicks, exitTicks = 0, 0
  orientationArmed = true
  correctionActive, correctionClearTicks = false, 0
  activeJunctionId = nil
  lastVehicleId = nil
  resetLateralTracking()
  if reason then rdLog('info', "Re-arming road detector: " .. reason) end
end

local function performScan()
  if not udpSend then return end
  local player = be:getPlayerVehicle(0)
  if not player then return end
  local vehicleId = player:getID()
  if lastVehicleId and lastVehicleId ~= vehicleId then resetTracking("vehicle change") end
  lastVehicleId = vehicleId

  if #edges == 0 then
    missCount = missCount + 1
    if missCount >= DORMANT_AFTER_MISSES then
      if not isDormant then rdLog('info', "No roads found on this map; detector is dormant.") end
      isDormant = true
      sendPacket(dormantPacket(), "DORMANT")
    end
    return
  end
  isDormant, missCount = false, 0

  local position = player:getPosition()
  local vehicleForward = player:getDirectionVector()
  local edge, projection = currentEdge, nil
  if edge then
    projection = projectOnEdge(position, edge)
    if not projectionInsideExitBoundary(projection, edge) then edge, projection = nil, nil end
  end
  if not edge then edge, projection = nearestCompatibleEdge(position) end
  if not edge or not projection then
    enterTicks = 0
    if onRoad then
      exitTicks = exitTicks + 1
      if exitTicks < EXIT_TICKS then return end
    end
    onRoad, currentEdge, orientationArmed = false, nil, true
    correctionActive, correctionClearTicks = false, 0
    resetLateralTracking()
    -- The map has a navgraph, but nothing vertically compatible is within the
    -- bounded search. R2 can represent that without pointing at an overpass;
    -- legacy has no equivalent and receives its safest silent state.
    sendPacket({state = "offRoad", oneWay = false, roadDirections = {},
      offRoad = nil, correction = nil, junction = nil}, "DORMANT")
    return
  end

  local withinEnter = projection.distance <= projection.radius + ENTER_SLACK_M
    and math.abs(projection.dz) <= OVERPASS_Z_TOLERANCE
  local withinExit = projection.distance <= projection.radius + EXIT_SLACK_M
    and math.abs(projection.dz) <= OVERPASS_Z_TOLERANCE
  if onRoad then
    if withinExit then exitTicks = 0 else exitTicks = exitTicks + 1 end
    if exitTicks >= EXIT_TICKS then
      onRoad, currentEdge = false, nil
      enterTicks, exitTicks = 0, 0
      orientationArmed = true
      correctionActive, correctionClearTicks = false, 0
      resetLateralTracking()
    end
  else
    if withinEnter then enterTicks = enterTicks + 1 else enterTicks = 0 end
    if enterTicks >= ENTER_TICKS then
      onRoad, currentEdge = true, edge
      enterTicks, exitTicks = 0, 0
    end
  end

  local forwardFlat = vec3(vehicleForward.x, vehicleForward.y, 0)
  if forwardFlat:length() < 1e-6 then forwardFlat = vec3(1, 0, 0) else forwardFlat = forwardFlat:normalized() end
  if not onRoad then
    local speed = flatLength(player:getVelocity())
    local intercept, interceptDistance = offRoadIntercept(
      edge, projection, position, vehicleForward, speed)
    local toRoad = vec3(intercept.x - position.x, intercept.y - position.y, 0)
    local bearing = 0
    if toRoad:length() > 1e-6 then bearing = bearingToDir(forwardFlat, toRoad:normalized()) end
    sendPacket({state = "offRoad", oneWay = false, roadDirections = {},
      offRoad = {bearing = bearing, distance = interceptDistance}, correction = nil,
      junction = nil}, string.format("OFF_ROAD,%.2f,%.2f", bearing, interceptDistance))
    return
  end

  currentEdge = edge
  local travelReference, directionSign = resolvedTravel(player, vehicleForward, edge)
  local speed = flatLength(player:getVelocity())
  local lookaheadDistance = clamp(8 + speed * 2, 12, 70)
  local target, ambiguous = followLookahead(edge, projection, directionSign, lookaheadDistance)
  local junction = findJunctionAhead(edge, projection, directionSign)
  local earlyDistance = clamp(speed * 7, 30, 140)
  local nearDistance = clamp(speed * 2, 12, 35)
  local junctionPacket = nil
  local inDecisionZone = junction and junction.distance <= nearDistance
  if junction then
    if activeJunctionId and junction.id ~= activeJunctionId then activeJunctionId = nil end
    if junction.distance <= earlyDistance or junction.id == activeJunctionId then
      activeJunctionId = junction.id
      -- Compensate for half of the 5 Hz sampling interval. Without this, a
      -- fast vehicle can cross the physical boundary between scans and acquire
      -- an outgoing edge before ever emitting the entry marker.
      local entryDistance = math.max(0.1, speed * JUNCTION_ENTRY_COMPENSATION_S)
      junction.entered = junction.distance <= entryDistance
      junction.phase = junction.distance <= nearDistance and "near" or "approach"
      junctionPacket = junction
    end
  else
    activeJunctionId = nil
  end

  local correction = {active = false, bearing = 0, severity = 0}
  if target and not ambiguous and not inDecisionZone then
    local pathTangent = edgeDirection(edge, directionSign)
    if pathTangent then
      local headingError = bearingToDir(travelReference, pathTangent)
      local lateralRatio = projection.distance / math.max(0.5, projection.radius)
      local offsetX = position.x - projection.point.x
      local offsetY = position.y - projection.point.y
      local signedLateral = pathTangent.x * offsetY - pathTangent.y * offsetX
      if lateralTrackEdgeId ~= edge.id or lateralTrackDirection ~= directionSign then
        lateralTrackEdgeId, lateralTrackDirection = edge.id, directionSign
        lastSignedLateral, smoothedLateralSpeed = signedLateral, 0
      elseif lastSignedLateral then
        local rawLateralSpeed = (signedLateral - lastSignedLateral) / SCAN_INTERVAL
        smoothedLateralSpeed = smoothedLateralSpeed
          + DRIFT_SPEED_SMOOTH_ALPHA * (rawLateralSpeed - smoothedLateralSpeed)
        lastSignedLateral = signedLateral
      else
        lastSignedLateral = signedLateral
      end
      -- Positive means the vehicle is moving farther from the centreline on
      -- whichever side it currently occupies; steady lane offset is zero.
      local outwardSpeed = 0
      if signedLateral > 0 then outwardSpeed = smoothedLateralSpeed
      elseif signedLateral < 0 then outwardSpeed = -smoothedLateralSpeed end
      local distanceToBoundary = math.max(0, projection.radius - math.abs(signedLateral))
      local secondsToBoundary = math.huge
      if outwardSpeed > DRIFT_MIN_OUTWARD_SPEED_MPS then
        secondsToBoundary = distanceToBoundary / outwardSpeed
      end
      local driftingOutward = outwardSpeed > DRIFT_MIN_OUTWARD_SPEED_MPS
        and secondsToBoundary <= DRIFT_WARNING_SECONDS
      -- Aim back toward the safe road interior over a short, speed-adjusted
      -- distance. This gives lateral warnings a useful side without inheriting
      -- the natural bearing of a distant lookahead point around a bend.
      local lateralAimDistance = clamp(8 + speed, 12, 25)
      local lateralCorrection = math.deg(math.atan2(-signedLateral, lateralAimDistance))
      local correctionBearing = headingError + lateralCorrection
      if correctionBearing > 180 then correctionBearing = correctionBearing - 360 end
      if correctionBearing < -180 then correctionBearing = correctionBearing + 360 end
      local shouldEnter = math.abs(headingError) > CORRECTION_HEADING_ENTER_DEG
        or lateralRatio > CORRECTION_LATERAL_ENTER_RATIO
        or driftingOutward
      local safelyAligned = math.abs(headingError) < CORRECTION_HEADING_EXIT_DEG
        and lateralRatio < CORRECTION_LATERAL_EXIT_RATIO
        and not driftingOutward
      if correctionActive then
        if safelyAligned then correctionClearTicks = correctionClearTicks + 1 else correctionClearTicks = 0 end
        if correctionClearTicks >= 3 then correctionActive, correctionClearTicks = false, 0 end
      elseif shouldEnter then
        correctionActive, correctionClearTicks = true, 0
      end
      if correctionActive then
        local headingSeverity = clamp(
          (math.abs(headingError) - CORRECTION_HEADING_ENTER_DEG) / 39, 0, 1)
        local lateralSeverity = clamp(
          (lateralRatio - CORRECTION_LATERAL_ENTER_RATIO)
            / (1 - CORRECTION_LATERAL_ENTER_RATIO), 0, 1)
        local driftSeverity = 0
        if driftingOutward then
          local timeSeverity = clamp(
            1 - secondsToBoundary / DRIFT_WARNING_SECONDS, 0, 1)
          local speedSeverity = clamp(
            (outwardSpeed - DRIFT_MIN_OUTWARD_SPEED_MPS) / 1.3, 0, 1)
          driftSeverity = math.max(timeSeverity, speedSeverity)
        end
        correction = {active = true, bearing = correctionBearing,
          severity = math.max(0.05, headingSeverity, lateralSeverity, driftSeverity)}
      end
    end
  else
    correctionActive, correctionClearTicks = false, 0
    resetLateralTracking()
  end

  local orientation = orientationArmed
  orientationArmed = false
  local directions = makeRoadDirections(vehicleForward, edge)
  local packet = {state = "onRoad", oneWay = edge.oneWay, roadDirections = directions,
    offRoad = nil, correction = correction, junction = junctionPacket, orientation = orientation}
  local legacy = "ON_ROAD"
  if orientation and #directions > 0 then
    local second = directions[2] or directions[1]
    legacy = string.format("ON_ROAD,%.2f,%.2f", directions[1], second)
  end
  sendPacket(packet, legacy)
end

local function setupSockets()
  if udpSend then pcall(function() udpSend:close() end); udpSend = nil end
  if udpCmd then pcall(function() udpCmd:close() end); udpCmd = nil end
  udpSend = socket.udp()
  if udpSend then udpSend:setpeername(PYTHON_HOST, PYTHON_PORT_DATA); udpSend:settimeout(0) end
  local ok, err = pcall(function()
    udpCmd = socket.udp()
    local bound, bindError = udpCmd:setsockname("127.0.0.1", CMD_LISTEN_PORT)
    if not bound then error(tostring(bindError), 0) end
    udpCmd:settimeout(0)
  end)
  if not ok then
    rdLog('error', "Failed to bind road command socket: " .. tostring(err))
    if udpCmd then pcall(function() udpCmd:close() end) end
    udpCmd = nil
  end
end

local CMD_BIND_RETRY_S, cmdBindRetry = 3.0, 0
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
  if ok and udpCmd then rdLog('info', "Road command socket rebound after retry.") end
end

function M.onExtensionUnloaded()
  if udpSend then pcall(function() udpSend:close() end); udpSend = nil end
  if udpCmd then pcall(function() udpCmd:close() end); udpCmd = nil end
end

function M.onExtensionLoaded()
  setExtensionUnloadMode(M, "manual")
  setupSockets()
  rebuildNavigationModel("extension load")
  resetTracking("extension load")
end

function M.onWorldReadyState(state)
  if state == 2 then
    setupSockets()
    rebuildNavigationModel("world ready")
    resetTracking("world ready")
  end
end

function M.onNavgraphReloaded()
  rebuildNavigationModel("navgraph reload")
  resetTracking("navgraph reload")
end

function M.onUpdate(dtReal)
  retryCmdBind(dtReal)
  if udpCmd then
    while true do
      local data = udpCmd:receive()
      if not data then break end
      local command = data:match("^%s*(.-)%s*$")
      local upper = command:upper()
      if upper == "ON" then
        if not isActive then resetTracking("toggle on") end
        isActive = true
      elseif upper == "OFF" then
        isActive = false
      else
        local privateValue = upper:match("^PRIVATE%s*,%s*([01])$")
        if privateValue then includePrivate = privateValue == "1" end
      end
    end
  end
  if not isActive then return end
  scanTimer = scanTimer + (dtReal or 0)
  if scanTimer >= SCAN_INTERVAL then
    scanTimer = scanTimer - SCAN_INTERVAL
    performScan()
  end
end

return M
