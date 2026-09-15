-- Run in GE Lua. Exercises the production resolver with small synthetic graphs.
local resolver = require('ge/extensions/roadSignals')
local function edge(a, b, x1, y1, x2, y2)
  return {inNode = a, outNode = b, inPos = vec3(x1, y1, 0), outPos = vec3(x2, y2, 0),
    length = math.sqrt((x2-x1)^2 + (y2-y1)^2), inRadius = 4, outRadius = 4}
end
local function project(pos, e)
  local d = e.outPos - e.inPos
  local t = (pos - e.inPos):dot(d) / d:squaredLength()
  local p = e.inPos + d * math.max(0, math.min(1, t))
  return {rawT = t, distance = pos:distance(p), radius = 4, dz = p.z-pos.z}
end
local a = edge('a', 'b', 0, 0, 100, 0)
local b = edge('b', 'c', 100, 0, 100, 100)
local function signal(id, e, pos, kind, state, dir)
  return {id = id, name = tostring(id), road = {n1 = e.inNode, n2 = e.outNode},
    pos = pos, dir = dir or (e.outPos-e.inPos):normalized(), active = true,
    getController = function() return {type = kind or 'signStop'} end,
    getState = function() return state or 'basicStop' end}
end
local signs = {signal(1, a, vec3(70, 2, 0))}
local api = {getData = function() return {loaded = true, active = true} end,
  getSignals = function() return signs end}
local choices = {}
local function outgoing(to) return choices[to] or {} end
local function find(pos, dir, limit)
  return resolver.find(api, a, project(pos, a), 1, dir or vec3(1, 0, 0),
    outgoing, project, limit or 180)
end
local result = find(vec3(20, 0, 0))
assert(result.id == '1' and result.distance == 50 and result.state == 'stop')
assert(find(vec3(20, 0, 0), vec3(-1, 0, 0)) == nil, 'opposing heading')
assert(find(vec3(75, 0, 0)) == nil, 'passed sign')
assert(find(vec3(20, 0, 0), nil, 40) == nil, 'range')
signs[1].dir = vec3(0, 1, 0)
assert(find(vec3(20, 0, 0)) == nil, 'cross traffic')
signs = {signal(2, b, vec3(102, 30, 0), 'lightsBasic', 'redTrafficLight')}
choices.b = {{edge = b, forward = true}}
result = find(vec3(80, 0, 0))
assert(result.id == '2' and result.distance == 50 and result.state == 'red', 'curved path')
choices.b[2] = {edge = a, forward = false}
assert(find(vec3(80, 0, 0)) == nil, 'ambiguous branch')
choices.b[2] = nil
signs[1].pos.z = 20
assert(find(vec3(80, 0, 0)) == nil, 'overpass')
signs[1].pos = vec3(120, 30, 0)
assert(find(vec3(80, 0, 0)) == nil, 'lateral mismatch')
signs = {signal(3, a, vec3(70, 0, 0), 'lightsCustom', 'customState')}
assert(find(vec3(60, 0, 0)).state == 'unknown', 'custom state')
signs[1].getState = function() return 'yellowFlashingTrafficLight' end
assert(find(vec3(60, 0, 0)).state == 'flashingYellow', 'flashing state')
signs[1].getState = function() return 'none' end
signs[1].active = false
assert(find(vec3(60, 0, 0)).state == 'off', 'disabled light')
-- A side road must not hide the stop farther along the current road.
local straight = edge('b', 'd', 100, 0, 200, 0)
choices.b = {{edge = straight, forward = true}, {edge = b, forward = true}}
signs = {signal(4, straight, vec3(170, 0, 0))}
assert(find(vec3(20, 0, 0)).distance == 150, 'look through side street')
local slipTurn = edge('b', 'slip', 100, 0, 150, 38)
choices.b = {{edge = straight, forward = true}, {edge = slipTurn, forward = true}}
signs = {signal(8, straight, vec3(170, 0, 0), 'lightsBasic', 'redTrafficLight')}
local slipPreview = find(vec3(20, 0, 0))
assert(slipPreview and slipPreview.preview and slipPreview.state == 'unknown', 'shallow slip road preview')
-- Close forks leading to the same controlled junction get an advance warning,
-- with no guessed light colour before the driver commits to an arm.
local fork = edge('b', 'e', 100, 0, 200, 20)
choices.b = {{edge = straight, forward = true}, {edge = fork, forward = true}}
signs = {signal(5, straight, vec3(170, 0, 0), 'lightsBasic', 'redTrafficLight'),
  signal(6, fork, vec3(170, 14, 0), 'lightsBasic', 'greenTrafficLight')}
