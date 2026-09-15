-- Run in GE while paused, then unpause normally. Capture 30 simulation seconds
-- without changing the AI driver, and pause again automatically.
if _trafficApproachProbe and _trafficApproachProbe.proxy then
  _trafficApproachProbe.proxy:destroy()
end
local probe = {rows = {}, time = 0, nextSample = 0, duration = 30}
_trafficApproachProbe = probe
local function upvalue(fn, name)
  for i = 1, 100 do
    local key, value = debug.getupvalue(fn, i)
    if not key then break end
    if key == name then return value end
  end
end
function probe:onUpdate(dtReal, dtSim)
  if self.done or simTimeAuthority.getPause() then return end
  self.time = self.time + (dtSim or 0)
  if self.time < self.nextSample then return end
  self.nextSample = self.time + 0.2
  local player = be:getPlayerVehicle(0)
  if not player then return end
  local position, forward = player:getPosition(), player:getDirectionVector()
  local scan = upvalue(extensions.roadDetector.onUpdate, 'performScan')
  local edge = scan and upvalue(scan, 'currentEdge')
  local near = {}
  for _, signal in ipairs(core_trafficSignals.getSignals()) do
    local distance = signal.pos:distance(position)
    if distance < 250 and signal.dir:dot(forward) > 0.4 then
      near[#near + 1] = {id = signal.id, distance = distance}
    end
  end
  local frontOffset = 0
  for cid = 0, player:getNodeCount() - 1 do
    frontOffset = math.max(frontOffset, forward:dot(vec3(player:getNodePosition(cid))))
  end
  self.rows[#self.rows + 1] = {
    time = self.time, pos = {position.x, position.y, position.z},
    forward = {forward.x, forward.y, forward.z}, frontOffset = frontOffset,
    speed = player:getVelocity():length(), edge = edge and edge.id,
    snapshot = deepcopy(extensions.roadDetector.signalSnapshot()), near = near,
  }
  if self.time >= self.duration or (self.stopOnSignal and self.time > 1
    and self.rows[#self.rows].snapshot.signal) then
    self.done = true
    simTimeAuthority.pause(true)
  end
end
probe.proxy = newExtensionProxy(extensions.roadDetector, 'trafficApproachProbe')
probe.proxy:submitEventSinks({probe})
return 'Traffic approach recording armed; automatically pauses after 30 simulation seconds.'
