-- cannonShot.lua
--
-- What happened to the car you just fired out of the stock large_cannon.
--
-- The cannon is pure spectacle, and essentially none of it is legible without sight: a car
-- launches, tumbles, hits something and comes to rest, and the driver learns nothing at all.
-- This is not an aiming instrument -- the cannon has no horizontal aim to give, and predicting
-- where a soft-body car thrown by a hydraulic ram will land is not something anyone can derive.
-- It is an OUTCOME instrument: it watches the shot and reports where the car ended up. That
-- costs nothing extra, because the car has to be reset and re-driven into the ramp regardless.
--
-- It also quietly accumulates the one thing that would make prediction possible later: an
-- (elevation, strength) -> observed range row per shot, gathered from shots fired for their own
-- sake. Nothing here predicts anything, and nothing here commands the cannon.
--
-- Silent on ordinary vehicles by construction: with no large_cannon near the player a tick costs
-- one getPlayerVehicle, one cheap cache lookup per nearby vehicle, and an early return. There is
-- no per-vehicle name check anywhere in this file -- the capability test is rampGeometry.isCannon,
-- which asks the vehicle VM whether large_cannon's own controller is present.

local M = {}

local PYTHON_HOST      = "127.0.0.1"
local PYTHON_PORT_DATA = 4473   -- send shot outcomes to Python

-- No command port. The only user-facing setting is whether the outcome is SPOKEN, which is a
-- speech preference Python can honour on its own; pushing it here would buy nothing and cost a
-- socket, a handshake and a startup ordering problem. uiToggle is the existing precedent for a
-- single-direction port going the other way.

-- ================================================================================================
--  Launch detection
-- ================================================================================================
-- Observational, never hooked. The mod uses onGameplayEvent nowhere and has no visibility of the
-- cannon's `controller:large_cannon` fire trigger, and going and getting one would tie this file
-- to the way a shot happens to be triggered. implementProximity already derives "which cannon are
-- you in" by polling resolved geometry rather than by event; this follows it, and it is the same
-- measure-the-actual-thing rule that makes actualSteering and the jacking detector work.

-- How far along the machine, past the mouth plane, the projectile may be when it is launched.
-- large_cannon is about 16 m from mouth to muzzle, so this covers the whole barrel with room to
-- spare while excluding anything out in front of the machine.
local LAUNCH_MAX_ALONG_M = 25.0
-- ...and how far off the ramp centreline, over and above the mouth's own half-width.
local LAUNCH_LATERAL_PAD_M = 1.0
-- Speed ALONG the firing axis that says this is a launch and not a drive-in. You enter a ramp at
-- walking-to-jogging pace and leave the barrel at tens of metres per second, so there is a very
-- wide empty band between the two and the exact figure hardly matters.
local LAUNCH_MPS = 15.0
-- How near the player has to be to a cannon before it is watched at all.
local WATCH_RANGE_M = 60.0

-- ================================================================================================
--  Settling
-- ================================================================================================
-- These three are copied deliberately, not shared. vehicleSpawnerAccessible.lua carries the same
-- detector for its teleport-launch feature, and the two answer different questions -- that one
-- decomposes a MISS against an intended target, this one reports position along a firing axis --
-- so folding them together would put two callers behind one shape for the sake of fifteen lines.
-- The mod already duplicates small helpers on this reasoning (groundHeightBelow lives separately
-- in cameraInfo.lua and vehicleSpawnerAccessible.lua, "standalone by design"). What must NOT
-- happen is the numbers drifting apart, so cannon_shot_sim.py greps both files and asserts they
-- are identical.
local LAUNCH_TRACK_TIMEOUT = 30   -- seconds; give up rather than track forever
local LAUNCH_SETTLE_SPEED  = 2.0  -- m/s below which we call it landed
local LAUNCH_SETTLE_TIME   = 0.4  -- seconds it must stay that slow
-- ...and, as in the spawner, do not start counting until it is actually moving, so the frame or
-- two between the latch and the impulse taking effect cannot be mistaken for a landing.
local LAUNCH_ARM_S = 0.5

-- Something within this of the resting car is worth naming. Beyond it there is nothing to say,
-- and saying "nearest thing, ninety metres away" about open ground is worse than silence.
local NEAR_OBJECT_M = 8.0

