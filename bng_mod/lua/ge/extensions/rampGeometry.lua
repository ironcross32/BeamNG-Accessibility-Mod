-- rampGeometry.lua
--
-- Resolves the drive-in mouth of a ramp-equipped vehicle -- today the stock large_cannon,
-- whose cannon_ramp slot is a funnel you drive a car into before it is fired out of the
-- barrel. A blind driver has no way to find that mouth, sit on its centreline, square up to
-- it, or know whether the car fits between its walls, and none of the existing instruments
-- can answer any of it: the docking instrument measures from an implement the car does not
-- have, and the scanner reports a bearing to a box centre ten metres behind the mouth.
--
-- This file is deliberately a sibling of vehicleGeometry.lua rather than an addition to it.
-- vehicleGeometry answers "where is this vehicle's surface" for every vehicle; this answers
-- "where is this vehicle's ramp mouth" for the vanishingly small number that have one. Same
-- cross-VM resolve, same epoch guard, same terminal-failure discipline, but a completely
-- different question, and folding it in would put a name-matched special case inside the one
-- file whose whole premise is that it has none.
--
-- Everything here is silent on ordinary vehicles by construction: the name match finds
-- nothing, the resolve lands in `failed` on its first and only round trip, and every caller
-- gets nil forever after. There is no per-vehicle check anywhere in it.

local M = {}

-- Node set is resolved by NAME, never by geometry -- the same rule, and the same reasoning,
-- as the implement node set in 796F6C6F313035.lua. Anchored so that a node called
-- "guardramp_3" cannot join the set.
local RAMP_NODE_PAT = "^ramp_"
-- A floor, not a fit. The real large_cannon mouth row alone holds around seventeen nodes;
-- anything under eight is not a ramp and is not worth trying to find two end rows in.
local MIN_RAMP_NODES = 8
-- Fraction of the along-ramp span forming each end row.
--
-- Much wider than the implement resolver's 0.15 fore/aft band, and deliberately so: a mouth is
-- not a single row of nodes. large_cannon's is three distinct rows spread over 1.26 m of a
-- 5.79 m span -- the toe on the ground at -13.889, the floor lip at -12.889, and the raised
-- side wall at -12.625 -- and the wall row is the one the half-width rule below depends on. At
-- 0.15 the band is 0.87 m and captures only the toe, so the wall is never seen and the rule
-- silently falls through to its own fallback.
--
-- The band has to be at least 1.264 / 5.792 = 0.218 to hold all three, and less than
-- 4.528 / 5.792 = 0.782 or the INNER row's band would start swallowing mouth nodes. 0.35 sits
-- with 60% margin above the floor and well clear of the ceiling.
local ROW_BAND = 0.35
-- A mouth-row node this far above the mouth floor plane counts as WALL. The real wall sits
-- 0.483 m above the toe floor, so this is 3x below it and comfortably above any modelling seam
-- or soft-body sag.
local WALL_MIN_H = 0.15
-- ...and no further above it than this.
--
-- A side wall is something you could drive INTO; something two metres up is something you
-- drive UNDER, and the two must not be confused because the wall rule below takes the node
-- with the SMALLEST lateral offset. large_cannon has exactly such a node: ramp_M_0, sitting
-- 2.1 m above the mouth floor on the centreline. Without a ceiling on the rule it is picked as
-- a mouth edge and the drivable width collapses to whatever its lateral offset happens to be.
--
-- The height cap is the discriminator that actually holds, because it is a property of the
-- node rather than of where the ramp happens to be sitting: the real wall is 0.483 m up and
-- ramp_M_0 is 2.100 m up, so 1.0 m separates them with better than 2x margin either side.
local WALL_MAX_H = 1.0
-- ...and a wall candidate must also be at least this far off the mouth centreline.
--
-- Kept alongside the height cap rather than replaced by it, because the two catch different
-- things: this one stops a low ridge or a centre rib being read as the left wall, which the
-- height cap would happily admit. It is NOT sufficient on its own -- it only excludes an
-- overhead node while that node sits near the centreline, and the very first offset ramp puts
-- it back in scope. That is what the height cap is for. 0.40 m is comfortably inside the
-- innermost real floor node (0.546 m) and outside anything describable as on the centreline.
local MIN_WALL_LATERAL_M = 0.40
-- The along-ramp axis is derived from the displacement of the ramp's centroid from the whole
-- machine's centroid, and that displacement must beat the other two axes by this factor or the
-- resolve is REJECTED rather than guessed at.
--
-- The obvious alternative -- take the ramp cloud's longest axis -- gets it WRONG on the very
-- vehicle this exists for, and not by a hair. large_cannon's ramp is 5.792 m long, but its
-- inner row flares out to ramp_L_6b at x 3.449, making the cloud 6.898 m WIDE. The longest
-- axis of the ramp is therefore the lateral one, and a rule built on it would call the mouth a
-- side wall and report lateral offset as range.
--
-- The centroid displacement has no such problem, because it does not depend on the ramp's
-- proportions at all: the ramp hangs off one end of a ~16 m machine, so the displacement is
-- many metres along the ramp axis and near zero on the other two. It also derives which end is
-- the mouth in the same step, which the longest-axis rule cannot do at all. Same
-- centroid-comparison trick 796F6C6F313035.lua uses for the implement's fore/aft sign.
local AXIS_DOMINANCE = 3.0
local MIN_AXIS_DISP_M = 1.0  -- ...and it must be a real displacement, not numerical residue
-- Degenerate baseline guard, mirroring the implement resolver's MIN_EDGE_WIDTH_M and there for
-- the same reason: a two-point baseline shorter than this points wherever soft-body jitter
-- says it does, and every angle derived from it inherits that.
local MIN_MOUTH_WIDTH_M = 0.30

