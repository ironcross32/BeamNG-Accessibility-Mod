-- Deterministic graph/state diagnostics for roadDetector.lua. Run with Lua 5.1+
-- (or through the BeamNG Lua console); no game globals are required.

local function assertEqual(actual, expected, label)
  if actual ~= expected then
    error(string.format("%s: expected %s, got %s", label, tostring(expected), tostring(actual)))
  end
end

local function clamp(v, lo, hi) return math.max(lo, math.min(hi, v)) end
local function point(x, y, z) return {x = x, y = y, z = z or 0} end
local function distance(a, b)
  local dx, dy = a.x - b.x, a.y - b.y
  return math.sqrt(dx * dx + dy * dy)
end
local function project(p, edge)
  local dx, dy = edge.outPos.x - edge.inPos.x, edge.outPos.y - edge.inPos.y
  local lenSq = dx * dx + dy * dy
  local raw = ((p.x - edge.inPos.x) * dx + (p.y - edge.inPos.y) * dy) / lenSq
  local t = clamp(raw, 0, 1)
  local cp = point(edge.inPos.x + dx * t, edge.inPos.y + dy * t,
    edge.inPos.z + (edge.outPos.z - edge.inPos.z) * t)
  return distance(p, cp), cp.z - p.z, t, edge.inRadius + (edge.outRadius - edge.inRadius) * t
end

local function build(singleSided)
  local edges, adjacency = {}, {}
  local function add(node, record)
    adjacency[node] = adjacency[node] or {}
    table.insert(adjacency[node], record)
  end
  for _, data in ipairs(singleSided) do
    table.insert(edges, data)
    add(data.inNode, {edge = data, to = data.outNode, forward = true})
    add(data.outNode, {edge = data, to = data.inNode, forward = false})
  end
  return edges, adjacency
end

local function edge(id, a, b, radius, options)
  options = options or {}
  return {id = id, inNode = a, outNode = b, inPos = options.inPos or point(options.ax or 0, options.ay or 0, options.az or 0),
    outPos = options.outPos or point(options.bx or 100, options.by or 0, options.bz or 0),
    inRadius = options.inRadius or radius, outRadius = options.outRadius or radius,
    oneWay = options.oneWay or false, private = options.private or false,
    drivability = options.drivability or 1}
end

local function legal(record, includePrivate)
  return record.edge.drivability >= 0.25
    and (includePrivate or not record.edge.private)
    and (not record.edge.oneWay or record.forward)
end

local function classify(exits, complex)
  if #exits == 0 then return "deadEnd" end
  if complex then return "complex" end
  local left, right, straight, forward = false, false, false, true
  for _, bearing in ipairs(exits) do
    if math.abs(bearing) <= 15 then straight = true
    elseif bearing > 0 then left = true else right = true end
    if math.abs(bearing) > 45 then forward = false end
  end
  if #exits == 2 and forward and left and right then return "fork" end
  if #exits == 2 and left and right and not straight then return "tJunction" end
  if #exits >= 3 and left and right and straight then return "crossroads" end
  return "intersection"
end

