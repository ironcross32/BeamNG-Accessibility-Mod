-- Replay vehicleGeometry.lua's node classification and surface-distance queries against
-- synthetic node clouds with known ground truth.
--
--     lua diagnostic/vehicle_geometry_sim.lua
--
-- This exists because the bug it replaces was invisible to every kind of inspection except
-- measuring: vehicleScanner reported originPos:distance(targetPos), and getPosition() is the
-- object's reference node. The number looked plausible at every distance. What gave it away
-- was that it did not change when the TARGET rotated -- broadside and nose-on to the same
-- car at the same real gap reported the same figure, differing from the truth by more than a
-- metre in one of the two cases. Scenario 2 below is that exact comparison, and it is the
-- single check most worth keeping.
--
-- The tuning constants are parsed out of vehicleGeometry.lua rather than copied, so retuning
-- there cannot silently invalidate these checks. Only the *logic* is duplicated here.

local SRC = "bng_mod/lua/ge/extensions/vehicleGeometry.lua"
local PROX_SRC = "bng_mod/lua/ge/extensions/implementProximity.lua"
local RAMP_SRC = "bng_mod/lua/ge/extensions/rampGeometry.lua"
local SCANNER_SRC = "bng_mod/lua/ge/extensions/vehicleScanner.lua"

local function readConstFrom(path, name)
  local fh = assert(io.open(path, "r"), "cannot open " .. path)
  local body = fh:read("*a")
  fh:close()
  local val = body:match("\nlocal " .. name .. "%s*=%s*([%-%d%.]+)")
  assert(val, "could not find " .. name .. " in " .. path)
  return tonumber(val)
end

local function readConst(name) return readConstFrom(SRC, name) end

local FACE_FRAC   = readConst("FACE_FRAC")
local BAND_FRAC   = readConst("BAND_FRAC")
local HULL_MAX    = readConst("HULL_MAX")
local BAND_MAX    = readConst("BAND_MAX")
local HIST_BINS   = readConst("HIST_BINS")
local NEAR_NODE_M = readConst("NEAR_NODE_M")
local OUTLIER_GAP_M = readConst("OUTLIER_GAP_M")
local TRIM_MAX_FRAC = readConst("TRIM_MAX_FRAC")

-- =================================================================================================
--  Minimal vec3, matching the operations the extension uses
-- =================================================================================================

local vec = {}
vec.__index = vec
local function v3(x, y, z) return setmetatable({x = x, y = y, z = z}, vec) end
vec.__add = function(a, b) return v3(a.x + b.x, a.y + b.y, a.z + b.z) end
vec.__sub = function(a, b) return v3(a.x - b.x, a.y - b.y, a.z - b.z) end
vec.__mul = function(a, s) return v3(a.x * s, a.y * s, a.z * s) end
function vec:dot(o) return self.x * o.x + self.y * o.y + self.z * o.z end
function vec:length() return math.sqrt(self:dot(self)) end
function vec:normalized() local l = self:length(); return v3(self.x / l, self.y / l, self.z / l) end
function vec:cross(o)
  return v3(self.y * o.z - self.z * o.y,
            self.z * o.x - self.x * o.z,
            self.x * o.y - self.y * o.x)
end
function vec:distance(o) return (self - o):length() end
function vec:squaredDistance(o) local d = self - o; return d:dot(d) end

-- =================================================================================================
--  The logic under test, mirroring vehicleGeometry.lua
-- =================================================================================================

