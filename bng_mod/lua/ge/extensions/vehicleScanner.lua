-- =================================================================================================
--
--  Vehicle Scanner for BeamNG.drive Accessibility Mod (GE Extension)
--
--  Description: Scans for the nearest vehicle and sends bearing/distance data to the Python
--               backend via UDP as a plain "bearing,distance" text string.
--               Receives ON/OFF commands via UDP (no file I/O).
--
--  Loaded by: scripts/bng_screenreader_mod/modScript.lua
--
--  Version:     7.0 (GE extension with modScript loader, UDP commands)
--  Target Game: BeamNG.drive 0.37+
--
-- =================================================================================================

local M = {}

-- Configuration
local PYTHON_HOST         = "127.0.0.1"
local PYTHON_PORT_SCANNER = 4445   -- send scan data to Python on this port
local CMD_LISTEN_PORT     = 4448   -- receive ON/OFF commands from Python on this port
local SCAN_INTERVAL       = 0.1

-- Internal State
local udpSend           = nil
local udpCmd            = nil      -- command listener socket
local isScanModeActive  = false
local currentTargetID   = nil
local currentTargetDist = math.huge
local scanTimer         = 0
local lastPlayerID      = nil      -- for vehicle-switch detection

-- Which end of the player's machine is the business end. Reversing does not just mean "the
-- target is behind me": it means the rear bumper is what will hit something, so the contact
-- set has to move to the rear node band and the bearing has to be measured against the
-- direction of travel. On a loader it additionally means the bucket -- the FAR end -- must
-- stop being used as the origin, which is the case that was most wrong before.
--
-- Gear is pushed from Python, which already decodes it from the extended telemetry struct,
-- rather than polled cross-VM from electrics. Velocity is the fallback because gear alone
-- misses rolling backwards in neutral, and gear is preferred over velocity because at a
-- standstill about to reverse the readout should already be rear-referenced -- velocity
-- reads zero there and would answer "forwards".
local activeDirection   = 1        -- 1 = forward, -1 = reverse
local lastBearingVec    = nil      -- held while the origin is inside the target
local gearDirection     = nil      -- last direction Python told us, nil until it does
local gearStaleTimer    = 0
local GEAR_STALE_SEC    = 2.0      -- fall back to velocity if Python stops sending
local REVERSE_SPEED_MS  = 0.4      -- ignore creep and soft-body jitter around standstill

-- Alignment State
local alignPending      = false
local alignTimeout      = 0
local ALIGN_TIMEOUT_SEC = 3.0

-- Standoff for a fifth-wheel align. The gap is DERIVED from the two vehicles' bodies
-- (how far the truck extends behind its fifth wheel, plus how far the trailer extends
-- ahead of its king pin) and this is the clearance added on top.
--
-- There is no auto-coupling here, and that is the design. `beamstate.activateAutoCoupling()`
-- CANNOT couple a fifth wheel: the T-series' `fwk2` carries `couplerLock = true`, and
-- beamstate builds its visible tag set from `not couplerWeld and not couplerLock and
-- couplerTag`, so the fifth wheel is excluded and only the pintle is armed. The trailer
-- side is worse -- a stock tanker has ZERO couplerTag nodes (712 nodes, 0 matches); its
-- king pin carries only `tag = "fifthwheel_v2"`, read by the game's couplings/kingpin
-- controller. The real attach is done by couplings/fifthwheel.lua's updateGFX, which
-- needs the two vehicles physically COLLIDING and the king pin within
-- `couplerRadius * 0.8` = 0.08 m. So the mod parks you square and short, and the driver
-- reverses the last few metres under the coupler homing tone -- exactly as a ball hitch
-- has always worked.
local FW_ALIGN_CLEARANCE_M = 1.0

-- A body overhang is a max over every node, and a detached part stays in the vehicle's own
-- node cloud, so a wrecked truck could otherwise ask to be parked absurdly far back.
-- Over-estimating only parks you further away, so a loose ceiling is enough.
local ALIGN_OVERHANG_MAX_M = 20.0

-- How far the vehicle may end up from the position the align asked for before the readout
-- has to admit it. safeTeleport relocates rather than refusing when the requested spot
-- intersects something, and it moves SIDEWAYS as well as back -- which silently destroys
-- the very alignment this key exists to produce.
local ALIGN_DISPLACE_SAY_M = 0.5

-- The align teleports TWICE, and the second one is not a retry -- it is the placement.
--
-- `spawn.safeTeleport` does not put the reference node where you ask when the HEADING also
-- changes. It converts the requested reference position into a box centre using the vehicle's
-- CURRENT rotation, then re-derives the reference node from the box centre using the NEW one,
-- so the reference node lands off by `(I - diffRot) * (centre - ref)`. On a T-series that
-- offset is 0.658 m, and a 37 degree turn therefore misses by 0.40 m -- measured, against an
-- 0.08 m attach radius. Aligning from a truck already pointing the right way misses by
-- 0.001 m, which is why this hid: the obvious test starts from the pose the last align left.
-- Re-issuing once the heading already matches makes diffRot identity and the artifact
-- vanishes: measured 0.3985 m on the first pass, 0.0175 m on the second.
--
-- The wait exists because `getPosition()` does not reflect a teleport within the same frame
-- (setSafePosition defers the cluster move), so both the second placement and the landing
-- check have to happen on a later tick. Reading it immediately returns the PRE-teleport
-- position, which would report a displacement of however far the vehicle just travelled and
-- fire the warning on every single align.
local ALIGN_SETTLE_S = 0.15
local _alignSettle = nil

-- Coupler Tracking State
local couplerTrackActive  = false
local _playerCouplerCid   = nil
local _targetCouplerCid   = nil
local _couplerTargetID    = nil
local _pendingPlayerInfo  = nil   -- nil=waiting, {cid=N,tag=S,nx,ny,nz} or false (no coupler)
local _pendingTargetInfo  = nil   -- nil=waiting, {cid,tag,cx,cy,cz,ox,oy,oz} or false (o=target center)
local COUPLER_RANGE_M     = 1.5
local COUPLER_TRACK_HZ    = 20
local couplerTrackTimer   = 0

-- Coupler Distance Mode State (speech callouts every 2.5s, survives vehicle switches)
local couplerDistMode       = false
local couplerDistReady      = false
local couplerDistTimer      = 0
local COUPLER_DIST_INTERVAL = 2.5
local _cdPlayerCid          = nil
local _cdTargetCid          = nil
local _cdTargetID           = nil
local _cdPlayerID           = nil
local _cdDiscovering        = false
local _cdPendingPlayer      = nil   -- nil=not yet, false=no coupler, number=cid
local _cdPendingTarget      = nil
local _cdPlayerOverhang     = 0     -- how far truck body extends behind its coupler (meters)
local _cdTargetOverhang     = 0     -- how far trailer body extends past its coupler toward truck (meters)
local _cdEpoch              = 0     -- incremented each discovery; callbacks with wrong epoch are stale
local _cdDiscoveryTimer     = 0     -- time elapsed since _cdDiscovering was set true
local CD_DISCOVERY_TIMEOUT  = 3.0   -- seconds before retrying a stalled discovery

-- Coupler Attach Monitor State
-- Arms the onCouplerAttached GE hook to detect when the target vehicle physically couples.
-- Auto-starts when an ALIGN command is received; auto-stops on detection or scanner OFF.
local couplerAttachMonitor    = false

-- Coupler tag compatibility table
local COUPLER_COMPAT = {
  tow_hitch  = "tow_bar",
  tow_bar    = "tow_hitch",
  fifthwheel = "fifthwheel",
}

-- Logging helper
local function scannerLog(level, msg)
  log(level, 'VehicleScanner', msg)
end

local function normalizeSpeechValue(value, source)
  while type(value) == "table" do
    -- BeamNG's localized text shape: {txt = "<i18n key>", ctx = {...}}. Two entries, so the
    -- single-entry unwrap below would refuse it anyway -- but quietly, after logging a warning
    -- on every call. Translate it instead; core_locales handles both the plain and the
    -- context-substituted forms.
    if value.txt ~= nil and extensions and extensions.core_locales then
      local ok, translated = pcall(extensions.core_locales.translateWithOrWithoutContext, value)
      if ok and type(translated) == "string" and translated ~= "" then return translated end
      return nil
    end
    local count, only = 0, nil
    for _, item in pairs(value) do count = count + 1; only = item end
    local contents = "<unserializable>"
    pcall(function() contents = jsonEncode(value) end)
    pcall(function() scannerLog('warn', string.format("[LUA_TABLE_SPEECH] source=%s count=%d contents=%s", source, count, contents)) end)
    if count ~= 1 then return nil end
    value = only
  end
  local kind = type(value)
  if kind == "string" then
    if value:match("^table:%s*0x[%da-fA-F]+$") then
      scannerLog('warn', "[LUA_TABLE_SPEECH] source=" .. source .. " collapsed pointer; contents lost upstream")
      return nil
    end
    return value
  end
  if kind == "number" or kind == "boolean" then return tostring(value) end
  return nil
end

-- Human-readable name for a vehicle, resolved ENTIRELY ON THE GE SIDE.
--
-- This used to be a cross-VM round trip that read `v.data.information.name` in the vehicle
-- VM and sent the string back. BeamNG localized that field: it is no longer a string but a
-- `{txt = "<i18n key>", ctx = {brand1 = "<i18n key>"}}` structure, and the vehicle VM has no
-- translator at all (`_tr` is installed only by the GE-side core_locales, which is why
-- `lua/common/jbeam/io.lua` guards its own use of it with `and _tr`). So the round trip could
-- only ever come back with a two-entry table, `normalizeSpeechValue` rightly refused to speak
-- it, and every vehicle fell through to the JBeam basename -- "midsize" for a Pessima.
--
-- `vehicleNaming.describe` reads `core_vehicles.getVehicleList()`, where the game has already
-- run the model name through `_tr`, so it needs no translation of its own and no round trip.
-- The JBeam basename stays as the last-resort fallback, which is the answer the broken path
-- was accidentally giving for everything.
local function describeVehicle(veh, source)
  if not veh then return "unknown" end
  if extensions and extensions.vehicleNaming and extensions.vehicleNaming.describe then
    local ok, name = pcall(extensions.vehicleNaming.describe, veh)
    if ok then
      local normalized = normalizeSpeechValue(name, source)
      if normalized and normalized ~= "" then return normalized end
    end
  end
  local f = veh:getJBeamFilename() or "unknown"
  return f:match("([^/\\]+)%.jbeam$") or f
end

local function areCouplerTagsCompatible(tag1, tag2)
  if tag1 == "" or tag2 == "" then return false end
  local t1, t2 = tag1:lower(), tag2:lower()
  if t1 == t2 then return true end
  -- Exact match in compat table
  if COUPLER_COMPAT[t1] == t2 then return true end
  -- Pattern-based matching for tags with extra characters
  -- e.g. "tow_hitch_heavy" should still match with "tow_bar"
  local function classify(tag)
    if tag:find("fifthwheel") or tag:find("fifth_wheel") then return "fifthwheel" end
    if tag:find("tow_hitch") then return "tow_hitch" end
    if tag:find("tow_bar") then return "tow_bar" end
    return tag
  end
  local c1, c2 = classify(t1), classify(t2)
  if c1 == c2 then return true end
  return COUPLER_COMPAT[c1] == c2
