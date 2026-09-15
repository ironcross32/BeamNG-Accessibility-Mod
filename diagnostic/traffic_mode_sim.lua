-- GE Lua: verify the production scanner keeps its context across mode handoffs.
local source = assert(readFile('/lua/ge/extensions/roadDetector.lua'))
local assistant, command, latest, count = false, nil, nil, 0
local edge = {inNode='a',outNode='b',drivability=1,speedLimit=10}
local nodes = {a={pos=vec3(0,0,0),radius=4,links={b=edge}},
  b={pos=vec3(1000,0,0),radius=4,links={a=edge}}}
local player = {getID=function() return 42 end,
  getPosition=function() return vec3(100,0,0) end,
  getDirectionVector=function() return vec3(1,0,0) end,
  getVelocity=function() return vec3(0,0,0) end}
local env = setmetatable({
  map={getMap=function() return {nodes=nodes} end},
  be={getPlayerVehicle=function() return player end},
  extensions={beamtelAI={assistantActive=function() return assistant end}},
  core_trafficSignals={}, setExtensionUnloadMode=function() end, log=function() end,
  socket={udp=function() return {
    setpeername=function() return true end, setsockname=function() return true end,
    settimeout=function() end, close=function() end,
    receive=function() local c=command; command=nil; return c end,
    send=function(_,s) if s:sub(1,3)=='R2|' then latest=jsonDecode(s:sub(4)); count=count+1 end end,
  } end},
}, {__index=_G})
local chunk = assert(loadstring(source, 'traffic-mode-scanner'))
setfenv(chunk, env)
local scanner = chunk()
scanner.onExtensionLoaded()
local function tick() for _=1,12 do scanner.onUpdate(0.05) end end
tick(); assert(count==0, 'Neither mode: no scan')
assistant=true; tick(); assert(latest.state=='onRoad')
local context = latest.signalContext
command='ON'; tick(); assert(latest.signalContext==context, 'Enabling road mode preserves assist alerts')
command='OFF'; local before=count; tick()
assert(count>before and latest.signalContext==context, 'Road off: assistant keeps scanning')
command='ON'; tick()
assistant=false; before=count; tick()
assert(count>before and latest.signalContext==context, 'Assistant off: road mode keeps scanning')
command='OFF'; before=count; tick(); assert(count==before, 'Both off: stop scanning')
scanner.onExtensionUnloaded()
print('traffic_mode_sim.lua: scanner handoffs passed')
