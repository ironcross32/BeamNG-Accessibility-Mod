-- vehicleGeometry.lua
--
-- Derived node geometry for ANY vehicle -- the player's or a target's -- so that callers can
-- ask "how far is the gap" instead of "how far apart are the two origins".
--
-- Why this exists: vehicleScanner measured distance as originPos:distance(targetPos), and
-- getPosition() is the object's reference node, near its centre. Every distance it reported
-- was therefore centre-to-centre, ~2.5 m of phantom range on a Pickup, and -- worse --
-- orientation-dependent: broadside and nose-on to the same car reported the same number
-- while the real gap differed by more than a metre. Lining a loader's forks up with a pallet
-- needs the gap, not the centre distance.
--
-- The design rule this file exists to serve is that NOTHING downstream asks "is this a
-- loader". It asks "what are my contact points" and "what are the target's surface points",
-- and gets an answer for every vehicle. A generic car in forward gear is its own front node
-- band; the same car in reverse is its rear band; a WL-40 in forward is the five implement
-- cids that 796F6C6F313035.lua already resolves. The loader is a different ANSWER, not a
-- different code path, which is what makes it impossible for loader work to regress the
-- generic case.
--
-- Resolution is the cross-VM pattern nodeGrabberAccessible.lua already uses (queueLuaCommand
-- -> classify in the vehicle VM -> CSV payload -> obj:queueGameEngineLua -> GE cache), plus
-- the epoch guard vehicleScanner's coupler discovery uses, which is needed here because the
-- scanner's target can change while a round trip is in flight and a late reply would
-- otherwise install one vehicle's cids under another vehicle's id.

local M = {}

-- Node classification. A node counts as "hull" if it sits near any face of the normalised
-- bounding box; that is a cheap shell approximation, not a convex hull, and it is all the
-- narrow phase needs since interior nodes can never be the nearest point to an outside
-- observer.
local FACE_FRAC   = 0.15   -- normalised distance from a face that still counts as hull
local BAND_FRAC   = 0.15   -- fore/aft slice at each end that forms the contact bands
-- Hull cap. This is the one constant with a real accuracy cost attached: the node tier
-- reports the distance to the nearest SAMPLED node, so it over-reads near contact by up to
-- half the sampling spacing. Thinning a car's shell to 48 measured 0.19 m of over-read at
-- the surface in vehicle_geometry_sim.lua -- clear air reported while the tines are already
-- touching. The queries only run inside NEAR_NODE_M and only for a locked target, so the
-- per-tick cost of a denser set is small and worth paying.
local HULL_MAX    = 128    -- cap on cached hull cids
local BAND_MAX    = 16     -- cap on cached front/rear band cids
local HIST_BINS   = 24     -- vertical occupancy bins, consumed by the band selector
-- A bin holding less than this share of the fullest bin reads as empty space. Judged
-- relative to the peak rather than as an absolute count because node density varies by an
-- order of magnitude between a cone and a truck, and the question is only ever "is there
-- material at this height compared with the rest of this object".
local BAND_OCC_FRAC = 0.25
local MIN_NODES   = 4      -- below this a vehicle has no meaningful shape
-- Outlier rejection for the extents. A percentile trim is the wrong tool here: a detached
-- bumper is a DENSE cluster of ten or twenty nodes sitting on its own, so trimming a fixed
-- fraction either misses it entirely or eats real bodywork. What separates it from the body
-- is empty space, so that is what gets measured -- sort each axis, walk in from each end,
-- and cut at the first gap this wide. The cap stops a genuinely sparse object (a cone, a
-- pallet) from being whittled away by its own legitimate spacing.
local OUTLIER_GAP_M  = 0.5   -- empty span along an axis that marks separated geometry
local TRIM_MAX_FRAC  = 0.15  -- most of the nodes that may be cut from either end

-- Beyond this the oriented box is used (one closed-form point, no node queries); inside it
-- the hull cids are queried live. Same box/cloud split implementProximity.lua already argues
-- for, and it means the per-tick node cost is only paid when something is actually close.
local NEAR_NODE_M = 3.0

local RESOLVE_TIMEOUT_S = 3.0
local MAX_TRIES         = 3