end

local function _tryCouplerDistReady()
  if _cdPendingPlayer == nil or _cdPendingTarget == nil then return end
  _cdDiscovering = false
  if _cdPendingPlayer == false then
    if udpSend then udpSend:send("COUPLER_DIST_FAIL:No coupler on your vehicle") end
    return
  end
  if _cdPendingTarget == false then
    if udpSend then udpSend:send("COUPLER_DIST_FAIL:No coupler on target") end
    return
  end
  couplerDistReady = true
  couplerDistTimer = COUPLER_DIST_INTERVAL  -- fire immediately on next tick
  scannerLog('info', string.format(
    "Coupler distance mode ready. Player cid=%d rearOverhang=%.2fm, Target cid=%d frontOverhang=%.2fm",
    _cdPlayerCid, _cdPlayerOverhang, _cdTargetCid, _cdTargetOverhang))
end

local function _tryCompleteCouplerSetup()
  -- Wait until both callbacks have arrived
  if _pendingPlayerInfo == nil or _pendingTargetInfo == nil then return end
  alignPending = false
  alignTimeout = 0

  -- Check if both have couplers
  if _pendingPlayerInfo == false then
    if udpSend then udpSend:send("COUPLER_FAIL:No coupler found on your vehicle") end
    return
  end
  if _pendingTargetInfo == false then
    if udpSend then udpSend:send("COUPLER_FAIL:No coupler found on target vehicle") end
    return
  end

  -- Check compatibility
  local pTag = _pendingPlayerInfo.tag
  local tTag = _pendingTargetInfo.tag
  if not areCouplerTagsCompatible(pTag, tTag) then
    if udpSend then
      udpSend:send("COUPLER_FAIL:Incompatible couplers. Your vehicle: " .. pTag .. ", Target: " .. tTag)
    end
    return
  end

  -- Compatible! Do alignment teleport
  local player = be:getPlayerVehicle(0)
  if not player then
    if udpSend then udpSend:send("COUPLER_FAIL:No player vehicle") end
    return
  end

  local ti = _pendingTargetInfo
  local pi = _pendingPlayerInfo
  local couplerPos = vec3(ti.cx, ti.cy, ti.cz)
  local targetCenter = vec3(ti.ox, ti.oy, ti.oz)

  -- Classify coupling type
  local function classifyCoupler(tag)
    local t = tag:lower()
    if t:find("fifthwheel") or t:find("fifth_wheel") then return "fifthwheel" end
    if t:find("tow_hitch") then return "tow_hitch" end
    if t:find("tow_bar") then return "tow_bar" end
    return t
  end
  local isFifthWheel = classifyCoupler(pTag) == "fifthwheel" and classifyCoupler(tTag) == "fifthwheel"

  -- Compute "away from trailer" direction for alignment.
  local awayDir
  local target = scenetree.findObjectById(currentTargetID)

  if isFifthWheel and target then
    -- Fifth wheel: use the trailer's actual forward direction for a clean, centered axis.
    -- The reference-to-coupler vector can be skewed if the reference node is off-center.
    local tFwd = target:getDirectionVector()
    awayDir = vec3(tFwd.x, tFwd.y, 0)
    -- Ensure it points from trailer body toward king pin (same side as coupler)
    local refToCoupler = couplerPos - targetCenter
    refToCoupler.z = 0
    if awayDir:dot(refToCoupler) < 0 then
      awayDir = -awayDir
    end
  else
    -- Ball hitch / fallback: use reference-to-coupler direction
    awayDir = (couplerPos - targetCenter)
    awayDir.z = 0
    local awayLen = awayDir:length()
    if awayLen < 0.5 and target then
      awayDir = vec3(target:getDirectionVector())
      awayDir.z = 0
    end
  end
  awayDir = awayDir:normalized()

  local alignPos, alignDist

  if isFifthWheel then
    -- Fifth wheel: park the truck SHORT of the king pin by a gap wide enough that the two
    -- bodies do not overlap, and let the driver reverse the rest.
    --
    -- This used to ask for a ZERO gap -- fifth wheel exactly on the king pin. That places
    -- the truck's body inside the trailer's, so spawn.safeTeleport's collision search
    -- (placeVehicle -> placeVehRec) relocated the vehicle instead of using the requested
    -- spot. Measured on a T-series and a stock tanker: shoved 4.43 m back AND 1.26 m
    -- sideways, leaving a 4.725 m gap to a coupler with an 0.08 m attach radius, and
    -- destroying the squareness that is the whole point of the key. The align was asking
    -- for a position the game was always going to refuse.
    --
    -- The gap is derived rather than fixed because it is a property of the PAIR: the
    -- truck's tail reaches back past its fifth wheel and the trailer's nose reaches
    -- forward past its king pin, and both vary by vehicle. Measured for this pair:
    -- 1.680 + 0.866, i.e. the boxes touch at 2.546 m -- a fixed 1.5 m gap of the sort the
    -- ball-hitch path uses would still have overlapped.
    local rearOverhang  = math.min(pi.rearOverhang or 0, ALIGN_OVERHANG_MAX_M)
    local frontOverhang = math.min(ti.frontOverhang or 0, ALIGN_OVERHANG_MAX_M)
    alignDist = math.abs(pi.ny or 0) + rearOverhang + frontOverhang + FW_ALIGN_CLEARANCE_M
    alignPos = couplerPos + awayDir * alignDist
  else
    -- Ball hitch: leave a 1.5m gap so the player reverses to couple up.
    local couplerForwardOffset = math.abs(pi.ny or 0)
    local COUPLER_GAP_M = 1.5
    alignDist = math.max(couplerForwardOffset + COUPLER_GAP_M, 5.0)
    alignPos = couplerPos + awayDir * alignDist
  end

  -- Put the COUPLER on the target's axis, not the reference node.
  --
  -- The two are not the same point on a real vehicle: the T-series' fifth wheel sits
  -- 0.434 m off its own reference node laterally, so placing the reference node on the
  -- king pin axis left the fifth wheel a third of a metre to one side of a coupler with an
  -- 0.08 m attach radius. `pi.nlat` is the coupler's offset along the vehicle's own left
  -- vector (up:cross(fwd), the mod-wide positive-is-LEFT convention); subtracting it along
  -- the ALIGNED left vector cancels it, because after the teleport the vehicle is facing
  -- awayDir. Same correction RAMPALIGN already carries, applied to a different point --
  -- there it is the body centre, here it is the coupler itself.
  local alignLeft = vec3(0, 0, 1):cross(awayDir)
  alignPos = alignPos - alignLeft * (pi.nlat or 0)

  -- Use the player's current Z for ground level — the player truck is already on the ground.
  -- Don't use the coupler Z, which may be elevated (e.g. trailer deck height).
  alignPos.z = player:getPosition().z + 0.3

  scannerLog('info', string.format(
    "Align: couplerPos=(%.1f,%.1f,%.1f) targetCenter=(%.1f,%.1f,%.1f) alignDist=%.2f finalPos=(%.1f,%.1f,%.1f) fifthWheel=%s localNy=%.2f lat=%.2f rearOverhang=%.2f frontOverhang=%.2f",
    couplerPos.x, couplerPos.y, couplerPos.z,
    targetCenter.x, targetCenter.y, targetCenter.z,
    alignDist,
    alignPos.x, alignPos.y, alignPos.z,
    tostring(isFifthWheel), (pi.ny or 0), (pi.nlat or 0),
    (pi.rearOverhang or 0), (ti.frontOverhang or 0)))

  local rot = quatFromDir(awayDir, vec3(0, 0, 1))
  -- 4th/5th params are checkOnlyStatics and visibilityPoint. visibilityPoint must
  -- be a vec3: spawn.lua feeds it to getVisibilityStatus, which does
  -- `randPoint - intendedPos`, so a boolean there throws inside LuaVec3.__sub
  -- and the alignment teleport never happened. So those two stay nil.
  --
  -- The 8th argument is resetVehicle, and it MUST be false. It defaults to true, which
  -- makes spawn.lua call setPosRot + resetBrokenFlexMesh and re-place the vehicle from
  -- its INITIAL node positions -- i.e. a full respawn: every dent repaired, and the
  -- engine stopped for anyone who has "reset stops the engine" set in gameplay options.
  -- An align is a placement, never a repair. With it false the cluster move, the
  -- velocity zeroing and setOriginalTransform all still run against the DEFORMED body,
  -- so physics still settles at the new spot.
  spawn.safeTeleport(player, alignPos, rot, nil, nil, nil, false, false)

  -- Hand off to the settle state machine: it re-issues the placement now that the heading is
  -- right, then checks where the vehicle actually ended up before announcing anything. See
  -- ALIGN_SETTLE_S for why both halves have to wait a tick.
  _alignSettle = {
    phase    = 1,
    timer    = 0,
    pos      = alignPos,
    rot      = rot,
    pTag     = pTag,
    tTag     = tTag,
    gap      = alignDist - math.abs(pi.ny or 0),
    pCid     = _pendingPlayerInfo.cid,
    tCid     = _pendingTargetInfo.cid,
    targetID = currentTargetID,
  }
end

-- =================================================================================================
--  Target Cycling
-- =================================================================================================

