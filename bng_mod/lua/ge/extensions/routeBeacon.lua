-- routeBeacon.lua
--
-- Where the destination of the map's navigation route is, so Python can sound a beacon at it.
--
-- Setting a route on the big map draws a ground-marker line and, when the game's navigation
-- arrows are enabled, occasionally emits "Please make a U-turn when possible" as a ui_message.
-- That sentence is the ONLY thing a driver who cannot see the line gets out of a set route --
-- it says the current heading is wrong and nothing whatever about which way is right. This file
-- supplies the missing half: the destination's world position, from which Python derives a
-- crow-flies bearing and pulses a panned beacon at it.
--
-- Send only, port 4482. No command port, and that is deliberate rather than incidental, for the
-- reason trailerAngle.lua gives: "is a route set" is a fact the game already maintains, so there
-- is nothing to tell this file. Whether the beacon SOUNDS is a Python-side toggle; whether a
-- route EXISTS is not a question the driver answers. That also keeps this extension outside the
-- setsockname/onExtensionUnloaded/retryCmdBind contract the listening extensions share and that
-- vehicle_geometry_sim.lua scenario 12 polices -- it binds nothing, so it is on none of it.
--
-- Silent on a game with no route by construction: currentlyHasTarget() is one field test, and a
-- tick with no route costs that plus an early return.

local M = {}

local PYTHON_HOST      = "127.0.0.1"
local PYTHON_PORT_DATA = 4482   -- send the route destination to Python

-- ================================================================================================
--  Rates and thresholds
-- ================================================================================================
-- The destination is STATIC -- it only moves when the route is changed -- so there is nothing
-- here that needs the 20 Hz the trailer angle runs at. Python derives the bearing itself from
-- its own 60 Hz telemetry, which is the whole reason this file sends a position rather than an
-- angle: the beacon's panning then updates at telemetry rate and cannot jitter on a dropped
-- datagram.
local TICK_HZ = 10.0

-- Resend when the rounded destination moves by this much.
local SEND_EPSILON_M = 0.5
-- ...and a heartbeat regardless. It does double duty here: it is what keeps Python's age-out
-- from expiring a live route, AND it is what refreshes the remaining-route distance, which is
-- deliberately NOT part of the change test below. Must stay well under Python's ROUTE_STALE_SEC.
local HEARTBEAT_S = 0.35

-- ================================================================================================
--  State
-- ================================================================================================
local udpSend = nil
local tickAcc = 0
local lastSentDest = nil     -- {x, y, z} last put on the wire, or nil if CLEAR was last sent
local lastSendT = 0
local sinceT = 0             -- accumulated real time, for the heartbeat

local function rbLog(level, msg) log(level, 'routeBeacon', msg) end

local function send(line)
  if udpSend then pcall(function() udpSend:send(line) end) end
end

