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
-- An exit through a gate, or onto a private road, is named in the prompt,
-- before the choice is made.
before = #callbacks
local gatedPlan = {path={'a','b'},distance=60,junction={id='gated-prompt',
  choices={left={'a','b','l'},straight={'a','b','s'},right={'a','b','r'}},
  access={left='private',right='gate'}}}
vm.start('gated-prompt',gatedPlan)
vm.heartbeat('gated-prompt'); vm.setPlan('gated-prompt',gatedPlan); vm.updateGFX(0.05)
local prompt
for _, e in ipairs(events(before)) do if e.message:find('Junction ahead',1,true) then prompt = e.message end end
assert(prompt and prompt:find('left onto a private road or straight or right through a gate. Hold a turn to select.',1,true),
  tostring(prompt))
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

-- Join stage: stock AI gets the whole car, then lets go of it for the assisted start.
-- Like stock: entering through driveUsingPath sets arcade without saving the
-- player's mode, so disabling restores nothing.
local drive, setMode, joinArgs = ai.driveUsingPath, ai.setMode, nil
local function stockJoin(p) joinArgs=p; ai.mode='manual'; values.gearboxMode='arcade' end
ai.setMode=function(mode) ai.mode=mode end
ai.driveUsingPath=stockJoin
vm.join('joining','b',10)
ai.driveUsingPath=drive
assert(values.gearboxMode=='arcade')
assert(joinArgs.wpTargetList[1]=='b' and joinArgs.routeSpeed==10 and joinArgs.routeSpeedMode=='limit')
assert(vm.diagnosticState().joining and filters.throttle==original.throttle, 'The join stage filters nothing')
vm.updateGFX(0.25)
assert(callbacks[#callbacks]:find('"alive"',1,true))
vm.start('joining',{path={'a','b'},distance=100})
assert(not vm.diagnosticState().joining and ai.mode=='manual' and filters.steering.ai
  and filters.throttle['local'] and not filters.throttle.ai, 'Assisted start takes over from the join')
assert(callbacks[#callbacks]:find('"ready"',1,true))
assert(values.gearboxMode=='realistic', 'Hand-over restores the player gearbox mode')
vm.stop('joining')
assert(ai.mode=='disabled' and filters.throttle==original.throttle)
ai.driveUsingPath=stockJoin
vm.join('quit','b',10)
ai.driveUsingPath=drive
ai.mode='disabled'
vm.updateGFX(0.1)
assert(not vm.diagnosticState().active and callbacks[#callbacks]:find('AI stopped driving',1,true),
  'AI that stops driving on its own ends the join')
assert(values.gearboxMode=='realistic', 'AI quitting restores the player gearbox mode')
ai.driveUsingPath=stockJoin
vm.join('lost','b',10)
ai.driveUsingPath=drive
vm.updateGFX(3.1)
assert(not vm.diagnosticState().active and ai.mode=='disabled', 'A silent receiver ends the join')
assert(values.gearboxMode=='realistic', 'Giving up restores the player gearbox mode')
ai.driveUsingPath=stockJoin
vm.join('reset','b',10)
ai.driveUsingPath=drive
vm.onReset()
assert(values.gearboxMode=='realistic', 'A reset during the join restores the player gearbox mode')
ai.setMode=setMode
values.gearIndex, speed = 1, 10

-- A route round a block repeats a node: extending must use the visit it reached.
vm.start('looped',{path={'s','a','b','c','a'},distance=100,clear=true,replanRevision=0})
vm.setPlan('looped',{path={'s','a','b','c','a','d'},distance=100,clear=true,replanRevision=0})
local loopTail=paths[#paths]
assert(loopTail[#loopTail]=='d' and loopTail[#loopTail-1]=='a' and loopTail[#loopTail-2]=='c' and #loopTail==5,
  'The second visit to a is the one extended from')
vm.stop('looped')
-- A followed route: no junction reaches the vehicle, so held steering cannot
-- select anything, and only a new route revision restarts the planner.
before = #callbacks
input.lastInputs['local'].steering = -1
vm.start('routed',{path={'a','b','l'},distance=100,clear=true,replanRevision=0})
for _ = 1, 3 do
  vm.heartbeat('routed')
  vm.setPlan('routed',{path={'a','b','l'},distance=100,clear=true,replanRevision=0})
  vm.updateGFX(0.5)
end
for _, e in ipairs(events(before)) do
  assert(e.kind~='selection' and e.kind~='junction' and e.kind~='choice', 'Steering selects nothing on a route')
end
assert(vm.diagnosticState().plannerStarts==1)
vm.setPlan('routed',{path={'a','b','l','m'},distance=100,clear=true,replanRevision=0})
assert(vm.diagnosticState().plannerStarts==1 and vm.diagnosticState().pathExtensions==1,
  'Further along the same route appends')
vm.setPlan('routed',{path={'a','b','x','y'},distance=100,clear=true,replanRevision=1})
assert(vm.diagnosticState().plannerStarts==2 and vm.diagnosticState().active,
  'A different road ahead restarts the planner rather than failing to append')
assert(paths[#paths][#paths[#paths]]=='y')
vm.setPlan('routed',{path={'a','b','x','y'},distance=100,clear=true,replanRevision=1})
assert(vm.diagnosticState().plannerStarts==2, 'Each revision restarts it once')
vm.stop('routed')
input.lastInputs['local'].steering = 0

-- The three point turn in the vehicle: steering held, nothing else touched, then
-- handed to an ordinary start with the steering taken afresh.
local turnFrom = #callbacks
values.gearIndex = 0
vm.turn('turning')
assert(vm.diagnosticState().turning and ai.mode=='disabled', 'Stock AI has no part in it')
assert(filters.steering.ai and filters.throttle['local'] and not filters.throttle.ai, 'The pedals stay with the player')
vm.setTurn('turning',-1)
vm.heartbeat('turning'); vm.updateGFX(0.05)
assert(input.lastInputs.ai.steering==-1, 'Full lock as told')
local gearReports = 0
for i = turnFrom + 1, #callbacks do if callbacks[i]:find('"gear","0"',1,true) then gearReports = gearReports + 1 end end
assert(gearReports==1, 'The gear is reported')
values.gearIndex = -1
vm.updateGFX(0.05)
assert(callbacks[#callbacks]:find('"gear","-1"',1,true), 'And each change')
local gearFrom = #callbacks
vm.updateGFX(0.05)
for i = gearFrom + 1, #callbacks do assert(not callbacks[i]:find('"gear"',1,true), 'But not when unchanged') end
values.gearIndex = 1
vm.start('turning',{path={'a','b'},distance=100,clear=true,replanRevision=0})
assert(not vm.diagnosticState().turning and ai.mode=='manual' and filters.steering.ai, 'Handed to the route')
vm.stop('turning')
for name, value in pairs(original) do assert(filters[name]==value, name..' restored after a turn') end
vm.turn('turnstop')
vm.setTurn('turnstop',1)
vm.heartbeat('turnstop'); vm.updateGFX(0.05)
vm.stop('turnstop')
assert(not vm.diagnosticState().turning and filters.steering==original.steering, 'Stopping mid-turn gives the steering back')
vm.turn('turnlost')
vm.updateGFX(1.2)
assert(not vm.diagnosticState().active and filters.steering==original.steering, 'A silent GE ends the turn')
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
-- A public road meeting a private road straight on and a rough track to the
-- left, as map.lua leaves them: the private road's links are type = 'private',
-- and the link where it meets public is flagged gatedRoad. Past it the road
-- runs to a true dead end.
for name, x in pairs({g1=2000, g2=2100, g3=2120, g4=2300}) do nodes[name]=node(x,0); nodes[name].radius=1 end
nodes.track=node(2100,100)
link('g1','g2'); link('g2','g3',false,{type='private',gatedRoad=true}); link('g3','g4',false,{type='private'})
link('g2','track',false,{drivability=0.2})
-- A driveway inside private ground with a boundary flag between two private
-- links (Johnson Valley's DR155): no gate stands there.
for name, x in pairs({q1=8000, q2=8100, q3=8110, q4=8300}) do nodes[name]=node(x,0); nodes[name].radius=1 end
link('q1','q2',false,{type='private'}); link('q2','q3',false,{type='private',gatedRoad=true})
link('q3','q4',false,{type='private'})
-- A public road that simply becomes private at a two-link node, far from any
-- junction: the map.lua case that flags the PUBLIC link.
for name, x in pairs({e0=9000, e1=9100, e2=9400, e3=9700}) do nodes[name]=node(x,0); nodes[name].radius=1 end
link('e0','e1',false,{gatedRoad=true}); link('e1','e2',false,{type='private'}); link('e2','e3',false,{type='private'})
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
-- A road leading to a block: a destination behind the car is reached by going round it.
for name, xy in pairs({b0={19900,0}, b1={20000,0}, b2={20100,0}, b3={20200,0}, b4={20200,100}, b5={20100,100}}) do
  nodes[name]=node(xy[1],xy[2])
end
link('b0','b1'); link('b1','b2'); link('b2','b3'); link('b3','b4'); link('b4','b5'); link('b5','b2')
-- Two roads leaving almost straight on: the route has to say which.
nodes.f1, nodes.f2, nodes.f3, nodes.f4 = node(30000,0), node(30100,0), node(30200,20), node(30200,-20)
link('f1','f2'); link('f2','f3'); link('f2','f4')
-- A main road with a private slip link every so often, as on Johnson Valley's
-- divided roads: 24 degrees off, onto a parallel road.
nodes.s0, nodes.s1, nodes.s2, nodes.s3 = node(31000,0), node(31100,0), node(31200,0), node(31300,0)
nodes.t1, nodes.t2 = node(31120,9), node(31300,9)
link('s0','s1'); link('s1','s2'); link('s2','s3')
link('s1','t1',false,{type='private',drivability=0.5}); link('t1','t2',false,{type='private',drivability=0.5})
-- A road bending 40 degrees with a side link off the other way: only a bend.
nodes.k0, nodes.k1, nodes.k2, nodes.k3 = node(32000,0), node(32100,0), node(32177,64), node(32254,128)
nodes.k4 = node(32117,-98)
link('k0','k1'); link('k1','k2'); link('k2','k3'); link('k1','k4')
-- A real crossroads of equal roads, driven straight across.
nodes.x0, nodes.x1, nodes.x2, nodes.x3, nodes.x4 = node(33000,0), node(33100,0), node(33300,0), node(33100,100), node(33100,-100)
link('x0','x1'); link('x1','x2'); link('x1','x3'); link('x1','x4')
-- A road ending in a tiny two-way loop, as at Italy's port.
nodes.p0, nodes.p1, nodes.p2, nodes.p3, nodes.p4 = node(34900,0), node(35100,0), node(35116,-4), node(35108,8), node(35106,4)
link('p0','p1'); link('p1','p2'); link('p2','p3'); link('p3','p4'); link('p4','p1')
-- The main road bends away and the route carries straight on to a private road.
nodes.y0, nodes.y1, nodes.y2, nodes.y3 = node(34000,0), node(34100,0), node(34180,-60), node(34300,0)
link('y0','y1'); link('y1','y2'); link('y1','y3',false,{type='private'})
local pos, facing=vec3(10,0,0),vec3(1,0,0)
local player={getPosition=function() return pos end, getDirectionVector=function() return facing end,
  getVelocity=function() return vec3(10,0,0) end}
local rayHit, dropAt
local gatesModule=module('bng_mod/lua/ge/extensions/roadGates.lua')
local rd=module('bng_mod/lua/ge/extensions/roadDetector.lua', {
  map={getMap=function() return {nodes=nodes} end}, log=function() end,
  -- Not the game's: a real session in progress would lend stopBarAt its live route.
  extensions={},
  castRayStatic=function(origin, direction, range)
    -- Downward rays see the ground 2 m below their start, unless over a drop.
    if direction.z < -0.5 then
      if dropAt and origin.y > pos.y + 2.25 + dropAt then return range end
      return 2
    end
    return rayHit or range
  end,
  require=function(name)
    if name=='ge/extensions/assistantRoutes' then return routes end
    if name=='ge/extensions/roadSignals' then return module('bng_mod/lua/ge/extensions/roadSignals.lua') end
    if name=='ge/extensions/roadGates' then return gatesModule end
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

-- Off the road: join at the nearest edge's far end, in its legal direction.
pos,facing=vec3(50,30,0),vec3(1,0,0)
assert(not rd.assistantStart(player), 'Thirty metres off is not on the road')
local join=assert(rd.assistantJoinTarget(player))
assert(join.target=='b' and math.abs(join.distance-26)<0.01 and join.bearing<-80, 'Road to the right, toward b')
facing=vec3(-1,0,0)
assert(rd.assistantJoinTarget(player).target=='a', 'A two-way road is joined the way the car faces')
pos=vec3(1050,12,0)
assert(rd.assistantJoinTarget(player).target=='v', 'A one-way road is joined only in its legal direction')
-- The join hands over only once properly on the road.
pos,facing=vec3(50,7,0),vec3(1,0,0)
assert(rd.assistantStart(player) and not rd.assistantStart(player,1,30), 'A verge is not yet the road')
pos,facing=vec3(50,2,0),vec3(1,1,0):normalized()
assert(rd.assistantStart(player) and not rd.assistantStart(player,1,30), 'Forty-five degrees is not yet aligned')
facing=vec3(1,0.2,0):normalized()
assert(rd.assistantStart(player,1,30), 'On the road and aligned')

-- Private roads and rough tracks are routes for the assistant, not dead ends.
-- The map's gated flag is no gate: only a gate mesh on the road is called one.
pos,facing=vec3(2010,0,0),vec3(1,0,0)
local gated=assert(rd.assistantStart(player))
assert(rd.assistantUpdate(gated,player))
assert(gated.junction and gated.junction.choices.straight and gated.junction.choices.left,
  'Private road and rough track are both exits')
local manual=rd.proximityAt(vec3(2050,0,0),facing,10)
assert(manual and manual.kind=='private', 'Manual guidance: a public road that only goes on private says private')
local stop=rd.proximityAt(vec3(2050,0,0),facing,10,nil,gated)
assert(stop.kind=='junction' and math.abs(stop.distance-49)<0.01, 'Route stop bar sits at the junction boundary')
assert(rd.assistantChoose(gated,'straight') and rd.assistantUpdate(gated,player))
stop=rd.proximityAt(vec3(2050,0,0),facing,10,nil,gated)
assert(stop.kind=='junction' and stop.id=='junction:g2' and stop.privateEntry=='private:g2',
  'Committed onto a private road: the junction keeps its bar and carries the private entry')
stop=rd.proximityAt(vec3(2150,0,0),facing,30,nil,gated)
assert(stop.kind=='deadEnd' and math.abs(stop.distance-149)<0.01 and not stop.privateEntry,
  'Route ends at a dead end bar')
local gatedExits=assert(rd.assistantStart(player))
pos=vec3(2010,0,0); assert(rd.assistantUpdate(gatedExits,player))
assert(gatedExits.junction.choices.straight.access=='private' and not gatedExits.junction.choices.left.access,
  'The exit onto the private road is named for the prompt, the rough track is not')
-- A gate mesh on the private road, 10 m in from the junction.
local function project(p, edge)
  local a, b = edge.inPos, edge.outPos
  local abx, aby = b.x - a.x, b.y - a.y
  local t = math.max(0, math.min(1, ((p.x - a.x) * abx + (p.y - a.y) * aby) / (abx * abx + aby * aby)))
  local qx, qy, qz = a.x + abx * t, a.y + aby * t, a.z + (b.z - a.z) * t
  return {t = t, distance = math.sqrt((p.x - qx) ^ 2 + (p.y - qy) ^ 2), dz = qz - p.z}
end
assert(gatesModule.classify('/levels/hirochi_raceway/art/shapes/hr_boomgate_r.dae')=='hr_boomgate_r')
assert(gatesModule.classify('s_metal_gate_left.DAE') and not gatesModule.classify('s_sign_gate_pillar.dae')
  and not gatesModule.classify('guardhouse.dae') and not gatesModule.classify('italy_barrier_polizia.dae'),
  'Only meshes that close a road are gates')
local fence={}
for k=0,5 do fence[#fence+1]={pos=vec3(k*3,500,0),base='ind_mesh_fence_gate_3m'} end
fence[#fence+1]={pos=vec3(2110,2,0),base='hr_boomgate'}
fence[#fence+1]={pos=vec3(2113,-2,0),base='hr_boomgate_r'}
fence[#fence+1]={pos=vec3(2200,40,0),base='hr_boomgate'}
local kept=gatesModule.dropFenceRuns(fence)
assert(#kept==3, 'A run of gate panels is a fence; a boom pair and a lone boom are kept')
assert(gatesModule.attach(kept, rd.edgesNear, project)==1,
  'The boom pair is one gate on the road; the boom 40 m off it is on none')
manual=rd.proximityAt(vec3(2050,0,0),facing,10)
assert(manual and manual.kind=='gate', 'Manual guidance: private road with a gate at its start says gate')
stop=rd.proximityAt(vec3(2101,0,0),facing,10,nil,gated)
assert(stop.kind=='gate' and math.abs(stop.distance-9)<0.01 and stop.id:find('gate:',1,true),
  'The route bar sits at the gate itself')
local throughGate=assert(rd.assistantStart(player))
pos=vec3(2010,0,0); assert(rd.assistantUpdate(throughGate,player))
assert(throughGate.junction.choices.straight.access=='gate', 'The exit through a gate mesh is named a gate')
for _, edge in ipairs(rd.edgesNear(vec3(2110,0,0))) do edge.gates=nil end
-- A boundary flag between two private links is not a gate and has no bar.
pos=vec3(8010,0,0)
local driveway=assert(rd.assistantStart(player))
assert(rd.assistantUpdate(driveway,player))
stop=rd.proximityAt(vec3(8050,0,0),facing,10,nil,driveway)
assert(stop==nil, 'No bar where the map flags a link between two private roads')
stop=rd.proximityAt(vec3(8250,0,0),facing,10,nil,driveway)
assert(stop and stop.kind=='deadEnd' and stop.id=='junction:q4' and not stop.privateEntry,
  'The next bar is the real dead end')
-- Manual guidance hides private roads only from public ones: on private ground
-- the private way on is listed, so this is the dead end at q4, not one at q3.
manual=rd.proximityAt(vec3(8105,0,0),facing,30)
assert(manual and manual.kind=='deadEnd' and manual.id=='junction:q4', tostring(manual and manual.id))
-- A public road becoming private far from any junction: a spoken entry with no bar.
pos=vec3(9010,0,0)
local entering=assert(rd.assistantStart(player))
assert(rd.assistantUpdate(entering,player))
stop=rd.proximityAt(vec3(9050,0,0),facing,10,nil,entering)
assert(stop and stop.kind=='privateEntry' and stop.privateEntry=='private:e1' and math.abs(stop.distance-50)<0.01,
  'The private entry is its own silent target when no stop follows closely')
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
assert(held.id=='junction:g2' and held.kind=='private' and math.abs(held.signedDistance-1.5)<0.01)
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

-- Join stage: off the road, stock AI drives the whole car until the strict test passes.
local joinCommands, joinEvents, onRoad, joinSpeed = {}, {}, false, 5
local joinVehicle={getID=function() return 42 end,getVelocity=function() return vec3(joinSpeed,0,0) end,
  queueLuaCommand=function(_,c) joinCommands[#joinCommands+1]=c end}
local joiner=module('bng_mod/lua/ge/extensions/drivingAssistant.lua', {
  be={getPlayerVehicle=function() return joinVehicle end,getPlayerVehicleID=function() return 42 end},
  scenetree={findObjectById=function() return joinVehicle end},
  extensions={roadDetector={
    assistantStart=function(_, margin, angle)
      assert(margin==1, 'The verge is not the road, at switch-on or hand-over')
      if angle then assert(angle==30, 'Hand over only once aligned with the road') end
      if onRoad then return {path={'a','b'},distance=100,records={}} end
      return nil, 'No reachable navigable road nearby'
    end,
    assistantUpdate=function(s) return s end,
    assistantJoinTarget=function() return {target='b',distance=26,bearing=-90} end}},
  serialize=function() return '{}' end,jsonEncode=function(v) joinEvents[#joinEvents+1]=v; return '{}' end})
joiner.command('ASSISTANT_TOGGLE','join',function() end)
assert(joinCommands[1]:find('beamtelAssistant.join("join","b",10)',1,true))
assert(joinEvents[1].kind=='say' and joinEvents[1].message:find('26 metres to your right',1,true))
assert(joiner.owns() and joiner.diagnosticState().joining and joiner.route()==nil)
joiner.update(0.1); joiner.event(42,'join','alive','')
assert(#joinCommands==1, 'Still off the road: AI keeps driving')
onRoad=true
joiner.update(0.1)
assert(joinCommands[2]:find('beamtelAssistant.start("join"',1,true))
joiner.event(42,'join','ready','on')
assert(joiner.status()=='driving assistant')
joiner.update(0.1)
assert(joinCommands[3]:find('setPlan("join"',1,true), 'Handed over: ordinary plans follow')
joiner.off()
onRoad, joinSpeed = false, 0
joiner.command('ASSISTANT_TOGGLE','stuck',function() end)
for _ = 1, 16 do
  joiner.update(1); joiner.event(42,'stuck','alive','')
  joiner.command('ASSISTANT_HEARTBEAT','stuck',function() end)
end
assert(not joiner.owns() and joinEvents[#joinEvents].message:find('Could not reach a road',1,true),
  'A car that never reaches a road is let go')

-- ================================================================================================
--  Following a destination
-- ================================================================================================
local function near(a, b, tolerance) return math.abs(a-b) <= (tolerance or 0.01) end
local function pathHas(names, name)
  for _, value in ipairs(names) do if value==name then return true end end
  return false
end

pos,facing=vec3(10,0,0),vec3(1,0,0)
local routed=assert(rd.assistantStart(player))
assert(rd.assistantUpdate(routed,player))
assert(routed.junction, 'Without a destination the junction is the player\'s')
assert(rd.assistantSetDestination(routed,vec3(100,150,0),10,'l'))
assert(routed.junction==nil and pathHas(routed.path,'l') and pathHas(routed.path,'endL'),
  'A destination drives through the junction with no choice left open')
assert(#routed.nav.turns==1 and routed.nav.turns[1].name=='left')
local guide=rd.assistantGuidance(routed)
assert(near(guide.destDistance,240) and guide.turn=='left' and guide.turnId==1, 'Distances along the route')
assert(near(guide.turnDistance,86), 'A turn is counted to where its junction starts')
assert(guide.offRoad==nil)
pos,facing=vec3(100,50,0),vec3(0,1,0)
assert(rd.assistantUpdate(routed,player))
guide=rd.assistantGuidance(routed)
assert(near(guide.destDistance,100) and guide.turn==nil and guide.turnDistance==nil,
  'Past the last turn: no turn field at all, never a zero')
pos=vec3(100,120,0)
assert(rd.assistantUpdate(routed,player))
local stop=rd.proximityAt(pos,facing,10,nil,routed)
assert(stop and stop.kind=='destination' and near(stop.distance,30), 'The destination is a stop on the route')
pos,facing=vec3(10,0,0),vec3(1,0,0)

routed=assert(rd.assistantStart(player))
assert(rd.assistantUpdate(routed,player))
assert(rd.assistantSetDestination(routed,vec3(100,150,0),10,'l'))
assert(rd.assistantClearDestination(routed,10))
assert(routed.nav==nil and routed.junction and routed.junction.choices.left and not pathHas(routed.path,'l'),
  'Clearing the route gives the next junction back')

-- A destination behind the car: forward only, the long way round.
pos,facing=vec3(20050,0,0),vec3(1,0,0)
local behind=assert(rd.assistantStart(player))
assert(rd.assistantUpdate(behind,player))
assert(rd.assistantSetDestination(behind,vec3(19950,0,0),10,'b1'))
local records=behind.nav.records
assert(records[1].to=='b2' and records[#records].from=='b1' and records[#records].to=='b0',
  'It leaves forwards and comes back along the road')
assert(near(rd.assistantGuidance(behind).destDistance,50+100+100+100+100+100+50), 'Once round the block')
-- The same, with the destination 40 m off the road.
behind=assert(rd.assistantStart(player))
assert(rd.assistantUpdate(behind,player))
assert(rd.assistantSetDestination(behind,vec3(19950,40,0),10,'b1'))
assert(behind.nav.offRoad==true and rd.assistantGuidance(behind).offRoad==true)

-- One-way: behind on a one-way road is not reachable, and the route is left alone.
pos,facing=vec3(1050,0,0),vec3(1,0,0)
local oneWay=assert(rd.assistantStart(player))
assert(rd.assistantUpdate(oneWay,player))
local unchanged=oneWay.records[1]
local noRoute,noReason=rd.assistantSetDestination(oneWay,vec3(1010,0,0),10,'u')
assert(not noRoute and noReason=='No forward route to the destination')
assert(oneWay.nav==nil and oneWay.records[1]==unchanged)
-- Ahead on the same road is fine.
assert(rd.assistantSetDestination(oneWay,vec3(1090,0,0),10,'u'))
assert(near(rd.assistantGuidance(oneWay).destDistance,40))

-- A median gap is not a road to the destination beyond it.
pos,facing=vec3(6000,10,0),vec3(0,1,0)
local median=assert(rd.assistantStart(player))
assert(rd.assistantUpdate(median,player))
assert(not rd.assistantSetDestination(median,vec3(5998,0,0),10,'m4'),
  'A gap that turns straight back cannot be routed through')

-- A roundabout is refused when the player has to name an exit. A route names it.
pos,facing=vec3(4960,0,0),vec3(1,0,0)
local ring2=assert(rd.assistantStart(player))
assert(rd.assistantUpdate(ring2,player))
assert(ring2.junction and ring2.junction.unsupported, 'Refused without a route')
assert(rd.assistantSetDestination(ring2,vec3(5030,8,0),10,'r5'))
assert(ring2.junction==nil and pathHas(ring2.path,'r3') and pathHas(ring2.path,'r5'), 'Driven as nodes')
assert(ring2.nav.turns[1].name=='roundabout' and #ring2.nav.turns==1, 'One word for the ring')

-- A fork inside 30 degrees is still a choice, and has to be named.
pos,facing=vec3(30010,0,0),vec3(1,0,0)
local fork=assert(rd.assistantStart(player))
assert(rd.assistantUpdate(fork,player))
assert(rd.assistantSetDestination(fork,vec3(30150,-10,0),10,'f4'))
assert(fork.nav.turns[1].name=='keep right', 'Negative bearings are right')
-- Straight on past a slip link: no instruction and no stop bar. Run on the live
-- map, this said "keep left" every 300 feet and never stopped counting in.
pos,facing=vec3(31010,0,0),vec3(1,0,0)
local slip=assert(rd.assistantStart(player))
assert(rd.assistantUpdate(slip,player))
assert(rd.assistantSetDestination(slip,vec3(31250,0,0),10,'s3'))
assert(#slip.nav.turns==0, 'Carrying straight on is not a turn')
pos=vec3(31080,0,0)
assert(rd.assistantUpdate(slip,player))
assert(rd.proximityAt(pos,facing,10,nil,slip)==nil, 'A junction driven straight through is no stop')
pos=vec3(31220,0,0)
assert(rd.assistantUpdate(slip,player))
assert(rd.proximityAt(pos,facing,10,nil,slip).kind=='destination', 'The destination still is')
-- A bend with a side link is no turn, however far the road swings.
pos,facing=vec3(32010,0,0),vec3(1,0,0)
local bend=assert(rd.assistantStart(player))
assert(rd.assistantUpdate(bend,player))
assert(rd.assistantSetDestination(bend,vec3(32240,116,0),10,'k3'))
assert(#bend.nav.turns==0, 'The road only bends: 40 degrees, its rival twice that')
-- Straight across a real crossroads: nothing to say, but it does get a bar.
pos,facing=vec3(33010,0,0),vec3(1,0,0)
local cross=assert(rd.assistantStart(player))
assert(rd.assistantUpdate(cross,player))
assert(rd.assistantSetDestination(cross,vec3(33250,0,0),10,'x2'))
assert(#cross.nav.turns==0)
pos=vec3(33050,0,0)
assert(rd.assistantUpdate(cross,player))
assert(rd.proximityAt(pos,facing,10,nil,cross).id=='junction:x1', 'Equal roads crossing keep their bar')
-- Straightest is not enough when it leaves the main road: that is worth a word.
pos,facing=vec3(34010,0,0),vec3(1,0,0)
local leaving=assert(rd.assistantStart(player))
assert(rd.assistantUpdate(leaving,player))
assert(rd.assistantSetDestination(leaving,vec3(34250,0,0),10,'y3'))
assert(leaving.nav.turns[1] and leaving.nav.turns[1].name=='keep left', 'The main road bends off to the right')
-- Italy's port: the only way round is a 40 m loop a car cannot drive. No route is
-- the honest answer, not a route off the road.
pos,facing=vec3(35050,0,0),vec3(1,0,0)
local port=assert(rd.assistantStart(player))
assert(rd.assistantUpdate(port,player))
local portRoute,portWhy=rd.assistantSetDestination(port,vec3(34950,0,0),10,'p0')
assert(not portRoute and portWhy=='No forward route to the destination', 'A 40 m loop is not a way round')
-- A true roundabout of the same size is built to be driven round.
pos,facing=vec3(4960,0,0),vec3(1,0,0)
local roundAgain=assert(rd.assistantStart(player))
assert(rd.assistantUpdate(roundAgain,player))
assert(rd.assistantSetDestination(roundAgain,vec3(4955,0,0),10,'r1'), 'A one-way ring can turn a car round')
assert(pathHas(roundAgain.path,'r3') and pathHas(roundAgain.path,'r4'))
-- A real turn on a route still gets its bar.
pos,facing=vec3(10,0,0),vec3(1,0,0)
local barTurn=assert(rd.assistantStart(player))
assert(rd.assistantUpdate(barTurn,player))
assert(rd.assistantSetDestination(barTurn,vec3(100,150,0),10,'l'))
pos=vec3(50,0,0)
assert(rd.assistantUpdate(barTurn,player))
assert(rd.proximityAt(pos,facing,10,nil,barTurn).id=='junction:b', 'The turn is a stop')
pos,facing=vec3(10,0,0),vec3(1,0,0)

-- The coordinator, on the real detector, with the game's route mocked.
local routeEvents, routePlan, routeCommands, focusCleared = {}, nil, {}, 0
local routeVehicle={getID=function() return 42 end,getVelocity=player.getVelocity,
  getPosition=player.getPosition,getDirectionVector=player.getDirectionVector,
  queueLuaCommand=function(_,c) routeCommands[#routeCommands+1]=c end}
local gm={hasTarget=false,clearPathOnReachingTarget=true,routePlanner={path={}},
  currentlyHasTarget=function() return gm_hasTarget end}
local function setRoute(x,y,wp)
  gm.currentlyHasTarget=function() return true end
  gm.routePlanner.path={{pos=vec3(pos.x,pos.y,0)},{pos=vec3(0,0,0),wp=wp},{pos=vec3(x,y,0)}}
end
local function clearRoute()
  gm.currentlyHasTarget=function() return false end
  gm.routePlanner.path={}
end
gm.setPath=function(value) assert(value==nil); clearRoute() end
clearRoute()
local bigMap={setNavFocus=function(value) assert(value==nil); focusCleared=focusCleared+1; clearRoute() end}
local route=module('bng_mod/lua/ge/extensions/drivingAssistant.lua', {
  be={getPlayerVehicle=function() return routeVehicle end,getPlayerVehicleID=function() return 42 end},
  scenetree={findObjectById=function() return routeVehicle end},
  core_groundMarkers=gm, freeroam_bigMapMode=bigMap, log=function() end,
  extensions={roadDetector=rd},
  require=function(name)
    if name=='ge/extensions/assistantRoutes' then return routes end
    return {}
  end,
  serialize=function(value) routePlan=value; return '{}' end,
  jsonEncode=function(value) routeEvents[#routeEvents+1]=value; return '{}' end})
local function kinds(kind)
  local out={}
  for _, e in ipairs(routeEvents) do if e.kind==kind then out[#out+1]=e end end
  return out
end
local function session(token)
  routeEvents={}
  route.command('ASSISTANT_TOGGLE',token,function() end)
  route.event(42,token,'ready','Driving assistant on')
end
local function tick(token, dt)
  route.command('ASSISTANT_HEARTBEAT',token,function() end)
  route.event(42,token,'alive','')
  route.update(dt or 0.1)
end

-- Set before switch-on: spoken after "on", which would otherwise interrupt it.
pos,facing=vec3(10,0,0),vec3(1,0,0)
setRoute(100,150,'l')
session('r1')
assert(routeEvents[1].kind=='on' and routeEvents[2].kind=='route_set')
assert(near(routeEvents[2].data.destDistance,240) and routeEvents[2].data.turn=='left')
assert(routePlan.junction==nil and pathHas(routePlan.path,'l'), 'The vehicle starts on the route')
assert(routePlan.replanRevision==0, 'The first path is no replan')
assert(route.diagnosticState().routeFollowing)
tick('r1',0.6)
assert(#kinds('route')==1 and near(kinds('route')[1].data.turnDistance,86))
assert(#kinds('route_set')==1, 'An unchanged route is not searched or announced again')
-- Removed by the player: not an arrival, and the session carries on.
clearRoute()
tick('r1')
assert(route.owns() and #kinds('route_clear')==1 and not route.diagnosticState().routeFollowing)
assert(routePlan.replanRevision==1 and routePlan.junction and routePlan.junction.choices.left)
assert(#kinds('off')==0)
-- Set mid-session: announced, and the vehicle's planner is restarted.
setRoute(100,150,'l')
tick('r1')
assert(#kinds('route_set')==2 and routePlan.replanRevision==2 and pathHas(routePlan.path,'l'))
-- Moved: searched again, but a route that keeps moving cannot search every tick.
setRoute(100,180,'l')
tick('r1')
assert(#kinds('route_set')==2, 'Throttled')
tick('r1',1.1); tick('r1',1.1)
assert(#kinds('route_set')==3 and routePlan.replanRevision==3)
-- Through reverse: the fresh forward state follows the route again.
route.event(42,'r1','motion','reverse')
tick('r1')
assert(route.diagnosticState().motion=='reverse')
route.event(42,'r1','motion','forward')
tick('r1')
assert(route.diagnosticState().routeFollowing, 'Route survives backing up')
assert(#kinds('route_set')==4)
-- Arrival on the road: cleared once, session over, nothing left to trap the next start.
pos,facing=vec3(100,50,0),vec3(0,1,0)
tick('r1')
pos=vec3(100,175,0)
tick('r1')
assert(not route.owns() and focusCleared==1 and not gm.currentlyHasTarget())
local off=kinds('off')[1]
assert(off.data.arrival.offRoad==false and off.message:find('Destination reached',1,true))
pos,facing=vec3(10,0,0),vec3(1,0,0)
session('r2')
assert(route.owns() and #kinds('off')==0 and not route.diagnosticState().routeFollowing,
  'Switching on again just works')
route.off()

-- Off the road: stock would never clear this one, so the mod has to.
focusCleared=0
setRoute(140,150,'l')
session('r3')
assert(routeEvents[2].data.offRoad==true)
pos,facing=vec3(100,50,0),vec3(0,1,0)
tick('r3')
pos=vec3(100,145,0)
tick('r3')
off=kinds('off')[1]
assert(not route.owns() and focusCleared==1, 'The mod clears what stock would not')
assert(off.data.arrival.offRoad==true and near(off.data.arrival.distance,40.3,0.1))
assert(off.data.arrival.bearing<-80 and off.data.arrival.bearing>-90, 'Facing north, east is on the right')
assert(off.data.arrival.x==140 and off.data.arrival.y==150, 'Enough to mark it as a waypoint')

-- Stock arrives first (within 7 m by its own count): the hook is the arrival.
pos,facing=vec3(10,0,0),vec3(1,0,0)
setRoute(140,150,'l')
session('r4')
route.reached()
assert(not route.owns() and kinds('off')[1].data.arrival.offRoad==false, 'The game says it was reached')
clearRoute()
route.reached() -- no session: nothing to do, and no error

-- A route the mod does not own is left alone, and cannot trap the next switch-on.
focusCleared=0
setRoute(100,150,'l')
gm.clearPathOnReachingTarget=false
session('r5')
pos,facing=vec3(100,50,0),vec3(0,1,0)
tick('r5')
pos=vec3(100,145,0)
tick('r5')
assert(not route.owns() and focusCleared==0 and gm.currentlyHasTarget(), 'Not ours to clear')
session('r6')
assert(route.owns() and #kinds('off')==0 and not route.diagnosticState().routeFollowing,
  'It still engages, at the same place, with the same route set')
local foundAlready=false
for _, e in ipairs(kinds('say')) do if e.message=='Already at the destination' then foundAlready=true end end
assert(foundAlready)
tick('r6')
assert(#kinds('route_set')==0, 'And it stays ignored')
route.off()
gm.clearPathOnReachingTarget=true

-- Already at a route we own: cleared at switch-on, and the session continues.
focusCleared=0
setRoute(100,147,'l')
session('r7')
assert(route.owns() and focusCleared==1 and not gm.currentlyHasTarget())
route.off()

-- No forward route: said once, ordinary junctions, no disengagement.
pos,facing=vec3(1050,0,0),vec3(1,0,0)
setRoute(1010,0,'u')
session('r8')
for _ = 1, 5 do tick('r8',1.1) end
local refused=0
for _, e in ipairs(kinds('say')) do if e.message:find('No forward route',1,true) then refused=refused+1 end end
assert(route.owns() and refused==1 and not route.diagnosticState().routeFollowing, 'One apology, not one a tick')
route.off()

-- A route still being computed is neither a route nor a removal.
pos,facing=vec3(10,0,0),vec3(1,0,0)
gm.currentlyHasTarget=function() return true end
gm.routePlanner.path={}
session('r9')
assert(route.owns() and #kinds('route_set')==0 and #kinds('route_clear')==0)
setRoute(100,150,'l')
tick('r9')
assert(#kinds('route_set')==1 and routePlan.replanRevision==1, 'Picked up when it is ready')
route.off()
clearRoute()

-- Set before an off-road switch-on: the join stage hands over onto the route.
pos,facing=vec3(50,60,0),vec3(1,0,0)
setRoute(100,150,'l')
routeEvents={}
route.command('ASSISTANT_TOGGLE','r10',function() end)
assert(route.diagnosticState().joining and #kinds('route_set')==0)
pos,facing=vec3(10,0,0),vec3(1,0,0)
route.update(0.1)
route.event(42,'r10','ready','on')
assert(#kinds('route_set')==1 and pathHas(routePlan.path,'l'), 'The route waits out the join')
route.off()
clearRoute()

-- ================================================================================================
--  Three point turn
-- ================================================================================================
local stepInput = function(err, speed_, gear, front, rear)
  return {err=err, speed=speed_, gear=gear, front=front, rear=rear, dt=0.1}
end
local leg = routes.turnBegin(170, 20, 10, 12, 3)
assert(leg.side==1 and leg.phase=='forward', 'Dead away: the roomier side, the roomier way')
assert(routes.turnBegin(170, 3, 10, 2, 9).side==-1 and routes.turnBegin(170, 3, 10, 2, 9).phase=='reverse')
assert(routes.turnBegin(-100, 5, 5, 20, 0).side==-1, 'Otherwise the side the road is on')
local step = routes.turnStep(leg, stepInput(170, 0, 0, 20, 10))
assert(step.steering==-1 and leg.waiting and not step.say, 'Left lock forward; waits for a forward gear')
step = routes.turnStep(leg, stepInput(170, 0, 1, 20, 10))
assert(not leg.waiting)
step = routes.turnStep(leg, stepInput(150, 2, 1, 5, 10))
assert(not step.say, 'Room left')
step = routes.turnStep(leg, stepInput(120, 2, 1, 1.5, 10))
assert(step.say=='Stop. Reverse' and leg.phase=='reverse' and step.steering==1,
  'Out of room: reverse, on opposite lock so the nose keeps swinging left')
step = routes.turnStep(leg, stepInput(120, 1, 1, 1.5, 10))
assert(not step.say, 'Said once')
for _ = 1, 21 do step = routes.turnStep(leg, stepInput(120, 1, 1, 1.5, 10)); if step.say then break end end
assert(step.say=='Stop. Reverse', 'Said again if the car is still rolling on towards it')
step = routes.turnStep(leg, stepInput(120, 0, -1, 1.5, 10))
assert(not leg.waiting and not step.say)
step = routes.turnStep(leg, stepInput(80, -1.5, -1, 8, 1))
assert(step.say=='Stop. Forward' and leg.phase=='forward' and step.steering==-1)
step = routes.turnStep(leg, stepInput(80, 0, 1, 8, 8))
step = routes.turnStep(leg, stepInput(20, 2, 1, 8, 8))
assert(step.done, 'Facing the road, in a forward gear: done')
-- Facing the road but still in reverse: one more shift first.
leg = routes.turnBegin(100, 1, 10, 5, 5)
routes.turnStep(leg, stepInput(100, 0, -1, 1, 10))
step = routes.turnStep(leg, stepInput(10, -1, -1, 5, 5))
assert(step.say=='Stop. Forward' and not step.done)
step = routes.turnStep(leg, stepInput(10, 0, 1, 5, 5))
assert(step.done)
-- Walled in both ways, and a turn that never finishes, both end honestly.
leg = routes.turnBegin(100, 5, 5, 5, 5)
routes.turnStep(leg, stepInput(100, 0, 1, 5, 5))
step = routes.turnStep(leg, stepInput(100, 0, 1, 0.3, 0.3))
assert(step.fail=='No room to turn round')
leg = routes.turnBegin(100, 5, 5, 5, 5)
leg.time = 200
assert(routes.turnStep(leg, stepInput(100, 0, 1, 5, 5)).fail=='Could not turn round')

-- When a turn is worth it, on the real detector.
pos,facing=vec3(20050,0,0),vec3(1,0,0)
local choiceState=assert(rd.assistantStart(player))
assert(rd.assistantUpdate(choiceState,player))
local choice=rd.assistantTurnChoice(choiceState,vec3(19950,0,0),'b1',player)
assert(choice.turn and near(choice.backLength,100) and near(choice.forwardLength,600),
  'Once round the block costs 500 m against turning round')
choice=rd.assistantTurnChoice(choiceState,vec3(20150,0,0),'b3',player)
assert(not (choice and choice.turn), 'Ahead already: no turn')
pos,facing=vec3(10,0,0),vec3(1,0,0)
choiceState=assert(rd.assistantStart(player))
assert(rd.assistantUpdate(choiceState,player))
choice=rd.assistantTurnChoice(choiceState,vec3(100,150,0),'l',player)
assert(not (choice and choice.turn), 'On the way: no turn')
-- No forward route at all, as at a tiny loop.
pos,facing=vec3(35050,0,0),vec3(1,0,0)
choiceState=assert(rd.assistantStart(player))
assert(rd.assistantUpdate(choiceState,player))
choice=rd.assistantTurnChoice(choiceState,vec3(34950,0,0),'p0',player)
assert(choice.turn and choice.forwardLength==nil, 'No way forward at all: turning round is the way')
-- A one-way road has no turning round.
pos,facing=vec3(1050,0,0),vec3(1,0,0)
choiceState=assert(rd.assistantStart(player))
assert(rd.assistantUpdate(choiceState,player))
assert(rd.assistantTurnChoice(choiceState,vec3(1010,0,0),'u',player)==nil)
-- A lane between walls does not get a three point turn.
pos,facing=vec3(20050,0,0),vec3(1,0,0)
choiceState=assert(rd.assistantStart(player))
assert(rd.assistantUpdate(choiceState,player))
rayHit=3
assert(not rd.assistantTurnChoice(choiceState,vec3(19950,0,0),'b1',player).turn, 'Six metres across is not enough')
-- Room: a wall ahead, and a drop no forward ray can see.
local reading=rd.assistantTurnReading(player,choiceState.records[1])
assert(near(reading.front,3) and near(reading.rear,3))
rayHit=nil
reading=rd.assistantTurnReading(player,choiceState.records[1])
assert(near(math.abs(reading.err),180,0.01), 'The road the other way')
assert(reading.front==30, 'Along the road nothing stops it')
facing=vec3(0,1,0)
reading=rd.assistantTurnReading(player,choiceState.records[1])
assert(near(reading.front,5.5-2.25,0.01), 'Across the road: the verge limits it')
dropAt=1.5
reading=rd.assistantTurnReading(player,choiceState.records[1])
assert(near(reading.front,2,0.01), 'A drop, as off the edge of a pier, ends the leg before the verge')
dropAt=nil
facing=vec3(1,0,0)

-- The whole sequence, in the coordinator.
local turnEvents, turnCommands = {}, {}
local turnVehicle={getID=function() return 42 end,getVelocity=function() return vec3(0,0,0) end,
  getPosition=player.getPosition,getDirectionVector=player.getDirectionVector,
  queueLuaCommand=function(_,c) turnCommands[#turnCommands+1]=c end}
local turner=module('bng_mod/lua/ge/extensions/drivingAssistant.lua', {
  be={getPlayerVehicle=function() return turnVehicle end,getPlayerVehicleID=function() return 42 end},
  scenetree={findObjectById=function() return turnVehicle end},
  core_groundMarkers=gm, freeroam_bigMapMode=bigMap, log=function() end,
  extensions={roadDetector=rd},
  require=function(name)
    if name=='ge/extensions/assistantRoutes' then return routes end
    return {}
  end,
  serialize=function() return '{}' end,
  jsonEncode=function(value) turnEvents[#turnEvents+1]=value; return '{}' end})
local function hasCommand(text)
  for _, c in ipairs(turnCommands) do if c:find(text,1,true) then return true end end
  return false
end
pos,facing=vec3(20050,0,0),vec3(1,0,0)
setRoute(19950,0,'b1')
turner.command('ASSISTANT_TOGGLE','t1',function() end)
assert(turner.owns() and turner.diagnosticState().turning, 'Stopped, destination behind, long way round: turn')
assert(hasCommand('beamtelAssistant.turn("t1")') and not hasCommand('beamtelAssistant.start'))
assert(turnEvents[1].kind=='say' and turnEvents[1].message:find('Three point turn',1,true) and turnEvents[1].data.protect)
turner.update(0.1)
assert(hasCommand('setTurn("t1",'), 'Steering is sent')
turner.event(42,'t1','gear','1')
turner.update(0.1)
facing=vec3(-1,0,0)
turner.update(0.1)
assert(not turner.diagnosticState().turning and hasCommand('beamtelAssistant.start("t1"'), 'Facing the road: handed to the route')
turner.event(42,'t1','ready','Driving assistant on')
local turnSet
for _, e in ipairs(turnEvents) do if e.kind=='route_set' then turnSet=e end end
assert(turnSet and near(turnSet.data.destDistance,100), 'The route now runs straight there')
turner.update(0.1)
assert(turner.diagnosticState().routeFollowing)
turner.off()
clearRoute()

print('Driving assistant diagnostics passed')
