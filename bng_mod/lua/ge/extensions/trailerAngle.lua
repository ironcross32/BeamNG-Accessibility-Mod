-- trailerAngle.lua
--
-- The yaw between the vehicle you are driving and the trailer hooked to it.
--
-- Backing a trailer is among the hardest things to do without sight, and the failure mode is
-- the quietest: a jackknife gives no warning at all until the trailer is already folded into
-- the cab. The mod already sonifies exactly this quantity for the WL-40 -- the yaw between the
-- front and rear halves of an articulated loader -- as a continuous tone that goes silent when
-- the two are in line. A tractor and its trailer are the same geometry with the hinge moved to
-- the coupler, so this file measures the same angle and Python feeds it to the same tone. There
-- is no second instrument and no second set of tones to learn.
--
-- Silent on ordinary vehicles by construction: with nothing coupled a tick costs one
-- getPlayerVehicle, one walk of a table that is empty on a solo car, and an early return. There
-- is no per-vehicle name check anywhere in this file, and no keybind -- "is a trailer attached"
-- is a fact the game already maintains, so there is nothing for the driver to switch on.
--
-- Send only, port 4476. No command port, and that is deliberate rather than incidental: this
-- file has nothing to be told. It follows cannonShot.lua, which makes the same argument. It
-- also means this extension binds no listening socket, so it is outside the
-- setsockname/onExtensionUnloaded/retryCmdBind contract that the fourteen listening extensions
-- share and that vehicle_geometry_sim.lua scenario 12 polices.

local M = {}

local PYTHON_HOST      = "127.0.0.1"
local PYTHON_PORT_DATA = 4476   -- send the trailer angle to Python

-- ================================================================================================
--  Rates and thresholds
-- ================================================================================================
-- The tone smooths its own pitch (HYDRO_STEER_PITCH_TAU 0.05) and its own envelope, so feeding
-- it faster than this buys nothing audible. 20 Hz is the same rate the implement block runs at.
local TICK_HZ = 20.0

-- Resend when the rounded angle moves by this much. Rounding BEFORE the change test, the way the
-- ramp hydraulics push does, is what stops a trailer sitting on its own springs jittering in the
-- sixth decimal and re-sending forever while the rig is parked.
local SEND_EPSILON_DEG = 0.25
-- ...and a heartbeat regardless, so Python's age-out cannot expire a live, perfectly steady
-- reading. Must stay well under the Python side's TRAILER_STALE_SEC.
local HEARTBEAT_S = 0.35

-- Sign calibration, and the reason it is a named constant rather than a bare negation buried in
-- the expression: which way the tone should lean for a trailer swinging a given way is a question
-- only the driver's seat settles, and the two candidate answers are both entirely reasonable read
-- from the source. 796F6C6F313035.lua carries STEER_ARTIC_SIGN for exactly this, for exactly this
-- reason. Flip this one constant if the pan comes out mirrored; change nothing else.
local TRAILER_ANGLE_SIGN = 1.0

-- ================================================================================================
--  State
-- ================================================================================================
local udpSend = nil
local tickAcc = 0
local lastSentDeg = nil      -- the last angle put on the wire, or nil if CLEAR was last sent
local lastSendT = 0
local sinceT = 0             -- accumulated real time, for the heartbeat
local lastGoodFwd = {}       -- vehID -> last non-degenerate FLATTENED forward vector

local function taLog(level, msg) log(level, 'trailerAngle', msg) end

local function send(line)
  if udpSend then pcall(function() udpSend:send(line) end) end
end

-- Strip the separators the CSV protocol relies on, as cannonShot and vehicleScanner do.
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
--  Finding the trailer
-- ================================================================================================
-- Read from the game's own registry rather than hooked. vehicleScanner already implements
-- onCouplerAttached, and it is precisely the wrong shape for this question: it fires once, at
-- the instant of coupling, and it is gated on an align having been armed first -- so a trailer
-- coupled any other way (drove up and pressed L, a save reloaded, a career session resumed) is
-- invisible to it forever. What this file needs is not an event but a fact, continuously: what
-- is hooked to me right now. The game maintains exactly that, GE-side, in two places.

-- Tier 1: core_trailerRespawn keeps a DIRECTED registry, tractor -> trailer, so it answers which
-- end is which without any geometry. It can be switched off (setEnabled(false) nops its hooks),
-- which is why it is a preference and not the base.
local function trailerFromRespawn(playerID)
  local ok, res = pcall(function()
    if not core_trailerRespawn or not core_trailerRespawn.getTrailerData then return nil end
    if core_trailerRespawn.getEnabled and not core_trailerRespawn.getEnabled() then return nil end
    local reg = core_trailerRespawn.getTrailerData()
    local entry = reg and reg[playerID]
    return entry and entry.trailerId or nil
  end)
  if ok then return res end
  return nil
end

