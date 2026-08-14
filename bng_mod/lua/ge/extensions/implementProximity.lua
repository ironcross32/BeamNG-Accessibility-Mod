-- implementProximity.lua
--
-- Tells a blind operator what the loader's bucket or forks are about to run into.
--
-- Scope note: the GROUND is deliberately NOT handled here. That is computed in the vehicle
-- VM (lua/vehicle/protocols/796F6C6F313035.lua), which owns the implement node set and has
-- both obj:getSurfaceHeightBelow and obj:castRayStatic. This extension covers only the
-- things the vehicle VM cannot see: other spawned objects.
--
-- Why node-level and not just bounding boxes: when a part breaks off a vehicle in BeamNG,
-- its nodes stay part of the SAME object. A detached bumper lying in the dirt is therefore
-- invisible to that vehicle's oriented bounding box but perfectly visible in its node
-- cloud. Requirement "don't let the forks hit a piece that fell off" only works node-side.
-- The bounding box is still used, but for the opposite question: is the implement INSIDE
-- the vehicle's volume without touching it, i.e. are the tines under the frame ready to
-- lift.
--
-- Note that in BeamNG most "props" -- cones, barriers, haybales, pallets -- are spawned
-- vehicle objects, so be:getObject covers them. Map-placed static clutter (TSStatic) is
-- not enumerable this way and shows up only in the vehicle VM's ground raycast.

local M = {}

local PYTHON_HOST      = "127.0.0.1"
local PYTHON_PORT_DATA = 4469  -- send proximity events to Python
local CMD_LISTEN_PORT  = 4470  -- receive ON/OFF/REBUILD from Python

local SCAN_INTERVAL   = 0.1    -- 10 Hz
local BROAD_RADIUS_M  = 25.0   -- object-position prefilter
local MAX_CANDIDATES  = 3      -- narrow-phase budget per tick
local NODE_STRIDE     = 8      -- sample every Nth node of a candidate
local CONTACT_M       = 0.12   -- closer than this counts as touching
-- Don't bother Python past this; the proximity speech's enter threshold is 3 m. This is
-- also the hard ceiling on the docking instrument's range: audio.py's DOCK_MAX_RANGE_M
-- must not exceed it, because past here the feed simply stops and the instrument would cut
-- out with no fade and no explanation rather than fading at its own configured range.
local REPORT_M        = 6.0
local INSIDE_MIN_PTS  = 2      -- implement points that must be inside the box
local MIN_EDGE_WIDTH_M = 0.30  -- shortest edgeL->edgeR baseline worth deriving a heading from

local udpSend, udpCmd = nil, nil
-- Active by default, unlike the scanner and obstacle detector which are keybind-toggled.
-- This one needs no handshake because it is silent by construction: with no implement
-- resolved, a tick costs one getPlayerVehicle and an early return. That also means it works
-- when BeamNG is started after beamtel, which an ON-on-startup message would not.
local isActive     = true
local scanTimer    = 0

-- Implement node set, pushed to us by the vehicle VM's telemetry protocol when it resolves
-- (on spawn and after every reset). Pushed rather than pulled on purpose: the resolution is
-- name-driven and lives in one place, and because it only ever changes when it re-runs, the
-- newest push is always authoritative -- there is no window in which we could be holding
-- cids that belong to a part that is no longer fitted.
local implVehID   = nil
local implCids    = nil
local implSampleCids = nil  -- exactly {edgeL, edgeC, edgeR, heelL, heelR}
local implName    = nil
local lastSentName = nil
local nameEverSent = false  -- distinguishes "not sent yet" from "sent NONE"
local lastSentLine = nil

-- The docking instrument. Off by default and toggled from Python, unlike the proximity
-- speech above: it is a deliberate mode you enter to line something up, not ambient
-- awareness, and its readout is meaningless on a machine with no implement fitted.
local dockActive   = false
local lastDockLine = nil

local function ipLog(level, msg) log(level, 'implementProximity', msg) end