-- Copied from vehicleGeometry deliberately, so there is one retry cadence across the mod.
local RESOLVE_TIMEOUT_S = 3.0
local MAX_TRIES         = 3

-- cache[vehID] = {cids = {mouthL, mouthC, mouthR, innerL, innerR},
--                 halfW, alongSpan, floorU, nNodes, wallUsed, naiveHalfW}
local cache   = {}
local pending = {}  -- vehID -> {epoch = n, timer = seconds, tries = n}
-- Vehicles that answered with nothing usable, or used up their retries. Same flag and the same
-- justification as vehicleGeometry's: M.request is documented as cheap to call every tick and
-- every caller does, so clearing `pending` with nothing in `cache` merely re-arms the resolve
-- on the next frame. Here it matters far more than it does there, because the overwhelming
-- majority of vehicles will never have a ramp -- without this flag every ordinary car in the
-- scene would re-issue a cross-VM chunk every three seconds for the whole session.
local failed  = {}
-- Vehicles that replied "I have no node data yet", counted rather than flagged: that is a
-- statement about a VM still spawning, not about the vehicle, and treating it as terminal is
-- how a cannon becomes permanently invisible depending on when the first resolve happened to
-- fire. Bounded by MAX_TRIES so it cannot become an unbounded retry loop.
local notReady = {}
local epochCounter = 0

local function rgLog(level, msg) log(level, 'rampGeometry', msg) end

-- =================================================================================================
--  Resolution (vehicle VM -> GE)
-- =================================================================================================