local function cluster(values)
  table.sort(values)
  local result = {}
  for _, value in ipairs(values) do
    local last = result[#result]
    if not last or math.abs(value - last) > 15 then table.insert(result, value)
    else result[#result] = (last + value) * 0.5 end
  end
  return result
end

local main = edge("main", "a", "b", 4, {inPos = point(0, 0), outPos = point(100, 0)})
local branch = edge("branch", "b", "c", 4, {inPos = point(100, 0), outPos = point(100, 80)})
local edges, adjacency = build({main, branch})
assertEqual(#edges, 2, "single-sided edge count")
assertEqual(#adjacency.b, 2, "bidirectional adjacency")
assertEqual(adjacency.a[1].forward, true, "forward record")
assertEqual(adjacency.c[1].forward, false, "reverse record")

local tapered = edge("taper", "a", "b", 2, {inPos = point(0, 0), outPos = point(100, 0), inRadius = 2, outRadius = 8})
local d, dz, t, radius = project(point(75, 6, 0), tapered)
assertEqual(math.floor(d + 0.5), 6, "edge projection")
assertEqual(t, 0.75, "edge t")
assertEqual(radius, 6.5, "edge-specific radius")
assertEqual(dz, 0, "edge z")

local low = edge("low", "l1", "l2", 4, {inPos = point(0, 0, 0), outPos = point(100, 0, 0)})
local high = edge("high", "h1", "h2", 4, {inPos = point(0, 0, 12), outPos = point(100, 0, 12)})
local lowDist, lowDz = project(point(30, 1, 0), low)
local highDist, highDz = project(point(30, 0, 0), high)
assert(lowDist > highDist, "parallel planar candidate setup")
assert(math.abs(lowDz) <= 6 and math.abs(highDz) > 6, "overpass rejection")

local oneWay = edge("oneway", "in", "out", 4, {oneWay = true})
local _, oneAdj = build({oneWay})
assert(legal(oneAdj["in"][1], false), "one-way forward is legal")
assert(not legal(oneAdj["out"][1], false), "one-way reverse is rejected")
local private = edge("private", "p1", "p2", 4, {private = true})
local _, privateAdj = build({private})
assert(not legal(privateAdj.p1[1], false), "private road filtered")
assert(legal(privateAdj.p1[1], true), "private road opt-in")

assertEqual(classify({}, false), "deadEnd", "dead end")
assertEqual(classify({30, -30}, false), "fork", "fork")
assertEqual(classify({90, -90}, false), "tJunction", "T junction")
assertEqual(classify({90, 0, -90}, false), "crossroads", "crossroads")
assertEqual(classify({70, 0}, false), "intersection", "ordinary intersection")
assertEqual(classify({45, -45}, true), "complex", "roundabout zone")
assertEqual(#cluster({-4, 5, 80, 94}), 2, "15-degree exit clustering")

local stable = {onRoad = false, enter = 0, exit = 0, orientation = true}
local function scan(insideEnter, insideExit)
  local oriented = false
  if stable.onRoad then
    stable.exit = insideExit and 0 or stable.exit + 1
    if stable.exit >= 3 then
      stable.onRoad, stable.enter, stable.exit, stable.orientation = false, 0, 0, true
    end
  else
    stable.enter = insideEnter and stable.enter + 1 or 0
    if stable.enter >= 2 then
      stable.onRoad, stable.enter, stable.exit = true, 0, 0
      if stable.orientation then oriented, stable.orientation = true, false end
    end
  end
  return oriented
end
assert(not scan(true, true), "first entry scan is quiet")
assert(scan(true, true), "second entry scan orients once")
assert(not scan(true, true), "orientation does not repeat")
assert(not scan(false, false) and stable.onRoad, "first exit scan held")
assert(not scan(false, false) and stable.onRoad, "second exit scan held")
scan(false, false)
assert(not stable.onRoad, "third exit scan leaves road")
assert(not scan(true, true) and scan(true, true), "off-road transition rearms orientation")

assertEqual(clamp(0 * 7, 30, 140), 30, "stopped early threshold")
assertEqual(clamp(20 * 7, 30, 140), 140, "fast early threshold")
assertEqual(clamp(0 * 2, 12, 35), 12, "stopped near threshold")
assertEqual(clamp(20 * 2, 12, 35), 35, "fast near threshold")
assertEqual(clamp(8 + 0 * 2, 12, 70), 12, "minimum lookahead")
assertEqual(clamp(8 + 40 * 2, 12, 70), 70, "maximum lookahead")

-- Off-road acquisition aims along the road, producing a shallow merge rather
-- than pointing at the perpendicular projection directly beside the vehicle.
local function interceptLead(lateralDistance, speed)
  return clamp(lateralDistance / math.tan(math.rad(30)) + speed * 1.5, 20, 180)
end
local lead = interceptLead(10, 0)
local mergeAngle = math.deg(math.atan(10 / lead))
assert(lead >= 20 and mergeAngle <= 30, "off-road intercept produces shallow merge")
assertEqual(interceptLead(200, 40), 180, "off-road intercept is bounded")

-- Correction is local-road error, not raw bearing to a point around a curve.
-- Ratio 0.5 is a normal lane position on a two-way road and must stay quiet.
local correctionState = {active = false, clear = 0}
local function correctionScan(headingError, lateralRatio)
  local enter = math.abs(headingError) > 6 or lateralRatio > 0.75
  local aligned = math.abs(headingError) < 3 and lateralRatio < 0.60
  if correctionState.active then
    correctionState.clear = aligned and correctionState.clear + 1 or 0
    if correctionState.clear >= 3 then
      correctionState.active, correctionState.clear = false, 0
    end
  elseif enter then
    correctionState.active, correctionState.clear = true, 0
  end
  return correctionState.active
end
assert(not correctionScan(0, 0.5), "centred-in-lane curved driving is silent")
assert(correctionScan(10, 0.5), "real heading error enters correction")
assert(correctionScan(0, 0.5), "first aligned release scan holds")
assert(correctionScan(0, 0.5), "second aligned release scan holds")
assert(not correctionScan(0, 0.5), "normal lane position releases correction")
assert(correctionScan(0, 0.8), "outer road quarter enters lateral correction")
local rightCorrection = math.deg(math.atan(-3, 12))
local leftCorrection = math.deg(math.atan(3, 12))
assert(rightCorrection < 0 and leftCorrection > 0, "lateral pan points toward road interior")

local driftLast, driftSpeed = nil, 0
local function driftStep(offset, radius)
  if driftLast == nil then driftLast = offset; return false end
  local rawSpeed = (offset - driftLast) / 0.2
  driftSpeed = driftSpeed + 0.35 * (rawSpeed - driftSpeed)
  driftLast = offset
  local outward = offset > 0 and driftSpeed or -driftSpeed
  local seconds = math.huge
  if outward > 0.20 then seconds = math.max(0, radius - math.abs(offset)) / outward end
  return outward > 0.20 and seconds <= 3
end
assert(not driftStep(2.0, 4), "steady lane offset initializes drift tracker")
assert(not driftStep(2.2, 4), "drift filter rejects first noisy sample")
assert(driftStep(2.4, 4), "outward motion predicts road departure")
assert(not driftStep(2.2, 4), "inward correction disarms drift prediction")
driftLast, driftSpeed = nil, 0
assert(not driftStep(-2.0, 4), "opposite lane initializes drift tracker")
assert(not driftStep(-2.2, 4), "opposite-side filter rejects first sample")
assert(driftStep(-2.4, 4), "opposite-side outward drift is detected")

local activeJunction = nil
local function junctionVisible(id, distanceAhead, earlyLimit)
  if not id then activeJunction = nil; return false end
  if activeJunction and id ~= activeJunction then activeJunction = nil end
  if distanceAhead <= earlyLimit or id == activeJunction then
    activeJunction = id
    return true
  end
  return false
end
assert(junctionVisible("j1", 60, 70), "junction enters early zone")
assert(junctionVisible("j1", 60, 30), "speed-threshold shrink keeps active junction")
assert(not junctionVisible(nil, 0, 30), "leaving forward walk clears junction latch")
assert(not junctionVisible("j1", 60, 30), "cleared encounter does not relatch outside threshold")

local function junctionMarkers(distanceAhead, nearLimit, speed)
  local entryDistance = math.max(0.1, speed * 0.1)
  return distanceAhead <= nearLimit and "near" or "approach", distanceAhead <= entryDistance
end
local phase, entered = junctionMarkers(12, 20, 10)
assertEqual(phase, "near", "near-junction phase")
assert(not entered, "near phase is not entry")
phase, entered = junctionMarkers(0.8, 20, 10)
assertEqual(phase, "near", "wire phase stays backward compatible at entry")
assert(entered, "physical junction boundary emits entry marker")

local zoneIds = {"roundaboutC", "roundaboutA", "roundaboutB"}
table.sort(zoneIds)
assertEqual(table.concat(zoneIds, "+"), "roundaboutA+roundaboutB+roundaboutC", "stable junction id")

print("road_detector_sim: all diagnostics passed")