-- Mirrors VEH_SCRIPT's two passes. Input is a list of {cid, pos} in the vehicle's own frame
-- (which is what fwd/rgt/up dotted against getNodePosition produces).
local function classify(nodes)
  local n = #nodes
  local function trimmed(axis)
    local s = {}
    for i = 1, n do s[i] = nodes[i].pos[axis] end
    table.sort(s)
    local maxCut = math.floor(n * TRIM_MAX_FRAC)
    local lo, hi = 1, n
    for j = n, 2, -1 do
      if (n - j + 1) > maxCut then break end
      if s[j] - s[j - 1] > OUTLIER_GAP_M then hi = j - 1; break end
    end
    for j = 1, hi - 1 do
      if (j - 1) > maxCut then break end
      if s[j + 1] - s[j] > OUTLIER_GAP_M then lo = j + 1; break end
    end
    return s[lo], s[hi]
  end
  local minF, maxF = trimmed("y")
  local minR, maxR = trimmed("x")
  local minU, maxU = trimmed("z")
  local fRange, rRange, uRange = maxF - minF, maxR - minR, maxU - minU

  local hull, front, rear, hist = {}, {}, {}, {}
  for i = 1, HIST_BINS do hist[i] = 0 end
  for _, nd in ipairs(nodes) do
    local p = nd.pos
    local fN = fRange > 0.01 and (p.y - minF) / fRange or 0.5
    local rN = rRange > 0.01 and (p.x - minR) / rRange or 0.5
    local uN = uRange > 0.01 and (p.z - minU) / uRange or 0.5
    if fN < FACE_FRAC or fN > 1 - FACE_FRAC or rN < FACE_FRAC or rN > 1 - FACE_FRAC
        or uN < FACE_FRAC or uN > 1 - FACE_FRAC then
      hull[#hull + 1] = nd
    end
    if fN > 1 - BAND_FRAC then front[#front + 1] = nd end
    if fN < BAND_FRAC then rear[#rear + 1] = nd end
    local b = math.floor(uN * HIST_BINS) + 1
    if b < 1 then b = 1 end
    if b > HIST_BINS then b = HIST_BINS end
    hist[b] = hist[b] + 1
  end

  local function thin(t, cap)
    if #t <= cap then return t end
    local step, out, acc = #t / cap, {}, 1.0
    while #out < cap and math.floor(acc) <= #t do
      out[#out + 1] = t[math.floor(acc)]
      acc = acc + step
    end
    return out
  end

  return {
    ext   = {minF = minF, maxF = maxF, minR = minR, maxR = maxR, minU = minU, maxU = maxU},
    hull  = thin(hull, HULL_MAX),
    front = thin(front, BAND_MAX),
    rear  = thin(rear, BAND_MAX),
    hist  = hist,
  }
end

local function clamp(v, lo, hi)
  if v < lo then return lo end
  if v > hi then return hi end
  return v
end

local function closestOnBox(frame, ext, p)
  local d = p - frame.c
  local f = clamp(frame.f:dot(d), ext.minF, ext.maxF)
  local r = clamp(frame.r:dot(d), ext.minR, ext.maxR)
  local u = clamp(frame.u:dot(d), ext.minU, ext.maxU)
  return frame.c + frame.f * f + frame.r * r + frame.u * u
end

-- Mirrors nearestOnContext: box tier decides, node tier refines inside NEAR_NODE_M.
local function nearestSurfacePoint(target, worldPos)
  local boxPt = closestOnBox(target.frame, target.geo.ext, worldPos)
  local boxD  = worldPos:distance(boxPt)
  if boxD > NEAR_NODE_M or #target.geo.hull == 0 then return boxPt, boxD end
  local best, bestD = nil, math.huge
  for _, nd in ipairs(target.geo.hull) do
    local wp = target.frame.c
             + target.frame.f * nd.pos.y
             + target.frame.r * nd.pos.x
             + target.frame.u * nd.pos.z
    local d = worldPos:squaredDistance(wp)
    if d < bestD then best, bestD = wp, d end
  end
  if not best then return boxPt, boxD end
  bestD = math.sqrt(bestD)
  -- The two tiers fail in opposite directions, so keep the smaller: the box under-reads
  -- (it encloses the body) but is exact at the surface, the sparse node sweep over-reads
  -- near contact but is the only thing that sees geometry outside the box.
  if boxD < bestD then return boxPt, boxD end
  return best, bestD
end

local function nearestApproach(pts, target)
  local bestD = math.huge
  for _, p in ipairs(pts) do
    local _, d = nearestSurfacePoint(target, p)
    if d < bestD then bestD = d end
  end
  return bestD
end

-- =================================================================================================
--  Synthetic bodies
-- =================================================================================================

-- A filled grid, in the vehicle's own frame: x lateral, y fore/aft, z up.
local function boxCloud(len, wide, tall, step)
  local nodes = {}
  local ny = math.max(2, math.floor(len / step))
  local nx = math.max(2, math.floor(wide / step))
  local nz = math.max(2, math.floor(tall / step))
  for i = 0, ny do
    for j = 0, nx do
      for k = 0, nz do
        nodes[#nodes + 1] = {
          cid = #nodes,
          pos = v3(-wide / 2 + wide * j / nx,
                   -len / 2 + len * i / ny,
                   -tall / 2 + tall * k / nz),
        }
      end
    end
  end
  return nodes
end

-- Place a body in the world at a position and a yaw, and classify it.
local function body(nodes, pos, yawDeg)
  local a = math.rad(yawDeg)
  local fwd = v3(math.sin(a), math.cos(a), 0)
  local up  = v3(0, 0, 1)
  local rgt = up:cross(fwd)
  return {
    geo = classify(nodes),
    frame = {c = pos, f = fwd, u = up, r = rgt},
  }
end

-- =================================================================================================
--  Checks
-- =================================================================================================

local failures = {}

local function check(label, ok, detail)
  print(string.format("   %s: %s%s", label, ok and "OK" or "FAIL",
    detail and (" - " .. detail) or ""))
  if not ok then failures[#failures + 1] = label end
end

local function approx(a, b, tol) return math.abs(a - b) <= tol end

local CAR_LEN, CAR_WIDE, CAR_TALL = 4.4, 1.8, 1.4
local carNodes = boxCloud(CAR_LEN, CAR_WIDE, CAR_TALL, 0.25)

print(string.format(
  "tuning: face %.2f  band %.2f  hull cap %d  band cap %d  bins %d  node tier %.1f m",
  FACE_FRAC, BAND_FRAC, HULL_MAX, BAND_MAX, HIST_BINS, NEAR_NODE_M))
print(string.format("car: %d nodes, %.1f x %.1f x %.1f m", #carNodes, CAR_LEN, CAR_WIDE, CAR_TALL))
print()

print("0. the extension and its cross-VM chunk both compile")
do
  -- The chunk is assembled as a string and only ever executed inside a vehicle's own Lua VM,
  -- so a syntax error in it does not fail here -- it fails over there, silently, and the
  -- only symptom in game is that geometry never resolves for anything.
  local chunk, err = loadfile(SRC)
  check("vehicleGeometry.lua parses", chunk ~= nil, err or "")
  if chunk then
    -- setExtensionUnloadMode and friends are only touched inside hooks, so running the
    -- module body needs no game stubs.
    local ok, mod = pcall(chunk)
    check("module body runs", ok and type(mod) == "table", not ok and tostring(mod) or "")
    if ok and type(mod) == "table" and mod.debugVehScript then
      local src = mod.debugVehScript()
      local c2, e2 = load(src, "vehScript")
      check("the vehicle-VM chunk parses", c2 ~= nil, e2 or "")
      check("no unsubstituted format specifier survived", not src:find("%%[dqs]"),
        "a stray %d would be sent to the vehicle VM verbatim")
    end
  end
end
print()

print("1. classification produces a usable hull and two distinct contact bands")
do
  local g = classify(carNodes)
  check("hull is non-empty and capped", #g.hull > 0 and #g.hull <= HULL_MAX,
    string.format("%d hull nodes (cap %d)", #g.hull, HULL_MAX))
  check("front band non-empty and capped", #g.front > 0 and #g.front <= BAND_MAX,
    string.format("%d front nodes", #g.front))
  check("rear band non-empty and capped", #g.rear > 0 and #g.rear <= BAND_MAX,
    string.format("%d rear nodes", #g.rear))

  -- The bands must not overlap, or "the business end in reverse" would include the nose.
  local shared = 0
  for _, a in ipairs(g.front) do
    for _, b in ipairs(g.rear) do
      if a.cid == b.cid then shared = shared + 1 end
    end
  end
  check("front and rear bands are disjoint", shared == 0,
    string.format("%d shared cids", shared))

  -- And they must be at opposite ends, not merely different.
  local fMin, rMax = math.huge, -math.huge
  for _, n in ipairs(g.front) do if n.pos.y < fMin then fMin = n.pos.y end end
  for _, n in ipairs(g.rear)  do if n.pos.y > rMax then rMax = n.pos.y end end
  check("front band is ahead of rear band", fMin > rMax,
    string.format("front starts at %.2f, rear ends at %.2f", fMin, rMax))

  check("extents recover the real body size",
    approx(g.ext.maxF - g.ext.minF, CAR_LEN, 0.01)
    and approx(g.ext.maxR - g.ext.minR, CAR_WIDE, 0.01)
    and approx(g.ext.maxU - g.ext.minU, CAR_TALL, 0.01),
    string.format("%.2f x %.2f x %.2f",
      g.ext.maxF - g.ext.minF, g.ext.maxR - g.ext.minR, g.ext.maxU - g.ext.minU))
end
print()

print("2. THE BUG: same real gap, different target orientation, must report the same number")
do
  local GAP = 2.0
  -- Nose-on: probe sits ahead of a car pointing at us, GAP clear of its front face.
  local carA = body(carNodes, v3(0, 0, 0), 0)
  local probeNose = v3(0, CAR_LEN / 2 + GAP, 0)
  local _, dNose = nearestSurfacePoint(carA, probeNose)

  -- Broadside: same car turned 90 degrees, probe GAP clear of its (now facing) side.
  local carB = body(carNodes, v3(0, 0, 0), 90)
  local probeSide = v3(0, CAR_WIDE / 2 + GAP, 0)
  local _, dSide = nearestSurfacePoint(carB, probeSide)

  check("nose-on gap is the true gap", approx(dNose, GAP, 0.15),
    string.format("reported %.3f m, truth %.2f m", dNose, GAP))
  check("broadside gap is the true gap", approx(dSide, GAP, 0.15),
    string.format("reported %.3f m, truth %.2f m", dSide, GAP))
  check("the two agree", approx(dNose, dSide, 0.15),
    string.format("nose-on %.3f vs broadside %.3f", dNose, dSide))

  -- And prove the sim would have caught the old implementation, which measured to the
  -- target's origin and so could not tell these two cases apart at all.
  local oldNose = probeNose:distance(v3(0, 0, 0))
  local oldSide = probeSide:distance(v3(0, 0, 0))
  check("old centre-to-centre measure was wrong in at least one pose",
    not approx(oldNose, GAP, 0.15) or not approx(oldSide, GAP, 0.15),
    string.format("centre-to-centre reported %.2f nose-on and %.2f broadside for a %.2f m gap",
      oldNose, oldSide, GAP))
end
print()

print("3. gap shrinks monotonically on a straight approach and reaches zero at contact")
do
  local car = body(carNodes, v3(0, 0, 0), 0)
  local prev, mono = math.huge, true
  for i = 0, 40 do
    local y = CAR_LEN / 2 + 4.0 - i * 0.1
    local _, d = nearestSurfacePoint(car, v3(0, y, 0))
    if d > prev + 1e-6 then mono = false end
    prev = d
  end
  check("monotonic while closing", mono, "")
  local _, dTouch = nearestSurfacePoint(car, v3(0, CAR_LEN / 2, 0))
  check("zero at the surface", dTouch < 0.15, string.format("%.3f m at contact", dTouch))
  -- Inside the body the clamped box query returns 0 rather than a negative number, which is
  -- what lets the Python side floor it without hiding a sign error.
  local _, dIn = nearestSurfacePoint(car, v3(0, 0, 0))
  check("never negative inside the body", dIn >= 0.0, string.format("%.3f m at the centre", dIn))
end
print()

print("4. the box and node tiers agree across the handover at NEAR_NODE_M")
do
  local car = body(carNodes, v3(0, 0, 0), 0)
  local worst = 0
  for i = -6, 6 do
    local y = CAR_LEN / 2 + NEAR_NODE_M + i * 0.05
    local p = v3(0, y, 0)
    local _, dMixed = nearestSurfacePoint(car, p)
    local dBox = p:distance(closestOnBox(car.frame, car.geo.ext, p))
    if math.abs(dMixed - dBox) > worst then worst = math.abs(dMixed - dBox) end
  end
  -- A discontinuity here would be audible: the scanner beep rate is exp(-d/half), so a jump
  -- at the handover would make the train stutter at a fixed distance from every target.
  check("no step at the tier handover", worst < 0.2,
    string.format("largest box-vs-node disagreement %.3f m", worst))
end
print()

print("5. a contact SET beats a single point -- a wide implement reaches first at its corner")
do
  local car = body(carNodes, v3(0, 0, 0), 0)
  -- Five implement points spanning 2.4 m, approaching a car that is off to one side.
  local carOff = body(carNodes, v3(2.0, 0, 0), 0)
  local implement = {}
  for i = -2, 2 do implement[#implement + 1] = v3(i * 0.6, 3.0, 0) end
  local setGap = nearestApproach(implement, carOff)
  local _, centreGap = nearestSurfacePoint(carOff, v3(0, 3.0, 0))
  check("set reports a nearer gap than its centre point alone", setGap < centreGap,
    string.format("set %.3f m vs centre-only %.3f m", setGap, centreGap))
  check("single-point set matches the point query",
    approx(nearestApproach({v3(0, 3.0, 0)}, car),
           select(2, nearestSurfacePoint(car, v3(0, 3.0, 0))), 1e-9), "")
end
print()

print("6. a detached part is seen, and does not drag the box out over the empty air")
do
  -- A bumper that has broken off stays part of the SAME object in BeamNG, so nothing that
  -- only knows where the parent vehicle is can see it. Put one 3 m off the car's flank.
  local nodes = {}
  for _, nd in ipairs(carNodes) do nodes[#nodes + 1] = {cid = nd.cid, pos = nd.pos} end
  for i = 0, 8 do
    nodes[#nodes + 1] = {cid = #nodes, pos = v3(3.0 + 0.05 * i, -0.4 + 0.1 * i, -0.6)}
  end
  local wreck = body(nodes, v3(0, 0, 0), 0)
  -- Probe positions are built from the frame rather than hardcoded in world axes, because
  -- r = up x fwd does not point along +x and a hardcoded probe would silently test the far
  -- side of the car.
  local function at(r, f, u)
    return wreck.frame.c + wreck.frame.r * r + wreck.frame.f * f + wreck.frame.u * u
  end

  local _, dDebris = nearestSurfacePoint(wreck, at(3.6, 0, -0.6))
  check("debris 0.2 m away is reported as close", dDebris < 0.5,
    string.format("%.3f m (the car's own flank is %.2f m from that probe)",
      dDebris, 3.6 - CAR_WIDE / 2))

  -- The trimmed extents are what stop the box spanning car-to-debris. Without them the
  -- midpoint of that empty air would read as solid surface and the docking instrument would
  -- report contact with nothing at all.
  local _, dAir = nearestSurfacePoint(wreck, at(2.0, 0, -0.6))
  check("the empty air between them is not reported as surface", dAir > 0.8,
    string.format("%.3f m at the midpoint (nearest real material is ~%.2f m)",
      dAir, 3.0 - 2.0))
  check("trimmed extents kept the box on the body",
    wreck.geo.ext.maxR < 2.0,
    string.format("box reaches %.2f m laterally; the body ends at %.2f",
      wreck.geo.ext.maxR, CAR_WIDE / 2))
end
print()

print("7. the vertical histogram shows a pallet's void as a gap between two solid runs")
do
  -- A pallet, built by filling a grid and keeping only where material exists, so the void is
  -- genuinely empty rather than an artefact of how the generator was written. The band
  -- selector's whole job is finding that void, so it has to survive binning.
  local nodes = {}
  local DECK_Z0, TOP = 0.10, 0.15
  for i = 0, 16 do
    for j = 0, 10 do
      for k = 0, 14 do
        local x = -0.4 + 0.8 * i / 16
        local y = -0.6 + 1.2 * j / 10
        local z = TOP * k / 14
        local solid = (z >= DECK_Z0)                       -- deck
                   or (x <= -0.30 or x >= 0.30)            -- the two blocks
        if solid then nodes[#nodes + 1] = {cid = #nodes, pos = v3(x, y, z)} end
      end
    end
  end
  local g = classify(nodes)
  check("histogram is populated", #nodes > 0, string.format("%d nodes", #nodes))

  -- Collapse into runs the way the band selector will: find a low-occupancy run with
  -- populated bins above it. That pattern IS the fork pocket.
  local peak = 0
  for i = 1, HIST_BINS do if g.hist[i] > peak then peak = g.hist[i] end end
  local thresh = peak * 0.35
  local voidLo, voidHi = nil, nil
  for i = 1, HIST_BINS do
    if g.hist[i] < thresh then
      if not voidLo then voidLo = i end
      voidHi = i
    elseif voidLo and voidHi and voidHi < i - 1 then
      break
    end
  end
  check("a low-occupancy run exists", voidLo ~= nil,
    voidLo and string.format("bins %d-%d of %d", voidLo, voidHi, HIST_BINS) or "none found")
  local above = 0
  if voidHi then
    for i = voidHi + 1, HIST_BINS do above = above + g.hist[i] end
  end
  check("solid material sits above the void", above > 0,
    string.format("%d nodes above bin %s", above, tostring(voidHi)))
  check("the void is in the lower half, where a pocket belongs",
    voidLo ~= nil and voidLo <= HIST_BINS * 0.6,
    voidLo and string.format("void starts at bin %d", voidLo) or "")
end
print()

print("8. reversing flips the heading but NOT which side is left")
do
  -- Mirrors the bearing block in vehicleScanner.scanAndSendVehicleData. The whole point is
  -- the asymmetry: forwardVec is negated in reverse so a target dead behind reads ~0
  -- degrees, but playerLeftVec must keep being built from the UN-negated vector. up x fwd
  -- negates along with fwd, so using the reversed heading there swaps left and right for as
  -- long as the vehicle is in reverse. The convention is positive = LEFT in every gear, and
  -- getting it backwards has already produced one false bug report.
  local function bearingDeg(fwd, up, origin, target, dir, useNegatedForLeft)
    local heading = fwd
    if dir < 0 then heading = v3(-fwd.x, -fwd.y, -fwd.z) end
    local toT = (target - origin):normalized()
    local ang = math.deg(math.acos(clamp(heading:dot(toT), -1, 1)))
    -- useNegatedForLeft reproduces the bug, so the check below proves it is discriminating
    -- rather than passing for free.
    local left = up:cross(useNegatedForLeft and heading or fwd)
    return ang * (left:dot(toT) < 0 and -1 or 1)
  end

  local fwd, up, origin = v3(0, 1, 0), v3(0, 0, 1), v3(0, 0, 0)
  -- Facing +y with up +z, up x fwd = (-1,0,0): the driver's left is -x.
  local aheadLeft  = v3(-1, 3, 0)
  local behindLeft = v3(-1, -3, 0)

  local fwdBearing = bearingDeg(fwd, up, origin, aheadLeft, 1, false)
  check("forward, target ahead-left reads LEFT", fwdBearing > 0,
    string.format("%.1f deg", fwdBearing))

  local revAngle = bearingDeg(fwd, up, origin, behindLeft, -1, false)
  check("reverse, target behind reads a small angle, not ~180",
    math.abs(revAngle) < 45,
    string.format("%.1f deg reversing toward it", revAngle))

  local fwdAngle = bearingDeg(fwd, up, origin, behindLeft, 1, false)
  check("the same target in forward still reads ~180", math.abs(fwdAngle) > 135,
    string.format("%.1f deg", fwdAngle))

  check("reverse, target on the driver's left still reads LEFT", revAngle > 0,
    string.format("%.1f deg (positive must mean left in every gear)", revAngle))

  -- And prove the check would fail if the cross product were fed the reversed heading.
  local buggy = bearingDeg(fwd, up, origin, behindLeft, -1, true)
  check("building left from the negated heading would invert it",
    (buggy < 0) ~= (revAngle < 0),
    string.format("correct %.1f vs bug %.1f", revAngle, buggy))
end
print()

print("9. the slam gate latches on decisions, and does not chatter at the thresholds")
do
  -- Mirrors resolveSlam in implementProximity.lua. Nulling is the wrong shape for dropping a
  -- bucket on something: you want to be decisively above, then over, then commit. So this is
  -- three discrete states, and the whole risk in three discrete states is that they flutter
  -- at the boundary -- which for a cue meant to mark a decision point is worse than useless.
  local ENTER = readConstFrom(PROX_SRC, "SLAM_CLEAR_ENTER_M")
  local EXIT  = readConstFrom(PROX_SRC, "SLAM_CLEAR_EXIT_M")
  local MINPT = readConstFrom(PROX_SRC, "SLAM_OVER_MIN_PTS")

  local state = "NONE"
  -- implMinZ is the implement's lowest point; boxTop the target's highest; nOver how many
  -- implement points fall inside the target's plan outline.
  local function step(implMinZ, boxTop, nOver)
    local wasClear = (state == "CLEAR" or state == "COMMITTED")
    local margin = wasClear and EXIT or ENTER
    local clear = implMinZ >= (boxTop + margin)
    local wasOver = (state == "OVER" or state == "COMMITTED")
    local over = (wasOver and nOver > 0) or (nOver >= MINPT)
    if clear and over then state = "COMMITTED"
    elseif clear then state = "CLEAR"
    elseif over then state = "OVER"
    else state = "NONE" end
    return state
  end

  check("nothing claimed from nowhere near it", step(0.0, 1.0, 0) == "NONE")
  check("above but not over reads CLEAR", step(1.0 + ENTER + 0.05, 1.0, 0) == "CLEAR")
  check("over but not above reads OVER", step(0.5, 1.0, MINPT) == "OVER")
  state = "NONE"
  check("both together reads COMMITTED",
    step(1.0 + ENTER + 0.05, 1.0, MINPT) == "COMMITTED")

  -- Once clear, the implement may sag back below the ENTER margin without losing the state:
  -- that is the hysteresis, and without it a machine idling on its suspension would toggle.
  state = "NONE"
  step(1.0 + ENTER + 0.01, 1.0, 0)
  local held = step(1.0 + (ENTER + EXIT) / 2, 1.0, 0)
  check("clearance holds between the exit and enter margins", held == "CLEAR",
    string.format("at +%.3f m, between exit %.2f and enter %.2f", (ENTER + EXIT) / 2, EXIT, ENTER))
  check("and is lost below the exit margin",
    step(1.0 + EXIT - 0.01, 1.0, 0) == "NONE")

  -- The real chatter test: sit right on the enter threshold with a small oscillation, the
  -- way a loader idling with a raised bucket actually behaves.
  state = "NONE"
  step(1.0 + ENTER + 0.01, 1.0, 0)
  local flips, prev = 0, state
  for i = 1, 200 do
    local wobble = ((ENTER - EXIT) * 0.4) * math.sin(i * 0.7)
    local s = step(1.0 + ENTER - 0.005 + wobble, 1.0, 0)
    if s ~= prev then flips = flips + 1 end
    prev = s
  end
  check("no chatter across 200 ticks of threshold wobble", flips == 0,
    string.format("%d state changes", flips))

  -- Same asymmetry on the footprint: two points to claim it, all of them to lose it, so
  -- drifting along an edge cannot flutter either.
  state = "NONE"
  step(0.5, 1.0, MINPT)
  check("footprint held on a single remaining point", step(0.5, 1.0, 1) == "OVER")
  check("footprint lost only when no point remains", step(0.5, 1.0, 0) == "NONE")

  -- ...which is exactly why the point set has to be the FIVE sample cids and not the whole
  -- implement cloud. MINPT is a count, not a fraction, so feeding it ~120 nodes silently
  -- redefines "over it" as "about 2% of the bucket is over it" -- and then, because losing the
  -- footprint takes every point, the state cannot be dropped again either. Below: a bucket
  -- clipping the far corner of a target, dense cloud versus the sample set.
  local function nOverGrazing(cols, rows)
    -- Implement spans x in [-1, 1]; the target's footprint starts at x = 0.93, so only the
    -- outermost sliver of the bucket is over it.
    local n = 0
    for i = 0, cols - 1 do
      local x = -1.0 + 2.0 * i / math.max(1, cols - 1)
      if x >= 0.93 then n = n + rows end
    end
    return n
  end
  local cloudOver  = nOverGrazing(24, 5)   -- ~120 nodes, the full implCids list
  local sampleOver = nOverGrazing(3, 1)    -- edgeL/C/R at the same three x positions
  -- heelL/R sit rearward of the edge and are not over the target at all in this pose.
  check("a grazing corner claims the footprint from the full node cloud",
    cloudOver >= MINPT,
    string.format("%d of 120 cloud points inside -- COMMITTED would fire beside the target",
      cloudOver))
  check("...and does not from the five sample cids", sampleOver < MINPT,
    string.format("%d of 5 sample points inside", sampleOver))
end
print()

print("10. THE ARTICULATION BUG: sign and magnitude must come from the SAME frame")
do
  -- An articulated loader has two frames: the cab/rear frame that getDirectionVector
  -- describes, and the front frame the bucket is bolted to, which yaws relative to it by the
  -- articulation angle -- up to 40 degrees on the WL-40.
  --
  -- The bug: the angle MAGNITUDE was measured from the bucket while the left/right SIGN was
  -- measured from the cab. The failure needs the target near the CAB's forward axis, where
  -- the cab-side sign is on a knife edge, while the bucket is articulated well away from it.
  -- The sign then flips as the target drifts across the cab's centreline, but the magnitude
  -- is the articulation angle, not zero -- so the reading snaps from -30 to +30 without ever
  -- passing through zero. On a rigid vehicle the two frames coincide and the magnitude is
  -- zero exactly where the sign flips, which is why this was invisible everywhere else.
  local up = v3(0, 0, 1)
  local cab = v3(0, 1, 0)
  local function heading(deg)
    local a = math.rad(deg)
    return v3(-math.sin(a), math.cos(a), 0)   -- positive = LEFT, the project convention
  end
  local function bearing(implYaw, targetBearing, sameFrame)
    local impl, toT = heading(implYaw), heading(targetBearing)
    local mag = math.deg(math.acos(clamp(impl:dot(toT), -1, 1)))
    local left = up:cross(sameFrame and impl or cab)
    return mag * (left:dot(toT) < 0 and -1 or 1)
  end
  local function sweep(implYaw, from, to, steps, sameFrame)
    local flips, worst, prev = 0, 0, nil
    for i = 0, steps do
      local b = bearing(implYaw, from + (to - from) * i / steps, sameFrame)
      if prev then
        if (b < 0) ~= (prev < 0) then flips = flips + 1 end
        worst = math.max(worst, math.abs(b - prev))
      end
      prev = b
    end
    return flips, worst
  end

  -- Bucket articulated 30 degrees left; target drifting across the CAB's centreline.
  local oldFlips, oldJump = sweep(30, -5, 5, 100, false)
  local newFlips, newJump = sweep(30, -5, 5, 100, true)
  check("the old form flips sign across the cab centreline", oldFlips > 0,
    string.format("%d sign changes while the target barely moves", oldFlips))
  check("...and jumps by roughly twice the articulation angle", oldJump > 50.0,
    string.format("largest single-step jump %.1f degrees", oldJump))
  check("the fixed form does not flip there", newFlips == 0,
    string.format("%d sign changes", newFlips))
  check("...and stays continuous", newJump < 1.0,
    string.format("largest single-step jump %.3f degrees", newJump))

  -- The fixed form must still cross zero once, where it should: as the target passes the
  -- BUCKET's heading, not the cab's.
  local crossFlips, crossJump = sweep(30, 0, 60, 200, true)
  check("the fixed form crosses zero exactly once, at the bucket heading", crossFlips == 1,
    string.format("%d crossings sweeping the target past 30 degrees", crossFlips))
  check("...and crosses smoothly", crossJump < 1.0,
    string.format("largest step %.3f degrees", crossJump))

  -- With no articulation the two forms are identical, which is why every ordinary vehicle
  -- was unaffected and the bug survived so long.
  local same = true
  for i = -60, 60 do
    if math.abs(bearing(0, i, false) - bearing(0, i, true)) > 1e-9 then same = false end
  end
  check("a rigid vehicle is unaffected either way", same,
    "the two frames coincide, so the bug cannot occur")
end
print()

print("11. the two modules must measure lateral offsets along the SAME vector")
do
  -- implementProximity's boxFrame projects a point onto its own lateral vector and clamps the
  -- result against minR/maxR -- which it takes straight out of vehicleGeometry's cache, where
  -- they were measured by VEH_SCRIPT along ITS lateral vector. Nothing in either file forces
  -- the two to agree, and they did not: one built up:cross(fwd), the other fwd:cross(up), the
  -- negation. Every lateral test against a cached target was therefore mirrored -- the "tines
  -- are under it" call and the slam gate's plan-view footprint both answering for the far side
  -- of the vehicle.
  --
  -- What made it survive review is that the strided fallback measures with whatever frame it
  -- is handed, so it is self-consistent either way: the readout is correct for the first few
  -- ticks after a spawn and flips the moment the async resolve lands.
  local fwd, up = v3(0, 1, 0), v3(0, 0, 1)
  local left = v3(-1, 0, 0)          -- the driver's left, with fwd = +Y
  local geoR  = up:cross(fwd)        -- vehicleGeometry.lua VEH_SCRIPT and its boxFrame
  local proxR = up:cross(fwd)        -- implementProximity.lua boxFrame -- must match
  local buggyR = fwd:cross(up)       -- what it used to be

  check("the two lateral vectors agree",
    math.abs(geoR:dot(proxR) - 1.0) < 1e-9,
    string.format("dot = %.6f (1 = same vector, -1 = mirrored)", geoR:dot(proxR)))
  check("a point on the driver's left projects the same sign in both",
    (geoR:dot(left) < 0) == (proxR:dot(left) < 0),
    string.format("geo %.1f, prox %.1f", geoR:dot(left), proxR:dot(left)))
  -- The check must not be able to pass for free.
  check("the old form would have inverted it",
    (geoR:dot(left) < 0) ~= (buggyR:dot(left) < 0),
    string.format("geo %.1f vs fwd:cross(up) %.1f", geoR:dot(left), buggyR:dot(left)))

  -- And the shipped sources must actually use that form -- the arithmetic above only proves
  -- which one is right.
  -- Comment lines are skipped: both files discuss the wrong form by name, which is worth
  -- keeping and would otherwise trip the last check.
  local function usesLateral(path, form)
    local fh = assert(io.open(path, "r"), "cannot open " .. path)
    local found = false
    for line in fh:lines() do
      if not line:match("^%s*%-%-") and line:find(form, 1, true) then found = true end
    end
    fh:close()
    return found
  end
  check("vehicleGeometry.lua builds up:cross(fwd)", usesLateral(SRC, "up:cross(fwd)"))
  check("implementProximity.lua builds up:cross(fwd)",
    usesLateral(PROX_SRC, "up:cross(fwd)"))
  check("...and no longer builds fwd:cross(up)",
    not usesLateral(PROX_SRC, "fwd:cross(up)"),
    "the negated form must not reappear anywhere in the file")
  -- rampGeometry projects onto its own lateral vector and hands the result to
  -- implementProximity, which compares it against values measured along that file's. Three
  -- files now have to agree, and nothing but this grep enforces it.
  check("rampGeometry.lua builds up:cross(fwd)", usesLateral(RAMP_SRC, "up:cross(fwd)"))
  check("...and never builds fwd:cross(up)",
    not usesLateral(RAMP_SRC, "fwd:cross(up)"),
    "the negated form must not reappear anywhere in the file")
  -- vehicleScanner is the fourth site, and the one with the most to lose: its coupler align
  -- measures the coupler's offset along a lateral vector and then CANCELS it along another,
  -- built at the point of use. If those two disagree the correction doubles the error
  -- instead of removing it -- 0.868 m instead of 0.434 m on a T-series -- and the symptom
  -- (truck parked about a metre off line) is identical in both directions.
  --
  -- It MEASURES in the vehicle's body frame (up:cross(fwd), the convention above) and
  -- CANCELS in a world frame (vec3(0,0,1):cross(awayDir)). Those agree only because the
  -- teleport puts the truck level and facing awayDir. Measuring in a ground frame instead
  -- is roll-sensitive -- the fifth-wheel plate sits ~1.1 m up, so ten degrees of roll swings
  -- it 0.19 m and the align cancels an offset the settled truck does not have.
  check("vehicleScanner.lua measures the coupler offset with up:cross(fwd)",
    usesLateral(SCANNER_SRC, "up:cross(fwd)"))
  check("...and cancels it along the aligned heading",
    usesLateral(SCANNER_SRC, "vec3(0, 0, 1):cross(awayDir)"))
  check("...and never builds either cross the other way round",
    not usesLateral(SCANNER_SRC, "fwd:cross(up)")
      and not usesLateral(SCANNER_SRC, "awayDir:cross(vec3(0, 0, 1))"),
    "the negated form must not reappear anywhere in the file")
end
print()

-- ==================================================================================================
--  12. every listening extension must survive a failed bind and a reload
-- ==================================================================================================
-- This is a mod-wide grep rather than a test of any one file, and it lives here for the same
-- reason scenario 11 does: nothing in the sources forces fourteen files to agree, and the
-- failure is invisible from the seat.
--
-- The bug it pins was found the expensive way. F9+Shift+V spoke "Aligning to ramp" and then did
-- nothing at all -- not a failure message, because the failure path was on the far side of a
-- socket that had never opened. netstat showed 4470 absent entirely while every other command
-- port was bound. Three causes, stacked, and every listening extension in the mod had all three:
--
--   1. setsockname RETURNS nil plus a message; it does not throw. The pcall around it reported
--      success on a socket bound to nothing, and the error branch never ran, so NOTHING was
--      logged. The extension still SENT normally -- a UDP sender needs no bind -- which is what
--      makes this undiagnosable rather than merely broken: half dead, and talking.
--   2. setupSockets closes the sockets held by THIS module instance, and extensions.reload
--      builds a fresh instance whose locals are nil. So it closes nothing, the outgoing
--      instance keeps the port, and the incoming one can never have it.
--   3. A failed bind was permanent for the session, over a condition -- a leaked socket the GC
--      frees moments later -- that clears itself within seconds if anything ever looks again.
--
-- The three fixes are deliberately written in the SAME shape in all fifteen files so that one
-- grep can police them. Reshaping one is what this scenario exists to catch.
print("12. every listening extension checks its bind, closes on unload, and retries")
do
  local LISTENERS = {
    "beamtelAI", "bindingLearn", "cameraInfo", "clickspotAccessible", "consoleAccessible",
    "environmentAccessible", "implementProximity", "nodeGrabberAccessible", "obstacleDetector",
    "roadDetector", "terrainScanner", "uiToggle", "vehicleBindings", "vehicleInfo",
    "vehicleScanner", "vehicleSlots", "vehicleSpawnerAccessible",
  }

  local function bodyOf(name)
    local path = "bng_mod/lua/ge/extensions/" .. name .. ".lua"
    local fh = assert(io.open(path, "r"), "cannot open " .. path)
    local body = fh:read("*a")
    fh:close()
    return body
  end

  -- Anything that binds a command port is a listener and must be in the list above. A new
  -- extension added without one of the three fixes is exactly what this catches.
  local missing = {}
  for _, name in ipairs(LISTENERS) do
    local body = bodyOf(name)
    if not body:find("local bound, berr = udpCmd:setsockname", 1, true) then
      missing[#missing + 1] = name .. ": unchecked bind"
    end
    if not body:find("function M.onExtensionUnloaded", 1, true) then
      missing[#missing + 1] = name .. ": no unload hook"
    end
    if not body:find("local function retryCmdBind(dtReal)", 1, true) then
      missing[#missing + 1] = name .. ": no bind retry"
    end
    if not body:find("\n  retryCmdBind(dtReal)", 1, true) then
      missing[#missing + 1] = name .. ": retry never called"
    end
    -- The retry has to be driven from onUpdate; defining it and never calling it is a fix
    -- that reads correctly and does nothing.
    local upd = body:match("function M%.onUpdate%(dtReal, dtSim, dtRaw%)\r?\n(.-)\r?\n")
    if upd ~= "  retryCmdBind(dtReal)" then
      missing[#missing + 1] = name .. ": retry not first in onUpdate"
    end
  end
  check(string.format("all %d listening extensions carry all three fixes", #LISTENERS),
    #missing == 0, table.concat(missing, "; "))

  -- The list must not silently fall behind the directory. Every file that binds a UDP port
  -- is a listener, so discovering one that is not named above is itself a failure.
  -- The enumeration itself has to be verified, or a shell that cannot list the directory
  -- turns this into a check that passes because it found nothing. It is run under both cmd
  -- and a POSIX shell in this project, so try each and require that one of them worked.
  local seen, known = {}, {}
  for _, n in ipairs(LISTENERS) do known[n] = true end
  for _, cmd in ipairs({'dir /b "bng_mod\\lua\\ge\\extensions\\*.lua"',
                        'ls bng_mod/lua/ge/extensions/*.lua'}) do
    if #seen == 0 then
      local pipe = io.popen(cmd .. ' 2>&1')
      if pipe then
        for line in pipe:lines() do
          local base = line:match("([^/\\]+)%.lua%s*$")
          if base then seen[#seen + 1] = base end
        end
        pipe:close()
      end
    end
  end
  check("the extension directory can actually be enumerated",
    #seen >= #LISTENERS,
    string.format("listed %d files, expected at least %d -- an empty listing would make the "
      .. "next check pass for free", #seen, #LISTENERS))

  local unlisted = {}
  for _, base in ipairs(seen) do
    if not known[base] then
      local body = bodyOf(base)
      -- Comment lines are skipped, for the same reason scenario 13 skips them: a send-only
      -- extension has every reason to write "setsockname" in its header, explaining that it
      -- deliberately binds nothing and is therefore outside this contract. Grepping the raw
      -- body turns that explanation into a failure, i.e. fails the file for documenting the
      -- rule correctly. trailerAngle.lua is the first one to do it.
      local binds = false
      for line in body:gmatch("[^\n]+") do
        if not line:find("^%s*%-%-") and line:find("setsockname", 1, true) then
          binds = true
        end
      end
      if binds then unlisted[#unlisted + 1] = base end
    end
  end
  check("...and no extension binds a port without being on the list",
    #unlisted == 0, table.concat(unlisted, ", "))

  -- The retry must not be able to succeed on an unbound socket the way the original did --
  -- i.e. it has to check the return there too, not just in setupSockets.
  local unchecked = {}
  for _, name in ipairs(LISTENERS) do
    local body = bodyOf(name)
    local fn = body:match("local function retryCmdBind%(dtReal%)(.-)\nend\n")
    if not (fn and fn:find("if not bound then sk:close()", 1, true)) then
      unchecked[#unchecked + 1] = name
    end
  end
  check("the retry checks its own bind's return as well",
    #unchecked == 0, table.concat(unchecked, ", "))
end
print()

-- ==================================================================================================
--  13. no teleport in the mod may reset the vehicle
-- ==================================================================================================
-- spawn.safeTeleport's 8th argument is resetVehicle and it DEFAULTS TO TRUE. When true, spawn.lua
-- calls veh:setPosRot + veh:resetBrokenFlexMesh and then re-places the vehicle from its INITIAL
-- node positions -- which is a respawn in everything but name: every dent repaired, and the engine
-- stopped for anyone who has "reset stops the engine" set in the gameplay options. Nothing about
-- any of this mod's teleports is a request for that. F9+Shift+V (both the coupler align and the
-- ramp align) and the spawner's arrange are placements: put my damaged car over there, facing that
-- way. Passing the argument false keeps the safe-position search, the cluster move, the velocity
-- zeroing and setOriginalTransform -- all of it against the DEFORMED body -- and skips only the
-- respawn.
--
-- This is a grep because the failure is entirely invisible in the source: a call that omits the
-- argument reads as "use the sensible defaults", and reviewing it tells you nothing about what the
-- default is. It only shows up from the seat, as a car that arrives at the ramp repaired and with
-- its engine off. Written as the same explicit trailing form in every file so one pattern polices
-- it; vehicleSpawnerAccessible's teleportVehicleTo was the only site that ever had it right, and
-- is where the shape comes from.
print("13. every safeTeleport passes resetVehicle = false")
do
  local files = {}
  for _, cmd in ipairs({'dir /b "bng_mod\\lua\\ge\\extensions\\*.lua"',
                        'ls bng_mod/lua/ge/extensions/*.lua'}) do
    if #files == 0 then
      local pipe = io.popen(cmd .. ' 2>&1')
      if pipe then
        for line in pipe:lines() do
          local base = line:match("([^/\\]+)%.lua%s*$")
          if base then files[#files + 1] = base end
        end
        pipe:close()
      end
    end
  end
  check("the extension directory can actually be enumerated (scenario 13)",
    #files > 0, "an empty listing would make every check below pass for free")

  local calls, bad, resets = 0, {}, {}
  for _, base in ipairs(files) do
    local fh = assert(io.open("bng_mod/lua/ge/extensions/" .. base .. ".lua", "r"))
    local body = fh:read("*a")
    fh:close()
    -- Every safeTeleport call site, whether called directly or handed to pcall. Comment
    -- lines are skipped: the prose explaining WHY resetVehicle must be false necessarily
    -- writes "safeTeleport," itself, and would otherwise fail the check it documents.
    for line in body:gmatch("[^\n]+") do
      if not line:find("^%s*%-%-") then
        for args in line:gmatch("safeTeleport[%(,](.*)") do
          calls = calls + 1
          -- The argument list may be closed on this line (direct call) or be the tail of a
          -- pcall(spawn.safeTeleport, ...) -- both end with the two explicit falses.
          if not args:find("false,%s*false%s*%)") then
            bad[#bad + 1] = base .. ": " .. args:gsub("^%s+", "")
          end
        end
      end
    end
    -- The other three routes to the same respawn. core_camera.setPosRot is a dot call on the
    -- camera module and is deliberately not matched; these are all method calls on a vehicle.
    for _, banned in ipairs({":setPosRot%(", ":setPositionRotation%(", ":requestReset%("}) do
      if body:find(banned) then resets[#resets + 1] = base .. " " .. banned:gsub("%%", "") end
    end
  end

  check("safeTeleport call sites were actually found", calls >= 4,
    string.format("found %d -- the pattern has stopped matching", calls))
  check("...and every one of them passes resetVehicle = false",
    #bad == 0, table.concat(bad, " | "))
  check("...and nothing reaches the same respawn by another route",
    #resets == 0, table.concat(resets, ", "))
end
print()

if #failures > 0 then
  print(string.format("%d FAILURE(S): %s", #failures, table.concat(failures, ", ")))
  os.exit(1)
end
print("all checks passed")