-- Scan rate. The flight is seconds long and the numbers are metres, so there is nothing to gain
-- from running this per frame on a machine already doing soft-body physics for a tumbling car.
local TICK_HZ = 20.0

local udpSend = nil
local shot = nil          -- the in-flight shot, or nil
local session = {}        -- every completed shot this session, oldest first
local tickAcc = 0         -- real time, because the scan RATE is a real-time thing
local simAcc = 0          -- simulated time, because the FLIGHT is a physical thing

local function csLog(level, msg) log(level, 'cannonShot', msg) end

local function send(line)
  if udpSend then pcall(function() udpSend:send(line) end) end
end

-- Strip the separators the CSV protocol relies on, as implementProximity and vehicleScanner do.
local function cleanName(s)
  s = tostring(s or "")
  s = s:gsub("[,|;\r\n]", " ")
  s = s:gsub("^%s*(.-)%s*$", "%1")
  if s == "" or s:find("^table: ") then return "unknown" end
  return s
end

local function nameOf(veh)
  local ok, n = pcall(function()
    if extensions and extensions.vehicleNaming and extensions.vehicleNaming.describe then
      return extensions.vehicleNaming.describe(veh)
    end
    return nil
  end)
  if ok and type(n) == "string" and n ~= "" then return cleanName(n) end
  local f = veh:getJBeamFilename() or "unknown"
  return cleanName(f:match("([^/\\]+)%.jbeam$") or f)
end

-- ================================================================================================
--  Finding the cannon
-- ================================================================================================

-- The nearest large_cannon, or nil. rampGeometry.isCannon is self-arming and is documented as
-- cheap to call every tick, so this needs no state of its own -- and an ordinary car costs exactly
-- one cross-VM chunk for the whole session before it lands in that module's `failed` set and
-- answers nil forever.
--
-- isCannon, NOT has. "You can drive into this" and "this can throw a car" are different questions,
-- and they were the same predicate only for as long as large_cannon was the only vehicle whose
-- ramp resolved at all. rampGeometry's part tiers ended that: a rollback, a tilt deck and a dry
-- van all resolve, and every one of them then became a cannon this file was watching for a launch
-- out of. That is not a hypothetical -- it fired. A `us_semi_rollback_deck` parked at the roadside
-- resolves a 9.24 m ramp whose throat this file's gates cover to 25 m along and +-1.9 m lateral,
-- so simply DRIVING PAST it at road speed inside that corridor satisfies every launch condition,
-- and a car that was never in a cannon is tracked as a projectile. It is the same correction, for
-- the same reason, that implementProximity's CANNON: line already had to make.
local function findCannon(playerID, playerPos)
  local best, bestD = nil, WATCH_RANGE_M
  local rg = extensions and extensions.rampGeometry
  if not (rg and rg.isCannon) then return nil end
  for i = 0, be:getObjectCount() - 1 do
    local obj = be:getObject(i)
    if obj then
      local id = obj:getID()
      if id ~= playerID then
        local ok, d = pcall(function() return (vec3(obj:getPosition()) - playerPos):length() end)
        if ok and d and d < bestD and rg.isCannon(id) then best, bestD = id, d end
      end
    end
  end
  return best
end

-- ================================================================================================
--  Strength
-- ================================================================================================
-- The only figure here that is not geometry. large_cannon's controller publishes it by rewriting
-- electrics.values.gear as a percentage string, which is the same abuse beamtel.py suppresses on
-- the gear-change announcement. One chunk at launch; the answer lands a frame or two later, long
-- before the car does.
--
-- Built by concatenation with no string.format anywhere, so the percent sign in that string is
-- data rather than a format specifier -- the trap rampGeometry.lua's chunk documents at length.
local function requestStrength(cannonID)
  local veh = scenetree.findObjectById(cannonID)
  if not veh then return end
  pcall(function()
    veh:queueLuaCommand(
      "local g = tostring((electrics and electrics.values and electrics.values.gear) or '') "
      .. "local n = tonumber((g:gsub('[^0-9]', ''))) or -1 "
      .. "obj:queueGameEngineLua('if extensions.cannonShot then "
      .. "extensions.cannonShot.onStrength(' .. obj:getID() .. ',' .. n .. ') end')")
  end)
