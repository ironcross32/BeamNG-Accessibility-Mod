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
vm.start('dead-end',{path={'a','b'},distance=10,junction={id='dead',choices={}}})
vm.updateGFX(0.01)
assert(ai.mode=='disabled', 'A dead end still has no valid route')
local savedDrive = ai.driveUsingPath
ai.driveUsingPath = function() error('planner failure') end
vm.start('failure',{path={'a','b'},distance=100})
assert(ai.mode=='disabled' and main.setGearboxMode==originalSetter, 'Restore the gearbox setter on failure')
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

-- A one-second turn hold survives release, without extending the route early.
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
hold(-1,0.4); hold(0,0.1); hold(-1,0.6)
assert(not vm.diagnosticState().selection, 'Separate short holds must not add up')
hold(-1,0.4)
assert(vm.diagnosticState().selection=='left' and not vm.diagnosticState().locked)
assert(#paths==latchPaths, 'Selecting early does not commit or replan the route')
hold(0,0.5); hold(0,0.5)
assert(vm.diagnosticState().selection=='left', 'Release must preserve the selected turn')
hold(1,0.5)
assert(vm.diagnosticState().selection=='left', 'A short correction cannot replace the selection')
hold(1,0.5)
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
values.gearIndex, speed = 0, -0.5
vm.start('rollback',{path={'a','b'},distance=100})
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
local function link(a,b,oneWay)
  local edge={inNode=a,outNode=b,oneWay=oneWay,drivability=1}
  nodes[a].links[b]=edge; nodes[b].links[a]=edge
end
link('a','b'); link('b','c',true); link('b','l'); link('c','d'); link('c','r');
link('l','endL'); link('r','endR')
nodes.u, nodes.v = node(1000,0), node(1100,0)
link('u','v',true)
local pos, facing=vec3(10,0,0),vec3(1,0,0)
local player={getPosition=function() return pos end, getDirectionVector=function() return facing end,
  getVelocity=function() return vec3(10,0,0) end}
local rayHit
local rd=module('bng_mod/lua/ge/extensions/roadDetector.lua', {
  map={getMap=function() return {nodes=nodes} end}, log=function() end,
  castRayStatic=function(_, _, range) return rayHit or range end,
  require=function(name) if name=='ge/extensions/assistantRoutes' then return routes else return {} end end})
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
assert(legal.roadMessage=='One-way road. Legal travel is ahead')

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
