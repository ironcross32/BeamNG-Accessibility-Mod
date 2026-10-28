-- Run in BeamNG's Lua runtime (uses vec3). Supply the source map
-- BEAMTEL_ASSISTANT_SOURCES and run in an isolated environment (no live control).
local sources = BEAMTEL_ASSISTANT_SOURCES or {}
local function module(path, env)
  local source = sources[path]
  local chunk = source and assert(loadstring(source, path)) or assert(loadfile(path))
  setfenv(chunk, setmetatable(env or {}, {__index = _G}))
  return chunk()
end
local routePath = 'bng_mod/lua/ge/extensions/assistantRoutes.lua'
local routes = module(routePath)
assert(routes.direction(-0.251) == 'left')
assert(routes.direction(-0.25) == 'straight')
assert(routes.direction(0.25) == 'straight')
assert(routes.direction(0.251) == 'right')
assert(routes.classify(30) == 'straight' and routes.classify(-30) == 'straight')
assert(routes.classify(150) == 'left' and routes.classify(-150) == 'right')
assert(routes.classify(151) == nil)
local choices = routes.choices({{bearing=80,id='b'}, {bearing=100,id='a'},
  {bearing=-90,id='r'}, {bearing=0,id='s'}})
assert(choices.left.id == 'a' and choices.right.id == 'r' and choices.straight.id == 's')
assert(routes.nodes({{from='a',to='b'},{from='x',to='c'}}) == nil)
assert(table.concat(routes.nodes({{from='a',to='b'},{from='b',to='c'}}), ',') == 'a,b,c')

local callbacks, inputs, values, main = {}, {}, {gearboxMode='realistic',gearIndex=1}, {}
local original = {steering={['local']=true, remote=false}, throttle={}, clutch={remote=true}}
local filters = {}
for _, name in ipairs({'steering','throttle','brake','parkingbrake','clutch'}) do
  filters[name] = original[name]
  inputs[name] = {val=0, filter=7, angle=900, lockType=2,
    smootherKBD={set=function() end}, smootherPAD={set=function() end}}
end
local speed = 10
local motionPosition, motionFacing = vec3(0,0,0), vec3(1,0,0)
local input = {state=inputs, allowedInputSources=filters,
  lastInputs={['local']={steering=0,throttle=0.4,brake=0.3,clutch=0.6},ai={brake=1}}}
function input.event(name, value, filter, angle, lock, time, source)
  input.lastInputs[source] = input.lastInputs[source] or {}
  input.lastInputs[source][name] = value
  if input.allowedInputSources[name] and input.allowedInputSources[name][source] then
    inputs[name].val, inputs[name].filter = value, filter
  end