signs[1].intersectionId, signs[2].intersectionId = 100, 100
local preview = find(vec3(20, 0, 0))
assert(preview and preview.preview and preview.state == 'unknown', 'common fork warning')
signs[2].intersectionId = 101
assert(find(vec3(20, 0, 0)) == nil, 'different junctions remain ambiguous')
local slip = edge('p', 'q', 0, 0, 100, 20)
local matched = resolver.matchRoad(vec3(25, 3, 0), vec3(1, 0, 0), {slip, a}, project)
assert(matched == a, 'alignment beats overlapping closer slip lane')
local player = {getPosition = function() return vec3(20, 0, 0) end,
  getDirectionVector = function() return vec3(1, 0, 0) end,
  getNodeCount = function() return 3 end,
  getNodePosition = function(_, cid) return ({vec3(-2, 0, 0), vec3(3, 1, 0), vec3(2, -1, 0)})[cid+1] end}
local front, cid, extent = resolver.frontPosition(player)
assert(front.x == 23 and cid == 1 and extent == 3, 'front-most live node')
signs = {signal(7, a, vec3(70, 0, 0))}
assert(find(front).distance == 47, 'distance starts at front node')
assert(resolver.warningDistance(0) == 60, 'minimum advance distance')
assert(resolver.warningDistance(30) >= 240, 'eight seconds at road speed')
assert(resolver.warningDistance(40) >= 40*3 + 40*40/6, 'reaction plus braking')
api.getData = function() return {loaded = true, active = false} end
local _, status = find(vec3(60, 0, 0))
assert(status == 'inactive')
local junction = {id = 'T', distance = 90, centreDistance = 96}
local light = {id = 'light', state = 'red', distance = 85}
local cue = resolver.proximity(light, junction, 20)
assert(cue.source == 'stopPoint' and cue.action == 'stop' and cue.distance == 85)
light.state = 'green'
cue = resolver.proximity(light, junction, 20)
assert(cue.distance == 85 and cue.action == 'approach', 'green retains stopping-point distance')
for _, state in ipairs({'unknown', 'yellow', 'off', 'flashingYellow', 'flashingGreen'}) do
  light.state = state
  assert(resolver.proximity(light, junction, 20).action == 'approach')
end
for _, state in ipairs({'red', 'redYellow', 'flashingRed', 'stop'}) do
  light.state = state
  assert(resolver.proximity(light, junction, 20).action == 'stop')
  light.preview = true
  assert(resolver.proximity(light, junction, 20).action == 'approach', 'preview is not a stop instruction')
  light.preview = nil
end
light.distance = 110
cue = resolver.proximity(light, junction, 20)
assert(cue.source == 'junctionBoundary' and cue.distance == 90, 'do not skip nearer junction')
assert(resolver.proximity(nil, junction, 0) == nil, 'outside warning range')
assert(resolver.proximity(nil, junction, 0, 'junction:T').distance == 90, 'retain cue while slowing')
assert(resolver.proximity(nil, junction, 20).distance == 90)
assert(resolver.proximity(nil, nil, 20) == nil)
assert(resolver.proximity(light, nil, 20).distance == 110, 'signals without graph junctions')
return 'road_signals_sim: all diagnostics passed'