-- cache[vehID] = {ext = {...}, hull = {cid...}, front = {cid...}, rear = {cid...},
--                 hist = {n...}, histLo = number, histHi = number}
-- All scalars are offsets from the vehicle ORIGIN along the vehicle's own axes, which is
-- what getNodePosition returns once projected, so they stay valid as the vehicle moves.
local cache   = {}
local pending = {}  -- vehID -> {epoch = n, timer = seconds, tries = n}
local epochCounter = 0

local function vgLog(level, msg) log(level, 'vehicleGeometry', msg) end

-- =================================================================================================
--  Resolution (vehicle VM -> GE)
-- =================================================================================================

-- Runs once per vehicle in that vehicle's own VM, which is the only place v.data.nodes
-- exists. Deliberately built with plain concatenation rather than an inner string.format:
-- this whole chunk is itself the subject of an outer string.format, so every '%' would need
-- doubling, and that escaping is exactly the kind of thing that breaks silently. Every value
-- interpolated below is numeric, so single-quoting the CSVs needs no escaping either.
--
-- Coordinates are projections onto the vehicle's live direction vectors rather than raw
-- design-space nd.pos. Design space would save the getNodePosition calls, but its fore/aft
-- sign convention is not guaranteed -- 796F6C6F313035.lua has to derive it from a centroid
-- comparison -- and getting the front band backwards would point every contact test at the
-- wrong end of the machine.
local VEH_SCRIPT = [[
local ok, err = pcall(function()
  if not (v.data and v.data.nodes) then return end
  local fwd = vec3(obj:getDirectionVector())
  local up  = vec3(obj:getDirectionVectorUp())
  local rgt = up:cross(fwd)

  local cids, fs, rs, us = {}, {}, {}, {}
  local n = 0
  for _, nd in pairs(v.data.nodes) do
    local lp = vec3(obj:getNodePosition(nd.cid))
    n = n + 1
    cids[n], fs[n], rs[n], us[n] = nd.cid, fwd:dot(lp), rgt:dot(lp), up:dot(lp)
  end
  if n < ]] .. MIN_NODES .. [[ then return end

  -- Extents are a trimmed range, not the absolute min and max, and that matters more than
  -- it looks. When a part breaks off a vehicle its nodes stay in the SAME object, so a
  -- bumper lying three metres away would stretch the raw min/max across both -- giving a box
  -- that spans the empty air between them and reports solid surface where there is none.
  -- It also wrecks the normalisation below: with one node at 3.4 m and the body inside
  -- 0.9 m, every real body node collapses into a narrow slice of the 0-1 range and the hull
  -- and band classification stops distinguishing anything. Trimming the tails describes the
  -- BODY; the debris is still picked up as a hull node (it normalises outside 0-1, so it
  -- trips the face test) and remains visible to the node tier, which is the tier that exists
  -- to see it.
  local function trimmed(t)
    local s = {}
    for i = 1, n do s[i] = t[i] end
    table.sort(s)
    local gap = ]] .. OUTLIER_GAP_M .. [[
    local maxCut = math.floor(n * ]] .. TRIM_MAX_FRAC .. [[)
    local lo, hi = 1, n
    for j = n, 2, -1 do
      if (n - j + 1) > maxCut then break end
      if s[j] - s[j - 1] > gap then hi = j - 1; break end
    end
    for j = 1, hi - 1 do
      if (j - 1) > maxCut then break end
      if s[j + 1] - s[j] > gap then lo = j + 1; break end
    end
    return s[lo], s[hi]
  end
  local minF, maxF = trimmed(fs)
  local minR, maxR = trimmed(rs)
  local minU, maxU = trimmed(us)

  local fRange, rRange, uRange = maxF - minF, maxR - minR, maxU - minU
  local hull, front, rear, hist = {}, {}, {}, {}
  for i = 1, ]] .. HIST_BINS .. [[ do hist[i] = 0 end

  for i = 1, n do
    local fN = fRange > 0.01 and (fs[i] - minF) / fRange or 0.5
    local rN = rRange > 0.01 and (rs[i] - minR) / rRange or 0.5
    local uN = uRange > 0.01 and (us[i] - minU) / uRange or 0.5
    local face = ]] .. FACE_FRAC .. [[
    if fN < face or fN > 1 - face or rN < face or rN > 1 - face
        or uN < face or uN > 1 - face then
      hull[#hull + 1] = cids[i]
    end
    local band = ]] .. BAND_FRAC .. [[
    if fN > 1 - band then front[#front + 1] = cids[i] end
    if fN < band then rear[#rear + 1] = cids[i] end
    local b = math.floor(uN * ]] .. HIST_BINS .. [[) + 1
    if b < 1 then b = 1 end
    if b > ]] .. HIST_BINS .. [[ then b = ]] .. HIST_BINS .. [[ end
    hist[b] = hist[b] + 1
  end

  -- Stride rather than truncate: taking the first N would bias every band to whichever
  -- corner the jbeam happens to list first, and the narrow phase would then miss the other.
  local function thin(t, cap)
    if #t <= cap then return t end
    local step, out = #t / cap, {}
    local acc = 1.0
    while #out < cap and math.floor(acc) <= #t do
      out[#out + 1] = t[math.floor(acc)]
      acc = acc + step
    end
    return out
  end
  hull  = thin(hull,  ]] .. HULL_MAX .. [[)
  front = thin(front, ]] .. BAND_MAX .. [[)
  rear  = thin(rear,  ]] .. BAND_MAX .. [[)

  local ext = tostring(minF) .. "," .. tostring(maxF) .. "," .. tostring(minR) .. ","
           .. tostring(maxR) .. "," .. tostring(minU) .. "," .. tostring(maxU)
  obj:queueGameEngineLua(
    "if extensions.vehicleGeometry then extensions.vehicleGeometry.onVehicleGeometry("
    .. obj:getID() .. "," .. %d
    .. ",'" .. ext .. "','" .. table.concat(hull, ",") .. "','"
    .. table.concat(front, ",") .. "','" .. table.concat(rear, ",") .. "','"
    .. table.concat(hist, ",") .. "') end")
end)
if not ok then
  obj:queueGameEngineLua("log('E','vehicleGeometry','vehicle-side resolve failed: "
    .. tostring(err):gsub("'", " ") .. "')")
end
]]

local function parseNums(csv)
  local out = {}
  for s in tostring(csv or ""):gmatch("[^,]+") do
    local v = tonumber(s)
    if v then out[#out + 1] = v end
  end
  return out
end

-- Called back from the vehicle VM. Stale replies are dropped on the epoch, not on the id:
-- the id is the same across a reset, so an id check alone would happily install pre-reset
-- cids for a part configuration that has since changed.
function M.onVehicleGeometry(vehID, epoch, extCsv, hullCsv, frontCsv, rearCsv, histCsv)
  local p = pending[vehID]
  if not p or p.epoch ~= epoch then
    vgLog('D', string.format("stale geometry reply for %s (epoch %s)",
      tostring(vehID), tostring(epoch)))
    return
  end
  pending[vehID] = nil

  local ext = parseNums(extCsv)
  if #ext ~= 6 then
    vgLog('W', string.format("vehicle %s returned %d extents, expected 6", tostring(vehID), #ext))
    return
  end
  local hull  = parseNums(hullCsv)
  local hist  = parseNums(histCsv)
  cache[vehID] = {
    ext   = {minF = ext[1], maxF = ext[2], minR = ext[3],
             maxR = ext[4], minU = ext[5], maxU = ext[6]},
    hull  = (#hull > 0) and hull or nil,
    front = parseNums(frontCsv),
    rear  = parseNums(rearCsv),
    hist  = (#hist == HIST_BINS) and hist or nil,
    histLo = ext[5],
    histHi = ext[6],
  }
  vgLog('I', string.format(
    "vehicle %d geometry: %d hull, %d front, %d rear, span %.2f x %.2f x %.2f m",
    vehID, #hull, #cache[vehID].front, #cache[vehID].rear,
    ext[2] - ext[1], ext[4] - ext[3], ext[6] - ext[5]))
end

-- Fire a resolve for this vehicle unless one is already cached or in flight. Cheap to call
-- every tick; callers are not expected to track state.
function M.request(vehID)
  if not vehID or cache[vehID] or pending[vehID] then return end
  local veh = scenetree.findObjectById(vehID)
  if not veh then return end
  epochCounter = epochCounter + 1
  pending[vehID] = {epoch = epochCounter, timer = 0, tries = 1}
  pcall(function() veh:queueLuaCommand(string.format(VEH_SCRIPT, epochCounter)) end)
end

function M.get(vehID)
  return cache[vehID]
end

-- The resolved cross-VM chunk, for diagnostics. This exists because a syntax error inside a
-- queueLuaCommand string is invisible: it does not fail here, it fails later in another VM,
-- where the only symptom is that the callback never arrives and every caller quietly sits on
-- the getPosition() fallback for the rest of the session. vehicle_geometry_sim.lua compiles
-- this so that class of error is caught before the game is ever started.
function M.debugVehScript()
  return string.format(VEH_SCRIPT, 0)
end

-- =================================================================================================
--  Queries
-- =================================================================================================

-- Oriented box in the vehicle's own frame. Deliberately not
-- be:getObjectOOBBHalfExtentsXYZ, which is axis-aligned to the WORLD: for a car parked at
-- any angle other than dead-on that describes a box it is not in.
function M.boxFrame(vehID)
  local veh = scenetree.findObjectById(vehID)
  if not veh then return nil end
  local ok, res = pcall(function()
    local fwd = vec3(veh:getDirectionVector()):normalized()
    local up  = vec3(veh:getDirectionVectorUp()):normalized()
    local rgt = up:cross(fwd)
    if rgt:length() < 1e-4 then return nil end
    return {c = vec3(veh:getPosition()), f = fwd, u = up, r = rgt:normalized()}
  end)
  if not ok then return nil end
  return res
end

local function clamp(v, lo, hi)
  if v < lo then return lo end
  if v > hi then return hi end
  return v
end

-- Closest point on the oriented box, closed form: project into the box frame, clamp each
-- axis to its extent, project back.
local function closestOnBox(frame, ext, p)
  local d = p - frame.c
  local f = clamp(frame.f:dot(d), ext.minF, ext.maxF)
  local r = clamp(frame.r:dot(d), ext.minR, ext.maxR)
  local u = clamp(frame.u:dot(d), ext.minU, ext.maxU)
  return frame.c + frame.f * f + frame.r * r + frame.u * u
end

-- World positions of a cid list on a vehicle. getNodePosition returns an offset from the
-- vehicle origin expressed in WORLD axes, so this is a plain addition.
local function nodeWorldPoints(veh, cids)
  if not cids or #cids == 0 then return nil end
  local base = vec3(veh:getPosition())
  local pts = {}
  for _, cid in ipairs(cids) do
    local ok, p = pcall(function() return vec3(veh:getNodePosition(cid)) end)
    if ok and p then pts[#pts + 1] = base + p end
  end
  if #pts == 0 then return nil end
  return pts
end

-- Everything a surface query needs, resolved once. Callers that test many points against one
-- target build this once instead of paying a scenetree lookup, a boxFrame and a hull node
-- sweep per point -- which for the loader's five implement points was five times over.
local function surfaceContext(vehID)
  local veh = scenetree.findObjectById(vehID)
  if not veh then return nil end
  M.request(vehID)
  local entry = cache[vehID]
  local ctx = {veh = veh, entry = entry}
  if entry then
    ctx.frame = M.boxFrame(vehID)
    -- Hull positions are fetched lazily: most ticks never get inside NEAR_NODE_M, and this
    -- is the only part of the query that costs a call per node.
  end
  return ctx
end

-- The nearest point on the target's surface to worldPos, and the distance to it.
--
-- The fallback chain is the whole safety property of this file, and it terminates at exactly
-- today's behaviour: a vehicle whose resolve never lands still answers, it just answers less
-- precisely. Nothing downstream needs to handle a nil.
--
--   hull cids cached and close  -> nearest hull node   (sees detached parts, true surface)
--   extents cached              -> closest point on the oriented box
--   neither                     -> getPosition()       (centre-to-centre, the old behaviour)
local function nearestOnContext(ctx, worldPos)
  if not ctx then return nil, nil end
  local entry, frame = ctx.entry, ctx.frame
  if not (entry and frame) then
    local c = vec3(ctx.veh:getPosition())
    return c, worldPos:distance(c)
  end

  local boxPt = closestOnBox(frame, entry.ext, worldPos)
  local boxD  = worldPos:distance(boxPt)
  -- Tier decided on the cheap box distance, so the node sweep is only paid for when
  -- something is genuinely close.
  if boxD > NEAR_NODE_M or not entry.hull then
    return boxPt, boxD
  end

  if ctx.hullPts == nil then
    ctx.hullPts = nodeWorldPoints(ctx.veh, entry.hull) or false
  end
  if not ctx.hullPts then return boxPt, boxD end
  local best, bestD = nil, math.huge
  for _, p in ipairs(ctx.hullPts) do
    local d = worldPos:squaredDistance(p)
    if d < bestD then best, bestD = p, d end
  end
  if not best then return boxPt, boxD end
  bestD = math.sqrt(bestD)

  -- Keep the SMALLER of the two, the way the vehicle VM keeps the smaller of its terrain
  -- height and its raycast. The two tiers fail in opposite directions and the minimum takes
  -- the good half of each. The box encloses the body, so its distance can only ever
  -- under-read the true gap -- but it is exact right at the surface, which is where docking
  -- precision matters. The node sweep is a sparse sample of that surface (HULL_MAX cids on a
  -- car with hundreds of nodes), so near contact it OVER-reads by up to half the node
  -- spacing -- 0.19 m on a test car, i.e. reporting clear air when the tines are touching.
  -- What the node sweep uniquely sees is geometry OUTSIDE the box: a bumper that has broken
  -- off still belongs to the same object, and there the node distance is the smaller one and
  -- wins on its own merits.
  if boxD < bestD then return boxPt, boxD end
  return best, bestD
end

function M.nearestSurfacePoint(vehID, worldPos)
  return nearestOnContext(surfaceContext(vehID), worldPos)
end

-- The business end of a vehicle: its front node band in forward, its rear band in reverse.
-- Falls back to the vehicle origin so callers always get at least one point.
function M.contactPoints(vehID, dir)
  local veh = scenetree.findObjectById(vehID)
  if not veh then return nil end
  M.request(vehID)
  local entry = cache[vehID]
  if entry then
    local list = (dir and dir < 0) and entry.rear or entry.front
    local pts = nodeWorldPoints(veh, list)
    if pts then return pts end
  end
  return {vec3(veh:getPosition())}
end

-- Nearest approach between a set of world points (the caller's contact set) and a vehicle's
-- surface. Returns the gap, the winning contact point, and the surface point it won against.
function M.nearestApproach(pts, targetID)
  if not pts or #pts == 0 then return nil, nil, nil end
  local ctx = surfaceContext(targetID)
  if not ctx then return nil, nil, nil end
  local bestD, bestP, bestSurf = math.huge, nil, nil
  for _, p in ipairs(pts) do
    local surf, d = nearestOnContext(ctx, p)
    if surf and d and d < bestD then bestD, bestP, bestSurf = d, p, surf end
  end
  if not bestP then return nil, nil, nil end
  return bestD, bestP, bestSurf
end

-- Collapse the vertical occupancy histogram into alternating GAP and SOLID runs, with world
-- heights.
--
-- This is deliberately not a "pocket finder". Lifting and ramming want different bands out of
-- identical geometry -- forks into a pallet want the void, forks through a window want the
-- glass, a bucket into the flank wants the sill -- so the profile is built once and the
-- reference is *selected* rather than derived. That is also what lets the selection be
-- announced: with a derived pocket the operator could never tell what the readout was
-- measuring against.
--
-- Heights come back in world Z, since that is the frame the implement's own position is in.
-- The conversion samples the box's central column, so on a vehicle tilted well off level the
-- reported heights lean toward that column rather than describing a horizontal slice. That is
-- the right error to have: a tilted target is one you are about to approach from a slope, and
-- the centre of it is what the implement meets.
function M.bands(vehID)
  local entry = cache[vehID]
  if not (entry and entry.hist) then return nil end
  local frame = M.boxFrame(vehID)
  if not frame then return nil end

  local peak = 0
  for i = 1, HIST_BINS do
    if entry.hist[i] > peak then peak = entry.hist[i] end
  end
  if peak <= 0 then return nil end
  local thresh = peak * BAND_OCC_FRAC

  local lo, span = entry.histLo, entry.histHi - entry.histLo
  if span <= 1e-4 then return nil end
  local e = entry.ext
  local fMid = (e.minF + e.maxF) * 0.5
  local rMid = (e.minR + e.maxR) * 0.5
  local function zAt(uNorm)
    local base = frame.c + frame.f * fMid + frame.r * rMid + frame.u * (lo + span * uNorm)
    return base.z
  end

  local out = {}
  local curKind, curStart = nil, 1
  for i = 1, HIST_BINS do
    local kind = (entry.hist[i] < thresh) and "GAP" or "SOLID"
    if kind ~= curKind then
      if curKind then
        out[#out + 1] = {kind = curKind,
                         loZ = zAt((curStart - 1) / HIST_BINS),
                         hiZ = zAt((i - 1) / HIST_BINS)}
      end
      curKind, curStart = kind, i
    end
  end
  out[#out + 1] = {kind = curKind, loZ = zAt((curStart - 1) / HIST_BINS), hiZ = zAt(1.0)}
  return out
end

-- Centre of the target's oriented box, in world space. This is what bearing should aim at:
-- a bearing to the nearest SURFACE point swings wildly at close range as the winning point
-- hops between corners, and bearing is the thing you steer with.
function M.boxCentre(vehID)
  local veh = scenetree.findObjectById(vehID)
  if not veh then return nil end
  M.request(vehID)
  local entry = cache[vehID]
  local frame = entry and M.boxFrame(vehID) or nil
  if not (entry and frame) then return vec3(veh:getPosition()) end
  local e = entry.ext
  return frame.c
       + frame.f * ((e.minF + e.maxF) * 0.5)
       + frame.r * ((e.minR + e.maxR) * 0.5)
       + frame.u * ((e.minU + e.maxU) * 0.5)
end

-- =================================================================================================
--  GE Extension Hooks
-- =================================================================================================

local function dropVehicle(vehID)
  if vehID == nil then return end
  cache[vehID] = nil
  pending[vehID] = nil
end

local function dropAll()
  cache, pending = {}, {}
end

function M.invalidate(vehID)
  if vehID == nil then dropAll() else dropVehicle(vehID) end
end

function M.onExtensionLoaded()
  setExtensionUnloadMode(M, "manual")
  vgLog('I', "Vehicle geometry extension loaded.")
end

function M.onWorldReadyState(state)
  if state == 2 then dropAll() end
end

function M.onVehicleSwitched(oldId, newId, player)
  -- Only the player's own geometry is pose-dependent enough to care, but a switch is also
  -- the cheapest moment to shed entries for vehicles that no longer exist.
  dropVehicle(oldId)
end

function M.onVehicleResetted(vehId)
  -- A reset can change the part configuration, which changes the node set entirely.
  dropVehicle(vehId)
end

function M.onVehicleDestroyed(vehId)
  dropVehicle(vehId)
end

function M.onUpdate(dtReal, dtSim, dtRaw)
  -- Retry stalled resolves. A vehicle VM that is still spawning will not answer, and without
  -- this the entry would stay pending forever and the caller would sit on the getPosition()
  -- fallback for the rest of the session.
  for vehID, p in pairs(pending) do
    p.timer = p.timer + dtReal
    if p.timer >= RESOLVE_TIMEOUT_S then
      if p.tries >= MAX_TRIES then
        vgLog('W', string.format("vehicle %s never returned geometry; staying on box/origin fallback",
          tostring(vehID)))
        pending[vehID] = nil
      else
        local veh = scenetree.findObjectById(vehID)
        if veh then
          epochCounter = epochCounter + 1
          p.epoch, p.timer, p.tries = epochCounter, 0, p.tries + 1
          pcall(function() veh:queueLuaCommand(string.format(VEH_SCRIPT, epochCounter)) end)
        else
          pending[vehID] = nil
        end
      end
    end
  end
end

return M
