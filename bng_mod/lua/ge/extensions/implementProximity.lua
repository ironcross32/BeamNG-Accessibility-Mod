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

-- Ramp mode. The docking instrument's other answer: lining an ordinary vehicle up with the
-- drive-in mouth of a ramp-equipped machine (the stock large_cannon) instead of lining an
-- implement up with a load. Same instrument, same toggle, same wire format -- see scan().
--
-- The ranges are PER MODE and deliberately not a raise of REPORT_M. The implement instrument
-- is short-range on purpose: a bucket's approach is the last two metres, and stretching its
-- pulse rate across twenty would make the whole of a loader's working envelope crawl.
--
-- This is the FEED cutoff, not the range at which the instrument sonifies anything. Those are
-- two different jobs and conflating them is what made the whole thing go dead in the one place
-- it was most needed. At 20 m -- measured to the MOUTH -- parking beside a sixteen-metre cannon
-- at the barrel end puts the mouth comfortably outside it, so the mod sent DOCKCLEAR: no tones,
-- no reason, no readout, an instrument indistinguishable from a switched-off one while the
-- scanner cheerfully reported the machine three metres away. Reported from the seat as "I'm
-- getting nothing", and F9+I could only answer "nothing in range".
--
-- The feed now runs well past where audio fades out (DOCK_RAMP_MAX_RANGE_M, 25 m), so the
-- spoken readout keeps answering after the tones have gone quiet -- which is exactly the state
-- in which someone taps the key. 45 m covers standing anywhere around a machine of this size
-- with room to spare, and the packet is a hundred bytes at 10 Hz.
local RAMP_REPORT_M   = 45.0
-- Object-position prefilter for ramp candidates, and it has to be much wider than
-- BROAD_RADIUS_M rather than reusing it. That radius is measured to the object's REFERENCE
-- node, which on a ~16 m cannon sits ten to fifteen metres behind the ramp mouth -- so a mouth
-- twenty metres away can easily have its object centre thirty-five metres away and be filtered
-- out before it is ever looked at. Costs nothing: the candidate loop already visits every
-- object, and rampGeometry.get is only consulted for what survives.
--
-- It has to keep clearing RAMP_REPORT_M by more than half a machine length, or the prefilter
-- silently becomes the real ceiling and the raise above buys nothing.
local RAMP_SEARCH_M   = 70.0
-- Degenerate mouth baseline guard, mirroring MIN_EDGE_WIDTH_M and there for the same reason.
local RAMP_MIN_WIDTH_M = 0.30
-- Clear air left between the vehicle's NOSE and the mouth by the align teleport. 20 ft.
--
-- Measured nose-to-mouth rather than origin-to-mouth, which is the whole reason the forward
-- extent is looked up below: the reference node is nowhere near the front bumper on most
-- vehicles and is metres from it on a semi, so an origin-referenced standoff would give a
-- hatchback twenty feet of run-up and a truck about eight. The number the driver is being
-- promised is the gap they can see, so that is the number that has to be constant.
local RAMP_ALIGN_STANDOFF_M = 6.096
-- ...and what to assume when vehicleGeometry has not resolved the player yet. Deliberately a
-- generous nose rather than zero: over-estimating it parks you further back, which costs a
-- second of driving, while under-estimating it parks you inside the ramp mouth.
local RAMP_ALIGN_NOSE_FALLBACK_M = 3.0
-- How high the ramp's own lip may sit above the ground before driving at it is a collision
-- rather than an approach. Reported, never enforced: lining up before deploying the ramp is a
-- perfectly reasonable order to do things in, and refusing the teleport would make the key
-- useless in exactly that case. Measured on a us_semi tc82s_rollback, where the whole span is
-- decided by the bed rather than by the tilt: home and level the lip sits 1.30 m up, full tilt
-- alone brings it to 0.95 m, and only running the bed fully out AND tilting puts it at 0.15 m.
-- So a threshold anywhere in the middle separates "deployed" from "you are about to hit the
-- back of a truck" with a wide margin on both sides, which is what this bug turned out to be:
-- the align placed the driver twenty feet in front of a four-foot wall and said only where
-- they were.
local RAMP_ALIGN_LIP_SAY_M = 0.30

local udpSend, udpCmd = nil, nil
-- ...and retried, because a failed bind is otherwise permanent. The most likely cause is a
-- socket leaked by a previous extensions.reload(), which the GC frees moments later -- so the
-- condition that breaks the extension for a whole session clears itself within seconds if
-- anything ever looks again. Cheap: one comparison per frame while bound, one socket call
-- every few seconds while not.
local CMD_BIND_RETRY_S = 3.0
local cmdBindRetry = 0
-- Active by default, unlike the scanner and obstacle detector which are keybind-toggled.
-- This one needs no handshake because it is silent by construction: with no implement
-- resolved, a tick costs one getPlayerVehicle and an early return. That also means it works
-- when BeamNG is started after beamtel, which an ON-on-startup message would not.
local isActive     = true
local scanTimer    = 0
-- Monotonic seconds since load, for the RAMPSELF: heartbeat. scanTimer is reset on every tick
-- and so cannot answer "how long since we last sent", which is a different question.
local scanClock    = 0

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
-- Which cannon the player is sitting in: OLD (ballistic barrel), LARGE (drive-in ramp), or
-- "0". Latched so the type line is only sent on change.
local lastCannon   = nil
-- What the ramp machine you are SITTING IN is doing with its own deck, latched so the line is
-- only sent on change. Stored per vehicle for exactly the reason implByVeh is: every vehicle VM
-- pushes its own hydraulics, and only the one being driven may answer the readout.
local rampHydByVeh = {}
local lastRampSelf = nil
-- ...and re-sent on a heartbeat as well as on change, the same correction 796F6C6F313035.lua's
-- cid push already carries. A latch here cannot be cleared by beamtel restarting, and beamtel
-- restarting while the game keeps running is the normal way to pick up a change. The ON probe
-- does re-arm it, but relying on that alone leaves the readout mute for any other way the two
-- halves can fall out of step, and the failure is silent: the key answers "docking instrument
-- is off", which is a real answer for a real reason, so nothing looks broken.
local RAMP_SELF_HEARTBEAT_S = 4.0
local rampSelfSentAt = -1e9
-- Last heading we were confident in, so a momentarily degenerate frame holds position rather
-- than mirroring. See the length guard in getImplementFrame.
local lastGoodFwd  = nil

local function ipLog(level, msg) log(level, 'implementProximity', msg) end

-- Pushes are stored PER VEHICLE and only the player's is ever made active.
--
-- They used to land straight in the shared globals, whichever vehicle sent them. That was
-- survivable while each vehicle pushed exactly once (last spawn quietly won), but the push
-- is a heartbeat now, so two vehicles claiming an implement take it in turns every few
-- seconds. Worse, the vehicle-side match is pure name matching against a wide word list --
-- "bucket", "fork", "blade", "scoop", "plow" -- which a bucket SEAT, a wiper blade or a hood
-- scoop can satisfy. So an ordinary car parked next to the loader can take ownership, and
-- the scanner's origin then flips between the bucket's cutting edge and the loader's
-- reference node on a four-second cycle. Nothing logged it, because the re-push is
-- idempotent and only speaks up when the payload changes.
--
-- The vehicle side now gates its resolution on the machine actually having implement lift/tilt
-- rams, so a car can no longer claim one at all -- but this scoping is not made redundant by
-- that and must stay: two loaders in the same scene both legitimately push, and only the one
-- being driven may own the globals.
local implByVeh = {}  -- vehID -> {cids = {...}, sample = {...}|nil, name = string} or false