end

function M.onStrength(cannonID, pct)
  -- Only ever accepted for the shot that asked, and only while it is still in the air. A reply
  -- that arrives after the car has landed describes a cannon that has since retracted.
  if shot and shot.cannonID == cannonID and shot.strength < 0 then
    shot.strength = tonumber(pct) or -1
  end
end

-- ================================================================================================
--  The shot
-- ================================================================================================

local function beginShot(cannonID, frame, player, alongSpeed)
  local pos = vec3(player:getPosition())
  shot = {
    cannonID  = cannonID,
    vehID     = player:getID(),
    vehName   = nameOf(player),
    -- The frame is SNAPSHOTTED, never re-read. The assembly retracts and re-levels within a
    -- couple of seconds of firing, so measuring a landing against the live mouth would silently
    -- re-baseline the shot against a machine that has since moved -- and the elevation, which is
    -- the whole point of the range card, would be read after it had already gone.
    origin    = pos,
    axis      = vec3(frame.axis),
    left      = vec3(frame.left),
    -- The ramp and the barrel are on the same tilting assembly, so the ramp's live pitch IS the
    -- barrel elevation, already computed by mouthFrame and needing no bore resolve of its own.
    -- It is NOT the bore angle -- the two sit at different angles on the assembly -- but it is
    -- monotonic with it and exactly repeatable, which is all a range-card key has to be.
    rampPitch = frame.pitchDeg,
    strength  = -1,
    t         = 0,
    slowFor   = 0,
    apex      = 0,
    topSpeed  = alongSpeed,
  }
  requestStrength(cannonID)
  csLog('I', string.format(
    "shot away: %s from vehicle %d, ramp pitch %.1f deg, %.1f m/s along the axis",
    shot.vehName, cannonID, frame.pitchDeg, alongSpeed))
end

-- Whatever the car came to rest beside, or nil. Deliberately be:getObject only: in BeamNG most
-- props -- cones, barriers, haybales, pallets -- ARE spawned vehicle objects, while TSStatic is
-- thousands of trees and building shells on a stock map.
local function nearestTo(pos, exceptA, exceptB)
  local best, bestD = nil, NEAR_OBJECT_M
  for i = 0, be:getObjectCount() - 1 do
    local obj = be:getObject(i)
    if obj then
      local id = obj:getID()
      if id ~= exceptA and id ~= exceptB then
        local ok, d = pcall(function() return (vec3(obj:getPosition()) - pos):length() end)
        if ok and d and d < bestD then best, bestD = obj, d end
      end
    end
  end
  if not best then return nil, -1 end
  return nameOf(best), bestD
end

local function finishShot(veh, timedOut)
  local s = shot
  shot = nil

  local pos = vec3(veh:getPosition())
  local d = pos - s.origin
  local flat = vec3(d.x, d.y, 0)
  -- Measured from where the car WAS when it was launched, not from the mouth. That is the datum
  -- a person means by "how far did it go", and unlike the muzzle it needs no extra resolve.
  local downrange = flat:dot(s.axis)
  local lateral = flat:dot(s.left)   -- positive is LEFT, the mod-wide convention

  local nearName, nearDist = nearestTo(pos, s.vehID, s.cannonID)

  session[#session + 1] = {
    downrange = downrange, lateral = lateral, apex = s.apex, flight = s.t,
    pitch = s.rampPitch, strength = s.strength, veh = s.vehName, settled = not timedOut,
  }

  csLog('I', string.format(
    "shot %d: %.1f m downrange, %.1f m lateral, apex %.1f m, %.1f s, ramp pitch %.1f deg, "
      .. "strength %d, %s%s",
    #session, downrange, lateral, s.apex, s.t, s.rampPitch, s.strength, s.vehName,
    timedOut and " TIMEOUT" or ""))

  -- Metres and seconds; Python owns the units and every word of the phrasing. The previous
  -- shot's downrange rides along so the comparison clause does not need Python to have been
  -- listening for the whole session -- it is the mod that knows the shot count.
  -- Only a SETTLED previous shot is a distance worth comparing against; one that timed out has
  -- no landing place, so comparing to it would put a number on the thing that had none.
  local prev = -1e9
  for i = #session - 1, 1, -1 do
    if session[i].settled then prev = session[i].downrange break end
  end
  -- The two names go last because they are the only free-text fields; every numeric field keeps
  -- a fixed index, so Python's positional parse is unaffected by an empty name. prev trails them
  -- as the optional tail, the same contract the DOCK line's entry fields use.
  send(string.format("SHOT:%.2f,%.2f,%.2f,%.2f,%.2f,%d,%d,%d,%.2f,%s,%s,%.2f",
    downrange, lateral, s.apex, s.t, s.rampPitch, s.strength,
    timedOut and 0 or 1, #session, nearDist,
    nearName or "", s.vehName, prev))