-- Called from the vehicle VM. cidCsv is empty when the machine has no implement.
-- sampleCsv carries exactly five cids in a fixed order: edgeL, edgeC, edgeR, heelL, heelR.
function M.onImplementCids(vehID, friendlyName, sampleCsv, cidCsv)
  local function parse(csv)
    local out = {}
    for s in tostring(csv or ""):gmatch("[^,]+") do
      local n = tonumber(s)
      if n then out[#out + 1] = n end
    end
    return out
  end
  local cids = parse(cidCsv)
  local sample = parse(sampleCsv)
  if #sample ~= 5 then sample = nil end

  -- Idempotent, because this now arrives as a heartbeat rather than once. Resetting the
  -- announce latch unconditionally would re-send the IMPLEMENT: line every few seconds, and
  -- Python treats that as a part swap -- it drops whatever it was tracking, so the approach
  -- speech would re-announce "Bucket approaching X" on a loop while you sat still.
  local newName = (#cids > 0) and friendlyName or nil
  local same = (implVehID == vehID)
           and (implName == newName)
           and (implCids ~= nil) == (#cids > 0)
  if same and implCids then
    same = (#implCids == #cids)
    if same then
      for i = 1, #cids do
        if implCids[i] ~= cids[i] then same = false; break end
      end
    end
  end

  implSampleCids = sample
  if #cids > 0 then
    implVehID, implCids, implName = vehID, cids, friendlyName
    if not same then
      ipLog('I', string.format("implement '%s' on vehicle %d: %d sample nodes",
        tostring(friendlyName), vehID, #cids))
    end
  else
    implVehID, implCids, implName = vehID, nil, nil
    if not same then ipLog('I', string.format("vehicle %d reports no implement", vehID)) end
  end
  -- Only an actual change forces a fresh IMPLEMENT: line.
  if not same then lastSentName, nameEverSent = nil, false end
end

-- Where the implement is and which way it points, in world space. Returns nil on anything
-- without an implement, so callers can fall back to the whole-vehicle frame.
--
-- This exists because an articulated machine has TWO frames. player:getDirectionVector()
-- describes the rear one, where the cab and the reference nodes live; the bucket is bolted
-- to the front one, which yaws relative to it as the frame bends. Aiming with the rear
-- frame is actively misleading: bend left toward a target and the rear initially swings
-- RIGHT as the machine pivots, so a bearing that should be closing opens up instead.
function M.getImplementFrame()
  if not implSampleCids then return nil end
  local player = be:getPlayerVehicle(0)
  if not player then return nil end
  if implVehID and player:getID() ~= implVehID then return nil end

  local ok, res = pcall(function()
    local base = vec3(player:getPosition())
    local function midOf(first, last)
      local acc, n = vec3(0, 0, 0), 0
      for i = first, last do
        acc = acc + (base + vec3(player:getNodePosition(implSampleCids[i])))
        n = n + 1
      end
      return acc / n
    end
    local edge = midOf(1, 3)   -- cutting edge / tine tips
    local heel = midOf(4, 5)   -- the implement's rear-bottom

    -- Derive the heading from the implement's LATERAL axis (edgeL -> edgeR), not from
    -- heel -> edge. Tilt rotates the implement about that lateral axis, so the lateral
    -- axis is the one thing curling cannot skew. Heel -> edge shortens as the bucket
    -- curls back and can invert entirely past vertical, which would flip the bearing
    -- left-for-right at exactly the moment you are carrying a full load.
    local left, right = vec3(player:getNodePosition(implSampleCids[1])),
                        vec3(player:getNodePosition(implSampleCids[3]))
    local lateral = right - left
    lateral.z = 0
    local fwd
    -- Needs a real baseline. A short one points wherever soft-body jitter says it does,
    -- and since the hemisphere test below flips on the sign of a dot product, a noisy
    -- heading does not degrade gracefully -- it snaps between left and right.
    if lateral:length() > MIN_EDGE_WIDTH_M then
      -- Rotate the lateral axis 90 degrees in the horizontal plane, then resolve which of
      -- the two normals points ahead using the machine's own heading. Only the hemisphere
      -- matters, and the rear frame is never further off than the articulation angle, so
      -- this cannot pick the wrong one.
      fwd = vec3(-lateral.y, lateral.x, 0)
      local veh = vec3(player:getDirectionVector())
      veh.z = 0
      if fwd:dot(veh) < 0 then fwd = -fwd end
    else
      -- Degenerate lateral axis (a narrow implement where both edge picks collapsed onto
      -- the centre). Fall back to the along-axis, which is better than nothing.
      fwd = edge - heel
      fwd.z = 0
    end
    if fwd:length() < 1e-3 then return nil end
    -- Origin is the cutting edge, not the implement centroid: that is the part of the
    -- machine you are actually trying to put somewhere, so a distance of zero means
    -- "touching" rather than "half a bucket short".
    return {pos = edge, fwd = fwd:normalized(), name = implName}
  end)
  if not ok then return nil end
  return res
end

-- The implement's world-space contact points, for callers that need a contact SET rather
-- than the single origin getImplementFrame returns. This is what lets vehicleScanner treat
-- a loader as "a different cid list" instead of a special case: with an implement fitted the
-- contact set is the bucket or tines, without one it is the vehicle's own front node band,
-- and the distance code downstream cannot tell the difference.
--
-- The five sample cids are used rather than the full implCids list: they are the edge and
-- heel extremes, which are the parts that actually reach a target first, and five points
-- keeps the nearest-approach sweep cheap enough to run every scanner tick.
function M.getImplementPoints()
  if not (implSampleCids and implCids) then return nil end
  local player = be:getPlayerVehicle(0)
  if not player then return nil end
  if implVehID and player:getID() ~= implVehID then return nil end
  local ok, pts = pcall(function()
    local base = vec3(player:getPosition())
    local out = {}
    for _, cid in ipairs(implSampleCids) do
      out[#out + 1] = base + vec3(player:getNodePosition(cid))
    end
    return out
  end)
  if not ok or not pts or #pts == 0 then return nil end
  return pts
end

local function send(line)
  if udpSend then pcall(function() udpSend:send(line) end) end
end

-- Strip the separators the CSV protocol relies on, the way vehicleScanner does for names.
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

-- World positions of the implement's sample nodes. getNodePosition returns an offset from
-- the vehicle origin expressed in WORLD axes, so this is a plain addition.
local function implementPoints(player)
  if not implCids then return nil end
  local base = vec3(player:getPosition())
  local pts = {}
  for _, cid in ipairs(implCids) do
    local ok, p = pcall(function() return vec3(player:getNodePosition(cid)) end)
    if ok and p then pts[#pts + 1] = base + p end
  end
  if #pts == 0 then return nil end
  return pts
end

-- Oriented box test in the TARGET's own frame. Deliberately not
-- be:getObjectOOBBHalfExtentsXYZ against world axes: for a car parked at any angle other
-- than dead-on that would describe a box it isn't in.
local function boxFrame(veh)
  local ok, res = pcall(function()
    local fwd = vec3(veh:getDirectionVector()):normalized()
    local up  = vec3(veh:getDirectionVectorUp()):normalized()
    local right = fwd:cross(up)
    if right:length() < 1e-4 then return nil end
    return {c = vec3(veh:getPosition()), f = fwd, u = up, r = right:normalized()}
  end)
  if not ok or not res then return nil end
  return res
end

-- Half-extents and the vertical span, measured from the node cloud rather than taken from
-- the engine, so a deformed or partly-detached vehicle still reports its real footprint.
--
-- Preferred source is vehicleGeometry's cache, for two reasons that are about accuracy
-- rather than speed. Its extents are measured over EVERY node once, where the strided
-- fallback below samples every 8th and can therefore miss the actual extreme node and
-- under-report the box. And its cached cids are hull nodes -- ones near a face of the
-- bounding box -- which is exactly the set a nearest-approach-from-outside test wants,
-- whereas an arbitrary stride wastes most of its budget on interior nodes that can never be
-- the closest point. The fallback stays because the cache is resolved asynchronously and is
-- not there for the first few ticks after a spawn.
local function measureTargetCached(veh, frame)
  local geo = extensions and extensions.vehicleGeometry or nil
  if not geo then return nil end
  local vehID = veh:getID()
  local entry = geo.get(vehID)
  if not entry then
    pcall(geo.request, vehID)
    return nil
  end
  if not entry.hull then return nil end
  local base = vec3(veh:getPosition())
  local pts, minZ, maxZ = {}, math.huge, -math.huge
  for _, cid in ipairs(entry.hull) do
    local ok, p = pcall(function() return base + vec3(veh:getNodePosition(cid)) end)
    if ok and p then
      pts[#pts + 1] = p
      if p.z < minZ then minZ = p.z end
      if p.z > maxZ then maxZ = p.z end
    end
  end
  if #pts == 0 then return nil end
  local e = entry.ext
  return {
    pts = pts,
    minF = e.minF, maxF = e.maxF, minR = e.minR, maxR = e.maxR,
    minU = e.minU, maxU = e.maxU,
    minZ = minZ, maxZ = maxZ,
  }
end

local function measureTarget(veh, frame)
  local cached = measureTargetCached(veh, frame)
  if cached then return cached end

  local n = veh:getNodeCount()
  if not n or n < 4 then return nil end
  local base = vec3(veh:getPosition())
  local minF, maxF = math.huge, -math.huge
  local minR, maxR = math.huge, -math.huge
  local minU, maxU = math.huge, -math.huge
  local minZ, maxZ = math.huge, -math.huge
  local pts = {}
  local i = 0
  while i < n do
    local ok, p = pcall(function() return base + vec3(veh:getNodePosition(i)) end)
    if ok and p then
      pts[#pts + 1] = p
      local d = p - frame.c
      local f, r, u = frame.f:dot(d), frame.r:dot(d), frame.u:dot(d)
      if f < minF then minF = f end
      if f > maxF then maxF = f end
      if r < minR then minR = r end
      if r > maxR then maxR = r end
      if u < minU then minU = u end
      if u > maxU then maxU = u end
      if p.z < minZ then minZ = p.z end
      if p.z > maxZ then maxZ = p.z end
    end
    i = i + NODE_STRIDE
  end
  if #pts == 0 then return nil end
  return {
    pts = pts,
    minF = minF, maxF = maxF, minR = minR, maxR = maxR, minU = minU, maxU = maxU,
    minZ = minZ, maxZ = maxZ,
  }
end

-- =================================================================================================
--  Docking reference band
-- =================================================================================================

-- Which band of the target the implement is being lined up against. Lifting and ramming want
-- different bands out of identical geometry, so the reference is selected rather than derived,
-- and it is announced -- with a derived pocket the operator could never tell what the readout
-- was measuring against.
--
-- Auto-selection needs to know a bucket from a pair of forks, which is otherwise a Python-side
-- job (beamtel's _implement_word). Only the CHOICE is made here; the spoken naming stays over
-- there. Splitting it this way keeps one copy of each: Lua picks, Python speaks.
local BAND_MIN_HEIGHT_M = 0.10  -- a void shorter than this is a modelling seam, not a pocket
local bandIndex    = nil        -- nil means auto-select
local bandTargetID = nil        -- which target the manual index belongs to

local function implementIsFork()
  local s = tostring(implName or ""):lower()
  return s:find("fork", 1, true) ~= nil
      or s:find("tine", 1, true) ~= nil
      or s:find("grapple", 1, true) ~= nil
end

-- Forks want the lowest real void: that is the pallet pocket, or the air under a car's
-- rocker. A bucket wants the tallest solid run, because you are lining up against a face --
-- a pile, a flank, a door -- rather than trying to get inside anything.
local function autoSelectBand(bands)
  if not bands or #bands == 0 then return nil end
  if implementIsFork() then
    for i, b in ipairs(bands) do
      if b.kind == "GAP" and (b.hiZ - b.loZ) >= BAND_MIN_HEIGHT_M then return i end
    end
  end
  local best, bestH = nil, -1
  for i, b in ipairs(bands) do
    if b.kind == "SOLID" and (b.hiZ - b.loZ) > bestH then best, bestH = i, b.hiZ - b.loZ end
  end
  if best then return best end
  return 1
end

local function resolveBand(targetID)
  local geo = extensions and extensions.vehicleGeometry or nil
  if not geo or not geo.bands then return nil, nil, 0, "vehicleGeometry not loaded" end
  local ok, bands, why = pcall(geo.bands, targetID)
  if not ok then return nil, nil, 0, "bands threw" end
  if not bands then return nil, nil, 0, why or "no bands" end
  if #bands == 0 then return nil, nil, 0, "no bands" end
  -- A manual pick belongs to the target it was made against; a new target starts on auto
  -- again rather than inheriting an index that now points at unrelated geometry.
  if bandTargetID ~= targetID then
    bandIndex, bandTargetID = nil, targetID
  end
  local idx = bandIndex or autoSelectBand(bands) or 1
  if idx < 1 then idx = 1 end
  if idx > #bands then idx = #bands end
  -- Write the clamp back, or holding the cycle key past the end would run the index away and
  -- the same number of presses would then be needed to get back into range.
  if bandIndex then bandIndex = idx end
  return bands[idx], idx, #bands
end

local function cycleBand(step)
  bandIndex = (bandIndex or 0) + step
  -- Clamped, not wrapped: wrapping a short list is disorienting when you cannot see it, and
  -- hitting the end is itself information.
  if bandIndex < 1 then bandIndex = 1 end
end

-- =================================================================================================
--  Slam gate (using the implement as a tool of destruction)
-- =================================================================================================

-- Nulling is the wrong shape for dropping a bucket on something. Lining up to lift wants a
-- vertical error driven to zero; an overhead slam wants the opposite -- get decisively above
-- it, get over it, then commit. So this is three discrete states with hysteresis rather than
-- a continuous readout, and it is deliberately coarse: ramming is forgiving in a way that
-- sliding tines into a pallet pocket is not, so the precision instrument stays off here and
-- the whole thing costs three booleans.
--
-- It needs no new geometry at all. edgeL/C/R and heelL/R are already resolved and already
-- pushed to this VM; the mode only changes WHICH of those five points drive the test --
-- heels for what lands in an overhead drop, the cutting edge for a flat ram.
local SLAM_CLEAR_ENTER_M = 0.20  -- implement underside this far above the target's top
local SLAM_CLEAR_EXIT_M  = 0.05  -- ...and how far it must fall back through to lose it
local SLAM_OVER_MIN_PTS  = 2     -- implement points inside the target's plan outline
local slamState = "NONE"

-- Plan-view containment: inside the target's footprint looking straight down, ignoring
-- height entirely. This is the inverse of the lift test above, which additionally requires
-- the implement centroid BELOW the box mid-height because it is asking "are the tines
-- underneath ready to lift". Here the question is "am I over it", and the height half of the
-- answer is the clearance test, not this one.
local function overBox(p, frame, box)
  local d = p - frame.c
  local f, r = frame.f:dot(d), frame.r:dot(d)
  return f >= box.minF and f <= box.maxF and r >= box.minR and r <= box.maxR
end

local function resolveSlam(best, pts, implMinZ)
  if not best or not best.box then return "NONE" end
  local box, frame = best.box, best.frame

  -- Hysteresis on the clearance, or hunting on the lift lever at the threshold chatters the
  -- state -- and this cue is meant to be a decision point, not a flicker.
  local margin = (slamState == "CLEAR" or slamState == "COMMITTED")
                 and SLAM_CLEAR_EXIT_M or SLAM_CLEAR_ENTER_M
  local clear = implMinZ >= (box.maxZ + margin)

  local nOver = 0
  for _, p in ipairs(pts) do
    if overBox(p, frame, box) then nOver = nOver + 1 end
  end
  -- Asymmetric by construction: it takes two points to claim the footprint and losing it
  -- takes all of them, so drifting along an edge cannot flutter.
  local wasOver = (slamState == "OVER" or slamState == "COMMITTED")
  local over = wasOver and (nOver > 0) or (nOver >= SLAM_OVER_MIN_PTS)

  if clear and over then return "COMMITTED" end
  if clear then return "CLEAR" end
  if over then return "OVER" end
  return "NONE"
end

local function insideBox(p, frame, box)
  local d = p - frame.c
  local f, r, u = frame.f:dot(d), frame.r:dot(d), frame.u:dot(d)
  return f >= box.minF and f <= box.maxF
     and r >= box.minR and r <= box.maxR
     and u >= box.minU and u <= box.maxU
end

-- The docking readout: where the implement is relative to the selected band of the nearest
-- target, resolved onto the implement's own axes rather than the machine's.
--
-- Three numbers plus a reference, which is the ceiling for anything a person can track at
-- once. Lateral and vertical are the ones you steer and lift with; range tells you how long
-- you have left to get them right. Yaw is reported but is not one of the continuous
-- channels -- it only matters at the moment of entry, and sonifying a fourth axis is exactly
-- how the obstacle detector became unusable.
-- Every path that cannot produce a reading says so, rather than returning silently.
-- The first version returned nil from five different places with no output anywhere, so a
-- failure was indistinguishable from "nothing nearby" -- and since the instrument shares its
-- soundscape with the scanner, which pans and changes pitch and pulses, a dead instrument
-- was also indistinguishable from a working one. Three rounds of play-testing were spent
-- guessing at which. A named reason costs one UDP line and F9+I reads it straight out.
local function dockFail(reason)
  local line = "DOCKFAIL:" .. reason
  if lastDockLine ~= line then
    lastDockLine = line
    send(line)
  end
end

local function sendDockLine(best, reason)
  if not dockActive then return end
  if reason then return dockFail(reason) end
  if not implCids then return dockFail("no implement resolved") end
  if not best then
    if lastDockLine ~= "DOCKCLEAR" then
      lastDockLine = "DOCKCLEAR"
      send("DOCKCLEAR")
    end
    return
  end

  local ok, err = pcall(function()
    local frame = M.getImplementFrame()
    if not frame then return dockFail("no implement frame") end
    local band, idx, count, why = resolveBand(best.id)
    if not band then return dockFail(why or "no reference band") end

    -- Implement axes. Left is world-up cross the implement heading, keeping the mod-wide
    -- convention that a positive bearing means LEFT.
    local fwd = frame.fwd
    local left = vec3(0, 0, 1):cross(fwd)
    if left:length() < 1e-4 then return dockFail("degenerate implement heading") end
    left = left:normalized()

    local geo = extensions and extensions.vehicleGeometry or nil
    local centre = nil
    if geo and geo.boxCentre then
      local okC, c = pcall(geo.boxCentre, best.id)
      if okC and c then centre = c end
    end
    if not centre then centre = vec3(best.veh:getPosition()) end

    local d = centre - frame.pos
    local lateral = left:dot(d)

    -- Positive vertical means the band sits ABOVE the cutting edge, i.e. raise to meet it.
    -- Measured against the band's mid-height rather than its floor because for a fork pocket
    -- the middle is what you aim the tines at, and for a solid band it is the centre of the
    -- face you are about to hit.
    local bandMid = (band.loZ + band.hiZ) * 0.5
    local vertical = bandMid - frame.pos.z

    -- Squareness to the target's face, folded to +/-90: entering a pallet at an angle jams
    -- it, but a pallet has no front or back as far as the tines are concerned.
    local tf = vec3(best.veh:getDirectionVector())
    tf.z = 0
    local yaw = 0
    if tf:length() > 1e-4 then
      tf = tf:normalized()
      local c = math.max(-1, math.min(1, fwd:dot(tf)))
      yaw = math.deg(math.acos(c))
      if yaw > 90 then yaw = 180 - yaw end
      if left:dot(tf) < 0 then yaw = -yaw end
    end

    send(string.format("DOCK:%s,%.3f,%.3f,%.3f,%d,%d,%s,%.3f,%.3f,%.1f,%d",
      best.name, best.d, lateral, vertical, idx, count, band.kind,
      band.loZ, band.hiZ, yaw, (bandIndex ~= nil) and 1 or 0))
    lastDockLine = "DOCK"
  end)
  if not ok then
    ipLog('E', "dock readout threw: " .. tostring(err))
    dockFail("readout error")
  end
end

local function scan()
  local player = be:getPlayerVehicle(0)
  if not player then
    sendDockLine(nil, "no player vehicle")
    return
  end

  -- A push we haven't matched to the current vehicle is not usable.
  --
  -- Every one of these early returns used to be silent, which is why "no implement fitted"
  -- was untraceable: the readout never got as far as the code that could explain itself, so
  -- the only thing that ever spoke was Python's own guess at the state. Each one now names
  -- what it saw, including the two vehicle ids, because "the mod holds an implement for a
  -- vehicle you are not sitting in" and "the mod holds nothing at all" need completely
  -- different fixes and sound identical from the seat.
  if implVehID and player:getID() ~= implVehID then
    sendDockLine(nil, string.format(
      "implement belongs to vehicle %s, you are in %d",
      tostring(implVehID), player:getID()))
    return
  end

  -- Compare the STRING we would send, not implName itself, and track whether anything has
  -- been sent at all. Comparing implName against lastSentName looks equivalent and is not:
  -- on a vehicle with no implement both are nil, so they test equal and the NONE line is
  -- never sent. Python then keeps whatever name it last heard, and announces "measuring
  -- from the forks" on a car you climbed into after getting out of the loader.
  local desiredName = implName and cleanName(implName) or "NONE"
  if (not nameEverSent) or desiredName ~= lastSentName then
    nameEverSent = true
    lastSentName = desiredName
    send("IMPLEMENT:" .. desiredName)
  end
  if not implCids then
    sendDockLine(nil, string.format(
      "mod holds no implement for vehicle %d (last push %s)",
      player:getID(), implVehID and ("from " .. tostring(implVehID)) or "never arrived"))
    return
  end

  local pts = implementPoints(player)
  if not pts then return end

  local implMinZ, implMaxZ = math.huge, -math.huge
  local implCentre = vec3(0, 0, 0)
  for _, p in ipairs(pts) do
    if p.z < implMinZ then implMinZ = p.z end
    if p.z > implMaxZ then implMaxZ = p.z end
    implCentre = implCentre + p
  end
  implCentre = implCentre / #pts

  -- Broad phase on object position. Radius is generous rather than tight because a part
  -- that has fallen off sits well outside its parent's centre.
  local playerID = player:getID()
  local cands = {}
  for i = 0, be:getObjectCount() - 1 do
    local obj = be:getObject(i)
    if obj and obj:getID() ~= playerID then
      local ok, d = pcall(function() return implCentre:distance(vec3(obj:getPosition())) end)
      if ok and d and d < BROAD_RADIUS_M then
        cands[#cands + 1] = {obj = obj, d = d}
      end
    end
  end
  if #cands == 0 then
    if lastSentLine ~= "CLEAR" then lastSentLine = "CLEAR"; send("CLEAR") end
    return
  end
  table.sort(cands, function(a, b) return a.d < b.d end)

  local best = nil
  for ci = 1, math.min(#cands, MAX_CANDIDATES) do
    local veh = cands[ci].obj
    local frame = boxFrame(veh)
    local box = frame and measureTarget(veh, frame)
    if box then
      -- Narrow phase: nearest approach between the implement's sample points and the
      -- target's node cloud. This is the test that sees detached parts.
      local minD = math.huge
      for _, ip in ipairs(pts) do
        for _, tp in ipairs(box.pts) do
          local d = ip:squaredDistance(tp)
          if d < minD then minD = d end
        end
      end
      minD = math.sqrt(minD)

      if minD < REPORT_M then
        -- "Under the frame, not touching it" needs all three of these. Box overlap alone
        -- fires constantly, because a solid box also contains the air beside a low car's
        -- wheels -- which is not a place you can lift from.
        local nInside = 0
        for _, ip in ipairs(pts) do
          if insideBox(ip, frame, box) then nInside = nInside + 1 end
        end
        local boxMidZ = (box.minZ + box.maxZ) * 0.5
        local inside = (nInside >= INSIDE_MIN_PTS)
                   and (minD > CONTACT_M)
                   and (implCentre.z < boxMidZ)

        local relation
        if implMinZ >= box.maxZ then
          relation = "ABOVE"
        elseif implMaxZ <= box.minZ then
          relation = "BELOW"
        else
          relation = "LEVEL"
        end

        if not best or minD < best.d then
          best = {
            d = minD,
            name = nameOf(veh),
            relation = relation,
            inside = inside and 1 or 0,
            contact = (minD <= CONTACT_M) and 1 or 0,
            veh = veh,
            id = veh:getID(),
            frame = frame,
            box = box,
          }
        end
      end
    end
  end

  local line
  if best then
    line = string.format("NEAR:%s,%.3f,%s,%d,%d",
      best.name, best.d, best.relation, best.inside, best.contact)
  else
    line = "CLEAR"
  end
  -- Python owns the hysteresis and the speech; resend a live NEAR every tick so it can see
  -- the distance move, but collapse repeated CLEARs so an idle machine sends nothing.
  if line ~= "CLEAR" or lastSentLine ~= "CLEAR" then
    lastSentLine = line
    send(line)
  end

  sendDockLine(best)

  -- The slam gate rides on the docking toggle rather than getting one of its own. It is the
  -- same act -- putting the implement somewhere specific relative to an object -- approached
  -- from the other end, and a second mode key for it would be one more thing to remember
  -- mid-manoeuvre. Sent only on change; these are decision points, not a readout.
  if dockActive then
    local st = resolveSlam(best, pts, implMinZ)
    if st ~= slamState then
      slamState = st
      send("SLAM:" .. st)
    end
  elseif slamState ~= "NONE" then
    slamState = "NONE"
  end
end

-- =================================================================================================
--  GE Extension Hooks
-- =================================================================================================

local function setupSockets()
  if udpSend then pcall(function() udpSend:close() end); udpSend = nil end
  if udpCmd  then pcall(function() udpCmd:close()  end); udpCmd  = nil end

  udpSend = socket.udp()
  if udpSend then
    udpSend:setpeername(PYTHON_HOST, PYTHON_PORT_DATA)
    udpSend:settimeout(0)
  else
    ipLog('E', "Failed to create UDP send socket.")
  end

  local ok, err = pcall(function()
    udpCmd = socket.udp()
    udpCmd:setsockname("127.0.0.1", CMD_LISTEN_PORT)
    udpCmd:settimeout(0)
  end)
  if not (ok and udpCmd) then
    ipLog('E', "Failed to create UDP command socket: " .. tostring(err))
    udpCmd = nil
  end
end

local function resetState()
  isActive = true
  scanTimer = 0
  implVehID, implCids, implName, implSampleCids = nil, nil, nil, nil
  lastSentName, lastSentLine, nameEverSent = nil, nil, false
  -- A manual band pick is tied to a target and to the implement that was fitted when it was
  -- made; both may have just changed, so fall back to auto rather than keeping an index that
  -- now points at unrelated geometry.
  bandIndex, bandTargetID, lastDockLine = nil, nil, nil
  slamState = "NONE"
end

function M.onExtensionLoaded()
  setExtensionUnloadMode(M, "manual")
  ipLog('I', "Implement proximity extension loaded.")
  setupSockets()  -- here too, so a Ctrl+L Lua reload re-opens them
end

function M.onWorldReadyState(state)
  if state == 2 then
    resetState()
    setupSockets()
  end
end

-- The cid cache belongs to one vehicle and one part configuration. Drop it on any event
-- that could invalidate either and wait for the vehicle VM to push a fresh one; reporting
-- against stale cids would give confident, wrong positions rather than an error.
function M.onVehicleSwitched(oldId, newId, player)
  if player ~= nil and player ~= 0 then return end
  implVehID, implCids, implName, implSampleCids = nil, nil, nil, nil
  lastSentName, lastSentLine, nameEverSent = nil, nil, false
  -- A manual band pick is tied to a target and to the implement that was fitted when it was
  -- made; both may have just changed, so fall back to auto rather than keeping an index that
  -- now points at unrelated geometry.
  bandIndex, bandTargetID, lastDockLine = nil, nil, nil
  slamState = "NONE"
end

function M.onVehicleResetted(vehId)
  if implVehID and vehId ~= implVehID then return end
  implVehID, implCids, implName, implSampleCids = nil, nil, nil, nil
  lastSentName, lastSentLine, nameEverSent = nil, nil, false
  -- A manual band pick is tied to a target and to the implement that was fitted when it was
  -- made; both may have just changed, so fall back to auto rather than keeping an index that
  -- now points at unrelated geometry.
  bandIndex, bandTargetID, lastDockLine = nil, nil, nil
  slamState = "NONE"
end

function M.onUpdate(dtReal, dtSim, dtRaw)
  if udpCmd then
    local data
    repeat
      data = udpCmd:receive()
      if data then
        local cmd = data:match("^%s*(.-)%s*$"):upper()
        if cmd == "ON" then
          isActive = true
          lastSentName, lastSentLine, nameEverSent = nil, nil, false
        elseif cmd == "OFF" then
          isActive = false
        elseif cmd == "REBUILD" then
          implVehID, implCids, implName, implSampleCids = nil, nil, nil, nil
          lastSentName, lastSentLine, nameEverSent = nil, nil, false
        elseif cmd == "DOCK_ON" then
          dockActive, lastDockLine = true, nil
        elseif cmd == "DOCK_OFF" then
          dockActive, lastDockLine, slamState = false, nil, "NONE"
        elseif cmd == "BANDNEXT" then
          cycleBand(1)
        elseif cmd == "BANDPREV" then
          cycleBand(-1)
        elseif cmd == "BANDAUTO" then
          bandIndex = nil
        end
      end
    until not data
  end

  if not isActive then return end

  scanTimer = scanTimer + dtReal
  if scanTimer >= SCAN_INTERVAL then
    scanTimer = 0
    -- The GE onUpdate hook chain is dispatched WITHOUT pcall, so a throw here would
    -- silently stop every extension loaded after this one in modScript.lua.
    local ok, err = pcall(scan)
    if not ok then
      ipLog('E', "scan failed, disabling: " .. tostring(err))
      isActive = false
    end
  end
end

return M