local function cycleTarget(direction)  -- direction: 1 = next, -1 = prev
  if not udpSend then return end
  local player = be:getPlayerVehicle(0)
  if not player then return end

  local playerPos = player:getPosition()
  local playerID  = player:getID()

  -- Build distance-sorted list of non-player vehicles
  local vehicles = {}
  for i = 0, be:getObjectCount() - 1 do
    local obj = be:getObject(i)
    if obj and obj:getID() ~= playerID then
      table.insert(vehicles, { id = obj:getID(), dist = playerPos:distance(obj:getPosition()) })
    end
  end
  if #vehicles == 0 then
    udpSend:send("TARGET_NAME:No other vehicles")
    return
  end
  table.sort(vehicles, function(a, b) return a.dist < b.dist end)

  -- Find current target's index (0 = not in list)
  local currentIdx = 0
  for i, v in ipairs(vehicles) do
    if v.id == currentTargetID then currentIdx = i; break end
  end

  -- Advance/retreat with wrap; if no current target, start at first or last
  local newIdx
  if currentIdx == 0 then
    newIdx = direction == 1 and 1 or #vehicles
  else
    newIdx = ((currentIdx - 1 + direction + #vehicles) % #vehicles) + 1
  end

  local entry = vehicles[newIdx]
  local newVeh = scenetree.findObjectById(entry.id)
  if not newVeh then return end

  currentTargetID   = entry.id
  currentTargetDist = entry.dist
  scannerLog('info', "Target cycled to vehicle ID " .. entry.id)

  udpSend:send("TARGET_NAME:" .. describeVehicle(newVeh, "cycleTarget.vehicleNaming.describe"))
end

-- Lock onto the non-player vehicle closest to the player. Used by F9+CTRL+Tab.
local function targetClosest()
  if not udpSend then return end
  local player = be:getPlayerVehicle(0)
  if not player then
    udpSend:send("TARGET_NAME:No player vehicle")
    return
  end

  local playerPos = player:getPosition()
  local playerID  = player:getID()

  local closestID, closestDist = nil, math.huge
  for i = 0, be:getObjectCount() - 1 do
    local obj = be:getObject(i)
    if obj and obj:getID() ~= playerID then
      local d = playerPos:distance(obj:getPosition())
      if d < closestDist then
        closestDist = d
        closestID   = obj:getID()
      end
    end
  end

  if not closestID then
    udpSend:send("TARGET_NAME:No other vehicles")
    return
  end

  currentTargetID   = closestID
  currentTargetDist = closestDist
  scannerLog('info', string.format("Closest target locked: id=%d dist=%.1fm", closestID, closestDist))

  local newVeh = scenetree.findObjectById(closestID)
  udpSend:send("TARGET_NAME:" .. describeVehicle(newVeh, "targetClosest.vehicleNaming.describe"))
end

-- =================================================================================================
--  Core Scan Logic
-- =================================================================================================

-- Resolve which end is live. Gear wins while it is fresh; velocity covers the gaps (rolling
-- backwards in neutral, or Python not running yet).
local function resolveDirection(player, forwardVec)
  if gearDirection and gearStaleTimer < GEAR_STALE_SEC then
    return gearDirection
  end
  local ok, along = pcall(function()
    return vec3(player:getVelocity()):dot(forwardVec)
  end)
  if ok and along then
    if along < -REVERSE_SPEED_MS then return -1 end
    if along > REVERSE_SPEED_MS then return 1 end
  end
  return activeDirection  -- below the threshold, hold whatever we had rather than flapping
end

local function scanAndSendVehicleData()
  if not udpSend then return end

  local player = be:getPlayerVehicle(0)
  if not player then return end

  local playerPos        = player:getPosition()
  local playerForwardVec = player:getDirectionVector()
  local playerUpVec      = player:getDirectionVectorUp()
  local playerID         = player:getID()

  -- Auto-select the nearest non-player object only when no target is locked.
  -- Once a target is locked (by auto-select or manual cycling), never override it.
  if not currentTargetID then
    local closestVehID   = nil
    local closestVehDist = math.huge
    for i = 0, be:getObjectCount() - 1 do
      local obj = be:getObject(i)
      if obj and obj:getID() ~= playerID then
        local dist = playerPos:distance(obj:getPosition())
        if dist < closestVehDist then
          closestVehDist = dist
          closestVehID   = obj:getID()
        end
      end
    end
    if not closestVehID then return end
    currentTargetID   = closestVehID
    currentTargetDist = closestVehDist
    -- Name the auto-selected target. GE-side; see describeVehicle.
    local autoVeh = scenetree.findObjectById(closestVehID)
    if autoVeh and udpSend then
      udpSend:send("TARGET_NAME:" .. describeVehicle(autoVeh, "autoTarget.vehicleNaming.describe"))
    end
  end

  -- Look up the locked target; clear lock if it has disappeared
  local targetVehicle = scenetree.findObjectById(currentTargetID)
  if not targetVehicle then
    currentTargetID = nil
    return
  end

  activeDirection = resolveDirection(player, playerForwardVec)

  -- Aim from the implement when there is one. An articulated loader has two frames, and
  -- player:getDirectionVector() describes the REAR one (cab, reference nodes) while the
  -- bucket is on the front one. Measuring from the rear frame makes lining the bucket up
  -- close to impossible: bend the frame toward a target and the rear initially swings the
  -- other way as the machine pivots, so the bearing OPENS while the bucket is closing on
  -- it. Returns nil on anything that is not such a machine, which is every ordinary vehicle.
  --
  -- The override is deliberately scoped to FORWARD only. Phrasing the rule as "the implement
  -- supplies the forward contact set" rather than "loaders use the implement" is what makes
  -- reversing a loader fall out for free: it drops to the rear band like any other vehicle,
  -- with no loader-specific code. Backing up measured from the bucket was the worst case in
  -- the old build -- the whole machine length of error, in the wrong direction.
  local originPos, forwardVec = playerPos, playerForwardVec
  local refUpVec = playerUpVec
  local contactPts = nil
  if activeDirection > 0 and extensions and extensions.implementProximity then
    local ip = extensions.implementProximity
    if ip.getImplementFrame then
      local okFrame, implFrame = pcall(ip.getImplementFrame)
      if okFrame and implFrame then
        originPos, forwardVec = implFrame.pos, implFrame.fwd
        -- The implement heading is flattened by construction, so pair it with WORLD up.
        -- Using the chassis up here would tilt the derived left vector on a machine that is
        -- pitched or rolled, against a forward vector that is not.
        refUpVec = vec3(0, 0, 1)
      end
    end
    if ip.getImplementPoints then
      local okPts, pts = pcall(ip.getImplementPoints)
      if okPts and pts then contactPts = pts end
    end
  end

  -- With no implement the contact set is the player's own front or rear node band -- the
  -- same question answered for an ordinary car. This is the whole point of the
  -- vehicleGeometry split: the loader is a different cid list, not a different code path.
  local geo = extensions and extensions.vehicleGeometry or nil
  if not contactPts and geo then
    local okC, pts = pcall(geo.contactPoints, playerID, activeDirection)
    if okC and pts then
      contactPts = pts
      -- Bearing is measured from the contact set's own centroid, so backing toward
      -- something reports the gap and the angle from the bumper that will reach it.
      local acc = vec3(0, 0, 0)
      for _, p in ipairs(pts) do acc = acc + p end
      originPos = acc / #pts
    end
  end

  -- Reversing flips the reference heading so a target dead behind reads ~0 degrees rather
  -- than ~180. playerLeftVec below is built from the UN-negated forward vector on purpose:
  -- it is up x fwd, so negating fwd would silently negate the cross product too and swap
  -- left for right. Reversing does not move the driver's physical left side. This exact
  -- sign has already produced one false bug report; the geometry sim asserts it.
  -- Snapshot the heading BEFORE the reverse negation. This is the reference the left/right
  -- sign is derived from, and it has to satisfy two things at once that were previously in
  -- conflict:
  --
  --   * it must not be negated, or reverse swaps left and right (up x fwd negates with fwd),
  --   * it must be the SAME FRAME the angle magnitude is measured in.
  --
  -- The second one is what was broken. The magnitude came from the implement heading while
  -- the sign came from the chassis heading, and on an articulated machine those differ by
  -- the articulation angle -- up to 40 degrees on this one. For any target lying between the
  -- bucket's left-plane and the cab's left-plane, the output was |angle from bucket| with
  -- sign(cab side), so as the frame flexed the reading snapped +20 to -20 without ever
  -- passing through zero. On a rigid vehicle the two frames are identical and the bug is
  -- invisible, which is exactly why it only ever showed up on the loader.
  local refForwardVec = forwardVec
  if activeDirection < 0 then
    local f = vec3(forwardVec)
    forwardVec = vec3(-f.x, -f.y, -f.z)
  end

  -- Bearing aims at the target's box CENTRE while range measures to its nearest SURFACE.
  -- That split looks inconsistent and is deliberate: a bearing to the nearest surface point
  -- swings wildly at close range as the winning point hops from corner to corner, and
  -- bearing is the number you steer with.
  local targetPos = targetVehicle:getPosition()
  if geo then
    local okC, c = pcall(geo.boxCentre, currentTargetID)
    if okC and c then targetPos = c end
  end

  -- Range is the GAP, not the centre-to-centre distance. getPosition() is the object's
  -- reference node, so the old measurement carried half a vehicle of phantom range at each
  -- end and -- worse -- changed with orientation, reporting the same number broadside and
  -- nose-on to the same car. Every tier of nearestApproach falls back cleanly, ending at
  -- exactly the old behaviour, so a vehicle whose geometry never resolves still answers.
  local gap = nil
  if geo and contactPts then
    local okG, d = pcall(geo.nearestApproach, contactPts, currentTargetID)
    if okG and d then gap = d end
  end
  -- Clamp: once the implement is inside the target's hull the nearest approach is still
  -- positive, but the box tier can round through zero, and Python formats this with %.0f.
  currentTargetDist = math.max(0, gap or originPos:distance(targetPos))
  -- Bearing is a HORIZONTAL quantity, so flatten the direction to the target. The implement
  -- heading is already flat, and leaving this one in 3-D meant raising the boom -- three
  -- metres of vertical travel -- shrank the dot product and inflated the reported angle with
  -- no yaw change whatsoever. Lifting the bucket appeared to steer the machine.
  local toTargetVec    = targetPos - originPos
  toTargetVec.z        = 0
  if toTargetVec:length() < 1e-3 then
    -- Directly over or inside the target: the direction is soft-body noise at this point.
    -- Hold the last bearing rather than emitting a random one.
    toTargetVec = lastBearingVec or vec3(1, 0, 0)
  end
  toTargetVec          = toTargetVec:normalized()
  lastBearingVec       = toTargetVec

  local flatFwd = vec3(forwardVec); flatFwd.z = 0
  if flatFwd:length() < 1e-3 then flatFwd = vec3(forwardVec) else flatFwd = flatFwd:normalized() end
  local cosAngle       = flatFwd:dot(toTargetVec)
  local angleRadians   = math.acos(math.max(-1, math.min(1, cosAngle)))
  -- refForwardVec, never the possibly-negated forwardVec and never a different frame's: see
  -- the note above the negation.
  local playerLeftVec  = refUpVec:cross(refForwardVec)
  local dot            = playerLeftVec:dot(toTargetVec)
  local bearingDegrees = math.deg(angleRadians) * (dot < 0 and -1 or 1)

  -- Approach angle from target's frame: which face of the target is the player nearest to?
  -- 0 = player is in front of target, ±180 = player is behind target,
  -- positive = player is to target's LEFT, negative = player is to target's right
  local targetFwdVec   = targetVehicle:getDirectionVector()
  local targetUpVec    = targetVehicle:getDirectionVectorUp()
  local toPlayerVec    = (originPos - targetPos):normalized()
  local cosApproach    = targetFwdVec:dot(toPlayerVec)
  local approachRad    = math.acos(math.max(-1, math.min(1, cosApproach)))
  local targetLeftVec  = targetUpVec:cross(targetFwdVec) -- left, as above
  local approachDeg    = math.deg(approachRad) * (targetLeftVec:dot(toPlayerVec) < 0 and -1 or 1)

  -- Send as plain text "bearing,distance,approachDeg,direction" — parsed by Python with
  -- split(','). The direction rides along because the bearing is measured against the
  -- DIRECTION OF TRAVEL, and Python has to know which end 0 deg refers to before it can turn
  -- the number into speech. It used to mirror the GEAR:R/GEAR:F it had pushed, which is only
  -- one of the three ways activeDirection gets set here: resolveDirection also ages that push
  -- out after GEAR_STALE_SEC and falls back to velocity, and a vehicle switch clears it
  -- outright. Rolling backwards down a slope in D, this side re-referenced to the rear while
  -- Python still believed forward, and speech said "in front of you" about the thing being
  -- reversed into. Sending the resolved value leaves exactly one authority for it.
  local packet = string.format("%.4f,%.4f,%.4f,%d",
    bearingDegrees, currentTargetDist, approachDeg, activeDirection)
  udpSend:send(packet)
end

-- =================================================================================================
--  GE Extension Hooks (exported via M table)
-- =================================================================================================

local function setupSockets()
  if udpSend then pcall(function() udpSend:close() end); udpSend = nil end
  if udpCmd  then pcall(function() udpCmd:close()  end); udpCmd  = nil end

  udpSend = socket.udp()
  if udpSend then
    udpSend:setpeername(PYTHON_HOST, PYTHON_PORT_SCANNER)
    udpSend:settimeout(0)
    scannerLog('info', "UDP send socket created, targeting " .. PYTHON_HOST .. ":" .. PYTHON_PORT_SCANNER)
  else
    scannerLog('error', "Failed to create UDP send socket.")
  end

  local ok, err = pcall(function()
    udpCmd = socket.udp()
    -- setsockname RETURNS nil plus a message; it does not THROW. A pcall around it reports
    -- success on a socket bound to nothing, and the extension then goes deaf with nothing in
    -- the log -- it still sends normally, because a UDP sender needs no bind.
    local bound, berr = udpCmd:setsockname("127.0.0.1", CMD_LISTEN_PORT)
    if not bound then error(tostring(berr), 0) end
    udpCmd:settimeout(0)
  end)
  if ok and udpCmd then
    scannerLog('info', "UDP command socket listening on port " .. CMD_LISTEN_PORT)
  else
    scannerLog('error', "Failed to create UDP command socket: " .. tostring(err))
    if udpCmd then pcall(function() udpCmd:close() end) end
    udpCmd = nil
  end
end

-- A failed bind is otherwise permanent for the session, so re-arm it. This is the recovery
-- path, not a precaution, and it has been watched doing the job: the first reload of the
-- patched files leaked eight ports, because the OUTGOING code had no unload hook yet. The
-- retry could not take them while the old module tables were still referenced -- a socket held
-- that way is not one the collector is about to free -- and ticked uselessly for two minutes.
-- The Ctrl+L that followed did NOT re-load these extensions (no load line for any of them in
-- the log at that timestamp, so setupSockets never ran again); all thirteen ports came back
-- through THIS function instead, within one frame of each other, the moment those tables went
-- away. Without it the mod would have stayed deaf until the game was restarted.
local CMD_BIND_RETRY_S = 3.0
local cmdBindRetry = 0

local function retryCmdBind(dtReal)
  if udpCmd then return end
  cmdBindRetry = cmdBindRetry + (dtReal or 0)
  if cmdBindRetry < CMD_BIND_RETRY_S then return end
  cmdBindRetry = 0
  local ok = pcall(function()
    local sk = socket.udp()
    local bound = sk:setsockname("127.0.0.1", CMD_LISTEN_PORT)
    if not bound then sk:close(); error("still in use", 0) end
    sk:settimeout(0)
    udpCmd = sk
  end)
  if ok and udpCmd then
    scannerLog('info', "UDP command socket bound on port " .. CMD_LISTEN_PORT .. " after retry.")
  end
end

-- setupSockets closes the sockets held by THIS module instance, and extensions.reload builds a
-- fresh instance whose locals are nil -- so it closes nothing and the outgoing instance keeps
-- the port, leaving the reloaded copy permanently deaf. Hence this hook.
function M.onExtensionUnloaded()
  if udpSend then pcall(function() udpSend:close() end); udpSend = nil end
  if udpCmd  then pcall(function() udpCmd:close()  end); udpCmd  = nil end
end

function M.onExtensionLoaded()
  setExtensionUnloadMode(M, "manual")
  scannerLog('info', "Vehicle scanner extension loaded.")
  -- Bind sockets here so Ctrl+L Lua reload re-opens them.
  setupSockets()
end

function M.onWorldReadyState(state)
  scannerLog('info', "onWorldReadyState triggered with state: " .. tostring(state))

  if state == 2 then
    scannerLog('info', "World is ready. Initializing scanner systems.")

    -- Reset state for new map
    isScanModeActive  = false
    currentTargetID   = nil
    currentTargetDist = math.huge
    lastPlayerID      = nil
    activeDirection   = 1
    gearDirection     = nil
    gearStaleTimer    = 0
    alignPending      = false
    _alignSettle      = nil
    couplerTrackActive = false
    _playerCouplerCid  = nil
    _targetCouplerCid  = nil
    _couplerTargetID   = nil
    _pendingPlayerInfo = nil
    _pendingTargetInfo = nil
    couplerDistMode    = false
    couplerDistReady   = false
    _cdDiscovering     = false
    _cdPlayerCid       = nil
    _cdTargetCid       = nil
    _cdTargetID        = nil
    _cdPlayerID        = nil
    _cdPendingPlayer   = nil
    _cdPendingTarget   = nil
    _cdPlayerOverhang  = 0
    _cdTargetOverhang  = 0
    _cdEpoch             = 0
    _cdDiscoveryTimer    = 0
    couplerAttachMonitor = false

    setupSockets()
  end
end

function M.onUpdate(dtReal, dtSim, dtRaw)
  retryCmdBind(dtReal)
  -- Age the pushed gear. If Python stops sending -- it exited, or this is an older build --
  -- direction resolution falls back to velocity rather than freezing on the last gear seen.
  gearStaleTimer = gearStaleTimer + dtReal

  -- 1. Poll UDP for ON/OFF commands from Python
  if udpCmd then
    local data = udpCmd:receive()
    if data then
      local cmd = data:match("^%s*(.-)%s*$"):upper()
      -- Gear is pushed on change from the Python side, which already decodes it out of the
      -- extended telemetry struct. Handled ahead of the chain because it arrives whether or
      -- not scan mode is on: the direction has to be current the moment the scanner is
      -- switched on, not one gear change later.
      local gearArg = cmd:match("^GEAR:(%a*)$")
      if gearArg then
        gearDirection = (gearArg == "R") and -1 or 1
        gearStaleTimer = 0
      elseif cmd == "ON" and not isScanModeActive then
        isScanModeActive  = true
        currentTargetID   = nil
        currentTargetDist = math.huge
        scannerLog('info', "Scan mode activated via UDP.")
      elseif cmd == "OFF" and isScanModeActive then
        isScanModeActive = false
        -- couplerTrackActive, couplerAttachMonitor and _alignSettle deliberately SURVIVE.
        -- Switching the scanner off silences its periodic callouts; it is not a request to
        -- abandon a coupling you are halfway through. See section 2d.
        scannerLog('info', "Scan mode deactivated via UDP.")
      elseif cmd == "NEXT" then
        cycleTarget(1)
      elseif cmd == "PREV" then
        cycleTarget(-1)
      elseif cmd == "CLOSEST" then
        targetClosest()
      elseif cmd == "DAMAGE" then
        scannerLog('info', "DAMAGE command received, routing to active vehicle.")
        local player = be:getPlayerVehicle(0)
        if not player then
          -- No vehicle — send DONE immediately so Python doesn't hang
          local tmpSock = socket.udp()
          if tmpSock then
            tmpSock:setpeername("127.0.0.1", 4447)
            tmpSock:send("NONE")
            tmpSock:send("DONE")
            tmpSock:close()
          end
        else
          -- The vehicle name is resolved GE-side (see describeVehicle) and injected into the
          -- chunk; the vehicle VM cannot translate the localized name field. speechValue stays
          -- because the detached-part naming below still reads jbeam part information.
          local vehName = describeVehicle(player, "damage.vehicleNaming.describe")
          player:queueLuaCommand(string.format([[
            local _ds = require("socket").udp()
            _ds:setpeername("127.0.0.1", 4447)
            local function speechValue(value, source)
              while type(value) == "table" do
                if value.txt ~= nil then return nil end -- localized {txt=,ctx=}; untranslatable here
                local count, only = 0, nil
                for _, item in pairs(value) do count = count + 1; only = item end
                local contents = "<unserializable>"; pcall(function() contents = jsonEncode(value) end)
                pcall(function() log('W', 'vehicleScanner', '[LUA_TABLE_SPEECH] source=' .. source .. ' count=' .. count .. ' contents=' .. contents) end)
                if count ~= 1 then return nil end
                value = only
              end
              local kind = type(value)
              if kind == "string" or kind == "number" or kind == "boolean" then return tostring(value) end
              return nil
            end
            _ds:send("NAME:" .. %q)
            local found = false
            local ok, err = pcall(function()
              local bodyParts = {"FL", "FR", "ML", "MR", "RL", "RR"}
              local bodyLabels = {FL="Front left", FR="Front right", ML="Middle left", MR="Middle right", RL="Rear left", RR="Rear right"}
              for _, part in ipairs(bodyParts) do
                local val = damageTracker.getDamage("body", part)
                if type(val) == "number" and val > 0.01 then
                  local severity = val > 0.5 and "heavy" or (val > 0.2 and "moderate" or "light")
                  _ds:send(bodyLabels[part] .. " body " .. severity .. " damage")
                  found = true
                end
              end
              local engineChecks = {
                {"starvedOfOil","Oil starvation"},{"coolantOverheating","Coolant overheating"},
                {"oilOverheating","Oil overheating"},{"pistonRingsDamaged","Piston rings damaged"},
                {"rodBearingsDamaged","Rod bearings damaged"},{"headGasketDamaged","Head gasket damaged"},
                {"turbochargerHot","Turbocharger overheating"},{"turbochargerDamaged","Turbocharger damaged"},
                {"superchargerDamaged","Supercharger damaged"},{"inductionSystemDamaged","Induction system damaged"},
                {"engineHydrolocked","Engine hydrolocked"},{"engineDisabled","Engine disabled"},
                {"blockMelted","Engine block melted"},{"cylinderWallsMelted","Cylinder walls melted"},
                {"engineLockedUp","Engine locked up"},{"radiatorLeak","Radiator leaking"},
                {"oilpanLeak","Oil pan leaking"},{"oilRadiatorLeak","Oil radiator leaking"},
                {"oilLevelCritical","Oil level critical"},{"engineReducedTorque","Engine torque reduced"},
              }
              for _, pair in ipairs(engineChecks) do
                local val = damageTracker.getDamage("engine", pair[1])
                if val and val ~= false and val ~= 0 then
                  _ds:send(pair[2]); found = true
                end
              end
              local corners = {"FL", "FR", "RL", "RR"}
              local cornerLabels = {FL="Front left", FR="Front right", RL="Rear left", RR="Rear right"}
              for _, c in ipairs(corners) do
                local label = cornerLabels[c]
                if damageTracker.getDamage("wheels", "tire" .. c) == true then
                  _ds:send(label .. " tire burst"); found = true
                end
                if damageTracker.getDamage("wheels", c) == true then
                  _ds:send(label .. " wheel broken"); found = true
                end
                if damageTracker.getDamage("wheels", "brake" .. c) == true then
                  _ds:send(label .. " brake damaged"); found = true
                end
                local brakeHeat = damageTracker.getDamage("wheels", "brakeOverHeat" .. c)
                if type(brakeHeat) == "number" and brakeHeat > 0 then
                  _ds:send(label .. " brake fading"); found = true
                end
              end
              local ptChecks = {
                {"wheelaxleFL","Front left axle broken"},{"wheelaxleFR","Front right axle broken"},
                {"wheelaxleRL","Rear left axle broken"},{"wheelaxleRR","Rear right axle broken"},
                {"driveshaft","Driveshaft broken"},{"driveshaft_F","Front driveshaft broken"},
                {"mainEngine","Engine broken"},
              }
              for _, pair in ipairs(ptChecks) do
                local val = damageTracker.getDamage("powertrain", pair[1])
                if val and val ~= false then
                  _ds:send(pair[2]); found = true
                end
              end
              local fuelVal = damageTracker.getDamage("energyStorage", "mainTank")
              if fuelVal and fuelVal ~= false then
                _ds:send("Fuel tank damaged"); found = true
              end
              if damageTracker.getDamage("gearbox", "synchroWear") == true then
                _ds:send("Synchro wear"); found = true
              end
              -- Detect detached parts via break groups
              local bgMap = {}
              if v.data.beams then
                for _, b in pairs(v.data.beams) do
                  if b.breakGroup then
                    local groups = type(b.breakGroup) == "table" and b.breakGroup or {b.breakGroup}
                    for _, g in ipairs(groups) do
                      if not bgMap[g] then
                        bgMap[g] = {cids = {}, partPath = b.partPath}
                      end
                      table.insert(bgMap[g].cids, b.cid)
                    end
                  end
                end
              end
              local detachedParts = {}
              for gName, gData in pairs(bgMap) do
                local allBroken = #gData.cids > 0
                for _, cid in ipairs(gData.cids) do
                  if not obj:beamIsBroken(cid) then
                    allBroken = false
                    break
                  end
                end
                if allBroken and gData.partPath then
                  detachedParts[gData.partPath] = true
                end
              end
              -- Friendly name from partPath: take last segment, strip vehicle prefix, map directions
              local dirSuffixes = {{"_FL"," front left"},{"_FR"," front right"},{"_RL"," rear left"},{"_RR"," rear right"},{"_F"," front"},{"_L"," left"},{"_R"," right"}}
              local function friendlyName(pp)
                local seg = pp:match("([^/]+)$") or pp
                -- Strip vehicle prefix (first word before underscore)
                local stripped = seg:match("^%%w-_(.+)$")
                if stripped and #stripped > 0 then seg = stripped end
                -- Map direction suffix
                for _, d in ipairs(dirSuffixes) do
                  if seg:sub(-#d[1]) == d[1] then
                    seg = seg:sub(1, -#d[1]-1) .. d[2]
                    break
                  end
                end
                seg = seg:gsub("_", " ")
                return seg:sub(1,1):upper() .. seg:sub(2)
              end
              local apd = v.data.activePartsData
              for partPath, _ in pairs(detachedParts) do
                local name = nil
                if apd and apd[partPath] and apd[partPath].information and apd[partPath].information.name then
                  name = speechValue(apd[partPath].information.name, "damage.part.information.name")
                end
                if not name or name == "" then
                  name = friendlyName(partPath)
                end
                _ds:send(name .. " detached")
                found = true
              end
            end)
            if not ok then _ds:send("(error: " .. tostring(err) .. ")"); found = true end
            if not found then _ds:send("NONE") end
            _ds:send("DONE")
            _ds:close()
          ]], vehName))
        end
      elseif cmd == "DUMP" then
        scannerLog('info', "DUMP command received, routing to active vehicle.")
        local player = be:getPlayerVehicle(0)
        if not player then
          local tmpSock = socket.udp()
          if tmpSock then
            tmpSock:setpeername("127.0.0.1", 4447)
            tmpSock:send("NONE")
            tmpSock:send("DONE")
            tmpSock:close()
          end
        else
          player:queueLuaCommand([[
            local _ds = require("socket").udp()
            _ds:setpeername("127.0.0.1", 4447)
            local lines = {}
            local ok, err = pcall(function()
              for k, v in pairs(electrics.values) do
                lines[#lines + 1] = tostring(k) .. "=" .. tostring(v)
              end
            end)
            if not ok then lines[#lines + 1] = "(error: " .. tostring(err) .. ")" end
            table.sort(lines)
            for _, line in ipairs(lines) do
              _ds:send(line)
            end
            _ds:send("DONE")
            _ds:close()
          ]])
        end
      elseif cmd == "PDUMP" then
        scannerLog('info', "PDUMP command received, routing to active vehicle.")
        local player = be:getPlayerVehicle(0)
        if not player then
          local tmpSock = socket.udp()
          if tmpSock then
            tmpSock:setpeername("127.0.0.1", 4447)
            tmpSock:send("NONE")
            tmpSock:send("DONE")
            tmpSock:close()
          end
        else
          player:queueLuaCommand([[
            local _ds = require("socket").udp()
            _ds:setpeername("127.0.0.1", 4447)
            local lines = {}
            local ok, err = pcall(function()
              for k, v in pairs(powertrain) do
                local vt = type(v)
                if vt == "number" or vt == "string" or vt == "boolean" then
                  lines[#lines + 1] = tostring(k) .. "=" .. tostring(v)
                elseif vt == "table" then
                  for sk, sv in pairs(v) do
                    local svt = type(sv)
                    if svt == "number" or svt == "string" or svt == "boolean" then
                      lines[#lines + 1] = tostring(k) .. "." .. tostring(sk) .. "=" .. tostring(sv)
                    end
                  end
                end
              end
              if type(powertrain.getDevice) == "function" then
                local candidates = {
                  "engine", "gearbox", "transmission", "driveshaft",
                  "frontDifferential", "rearDifferential", "differential",
                  "clutch", "torqueConverter",
                  "steerR", "steerL", "steerF",
                  "hydraulicPump", "mainPump",
                  "boomLiftLeft", "boomLiftRight", "boomTilt",
                  "articulationJoint", "articulation",
                }
                for _, devName in ipairs(candidates) do
                  local dok, dev = pcall(function() return powertrain.getDevice(devName) end)
                  if dok and dev and type(dev) == "table" then
                    for sk, sv in pairs(dev) do
                      local svt = type(sv)
                      if svt == "number" or svt == "string" or svt == "boolean" then
                        lines[#lines + 1] = devName .. "." .. tostring(sk) .. "=" .. tostring(sv)
                      end
                    end
                  end
                end
              end
            end)
            if not ok then lines[#lines + 1] = "(error: " .. tostring(err) .. ")" end
            table.sort(lines)
            for _, line in ipairs(lines) do
              _ds:send(line)
            end
            _ds:send("DONE")
            _ds:close()
          ]])
        end
      elseif cmd == "HDUMP" then
        scannerLog('info', "HDUMP command received, routing to active vehicle.")
        local player = be:getPlayerVehicle(0)
        if not player then
          local tmpSock = socket.udp()
          if tmpSock then
            tmpSock:setpeername("127.0.0.1", 4447)
            tmpSock:send("(no vehicle spawned)")
            tmpSock:send("DONE")
            tmpSock:close()
          end
        else
          player:queueLuaCommand([[
            local _ds = require("socket").udp()
            _ds:setpeername("127.0.0.1", 4447)
            local lines = {}
            local ok, err = pcall(function()
              if hydros then
                lines[#lines + 1] = "--- hydros top-level keys ---"
                for k, v in pairs(hydros) do
                  lines[#lines + 1] = "TYPE:hydros." .. tostring(k) .. "=" .. type(v)
                end
                if hydros.values then
                  lines[#lines + 1] = "--- hydros.values keys ---"
                  local count = 0
                  for k, v in pairs(hydros.values) do
                    count = count + 1
                    local vt = type(v)
                    lines[#lines + 1] = "TYPE:values." .. tostring(k) .. "=" .. vt
                    if vt == "number" or vt == "string" or vt == "boolean" then
                      lines[#lines + 1] = "values." .. tostring(k) .. "=" .. tostring(v)
                    elseif vt == "table" then
                      for sk, sv in pairs(v) do
                        local svt = type(sv)
                        if svt == "number" or svt == "string" or svt == "boolean" then
                          lines[#lines + 1] = "values." .. tostring(k) .. "." .. tostring(sk) .. "=" .. tostring(sv)
                        else
                          lines[#lines + 1] = "TYPE:values." .. tostring(k) .. "." .. tostring(sk) .. "=" .. svt
                        end
                      end
                    end
                  end
                  lines[#lines + 1] = "values._count=" .. count
                else
                  lines[#lines + 1] = "(hydros.values is nil)"
                end
                if type(hydros.hydros) == "table" then
                  lines[#lines + 1] = "--- hydros.hydros entries ---"
                  for k, v in pairs(hydros.hydros) do
                    local prefix = "hydros." .. tostring(k)
                    local vt = type(v)
                    if vt == "number" or vt == "string" or vt == "boolean" then
                      lines[#lines + 1] = prefix .. "=" .. tostring(v)
                    elseif vt == "table" then
                      for sk, sv in pairs(v) do
                        local svt = type(sv)
                        if svt == "number" or svt == "string" or svt == "boolean" then
                          lines[#lines + 1] = prefix .. "." .. tostring(sk) .. "=" .. tostring(sv)
                        else
                          lines[#lines + 1] = "TYPE:" .. prefix .. "." .. tostring(sk) .. "=" .. svt
                        end
                      end
                    end
                  end
                end
                for k, v in pairs(hydros) do
                  if k ~= "values" and k ~= "hydros" then
                    local vt = type(v)
                    if vt == "number" or vt == "string" or vt == "boolean" then
                      lines[#lines + 1] = tostring(k) .. "=" .. tostring(v)
                    end
                  end
                end
              else
                lines[#lines + 1] = "(hydros module not present on this vehicle)"
              end
            end)
            if not ok then lines[#lines + 1] = "(error: " .. tostring(err) .. ")" end
            table.sort(lines)
            for _, line in ipairs(lines) do
              _ds:send(line)
            end
            _ds:send("DONE")
            _ds:close()
          ]])
        end
      elseif cmd == "ATTACH_MONITOR" then
        couplerAttachMonitor = not couplerAttachMonitor
        scannerLog('info', "Coupler attach monitor explicitly toggled " .. (couplerAttachMonitor and "ON" or "OFF"))
        if udpSend then
          udpSend:send("ATTACH_MONITOR:" .. (couplerAttachMonitor and "ON" or "OFF"))
        end
      elseif cmd == "ALIGN" then
        -- Auto-enable attach monitor whenever alignment is started
        couplerAttachMonitor = true
        scannerLog('info', "ALIGN command received; attach monitor armed.")
        if not currentTargetID then
          udpSend:send("COUPLER_FAIL:No vehicle target locked")
        else
          local targetVeh = scenetree.findObjectById(currentTargetID)
          if not targetVeh then
            udpSend:send("COUPLER_FAIL:Target vehicle not found")
          else
            local player = be:getPlayerVehicle(0)
            if not player then
              udpSend:send("COUPLER_FAIL:No player vehicle")
            else
              alignPending = true
              alignTimeout = 0
              _pendingPlayerInfo = nil
              _pendingTargetInfo = nil
              local tid = currentTargetID
              -- Query player vehicle for coupler node
              player:queueLuaCommand([[
                local best, btag, bpri = nil, "", 0
                -- ROLE beats tag, and the game declares the role itself.
                --
                -- `fifthwheel_v2` is a coupling STANDARD, not a role: the plate and the pin
                -- both carry it. A log_trailer has both -- its own king pin at the front
                -- (fwdOffset +7.193) and a fifth wheel of its own at the back (-3.075) so it
                -- can pull a second trailer -- and the name ladder below scores them
                -- identically, leaving `pairs()` order to decide. It picked the rear plate,
                -- so the align aimed the truck at a point 10.3 m behind the real king pin
                -- and reversed it into the trailer. A tanker worked only by luck: it has no
                -- couplerTag node at all, so its king pin won by default.
                --
                -- couplings/fifthwheel owns the TOWING end and couplings/kingpin the TOWED
                -- end, each naming its node in jbeam. That is a capability check on the
                -- thing that actually does the coupling -- the same shape of argument
                -- rampGeometry.isCannon() makes -- and it needs no allowlist of trailers.
                local roleName, roleTag = nil, nil
                for _, cc in pairs(v.data.controller or {}) do
                  local fn = tostring(cc.fileName or "")
                  if fn:find("couplings/fifthwheel", 1, true) and cc.fifthwheelNode then
                    roleName = cc.fifthwheelNode
                    roleTag = cc.fifthwheelKey or "fifthwheel_v2"
                  end
                end
                for _, nd in pairs(v.data.nodes) do
                  local p, t = 0, ""
                  if nd.couplerTag then
                    local cl = nd.couplerTag:lower()
                    p = 1; t = nd.couplerTag
                    if cl == "tow_bar" then p = 2 end
                    if cl:find("fifthwheel") or cl:find("tow_hitch") then p = 3 end
                  end
                  if p == 0 and nd.tag and type(nd.tag) == "string" then
                    local tl = nd.tag:lower()
                    if tl:find("fifthwheel") or tl:find("fifth_wheel") or tl:find("tow_hitch") then
                      p = 3; t = nd.couplerTag or nd.tag
                    elseif tl:find("tow_bar") then
                      p = 2; t = nd.couplerTag or nd.tag
                    end
                  end
                  if p == 0 and nd.couplerStrength and nd.couplerStrength > 0 then
                    p = 1; t = nd.couplerTag or nd.tag or "coupler"
                  end
                  if p > bpri then best = nd; btag = t; bpri = p end
                  if roleName and nd.name == roleName then best = nd; btag = roleTag; bpri = 4 end
                end
                if best then
                  -- Compute local forward offset by projecting node position onto vehicle forward.
                  -- getNodePosition returns world-relative offsets, so we project onto the vehicle's
                  -- actual forward direction to get the orientation-independent forward distance.
                  local np = vec3(obj:getNodePosition(best.cid))
                  -- BODY frame, not a ground frame: full 3-D forward and the vehicle's OWN
                  -- up, giving an orthonormal basis that rotates with the vehicle. These are
                  -- "where is the coupler on this truck", which must not change when the
                  -- truck rocks on its springs.
                  --
                  -- A flattened forward paired with world up is a GROUND frame, and it is
                  -- roll-sensitive: the fifth-wheel plate sits about 1.1 m above the
                  -- reference node, so ten degrees of roll swings it 0.19 m sideways and the
                  -- align cancels a lateral offset the settled truck does not have. Measured
                  -- while the truck was still rocking after a teleport: 0.224 m off axis,
                  -- against 0.001 m from a settled one. up:cross(fwd) is also the mod-wide
                  -- positive-is-LEFT convention (vehicleGeometry, implementProximity,
                  -- rampGeometry).
                  --
                  -- The align cancels along vec3(0,0,1):cross(awayDir), a WORLD frame -- and
                  -- the two agree because the teleport puts the truck level and facing
                  -- awayDir, which is exactly the pose in which body and ground frames
                  -- coincide.
                  local fwd = vec3(obj:getDirectionVector())
                  if fwd:length() > 0.01 then fwd = fwd:normalized() end
                  local up = vec3(obj:getDirectionVectorUp())
                  if up:length() > 0.01 then up = up:normalized() end
                  local left = up:cross(fwd)
                  if left:length() > 0.01 then left = left:normalized() end
                  local forwardOffset = np:dot(fwd)
                  -- The old code sent the raw world-relative np.x here, which is a lateral
                  -- offset only on an axis-aligned vehicle -- and nothing ever read it, so
                  -- the align put the reference node on the target's axis instead of the
                  -- coupler.
                  local lateralOffset = np:dot(left)
                  -- Rear overhang: how far the body reaches BEHIND the coupler. This is what
                  -- decides whether the requested align position overlaps the target, and so
                  -- whether safeTeleport will relocate us. Same sweep the COUPLER_DIST
                  -- discovery already does.
                  local rearOverhang = 0
                  for _, nd2 in pairs(v.data.nodes) do
                    local behindDist = -fwd:dot(vec3(obj:getNodePosition(nd2.cid)) - np)
                    if behindDist > rearOverhang then rearOverhang = behindDist end
                  end
                  -- Measured along the same body forward, so it is the truck's reach behind
                  -- its own plate rather than a figure that shrinks when the nose lifts.
                  obj:queueGameEngineLua(string.format(
                    'extensions.vehicleScanner.onPlayerCouplerInfo(%d, %q, %.4f, %.4f, %.4f, %.4f)',
                    best.cid, btag, lateralOffset, forwardOffset, np.z, rearOverhang))
                else
                  obj:queueGameEngineLua('extensions.vehicleScanner.onPlayerCouplerInfo(-1, "", 0, 0, 0, 0)')
                end
              ]])
              -- Query target vehicle for coupler node (with position/direction for alignment)
              targetVeh:queueLuaCommand(string.format([[
                local best, btag, bpri = nil, "", 0
                -- ROLE beats tag, and the game declares the role itself.
                --
                -- `fifthwheel_v2` is a coupling STANDARD, not a role: the plate and the pin
                -- both carry it. A log_trailer has both -- its own king pin at the front
                -- (fwdOffset +7.193) and a fifth wheel of its own at the back (-3.075) so it
                -- can pull a second trailer -- and the name ladder below scores them
                -- identically, leaving `pairs()` order to decide. It picked the rear plate,
                -- so the align aimed the truck at a point 10.3 m behind the real king pin
                -- and reversed it into the trailer. A tanker worked only by luck: it has no
                -- couplerTag node at all, so its king pin won by default.
                --
                -- couplings/fifthwheel owns the TOWING end and couplings/kingpin the TOWED
                -- end, each naming its node in jbeam. That is a capability check on the
                -- thing that actually does the coupling -- the same shape of argument
                -- rampGeometry.isCannon() makes -- and it needs no allowlist of trailers.
                local roleName, roleTag = nil, nil
                for _, cc in pairs(v.data.controller or {}) do
                  local fn = tostring(cc.fileName or "")
                  if fn:find("couplings/kingpin", 1, true) and cc.kingpinNode then
                    roleName = cc.kingpinNode
                    roleTag = cc.kingpinKey or "fifthwheel_v2"
                  end
                end
                for _, nd in pairs(v.data.nodes) do
                  local p, t = 0, ""
                  if nd.couplerTag then
                    local cl = nd.couplerTag:lower()
                    p = 1; t = nd.couplerTag
                    -- TOWED end wins here, the mirror of the player chunk. A tow_bar is the
                    -- drawbar you hook up TO; a tow_hitch is a hitch for towing something
                    -- else. Both ladders used to rank tow_hitch above tow_bar, which is
                    -- right for the truck and backwards for the trailer: on any trailer
                    -- carrying a rear hitch as well as its own drawbar it picked the hitch,
                    -- the same role confusion the log trailer hit with fifthwheel_v2, just
                    -- spelled with two different tags instead of one shared one.
                    if cl:find("tow_hitch") then p = 2 end
                    if cl:find("fifthwheel") or cl == "tow_bar" then p = 3 end
                  end
                  if p == 0 and nd.tag and type(nd.tag) == "string" then
                    local tl = nd.tag:lower()
                    if tl:find("fifthwheel") or tl:find("fifth_wheel") or tl:find("tow_bar") then
                      p = 3; t = nd.couplerTag or nd.tag
                    elseif tl:find("tow_hitch") then
                      p = 2; t = nd.couplerTag or nd.tag
                    end
                  end
                  if p == 0 and nd.couplerStrength and nd.couplerStrength > 0 then
                    p = 1; t = nd.couplerTag or nd.tag or "coupler"
                  end
                  if p > bpri then best = nd; btag = t; bpri = p end
                  if roleName and nd.name == roleName then best = nd; btag = roleTag; bpri = 4 end
                end
                if best then
                  local np = vec3(obj:getNodePosition(best.cid))
                  local op = vec3(obj:getPosition())
                  local wp = op + np
                  -- Front overhang: how far this vehicle's body reaches PAST its coupler,
                  -- toward whoever is coupling to it. On a tanker that is the nose ahead of
                  -- the king pin, and it is the other half of the clearance the align needs.
                  --
                  -- Measured along the vehicle's own flattened FORWARD, flipped to point at
                  -- the coupler -- which is exactly how the align derives awayDir, so the
                  -- overhang is measured along the same axis the standoff is applied along.
                  -- The COUPLER_DIST discovery uses the raw origin-to-coupler vector here
                  -- instead, which is skewed on any trailer whose king pin sits off the
                  -- centreline; that is tolerable for a spoken gap figure and is not
                  -- tolerable for a teleport.
                  local couplerDir = vec3(obj:getDirectionVector())
                  if couplerDir:length() > 0.01 then couplerDir = couplerDir:normalized() end
                  if couplerDir:dot(np) < 0 then couplerDir = -couplerDir end
                  local frontOverhang = 0
                  for _, nd2 in pairs(v.data.nodes) do
                    local aheadDist = couplerDir:dot(vec3(obj:getNodePosition(nd2.cid)) - np)
                    if aheadDist > frontOverhang then frontOverhang = aheadDist end
                  end
                  obj:queueGameEngineLua(string.format(
                    "extensions.vehicleScanner.onTargetCouplerForAlign(%d, %%d, %%.4f, %%.4f, %%.4f, %%.4f, %%.4f, %%.4f, '%%s', %%.4f)",
                    best.cid, wp.x, wp.y, wp.z, op.x, op.y, op.z, btag, frontOverhang
                  ))
                else
                  obj:queueGameEngineLua("extensions.vehicleScanner.onTargetCouplerForAlign(%d, -1, 0, 0, 0, 0, 0, 0, '', 0)")
                end
              ]], tid, tid))
            end
          end
        end
      elseif cmd == "COUPLER_DIST" then
        couplerDistMode = not couplerDistMode
        if not couplerDistMode then
          couplerDistReady = false
          _cdDiscovering = false
          _cdPlayerCid = nil      -- cleared so re-discovery fires unconditionally when toggled back on
          _cdPendingPlayer = nil
          _cdPendingTarget = nil
        end
        scannerLog('info', "Coupler distance mode " .. (couplerDistMode and "ON" or "OFF"))
        if udpSend then
          udpSend:send("COUPLER_DIST_MODE:" .. (couplerDistMode and "ON" or "OFF"))
        end
      end
    end
  end

  -- 2. Alignment timeout guard
  if alignPending then
    alignTimeout = alignTimeout + dtReal
    if alignTimeout >= ALIGN_TIMEOUT_SEC then
      alignPending = false
      alignTimeout = 0
      if udpSend then
        udpSend:send("COUPLER_FAIL:Alignment timed out")
      end
      scannerLog('warn', "Alignment callback timed out.")
    end
  end

  -- 2b. Align settle: place, re-place, then verify before announcing.
  if _alignSettle then
    _alignSettle.timer = _alignSettle.timer + dtReal
    if _alignSettle.timer >= ALIGN_SETTLE_S then
      _alignSettle.timer = 0
      local st = _alignSettle
      local player = be:getPlayerVehicle(0)
      local target = st.targetID and scenetree.findObjectById(st.targetID) or nil
      if not player or not target then
        _alignSettle = nil
        if udpSend then udpSend:send("COUPLER_FAIL:Lost the vehicle during alignment") end
      elseif st.phase == 1 then
        -- The heading is right now, so this pass lands where it is asked to. See
        -- ALIGN_SETTLE_S: this is the placement, not a retry.
        spawn.safeTeleport(player, st.pos, st.rot, nil, nil, nil, false, false)
        st.phase = 2
      else
        _alignSettle = nil
        local landed = player:getPosition()
        local shifted = (vec3(landed.x, landed.y, 0) - vec3(st.pos.x, st.pos.y, 0)):length()
        if shifted > ALIGN_DISPLACE_SAY_M then
          scannerLog('warn', string.format(
            "Align: vehicle ended up %.2f m from the requested position "
            .. "(requested %.2f,%.2f landed %.2f,%.2f) -- the spot was not clear.",
            shifted, st.pos.x, st.pos.y, landed.x, landed.y))
        end

        _playerCouplerCid = st.pCid
        _targetCouplerCid = st.tCid
        _couplerTargetID  = st.targetID
        couplerTrackActive = true
        couplerTrackTimer = 0

        if udpSend then
          -- Fields 3 and 4 are an optional positional tail: the gap the driver has to
          -- reverse, and how far the placement was displaced. Python guards on length.
          udpSend:send(string.format("COUPLER_START:%s,%s,%.2f,%.2f",
            st.pTag, st.tTag, st.gap, shifted))
        end
        scannerLog('info', string.format(
          "Coupler tracking started. Player: %s Target: %s gap=%.2f m shifted=%.2f m",
          st.pTag, st.tTag, st.gap, shifted))
      end
    end
  end

  -- 2c. Detect player vehicle switch by polling
  local player = be:getPlayerVehicle(0)
  local playerID = player and player:getID() or nil
  if playerID ~= lastPlayerID then
    lastPlayerID      = playerID
    currentTargetID   = nil
    currentTargetDist = math.huge
    -- The gear we were told belonged to the machine we just left. Drop back to forward and
    -- let velocity carry it until Python reports the new vehicle's gear, rather than
    -- measuring off the rear bumper because the last vehicle happened to be reversing.
    activeDirection   = 1
    gearDirection     = nil
    -- A pending placement belongs to the machine you just climbed out of.
    _alignSettle      = nil
    if couplerTrackActive then
      couplerTrackActive = false
      if udpSend then udpSend:send("COUPLER_LOST") end
    end
    -- Coupler distance mode survives switch but needs re-discovery
    if couplerDistMode then
      couplerDistReady = false
      _cdDiscovering = false
      _cdPlayerCid = nil      -- cleared so re-discovery fires even if switching back to original vehicles
      _cdPendingPlayer = nil
      _cdPendingTarget = nil
    end
    scannerLog('info', "Player vehicle changed; target lock reset.")
    if udpSend and player then
      -- Use the GE-side vehicleNaming helper for a rich identifier
      -- (brand, friendly model, configuration, color) without an async
      -- cross-VM round trip. Fall back to JBeam basename on any failure.
      local display = nil
      if extensions and extensions.vehicleNaming and extensions.vehicleNaming.describe then
        local ok, name = pcall(extensions.vehicleNaming.describe, player)
        if ok then display = normalizeSpeechValue(name, "vehicleSwitch.vehicleNaming.describe") end
      end
      if not display then
        local f = player:getJBeamFilename() or "unknown"
        display = f:match("([^/\\]+)%.jbeam$") or f
      end
      udpSend:send("SWITCHED:" .. display)
    end
  end

  -- 2d. Coupler tracking (send bearing/distance between coupler nodes).
  --
  -- Deliberately ABOVE the scan-mode gate. Once you have aligned, the homing tone is the
  -- instrument you are steering by, and the natural reason to switch the scanner off
  -- mid-manoeuvre is to stop its periodic "169 feet, behind you" callouts -- i.e. exactly
  -- when the tone matters most. Below the gate, that toggle silently ended the run: no
  -- tone for the last few metres, and no "Coupled" afterwards, because the OFF handler
  -- cleared the attach monitor too and switching the scanner back on re-armed neither.
  -- Observed on the first real coupling: aligned, scanner toggled 4 s later, coupled 19 s
  -- after that in silence. The run now ends only when it is actually over -- coupled,
  -- target lost, or a vehicle switch.
  if couplerTrackActive then
    couplerTrackTimer = couplerTrackTimer + dtReal
    if couplerTrackTimer >= (1.0 / COUPLER_TRACK_HZ) then
      couplerTrackTimer = 0
      local player = be:getPlayerVehicle(0)
      local target = scenetree.findObjectById(_couplerTargetID)
      if player and target and _playerCouplerCid and _targetCouplerCid then
        local pPos = vec3(player:getPosition()) + vec3(player:getNodePosition(_playerCouplerCid))
        local tPos = vec3(target:getPosition()) + vec3(target:getNodePosition(_targetCouplerCid))
        local dist = pPos:distance(tPos)

        -- Bearing from player's REAR direction to target coupler
        -- 0° = directly behind = aligned for reversing
        --
        -- Both vectors are FLATTENED before the angle is taken. This is a steering error,
        -- and a coupler sits about a metre off the ground on both vehicles, so any height
        -- difference between the two plates was being reported as horizontal error --
        -- worst exactly where it matters least tolerably: at 0.3 m separation a 0.15 m
        -- height difference is 27 degrees of phantom steering command, with a sign taken
        -- from an almost-vertical vector, i.e. arbitrary and flipping per tick. Same rule
        -- the scanner bearing already follows for the implement boom ("`toTargetVec` is
        -- also flattened, or three metres of boom travel ... appears to steer the machine").
        local playerFwd = player:getDirectionVector()
        local playerUp  = player:getDirectionVectorUp()
        local rearDir   = vec3(-playerFwd.x, -playerFwd.y, 0)
        if rearDir:length() > 0.01 then rearDir = rearDir:normalized() end
        local toTargetRaw = tPos - pPos
        local toTarget  = vec3(toTargetRaw.x, toTargetRaw.y, 0)
        if toTarget:length() > 0.01 then toTarget = toTarget:normalized() end
        local cosAngle  = math.max(-1, math.min(1, rearDir:dot(toTarget)))
        local angleRad  = math.acos(cosAngle)
        -- leftVec keeps the UN-negated forward: the driver's physical left does not move
        -- when they select reverse, and positive stays LEFT in every gear.
        local leftVec   = playerUp:cross(playerFwd)
        local dot       = leftVec:dot(toTarget)
        local bearing   = math.deg(angleRad) * (dot < 0 and -1 or 1)

        local inRange = dist <= COUPLER_RANGE_M and 1 or 0
        udpSend:send(string.format("COUPLER:%.4f,%.4f,%d", bearing, dist, inRange))
      else
        couplerTrackActive = false
        if udpSend then udpSend:send("COUPLER_LOST") end
        scannerLog('warn', "Coupler tracking stopped: vehicle lost.")
      end
    end
  end

  if not isScanModeActive then return end

  -- 3. Rate-limit scans to SCAN_INTERVAL seconds
  scanTimer = scanTimer + dtReal
  if scanTimer >= SCAN_INTERVAL then
    scanTimer = 0
    scanAndSendVehicleData()
  end

  -- 5. Coupler Distance Mode (periodic speech callouts)
  if couplerDistMode and isScanModeActive and currentTargetID then
    local plyr = be:getPlayerVehicle(0)
    local pID = plyr and plyr:getID() or nil

    -- (Re)discover couplers when player or target changes
    if not couplerDistReady and not _cdDiscovering and plyr then
      if _cdTargetID ~= currentTargetID or _cdPlayerID ~= pID or (_cdPlayerCid == nil) then
        _cdEpoch = _cdEpoch + 1
        local epoch = _cdEpoch
        _cdDiscovering = true
        _cdDiscoveryTimer = 0
        _cdPendingPlayer = nil
        _cdPendingTarget = nil
        _cdPlayerID = pID
        _cdTargetID = currentTargetID
        couplerDistReady = false

        -- Query player vehicle for best coupler node CID + rear overhang
        plyr:queueLuaCommand(string.format([[
          local best, bpri = nil, 0
          for _, nd in pairs(v.data.nodes) do
            local p = 0
            if nd.couplerTag then
              local cl = nd.couplerTag:lower()
              p = 1
              if cl == "tow_bar" then p = 2 end
              if cl:find("fifthwheel") or cl:find("tow_hitch") then p = 3 end
            end
            if p == 0 and nd.tag and type(nd.tag) == "string" then
              local tl = nd.tag:lower()
              if tl:find("fifthwheel") or tl:find("fifth_wheel") or tl:find("tow_hitch") then
                p = 3
              elseif tl:find("tow_bar") then
                p = 2
              end
            end
            if p == 0 and nd.couplerStrength and nd.couplerStrength > 0 then
              p = 1
            end
            if p > bpri then best = nd; bpri = p end
          end
          if best then
            -- Compute rear overhang: how far the vehicle body extends behind the coupler
            local cPos = vec3(obj:getNodePosition(best.cid))
            local fwd = vec3(obj:getDirectionVector())
            fwd.z = 0
            fwd = fwd:normalized()
            local rearOverhang = 0
            for _, nd in pairs(v.data.nodes) do
              local nPos = vec3(obj:getNodePosition(nd.cid))
              local diff = nPos - cPos
              local behindDist = -fwd:dot(diff)
              if behindDist > rearOverhang then rearOverhang = behindDist end
            end
            obj:queueGameEngineLua(string.format(
              'extensions.vehicleScanner.onCouplerDistPlayerInfo(%%d, %d, %%.4f)', best.cid, rearOverhang))
          else
            obj:queueGameEngineLua('extensions.vehicleScanner.onCouplerDistPlayerInfo(-1, %d, 0)')
          end
        ]], epoch, epoch))

        -- Query target vehicle for best coupler node CID + front overhang
        local targetVeh = scenetree.findObjectById(currentTargetID)
        if targetVeh then
          targetVeh:queueLuaCommand(string.format([[
            local best, bpri = nil, 0
            for _, nd in pairs(v.data.nodes) do
              local p = 0
              if nd.couplerTag then
                local cl = nd.couplerTag:lower()
                p = 1
                if cl == "tow_bar" then p = 2 end
                if cl:find("fifthwheel") or cl:find("tow_hitch") then p = 3 end
              end
              if p == 0 and nd.tag and type(nd.tag) == "string" then
                local tl = nd.tag:lower()
                if tl:find("fifthwheel") or tl:find("fifth_wheel") or tl:find("tow_hitch") then
                  p = 3
                elseif tl:find("tow_bar") then
                  p = 2
                end
              end
              if p == 0 and nd.couplerStrength and nd.couplerStrength > 0 then
                p = 1
              end
              if p > bpri then best = nd; bpri = p end
            end
            if best then
              -- Compute front overhang: how far the vehicle body extends past the coupler
              -- toward the coupling direction (from vehicle center toward coupler)
              local cPos = vec3(obj:getNodePosition(best.cid))
              local couplerDir = vec3(cPos.x, cPos.y, 0)
              local cdLen = couplerDir:length()
              if cdLen > 0.3 then
                couplerDir = couplerDir:normalized()
              else
                -- Coupler near center; use vehicle forward as fallback
                local fwd = vec3(obj:getDirectionVector())
                couplerDir = vec3(fwd.x, fwd.y, 0):normalized()
              end
              local frontOverhang = 0
              for _, nd in pairs(v.data.nodes) do
                local nPos = vec3(obj:getNodePosition(nd.cid))
                local diff = nPos - cPos
                local aheadDist = couplerDir:dot(diff)
                if aheadDist > frontOverhang then frontOverhang = aheadDist end
              end
              obj:queueGameEngineLua(string.format(
                'extensions.vehicleScanner.onCouplerDistTargetInfo(%%d, %d, %%.4f)', best.cid, frontOverhang))
            else
              obj:queueGameEngineLua('extensions.vehicleScanner.onCouplerDistTargetInfo(-1, %d, 0)')
            end
          ]], epoch, epoch))
        else
          _cdPendingTarget = false
          _tryCouplerDistReady()
        end
      end
    end

    -- Discovery timeout: retry if vehicle VM callbacks never arrived
    if _cdDiscovering then
      _cdDiscoveryTimer = _cdDiscoveryTimer + dtReal
      if _cdDiscoveryTimer >= CD_DISCOVERY_TIMEOUT then
        scannerLog('warn', "Coupler distance discovery timed out, will retry")
        _cdDiscovering = false
        _cdDiscoveryTimer = 0
        _cdPendingPlayer = nil
        _cdPendingTarget = nil
      end
    end

    -- Send distance/height data at interval when ready
    if couplerDistReady then
      couplerDistTimer = couplerDistTimer + dtReal
      if couplerDistTimer >= COUPLER_DIST_INTERVAL then
        couplerDistTimer = 0
        local p2 = be:getPlayerVehicle(0)
        local t2 = scenetree.findObjectById(_cdTargetID)
        if p2 and t2 and _cdPlayerCid and _cdTargetCid then
          local pPos = vec3(p2:getPosition()) + vec3(p2:getNodePosition(_cdPlayerCid))
          local tPos = vec3(t2:getPosition()) + vec3(t2:getNodePosition(_cdTargetCid))
          local dx = tPos.x - pPos.x
          local dy = tPos.y - pPos.y
          local horizDist = math.sqrt(dx * dx + dy * dy)
          local heightDiff = tPos.z - pPos.z
          -- Subtract body overhangs to report the physical gap, not coupler-to-coupler
          local gap = math.max(0, horizDist - _cdPlayerOverhang - _cdTargetOverhang)
          if udpSend then
            udpSend:send(string.format("COUPLER_DIST:%.3f,%.3f", gap, heightDiff))
          end
        else
          -- Lost a vehicle, need re-discovery next tick
          couplerDistReady = false
          _cdDiscovering = false
        end
      end
    end
  end

end

-- =================================================================================================
--  Coupler / Alignment Callbacks (called from vehicle VM via queueGameEngineLua)
-- =================================================================================================

-- Called from player vehicle VM with coupler info + local node position.
-- `nlat` replaced the old `nx`: that field was the raw world-axis x of the node offset,
-- which is a lateral offset only on an axis-aligned vehicle, and nothing ever read it.
-- `rearOverhang` is how far the body reaches behind the coupler, and is half of the
-- standoff the align needs to avoid asking safeTeleport for an occupied space.
-- Both are optional so a mod half older than this file still aligns, just without the
-- lateral correction and with the old zero-gap standoff.
function M.onPlayerCouplerInfo(cid, tag, nlat, ny, nz, rearOverhang)
  scannerLog('info', "Player coupler info: cid=" .. tostring(cid) .. " tag=" .. tostring(tag)
    .. " lat=" .. tostring(nlat) .. " fwd=" .. tostring(ny) .. " z=" .. tostring(nz)
    .. " rearOverhang=" .. tostring(rearOverhang))
  if cid == -1 then
    _pendingPlayerInfo = false
  else
    _pendingPlayerInfo = {cid = cid, tag = tag, nlat = nlat or 0, ny = ny or 0, nz = nz or 0,
                          rearOverhang = rearOverhang or 0}
  end
  _tryCompleteCouplerSetup()
end

-- Called from target vehicle VM with coupler info + coupler world pos + target center pos.
-- `frontOverhang` (optional, see onPlayerCouplerInfo) is how far the target's body reaches
-- past its own coupler toward us -- the other half of the align standoff.
function M.onTargetCouplerForAlign(tid, cid, cx, cy, cz, ox, oy, oz, tag, frontOverhang)
  scannerLog('info', "Target coupler info: tid=" .. tid .. " cid=" .. tostring(cid)
    .. " tag=" .. tostring(tag) .. " frontOverhang=" .. tostring(frontOverhang))
  if cid == -1 then
    _pendingTargetInfo = false
  else
    _pendingTargetInfo = {cid = cid, tag = tag, cx = cx, cy = cy, cz = cz, ox = ox, oy = oy, oz = oz,
                          frontOverhang = frontOverhang or 0}
  end
  _tryCompleteCouplerSetup()
end

-- Coupler Distance Mode callbacks (called from vehicle VM)
function M.onCouplerDistPlayerInfo(cid, epoch, overhang)
  if epoch ~= _cdEpoch then
    scannerLog('info', "Coupler dist player info: stale epoch " .. tostring(epoch) .. " (current " .. tostring(_cdEpoch) .. "), discarding")
    return
  end
  scannerLog('info', "Coupler dist player info: cid=" .. tostring(cid) .. " rearOverhang=" .. tostring(overhang))
  if cid == -1 then
    _cdPendingPlayer = false
  else
    _cdPendingPlayer = cid
    _cdPlayerCid = cid
    _cdPlayerOverhang = overhang or 0
  end
  _tryCouplerDistReady()
end

function M.onCouplerDistTargetInfo(cid, epoch, overhang)
  if epoch ~= _cdEpoch then
    scannerLog('info', "Coupler dist target info: stale epoch " .. tostring(epoch) .. " (current " .. tostring(_cdEpoch) .. "), discarding")
    return
  end
  scannerLog('info', "Coupler dist target info: cid=" .. tostring(cid) .. " frontOverhang=" .. tostring(overhang))
  if cid == -1 then
    _cdPendingTarget = false
  else
    _cdPendingTarget = cid
    _cdTargetCid = cid
    _cdTargetOverhang = overhang or 0
  end
  _tryCouplerDistReady()
end

function M.onTargetNameReady(displayName)
  if udpSend then
    local normalized = normalizeSpeechValue(displayName, "onTargetNameReady")
    if normalized then udpSend:send("TARGET_NAME:" .. normalized) end
  end
end

function M.onVehicleNameReady(displayName)
  if udpSend then
    local normalized = normalizeSpeechValue(displayName, "onVehicleNameReady")
    if normalized then udpSend:send("SWITCHED:" .. normalized) end
  end
end

function M.getCurrentTargetID()
  return currentTargetID
end

-- =================================================================================================
--  GE-level coupler hooks (called by beamstate via queueGameEngineLua)
-- =================================================================================================

-- Fired by the vehicle with the lower objectId when two vehicles physically couple.
-- objectId/obj2id are the two vehicle IDs; nodeId/obj2nodeId are the coupler node CIDs.
function M.onCouplerAttached(objectId, obj2id, nodeId, obj2nodeId)
  scannerLog('info', string.format("onCouplerAttached: obj=%s obj2=%s node=%s obj2node=%s",
    tostring(objectId), tostring(obj2id), tostring(nodeId), tostring(obj2nodeId)))

  if not couplerAttachMonitor then return end

  local player = be:getPlayerVehicle(0)
  local playerID = player and player:getID() or nil
  if not playerID or not currentTargetID then return end

  -- Check if the coupling involves our player and our current target
  local playerInvolved = (objectId == playerID or obj2id == playerID)
  local targetInvolved = (objectId == currentTargetID or obj2id == currentTargetID)

  if playerInvolved and targetInvolved then
    scannerLog('info', "ATTACH_MONITOR: player and target coupled — disabling tracking")
    couplerAttachMonitor = false

    -- Disable coupler distance mode locally (Python will mirror via COUPLED_DETECTED)
    if couplerDistMode then
      couplerDistMode = false
      couplerDistReady = false
      _cdDiscovering = false
      _cdPlayerCid = nil
    end

    -- Disable coupler tracking
    if couplerTrackActive then
      couplerTrackActive = false
    end

    if udpSend then udpSend:send("COUPLED_DETECTED:") end
  end
end

-- Fired when two vehicles decouple. Logged for diagnostics.
function M.onCouplerDetached(objectId, obj2id, nodeId, obj2nodeId)
  scannerLog('info', string.format("onCouplerDetached: obj=%s obj2=%s node=%s obj2node=%s",
    tostring(objectId), tostring(obj2id), tostring(nodeId), tostring(obj2nodeId)))
end

-- Called from vehicle VM (via queueGameEngineLua) when the player presses L to toggle couplers.
-- isActive=true means coupler mode was just enabled (visual indicators on);
-- isActive=false means it was just disabled.
--
-- vehID is filtered against the driven vehicle because EVERY spawned VM runs the telemetry
-- protocol and wraps its own couplings table. A trailer activating its own auto-coupling is
-- not the driver pressing L, and announcing it as such is indistinguishable from the real
-- thing. The id is optional so a mod half older than this file still reports (unfiltered)
-- rather than going silent -- bng_mod/ is a live junction and the two halves do go out of
-- step.
function M.onCouplerModeChange(vehID, isActive)
  if isActive == nil then
    -- Old one-argument form: the first parameter is the flag.
    isActive, vehID = vehID, nil
  end
  if vehID then
    local player = be:getPlayerVehicle(0)
    if not player or player:getID() ~= vehID then return end
  end
  if udpSend then
    udpSend:send(isActive and "COUPLER_MODE:ON" or "COUPLER_MODE:OFF")
  end
end

return M