end

local function updateShot(dt)
  local s = shot
  s.t = s.t + dt

  local veh = be:getObjectByID(s.vehID)
  if not veh then
    -- The projectile stopped existing: deleted, or the level went away. There is no outcome to
    -- report and inventing one would be worse than silence.
    shot = nil
    return
  end

  local pos = vec3(veh:getPosition())
  local rise = pos.z - s.origin.z
  if rise > s.apex then s.apex = rise end

  local spd = 0
  local okV, v = pcall(function() return vec3(veh:getVelocity()) end)
  if okV and v then spd = v:length() end
  if spd > s.topSpeed then s.topSpeed = spd end

  if s.t > LAUNCH_ARM_S and spd < LAUNCH_SETTLE_SPEED then
    s.slowFor = s.slowFor + dt
  else
    s.slowFor = 0
  end

  local settled = s.slowFor >= LAUNCH_SETTLE_TIME
  local timedOut = s.t >= LAUNCH_TRACK_TIMEOUT
  if settled or timedOut then finishShot(veh, timedOut) end
end

local function watchForLaunch(dt)
  -- No simulated time has passed, so nothing physical has happened and there is nothing to
  -- detect. It matters because the pose and the velocity a paused vehicle reports are the ones
  -- it had when the pause began: a car frozen at 26 m/s inside a barrel satisfies every gate
  -- below on every tick, forever. This is the cheap half of the pause fix, and it is worth
  -- having on its own -- the other half stops a latched shot ageing, this one stops it latching.
  if dt <= 0 then return end

  local player = be:getPlayerVehicle(0)
  if not player then return end
  local playerID = player:getID()

  local pos = vec3(player:getPosition())
  local cannonID = findCannon(playerID, pos)
  if not cannonID then return end

  local frame = extensions.rampGeometry.mouthFrame(cannonID)
  if not frame then return end

  local rel = pos - frame.centre
  local along = rel:dot(frame.axis)
  local lateral = rel:dot(frame.left)
  if along < 0 or along > LAUNCH_MAX_ALONG_M then return end
  if math.abs(lateral) > frame.halfW + LAUNCH_LATERAL_PAD_M then return end

  local okV, v = pcall(function() return vec3(player:getVelocity()) end)
  if not (okV and v) then return end
  if v:dot(frame.axis) < LAUNCH_MPS then return end

  beginShot(cannonID, frame, player, v:dot(frame.axis))
end

-- ================================================================================================
--  Sockets and hooks
-- ================================================================================================

local function setupSockets()
  if udpSend then pcall(function() udpSend:close() end); udpSend = nil end
  udpSend = socket.udp()
  if udpSend then
    udpSend:setpeername(PYTHON_HOST, PYTHON_PORT_DATA)
    udpSend:settimeout(0)
  else
    csLog('E', "Failed to create UDP send socket.")
  end
end

function M.onExtensionLoaded()
  setExtensionUnloadMode(M, "manual")
  setupSockets()
  csLog('I', "Cannon shot tracker loaded.")
end

function M.onWorldReadyState(state)
  if state == 2 then
    shot = nil
    session = {}
    tickAcc = 0
    simAcc = 0
    setupSockets()
  end
end

-- A reset mid-flight is the driver giving up on the shot, not a landing. Reporting one would put
-- a number on a flight that did not finish.
function M.onVehicleResetted(vehID)
  if shot and shot.vehID == vehID then shot = nil end
end