-- ================================================================================================
--  Reading the route
-- ================================================================================================
-- The destination is the LAST PATH NODE, never core_groundMarkers.getTargetPos()/endWP[1].
-- endWP[1] is a vec3 for a route set by clicking the map and a navgraph waypoint-name STRING for
-- one set by name -- the game's own markerInteraction.lua guards it with type(...) == "cdata"
-- before daring to use it. path[#path].pos is always a real position, so it needs no type test
-- and no map.getMap() lookup, and it is what the game's own minimap route drawer uses as the
-- destination marker.
--
-- Note also that ui_uiNavi.route_inprogress() is NOT the way to ask whether a route is set: that
-- is the legacy navigation path and its destination is not updated when the big map sets a route,
-- so it answers false on a live one.
local function readRoute()
  local gm = core_groundMarkers
  if not gm or not gm.currentlyHasTarget or not gm.currentlyHasTarget() then return nil end
  local planner = gm.routePlanner
  local path = planner and planner.path
  if type(path) ~= "table" then return nil end
  local n = #path
  if n < 1 then return nil end
  local dest = path[n] and path[n].pos
  if not dest then return nil end
  -- Remaining distance ALONG the route, which is a different number from the crow-flies range
  -- Python computes, and is worth having: it is what says whether the straight line the beacon
  -- draws is anything like the drive.
  local remaining = 0
  local okLen, len = pcall(gm.getPathLength)
  if okLen and type(len) == "number" then remaining = len end
  return dest, remaining
end

local function sendClear()
  if lastSentDest ~= nil then
    send("ROUTE:CLEAR")
    lastSentDest = nil
    lastSendT = 0
  end
end

local function tick()
  local dest, remaining = readRoute()
  if not dest then sendClear() return end

  -- Round before the change test, not after -- the rule the ramp hydraulics push and the trailer
  -- angle both record. A route node sitting on a vehicle-tracked path jitters in the low decimals,
  -- and testing the raw value would re-send forever about a destination that has not moved.
  local dx = math.floor(dest.x * 10 + 0.5) / 10
  local dy = math.floor(dest.y * 10 + 0.5) / 10
  local dz = math.floor(dest.z * 10 + 0.5) / 10

  -- The change test is on the DESTINATION ONLY. The remaining distance falls continuously while
  -- the car drives, so including it would make every tick a change and turn the throttle off. It
  -- rides the heartbeat instead, which refreshes it about three times a second -- far more often
  -- than the one place that reads it (the spoken toggle-on sentence) needs.
  local moved = (lastSentDest == nil)
    or math.abs(dx - lastSentDest[1]) >= SEND_EPSILON_M
    or math.abs(dy - lastSentDest[2]) >= SEND_EPSILON_M
    or math.abs(dz - lastSentDest[3]) >= SEND_EPSILON_M
  local due = (sinceT - lastSendT) >= HEARTBEAT_S
  if not (moved or due) then return end

  send(string.format("ROUTE:%.1f,%.1f,%.1f,%.1f", dx, dy, dz, remaining))
  lastSentDest = {dx, dy, dz}
  lastSendT = sinceT
end

-- ================================================================================================
--  Lifecycle
-- ================================================================================================
local function setupSockets()
  if udpSend then pcall(function() udpSend:close() end); udpSend = nil end
  udpSend = socket.udp()
  if udpSend then
    udpSend:setpeername(PYTHON_HOST, PYTHON_PORT_DATA)
    udpSend:settimeout(0)
  else
    rbLog('E', "Failed to create UDP send socket.")
  end
end

function M.onExtensionLoaded()
  setExtensionUnloadMode(M, "manual")
  setupSockets()
  rbLog('I', "Route beacon loaded.")
end

function M.onExtensionUnloaded()
  if udpSend then pcall(function() udpSend:close() end); udpSend = nil end
end

function M.onWorldReadyState(state)
  if state == 2 then
    lastSentDest = nil
    lastSendT = 0
    tickAcc = 0
    setupSockets()
  end
end

-- A route is per level, so leaving one ends it. Clearing the latch here rather than waiting for
-- the next tick is what makes the beacon stop at the loading screen instead of one tick into the
-- next level, still pointing at a destination on a map that has been unloaded.
function M.onClientEndMission()
  lastSentDest = nil
  lastSendT = 0
end

function M.onUpdate(dtReal, dtSim, dtRaw)
  -- The GE onUpdate chain is dispatched WITHOUT pcall, so an uncaught throw in here would
  -- silently stop every extension loaded after this one in modScript.lua.
  sinceT = sinceT + dtReal
  tickAcc = tickAcc + dtReal
  if tickAcc < 1.0 / TICK_HZ then return end
  tickAcc = 0
  local ok, err = pcall(tick)
  if not ok then rbLog('E', "tick failed: " .. tostring(err)) end
end

-- The failure mode here is a plausible wrong number -- a destination read off the wrong end of
-- the path, or a stale one left over from a cleared route -- and no amount of listening to the
-- beacon identifies it. Printing what it chose is the only thing that does. Same argument
-- rampTruth and dockTruth make.
function M.diag()
  local out = {}
  local gm = core_groundMarkers
  if not gm then
    out[#out + 1] = "core_groundMarkers is not loaded"
  elseif not gm.currentlyHasTarget() then
    out[#out + 1] = "no route set (core_groundMarkers.endWP is nil)"
  else
    local planner = gm.routePlanner
    local path = planner and planner.path
    local n = (type(path) == "table") and #path or 0
    out[#out + 1] = string.format("route set, %d path nodes", n)
    if n > 0 then
      local dest = path[n].pos
      out[#out + 1] = string.format("  destination (last path node): %.1f, %.1f, %.1f",
        dest.x, dest.y, dest.z)
      -- endWP[1] is shown alongside precisely because it is the thing NOT used: a string here
      -- with a position above it is the resolve working as intended, not a disagreement.
      local wp = gm.getTargetPos and gm.getTargetPos() or nil
      out[#out + 1] = "  endWP[1] (deliberately unused): " .. type(wp) .. " " .. tostring(wp)
      out[#out + 1] = string.format("  remaining along route: %.1f m", gm.getPathLength())
    end
    local player = be:getPlayerVehicle(0)
    if player and n > 0 then
      local p = vec3(player:getPosition())
      local d = vec3(path[n].pos)
      local ddx, ddy = d.x - p.x, d.y - p.y
      out[#out + 1] = string.format("  crow-flies from player: %.1f m",
        math.sqrt(ddx * ddx + ddy * ddy))
    end
  end
  out[#out + 1] = "  last sent: " .. (lastSentDest
    and string.format("%.1f, %.1f, %.1f", lastSentDest[1], lastSentDest[2], lastSentDest[3])
    or "CLEAR")
  local text = table.concat(out, "\n")
  print(text)
  return text
end

return M