-- Runs once per vehicle in that vehicle's own VM -- the only place v.data.nodes and therefore
-- node NAMES exist. Built with plain concatenation rather than an inner string.format for the
-- reason vehicleGeometry's chunk documents: the whole chunk is itself the subject of an outer
-- string.format, so every '%' would need doubling and that escaping breaks silently. There is
-- exactly ONE format slot in here, bound to EPOCH on the first line, because string.format with
-- a single argument and two slots is an error rather than a repeat.
--
-- Coordinates are projections onto the vehicle's LIVE direction vectors, never design-space
-- nd.pos. That is not merely the convention vehicleGeometry follows -- on this vehicle it is
-- mandatory. The whole cannon assembly tilts on its ramL/ramR hydraulic cylinders, so any
-- geometry derived from design space is a lie the moment the operator touches the inclination
-- control.
--
-- The chunk ALWAYS replies, including when it finds nothing. vehicleGeometry's returns silently
-- in that case, which makes "no answer yet" and "nothing to say" indistinguishable and costs
-- three retries over nine seconds to tell apart. Here the difference is the whole cost model:
-- an ordinary car must cost exactly one chunk and one reply, ever.
local VEH_SCRIPT = [[
local EPOCH = %d
local function reply(cidCsv, metaCsv, reason)
  obj:queueGameEngineLua(
    "if extensions.rampGeometry then extensions.rampGeometry.onRampGeometry("
    .. obj:getID() .. "," .. EPOCH .. ",'" .. cidCsv .. "','" .. metaCsv
    .. "','" .. reason .. "') end")
end
local ok, err = pcall(function()
  if not (v.data and v.data.nodes) then return reply("", "", "no node data") end
  local fwd = vec3(obj:getDirectionVector())
  local up  = vec3(obj:getDirectionVectorUp())
  -- MUST be up:cross(fwd), never the negation. implementProximity projects onto its own
  -- lateral vector and compares the result against values measured along this one; if the two
  -- disagree every lateral reading mirrors, and nothing but a grep enforces it.
  local rgt = up:cross(fwd)

  local mn, mf, mr, mu = 0, 0, 0, 0
  local rn, sf, sr, su = 0, 0, 0, 0
  local rc, xf, xr, xu = {}, {}, {}, {}
  for _, nd in pairs(v.data.nodes) do
    local lp = vec3(obj:getNodePosition(nd.cid))
    local f, r, u = fwd:dot(lp), rgt:dot(lp), up:dot(lp)
    mn = mn + 1
    mf, mr, mu = mf + f, mr + r, mu + u
    if nd.name and nd.name:find("]] .. RAMP_NODE_PAT .. [[") then
      rn = rn + 1
      rc[rn], xf[rn], xr[rn], xu[rn] = nd.cid, f, r, u
      sf, sr, su = sf + f, sr + r, su + u
    end
  end
  if mn < 1 then return reply("", "", "no nodes") end
  if rn < ]] .. MIN_RAMP_NODES .. [[ then
    return reply("", "", "only " .. rn .. " ramp nodes")
  end

  mf, mr, mu = mf / mn, mr / mn, mu / mn
  sf, sr, su = sf / rn, sr / rn, su / rn
  local df, dr, du = sf - mf, sr - mr, su - mu
  local af, ar, au = math.abs(df), math.abs(dr), math.abs(du)

  -- Dominant axis of the centroid displacement. Vertical is rejected outright: a node set
  -- displaced from the machine centroid mostly in Z is not a ramp we understand, and guessing
  -- would be worse than saying so.
  local along, lat, disp, oth
  if af >= ar and af >= au then
    along, lat, disp, oth = xf, xr, df, math.max(ar, au)
  elseif ar >= af and ar >= au then
    along, lat, disp, oth = xr, xf, dr, math.max(af, au)
  else
    return reply("", "", "ramp displaced vertically, not along the machine")
  end
  local dm = math.abs(disp)
  if dm < ]] .. MIN_AXIS_DISP_M .. [[ or dm < ]] .. AXIS_DOMINANCE .. [[ * oth then
    -- No string.format anywhere in this chunk, not even in a comment: this text is itself the
    -- subject of an outer string.format, so a percent sign here breaks the resolve at load
    -- time, in another VM, silently. That is what the sim's format check is for.
    return reply("", "", "along-axis not dominant (" .. tostring(dm)
      .. " m vs " .. tostring(oth) .. " m)")
  end

  -- The ramp hangs off the machine in the direction of `disp`, so the MOUTH is its far end.
  -- Derived, never assumed from a jbeam axis convention.
  local sgn = 1
  if disp < 0 then sgn = -1 end
  local tmin, tmax = math.huge, -math.huge
  local t = {}
  for i = 1, rn do
    t[i] = sgn * along[i]
    if t[i] < tmin then tmin = t[i] end
    if t[i] > tmax then tmax = t[i] end
  end
  local span = tmax - tmin
  if span < ]] .. MIN_AXIS_DISP_M .. [[ then
    return reply("", "", "ramp span only " .. tostring(span) .. " m")
  end
  local rowBand = span * ]] .. ROW_BAND .. [[

  local mouth, inner = {}, {}
  for i = 1, rn do
    if t[i] >= tmax - rowBand then mouth[#mouth + 1] = i end
    if t[i] <= tmin + rowBand then inner[#inner + 1] = i end
  end
  if #mouth < 3 or #inner < 2 then
    return reply("", "", "end rows too sparse (" .. #mouth .. "/" .. #inner .. ")")
  end

  -- Floor plane of each row, and the classification every pick below depends on.
  local function floorOf(row)
    local lo = math.huge
    for _, i in ipairs(row) do if xu[i] < lo then lo = xu[i] end end
    return lo
  end
  local mouthFloorU = floorOf(mouth)
  local innerFloorU = floorOf(inner)

  -- ===========================================================================================
  -- THE WALL RULE. The single most important block in this file.
  --
  -- The drivable half-width is NOT the lateral extreme of the lowest vertical band. That is
  -- the implement resolver's floor-band rule, and here it is simply wrong. On large_cannon's
  -- mouth row the floor sits at z 0.000 and holds ramp_M_1a (x 0) out through ramp_L_5a
  -- (x 2.155) -- and ALSO ramp_L_8a at x 2.785, which is the OUTER FOOT OF THE SIDE WALL and
  -- is at exactly floor height. The wall proper is ramp_L_6a/7a at z 0.383. So the floor-band
  -- rule answers 2.785 where the truth is 2.148: 0.63 m of phantom clearance per side, which
  -- on a car is the difference between fitting and tearing the mirrors off.
  --
  -- The wall is where something STICKS UP -- but only up to a point. Per side, take the
  -- smallest |lateral| among mouth-row nodes that are between WALL_MIN_H and WALL_MAX_H above
  -- the mouth floor plane and more than MIN_WALL_LATERAL_M off the centreline. Both of those
  -- extra clauses exist to keep an overhead node out of the pick; see the constants for which
  -- catches what and why neither is sufficient alone. A node above WALL_MAX_H is overhead
  -- structure -- neither wall nor floor -- and takes no part in any of this.
  --
  -- Where a side has no wall node at all, fall back to its floor extreme and record that it
  -- happened, so a half-width produced by the fallback is never mistaken for one produced by
  -- the rule.
  --
  -- The naive answer is computed alongside and shipped purely so the log line can print both.
  -- A disagreement between them is then one line to diagnose instead of a session.
  -- ===========================================================================================
  local wallL, wallR, floorL, floorR = nil, nil, nil, nil
  local floorLatHi, floorLatLo = -math.huge, math.huge
  for _, i in ipairs(mouth) do
    local h = xu[i] - mouthFloorU
    local raised = h > ]] .. WALL_MIN_H .. [[ and h <= ]] .. WALL_MAX_H .. [[
    local off = math.abs(xr[i])
    if raised and off > ]] .. MIN_WALL_LATERAL_M .. [[ then
      if xr[i] > 0 then
        if (not wallL) or xr[i] < xr[wallL] then wallL = i end
      else
        if (not wallR) or xr[i] > xr[wallR] then wallR = i end
      end
    elseif h <= ]] .. WALL_MIN_H .. [[ then
      -- Explicitly the floor band, not merely "not a wall": an overhead node fails the wall
      -- test too, and letting it fall through to here would let it set the floor extreme.
      if xr[i] > floorLatHi then floorLatHi, floorL = xr[i], i end
      if xr[i] < floorLatLo then floorLatLo, floorR = xr[i], i end
    end
  end
  local wallUsed = 0
  if wallL then wallUsed = wallUsed + 1 else wallL = floorL end
  if wallR then wallUsed = wallUsed + 1 else wallR = floorR end
  if not (wallL and wallR) then
    return reply("", "", "could not find both mouth edges")
  end
  local halfW = (xr[wallL] - xr[wallR]) * 0.5
  local naiveHalfW = 0
  if floorL and floorR then naiveHalfW = (floorLatHi - floorLatLo) * 0.5 end
  if halfW < ]] .. MIN_MOUTH_WIDTH_M .. [[ then
    return reply("", "", "mouth only " .. tostring(halfW * 2) .. " m wide")
  end

  -- The centre marker is picked against the midpoint of the two edges, NOT against the vehicle
  -- centreline. On a centred ramp the two rules agree, which is exactly why the centreline rule
  -- looks right; on anything offset it lands well off the pair's centre. Candidates are
  -- restricted to FLOOR nodes, or ramp_M_0 -- 2 m up and dead on the centreline -- wins it and
  -- the "centre" of the mouth ends up in mid-air.
  local midLat = (xr[wallL] + xr[wallR]) * 0.5
  local mouthC, bestC = nil, math.huge
  for _, i in ipairs(mouth) do
    if (xu[i] - mouthFloorU) <= ]] .. WALL_MIN_H .. [[ then
      local d = math.abs(xr[i] - midLat)
      if d < bestC then bestC, mouthC = d, i end
    end
  end
  if not mouthC then mouthC = wallL end

  -- The inner row exists only to give the ramp axis a second point, so it takes the plain
  -- floor extremes rather than the wall rule: their MIDPOINT is all that is consumed, and the
  -- inner row's raised nodes on this vehicle curve inward to well under half a metre, which
  -- would make a needlessly jittery baseline for a value that is then averaged away anyway.
  local innerL, innerR = nil, nil
  local iHi, iLo = -math.huge, math.huge
  for _, i in ipairs(inner) do
    if (xu[i] - innerFloorU) <= ]] .. WALL_MIN_H .. [[ then
      if xr[i] > iHi then iHi, innerL = xr[i], i end
      if xr[i] < iLo then iLo, innerR = xr[i], i end
    end
  end
  if not (innerL and innerR) then
    return reply("", "", "inner row has no floor band")
  end

  local cids = rc[wallL] .. "," .. rc[mouthC] .. "," .. rc[wallR] .. ","
            .. rc[innerL] .. "," .. rc[innerR]
  local meta = tostring(halfW) .. "," .. tostring(span) .. "," .. tostring(mouthFloorU) .. ","
            .. tostring(rn) .. "," .. tostring(wallUsed) .. "," .. tostring(naiveHalfW)
  reply(cids, meta, "")
end)
if not ok then
  obj:queueGameEngineLua("log('E','rampGeometry','vehicle-side resolve failed: "
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

-- Called back from the vehicle VM. Stale replies are dropped on the epoch, not on the id: the
-- id survives a reset, so an id check alone would install pre-reset cids for a part
-- configuration that has since changed.
function M.onRampGeometry(vehID, epoch, cidCsv, metaCsv, reason)
  local p = pending[vehID]
  if not p or p.epoch ~= epoch then
    rgLog('D', string.format("stale ramp reply for %s (epoch %s)",
      tostring(vehID), tostring(epoch)))
    return
  end
  pending[vehID] = nil

  local cids = parseNums(cidCsv)
  local meta = parseNums(metaCsv)

  -- No ramp, or one we could not make sense of. Terminal on the FIRST round trip, which is the
  -- property that keeps this extension free on ordinary vehicles: a car costs one chunk and one
  -- reply for the whole session, not three chunks and a warning. Logged at D rather than W
  -- because "this vehicle is not a cannon" is the overwhelmingly common case and is not news.
  if #cids == 0 then
    -- ...unless the VM was simply not ready. "no node data" and "no nodes" are not answers
    -- about the vehicle, they are answers about the moment: v.data.nodes does not exist yet
    -- while a vehicle is still spawning, and the whole terminal-on-first-reply design turns
    -- that into "this machine has no ramp" for the rest of the session. The symptom is a
    -- cannon that resolves perfectly one run and is invisible the next, decided by spawn
    -- timing, which is exactly the kind of thing that gets diagnosed as "it broke after a
    -- restart". Retried under the same budget as a silent VM, so a vehicle that answers this
    -- way forever still costs MAX_TRIES chunks and no more.
    local why = tostring(reason or "")
    if why == "no node data" or why == "no nodes" then
      local n = (notReady[vehID] or 0) + 1
      notReady[vehID] = n
      if n < MAX_TRIES then
        rgLog('D', string.format(
          "vehicle %s was not ready to answer the ramp resolve (%s), attempt %d of %d",
          tostring(vehID), why, n, MAX_TRIES))
        return  -- pending is already cleared, so the next M.request re-issues
      end
    end
    failed[vehID] = true
    rgLog('D', string.format("vehicle %s has no usable ramp: %s",
      tostring(vehID), why ~= "" and why or "no reason given"))
    return
  end
  -- A VM answering with the wrong shape will keep answering with the wrong shape, so this is
  -- terminal too -- the same guard, and the same reasoning, as vehicleGeometry's #ext ~= 6.
  if #cids ~= 5 or #meta ~= 6 then
    failed[vehID] = true
    rgLog('W', string.format("vehicle %s returned %d cids and %d meta, expected 5 and 6",
      tostring(vehID), #cids, #meta))
    return
  end

  cache[vehID] = {
    cids       = cids,
    halfW      = meta[1],
    alongSpan  = meta[2],
    floorU     = meta[3],
    nNodes     = meta[4],
    wallUsed   = meta[5],
    naiveHalfW = meta[6],
  }
  -- The naive half-width is printed even when the wall rule worked. It is the one line that
  -- turns "the clearance feels wrong" into an answer, and it costs nothing to carry.
  rgLog('I', string.format(
    "ramp on vehicle %d: %d nodes, span %.2f m along, mouth half-width %.3f m (%s; "
    .. "naive floor-band pick would have said %.3f m)",
    vehID, meta[4], meta[2], meta[1],
    (meta[5] >= 2) and "wall rule both sides"
      or string.format("wall rule on %d of 2 sides, floor extreme on the rest", meta[5]),
    meta[6]))
end

-- Fire a resolve for this vehicle unless one is cached, in flight, or known hopeless. Cheap to
-- call every tick; callers are not expected to track state.
function M.request(vehID)
  if not vehID or cache[vehID] or pending[vehID] or failed[vehID] then return end
  local veh = scenetree.findObjectById(vehID)
  if not veh then return end
  epochCounter = epochCounter + 1
  pending[vehID] = {epoch = epochCounter, timer = 0, tries = 1}
  pcall(function() veh:queueLuaCommand(string.format(VEH_SCRIPT, epochCounter)) end)
end

function M.get(vehID)
  return cache[vehID]
end

-- Does this vehicle have a ramp? Self-arming, so a caller can poll it without also having to
-- remember to request. This doubles as the "am I sitting in a cannon" test that beamtel's
-- firing readout keys off -- derived from geometry the mod already resolves, rather than from
-- a jbeam name allowlist that would need maintaining for every ramp vehicle ever released.
function M.has(vehID)
  M.request(vehID)
  return cache[vehID] ~= nil
end

-- The resolved cross-VM chunk, for diagnostics. Exists for the reason vehicleGeometry's does:
-- a syntax error inside a queueLuaCommand string does not fail where it is written, it fails
-- later in another VM, where the only symptom is that ramps never resolve and every caller
-- sits silent for the whole session. ramp_resolve_sim.lua compiles this so that class of error
-- is caught before the game is ever started.
function M.debugVehScript()
  return string.format(VEH_SCRIPT, 0)
end

-- =================================================================================================
--  Queries
-- =================================================================================================

-- The live world frame of the ramp mouth. Five getNodePosition calls -- the same per-tick
-- budget the implement's sample set already costs -- and everything else is derived from them,
-- so a ramp that has been bent, or tilted by its own hydraulics, is reported rather than
-- remembered.
function M.mouthFrame(vehID)
  local entry = cache[vehID]
  if not entry then return nil end
  local veh = scenetree.findObjectById(vehID)
  if not veh then return nil end

  local ok, res = pcall(function()
    local base = vec3(veh:getPosition())
    local p = {}
    for i = 1, 5 do
      p[i] = base + vec3(veh:getNodePosition(entry.cids[i]))
    end
    local mouthL, mouthC, mouthR, innerL, innerR = p[1], p[2], p[3], p[4], p[5]

    -- The two-point midpoint of the edges, NOT the mean of all three. mouthC is a real point
    -- but it is not a centre, and averaging it in drags the origin off the midline on anything
    -- asymmetric -- exactly the argument getImplementFrame makes for excluding edgeC.
    local centre   = (mouthL + mouthR) * 0.5
    local innerMid = (innerL + innerR) * 0.5

    local halfW = mouthL:distance(mouthR) * 0.5
    if halfW < MIN_MOUTH_WIDTH_M then return nil end

    -- Points INTO the ramp, which is the direction the driver travels.
    local raw = innerMid - centre
    local rise = raw.z
    local axis = vec3(raw.x, raw.y, 0)
    if axis:length() < 1e-4 then return nil end
    axis = axis:normalized()

    -- Positive-LEFT, the mod-wide convention. Same construction as implementProximity's dock
    -- readout; if this ever becomes fwd:cross(up) every lateral reading mirrors.
    local left = vec3(0, 0, 1):cross(axis)
    if left:length() < 1e-4 then return nil end
    left = left:normalized()

    local len = raw:length()
    local pitchDeg = 0
    if len > 1e-4 then
      pitchDeg = math.deg(math.asin(math.max(-1, math.min(1, rise / len))))
    end

    return {
      mouthL = mouthL, mouthC = mouthC, mouthR = mouthR,
      innerL = innerL, innerR = innerR,
      centre = centre, innerMid = innerMid,
      axis = axis, left = left,
      halfW = halfW,
      floorZ = math.min(mouthL.z, math.min(mouthC.z, mouthR.z)),
      pitchDeg = pitchDeg,
    }
  end)
  if not ok then return nil end
  return res
end

-- =================================================================================================
--  Lifecycle
-- =================================================================================================

local function dropVehicle(vehID)
  if vehID == nil then return end
  cache[vehID]   = nil
  pending[vehID] = nil
  -- The failed flag is cleared too. A reset or a part swap is the realistic way a vehicle
  -- acquires or loses a ramp, and it is the only thing that makes a fresh attempt possible.
  failed[vehID]  = nil
  notReady[vehID] = nil
end

local function dropAll()
  cache, pending, failed, notReady = {}, {}, {}, {}
end

function M.invalidate(vehID)
  if vehID == nil then dropAll() else dropVehicle(vehID) end
end

function M.onExtensionLoaded()
  setExtensionUnloadMode(M, "manual")
  rgLog('I', "Ramp geometry extension loaded.")
end

function M.onWorldReadyState(state)
  if state == 2 then dropAll() end
end

function M.onVehicleResetted(vehId)
  dropVehicle(vehId)
end

function M.onVehicleDestroyed(vehId)
  dropVehicle(vehId)
end

function M.onUpdate(dtReal, dtSim, dtRaw)
  -- Retry genuinely stalled resolves only. A vehicle VM that is still spawning cannot answer,
  -- and without this the entry would stay pending forever. A vehicle that answered "no ramp" is
  -- already in `failed` and never reaches this loop.
  for vehID, p in pairs(pending) do
    p.timer = p.timer + dtReal
    if p.timer >= RESOLVE_TIMEOUT_S then
      if p.tries >= MAX_TRIES then
        -- Flagged before clearing `pending`, so this is logged exactly once and the give-up is
        -- genuinely permanent rather than being re-armed by the next M.request.
        failed[vehID] = true
        rgLog('W', string.format("vehicle %s never answered the ramp resolve", tostring(vehID)))
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