function M.onVehicleDestroyed(vehID)
  if shot and (shot.vehID == vehID or shot.cannonID == vehID) then shot = nil end
end

-- Switching out of the projectile abandons the shot for the same reason: what is being reported
-- is what happened to the car you were in.
function M.onVehicleSwitched(oldId, newId, player)
  if (player == nil or player == 0) and shot and shot.vehID ~= newId then shot = nil end
end

function M.onUpdate(dtReal, dtSim, dtRaw)
  -- The GE onUpdate chain is dispatched WITHOUT pcall, so an uncaught throw in here would
  -- silently stop every extension loaded after this one in modScript.lua.
  tickAcc = tickAcc + dtReal
  simAcc = simAcc + dtSim
  if tickAcc < 1.0 / TICK_HZ then return end
  -- The scan is paced in REAL time and the shot is aged in SIMULATED time, and they are two
  -- different clocks on purpose. A flight is a physical event: its duration, its settle window
  -- and its timeout are all facts about the car, so they have to be measured in the time the car
  -- experiences. Ageing on dtReal instead means the game being paused or slowed -- opening the
  -- radial menu does exactly this -- runs the 30 s timeout down while the projectile hangs
  -- motionless in the air, and the shot is then reported as "did not settle" having never had
  -- the chance to. Worse, with the launch conditions frozen true it re-latches on the next tick
  -- and does it again, every thirty seconds, for as long as the menu is open. That is not a
  -- hypothetical either: eight of them in a row are in the log, all exactly 30.0 s apart.
  local dt = simAcc
  tickAcc = 0
  simAcc = 0

  local ok, err = pcall(function()
    if shot then updateShot(dt) else watchForLaunch(dt) end
  end)
  if not ok then
    shot = nil
    csLog('E', "cannon shot tick failed: " .. tostring(err))
  end
end

-- ================================================================================================
--  Diagnostics
-- ================================================================================================

-- This instrument's failure mode is a plausible wrong number -- a shot measured against the wrong
-- axis reads exactly like a shot that went that way -- so it has to be able to print what it
-- decided, the argument rampGeometry.diag() and implementProximity.dockTruth() already make.
function M.diag()
  local out = {}
  if shot then
    out[#out + 1] = string.format(
      "IN FLIGHT: %s from cannon %d, %.1fs, apex %.1f m, ramp pitch %.1f deg, strength %d",
      shot.vehName, shot.cannonID, shot.t, shot.apex, shot.rampPitch, shot.strength)
  else
    local player = be:getPlayerVehicle(0)
    if not player then
      out[#out + 1] = "idle: no player vehicle"
    else
      local id = player:getID()
      local cannonID = findCannon(id, vec3(player:getPosition()))
      if not cannonID then
        out[#out + 1] = string.format("idle: no large_cannon within %.0f m", WATCH_RANGE_M)
      else
        local f = extensions.rampGeometry.mouthFrame(cannonID)
        if not f then
          out[#out + 1] = string.format("idle: cannon %d resolved but mouthFrame is nil -- %s",
            cannonID, tostring(extensions.rampGeometry.stateOf(cannonID)))
        else
          local rel = vec3(player:getPosition()) - f.centre
          out[#out + 1] = string.format(
            "idle: watching cannon %d, you are %.1f m along / %.1f m lateral "
              .. "(need 0 to %.0f along, within %.2f lateral, %.0f m/s along to launch)",
            cannonID, rel:dot(f.axis), rel:dot(f.left),
            LAUNCH_MAX_ALONG_M, f.halfW + LAUNCH_LATERAL_PAD_M, LAUNCH_MPS)
        end
      end
    end
  end
  if #session == 0 then
    out[#out + 1] = "no shots this session"
  else
    for i, s in ipairs(session) do
      out[#out + 1] = string.format(
        "  %d: %.1f m downrange, %.1f m %s, apex %.1f m, %.1f s, pitch %.1f deg, strength %d, %s%s",
        i, s.downrange, math.abs(s.lateral), s.lateral >= 0 and "left" or "right",
        s.apex, s.flight, s.pitch, s.strength, s.veh, s.settled and "" or " (never settled)")
    end
  end
  local text = table.concat(out, "\n")
  print(text)
  return text
end

return M