-- Tier 2: core_vehicles.attachedCouplers is the flat pair list the engine appends to on every
-- attach and removes from on every detach and destroy. It is serialized across reloads, so it
-- survives the save/load case the hook cannot see. Pairs are unordered -- the engine fires from
-- whichever vehicle has the lower id -- so both slots are tested. freeformDelivery/goals.lua
-- reads it the same way.
local function partnersFromCouplers(playerID)
  local out = {}
  local ok = pcall(function()
    if not core_vehicles or not core_vehicles.attachedCouplers then return end
    for _, c in ipairs(core_vehicles.attachedCouplers) do
      if type(c) == "table" then
        if c[1] == playerID and c[2] then out[#out + 1] = c[2]
        elseif c[2] == playerID and c[1] then out[#out + 1] = c[1] end
      end
    end
  end)
  if not ok then return {} end
  return out
end

-- Only ever ONE trailer is reported, and on a rig with more than one it is the one hinged
-- directly to the driven vehicle. That is the joint that folds into the cab, it is the one the
-- driver can still do something about, and a second continuous channel for the trailer behind
-- the trailer is exactly the mistake the obstacle detector already made once.
--
-- Where the pair list offers a choice (a truck towing at both ends), the one BEHIND the player
-- wins: you jackknife what you are pulling, not what is pulling you.
local function findTrailer(playerID, playerFrame)
  local direct = trailerFromRespawn(playerID)
  if direct then return direct end

  local cands = partnersFromCouplers(playerID)
  if #cands == 0 then return nil end
  if #cands == 1 then return cands[1] end

  local best, bestBehind = nil, nil
  for _, id in ipairs(cands) do
    local f = extensions.vehicleGeometry.boxFrame(id)
    if f then
      -- Negative is behind. Most negative wins, so a genuine trailer beats a nose-on tow.
      local behind = (f.c - playerFrame.c):dot(playerFrame.f)
      if bestBehind == nil or behind < bestBehind then
        best, bestBehind = id, behind
      end
    end
  end
  return best or cands[1]
end

-- ================================================================================================
--  The angle
-- ================================================================================================

-- Flattened to horizontal, and that is not cosmetic. The quantity wanted is yaw -- how far the
-- trailer has swung round the hitch -- and an unflattened dot product folds pitch into it, so a
-- trailer nose-up on a ramp, cresting a hump or sat on soft suspension reports several degrees
-- of phantom swing while perfectly in line. Same rule the scanner bearing and getImplementFrame
-- already follow, for the same reason.
--
-- A frame that collapses to numerical residue when flattened (a vehicle on its roof or pitched
-- near vertical) does NOT degrade gracefully: normalizing residue picks a direction per tick,
-- and since the sign of the answer is a dot product against that vector, the reported side
-- flips at random. So the last good value is held instead of guessed at, exactly as
-- implementProximity's getImplementFrame holds lastGoodFwd.
local function flatFwd(vehID, frame)
  local f = vec3(frame.f.x, frame.f.y, 0)
  if f:length() < 1e-3 then
    return lastGoodFwd[vehID]
  end
  f = f:normalized()
  lastGoodFwd[vehID] = f
  return f
end

-- Signed yaw from the player's heading to the trailer's, in degrees, POSITIVE = LEFT.
--
-- The sign comes from vec3(0,0,1):cross(fwd), the mod-wide positive-is-LEFT convention that
-- vehicleGeometry, implementProximity, terrainScanner and the scanner bearing all share and that
-- vehicle_geometry_sim.lua greps for. Note the vector points to the driver's LEFT despite every
-- variable in the mod calling it some form of "right"; renaming it here is exactly how the files
-- would stop agreeing.
local function trailerYawDeg(playerFrame, trailerFrame, playerID, trailerID)
  local pf = flatFwd(playerID, playerFrame)
  local tf = flatFwd(trailerID, trailerFrame)
  if not pf or not tf then return nil end

  local d = pf:dot(tf)
  if d > 1 then d = 1 elseif d < -1 then d = -1 end
  local mag = math.deg(math.acos(d))

  local left = vec3(0, 0, 1):cross(pf)
  local sign = (tf:dot(left) >= 0) and 1 or -1
  return mag * sign * TRAILER_ANGLE_SIGN
end

-- ================================================================================================
--  Tick
-- ================================================================================================

-- CLEAR is a real message, not the absence of one. "Nothing is coupled" and "the mod stopped
-- talking" are different facts and Python must not have to guess which it is looking at from a
-- timeout alone -- a timeout is the failure path, and a tone that stays on because the failure
-- path was the only way to turn it off is the worst outcome this feature has.
local function sendClear()
  if lastSentDeg ~= nil then
    send("TRAILER:CLEAR")
    lastSentDeg = nil
    lastSendT = 0
  end
end

local function tick()
  local player = be:getPlayerVehicle(0)
  if not player then sendClear() return end
  local playerID = player:getID()

  local pFrame = extensions.vehicleGeometry.boxFrame(playerID)
  if not pFrame then sendClear() return end

  local trailerID = findTrailer(playerID, pFrame)
  if not trailerID then sendClear() return end

  local tFrame = extensions.vehicleGeometry.boxFrame(trailerID)
  if not tFrame then sendClear() return end

  local deg = trailerYawDeg(pFrame, tFrame, playerID, trailerID)
  if not deg then sendClear() return end

  -- Round before the change test, not after.
  deg = math.floor(deg * 100 + 0.5) / 100

  local moved = (lastSentDeg == nil) or (math.abs(deg - lastSentDeg) >= SEND_EPSILON_DEG)
  local due = (sinceT - lastSendT) >= HEARTBEAT_S
  if not (moved or due) then return end

  local tveh = scenetree.findObjectById(trailerID)
  local tname = tveh and nameOf(tveh) or "trailer"
  send(string.format("TRAILER:%.2f,%d,%s", deg, trailerID, tname))
  lastSentDeg = deg
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
    taLog('E', "Failed to create UDP send socket.")
  end
end

function M.onExtensionLoaded()
  setExtensionUnloadMode(M, "manual")
  setupSockets()
  taLog('I', "Trailer angle tracker loaded.")
end

function M.onExtensionUnloaded()
  if udpSend then pcall(function() udpSend:close() end); udpSend = nil end
end

function M.onWorldReadyState(state)
  if state == 2 then
    lastSentDeg = nil
    lastSendT = 0
    tickAcc = 0
    lastGoodFwd = {}
    setupSockets()
  end
end

-- A held forward vector belongs to a particular body in a particular pose. A reset or a part
-- swap makes it a statement about a vehicle that no longer exists, and holding it would let a
-- stale heading sign the angle for the new one.
function M.onVehicleResetted(vehID)
  lastGoodFwd[vehID] = nil
end

function M.onVehicleDestroyed(vehID)
  lastGoodFwd[vehID] = nil
end

-- Climbing into a different vehicle changes what the question even means, so the latch is
-- cleared and the next tick re-states the answer from scratch -- including the CLEAR that says
-- the machine you just got into is not towing anything.
function M.onVehicleSwitched(oldId, newId, player)
  if player == nil or player == 0 then
    lastSentDeg = nil
    lastSendT = 0
  end
end

function M.onUpdate(dtReal, dtSim, dtRaw)
  -- The GE onUpdate chain is dispatched WITHOUT pcall, so an uncaught throw in here would
  -- silently stop every extension loaded after this one in modScript.lua.
  sinceT = sinceT + dtReal
  tickAcc = tickAcc + dtReal
  if tickAcc < 1.0 / TICK_HZ then return end
  tickAcc = 0
  local ok, err = pcall(tick)
  if not ok then taLog('E', "tick failed: " .. tostring(err)) end
end

-- This resolve's failure mode is a plausible wrong number -- an angle signed off the wrong body,
-- or measured against a partner that is towing YOU -- and no amount of listening identifies it.
-- Printing what it chose is the only thing that does. Same argument rampTruth and dockTruth make.
function M.diag()
  local out = {}
  local player = be:getPlayerVehicle(0)
  if not player then
    out[#out + 1] = "no player vehicle"
  else
    local playerID = player:getID()
    out[#out + 1] = string.format("player %d (%s)", playerID, nameOf(player))

    local viaRespawn = trailerFromRespawn(playerID)
    out[#out + 1] = "  core_trailerRespawn says: " .. tostring(viaRespawn or "nothing")
    local cands = partnersFromCouplers(playerID)
    if #cands == 0 then
      out[#out + 1] = "  core_vehicles.attachedCouplers says: nothing coupled"
    else
      out[#out + 1] = "  core_vehicles.attachedCouplers says: " .. table.concat(cands, ", ")
    end

    local pFrame = extensions.vehicleGeometry.boxFrame(playerID)
    if not pFrame then
      out[#out + 1] = "  no boxFrame for the player -- cannot measure"
    else
      local trailerID = findTrailer(playerID, pFrame)
      if not trailerID then
        out[#out + 1] = "  chosen trailer: none -- silent"
      else
        local tveh = scenetree.findObjectById(trailerID)
        local tFrame = extensions.vehicleGeometry.boxFrame(trailerID)
        out[#out + 1] = string.format("  chosen trailer: %d (%s)", trailerID,
          tveh and nameOf(tveh) or "?")
        if not tFrame then
          out[#out + 1] = "  no boxFrame for the trailer -- cannot measure"
        else
          local deg = trailerYawDeg(pFrame, tFrame, playerID, trailerID)
          if not deg then
            out[#out + 1] = "  both frames collapse when flattened -- holding last good"
          else
            out[#out + 1] = string.format("  angle %.2f deg (%s), behind by %.2f m",
              math.abs(deg), deg >= 0 and "LEFT" or "right",
              -((tFrame.c - pFrame.c):dot(pFrame.f)))
          end
        end
      end
    end
  end
  out[#out + 1] = "  last sent: " .. (lastSentDeg and string.format("%.2f deg", lastSentDeg)
    or "CLEAR")
  out[#out + 1] = string.format("  sign constant: %+.0f", TRAILER_ANGLE_SIGN)
  local text = table.concat(out, "\n")
  print(text)
  return text
end

return M