end
local ai = {mode='disabled', setRecoverOnCrash=function(v) assert(v == false) end}
local spoken = {}
local hooks = {message=function(payload) spoken[#spoken+1]=payload end}
local originalMessage = hooks.message
function main.setGearboxMode(mode)
  hooks.message({txt='vehicle.vehicleController.shifterModeChanged'})
  values.gearboxMode=mode
  -- Stock manual gearbox, with auto clutch off: re-entering realistic mode
  -- drops first/reverse to neutral even when the requested mode is unchanged.
  if mode=='realistic' and math.abs(values.gearIndex)==1 then values.gearIndex=0 end
end
local originalSetter = main.setGearboxMode
local paths = {}
function ai.driveUsingPath(p)
  assert(p.driveInLane=='on' and p.avoidCars=='on')
  paths[#paths+1]=p.path
  ai.mode='manual'; main.setGearboxMode('arcade')
  hooks.message('unrelated warning')
end
function ai.setPath(tail)
  local last = paths[#paths]
  assert(last[#last] == tail[1], 'Stock AI requires an endpoint-connected extension')
  local combined = {}
  for _, node in ipairs(last) do combined[#combined+1] = node end
  for i=2,#tail do combined[#combined+1] = tail[i] end
  paths[#paths+1] = combined
end
function ai.setMode(mode)
  main.setGearboxMode('realistic')
  ai.mode=mode; inputs.throttle.val=1; inputs.steering.val=1
end
local vm = module('bng_mod/lua/vehicle/extensions/beamtelAssistant.lua', {
  input=input, ai=ai, electrics={values=values}, controller={mainController=main}, guihooks=hooks,
  obj={getID=function() return 42 end, getVelocity=function() return motionFacing*speed end,
    getPosition=function() return motionPosition end, getDirectionVector=function() return motionFacing end,
    queueGameEngineLua=function(_, command) callbacks[#callbacks+1]=command end}})
local plan = {path={'a','b'}, distance=50, junction={id='j', choices={left={'a','b','l'},straight={'a','b','s'}}}}
vm.start('one', plan)
assert(values.gearIndex==1 and main.setGearboxMode==originalSetter, 'Activation must preserve first gear')
assert(paths[1][1]=='b' and #paths[1]==1, 'Do not visit the occupied segment entry node behind the car')
assert(#spoken==1 and spoken[1]=='unrelated warning' and hooks.message==originalMessage)
main.setGearboxMode('realistic')
assert(#spoken==2, 'Player gearbox notifications must remain available')
assert(values.gearIndex==0, 'Stock setter behavior is exercised by this regression')
values.gearIndex=1
assert(values.gearboxMode=='realistic')
assert(filters.steering.ai and not filters.steering['local'])
assert(filters.throttle['local'] and not filters.throttle.ai)
assert(inputs.throttle.val==0.4)
vm.updateGFX(0.01) -- early announcement, not commitment
assert(#paths==1)
input.lastInputs['local'].steering=-1
plan.distance=15
vm.setPlan('one',plan); vm.updateGFX(0.01)
assert(paths[#paths][#paths[#paths]]=='l')
input.lastInputs['local'].steering=1
vm.setPlan('one',plan); vm.updateGFX(0.01)
assert(paths[#paths][#paths[#paths]]=='l') -- held input cannot rewrite committed exit
assert(vm.diagnosticState().plannerStarts==1, 'A junction choice must not restart AI')
vm.setPlan('one',{path={'b','l'},distance=80,clear=false})
assert(vm.diagnosticState().plannerStarts==1, 'Passing a node must not restart AI')
vm.setPlan('one',{path={'b','l','more'},distance=180,clear=false})
assert(vm.diagnosticState().plannerStarts==1 and vm.diagnosticState().pathExtensions==2)
local spokenBeforeStop = #spoken
speed, input.lastInputs['local'].throttle = 0, 0
vm.updateGFX(0.3)
assert(vm.diagnosticState().suspended and #spoken==spokenBeforeStop)
assert(values.gearIndex==1, 'Suspension must preserve first gear')
speed, input.lastInputs['local'].throttle = 0.15, 0.07
vm.updateGFX(0.01)
assert(vm.diagnosticState().suspended and vm.diagnosticState().plannerStarts==1,
  'Tiny stationary speed and pedal fluctuations must not restart the planner')
speed, input.lastInputs['local'].throttle = 10, 0.4
vm.updateGFX(0.01)
assert(not vm.diagnosticState().suspended and vm.diagnosticState().plannerStarts==2)
assert(values.gearIndex==1, 'Resuming must preserve first gear')
assert(#spoken==spokenBeforeStop+1 and spoken[#spoken]=='unrelated warning',
  'Resuming the planner must suppress only its shifter message')
vm.stop('obsolete')
assert(filters.steering.ai)
inputs.parkingbrake.val=1 -- stock toggleEvent bypasses lastInputs
vm.stop('one')
for name, value in pairs(original) do assert(filters[name]==value, name..' exact restoration') end
assert(filters.brake==nil and filters.parkingbrake==nil)
assert(inputs.steering.val==1 and inputs.brake.val==0.3 and inputs.clutch.val==0.6)
assert(inputs.parkingbrake.val==1)
assert(inputs.steering.angle==900 and inputs.steering.lockType==2)
assert(values.gearboxMode=='realistic' and ai.mode=='disabled')
assert(values.gearIndex==1, 'Disengagement must preserve first gear')
vm.start('one',plan)
assert(ai.mode=='disabled') -- a retired activation cannot start again
plan = {path={'a','b'},distance=10,junction={id='invalid',choices={left={'a','b','l'}}}}
input.lastInputs['local'].steering=0
vm.start('two',plan); vm.updateGFX(0.01)
assert(ai.mode=='manual' and paths[#paths][#paths[#paths]]=='l')
assert(vm.diagnosticState().automatic and vm.diagnosticState().requested=='straight')
vm.stop('two')
plan = {path={'a','b'},distance=10,junction={id='fallback-straight',
  choices={straight={'a','b','s'},right={'a','b','r'}}}}
input.lastInputs['local'].steering=-1
vm.start('fallback-straight',plan); vm.updateGFX(0.01)
assert(ai.mode=='manual' and paths[#paths][#paths[#paths]]=='s', 'Invalid left prefers a legal straight exit')
vm.stop('fallback-straight')
input.lastInputs['local'].steering=0
vm.start('near-auto',{path={'a','b'},distance=3,chooseNow=true,
  junction={id='close-auto',choices={right={'a','b','r'}}}})
assert(ai.mode=='manual' and paths[#paths][#paths[#paths]]=='r' and vm.diagnosticState().automatic)
vm.stop('near-auto')
-- A dead end keeps the assist on: the driver stops at the bar and can reverse out.
local deadPlan = {path={'a','b'},distance=10,chooseNow=true,junction={id='dead',choices={}}}
local callbacksBefore = #callbacks
vm.start('dead-end',deadPlan)
assert(ai.mode=='manual', 'Switching on beside a dead end follows the road to its bar')
for _ = 1, 4 do vm.heartbeat('dead-end'); vm.setPlan('dead-end',deadPlan); vm.updateGFX(0.4) end
assert(ai.mode=='manual' and vm.diagnosticState().active, 'A dead end does not starve or end the session')
local deadSpeech = 0
for i = callbacksBefore + 1, #callbacks do
  if callbacks[i]:find('Dead end ahead. Stop at the bar',1,true) then deadSpeech = deadSpeech + 1 end
end
assert(deadSpeech==1, 'The dead end is announced once')
vm.stop('dead-end')
-- Raw Lua errors never reach speech: a refusal speaks its reason, a fault a
-- generic one, and the raw text goes on as an unspoken error event for the log.
local function events(from)
  local out = {}
  for i = from + 1, #callbacks do
    local kind, message = callbacks[i]:match('onAssistantEvent%(%d+,"[^"]*","([^"]*)","(.-)"%) end$')
    out[#out+1] = {kind = kind, message = message or ''}
  end
  return out
end
local before = #callbacks
vm.start('roundabout',{path={'a','b'},distance=3,chooseNow=true,
  junction={id='ring',unsupported=true,choices={straight={'a','b','s'}}}})
local said = events(before)
assert(said[#said].kind=='off' and said[#said].message:find('Roundabout or unsupported junction just ahead',1,true))
for _, e in ipairs(said) do assert(e.kind~='error' and not e.message:find('.lua',1,true)) end
-- Stock AI that gives up holds exactly zero steering: release control at once.
before = #callbacks
vm.start('silent-ai',{path={'a','b'},distance=100})
input.lastInputs.ai.steering = 0.004
vm.heartbeat('silent-ai'); vm.setPlan('silent-ai',{path={'a','b'},distance=100}); vm.updateGFX(0.5)
assert(vm.diagnosticState().active, 'A small real steering output is not lost steering')
input.lastInputs.ai.steering = 0
vm.heartbeat('silent-ai'); vm.setPlan('silent-ai',{path={'a','b'},distance=100}); vm.updateGFX(0.2)
assert(vm.diagnosticState().active)
vm.heartbeat('silent-ai'); vm.setPlan('silent-ai',{path={'a','b'},distance=100}); vm.updateGFX(0.25)
said = events(before)
assert(not vm.diagnosticState().active and said[#said].message:find('Steering lost. Brake now',1,true),
  'Zero AI steering at speed hands back control with a brake warning')
input.lastInputs.ai.steering = nil
-- A gated exit is named in the prompt, before the choice is made.
before = #callbacks
local gatedPlan = {path={'a','b'},distance=60,junction={id='gated-prompt',
  choices={straight={'a','b','s'},right={'a','b','r'}},gated={right=true}}}
vm.start('gated-prompt',gatedPlan)
vm.heartbeat('gated-prompt'); vm.setPlan('gated-prompt',gatedPlan); vm.updateGFX(0.05)
local prompt
for _, e in ipairs(events(before)) do if e.message:find('Junction ahead',1,true) then prompt = e.message end end
assert(prompt and prompt:find('straight or right through a gate. Hold a turn to select.',1,true), tostring(prompt))
-- "Slow down" steps aside from junction speech for a second, not eight: the
-- tight turn straight after "Turning left" is when it matters.
input.lastInputs.ai.brake = 1
local open = {path={'a','b','r'},distance=100,clear=false}
vm.heartbeat('gated-prompt'); vm.setPlan('gated-prompt',open); vm.updateGFX(0.5)
local slow = false
for _, e in ipairs(events(before)) do slow = slow or e.kind=='slow' end
assert(not slow, 'No slow-down over the junction prompt')
vm.heartbeat('gated-prompt'); vm.setPlan('gated-prompt',open); vm.updateGFX(0.6)
for _, e in ipairs(events(before)) do slow = slow or e.kind=='slow' end
assert(slow, 'Slow down one second after junction speech')
vm.stop('gated-prompt')
local savedDrive = ai.driveUsingPath
ai.driveUsingPath = function() error('planner failure') end
before = #callbacks
vm.start('failure',{path={'a','b'},distance=100})
assert(ai.mode=='disabled' and main.setGearboxMode==originalSetter, 'Restore the gearbox setter on failure')
local sawFault = false
for _, e in ipairs(events(before)) do
  if e.kind=='error' then sawFault = e.message:find('planner failure',1,true) ~= nil end
  if e.kind=='off' then
    assert(not e.message:find('.lua',1,true) and not e.message:find('planner failure',1,true),
      'The spoken off reason carries no raw error')
  end
end
assert(sawFault, 'The raw fault is passed on for the log')
ai.driveUsingPath = savedDrive
vm.start('three',{path={'a','b'},distance=100})
vm.setPlan('obsolete',{path={'x','y'},distance=0})
vm.updateGFX(1.1)
assert(ai.mode=='disabled') -- lost planner connection releases filters
input.lastInputs['local'].steering=-1
vm.start('near-junction',{path={'a','b'},distance=3,chooseNow=true,
  junction={id='close',choices={left={'a','b','l'}}}})
assert(ai.mode=='manual' and paths[#paths][#paths[#paths]]=='l')
vm.stop('near-junction')

-- A half-second turn hold survives release, without extending the route early.
local function selectionPlan(id)
  return {path={'a','b'}, distance=60, junction={id=id,
    choices={left={'a','b','l'}, straight={'a','b','s'}, right={'a','b','r'}}}}
end
local latchPlan = selectionPlan('latch')
input.lastInputs['local'].steering=0
vm.start('latch', latchPlan)
local latchPaths = #paths
local function hold(value, dt)
  input.lastInputs['local'].steering=value
  vm.heartbeat('latch'); vm.setPlan('latch',latchPlan); vm.updateGFX(dt)
end
hold(-1,0.3); hold(0,0.1); hold(-1,0.3)
assert(not vm.diagnosticState().selection, 'Separate short holds must not add up')
hold(-1,0.2)
assert(vm.diagnosticState().selection=='left' and not vm.diagnosticState().locked)
assert(#paths==latchPaths, 'Selecting early does not commit or replan the route')
hold(0,0.5); hold(0,0.5)
assert(vm.diagnosticState().selection=='left', 'Release must preserve the selected turn')
hold(1,0.3)
assert(vm.diagnosticState().selection=='left', 'A short correction cannot replace the selection')
hold(1,0.2)
assert(vm.diagnosticState().selection=='right', 'Another full hold can change an uncommitted turn')
latchPlan.junction.choices.left=nil
hold(-1,0.5); hold(-1,0.5)
assert(vm.diagnosticState().selection=='right', 'Unavailable input must not erase a valid selection')
latchPlan.distance=10
hold(0,0.01)
assert(vm.diagnosticState().chosen=='right' and not vm.diagnosticState().automatic)
assert(paths[#paths][#paths[#paths]]=='r', 'Commit uses the latch after steering is released')
latchPlan={path={'a','b','r'},distance=100,clear=false} -- GE acknowledges the committed route.
hold(-1,0.5); hold(-1,0.5)
assert(vm.diagnosticState().chosen=='right', 'A committed turn cannot be replaced')
latchPlan={path={'b','r'},distance=100,clear=true,junction={id='next',choices={straight={'b','r','n'}}}}
hold(0,0.01)
assert(not vm.diagnosticState().selection and vm.diagnosticState().selectionJunction=='next',
  'Selection belongs to only one junction')
vm.stop('latch')
assert(not vm.diagnosticState().selection, 'Disengagement clears selection')

-- Reverse gear suspends stock forward AI immediately, before any backward motion.
values.gearIndex, speed = 1, 0
vm.start('reverse',{path={'a','b'},distance=100,motion='forward',motionRevision=0})
local starts = vm.diagnosticState().plannerStarts
values.gearIndex = -1
vm.updateGFX(0.01)
assert(vm.diagnosticState().reversing and ai.mode=='disabled' and vm.diagnosticState().motionWaiting)
assert(input.lastInputs.ai.steering==0 and filters.steering.ai)
vm.setPlan('reverse',{path={'a','b','s'},distance=10,motion='forward',motionRevision=0})
assert(vm.diagnosticState().plannerStarts==starts, 'In-flight forward plans cannot restart AI in reverse')
local reversePlan={path={'a','b'},distance=100,motion='reverse',motionRevision=1,
  reverseGuidance={target={-7,-1,0},blocked=false}}
vm.setPlan('reverse',reversePlan)
speed=-2
vm.updateGFX(0.1)
assert(input.lastInputs.ai.steering>0 and input.lastInputs.ai.steering<0.8,
  'Rear target to body-right needs proportional right steering, not a full-lock U-turn')
reversePlan.reverseGuidance.target={-7,0,0}
vm.setPlan('reverse',reversePlan); vm.updateGFX(0.1)
assert(input.lastInputs.ai.steering==0, 'Straight rear target releases steering lock')
reversePlan.reverseGuidance.target={7,-1,0}
vm.setPlan('reverse',reversePlan); vm.updateGFX(0.1)
assert(input.lastInputs.ai.steering==0, 'Never chase a forward target while backing')
values.gearIndex=1 -- still rolling backwards: do not change controller yet.
vm.updateGFX(0.1)
assert(vm.diagnosticState().reversing)
speed=0
vm.updateGFX(0.1)
assert(not vm.diagnosticState().reversing and vm.diagnosticState().motionWaiting)
vm.setPlan('reverse',reversePlan)
assert(ai.mode=='disabled', 'A stale reverse plan cannot acknowledge forward resumption')
vm.setPlan('reverse',{path={'c','d'},distance=100,motion='forward',motionRevision=2})
assert(ai.mode=='manual' and paths[#paths][1]=='d' and vm.diagnosticState().plannerStarts==starts+1,
  'Forward resumes once with a newly positioned route, not an append to old targets')
vm.stop('reverse')
-- A brake or impact bounce rocks the car back briefly; that is not reversing.
values.gearIndex, speed = 1, -0.9
vm.start('bounce',{path={'a','b'},distance=100})
inputs.brake.val = 1
vm.heartbeat('bounce'); vm.setPlan('bounce',{path={'a','b'},distance=100}); vm.updateGFX(0.4)
assert(not vm.diagnosticState().reversing, 'A rock back under the brake is not reverse')
inputs.brake.val = 0
vm.heartbeat('bounce'); vm.setPlan('bounce',{path={'a','b'},distance=100}); vm.updateGFX(0.4)
assert(not vm.diagnosticState().reversing, 'An impact bounce shorter than the hold is not reverse')
vm.stop('bounce')
inputs.brake.val = 0.3
values.gearIndex, speed = 0, -0.5
vm.start('rollback',{path={'a','b'},distance=100})
assert(not vm.diagnosticState().reversing, 'Rolling back must last before it counts')
vm.heartbeat('rollback'); vm.setPlan('rollback',{path={'a','b'},distance=100}); vm.updateGFX(0.35)
vm.heartbeat('rollback'); vm.setPlan('rollback',{path={'a','b'},distance=100}); vm.updateGFX(0.35)
assert(vm.diagnosticState().reversing and ai.mode=='disabled', 'Backward rolling is detected without reverse gear')
vm.setPlan('rollback',{path={'a','b'},motion='reverse',motionRevision=1,
  reverseGuidance={blocked=true,target={0,0,0},reason='Reverse route ends. Stop'}})
vm.updateGFX(0.1)
assert(not vm.diagnosticState().active and vm.diagnosticState().lastOffReason:find('Reverse route ends',1,true),
  'Exhausted reverse guidance releases control with the normal off event')
vm.stop('rollback')

-- Ideal low-speed bicycle model: production rear-target selection and steering
-- should converge from the shoulder without circling or retaining full lock.
local reverseRoad={records={{forward=true,edge={inPos=vec3(0,0,0),outPos=vec3(100,0,0),length=100}}}}
local function flatRoadProjection(point)
  local t=math.max(0,math.min(1,point.x/100))
  local nearest=vec3(t*100,0,0)
  return {point=nearest,t=t,radius=4,distance=point:distance(nearest),dz=-point.z}
end
motionPosition, motionFacing, speed, values.gearIndex=vec3(60,7,0),vec3(1,0,0),-2,-1
vm.start('reverse-model',{path={'a','b'},distance=100})
local yaw, peakSteering=0,0
for i=1,300 do
  local guide=assert(routes.reverseTarget(reverseRoad,motionPosition,motionFacing,2,flatRoadProjection))
  assert(not guide.blocked)
  vm.heartbeat('reverse-model')
  vm.setPlan('reverse-model',{path={'a','b'},distance=100,motion='reverse',motionRevision=1,reverseGuidance=guide})
  vm.updateGFX(0.05)
  local steering=input.lastInputs.ai.steering
  peakSteering=math.max(peakSteering,math.abs(steering))
  yaw=yaw-speed/3*math.tan(steering*0.6)*0.05
  motionFacing=vec3(math.cos(yaw),math.sin(yaw),0)
  motionPosition=motionPosition+motionFacing*speed*0.05
end
assert(math.abs(motionPosition.y-2)<0.5 and math.abs(yaw)<0.15 and peakSteering<0.95,
  'Reverse recovery converges to the nearby lane and straightens the wheels')
print(string.format('Reverse model: lane error %.3f m, heading %.3f rad, peak steering %.3f',
  motionPosition.y-2,yaw,peakSteering))
vm.stop('reverse-model')
motionPosition,motionFacing=vec3(0,0,0),vec3(1,0,0)
values.gearIndex, speed = 1, 10

-- Use the real detector adapter against a small directed graph.
local function node(x,y) return {pos=vec3(x,y,0),radius=4,links={}} end
local nodes = {a=node(0,0), b=node(100,0), c=node(110,0),
  d=node(250,0), l=node(100,100), r=node(110,-100), endL=node(100,200), endR=node(110,-200)}
local function link(a,b,oneWay,extra)
  local edge={inNode=a,outNode=b,oneWay=oneWay,drivability=1}
  for key, value in pairs(extra or {}) do edge[key]=value end
  nodes[a].links[b]=edge; nodes[b].links[a]=edge
end
link('a','b'); link('b','c',true); link('b','l'); link('c','d'); link('c','r');
link('l','endL'); link('r','endR')
nodes.u, nodes.v = node(1000,0), node(1100,0)
link('u','v',true)
-- A public road meeting a gated road straight on and a rough track to the left;
-- past the gate the road runs to a true dead end.
for name, x in pairs({g1=2000, g2=2100, g3=2120, g4=2300}) do nodes[name]=node(x,0); nodes[name].radius=1 end
nodes.track=node(2100,100)
link('g1','g2'); link('g2','g3',false,{gatedRoad=true}); link('g3','g4')
link('g2','track',false,{drivability=0.2})
-- A driveway: private road, a gate, more private road (Johnson Valley's DR155).
for name, x in pairs({q1=8000, q2=8100, q3=8110, q4=8300}) do nodes[name]=node(x,0); nodes[name].radius=1 end
link('q1','q2',false,{private=true}); link('q2','q3',false,{private=true,gatedRoad=true})
link('q3','q4',false,{private=true})
-- A grouped junction whose two-way crossovers form a triangle, as on Johnson
-- Valley's divided roads, and a one-way ring, which really is a roundabout.
nodes.h1, nodes.h2, nodes.h3, nodes.h4 = node(4000,0), node(4050,0), node(4058,6), node(4058,-6)
nodes.h5, nodes.h6 = node(4058,60), node(4058,-60)
link('h1','h2'); link('h2','h3'); link('h2','h4'); link('h3','h4'); link('h3','h5'); link('h4','h6')
nodes.r1, nodes.r2, nodes.r3, nodes.r4 = node(4950,0), node(5000,0), node(5008,8), node(5000,16)
nodes.r5, nodes.r6 = node(5040,8), node(4980,36)
link('r1','r2'); link('r2','r3',true); link('r3','r4',true); link('r4','r2',true)
link('r3','r5'); link('r4','r6')
-- A gap through a central reservation: its 6 m first link leaves at 144 degrees
-- left, then it runs back the way the car came. That is a U-turn, not a left.
nodes.m1, nodes.m2, nodes.m3 = node(6000,0), node(6000,100), node(6000,300)
nodes.m4, nodes.m5 = node(6000-6*math.sin(math.rad(144)), 100+6*math.cos(math.rad(144))), node(5996.5,-100)
link('m1','m2'); link('m2','m3'); link('m2','m4'); link('m4','m5')
local pos, facing=vec3(10,0,0),vec3(1,0,0)
local player={getPosition=function() return pos end, getDirectionVector=function() return facing end,
  getVelocity=function() return vec3(10,0,0) end}
local rayHit
local rd=module('bng_mod/lua/ge/extensions/roadDetector.lua', {
  map={getMap=function() return {nodes=nodes} end}, log=function() end,
  castRayStatic=function(_, _, range) return rayHit or range end,
  require=function(name)
    if name=='ge/extensions/assistantRoutes' then return routes end
    if name=='ge/extensions/roadSignals' then return module('bng_mod/lua/ge/extensions/roadSignals.lua') end
    return {}
  end})
local state=assert(rd.assistantStart(player))
assert(rd.assistantUpdate(state,player))
assert(state.junction and state.junction.choices.left and state.junction.choices.right)
rayHit=5
assert(rd.assistantUpdate(state,player,0.1) and not state.hazard, 'Do not announce one transient hit')
rayHit=nil
rd.assistantUpdate(state,player,0.1)
assert(state.hazardTime==0, 'A clear frame resets obstacle persistence')
rayHit=5
rd.assistantUpdate(state,player,0.1); rd.assistantUpdate(state,player,0.11)
assert(state.hazard and state.hazardDetail.reason=='obstruction', 'Persistent barrier on route')
facing=vec3(1,0,-0.2)
rd.assistantUpdate(state,player,0.3)
assert(not state.hazard and state.hazardDetail.reason=='roadSurface', 'Pitching nose ray hits road, not obstacle')
facing=vec3(0,1,0)
rd.assistantUpdate(state,player,0.3)
assert(not state.hazard and state.hazardDetail.reason=='outsideRoute', 'Do not announce a roadside hit outside route')
facing,rayHit=vec3(1,0,0),nil
local right=state.junction.choices.right.records
assert(#right==2 and right[1].from=='b' and right[1].to=='c' and right[2].to=='r')
assert(rd.assistantChoose(state,'right'))
assert(rd.assistantUpdate(state,player))
assert(state.committed)
pos=vec3(110,-15,0)
assert(rd.assistantUpdate(state,player))
assert(not state.committed)
assert(state.path[1]=='c' and state.path[2]=='r')
assert(#state.history==2, 'Forward travel retains passed road segments for backing')
local reverseState={records=state.records,history=state.history,generation=state.generation,path=state.path}
facing=vec3(0,-1,0)
assert(rd.assistantReverse(reverseState,player))
assert(reverseState.reverseGuidance.target[2]>pos.y and not reverseState.reverseGuidance.blocked,
  'Backing along the selected outgoing road targets behind the car')
pos=vec3(110,-1,0)
assert(rd.assistantReverse(reverseState,player))
assert(reverseState.reverseGuidance.target[1]<110, 'Reverse target walks into retained incoming segments')
pos,facing=vec3(110,-15,0),vec3(1,0,0)
pos=vec3(122,-15,0)
assert(rd.assistantUpdate(state,player,0.1))
assert(state.recovering and state.path[1]=='c', 'Recover on the selected outgoing road')
local result, err=rd.assistantUpdate(state,player,13)
assert(not result and err:find('Unable to rejoin',1,true))
pos=vec3(110,-20,0)
assert(rd.assistantUpdate(state,player,0.1) and not state.recovering)
pos,facing=vec3(1050,0,0),vec3(-1,0,0)
local wrong, reason=rd.assistantStart(player)
assert(not wrong and reason:find('One-way road. Legal travel is 180 degrees',1,true))
facing=vec3(1,0,0)
local legal=assert(rd.assistantStart(player))
assert(legal.roadMessage==nil, 'Starting with legal travel ahead says only that the assist is on')

-- Gated roads and rough tracks are routes for the assistant, not dead ends.
pos,facing=vec3(2010,0,0),vec3(1,0,0)
local gated=assert(rd.assistantStart(player))
assert(rd.assistantUpdate(gated,player))
assert(gated.junction and gated.junction.choices.straight and gated.junction.choices.left,
  'Gated road and rough track are both exits')
local manual=rd.proximityAt(vec3(2050,0,0),facing,10)
assert(manual and manual.kind=='gate', 'Manual guidance calls a road that only goes on gated a gate')
local stop=rd.proximityAt(vec3(2050,0,0),facing,10,nil,gated)
assert(stop.kind=='junction' and math.abs(stop.distance-49)<0.01, 'Route stop bar sits at the junction boundary')
assert(rd.assistantChoose(gated,'straight') and rd.assistantUpdate(gated,player))
stop=rd.proximityAt(vec3(2050,0,0),facing,10,nil,gated)
assert(stop.kind=='gate' and stop.id=='junction:g2', 'Committed into a gated road: the bar stays, as a gate')
stop=rd.proximityAt(vec3(2150,0,0),facing,30,nil,gated)
assert(stop.kind=='deadEnd' and math.abs(stop.distance-149)<0.01, 'Route ends at a dead end bar')
local gatedExits=assert(rd.assistantStart(player))
pos=vec3(2010,0,0); assert(rd.assistantUpdate(gatedExits,player))
assert(gatedExits.junction.choices.straight.gated and not gatedExits.junction.choices.left.gated,
  'The exit through the gate is flagged for the prompt, the rough track is not')
pos=vec3(8010,0,0)
local driveway=assert(rd.assistantStart(player))
assert(rd.assistantUpdate(driveway,player))
stop=rd.proximityAt(vec3(8050,0,0),facing,10,nil,driveway)
assert(stop and stop.kind=='gate' and stop.id=='junction:q2',
  'A gate between two private roads gets its own bar')
-- Ground models are named by family; unknown ones and the tire's own are silent.
assert(rd.surfaceWord('DIRT_DUSTY')=='Dirt' and rd.surfaceWord('ASPHALT_WET')=='Asphalt')
assert(rd.surfaceWord('GRASS2')=='Grass' and rd.surfaceWord('ROCKYDIRT')=='Dirt')
assert(rd.surfaceWord('SANDY_ROAD')=='Sand')
assert(rd.surfaceWord('RUBBER')==nil and rd.surfaceWord('')==nil and rd.surfaceWord(nil)==nil)
-- b and c are one grouped junction 10 m apart: once past b, no second bar at c.
pos=vec3(10,0,0)
local grouped=assert(rd.assistantStart(player))
assert(rd.assistantUpdate(grouped,player))
stop=rd.proximityAt(vec3(50,0,0),facing,30,nil,grouped)
assert(stop.id=='junction:b' and stop.kind=='junction', 'The bar sits at the group entry')
assert(rd.assistantChoose(grouped,'right'))
pos=vec3(105,0,0)
assert(rd.assistantUpdate(grouped,player))
stop=rd.proximityAt(vec3(105,0,0),facing,30,nil,grouped)
assert(stop and stop.id=='junction:endR' and stop.kind=='deadEnd',
  'Inside the group the next bar is beyond it, not mid-junction')
pos=vec3(4010,0,0)
local crossover=assert(rd.assistantStart(player))
assert(rd.assistantUpdate(crossover,player))
assert(crossover.junction and not crossover.junction.unsupported
  and crossover.junction.choices.left and crossover.junction.choices.right,
  'Two-way crossovers inside a junction are not a roundabout')
pos=vec3(4960,0,0)
local ring=assert(rd.assistantStart(player))
assert(rd.assistantUpdate(ring,player))
assert(ring.junction and ring.junction.unsupported, 'A one-way ring is still a roundabout')
pos=vec3(6000,10,0); facing=vec3(0,1,0)
local median=assert(rd.assistantStart(player))
assert(rd.assistantUpdate(median,player))
assert(median.junction and median.junction.choices.straight and not median.junction.choices.left,
  'A median gap that turns the car round is not offered as left')
-- Off the route, whatever the ray hits ahead is an obstacle.
pos,facing=vec3(10,0,0),vec3(1,0,0)
local offRoute=assert(rd.assistantStart(player))
assert(rd.assistantUpdate(offRoute,player))
pos,facing,rayHit=vec3(50,7,0),vec3(0,1,0),5
rd.assistantUpdate(offRoute,player,0.1); rd.assistantUpdate(offRoute,player,0.11)
assert(offRoute.recovering and offRoute.hazard and offRoute.hazardDetail.reason=='offRouteObstruction',
  'An obstacle ahead of a car off the route is warned about')
pos,facing,rayHit=vec3(10,0,0),vec3(1,0,0),nil

-- The bar is held as a line until the nose clears it, even as the target moves on.
local function bar(x)
  return rd.stopBarAt(vec3(x,0,0),facing,10,nil)
end
assert(math.abs(bar(2050).signedDistance-49)<0.01)
assert(math.abs(bar(2098.6).signedDistance-0.4)<0.01)
local held=bar(2097.5)
assert(held.id=='junction:g2' and held.kind=='gate' and math.abs(held.signedDistance-1.5)<0.01)
held=bar(2100.5)
assert(held and held.id=='junction:g2' and math.abs(held.signedDistance+1.5)<0.01 and held.distance==0,
  'Past the line but still on the bar')
assert(bar(2102) == nil, 'Cleared the bar')

local events, commands = {}, {}
local vehicle={getID=function() return 42 end,getVelocity=player.getVelocity,
  queueLuaCommand=function(_,c) commands[#commands+1]=c end}
local fakeState={path={'a','b'},distance=100,records={{from='a',to='b'}}}
local routeStarts, reverseUpdates=0,0
local coordinator=module('bng_mod/lua/ge/extensions/drivingAssistant.lua', {
  be={getPlayerVehicle=function() return vehicle end,getPlayerVehicleID=function() return 42 end},
  scenetree={findObjectById=function() return vehicle end},
  extensions={roadDetector={assistantStart=function() routeStarts=routeStarts+1; return fakeState end,
    assistantUpdate=function(s) return s end,
    assistantReverse=function(s) reverseUpdates=reverseUpdates+1; return s end}},
  serialize=function() return '{}' end,jsonEncode=function(v) events[#events+1]=v; return '{}' end})
coordinator.command('ASSISTANT_TOGGLE','fresh',function() end)
coordinator.event(42,'stale','ready','obsolete')
assert(coordinator.status()=='disabled')
coordinator.event(99,'fresh','ready','wrong vehicle')
assert(coordinator.status()=='disabled')
coordinator.event(42,'fresh','ready','on')
assert(coordinator.status()=='driving assistant')
coordinator.off()
local count=#events
coordinator.event(42,'fresh','ready','late')
assert(#events==count and not coordinator.owns())
coordinator.command('ASSISTANT_TOGGLE','fresh',function() end)
assert(not coordinator.owns())
coordinator.command('ASSISTANT_TOGGLE','new',function() end)
coordinator.command('ASSISTANT_OFF','fresh',function() end)
assert(coordinator.owns())
coordinator.event(42,'new','ready','on')
local beforeReverse=routeStarts
coordinator.event(42,'new','motion','reverse')
coordinator.update(0.1)
assert(reverseUpdates==1 and coordinator.diagnosticState().motion=='reverse')
coordinator.event(42,'new','motion','forward')
coordinator.update(0.1)
assert(routeStarts==beforeReverse+1 and coordinator.diagnosticState().motionRevision==2,
  'Forward resumption replaces the route once at the current vehicle position')
coordinator.off()
print('Driving assistant diagnostics passed')