-- Promote whichever stored push belongs to the vehicle currently being driven. Cheap enough
-- to run every scan tick, which is also what makes switching vehicles take effect at once
-- rather than waiting for the next heartbeat.
local function applyActivePush()
  local player = be:getPlayerVehicle(0)
  local pid = player and player:getID() or nil
  local e = pid and implByVeh[pid] or nil
  local newCids = e and e.cids or nil
  local newName = e and e.name or nil

  -- Idempotent, because this runs constantly now. Clearing the announce latch every time
  -- would re-send the IMPLEMENT: line on a loop, and Python reads that as a part swap -- it
  -- drops whatever it was tracking, so the approach speech would re-announce while the
  -- machine sat still.
  local same = (implVehID == pid) and (implName == newName)
           and ((implCids ~= nil) == (newCids ~= nil))
  if same and implCids and newCids then
    same = (#implCids == #newCids)
    if same then
      for i = 1, #newCids do
        if implCids[i] ~= newCids[i] then same = false; break end
      end
    end
  end

  implVehID = pid
  implCids = newCids
  implName = newName
  implSampleCids = e and e.sample or nil

  if not same then
    if newCids then
      ipLog('I', string.format("implement '%s' active on vehicle %s: %d nodes",
        tostring(newName), tostring(pid), #newCids))
    else
      ipLog('I', string.format("no implement active on vehicle %s", tostring(pid)))
    end
    lastSentName, nameEverSent = nil, false
  end
end

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

  if #cids > 0 then
    implByVeh[vehID] = {cids = cids, sample = sample, name = friendlyName}
  else
    implByVeh[vehID] = false  -- "asked and has none", distinct from "never heard from"
  end
  applyActivePush()
end

-- How far each of the machine's hydraulic groups has run out of its own stroke, pushed by
-- 796F6C6F313035.lua at 5 Hz and only when the rounded figures change. The payload is opaque
-- here and is forwarded verbatim: this extension knows where the ramp is, the vehicle VM knows
-- where the rams are, and neither needs to learn the other's job. An EMPTY string is a real
-- answer -- "this machine has no hydraulics" -- and must not be confused with never having
-- heard from the VM at all, which is what the nil default already means.
function M.onRampHydraulics(vehID, payload)
  rampHydByVeh[vehID] = tostring(payload or "")
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
    local function midOf(first, last, endsOnly)
      local acc, n = vec3(0, 0, 0), 0
      for i = first, last do
        if not (endsOnly and i ~= first and i ~= last) then
          acc = acc + (base + vec3(player:getNodePosition(implSampleCids[i])))
          n = n + 1
        end
      end
      return acc / n
    end
    -- The origin is the MIDPOINT of edgeL/edgeR, not the mean of all three edge picks.
    -- edgeC is a real contact point but it is not a centre: on an implement with nothing in
    -- the middle -- forks -- it lands on the inner face of one tine, so averaging it in
    -- pulls the origin a fifth of a tine spacing off the centreline and short of the tips.
    -- It stays in getImplementPoints, the narrow-phase sweep and the clearance sets, where
    -- being a contact point is the whole of what is asked of it.
    local edge = midOf(1, 3, true)   -- cutting edge / tine tips: (edgeL + edgeR) / 2
    local heel = midOf(4, 5)         -- the implement's rear-bottom

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
      -- The lateral baseline is length-guarded above; this operand was not, and it is half
      -- of the same sign test. Flattening the machine's heading collapses it to numerical
      -- residue whenever the machine pitches near vertical -- nose-down into a pile, or
      -- jacked up on its own bucket -- and the hemisphere test then picks a side at random
      -- per tick. It does not degrade gracefully: negating fwd negates the derived left
      -- vector, so the bearing MIRRORS. Hold the previous heading instead of guessing.
      if veh:length() < 0.2 then
        if lastGoodFwd then
          return {pos = edge, heel = heel, fwd = lastGoodFwd, name = implName}
        end
        return nil
      end
      if fwd:dot(veh) < 0 then fwd = -fwd end
    else
      -- Degenerate lateral axis (a narrow implement where both edge picks collapsed onto
      -- the centre). Fall back to the along-axis, which is better than nothing.
      fwd = edge - heel
      fwd.z = 0
    end
    if fwd:length() < 1e-3 then return nil end
    lastGoodFwd = fwd:normalized()
    -- Origin is the cutting edge, not the implement centroid: that is the part of the
    -- machine you are actually trying to put somewhere, so a distance of zero means
    -- "touching" rather than "half a bucket short".
    -- heel rides along so the entry gate can measure the floor plane's world pitch and the
    -- tine length without a second round of node reads.
    return {pos = edge, heel = heel, fwd = fwd:normalized(), name = implName}
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
--
-- The lateral vector MUST be up:cross(fwd), matching vehicleGeometry's VEH_SCRIPT and its own
-- boxFrame. measureTargetCached takes minR/maxR straight out of that cache, so the two have to
-- agree and nothing enforces it but this comment: fwd:cross(up) is the negation, which mirrors
-- every lateral test against a cached target. It fails in the nastiest possible way -- the
-- strided fallback measures with whatever frame it is handed and so stays self-consistent, so
-- the containment test and the slam footprint work correctly for the first few ticks after a
-- spawn and then flip sides the moment the async resolve lands.
--
-- With fwd = +Y and up = +Z the result actually points to the driver's LEFT, so `right` and
-- vehicleGeometry's `rgt` are both misnomers. Leave them: the only property that matters is
-- that the two agree, and renaming one of them is how they would stop agreeing. Nothing
-- outside the box tests reads this vector, and the readout's own left/right convention is
-- built separately in sendDockLine.
local function boxFrame(veh)
  local ok, res = pcall(function()
    local fwd = vec3(veh:getDirectionVector()):normalized()
    local up  = vec3(veh:getDirectionVectorUp()):normalized()
    local right = up:cross(fwd)
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

-- =================================================================================================
--  Entry gate (can the tines actually go into the selected band?)
-- =================================================================================================

-- The single cue that would have ended a four-round investigation in one press. Lateral,
-- vertical and range can all be nulled perfectly and the tines still cannot enter, because a
-- tilted implement sweeps through the band's whole thickness after a few centimetres of
-- travel and hits the far side. It is pure trigonometry on numbers already resolved, so it
-- costs nothing: the floor plane's angle against the band's thickness.
--
-- Reported as a DEPTH rather than as a maximum angle, because depth is the thing that is
-- actually true of the manoeuvre -- "the tines go in 15 centimetres" is directly actionable,
-- where "you are 12 degrees over the limit for this band" needs the operator to do the
-- geometry the mod has already done.
local IMPL_ENTRY_MIN_DEPTH_M   = 0.40  -- tine engagement below this cannot carry a load
local IMPL_ENTRY_EXIT_DEPTH_M  = 0.34  -- ...and how far back through it must fall to be lost
local IMPL_ENTRY_LEVEL_DEG     = 1.0   -- flatter than this and the tine enters its full length
local entryOK = false

-- theta: world pitch of the floor plane, degrees, edge above heel positive.
-- L:     tine length, edgeMid -> heelMid.
-- Returns depth in metres and the enterable flag (hysteretic, so a machine breathing on its
-- suspension at the threshold cannot chatter -- the same reason SLAM_CLEAR_* is asymmetric).
local function resolveEntry(frame, band)
  if not (frame and frame.heel and band) then
    entryOK = false
    return nil, nil, false
  end
  local L = frame.pos:distance(frame.heel)
  if L < 1e-3 then
    entryOK = false
    return nil, nil, false
  end
  local dz = frame.pos.z - frame.heel.z
  local theta = math.deg(math.asin(math.max(-1, math.min(1, dz / L))))
  local T = math.max(0.0, band.hiZ - band.loZ)

  local depth
  if math.abs(theta) < IMPL_ENTRY_LEVEL_DEG then
    depth = L
  else
    -- The tine climbs (or dives) T metres of band thickness over T / sin|theta| of travel,
    -- and cannot in any case go in further than it is long.
    depth = math.min(L, T / math.sin(math.rad(math.abs(theta))))
  end

  local threshold = entryOK and IMPL_ENTRY_EXIT_DEPTH_M or IMPL_ENTRY_MIN_DEPTH_M
  entryOK = depth >= threshold
  return theta, depth, entryOK
end

local function insideBox(p, frame, box)
  local d = p - frame.c
  local f, r, u = frame.f:dot(d), frame.r:dot(d), frame.u:dot(d)
  return f >= box.minF and f <= box.maxF
     and r >= box.minR and r <= box.maxR
     and u >= box.minU and u <= box.maxU
end

-- =================================================================================================
--  Ground truth dump (diagnostic)
-- =================================================================================================

-- Everything the docking readout is built from, as text, run from the accessible console:
--
--   return extensions.implementProximity.dockTruth()
--
-- WHY IT LIVES HERE rather than in diagnostic/. Two reasons, and the second is the real one.
-- The accessible console's input is a single-line control and EXEC carries one datagram, so a
-- pasted multi-line chunk arrives as one line -- at which point the first "--" comments out
-- everything after it and the whole thing evaluates to nothing, silently. Only a one-liner is
-- reliably runnable from the seat.
--
-- And a diagnostic that re-derives the band selection, the node set or the frame is a SECOND
-- implementation. It can tell you that two pieces of code disagree; it cannot tell you which
-- one the machine is obeying, which is the only question worth asking here. In here it calls
-- the same resolveBand, the same insideBox, the same sample cids that produced the reading
-- being questioned.
--
-- WHAT TO READ FIRST, given "I cannot get the forks under a car":
--   BANDS        -- the reference the vertical is nulling against. A SOLID pick whose middle
--                   sits near half the car's height means the instrument is correctly telling
--                   you to raise the tines into the door. Readout wrong, not operator.
--   OCCUPANCY    -- the raw bins behind that. A void that is real but sparsely noded can be
--                   classified SOLID and vanish from the band list; this is the evidence.
--   RANGE WINNER -- which two nodes produce the range you hear. A wheel winning means "zero"
--                   is a tyre with the sill still ahead of it, and pushing on shoves the car.
--   IMPLEMENT    -- tilt, and how far the tine root sits above the tip. The readout measures
--                   the cutting edge only, so this is the one axis it cannot currently see.
function M.dockTruth()
  local out = {}
  local function p(fmt, ...)
    if select('#', ...) == 0 then out[#out + 1] = tostring(fmt)
    else out[#out + 1] = string.format(fmt, ...) end
  end
  local function finish() return table.concat(out, "\n") end

  local geo = extensions and extensions.vehicleGeometry or nil
  if not geo then return "vehicleGeometry not loaded" end
  local player = be:getPlayerVehicle(0)
  if not player then return "no player vehicle" end

  -- core_terrain is the nil-returning height source (unlike the vehicle VM's
  -- getSurfaceHeightBelow, which returns a huge negative), so every ground figure below is
  -- optional and the dump still reads on a terrain-less map such as smallgrid.
  local function groundAt(pos)
    if not (core_terrain and core_terrain.getTerrainHeight) then return nil end
    local ok, h = pcall(core_terrain.getTerrainHeight, pos)
    if ok and type(h) == "number" then return h end
    return nil
  end
  local function agl(z, gz)
    if not gz then return "n/a" end
    return string.format("%+.3f", z - gz)
  end
  local SAMPLE_NAMES = {"edgeL", "edgeC", "edgeR", "heelL", "heelR"}

  p("=== DOCK TRUTH ===")
  p("player vehicle %d (%s)", player:getID(), tostring(player:getJBeamFilename()))
  p("implement '%s' pushed by vehicle %s, %d cids, %s sample set",
    tostring(implName), tostring(implVehID), implCids and #implCids or 0,
    implSampleCids and "5-point" or "MISSING")
  p("dock mode %s, band %s, slam %s, classified as %s",
    dockActive and "ON" or "OFF",
    bandIndex and ("MANUAL " .. bandIndex .. " on target " .. tostring(bandTargetID)) or "auto",
    tostring(slamState),
    implementIsFork() and "FORKS (auto-selects lowest void)"
                       or "BUCKET (auto-selects tallest face)")

  if not (implCids and implSampleCids) then
    p("")
    p("No implement resolved for this vehicle -- nothing below can be computed.")
    return finish()
  end

  -- ---- implement -------------------------------------------------------------------------
  local base = vec3(player:getPosition())
  local function samplePt(i) return base + vec3(player:getNodePosition(implSampleCids[i])) end
  local eL, eC, eR = samplePt(1), samplePt(2), samplePt(3)
  local hL, hR     = samplePt(4), samplePt(5)
  local edge = (eL + eC + eR) / 3
  local heel = (hL + hR) / 2
  local along = edge - heel
  local alongH = math.sqrt(along.x * along.x + along.y * along.y)
  -- Positive = tips below the heel, i.e. tilted forward and down. That is the state in which
  -- the readout's edge origin can sit at exactly the right height while the tines themselves
  -- are not enterable, so the sign is chosen to make that the loud case.
  local tiltDeg = (alongH > 1e-4) and math.deg(math.atan2(-along.z, alongH)) or 0
  local gEdge = groundAt(edge)

  p("")
  p("--- IMPLEMENT ---")
  p("edge (tine tips)  world z %.3f  above ground %s", edge.z, agl(edge.z, gEdge))
  p("heel (tine roots) world z %.3f  above ground %s", heel.z, agl(heel.z, gEdge))
  p("tine reach (horizontal) %.3f m, edge width %.3f m", alongH, (eR - eL):length())
  p("TILT %+.1f deg (positive = tips down; tines enter a pocket only near 0)", tiltDeg)
  p("  -> the tine root sits %.3f m higher than the tip", math.max(0, heel.z - edge.z))
  p("  -> so a pocket shorter than that cannot be entered, however well the tip is aimed")

  -- ---- target ----------------------------------------------------------------------------
  local playerID = player:getID()
  local bestObj, bestD = nil, math.huge
  for i = 0, be:getObjectCount() - 1 do
    local obj = be:getObject(i)
    if obj and obj:getID() ~= playerID then
      local ok, d = pcall(function() return edge:distance(vec3(obj:getPosition())) end)
      if ok and d and d < bestD then bestObj, bestD = obj, d end
    end
  end
  if not bestObj then
    p("")
    p("No other object in the scene.")
    return finish()
  end

  local tid = bestObj:getID()
  geo.request(tid)
  local entry = geo.get(tid)
  local frame = geo.boxFrame(tid)
  local gTgt = groundAt(vec3(bestObj:getPosition()))

  p("")
  p("--- TARGET ---")
  p("nearest object id %d (%s), centre %.2f m from the tine tips",
    tid, tostring(bestObj:getJBeamFilename()), bestD)
  if not (entry and frame) then
    p("geometry NOT RESOLVED -- the readout is on the box/origin fallback right now.")
    p("Run again in a few seconds; if it never resolves, grep the game log for vehicleGeometry.")
    return finish()
  end
  local e = entry.ext
  p("extents (own frame): fore/aft %.2f m, lateral %.2f m, vertical %.2f m",
    e.maxF - e.minF, e.maxR - e.minR, e.maxU - e.minU)
  p("hull nodes cached: %d", entry.hull and #entry.hull or 0)

  -- Live vertical envelope from the hull nodes: the honest "how high off the ground is the
  -- bottom of this thing", independent of the histogram that the band list depends on.
  local hullPts, tMinZ, tMaxZ = {}, math.huge, -math.huge
  local tBase = vec3(bestObj:getPosition())
  for _, cid in ipairs(entry.hull or {}) do
    local ok, q = pcall(function() return tBase + vec3(bestObj:getNodePosition(cid)) end)
    if ok and q then
      hullPts[#hullPts + 1] = q
      if q.z < tMinZ then tMinZ = q.z end
      if q.z > tMaxZ then tMaxZ = q.z end
    end
  end
  if #hullPts > 0 then
    p("hull z span %.3f .. %.3f (above ground %s .. %s)",
      tMinZ, tMaxZ, agl(tMinZ, gTgt), agl(tMaxZ, gTgt))
  end

  -- ---- bands: what the vertical is nulling against ----------------------------------------
  p("")
  p("--- BANDS (the vertical reference) ---")
  local okB, bands, whyB = pcall(geo.bands, tid)
  -- Hoisted out of the block below so the entry-gate section can report against the SAME band
  -- the vertical is nulling to; measuring the two against different references would make a
  -- disagreement between them impossible to interpret.
  local selectedBand = nil
  if not okB or not bands then
    p("no bands: %s", tostring(whyB or "bands threw"))
  else
    -- The real selector, not a copy of it. If this disagrees with what F9+I speaks, the bug is
    -- in the readout rather than in here, which is exactly the distinction worth preserving.
    --
    -- Saved and restored around the call because resolveBand MUTATES: it drops a manual pick
    -- back to auto whenever the target id differs from the one the pick was made against, and
    -- the nearest object to the tine tips is not necessarily the one the readout has locked.
    -- A diagnostic that silently discards the band the operator chose would be changing the
    -- very state it was run to explain.
    local savedIdx, savedTgt = bandIndex, bandTargetID
    local usedBand, usedIdx, count, whyR = resolveBand(tid)
    bandIndex, bandTargetID = savedIdx, savedTgt
    selectedBand = usedBand
    for i, b in ipairs(bands) do
      local mid = (b.loZ + b.hiZ) * 0.5
      p("%s %d/%d %-5s z %.3f..%.3f thick %.3f mid %.3f (agl %s) vertical %+.3f",
        (i == usedIdx) and "->" or "  ", i, #bands, b.kind, b.loZ, b.hiZ,
        b.hiZ - b.loZ, mid, agl(mid, gTgt), mid - edge.z)
    end
    if not usedBand then
      p("resolveBand returned nothing: %s", tostring(whyR))
    else
      local umid = (usedBand.loZ + usedBand.hiZ) * 0.5
      p("using band %d of %d (%s)", usedIdx, count, bandIndex and "manual" or "auto")
      p("=> instrument says %s %.3f m, putting the TINE TIPS at z %.3f",
        (umid - edge.z) >= 0 and "RAISE" or "LOWER", math.abs(umid - edge.z), umid)
      if gTgt then p("=> that is %.3f m above the ground the target sits on", umid - gTgt) end
      if usedBand.kind ~= "GAP" then
        p("!! reference is SOLID: nulling the vertical aims the tips AT material, not a void")
      end
    end
  end

  -- Raw bins. The collapser calls a bin empty below 25%% of the peak, so a genuine void that
  -- happens to be sparsely noded -- the air under a sill, with bodywork above and wheels on
  -- either side -- can be classified SOLID and disappear from the list above. Printing the
  -- verdict alone would hide exactly the failure this dump exists to catch.
  if entry.hist then
    local peak = 0
    for i = 1, #entry.hist do if entry.hist[i] > peak then peak = entry.hist[i] end end
    local lo, span = entry.histLo, entry.histHi - entry.histLo
    local fMid, rMid = (e.minF + e.maxF) * 0.5, (e.minR + e.maxR) * 0.5
    local function zAt(u)
      return (frame.c + frame.f * fMid + frame.r * rMid + frame.u * (lo + span * u)).z
    end
    p("")
    p("--- VERTICAL OCCUPANCY (peak %d nodes, GAP below %.1f) ---", peak, peak * 0.25)
    local n = #entry.hist
    for i = 1, n do
      local zMid, zLo, zHi = zAt((i - 0.5) / n), zAt((i - 1) / n), zAt(i / n)
      local here = ""
      if (edge.z >= math.min(zLo, zHi)) and (edge.z < math.max(zLo, zHi)) then
        here = "  <== tine tips at this height"
      end
      p("bin %2d z %.3f (agl %s) nodes %4d %s%s",
        i, zMid, agl(zMid, gTgt), entry.hist[i],
        (entry.hist[i] < peak * 0.25) and "GAP" or "SOLID", here)
    end
  end

  -- ---- range: which two nodes produce the number you hear ---------------------------------
  p("")
  p("--- RANGE WINNER ---")
  local implPts = {}
  for _, cid in ipairs(implCids) do
    local ok, q = pcall(function() return base + vec3(player:getNodePosition(cid)) end)
    if ok and q then implPts[#implPts + 1] = {cid = cid, p = q} end
  end
  if #hullPts == 0 or #implPts == 0 then
    p("not enough points (implement %d, hull %d)", #implPts, #hullPts)
  else
    local wD, wI, wT = math.huge, nil, nil
    for _, a in ipairs(implPts) do
      for _, b in ipairs(hullPts) do
        local d = a.p:squaredDistance(b)
        if d < wD then wD, wI, wT = d, a, b end
      end
    end
    wD = math.sqrt(wD)
    local which = " -- NOT one of the five sample points"
    for i = 1, 5 do
      if implSampleCids[i] == wI.cid then which = " -- this is " .. SAMPLE_NAMES[i]; break end
    end
    local d = wT - frame.c
    local tf, tr, tu = frame.f:dot(d), frame.r:dot(d), frame.u:dot(d)
    p("range %.3f m -- measured over the FULL implement cloud, in 3D, in every direction", wD)
    p("winning implement node cid %d, z %.3f (agl %s)%s", wI.cid, wI.p.z, agl(wI.p.z, gEdge), which)
    p("winning target node z %.3f (agl %s), target frame fore %+.3f lateral %+.3f up %+.3f",
      wT.z, agl(wT.z, gTgt), tf, tr, tu)
    p("  target spans fore %.3f..%.3f lateral %.3f..%.3f up %.3f..%.3f",
      e.minF, e.maxF, e.minR, e.maxR, e.minU, e.maxU)
    local outerR = math.max(math.abs(e.minR), math.abs(e.maxR))
    if gTgt and (wT.z - gTgt) < 0.45 and outerR > 0 and (math.abs(tr) / outerR) > 0.7 then
      p("!! winning target point is LOW and near the outer edge -- almost certainly a WHEEL.")
      p("!! 'zero range' then means tip against tyre, with the sill still ahead of it.")
    end
  end

  -- ---- is the five-point sample set actually at the extremes? -----------------------------
  --
  -- Everything the readout says about position is measured from the mean of edgeL/C/R, on the
  -- stated assumption that those three points ARE the cutting edge or the tine tips. Nothing
  -- checks it. They are picked once from design-space nd.pos by "forward and low", and if that
  -- pick lands on the carriage instead of the tines, the docking origin sits somewhere in the
  -- middle of the implement while the RANGE -- which sweeps the full cloud -- keeps reporting
  -- from the real tips. The readout is then internally inconsistent in the one way that is
  -- impossible to notice from the seat: range says zero exactly when the tips touch, while
  -- lateral and vertical are nulling a point that is somewhere else entirely.
  p("")
  p("--- SAMPLE SET vs FULL CLOUD ---")
  local fr = M.getImplementFrame()
  if not fr or #implPts == 0 then
    p("no implement frame available")
  else
    local fwd = fr.fwd
    local left = vec3(0, 0, 1):cross(fwd)
    if left:length() > 1e-4 then left = left:normalized() end
    local function proj(q)
      local d = q - base
      return fwd:dot(d), left:dot(d), q.z
    end
    local minA, maxA = math.huge, -math.huge
    local minZ2, maxZ2 = math.huge, -math.huge
    local fwdMost, lowMost = nil, nil
    for _, a in ipairs(implPts) do
      local f2, _, z2 = proj(a.p)
      if f2 > maxA then maxA, fwdMost = f2, a end
      if f2 < minA then minA = f2 end
      if z2 < minZ2 then minZ2, lowMost = z2, a end
      if z2 > maxZ2 then maxZ2 = z2 end
    end
    p("cloud of %d nodes: along-heading %.3f..%.3f m, world z %.3f..%.3f",
      #implPts, minA, maxA, minZ2, maxZ2)
    for i = 1, 5 do
      local f2, l2, z2 = proj(samplePt(i))
      p("  %-5s along %+.3f (%.3f m short of the foremost node), left %+.3f, z %.3f",
        SAMPLE_NAMES[i], f2, maxA - f2, l2, z2)
    end
    local ffA, _, ffZ = proj(fwdMost.p)
    local flA, _, flZ = proj(lowMost.p)
    p("foremost node cid %d at along %+.3f, z %.3f", fwdMost.cid, ffA, ffZ)
    p("lowest node   cid %d at along %+.3f, z %.3f", lowMost.cid, flA, flZ)
    p("=> if the foremost and lowest nodes are not among edgeL/C/R, the docking origin is not")
    p("=> the tip, and the vertical null refers to a point the tines are not at.")

    -- Longitudinal profile of the cloud, in the implement's own frame.
    --
    -- Five single picks cannot show SHAPE, which is the thing actually in question: whether
    -- "forward and low" in design space landed on the tines or on the top corners of the
    -- carriage. Slicing along the heading and reporting how LOW each slice reaches traces the
    -- underside of the implement, and the underside is where tines are and where a backplate
    -- is not. The lowest cid per slice is printed because those are the candidate replacements
    -- for the edge picks if the current ones turn out to be on the frame.
    local NSLICE = 10
    local span = maxA - minA
    if span > 1e-3 then
      local slices = {}
      for i = 1, NSLICE do
        slices[i] = {n = 0, minZ = math.huge, maxZ = -math.huge,
                     minL = math.huge, maxL = -math.huge, minCid = -1}
      end
      for _, a in ipairs(implPts) do
        local f2, l2, z2 = proj(a.p)
        local k = math.floor((f2 - minA) / span * NSLICE) + 1
        if k < 1 then k = 1 end
        if k > NSLICE then k = NSLICE end
        local s = slices[k]
        s.n = s.n + 1
        if z2 < s.minZ then s.minZ, s.minCid = z2, a.cid end
        if z2 > s.maxZ then s.maxZ = z2 end
        if l2 < s.minL then s.minL = l2 end
        if l2 > s.maxL then s.maxL = l2 end
      end
      p("longitudinal profile, rear of the implement to front:")
      for i = 1, NSLICE do
        local s = slices[i]
        if s.n > 0 then
          p("  along %+.2f..%+.2f %3d nodes  z %.3f..%.3f  left %+.2f..%+.2f  lowest cid %d",
            minA + span * (i - 1) / NSLICE, minA + span * i / NSLICE,
            s.n, s.minZ, s.maxZ, s.minL, s.maxL, s.minCid)
        end
      end
    end
  end

  -- ---- the 'under it' gate ----------------------------------------------------------------
  p("")
  p("--- 'UNDER IT' GATE ---")
  local box = {minF = e.minF, maxF = e.maxF, minR = e.minR, maxR = e.maxR,
               minU = e.minU, maxU = e.maxU}
  local nInside = 0
  for i = 1, 5 do
    local q = samplePt(i)
    local d = q - frame.c
    local within = insideBox(q, frame, box)
    if within then nInside = nInside + 1 end
    p("  %-5s fore %+.3f lateral %+.3f up %+.3f  %s", SAMPLE_NAMES[i],
      frame.f:dot(d), frame.r:dot(d), frame.u:dot(d), within and "INSIDE" or "outside")
  end
  local implCentre = vec3(0, 0, 0)
  for _, a in ipairs(implPts) do implCentre = implCentre + a.p end
  implCentre = implCentre / math.max(1, #implPts)
  local boxMidZ = (tMinZ + tMaxZ) * 0.5
  p("points inside %d (needs >= %d)", nInside, INSIDE_MIN_PTS)
  p("implement centroid z %.3f vs box mid z %.3f -> %s", implCentre.z, boxMidZ,
    (implCentre.z < boxMidZ) and "below, OK" or "ABOVE, gate blocked")
  p("contact threshold is %.2f m, but the gate no longer requires clearing it: 'under it'", CONTACT_M)
  p("and 'touching it' are independent, so tines resting against the underbody report both.")

  -- ---- the entry gate ----------------------------------------------------------------------
  p("")
  p("--- ENTRY GATE ---")
  if not fr or not fr.heel then
    p("no implement frame, cannot measure the floor plane")
  else
    local L = fr.pos:distance(fr.heel)
    local theta = 0.0
    if L > 1e-3 then
      theta = math.deg(math.asin(math.max(-1, math.min(1, (fr.pos.z - fr.heel.z) / L))))
    end
    p("floor plane edgeMid->heelMid: length %.3f m, world pitch %+.1f deg", L, theta)
    local bandNow = selectedBand
    if not bandNow then
      p("no reference band selected, so there is nothing to enter")
    else
      local T = math.max(0.0, bandNow.hiZ - bandNow.loZ)
      local depth = L
      if math.abs(theta) >= IMPL_ENTRY_LEVEL_DEG then
        depth = math.min(L, T / math.sin(math.rad(math.abs(theta))))
      end
      p("band thickness %.3f m -> usable insertion depth %.3f m (needs >= %.2f)",
        T, depth, IMPL_ENTRY_MIN_DEPTH_M)
      p("=> %s", (depth >= IMPL_ENTRY_MIN_DEPTH_M) and "ENTERABLE"
        or "TOO STEEP: the tines hit the far side of the band before they are in")
    end
  end

  return finish()
end

-- Park the free camera on the implement, from a named angle.
--
--   extensions.implementProximity.dockCam("side")   -- broadside, at tine height
--   extensions.implementProximity.dockCam("front")  -- straight down the heading
--   extensions.implementProximity.dockCam("top")    -- plan view
--   extensions.implementProximity.dockCam("side", 4) -- closer
--
-- This exists because the person who needs the screenshot is the person who cannot aim the
-- camera. Asking a blind operator to guess at a viewpoint and then guess again at whether it
-- showed anything is a loop that costs a session per useful image. The implement's own frame
-- is already resolved every tick, so a view relative to it -- broadside at tine height, which
-- is exactly the profile a tilt question needs -- is arithmetic, not aim.
--
-- Every engine call is wrapped: this is a debugging aid, and a camera API that has moved
-- between versions must report that plainly rather than throwing inside the console.
function M.dockCam(which, dist)
  which = tostring(which or "side"):lower()
  dist = tonumber(dist) or 7.0

  local player = be:getPlayerVehicle(0)
  if not player then return "no player vehicle" end
  if not implCids then return "no implement resolved" end
  local fr = M.getImplementFrame()
  if not fr then return "no implement frame" end

  -- Aim at the centroid of the whole implement, not at the cutting edge: the point of these
  -- views is to see the implement's SHAPE against its frame, so it wants to be in the middle
  -- of the picture rather than at one edge of it.
  local base = vec3(player:getPosition())
  local c, n = vec3(0, 0, 0), 0
  for _, cid in ipairs(implCids) do
    local ok, q = pcall(function() return base + vec3(player:getNodePosition(cid)) end)
    if ok and q then c = c + q; n = n + 1 end
  end
  if n == 0 then return "no implement node positions" end
  c = c / n

  local fwd = fr.fwd
  local left = vec3(0, 0, 1):cross(fwd)
  if left:length() < 1e-4 then return "degenerate implement heading" end
  left = left:normalized()

  local eye, upHint
  if which == "side" then
    eye, upHint = c + left * dist, vec3(0, 0, 1)
  elseif which == "front" then
    eye, upHint = c + fwd * dist, vec3(0, 0, 1)
  elseif which == "top" then
    -- Looking straight down, world up is parallel to the view direction and cannot orient the
    -- roll. Use the implement heading, which also makes "up the screen" mean "forward".
    eye, upHint = c + vec3(0, 0, dist), fwd
  else
    return "unknown view '" .. which .. "' (want side, front or top)"
  end

  local dir = c - eye
  if dir:length() < 1e-4 then return "camera would sit on the target" end
  dir = dir:normalized()

  local ok, err = pcall(function()
    commands.setFreeCamera()
    local q = quatFromDir(dir, upHint)
    core_camera.setPosRot(0, eye.x, eye.y, eye.z, q.x, q.y, q.z, q.w)
  end)
  if not ok then return "camera call failed: " .. tostring(err) end

  return string.format(
    "free camera at %.2f, %.2f, %.2f looking at the implement centroid %.2f, %.2f, %.2f (%s view, %.1f m)",
    eye.x, eye.y, eye.z, c.x, c.y, c.z, which, dist)
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

    -- Entry gate. Appended at the END of the line, so an older Python half simply parses the
    -- eleven fields it knows and ignores these -- the same contract the scanner packet's
    -- optional fourth field already uses.
    local theta, depth = resolveEntry(frame, band)

    -- Two more optional-tail fields, on the same contract: the MODE, so Python knows what unit
    -- the null channel is in and which wording to use, and the ramp-mode width margin, which is
    -- -1 here because an implement approach has no such thing. Mode is last-but-one rather than
    -- first because moving an existing field would break the very compatibility the tail exists
    -- to provide; a Python half older than this simply parses thirteen and assumes IMPL, which
    -- is exactly right, since a mod older than this has no other mode to be in.
    send(string.format("DOCK:%s,%.3f,%.3f,%.3f,%d,%d,%s,%.3f,%.3f,%.1f,%d,%.1f,%.3f,%s,%.3f",
      best.name, best.d, lateral, vertical, idx, count, band.kind,
      band.loZ, band.hiZ, yaw, (bandIndex ~= nil) and 1 or 0,
      theta or 0.0, depth or -1.0, "IMPL", -1.0))
    lastDockLine = "DOCK"
  end)
  if not ok then
    ipLog('E', "dock readout threw: " .. tostring(err))
    dockFail("readout error")
  end
end

-- =================================================================================================
--  Ramp mode
--
--  The instrument's second answer. Everything above measures FROM an implement TO a load; this
--  measures from the vehicle you are driving to the drive-in mouth of a ramp on some other
--  machine. It is a different answer, not a different instrument: it reduces to the same three
--  scalars, rides the same DOCK: line, and is reached through the same toggle -- so there is no
--  second mode key to remember and no second set of tones to learn.
-- =================================================================================================

-- The business end of a machine with no implement: the centroid of its own front contact band.
-- This is precisely the fallback vehicleScanner already uses when there is no implement
-- override, reused rather than reinvented so that the ramp readout and the scanner bearing
-- measure from the same point on the same vehicle.
--
-- Forward band unconditionally, never the scanner's activeDirection: you drive forwards into a
-- cannon. Reversing into one is not a manoeuvre this instrument is for, and making the origin
-- follow the gear would swap the reference mid-approach every time the driver rocked out of a
-- bad line.
local function dockOriginFrame(player)
  local geo = extensions and extensions.vehicleGeometry or nil
  if not (geo and geo.contactPoints) then return nil, "vehicleGeometry unavailable" end
  local okP, pts = pcall(geo.contactPoints, player:getID(), 1)
  if not (okP and pts and #pts > 0) then return nil, "no contact points for your vehicle" end

  local c = vec3(0, 0, 0)
  for _, p in ipairs(pts) do c = c + p end
  c = c / #pts

  local fwd = vec3(player:getDirectionVector())
  fwd.z = 0
  -- Same guard, and the same reason, as getImplementFrame's: flattening a heading collapses it
  -- to numerical residue when the machine is pitched near vertical, and a heading picked from
  -- residue flips per tick. Holding the last good one is the only graceful failure available,
  -- because negating fwd negates the derived left vector and MIRRORS the readout.
  if fwd:length() < 0.2 then
    if not lastGoodFwd then return nil, "vehicle heading is degenerate" end
    fwd = lastGoodFwd
  else
    fwd = fwd:normalized()
    lastGoodFwd = fwd
  end

  local left = vec3(0, 0, 1):cross(fwd)
  if left:length() < 1e-4 then return nil, "degenerate vehicle heading" end
  return {pos = c, fwd = fwd, left = left:normalized()}
end

-- Nearest vehicle with a resolvable ramp. rampGeometry.request is a no-op for anything already
-- cached, pending or known hopeless, so an ordinary car in the scene costs one cross-VM round
-- trip for the whole session and nothing thereafter.
local function findRampTarget(player, originPos)
  local rg = extensions and extensions.rampGeometry or nil
  if not (rg and rg.mouthFrame) then return nil, "rampGeometry unavailable" end
  local playerID = player:getID()

  local best, bestD, seen = nil, math.huge, 0
  -- The nearest object that did NOT produce a mouth, and why. "No ramp near you" has half a
  -- dozen causes that are identical from the seat -- the machine is out of range, its VM has
  -- not answered yet, it answered and has no ramp nodes, the resolve gave up three chunks ago
  -- -- and reporting only the count of objects seen distinguishes none of them. Which one it
  -- is decides whether the driver should reverse, wait, or stop looking.
  local nearMiss, nearMissD = nil, math.huge
  for i = 0, be:getObjectCount() - 1 do
    local obj = be:getObject(i)
    if obj and obj:getID() ~= playerID then
      local okD, d = pcall(function() return originPos:distance(vec3(obj:getPosition())) end)
      if okD and d and d < RAMP_SEARCH_M then
        seen = seen + 1
        local id = obj:getID()
        pcall(rg.request, id)
        local okF, mouth = pcall(rg.mouthFrame, id)
        if okF and mouth then
          local md = originPos:distance(mouth.centre)
          if md < bestD then
            bestD = md
            best = {id = id, veh = obj, name = nameOf(obj), mouth = mouth}
          end
        elseif d < nearMissD then
          nearMissD = d
          nearMiss = {id = id, name = nameOf(obj), dist = d}
        end
      end
    end
  end
  if not best then
    -- Two reasons, deliberately: the first is SPOKEN on every F9+I press and gets one clause,
    -- the second goes to rampTruth and gets the whole diagnosis. Handing the long one to speech
    -- makes the key useless -- it is pressed while manoeuvring, and "no ramp among 1 objects
    -- within 70 metres, nearest is Old Cannon white at 5 metres, gave up, only 0 ramp nodes" is
    -- a paragraph read out over the approach it is supposed to be helping with.
    if nearMiss then
      local short, long = "no state recorded", "no state recorded"
      if rg.shortStateOf then
        local okS, sv = pcall(rg.shortStateOf, nearMiss.id)
        if okS and sv then short = sv end
      end
      if rg.stateOf then
        local okL, lv = pcall(rg.stateOf, nearMiss.id)
        if okL and lv then long = lv end
      end
      return nil,
        -- Naming the machine you are standing next to is the useful half: "no ramp nearby" on
        -- its own leaves you wondering whether the mod can see the thing in front of you at all.
        string.format("nearest is %s, %.0f metres, %s", nearMiss.name, nearMiss.dist, short),
        string.format("no ramp among %d objects within %d metres; nearest is %s at %.0f m -- %s",
          seen, RAMP_SEARCH_M, nearMiss.name, nearMiss.dist, long)
    end
    local none = string.format("nothing within %d metres has a ramp", RAMP_SEARCH_M)
    return nil, none, string.format("no ramp among %d objects within %d metres",
      seen, RAMP_SEARCH_M)
  end
  return best
end

-- The three continuous channels plus the two things spoken rather than sonified.
local function rampMeasure(origin, mouth, playerHalfW)
  local d = mouth.centre - origin.pos

  -- Distance measured ALONG the ramp axis, not straight-line to the mouth centre. Straight-line
  -- would inflate with lateral offset -- drifting sideways would read as backing away -- and it
  -- would bottom out at the offset rather than at zero.
  --
  -- SIGNED, and it must stay that way. It used to be floored at zero, on the reasoning that
  -- crossing the mouth plane should read as arrived rather than as going negative. That is true
  -- of the last half metre and false of everything else: the negative half-space is not "just
  -- past the mouth", it is the ENTIRE hemisphere behind the mouth plane -- alongside the
  -- machine, behind it, anywhere in the yard on the far side. A driver circling a sixteen-metre
  -- cannon looking for its entrance spends nearly all of that time in it, and the floor pinned
  -- the range channel at 0 for the whole search: the pulse saturated, the speech said "mouth
  -- 0.0 feet", and the one number that could have said "you are past it, come back round" was
  -- the number being discarded. Consumers that want a floor apply their own.
  local range = mouth.axis:dot(d)

  -- Positive means the mouth is to the driver's LEFT, i.e. steer left. Mod-wide convention,
  -- same construction as the implement readout's.
  local lateral = mouth.left:dot(d)

  -- Squareness to the ramp axis, and deliberately NOT folded to +/-90 the way the implement's
  -- is. A pallet has no front or back as far as the tines are concerned, so folding is right
  -- there; a ramp emphatically does, and entering it backwards is not entering it. Positive
  -- means turn left to line up.
  local yaw = 0
  local c = origin.fwd:dot(mouth.axis)
  if c > 1 then c = 1 elseif c < -1 then c = -1 end
  yaw = math.deg(math.acos(c))
  if origin.left:dot(mouth.axis) < 0 then yaw = -yaw end

  -- Do you fit. -1 is the "not measured" sentinel and must never be 0, which would read as
  -- exactly touching both walls.
  local margin = -1.0
  if playerHalfW and playerHalfW > 0 then
    margin = mouth.halfW - math.abs(lateral) - playerHalfW
  end

  return range, lateral, yaw, margin, mouth.pitchDeg
end

-- Half the width of the vehicle being driven, taken as the WORSE side rather than half the
-- total. The origin is not necessarily on the vehicle's lateral centreline, and reporting
-- clearance from the roomier side is the one error here that ends with a mirror torn off.
local function playerHalfWidth(playerID)
  local geo = extensions and extensions.vehicleGeometry or nil
  if not (geo and geo.get) then return nil end
  local okG, entry = pcall(geo.get, playerID)
  if not (okG and entry and entry.ext) then return nil end
  return math.max(math.abs(entry.ext.minR), math.abs(entry.ext.maxR))
end

-- The body's own lateral centre and half-span, as opposed to the worse-side figure above. The
-- two answer different questions and the align needs both: where the car's MIDDLE is relative
-- to the reference node the teleport actually places, and how wide it is about that middle.
--
-- These are not the same number on a real vehicle and the difference is not small. An etk800
-- wagon measures minR -0.680 / maxR +1.340 -- 2.02 m wide, but with its reference node 0.33 m
-- off its own centreline. Placing that node on the ramp axis therefore puts the BODY a third
-- of a metre off centre, which in a 2.58 m mouth leaves 0.61 m on one side and -0.05 m on the
-- other. The readout then correctly announced "you do not fit" about a car with a comfortable
-- 0.28 m a side -- correct about where the align had just put it, and wrong about the car.
--
-- The worse-side rule stays exactly as it is for the live DOCK: readout, where the driver is
-- wherever they are and the margin has to be measured from the side that will hit first. It is
-- only the align, which gets to CHOOSE the lateral position, that should be centring the body.
local function playerLateralBody(playerID)
  local geo = extensions and extensions.vehicleGeometry or nil
  if not (geo and geo.get) then return nil, nil end
  local okG, entry = pcall(geo.get, playerID)
  if not (okG and entry and entry.ext) then return nil, nil end
  local lo, hi = entry.ext.minR, entry.ext.maxR
  if not (lo and hi) or hi <= lo then return nil, nil end
  return (lo + hi) * 0.5, (hi - lo) * 0.5
end

local function sendRampDockLine(player, origin, tgt)
  local mouth = tgt.mouth
  if mouth.halfW < RAMP_MIN_WIDTH_M then
    return dockFail(string.format("ramp mouth only %.2f m wide", mouth.halfW * 2))
  end
  local range, lateral, yaw, margin, pitch =
    rampMeasure(origin, mouth, playerHalfWidth(player:getID()))
  -- The feed ceiling is the IN-PLANE distance to the mouth, not the along-axis range. Two
  -- reasons, and each on its own is enough. With the along-range signed, testing it directly
  -- would leave the whole negative half-space permanently under the ceiling, so a mouth
  -- thirty-five metres behind you would feed as hard as one two metres in front. And on the
  -- approach side a driver ten metres out but eight metres off to one side is fifteen metres
  -- from the mouth, not ten -- the ceiling is meant to be "am I near this thing", which is a
  -- distance, not a projection.
  local reach = math.sqrt(range * range + lateral * lateral)
  -- Past the ceiling this NAMES ITSELF rather than going quiet. A bare DOCKCLEAR clears both
  -- the readout and the reason on the Python side, so F9+I answered "nothing in range" -- which
  -- is false and, worse, is the same thing it says when the instrument is not working at all.
  -- Ramp mode therefore never sends DOCKCLEAR: "there is no ramp" is already a DOCKFAIL, and
  -- "there is one and it is over there" is the single most useful sentence this instrument can
  -- produce for someone who cannot find it. Rounded to the metre so the dedupe still holds for
  -- most ticks.
  if reach > RAMP_REPORT_M then
    return dockFail(string.format("%s ramp mouth %.0f metres away", tgt.name, reach))
  end

  -- Field 3 (vertical) is zero and field 12 (entry depth) is -1 because ramp mode has neither.
  -- Squareness rides on the yaw field, which is where it belongs -- already signed, already in
  -- degrees, already positive-LEFT -- and it is SPOKEN rather than sonified, the same treatment
  -- the implement readout gives its own squareness and for the same reason: it matters at the
  -- instant of entry and not before. The audio side's null channel is the lateral offset, which
  -- is field 2 and needs nothing special from here.
  send(string.format("DOCK:%s,%.3f,%.3f,%.3f,%d,%d,%s,%.3f,%.3f,%.1f,%d,%.1f,%.3f,%s,%.3f",
    tgt.name, range, lateral, 0.0, 0, 0, "RAMP",
    -- -999, NEVER `pitch or 0.0`. A nil pitch means rampGeometry declined to publish one --
    -- its five chosen nodes were not collision surface, so the plane through them is structure
    -- rather than floor -- and zero there does not read as "unknown", it reads as "this ramp is
    -- dead level", which is the single most reassuring thing this instrument can say. Same
    -- sentinel and the same argument as the RAMPSELF line's.
    mouth.floorZ, mouth.floorZ, yaw, 0, pitch or -999, -1.0, "RAMP", margin))
  lastDockLine = "DOCK"
end

-- The ramp analogue of dockTruth(), and it earns its place by the same argument: this resolve's
-- failure mode is "it landed on the wrong node", which produces confident, plausible numbers
-- and is invisible to every kind of inspection except printing what it chose. Run from the
-- accessible console.
function M.rampTruth()
  local out = {}
  local function p(s) out[#out + 1] = s end
  local player = be:getPlayerVehicle(0)
  if not player then return "no player vehicle" end

  local origin, why = dockOriginFrame(player)
  if not origin then return "no origin: " .. tostring(why) end
  p(string.format("origin %.2f,%.2f,%.2f", origin.pos.x, origin.pos.y, origin.pos.z))

  local tgt, _, why2 = findRampTarget(player, origin.pos)
  if not tgt then
    -- Every object in the search radius, with what rampGeometry makes of it. This is the line
    -- that answers "why is it not seeing the cannon" without a log dive: it separates "that is
    -- not the cannon" from "the cannon is there and its resolve gave up", and names the reason
    -- in the second case.
    p(tostring(why2))
    local rg2 = extensions and extensions.rampGeometry or nil
    local playerID = player:getID()
    for i = 0, be:getObjectCount() - 1 do
      local obj = be:getObject(i)
      if obj and obj:getID() ~= playerID then
        local okD, d = pcall(function() return origin.pos:distance(vec3(obj:getPosition())) end)
        if okD and d and d < RAMP_SEARCH_M then
          local state = "rampGeometry unavailable"
          if rg2 and rg2.stateOf then
            local okS, sv = pcall(rg2.stateOf, obj:getID())
            if okS and sv then state = sv end
          end
          p(string.format("  %s [%d] at %.1f m: %s",
            nameOf(obj), obj:getID(), d, state))
        end
      end
    end
    p("retry a stuck resolve with: extensions.rampGeometry.retry(<id>)  (or retry() for all)")
    return table.concat(out, "\n")
  end

  local rg = extensions.rampGeometry
  local entry = rg.get(tgt.id)
  if entry then
    p(string.format("%s [%d]: %d ramp nodes, span %.2f m, cids %s",
      tgt.name, tgt.id, entry.nNodes, entry.alongSpan, table.concat(entry.cids, "/")))
    p(string.format("half-width %.3f m by %s (naive floor-band pick: %.3f m)",
      entry.halfW,
      (entry.wallUsed >= 2) and "wall rule"
        or string.format("wall rule on %d of 2 sides", entry.wallUsed),
      entry.naiveHalfW))
  end

  local m = tgt.mouth
  p(string.format("live mouth half-width %.3f m, floor z %.2f, pitch %s, mouth %d of %d",
    m.halfW, m.floorZ,
    m.pitchDeg and string.format("%.1f deg", m.pitchDeg)
      or "WITHHELD (chosen nodes are not collision surface)",
    m.mouthIndex or 1, m.mouthCount or 1))
  local hw = playerHalfWidth(player:getID())
  local range, lateral, yaw, margin = rampMeasure(origin, m, hw)
  -- The sign of the range is the single most useful thing this readout prints while the mouth
  -- is being hunted for: negative means you are behind the mouth plane, i.e. on the wrong side
  -- of the machine entirely, and no amount of nulling the other two channels will help until
  -- it is positive.
  p(string.format("range %+.2f m (%s), in-plane %.2f m, lateral %+.2f m (+ is LEFT), "
    .. "yaw %+.1f deg (+ is turn left)",
    range, (range >= 0) and "in front of the mouth" or "BEHIND the mouth plane",
    math.sqrt(range * range + lateral * lateral), lateral, yaw))
  p(string.format("your half-width %s, margin %s",
    hw and string.format("%.2f m", hw) or "unknown",
    (margin >= 0) and string.format("%.2f m each side", margin)
      or (hw and "NEGATIVE -- you do not fit" or "not measured")))
  return table.concat(out, " | ")
end

-- =================================================================================================
--  Ramp align teleport
--
--  The other half of the act vehicleScanner's ALIGN performs. That one places you to REVERSE
--  onto a trailer coupler; this one places you to DRIVE UP a ramp. Same key (F9 + Shift+V),
--  disambiguated by whether the docking instrument is on -- which is not a flag invented for
--  the purpose, it is the instrument you would already be running to drive onto something, and
--  it is the one that knows a ramp is there at all. A second keybind would be one more thing
--  to remember mid-manoeuvre, which is the argument the slam gate already made for riding on
--  the docking toggle rather than claiming a key.
--
--  It lives here rather than in vehicleScanner because everything it needs is already local:
--  dockActive, dockOriginFrame, findRampTarget and playerHalfWidth. Moving any of that into
--  the scanner would be a second copy of the ramp search, and the two would drift.
-- =================================================================================================

local function rampAlignFail(why)
  send("RAMPALIGN:FAIL," .. cleanName(why))
end

function M.rampAlign()
  -- The gate the whole feature hangs on. Checked here rather than only in Python so the mod is
  -- correct on its own terms: a RAMPALIGN arriving with the instrument off is a version skew,
  -- not a request to teleport somebody onto a machine they were trying to tow.
  if not dockActive then return rampAlignFail("docking instrument is off") end
  -- ...and the instrument has to be in RAMP mode, which is decided the same way scan() decides
  -- it: an implement fitted means implement mode, byte for byte. Refusing here rather than
  -- searching anyway keeps one rule for what mode the instrument is in, so the key cannot
  -- teleport you somewhere the readout was never talking about.
  if implCids then
    return rampAlignFail("implement fitted, so the instrument is not in ramp mode")
  end

  local player = be:getPlayerVehicle(0)
  if not player then return rampAlignFail("no player vehicle") end

  local ok, err = pcall(function()
    local origin, why = dockOriginFrame(player)
    if not origin then return rampAlignFail(why or "no origin") end
    -- The same search the readout runs, so what you are teleported to is by construction the
    -- machine the tones and the F9+I readout have been talking about. Its short reason is
    -- already speech-sized, which is exactly what this failure path needs.
    local tgt, why2 = findRampTarget(player, origin.pos)
    if not tgt then return rampAlignFail(why2 or "no ramp nearby") end

    local mouth = tgt.mouth
    if mouth.halfW < RAMP_MIN_WIDTH_M then
      return rampAlignFail(string.format("ramp mouth only %.2f m wide", mouth.halfW * 2))
    end

    -- Forward extent of the player's own node cloud, measured from its reference node along its
    -- own heading. Gap-trimmed by vehicleGeometry, so a bumper lying in the dirt does not buy
    -- the driver an extra three metres of standoff.
    local nose = RAMP_ALIGN_NOSE_FALLBACK_M
    local geo = extensions and extensions.vehicleGeometry or nil
    if geo and geo.get then
      local okG, entry = pcall(geo.get, player:getID())
      if okG and entry and entry.ext and entry.ext.maxF then nose = entry.ext.maxF end
    end
    local back = RAMP_ALIGN_STANDOFF_M + nose

    -- mouth.axis points INTO the ramp -- it is the direction of travel, derived in
    -- rampGeometry.mouthFrame from the inner row's displacement from the mouth row. So
    -- subtracting it walks BACK out of the mouth, and facing along it faces the driver AT the
    -- ramp. That is the opposite of vehicleScanner's align, which faces the truck AWAY from
    -- the coupler because that manoeuvre is reversed into. Negating either one is a mistake
    -- that looks perfectly reasonable in isolation, which is why both are asserted.
    -- Centre the car's BODY on the ramp axis, not its reference node. minR/maxR are measured
    -- along vehicleGeometry's lateral vector, which is up:cross(fwd) and therefore points to
    -- the driver's LEFT -- the same vector mouth.left is built from, so the offset needs no
    -- sign gymnastics: shift the placement by the body centre and the body lands on the axis.
    local bodyMid, bodyHalf = playerLateralBody(player:getID())
    local pos = mouth.centre - mouth.axis * back
    if bodyMid then pos = pos - mouth.left * bodyMid end

    -- Ground height. The player is by definition sitting on the ground right now, so its own Z
    -- is the best available reference -- the mouth's floorZ is NOT, because a tilt deck's mouth
    -- is a metre in the air until the deck is fully down. Floored against the terrain so a
    -- teleport across a dip cannot bury the vehicle; safeTeleport settles the rest.
    pos.z = player:getPosition().z + 0.3
    if core_terrain and core_terrain.getTerrainHeight then
      local okT, th = pcall(core_terrain.getTerrainHeight, pos)
      if okT and th and th > pos.z - 0.5 then pos.z = th + 0.5 end
    end

    local rot = quatFromDir(mouth.axis, vec3(0, 0, 1))
    -- checkOnlyStatics and visibilityPoint stay nil. vehicleScanner records the bug:
    -- visibilityPoint must be a vec3 because spawn.lua feeds it to getVisibilityStatus, which
    -- subtracts it from a position, so a boolean there throws inside LuaVec3.__sub and the
    -- teleport silently never happens.
    --
    -- The 8th argument is resetVehicle and MUST be false. Its default of true makes spawn.lua
    -- setPosRot + resetBrokenFlexMesh and re-place from the vehicle's INITIAL node positions,
    -- which is a respawn: all damage repaired, and the engine killed for anyone running
    -- "reset stops the engine". Lining up to drive up a ramp is a placement, not a repair --
    -- and the car about to be launched out of a cannon is exactly the one whose damage the
    -- driver came to keep. False still runs the safe-position search, the cluster move and the
    -- velocity zeroing, against the deformed body.
    spawn.safeTeleport(player, pos, rot, nil, nil, nil, false, false)

    -- A teleport is a discontinuity the readout should not have to infer. Clearing the dedupe
    -- latch makes the next tick re-send whatever it now sees rather than suppressing it as
    -- unchanged, which matters because the most likely next line is a mode or target
    -- announcement.
    lastDockLine = nil

    -- Lateral is zero by construction after this, so halfW minus your own half-width IS the
    -- clearance you will have. Reported rather than acted on: a machine that does not fit is
    -- still a machine somebody may want to be lined up with.
    --
    -- "Not measured" travels as the literal NA rather than as the -1 the DOCK: line uses. There
    -- the margin is a continuous channel that -1 can only ever mean the sentinel on, because it
    -- is recomputed ten times a second and a real -1 would be transient. Here it is a one-shot
    -- clearance figure, a real -1.00 m margin is exactly the case the readout exists to warn
    -- about, and a sentinel that can be a real value is a sentinel that silences the warning.
    -- ...and because the body is now centred, the clearance is the same on both sides, so the
    -- margin is the SYMMETRIC half-span rather than the worse-side figure the live readout uses.
    -- Quoting the worse side here would report the asymmetry of the reference node as though it
    -- were a property of the parking job that has just removed it.
    local hw = bodyHalf or playerHalfWidth(player:getID())
    local margin, marginStr = nil, "NA"
    if hw and hw > 0 then
      margin = mouth.halfW - hw
      marginStr = string.format("%.2f", margin)
    end

    -- How far the lip is off the ground, which is the one thing this readout could not say and
    -- most needed to. The comment on the teleport height above already knew a mouth can be a
    -- metre in the air; it used that fact to avoid burying the vehicle and then never passed it
    -- on. be:getSurfaceHeightBelow reports failure as a huge NEGATIVE number rather than nil --
    -- the vehicle VM's implement block documents the same trap -- so this is a magnitude band,
    -- not a nil check, and NA travels rather than a zero that would read as "lip on the ground".
    --
    -- ...and it is withheld on exactly the same grounds as the pitch, because it rests on the
    -- same nodes: floorZ is the minimum z of the three mouth cids, so if those are structure
    -- rather than surface the height above ground is measured from the wrong thing. The Wheel
    -- Roller tilt ramp is the case -- it announced "ramp not down, lip 1.9 feet up" while its
    -- onramp plate sat flat on the deck. NA already travels for "could not measure" and Python
    -- already renders it as silence, so declining costs no new plumbing.
    local lipStr = "NA"
    if mouth.floorTrusted ~= false then
      local okL, lip = pcall(function()
        local g = be:getSurfaceHeightBelow(mouth.centre)
        if not g or math.abs(g) > 1e5 then return nil end
        return mouth.floorZ - g
      end)
      if okL and lip then lipStr = string.format("%.2f", lip) end
    end

    ipLog('I', string.format(
      "ramp align: %s [%d], mouth (%.1f,%.1f,%.1f), nose %.2f m, standing off %.2f m, "
      .. "margin %s m, lip %s m above ground",
      tgt.name, tgt.id, mouth.centre.x, mouth.centre.y, mouth.centre.z, nose, back,
      marginStr, lipStr))

    -- The lip is a FIFTH field on an already-positional payload, and Python parses it with a
    -- length guard for the reason the DOCK: line's entry-gate tail carries: bng_mod/ is a live
    -- junction into the game install, so the two halves genuinely do go out of step.
    send(string.format("RAMPALIGN:OK,%s,%.2f,%s,%s",
      cleanName(tgt.name), RAMP_ALIGN_STANDOFF_M, marginStr, lipStr))
  end)
  if not ok then
    ipLog('E', "ramp align threw: " .. tostring(err))
    rampAlignFail("align failed")
  end
end

-- Everything ramp mode does in one tick. Every path that cannot produce a reading names what
-- it saw, the same contract the implement path follows -- a silent instrument and a working one
-- sound identical from the seat, which is the ambiguity DOCKFAIL exists to remove.
local function scanRamp(player)
  local origin, why = dockOriginFrame(player)
  if not origin then return dockFail(why or "no origin") end
  local tgt, why2 = findRampTarget(player, origin.pos)
  if not tgt then return dockFail(why2 or "no ramp nearby") end
  sendRampDockLine(player, origin, tgt)
end

-- The low ballistic solution for a stationary target. cannonGeometry predicts launch speed
-- before firing from the processed launcher spring and complete cannonball mass, so changing
-- the Powder or Weight configuration takes effect without a sacrificial calibration shot.
local function ballisticAngle(range, rise, speed)
  if not speed or speed <= 0 or range <= 0.05 then return nil, false end
  local gravity = 9.81
  pcall(function() gravity = math.abs(core_environment.getGravity()) end)
  gravity = math.max(0.01, gravity)
  local v2 = speed * speed
  local disc = v2 * v2 - gravity * (gravity * range * range + 2 * rise * v2)
  if disc < 0 then return nil, false end
  return math.deg(math.atan((v2 - math.sqrt(disc)) / (gravity * range))), true
end

-- Old Cannon aiming uses the scanner's selected target. The scanner continues to own the
-- horizontal HRTF cue; this packet adds the physical barrel elevation and the vertical
-- ballistic correction, atomically against that same target id.
local function sendOldCannonAim(player, frame, cg)
  local speed = cg.getLaunchSpeed and cg.getLaunchSpeed(player:getID()) or nil
  local vs = extensions and extensions.vehicleScanner or nil
  local targetID = vs and vs.getCurrentTargetID and vs.getCurrentTargetID() or nil
  local target = targetID and scenetree.findObjectById(targetID) or nil
  if not target then
    send(string.format("CANNONAIM:%.3f,999,999,999,-1,%.3f,0",
      frame.elevation, speed or -1))
    return
  end

  local targetPos = vec3(target:getPosition())
  local geo = extensions and extensions.vehicleGeometry or nil
  if geo and geo.boxCentre then
    local okC, c = pcall(geo.boxCentre, targetID)
    if okC and c then targetPos = c end
  end

  local delta = targetPos - frame.muzzle
  local range = math.sqrt(delta.x * delta.x + delta.y * delta.y)
  local los = math.deg(math.atan2(delta.z, math.max(0.001, range)))

  local boreFlat = vec3(frame.bore.x, frame.bore.y, 0)
  local toFlat = vec3(delta.x, delta.y, 0)
  local bearing = 0
  if boreFlat:length() > 1e-4 and toFlat:length() > 1e-4 then
    boreFlat = boreFlat:normalized()
    toFlat = toFlat:normalized()
    local c = math.max(-1, math.min(1, boreFlat:dot(toFlat)))
    local mag = math.deg(math.acos(c))
    local left = vec3(0, 0, 1):cross(boreFlat)
    bearing = mag * ((left:dot(toFlat) < 0) and -1 or 1)
  end

  local aim, reachable = ballisticAngle(range, delta.z, speed)
  if not aim then aim = los end
  send(string.format("CANNONAIM:%.3f,%.3f,%.3f,%.3f,%.3f,%.3f,%d",
    frame.elevation, bearing, aim, los, range, speed or -1, reachable and 1 or 0))
end

local function scan()
  local player = be:getPlayerVehicle(0)
  if not player then
    sendDockLine(nil, "no player vehicle")
    return
  end
  -- Cheap, and it means switching vehicles takes effect on the next tick rather than
  -- waiting out a heartbeat.
  applyActivePush()

  -- Are you sitting in either cannon? Sent on change and gated on nothing: this is a fact about
  -- the vehicle, not part of the docking toggle. Both types are derived from resolved live
  -- geometry rather than a jbeam filename allowlist.
  do
    local cg = extensions and extensions.cannonGeometry or nil
    local rg = extensions and extensions.rampGeometry or nil
    local kind = "0"
    local oldFrame = nil
    if cg and cg.has then
      local okO, old = pcall(cg.has, player:getID())
      if okO and old then
        local okF, f = pcall(cg.frame, player:getID())
        if okF and f then oldFrame = f; kind = "OLD" end
      end
    end
    -- isCannon, NOT has. "This machine has a drive-in ramp" and "this machine is a cannon"
    -- were the same predicate only while large_cannon was the only vehicle that resolved at
    -- all; rampGeometry's part tiers exist precisely so a rollback, a tilt deck and a dry van
    -- resolve too, and each of them then latched LARGE. From the seat of a us_semi rollback
    -- that read out as "Inclination 100 percent, strength unknown" -- the inclination being
    -- the truck's own engine RPM over a thousand, pegged because its hydraulic pump raises
    -- idle to 1500 -- and it also masked the ramp readout that machine SHOULD get, since this
    -- kind wins ahead of _dock_phrase_ramp on the alignment key.
    if kind == "0" and rg and rg.isCannon then
      local okC, r = pcall(rg.isCannon, player:getID())
      if okC and r then kind = "LARGE" end
    end
    if kind ~= lastCannon then
      lastCannon = kind
      send("CANNON:" .. kind)
    end
    if oldFrame then sendOldCannonAim(player, oldFrame, cg) end
  end

  -- What the deck of the ramp machine you are SITTING IN is doing. A fact about your own
  -- vehicle, gated on nothing, which is why it sits beside the CANNON: line rather than inside
  -- the docking toggle: you tilt a deck and run it out while parked, with the instrument off,
  -- before any alignment work has begun. Everything else in this file measures from one machine
  -- to another; this is the one readout about the machine under you.
  --
  -- The tilt travels as an ANGLE off live geometry, not as a percentage off the tilt ram. It is
  -- the figure the driver of the car about to go up the ramp actually needs, it is the same one
  -- _dock_phrase_ramp already speaks, and it survives on a ramp with no hydraulics at all -- a
  -- fixed dry-van ramp still reports its pitch. The rams then say how much stroke is LEFT,
  -- which the angle cannot.
  do
    local rg = extensions and extensions.rampGeometry or nil
    local pid = player:getID()
    local line = "NONE"
    if rg and rg.has then
      local okH, hasRamp = pcall(rg.has, pid)
      if okH and hasRamp then
        local pitch, lip = nil, nil
        local okF, f = pcall(rg.mouthFrame, pid)
        if okF and f then
          pitch = f.pitchDeg
          -- How far the lip is off the ground. Neither of the other two numbers answers "can a
          -- car get onto this" on its own: on a rollback the tilt alone only brings the lip
          -- from 1.30 m to 0.95 m, and it is running the BED out that does the rest. Same
          -- magnitude band as the align's copy, because getSurfaceHeightBelow reports failure
          -- as a huge negative rather than nil.
          local okL, v = pcall(function()
            local g = be:getSurfaceHeightBelow(f.centre)
            if not g or math.abs(g) > 1e5 then return nil end
            return f.floorZ - g
          end)
          if okL and v then lip = v end
        end
        -- -999 is the "could not measure" sentinel for BOTH figures and must never be 0, which
        -- reads as a perfectly level ramp with its lip on the ground -- the single most
        -- reassuring thing this readout can say. Rounded before the change test below, so a
        -- machine at rest sends nothing while its deck breathes on its own springs.
        line = string.format("%.1f,%.2f;%s",
          pitch or -999, lip or -999, rampHydByVeh[pid] or "")
      end
    end
    if line ~= lastRampSelf or (scanClock - rampSelfSentAt) >= RAMP_SELF_HEARTBEAT_S then
      lastRampSelf, rampSelfSentAt = line, scanClock
      send("RAMPSELF:" .. line)
    end
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
  -- The one structural change ramp mode required. This used to be an early return, which is
  -- why ramp mode could never have been reached: a machine with no implement is exactly the
  -- machine you drive INTO a ramp. It is now a fall-through, and every other early return in
  -- this function is untouched and still names itself -- which is what keeps ramp work from
  -- being able to regress the implement case.
  --
  -- Note the proximity speech above (NEAR:/CLEAR) is implement-only and stays that way: it
  -- reports what the bucket is about to hit, and on a car there is no bucket.
  if not implCids then
    -- scanRamp names its own failure, and its reasons are deliberately about the RAMP rather
    -- than about the implement: "there is no ramp near you" and "the mod holds no implement"
    -- need completely different fixes and sound identical from the seat.
    if dockActive then scanRamp(player) end
    if slamState ~= "NONE" then slamState = "NONE" end
    return
  end

  local pts = implementPoints(player)
  if not pts then
    -- Reachable when every getNodePosition on the cached cids throws, i.e. straight after a
    -- part swap and before the vehicle VM's next push. Returning silently here was the last
    -- hole in the DOCKFAIL contract: the readout went dead and F9+I read back whatever reason
    -- it had been given last, which is exactly the ambiguity the rest of this function was
    -- rewritten to remove.
    sendDockLine(nil, string.format(
      "implement node positions unavailable (%d cids)", #implCids))
    return
  end

  -- The gates below want the five sample cids -- edgeL/C/R and heelL/R -- not the whole
  -- implement cloud. SLAM_OVER_MIN_PTS and INSIDE_MIN_PTS are both sized for a five-point set,
  -- and handing them ~120 nodes quietly changes what they mean: two of a hundred and twenty is
  -- a single corner grazing the target, and since losing the footprint takes ALL the points,
  -- the state then cannot be dropped again. COMMITTED ended up firing beside the target rather
  -- than over it. The nearest-approach sweep further down keeps the full cloud on purpose --
  -- that is the tier that sees a part that has broken off.
  --
  -- The fallback is the pre-fix behaviour, for a push that arrived without a sample set: less
  -- precise, but it still answers rather than going silent.
  local gatePts = M.getImplementPoints() or pts

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
        -- "Under the frame" needs both of these. Box overlap alone fires constantly, because
        -- a solid box also contains the air beside a low car's wheels -- which is not a place
        -- you can lift from.
        --
        -- It deliberately does NOT also require minD > CONTACT_M. That third clause made
        -- "under it" and "touching it" mutually exclusive, so tines resting against the
        -- underside of a load -- the one state an operator most needs confirmed, and the
        -- state immediately before a lift -- reported contact and dropped the "under"
        -- entirely. contact is its own field on the NEAR: line, so the two are independent.
        local nInside = 0
        for _, ip in ipairs(gatePts) do
          if insideBox(ip, frame, box) then nInside = nInside + 1 end
        end
        local boxMidZ = (box.minZ + box.maxZ) * 0.5
        local inside = (nInside >= INSIDE_MIN_PTS)
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
    local st = resolveSlam(best, gatePts, implMinZ)
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

  -- setsockname RETURNS nil plus a message on failure -- it does not throw. So a pcall around
  -- it reports success, udpCmd is a perfectly real socket object that is simply not bound to
  -- anything, and the error branch below never runs. That is how this extension spent a whole
  -- session deaf with nothing in the log: every other command port in the mod was listening and
  -- 4470 was absent from netstat entirely, while the extension carried on sending normally,
  -- because a UDP sender needs no bind. DOCK_ON, REBUILD and RAMPALIGN all went into the void,
  -- so F9+Shift+V spoke "Aligning to ramp" and then did nothing -- not even a failure, since
  -- the failure path is on the far side of the socket that never opened.
  --
  -- Checked explicitly, therefore, and the reason is recorded rather than inferred: "address
  -- already in use" (a leaked socket from a previous reload) and "permission denied" want
  -- completely different things done about them.
  local ok, err = pcall(function()
    udpCmd = socket.udp()
    local bound, berr = udpCmd:setsockname("127.0.0.1", CMD_LISTEN_PORT)
    if not bound then error(tostring(berr), 0) end
    udpCmd:settimeout(0)
  end)
  if not (ok and udpCmd) then
    ipLog('E', string.format(
      "Failed to bind UDP command socket on %d: %s -- commands will be ignored until this "
      .. "succeeds; retrying every %.0f s",
      CMD_LISTEN_PORT, tostring(err), CMD_BIND_RETRY_S))
    if udpCmd then pcall(function() udpCmd:close() end) end
    udpCmd = nil
  end
  cmdBindRetry = 0
end

local function resetState()
  isActive = true
  scanTimer = 0
  implByVeh = {}
  rampHydByVeh, lastRampSelf = {}, nil
  implVehID, implCids, implName, implSampleCids = nil, nil, nil, nil
  lastSentName, lastSentLine, nameEverSent = nil, nil, false
  -- A manual band pick is tied to a target and to the implement that was fitted when it was
  -- made; both may have just changed, so fall back to auto rather than keeping an index that
  -- now points at unrelated geometry.
  bandIndex, bandTargetID, lastDockLine = nil, nil, nil
  slamState = "NONE"
  entryOK = false
end

function M.onExtensionLoaded()
  setExtensionUnloadMode(M, "manual")
  ipLog('I', "Implement proximity extension loaded.")
  setupSockets()  -- here too, so a Ctrl+L Lua reload re-opens them
end

-- Fired by extensions.reload() and by an explicit unload, and it is load-bearing rather than
-- tidy-up. setupSockets closes the sockets held by THIS module instance, and a reload builds a
-- fresh instance whose locals are nil -- so it closes nothing, and the outgoing instance keeps
-- CMD_LISTEN_PORT bound. The second bind does not fail on Windows and nothing is logged; the
-- port simply ends up with two owners and the datagram is delivered to one of them, which is
-- not reliably the instance that is running.
--
-- The result is an extension that goes half dead in the most confusing way available. udpSend
-- needs no bind, so it re-opens cleanly and the mod keeps talking -- readouts, DOCKFAIL, the
-- deck line, all fine -- while silently hearing nothing. Measured: DOCK_ON and RAMPALIGN both
-- went into the void, so F9+Shift+V spoke "Aligning to ramp" and then did nothing at all. Not
-- even a failure, because the failure path is on the far side of the socket that died.
--
-- Every listening extension in this mod had the same three holes; all of them now carry the
-- same three fixes, in the same shape, so a grep can check for them.
function M.onExtensionUnloaded()
  if udpSend then pcall(function() udpSend:close() end); udpSend = nil end
  if udpCmd  then pcall(function() udpCmd:close()  end); udpCmd  = nil end
  ipLog('I', "Implement proximity extension unloaded; sockets closed.")
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
  -- Stored pushes are per vehicle and survive a switch: the machine you stepped out of still
  -- has its implement, and heartbeats keep them all current. Only the ACTIVE selection is
  -- re-derived.
  applyActivePush()
  lastSentLine = nil
  -- Only the LATCH, never the stored pushes: the hauler you climbed out of is still holding its
  -- deck where you left it, and its heartbeat keeps that current. Clearing the latch is what
  -- forces the line to be re-stated for whichever machine you are now in, including the NONE
  -- that says the new one has no ramp at all.
  lastRampSelf = nil
  -- A manual band pick is tied to a target and to the implement that was fitted when it was
  -- made; both may have just changed, so fall back to auto rather than keeping an index that
  -- now points at unrelated geometry.
  bandIndex, bandTargetID, lastDockLine = nil, nil, nil
  slamState = "NONE"
  entryOK = false
end

-- Vehicle ids are handed out per session, but a scene reload starts them again from zero, so
-- a stored push can outlive the machine that sent it and be inherited by whatever spawns into
-- its id. That inherited entry is a vehicle believing it has an implement it has never had --
-- the same failure the name matching produces, arrived at from the other direction -- and it
-- would stand until the new vehicle's own push landed. resetState() covers the level reload;
-- this covers a delete mid-session.
function M.onVehicleDestroyed(vehId)
  rampHydByVeh[vehId], lastRampSelf = nil, nil
  if implByVeh[vehId] == nil then return end
  implByVeh[vehId] = nil
  applyActivePush()
end

function M.onVehicleResetted(vehId)
  -- Drop only the vehicle that reset; its own re-push will refill it. Other vehicles' cids
  -- are unaffected and must not be thrown away.
  implByVeh[vehId] = nil
  -- The reset re-arms the vehicle VM's own scan, so its next push refills this. Clearing the
  -- latch too is what makes the readout re-state itself rather than waiting for the deck to
  -- move before it will admit to a position it has held all along.
  rampHydByVeh[vehId], lastRampSelf = nil, nil
  applyActivePush()
  lastSentLine = nil
  -- A manual band pick is tied to a target and to the implement that was fitted when it was
  -- made; both may have just changed, so fall back to auto rather than keeping an index that
  -- now points at unrelated geometry.
  bandIndex, bandTargetID, lastDockLine = nil, nil, nil
  slamState = "NONE"
  entryOK = false
end

-- Re-arm a bind that failed, so the extension is not deaf for the rest of the session. This is
-- the recovery path, not a precaution, and it has been watched doing the job: the first reload
-- of the patched files leaked eight ports, because the OUTGOING code had no unload hook yet.
-- The retry could not take them while the old module tables were still referenced -- a socket
-- held that way is not one the collector is about to free -- and ticked uselessly for two
-- minutes. The Ctrl+L that followed did NOT re-load these extensions (no load line for any of
-- them in the log at that timestamp, so setupSockets never ran again); all thirteen ports came
-- back through this function instead, within one frame of each other, the moment those tables
-- went away. Without it the mod would have stayed deaf until the game was restarted.
--
-- Named and shaped identically in every listening extension, so one grep can tell whether a
-- file has it; vehicle_geometry_sim.lua scenario 12 is that grep.
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
    ipLog('I', string.format("UDP command socket bound on %d after retry.", CMD_LISTEN_PORT))
  end
end

function M.onUpdate(dtReal, dtSim, dtRaw)
  retryCmdBind(dtReal)
  if udpCmd then
    local data
    repeat
      data = udpCmd:receive()
      if data then
        local cmd = data:match("^%s*(.-)%s*$"):upper()
        if cmd == "ON" then
          isActive = true
          lastSentName, lastSentLine, nameEverSent = nil, nil, false
          -- Re-armed for the same reason nameEverSent is: Python probes with ON at startup,
          -- and a latch left set means the CANNON: line is never re-sent, so a beamtel
          -- restarted while sitting in the cannon would never learn it was there. RAMPSELF:
          -- is latched the same way and re-arms with it.
          lastCannon, lastRampSelf = nil, nil
        elseif cmd == "OFF" then
          isActive = false
        elseif cmd == "REBUILD" then
          implByVeh = {}
          implVehID, implCids, implName, implSampleCids = nil, nil, nil, nil
          lastSentName, lastSentLine, nameEverSent = nil, nil, false
          lastCannon, rampHydByVeh, lastRampSelf = nil, {}, nil
          -- A rebuild is the one command that says "forget what you resolved", so the ramp
          -- cache goes with it. Without this a part swap that fits or removes a ramp keeps
          -- answering from the old node set.
          if extensions and extensions.rampGeometry then
            pcall(function() extensions.rampGeometry.invalidate(nil) end)
          end
          if extensions and extensions.cannonGeometry then
            pcall(function() extensions.cannonGeometry.invalidate(nil) end)
          end
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
        elseif cmd == "RAMPALIGN" then
          M.rampAlign()
        end
      end
    until not data
  end

  if not isActive then return end

  scanTimer = scanTimer + dtReal
  scanClock = scanClock + dtReal
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
