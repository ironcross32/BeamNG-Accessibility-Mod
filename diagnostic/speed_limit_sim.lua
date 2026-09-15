-- Run in GE Lua. Reads production scanner source with isolated sockets and graph.
local source = assert(readFile('/lua/ge/extensions/roadDetector.lua'))
local edge = {inNode='a',outNode='b',drivability=1,speedLimit=10}
local nodes = {
  a={pos=vec3(0,0,0),radius=4,links={b=edge}},
  b={pos=vec3(1000,0,0),radius=4,links={a=edge}},
}
local position, facing = vec3(100,0,0), vec3(1,0,0)
local player = {getID=function() return 42 end,
  getPosition=function() return position end,
  getDirectionVector=function() return facing end,
  getVelocity=function() return vec3(0,0,0) end}
local latest
local mockSocket = {udp=function() return {
  setpeername=function() return true end, setsockname=function() return true end,
  settimeout=function() end, close=function() end, receive=function() return nil end,
  send=function(_, message) if message:sub(1,3)=='R2|' then latest=jsonDecode(message:sub(4)) end end,
} end}
local env = setmetatable({
  socket=mockSocket,
  map={getMap=function() return {nodes=nodes} end},
  be={getPlayerVehicle=function() return player end},
  extensions={beamtelAI={assistantActive=function() return true end}},
  core_trafficSignals={}, setExtensionUnloadMode=function() end, log=function() end,
  require=function(name) if name=='socket' then return mockSocket else return require(name) end end,
}, {__index=_G})
local chunk = assert(loadstring(source, 'speed-limit-scanner'))
setfenv(chunk,env)
local scanner = chunk()
scanner.onExtensionLoaded()
local function scan()
  for _=1,12 do scanner.onUpdate(0.05) end
  return assert(latest)
end
assert(scan().speedLimit==10, 'Valid navgraph speed reaches the packet')
edge.speedLimit=20
assert(scan().speedLimit==20, 'Changed navgraph data is read without a rebuild')
facing=vec3(-1,0,0)
assert(scan().speedLimit==20, 'Both directions resolve the same two-way road limit')
for _, invalid in ipairs({0,-1,math.huge,0/0}) do
  edge.speedLimit=invalid
  assert(scan().speedLimit==nil, 'Invalid values must not reach the wire')
end
edge.speedLimit=nil
assert(scan().speedLimit==nil, 'Missing limit remains unknown')
edge.speedLimit=10
position=vec3(100,100,0)
assert(scan().state=='offRoad' and latest.speedLimit==nil, 'Do not report a nearby road limit while off road')
scanner.onExtensionUnloaded()
print('speed_limit_sim.lua: all diagnostics passed')
