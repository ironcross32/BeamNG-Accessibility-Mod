-- Replay rampGeometry.lua's mouth resolve against the real large_cannon node coordinates.
--
--     lua diagnostic/ramp_resolve_sim.lua
--
-- The resolve is the whole risk in the ramp feature. Everything downstream -- the lateral
-- offset you steer by, the range you time the approach with, the clearance that says whether
-- you fit -- is derived from five cids picked once, and every way of picking them wrongly
-- produces confident, plausible numbers. There is no in-game symptom that says "the half-width
-- came off the wrong node"; there is only a clearance figure that feels a bit generous, and a
-- car with its mirrors torn off.
--
-- The node coordinates below are the real ones, read out of
-- content/vehicles/large_cannon.zip -> large_cannon_ramp.jbeam. Two of the checks are negative:
-- they assert that the OBVIOUS implementation gets a specific wrong answer, so that the check
-- cannot pass for free if someone later "simplifies" the rule back to it.
--
-- Tuning constants are parsed out of rampGeometry.lua rather than copied, so retuning there
-- cannot silently invalidate these checks. Only the *logic* is duplicated here.

local SRC = "bng_mod/lua/ge/extensions/rampGeometry.lua"

local function readConstFrom(path, name)
  local fh = assert(io.open(path, "r"), "cannot open " .. path)
  local body = fh:read("*a")
  fh:close()
  local val = body:match("\nlocal " .. name .. "%s*=%s*([%-%d%.]+)")
  assert(val, "could not find " .. name .. " in " .. path)
  return tonumber(val)
end
local function readConst(name) return readConstFrom(SRC, name) end

local MIN_RAMP_NODES     = readConst("MIN_RAMP_NODES")
local ROW_BAND           = readConst("ROW_BAND")
local WALL_MIN_H         = readConst("WALL_MIN_H")
local WALL_MAX_H         = readConst("WALL_MAX_H")
local MIN_WALL_LATERAL_M = readConst("MIN_WALL_LATERAL_M")
local AXIS_DOMINANCE     = readConst("AXIS_DOMINANCE")
local MIN_AXIS_DISP_M    = readConst("MIN_AXIS_DISP_M")
local MIN_MOUTH_WIDTH_M  = readConst("MIN_MOUTH_WIDTH_M")
local RESOLVE_TIMEOUT_S  = readConst("RESOLVE_TIMEOUT_S")
local MAX_TRIES          = readConst("MAX_TRIES")
local DECK_LENGTH_DOMINANCE = readConst("DECK_LENGTH_DOMINANCE")
local FLOOR_FIT_MIN_SPAN_M  = readConst("FLOOR_FIT_MIN_SPAN_M")
local ROW_BAND_MAX_M        = readConst("ROW_BAND_MAX_M")
local EDGE_TIE_M            = readConst("EDGE_TIE_M")
-- Not readConst: the source deliberately DERIVES this one from WALL_MIN_H rather than repeating
-- a number, because the equality is the argument -- a floor uncertain by more than the height
-- that defines a wall cannot support a wall pick. Asserted below rather than parsed, so the
-- derivation cannot be quietly replaced by a literal that drifts.
local FLOOR_FIT_MAX_RESIDUAL_M = WALL_MIN_H
do
  local body = assert(io.open(SRC, "r")):read("*a")
  assert(body:find("local FLOOR_FIT_MAX_RESIDUAL_M = WALL_MIN_H", 1, true),
    "FLOOR_FIT_MAX_RESIDUAL_M must stay derived from WALL_MIN_H")
end

-- The two word lists are read out of the source for the same reason the numbers are: the
-- allowlist is the part of this feature most likely to be edited (it is a data question about
-- shipped jbeam, settled only in game), and a copy here would let an edit there pass every
-- check while resolving nothing.
local function readListFrom(path, name)
  local fh = assert(io.open(path, "r"), "cannot open " .. path)
  local body = fh:read("*a")
  fh:close()
  local block = body:match(string.char(10) .. "local " .. name .. "%s*=%s*{(.-)}")
  assert(block, "could not find " .. name .. " in " .. path)
  local out = {}
  for w in block:gmatch('"([^"]+)"') do out[#out + 1] = w end
  assert(#out > 0, name .. " is empty")
  return out
end
local RAMP_PART_WORDS = readListFrom(SRC, "RAMP_PART_WORDS")
local RAMP_PART_DENY  = readListFrom(SRC, "RAMP_PART_DENY")

-- Mirrors the chunk's partIsRamp: deny as plain substrings, allow as whole words bounded at
-- both ends by a separator or by the end of the string.
local function isAlnumByte(b)
  if not b then return false end
  return (b >= 48 and b <= 57) or (b >= 65 and b <= 90) or (b >= 97 and b <= 122)
end
local function partIsRamp(s)
  if type(s) ~= "string" or s == "" then return false end
  local l = s:lower()
  for _, d in ipairs(RAMP_PART_DENY) do
    if l:find(d, 1, true) then return false end
  end
  for _, w in ipairs(RAMP_PART_WORDS) do
    local from = 1
    while true do
      local a, b = l:find(w, from, true)
      if not a then break end
      if (not isAlnumByte(l:byte(a - 1))) and (not isAlnumByte(l:byte(b + 1))) then
        return true
      end
      from = a + 1
    end
  end
  return false
end
local function nodeIsRampPart(nd)
  if partIsRamp(nd.part) then return true end
  local g = nd.group
  if type(g) == "string" then return partIsRamp(g) end
  if type(g) == "table" then
    for _, gv in ipairs(g) do
      if partIsRamp(gv) then return true end
    end
  end
  return false
end

-- =================================================================================================
--  The logic under test, mirroring rampGeometry.lua's VEH_SCRIPT
-- =================================================================================================

-- Nodes arrive as {cid, name, f, r, u} already projected onto the vehicle's own axes, which is
-- what fwd/rgt/up dotted against getNodePosition produces in the real chunk. `r` is positive to
-- the driver's LEFT, because rgt is up:cross(fwd).
local function resolve(nodes, opts)
  opts = opts or {}
  -- Two candidate sets filled in one pass, mirroring the chunk. The node-name tier wins
  -- outright when populated, which is what keeps large_cannon resolving through the code it
  -- always did no matter what the part allowlist grows to.
  local T = {
    {n = 0, c = {}, nm = {}, f = {}, r = {}, u = {}, sf = 0, sr = 0, su = 0},
    {n = 0, c = {}, nm = {}, f = {}, r = {}, u = {}, sf = 0, sr = 0, su = 0},
  }
  local function keep(t, nd)
    local i = t.n + 1
    t.n = i
    t.c[i], t.nm[i], t.f[i], t.r[i], t.u[i] = nd.cid, nd.name, nd.f, nd.r, nd.u
    t.sf, t.sr, t.su = t.sf + nd.f, t.sr + nd.r, t.su + nd.u
  end

  local mn, mf, mr, mu = 0, 0, 0, 0
  for _, nd in ipairs(nodes) do
    mn = mn + 1
    mf, mr, mu = mf + nd.f, mr + nd.r, mu + nd.u
    if nd.name and nd.name:find("^ramp_") then keep(T[1], nd) end
    if nodeIsRampPart(nd) then keep(T[2], nd) end
  end
  if mn < 1 then return nil, "no nodes" end

  local pick, tierName = nil, ""
  if T[1].n >= MIN_RAMP_NODES then
    pick, tierName = T[1], "node name"
  elseif T[2].n >= MIN_RAMP_NODES then
    pick, tierName = T[2], "part"
  end
  if not pick then
    return nil, "only " .. T[1].n .. " named and " .. T[2].n .. " part-matched ramp nodes"
  end

  local rn = pick.n
  local rc, nm, xf, xr, xu = pick.c, pick.nm, pick.f, pick.r, pick.u
  local sf, sr, su = pick.sf, pick.sr, pick.su

  mf, mr, mu = mf / mn, mr / mn, mu / mn
  sf, sr, su = sf / rn, sr / rn, su / rn
  local df, dr, du = sf - mf, sr - mr, su - mu
  local af, ar, au = math.abs(df), math.abs(dr), math.abs(du)

  -- `oth` is the other HORIZONTAL component. opts.oldOth restores the form that also weighed
  -- the vertical, which is the bug scenario 8 exists to pin. The vertical REJECTION is a
  -- separate mechanism and lives in the branch conditions, so opts.oldOth does not touch it.
  local along, lat, disp, oth, axisName, axisTier
  if af >= ar and af >= au then
    along, lat, disp, oth, axisName = xf, xr, df, opts.oldOth and math.max(ar, au) or ar, "f"
  elseif ar >= af and ar >= au then
    along, lat, disp, oth, axisName = xr, xf, dr, opts.oldOth and math.max(af, au) or af, "r"
  end
  local dm = 0
  if disp then dm = math.abs(disp) end
  if disp and dm >= MIN_AXIS_DISP_M and dm >= AXIS_DOMINANCE * oth and not opts.noDisplacement then
    axisTier = 1
  else
    -- The DECK tier. Reached only when the displacement rule found nothing to read, so it
    -- cannot change an answer the displacement rule already gave.
    local fLo, fHi, rLo, rHi = math.huge, -math.huge, math.huge, -math.huge
    for i = 1, rn do
      if xf[i] < fLo then fLo = xf[i] end
      if xf[i] > fHi then fHi = xf[i] end
      if xr[i] < rLo then rLo = xr[i] end
      if xr[i] > rHi then rHi = xr[i] end
    end
    local fSpan, rSpan = fHi - fLo, rHi - rLo
    if fSpan < MIN_AXIS_DISP_M or fSpan < DECK_LENGTH_DOMINANCE * rSpan then
      return nil, "along-axis not dominant"
    end
    along, lat, disp, oth, axisName, axisTier = xf, xr, -1, 0, "f", 2
  end

  local sgn = disp < 0 and -1 or 1
  local t, tmin, tmax = {}, math.huge, -math.huge
  for i = 1, rn do
    t[i] = sgn * along[i]
    if t[i] < tmin then tmin = t[i] end
    if t[i] > tmax then tmax = t[i] end
  end
  local span = tmax - tmin
  if span < MIN_AXIS_DISP_M then return nil, "ramp span too short" end
  -- opts.uncappedBand restores the pure fraction, which is what let a rollback deck's mouth
  -- "row" become a 4.2 m slab once the bed ran out.
  local rowBand = span * ROW_BAND
  if not opts.uncappedBand and rowBand > ROW_BAND_MAX_M then rowBand = ROW_BAND_MAX_M end

  local mouth, inner = {}, {}
  for i = 1, rn do
    if t[i] >= tmax - rowBand then mouth[#mouth + 1] = i end
    if t[i] <= tmin + rowBand then inner[#inner + 1] = i end
  end
  if #mouth < 3 or #inner < 2 then return nil, "end rows too sparse" end

  -- Row floor as a LINE along the ramp, mirroring floorFitOf in rampGeometry.lua. A row is a
  -- band, so tilt turns its along-extent into vertical extent and a flat lowest-z plane starts
  -- reporting the ramp's PITCH as wall height. opts.flatFloor restores that flat form, which is
  -- the bug scenario 8 pins.
  local function floorFitOf(row)
    local lo = math.huge
    for _, i in ipairs(row) do if xu[i] < lo then lo = xu[i] end end
    -- Four values, like the real one. Returning three leaves mouthCoherent nil, which is falsy,
    -- so this negative control would silently exercise the two-level path instead of the flat
    -- floor it is meant to pin.
    if opts.flatFloor then return lo, 0, lo, true end
    local n, st, su, stt, stu = 0, 0, 0, 0, 0
    local tLo, tHi = math.huge, -math.huge
    local ref = {}
    for _, i in ipairs(row) do
      if math.abs(xr[i]) <= MIN_WALL_LATERAL_M and (xu[i] - lo) <= WALL_MAX_H then
        n = n + 1
        ref[n] = i
        st, su = st + t[i], su + xu[i]
        stt, stu = stt + t[i] * t[i], stu + t[i] * xu[i]
        if t[i] < tLo then tLo = t[i] end
        if t[i] > tHi then tHi = t[i] end
      end
    end
    local den = n * stt - st * st
    if n >= 2 and (tHi - tLo) >= FLOOR_FIT_MIN_SPAN_M and math.abs(den) > 1e-9 then
      local k = (n * stu - st * su) / den
      local a = (su - k * st) / n
      -- Do the reference nodes actually lie on the line fitted to them? opts.noCoherence skips
      -- the question, which is the form that fitted a floor between a deck's two levels and
      -- then read the deck surface itself as wall.
      -- A SHARE of the reference set off the line, not the worst single one: two structural
      -- levels both run the length of the row, while a rib or a seam is one node.
      local off = 0
      for j = 1, n do
        local i = ref[j]
        if math.abs(xu[i] - (a + k * t[i])) > FLOOR_FIT_MAX_RESIDUAL_M then off = off + 1 end
      end
      return a, k, lo, opts.noCoherence or not (off >= 2 and off * 3 >= n)
    end
    return lo, 0, lo, true
  end
  local mouthFitA, mouthFitK, mouthFloorU, mouthCoherent = floorFitOf(mouth)
  local innerFitA, innerFitK, innerFloorU, innerCoherent = floorFitOf(inner)
  local function mouthH(i) return xu[i] - (mouthFitA + mouthFitK * t[i]) end
  local function innerH(i) return xu[i] - (innerFitA + innerFitK * t[i]) end

  -- The wall rule, and (opts.naive) the floor-band rule it replaces.
  -- Overhead structure excluded, or ramp_M_0 alone becomes the "top surface".
  local mouthLoU, mouthTopU = math.huge, -math.huge
  for _, i in ipairs(mouth) do if xu[i] < mouthLoU then mouthLoU = xu[i] end end
  for _, i in ipairs(mouth) do
    if (xu[i] - mouthLoU) <= WALL_MAX_H and xu[i] > mouthTopU then mouthTopU = xu[i] end
  end
  local function mouthIsFloor(i)
    if mouthCoherent then return mouthH(i) <= WALL_MIN_H end
    return (mouthTopU - xu[i]) <= WALL_MIN_H
  end

  local wallL, wallR, floorL, floorR = nil, nil, nil, nil
  local floorLatHi, floorLatLo = -math.huge, math.huge
  -- Lateral extreme first, station only on a tie. opts.orderTies restores the strict comparison,
  -- which resolves a tied side rail by table order and can put the mouth pair over a metre apart
  -- along the ramp.
  local TIE = opts.orderTies and 0 or EDGE_TIE_M
  local function takeMouthEdge(i)
    if (not floorL) or xr[i] > floorLatHi + TIE
       or (TIE > 0 and xr[i] > floorLatHi - TIE and t[i] > t[floorL]) then
      floorL = i
    end
    if xr[i] > floorLatHi then floorLatHi = xr[i] end
    if (not floorR) or xr[i] < floorLatLo - TIE
       or (TIE > 0 and xr[i] < floorLatLo + TIE and t[i] > t[floorR]) then
      floorR = i
    end
    if xr[i] < floorLatLo then floorLatLo = xr[i] end
  end
  local wallUsed = 0
  if mouthCoherent then
    for _, i in ipairs(mouth) do
      local h = mouthH(i)
      -- opts.noHeightCap and opts.noLateralFloor each drop one of the two guards that keep an
      -- overhead node out of the wall pick, so the checks can show what each one is worth.
      local raised = h > WALL_MIN_H and (opts.noHeightCap or h <= WALL_MAX_H)
      local farEnough = opts.noLateralFloor or (math.abs(xr[i]) > MIN_WALL_LATERAL_M)
      if raised and farEnough then
        if xr[i] > 0 then
          if (not wallL) or xr[i] < xr[wallL] then wallL = i end
        else
          if (not wallR) or xr[i] > xr[wallR] then wallR = i end
        end
      elseif h <= WALL_MIN_H then
        takeMouthEdge(i)
      end
    end
    -- The naive rule is exactly "lowest vertical band, then lateral extremes" -- the implement
    -- resolver's floor-band pick, applied here where it does not belong.
    if opts.naive then wallL, wallR = nil, nil end
    if wallL then wallUsed = wallUsed + 1 else wallL = floorL end
    if wallR then wallUsed = wallUsed + 1 else wallR = floorR end
  else
    -- The row is not one plane: the wall rule has no floor to measure from, so it does not run.
    for _, i in ipairs(mouth) do
      if mouthIsFloor(i) then takeMouthEdge(i) end
    end
    wallL, wallR = floorL, floorR
  end
  if not (wallL and wallR) then return nil, "could not find both mouth edges" end

  local halfW = (xr[wallL] - xr[wallR]) * 0.5
  local naiveHalfW = 0
  if floorL and floorR then naiveHalfW = (floorLatHi - floorLatLo) * 0.5 end
  if halfW < MIN_MOUTH_WIDTH_M then return nil, "mouth too narrow" end

  -- Centre marker: nearest the EDGE MIDPOINT, and floor nodes only.
  local midLat = (xr[wallL] + xr[wallR]) * 0.5
  -- opts.centreline reproduces the other tempting rule: nearest the vehicle centreline.
  -- opts.anyHeight drops the floor restriction, which is what lets ramp_M_0 become the centre.
  local aim = opts.centreline and 0 or midLat
  local mouthC, bestC = nil, math.huge
  for _, i in ipairs(mouth) do
    if opts.anyHeight or mouthIsFloor(i) then
      local d = math.abs(xr[i] - aim)
      if d < bestC then bestC, mouthC = d, i end
    end
  end
  if not mouthC then mouthC = wallL end

  local innerL, innerR = nil, nil
  local iHi, iLo = -math.huge, math.huge
  local innerLoU, innerTopU = math.huge, -math.huge
  for _, i in ipairs(inner) do if xu[i] < innerLoU then innerLoU = xu[i] end end
  for _, i in ipairs(inner) do
    if (xu[i] - innerLoU) <= WALL_MAX_H and xu[i] > innerTopU then innerTopU = xu[i] end
  end
  local function innerIsFloor(i)
    if innerCoherent then return innerH(i) <= WALL_MIN_H end
    return (innerTopU - xu[i]) <= WALL_MIN_H
  end
  for _, i in ipairs(inner) do
    if innerIsFloor(i) then
      if (not innerL) or xr[i] > iHi + TIE
         or (TIE > 0 and xr[i] > iHi - TIE and t[i] < t[innerL]) then
        innerL = i
      end
      if xr[i] > iHi then iHi = xr[i] end
      if (not innerR) or xr[i] < iLo - TIE
         or (TIE > 0 and xr[i] < iLo + TIE and t[i] < t[innerR]) then
        innerR = i
      end
      if xr[i] < iLo then iLo = xr[i] end
    end
  end
  if not (innerL and innerR) then return nil, "inner row has no floor band" end

  return {
    names = {nm[wallL], nm[mouthC], nm[wallR], nm[innerL], nm[innerR]},
    halfW = halfW, naiveHalfW = naiveHalfW, span = span, wallUsed = wallUsed,
    mouthCoherent = mouthCoherent, rowBand = rowBand,
    -- How far apart the two mouth markers sit ALONG the ramp. Zero on a real row; this is the
    -- quantity that mouthFrame turns into an inflated 3-D half-width and a skewed axis.
    edgeAlongGap = math.abs(t[wallL] - t[wallR]),
    axisName = axisName, sgn = sgn, dominance = dm / math.max(oth, 1e-9),
    tierName = tierName, axisTier = axisTier,
    mouthCount = #mouth, innerCount = #inner,
    mouthNames = (function()
      local o = {}
      for _, i in ipairs(mouth) do o[#o + 1] = nm[i] end
      return o
    end)(),
  }
end

-- =================================================================================================
--  The real large_cannon ramp, from large_cannon_ramp.jbeam
-- =================================================================================================

-- jbeam design space is x lateral, y forward, z up. Projected onto the vehicle axes that is
-- f = y, r = x, u = z. The cannon's ramp hangs off the -y end of a ~16 m machine.
local cid = 0
local function N(name, x, y, z)
  cid = cid + 1
  return {cid = cid, name = name, f = y, r = x, u = z}
end

local function rampNodes()
  local n = {}
  local function add(t) for _, e in ipairs(t) do n[#n + 1] = e end end

  -- Toe row, on the ground (*a_2), y -13.889 z -0.100. No wall nodes here.
  local toe = {0.546, 1.072, 1.556, 1.980, 2.155}
  add({N("ramp_M_1a_2", 0.0, -13.889, -0.100)})
  for i, x in ipairs(toe) do
    add({N("ramp_L_" .. i .. "a_2",  x, -13.889, -0.100),
         N("ramp_R_" .. i .. "a_2", -x, -13.889, -0.100)})
  end

  -- Mouth floor row (*a and *a_1), y -12.889 z 0.000. Note ramp_L_8a at 2.785: that is the
  -- OUTER FOOT of the side wall, and it is at exactly floor height. It is the whole reason the
  -- floor-band rule fails on this vehicle.
  local floor = {0.546, 1.072, 1.556, 1.980, 2.155}
  add({N("ramp_M_1a", 0.0, -12.889, 0.0), N("ramp_M_1a_1", 0.0, -12.889, 0.0)})
  for i, x in ipairs(floor) do
    add({N("ramp_L_" .. i .. "a",    x, -12.889, 0.0),
         N("ramp_L_" .. i .. "a_1",  x, -12.889, 0.0),
         N("ramp_R_" .. i .. "a",   -x, -12.889, 0.0),
         N("ramp_R_" .. i .. "a_1", -x, -12.889, 0.0)})
  end
  add({N("ramp_L_8a", 2.785, -12.889, 0.0), N("ramp_R_8a", -2.785, -12.889, 0.0)})

  -- The raised side wall (*6a, *7a), y -12.625 z 0.383. This is what the wall rule finds.
  add({N("ramp_L_6a", 2.148, -12.625, 0.383), N("ramp_L_7a", 2.670, -12.625, 0.383),
       N("ramp_R_6a", -2.148, -12.625, 0.383), N("ramp_R_7a", -2.670, -12.625, 0.383)})

  -- Inner row: flat floor (*b) plus the curved throat above it (*c), both at y -8.097.
  local innerFloor = {0.546, 1.072, 1.556, 1.980, 2.376, 3.449}
  add({N("ramp_M_1b", 0.0, -8.097, 0.0)})
  for i, x in ipairs(innerFloor) do
    add({N("ramp_L_" .. i .. "b",  x, -8.097, 0.0),
         N("ramp_R_" .. i .. "b", -x, -8.097, 0.0)})
  end
  local throat = {{0.477, 0.802}, {0.937, 0.941}, {1.360, 1.167},
                  {1.731, 1.472}, {2.035, 1.842}, {2.157, 2.070}, {2.684, 2.070}}
  add({N("ramp_M_1c", 0.0, -8.097, 0.755)})
  for i, p in ipairs(throat) do
    add({N("ramp_L_" .. i .. "c",  p[1], -8.097, p[2]),
         N("ramp_R_" .. i .. "c", -p[1], -8.097, p[2])})
  end

  return n
end

-- The lone node 2 m directly above the mouth centre. Kept separate so a scenario can leave it
-- out and show what it does when present.
local function rampM0() return N("ramp_M_0", 0.0, -12.300, 2.000) end

-- The rest of the machine: the cradle, pusher plate and springs, none of which match ^ramp_.
local function machineNodes()
  local n = {}
  for i = 1, 60 do
    n[#n + 1] = N("push_" .. i, (i % 5) - 2, 3.3 + (i % 3), 0.8 + (i % 6) * 0.8)
  end
  for i = 1, 40 do
    n[#n + 1] = N("base_" .. i, (i % 5) - 2, -6.0 + (i % 8), 0.5 + (i % 4) * 0.5)
  end
  return n
end

local function fullCannon(withM0)
  local n = machineNodes()
  for _, e in ipairs(rampNodes()) do n[#n + 1] = e end
  if withM0 then n[#n + 1] = rampM0() end
  return n
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

-- The two numbers this whole file exists to keep apart.
local TRUE_HALF_W  = 2.148
local NAIVE_HALF_W = 2.785

print(string.format(
  "tuning: rowBand %.2f (cap %.2f m)  wallMinH %.2f  minWallLat %.2f  dominance %.1f  "
  .. "minWidth %.2f  floorResid %.2f",
  ROW_BAND, ROW_BAND_MAX_M, WALL_MIN_H, MIN_WALL_LATERAL_M, AXIS_DOMINANCE, MIN_MOUTH_WIDTH_M,
  FLOOR_FIT_MAX_RESIDUAL_M))
print()

print("0. the extension and its cross-VM chunk both compile")
do
  local chunk, err = loadfile(SRC)
  check("rampGeometry.lua parses", chunk ~= nil, err or "")
  if chunk then
    local ok, mod = pcall(chunk)
    check("module body runs", ok and type(mod) == "table", not ok and tostring(mod) or "")
    if ok and type(mod) == "table" and mod.debugVehScript then
      -- string.format itself throws on a stray specifier, so getting a string back at all is
      -- half the check. The chunk is assembled here and only ever executed in a vehicle's own
      -- VM, so a syntax error in it fails over there, silently, and the only symptom in game is
      -- that ramps never resolve.
      local okFmt, src = pcall(mod.debugVehScript)
      check("the chunk survives string.format", okFmt,
        not okFmt and tostring(src) or "no stray percent escapes")
      if okFmt then
        local c2, e2 = load(src, "vehScript")
        check("the vehicle-VM chunk parses", c2 ~= nil, e2 or "")
        check("no unsubstituted format specifier survived", not src:find("%%[dqs]"),
          "a stray %d would be sent to the vehicle VM verbatim")
      end
    end
  end
end
print()

print("1. the along-ramp axis is derived from the centroid displacement, not the longest span")
do
  local nodes = fullCannon(true)
  local r = assert(resolve(nodes))
  check("resolve lands", r ~= nil)
  check("along axis is the machine's fore/aft", r.axisName == "f", "picked " .. r.axisName)
  check("dominance clears the threshold", r.dominance >= AXIS_DOMINANCE,
    string.format("%.1fx vs required %.1fx", r.dominance, AXIS_DOMINANCE))

  -- And now the negative half, which is stronger than "it might flip one day": on this very
  -- vehicle the longest axis of the ramp cloud is the LATERAL one. The inner row flares out to
  -- ramp_L_6b at x 3.449, making the cloud 6.898 m wide against 5.792 m long, so the tempting
  -- rule does not merely have a thin margin -- it gets the answer wrong, today, and with it
  -- the mouth becomes a side wall and lateral becomes range.
  local alongSpan, latSpan
  do
    local fLo, fHi, rLo, rHi = math.huge, -math.huge, math.huge, -math.huge
    for _, nd in ipairs(rampNodes()) do
      if nd.f < fLo then fLo = nd.f end
      if nd.f > fHi then fHi = nd.f end
      if nd.r < rLo then rLo = nd.r end
      if nd.r > rHi then rHi = nd.r end
    end
    alongSpan, latSpan = fHi - fLo, rHi - rLo
  end
  check("the longest-axis rule picks the WRONG axis on the real ramp", latSpan > alongSpan,
    string.format("lateral %.3f m exceeds along %.3f m", latSpan, alongSpan))
  check("...and the displacement rule gets it right anyway", r.axisName == "f",
    string.format("displacement %.1fx dominant along the machine", r.dominance))

  -- It is not a near miss that happens to fall the wrong way either: narrow the flare right in
  -- and the longest-axis rule is still only marginally right, while the displacement rule
  -- never depended on the ramp's proportions at all.
  local narrow = {}
  for _, nd in ipairs(nodes) do
    narrow[#narrow + 1] = {cid = nd.cid, name = nd.name, f = nd.f,
                           r = nd.name:find("^ramp_") and nd.r * 0.85 or nd.r, u = nd.u}
  end
  local rn2 = assert(resolve(narrow))
  check("a 15% narrower ramp still resolves the same way", rn2.axisName == "f"
    and approx(rn2.halfW, TRUE_HALF_W * 0.85, 1e-9),
    string.format("halfW %.3f m", rn2.halfW))
end
print()

print("2. which end is the mouth is derived, not assumed")
do
  local r = assert(resolve(fullCannon(true)))
  -- The mouth row must be the far end from the machine: the toe, the floor lip and the wall.
  local sawToe, sawWall = false, false
  for _, name in ipairs(r.mouthNames) do
    if name:find("a_2$") then sawToe = true end
    if name == "ramp_L_6a" then sawWall = true end
  end
  check("mouth row holds the ground toe", sawToe)
  check("mouth row holds the raised wall", sawWall,
    "if this fails the row band is too narrow and the wall rule cannot fire")
  check("mouth row excludes the inner rows", not table.concat(r.mouthNames, " "):find("b "),
    string.format("%d mouth nodes, %d inner", r.mouthCount, r.innerCount))

  -- Mirror the whole machine along its own axis: the answer must be identical.
  local flipped = {}
  for _, nd in ipairs(fullCannon(true)) do
    flipped[#flipped + 1] = {cid = nd.cid, name = nd.name, f = -nd.f, r = nd.r, u = nd.u}
  end
  local rf = assert(resolve(flipped))
  check("mirroring the machine mirrors nothing else",
    approx(rf.halfW, r.halfW, 1e-9) and rf.names[1] == r.names[1],
    string.format("sgn %d vs %d, halfW %.3f vs %.3f", rf.sgn, r.sgn, rf.halfW, r.halfW))
end
print()

print("3. THE WALL RULE -- the drivable half-width is not the floor's lateral extreme")
do
  local nodes = fullCannon(true)
  local r = assert(resolve(nodes))

  check("wall rule gives the true half-width", approx(r.halfW, TRUE_HALF_W, 1e-9),
    string.format("%.3f m", r.halfW))
  check("it landed on the wall nodes",
    r.names[1] == "ramp_L_6a" and r.names[3] == "ramp_R_6a",
    table.concat(r.names, " / "))
  check("both sides used the rule", r.wallUsed == 2, string.format("%d of 2", r.wallUsed))

  -- The negative half: the obvious implementation, and exactly how wrong it is.
  local naive = assert(resolve(nodes, {naive = true}))
  check("the floor-band rule gives the WRONG half-width",
    approx(naive.halfW, NAIVE_HALF_W, 1e-9), string.format("%.3f m", naive.halfW))
  check("it lands on the wall's outer foot",
    naive.names[1] == "ramp_L_8a" and naive.names[3] == "ramp_R_8a",
    table.concat(naive.names, " / "))
  check("the error is 0.637 m of phantom clearance per side",
    approx(naive.halfW - r.halfW, 0.637, 1e-9),
    string.format("%.3f m", naive.halfW - r.halfW))

  -- And the reason it matters: there is a whole band of approaches -- 1.198 m to 1.835 m off
  -- centre for a 1.9 m car -- where the two rules disagree about whether you fit at all.
  local CAR_HALF_W, OFFSET = 0.95, 1.50
  local marginTrue  = r.halfW - OFFSET - CAR_HALF_W
  local marginNaive = naive.halfW - OFFSET - CAR_HALF_W
  check("wall rule says a 1.9 m car 1.50 m off centre does NOT fit", marginTrue < 0,
    string.format("%.3f m", marginTrue))
  check("floor-band rule says it does", marginNaive > 0,
    string.format("%.3f m -- this is the mirrors coming off", marginNaive))
end
print()

print("4. an overhead node does not collapse the mouth, whatever the ramp is doing")
do
  -- ramp_M_0 is a lone node 2.1 m above the mouth floor on the centreline. It matches the name
  -- pattern and it is raised, so the smallest-lateral wall pick takes it unless something stops
  -- it. Two guards do, and they are NOT redundant -- each covers a case the other misses.
  local withM0  = assert(resolve(fullCannon(true)))
  local without = assert(resolve(fullCannon(false)))
  check("the answer is the same with and without it",
    approx(withM0.halfW, without.halfW, 1e-9) and approx(withM0.halfW, TRUE_HALF_W, 1e-9),
    string.format("%.3f m vs %.3f m", withM0.halfW, without.halfW))

  -- What the centreline guard is for, which the height cap does NOT cover: a low centre rib.
  -- Not present on large_cannon, but a spine down the middle of a trough is an ordinary way to
  -- model one, and at 0.30 m it sits squarely inside the wall band.
  local ribbed = {}
  for _, nd in ipairs(fullCannon(true)) do ribbed[#ribbed + 1] = nd end
  ribbed[#ribbed + 1] = N("ramp_M_rib", 0.0, -12.889, 0.300)
  local rib = assert(resolve(ribbed))
  check("a low centre rib is not mistaken for a wall", approx(rib.halfW, TRUE_HALF_W, 1e-9),
    string.format("%.3f m, edges %s / %s", rib.halfW, rib.names[1], rib.names[3]))
  local bug = assert(resolve(ribbed, {noLateralFloor = true}))
  check("...and dropping the centreline guard WOULD halve the mouth on it",
    bug.halfW < TRUE_HALF_W * 0.75 and bug.names[3] == "ramp_M_rib",
    string.format("%.3f m instead of %.3f, edges %s / %s",
      bug.halfW, TRUE_HALF_W, bug.names[1], bug.names[3]))

  -- The case the centreline guard cannot cover. Slide the ramp 0.8 m sideways -- a side-shifted
  -- or simply off-centre attachment -- and ramp_M_0 is no longer on the centreline, so that
  -- guard stops applying. Only the height cap still holds.
  local off = {}
  for _, nd in ipairs(fullCannon(true)) do
    off[#off + 1] = {cid = nd.cid, name = nd.name, f = nd.f,
                     r = nd.name:find("^ramp_") and (nd.r + 0.8) or nd.r, u = nd.u}
  end
  local ro = assert(resolve(off))
  check("an offset ramp still measures its true width", approx(ro.halfW, TRUE_HALF_W, 1e-9),
    string.format("%.3f m, edges %s / %s", ro.halfW, ro.names[1], ro.names[3]))
  local bugH = assert(resolve(off, {noHeightCap = true}))
  check("...and there the centreline guard is worthless -- only the height cap saves it",
    bugH.halfW < TRUE_HALF_W * 0.75 and bugH.names[1] == "ramp_M_0",
    string.format("%.3f m, edge %s", bugH.halfW, bugH.names[1]))

  -- It must not become the centre marker either.
  check("it is not picked as the mouth centre", withM0.names[2] ~= "ramp_M_0",
    "centre is " .. withM0.names[2])
  -- On the real geometry there are floor nodes on the centreline too, so ramp_M_0 loses the
  -- centre pick on a tie regardless. Strip them -- a ramp modelled without a centre rail -- and
  -- the floor restriction becomes the only thing keeping the mouth centre out of mid-air.
  local noCentreRail = {}
  for _, nd in ipairs(fullCannon(true)) do
    if not nd.name:find("^ramp_M_1a") then noCentreRail[#noCentreRail + 1] = nd end
  end
  local air = assert(resolve(noCentreRail, {anyHeight = true}))
  check("without the floor restriction the centre WOULD be 2 m up",
    air.names[2] == "ramp_M_0", "centre would be " .. air.names[2])
  local ok4 = assert(resolve(noCentreRail))
  check("with it, the centre stays on the floor", ok4.names[2] ~= "ramp_M_0",
    "centre is " .. ok4.names[2])
end
print()

print("5. a ramp with no raised wall falls back to the floor extreme, and records it")
do
  local flat = {}
  for _, nd in ipairs(fullCannon(false)) do
    -- Strip the raised mouth-row nodes, leaving a ramp that is genuinely just a flat plate.
    if not (nd.name == "ramp_L_6a" or nd.name == "ramp_L_7a"
            or nd.name == "ramp_R_6a" or nd.name == "ramp_R_7a") then
      flat[#flat + 1] = nd
    end
  end
  local r = assert(resolve(flat))
  check("it still resolves", r ~= nil)
  check("neither side used the wall rule", r.wallUsed == 0, string.format("%d of 2", r.wallUsed))
  check("half-width came off the floor extreme", approx(r.halfW, NAIVE_HALF_W, 1e-9),
    string.format("%.3f m", r.halfW))
end
print()

print("6. the centre marker is picked against the edge midpoint, not the vehicle centreline")
do
  -- Shift the whole ramp 0.8 m to the driver's left, as a side-shifted or offset attachment
  -- would be. On a centred ramp both rules agree, which is exactly why the centreline rule
  -- looks correct; this is the case that separates them.
  local SHIFT = 0.8
  local off = {}
  for _, nd in ipairs(fullCannon(true)) do
    off[#off + 1] = {cid = nd.cid, name = nd.name, f = nd.f,
                     r = nd.name:find("^ramp_") and (nd.r + SHIFT) or nd.r, u = nd.u}
  end
  local r = assert(resolve(off))
  local rc = assert(resolve(off, {centreline = true}))
  check("the two rules now disagree", r.names[2] ~= rc.names[2],
    string.format("midpoint picks %s, centreline picks %s", r.names[2], rc.names[2]))
  check("the midpoint rule picks the node nearest the true centre",
    r.names[2] == "ramp_M_1a" or r.names[2] == "ramp_M_1a_1" or r.names[2] == "ramp_M_1a_2",
    "picked " .. r.names[2])
end
print()

print("7. an ordinary car resolves terminally, on its first and only round trip")
do
  local car = {}
  for i = 1, 200 do
    car[#car + 1] = N("body_" .. i, (i % 7) * 0.3 - 0.9, (i % 11) * 0.4 - 2.2, (i % 5) * 0.3)
  end
  local r, why = resolve(car)
  check("no ramp is found", r == nil, why or "")
  check("and the reason names what it saw", (why or ""):find("ramp nodes") ~= nil, why or "")

  -- The GE half is what makes this terminal: an empty reply sets `failed`, and M.request is a
  -- no-op thereafter. Without it, every car in the scene re-issues a cross-VM chunk every
  -- RESOLVE_TIMEOUT_S for the whole session -- the exact bug vehicleGeometry's `failed` set
  -- was added to fix, which is why this file inherits it.
  local body = assert(io.open(SRC, "r")):read("*a")
  check("an empty cid list is terminal in onRampGeometry",
    body:find("if #cids == 0 then") ~= nil and body:find("failed%[vehID%] = true") ~= nil)
  check("M.request short-circuits on the failed set",
    body:find("if not vehID or cache%[vehID%] or pending%[vehID%] or failed%[vehID%] then") ~= nil)
  check("the chunk always replies, even with nothing to say",
    body:find('reply%("", "", "only " %.%. T%[1%]%.n') ~= nil,
    "silence would be indistinguishable from a VM that has not answered yet")
  -- ...but "I have no node data yet" is a statement about a VM still spawning, not about the
  -- vehicle. Terminal on that reply, a cannon is permanently invisible or not depending purely
  -- on when the first resolve fired -- a bug that presents as "it worked, then I restarted and
  -- got nothing", with no way to tell it from a dozen other causes.
  check("a not-ready reply is retried rather than being terminal",
    body:find('if why == "no node data" or why == "no nodes" then') ~= nil
      and body:find("notReady%[vehID%] = n") ~= nil)
  check("...but still under the same retry budget", body:find("if n < MAX_TRIES then") ~= nil,
    "otherwise a VM that always answers this way retries forever")
  check("...and the counter is cleared with the rest of the per-vehicle state",
    body:find("notReady%[vehID%] = nil") ~= nil)
end
print()

print("8. a TILTED assembly is still a ramp -- the dominance guard is horizontal-only")
do
  -- large_cannon tilts its whole assembly, ramp included, on the ramL/ramR cylinders. The old
  -- guard weighed the along-axis displacement against math.max(lateral, VERTICAL), so raising
  -- the barrel swung the ramp down, grew the vertical term, and collapsed the ratio -- and the
  -- machine stopped having a ramp at all. It happens to be level at spawn, which is why nothing
  -- ever saw this; a reset, a part swap, a reload or M.retry at elevation killed ramp mode for
  -- the session and answered NO RAMP ON IT.
  --
  -- Part A pins the rule against the numbers the game actually produced, so this check is
  -- anchored to a measured failure rather than to a pivot guessed at here.

  -- Measured in game on a spawned large_cannon at full inclination, from the resolve's own
  -- rejection log. The ramp is symmetric about the centreline, so the lateral term is residue.
  local FIELD_AF, FIELD_AR, FIELD_AU = 8.4615408894561, 0.0, 2.8524626907542

  -- The gate itself, mirroring the chunk. Returns the tier, or nil when it declines.
  local function axisGate(af, ar, au, oldOth)
    local disp, oth
    if af >= ar and af >= au then
      disp, oth = af, oldOth and math.max(ar, au) or ar
    elseif ar >= af and ar >= au then
      disp, oth = ar, oldOth and math.max(af, au) or af
    end
    if disp and disp >= MIN_AXIS_DISP_M and disp >= AXIS_DOMINANCE * oth then return 1 end
    return nil
  end

  local ratioOld = FIELD_AF / math.max(FIELD_AU, 1e-9)
  check("the measured tilted pose misses the old threshold, and only just",
    ratioOld < AXIS_DOMINANCE and ratioOld > AXIS_DOMINANCE * 0.98,
    string.format("%.3f against %.1f -- about 3 cm short", ratioOld, AXIS_DOMINANCE))
  check("the OLD rule rejects a cannon that plainly has a ramp",
    axisGate(FIELD_AF, FIELD_AR, FIELD_AU, true) == nil,
    "this is the bug: NO RAMP ON IT, on a machine whose ramp resolved a moment earlier")
  check("the new rule accepts it", axisGate(FIELD_AF, FIELD_AR, FIELD_AU) == 1,
    "the axis is horizontal, so only the other horizontal component says whether it is clear")

  -- The vertical REJECTION is a separate mechanism and must survive the change: it lives in the
  -- branch conditions, not in oth. large_spinner's wall is the shape it exists for, and
  -- scenario 15 asserts the whole-resolve version of this.
  check("a vertically displaced cloud is still declined, under BOTH rules",
    axisGate(0.093, 0.0, 0.917) == nil and axisGate(0.093, 0.0, 0.917, true) == nil,
    "guessing at a node set displaced mostly in Z would be worse than declining")
  -- ...and the guard must still guard in the plane it actually operates in.
  check("a diagonally displaced cloud is still declined",
    axisGate(6.0, 4.0, 0.2) == nil,
    "6 m along against 4 m across does not say which way the ramp faces")

  -- Part B: the same thing end to end, on rotated geometry. The transform is new -- every other
  -- scenario in this file edits the already-projected f/r/u by an axis-aligned scalar, and this
  -- is the only one that rotates. Ramp and cannon body (the push_ nodes) turn together about a
  -- pivot; the base stays welded, which is exactly how the machine is built.
  local function tilted(nodes, deg, pf, pu)
    local c, s = math.cos(math.rad(deg)), math.sin(math.rad(deg))
    local out = {}
    for _, nd in ipairs(nodes) do
      local moves = nd.name:find("^ramp_") ~= nil or nd.name:find("^push_") ~= nil
      if moves then
        local df, du = nd.f - pf, nd.u - pu
        out[#out + 1] = {cid = nd.cid, name = nd.name, part = nd.part,
                         f = pf + df * c - du * s, r = nd.r, u = pu + df * s + du * c}
      else
        out[#out + 1] = nd
      end
    end
    return out
  end

  local level = fullCannon(true)
  local flat  = assert(resolve(level))
  local up    = assert(resolve(tilted(level, 12.0, 0.0, 0.0)))

  check("a tilted cannon still resolves through the displacement tier", up.axisTier == 1,
    string.format("tier %d", up.axisTier))
  -- The strongest assertion available here: an f-u rotation leaves the lateral coordinate
  -- untouched, so the drivable half-width is rotation-invariant EXACTLY. Any drift at all means
  -- something downstream of the gate is reading the pose.
  check("the half-width is unchanged to the last decimal",
    approx(up.halfW, TRUE_HALF_W, 1e-9), string.format("%.6f m", up.halfW))
  check("...and it is still the wall rule answering, both sides", up.wallUsed == 2,
    string.format("%d of 2", up.wallUsed))
  check("the mouth is still the same end of the machine", up.sgn == flat.sgn,
    "a tilt must not flip which way you drive in")
  check("...and the same five nodes", table.concat(up.names, " ") == table.concat(flat.names, " "),
    table.concat(up.names, " / "))

  -- Part C: THE SECOND BUG, which the first fix would otherwise have unmasked.
  --
  -- A row is a band, not a plane: the mouth row spans 1.26 m of a 5.79 m ramp. Measuring wall
  -- height above the row's LOWEST NODE therefore reads the ramp's own pitch as wall height once
  -- the assembly tilts. large_cannon's floor lip sits 1.0 m up-ramp of the toe and 0.1 m above
  -- it, so it alone clears WALL_MIN_H at about 2.9 degrees -- and the wall rule, which takes the
  -- SMALLEST qualifying lateral, then answers 0.546 m instead of 2.148 m. A mouth reported at a
  -- quarter of its width, with full confidence.
  --
  -- This was reachable in the shipped mod today at any tilt over ~3 degrees; it stayed hidden
  -- only because the cannon spawns level and the chosen cids are then cached for the session.
  -- Fixing the dominance gate without this would have turned an honest "no ramp" into a
  -- confident wrong number, which is strictly worse.
  local flatFloorLevel = assert(resolve(level, {flatFloor = true}))
  check("at level the two floor forms agree exactly",
    approx(flatFloorLevel.halfW, TRUE_HALF_W, 1e-9),
    "the fit must not disturb the answer every other vehicle already gets")

  local broke = nil
  for tenths = 0, 300 do
    local r = resolve(tilted(level, tenths / 10, 0.0, 0.0), {flatFloor = true})
    if not (r and approx(r.halfW, TRUE_HALF_W, 1e-6)) then broke = tenths / 10 break end
  end
  check("the flat-floor form collapses the mouth at a shallow angle", broke ~= nil and broke < 5.0,
    broke and string.format("%.1f deg -- and the cannon's ramp tilts far past that", broke)
      or "it never broke, so this check has stopped testing anything")
  local bad = resolve(tilted(level, 12.0, 0.0, 0.0), {flatFloor = true})
  check("...to about a quarter of the true width", bad ~= nil and bad.halfW < 1.0,
    bad and string.format("%.3f m against %.3f m, on the floor lip (%s)",
      bad.halfW, TRUE_HALF_W, bad.names[1]) or "no resolve")

  -- And the fitted form holds across everything the machine can do, which is the property that
  -- makes a resolve a fact about the VEHICLE rather than about the moment it happened to run.
  local worst, worstAt = 0, 0
  for tenths = 0, 300 do
    local r = resolve(tilted(level, tenths / 10, 0.0, 0.0))
    if not r then worst, worstAt = math.huge, tenths / 10 break end
    local e = math.abs(r.halfW - TRUE_HALF_W)
    if e > worst then worst, worstAt = e, tenths / 10 end
  end
  check("the fitted form holds the half-width from 0 to 30 degrees", worst < 1e-9,
    string.format("worst error %.2e m at %.1f deg", worst, worstAt))
end
print()

print("9. the readout's signs, ranges and margin -- mirroring rampMeasure in implementProximity")
do
  -- Minimal vec3, matching only the operations rampMeasure uses.
  local vec = {}
  vec.__index = vec
  local function v3(x, y, z) return setmetatable({x = x, y = y, z = z}, vec) end
  vec.__add = function(a, b) return v3(a.x + b.x, a.y + b.y, a.z + b.z) end
  vec.__sub = function(a, b) return v3(a.x - b.x, a.y - b.y, a.z - b.z) end
  function vec:dot(o) return self.x * o.x + self.y * o.y + self.z * o.z end
  function vec:length() return math.sqrt(self:dot(self)) end
  function vec:normalized() local l = self:length(); return v3(self.x / l, self.y / l, self.z / l) end
  function vec:cross(o)
    return v3(self.y * o.z - self.z * o.y, self.z * o.x - self.x * o.z,
              self.x * o.y - self.y * o.x)
  end

  local UP = v3(0, 0, 1)
  local function frameFor(fwd) return {fwd = fwd, left = UP:cross(fwd):normalized()} end

  local function measure(originPos, fwd, mouthCentre, axis, halfW, playerHalfW)
    local origin = frameFor(fwd)
    origin.pos = originPos
    local mouth = {centre = mouthCentre, axis = axis, left = UP:cross(axis):normalized(),
                   halfW = halfW}
    local d = mouth.centre - origin.pos
    local range = mouth.axis:dot(d)  -- SIGNED; see rampMeasure for why the floor was removed
    local lateral = mouth.left:dot(d)
    local c = origin.fwd:dot(mouth.axis)
    if c > 1 then c = 1 elseif c < -1 then c = -1 end
    local yaw = math.deg(math.acos(c))
    if origin.left:dot(mouth.axis) < 0 then yaw = -yaw end
    local margin = -1.0
    if playerHalfW and playerHalfW > 0 then
      margin = mouth.halfW - math.abs(lateral) - playerHalfW
    end
    return range, lateral, yaw, margin
  end

  -- Ramp axis points into the mouth along +y. With up = +z, up x axis = (-1,0,0), so the
  -- driver's left is -x, matching vehicle_geometry_sim scenario 8's convention exactly.
  local AXIS, AT = v3(0, 1, 0), v3(0, 1, 0)
  local HALF_W = TRUE_HALF_W

  -- LATERAL SIGN. A sign error here reads as perfectly plausible speech -- "0.4 metres right"
  -- when it is 0.4 metres left -- and this project has already lost time to exactly that.
  local _, latLeft = measure(v3(0, 0, 0), AT, v3(-1.0, 10, 0), AXIS, HALF_W)
  local _, latRight = measure(v3(0, 0, 0), AT, v3(1.0, 10, 0), AXIS, HALF_W)
  check("a mouth on the driver's LEFT reads positive", latLeft > 0,
    string.format("%+.2f m", latLeft))
  check("a mouth on the driver's RIGHT reads negative", latRight < 0,
    string.format("%+.2f m", latRight))
  -- And prove it is discriminating: the negated lateral vector would swap them.
  local badLeft = v3(0, 1, 0):cross(UP)
  check("the negated lateral vector WOULD invert it",
    (badLeft:dot(v3(-1.0, 10, 0) - v3(0, 0, 0)) < 0) ~= (latLeft < 0),
    "fwd:cross(up) instead of up:cross(fwd)")

  -- YAW SIGN. Nose pointing to the right of the ramp axis must say "turn left".
  local noseRight = v3(math.sin(math.rad(10)), math.cos(math.rad(10)), 0)
  local noseLeft  = v3(-math.sin(math.rad(10)), math.cos(math.rad(10)), 0)
  local _, _, yawR = measure(v3(0, 0, 0), noseRight, v3(0, 10, 0), AXIS, HALF_W)
  local _, _, yawL = measure(v3(0, 0, 0), noseLeft,  v3(0, 10, 0), AXIS, HALF_W)
  check("nose right of the ramp axis reads positive, i.e. turn LEFT", yawR > 0,
    string.format("%+.1f deg", yawR))
  check("nose left reads negative", yawL < 0, string.format("%+.1f deg", yawL))
  check("the magnitude is the real angle", approx(math.abs(yawR), 10.0, 1e-6),
    string.format("%.3f deg", math.abs(yawR)))

  -- Yaw is NOT folded to +/-90: a ramp has a front and a back, and driving away from the mouth
  -- must not read as nearly square the way it would for a pallet face.
  local away = v3(0, -1, 0)
  local _, _, yawAway = measure(v3(0, 0, 0), away, v3(0, 10, 0), AXIS, HALF_W)
  check("facing away from the mouth reads ~180, not ~0", math.abs(yawAway) > 170,
    string.format("%.1f deg", math.abs(yawAway)))

  -- RANGE IS ALONG-AXIS. Sliding sideways at a fixed along-distance must not change it. The
  -- direct analogue of vehicle_geometry_sim scenario 2, and worth keeping for the same reason:
  -- a straight-line range would inflate with lateral offset, so drifting sideways would sound
  -- like backing away, and it would bottom out at the offset instead of at zero.
  local r0 = select(1, measure(v3(0, 0, 0), AT, v3(0, 8, 0), AXIS, HALF_W))
  local worst = 0
  for off = -3.0, 3.0, 0.5 do
    local r = select(1, measure(v3(0, 0, 0), AT, v3(off, 8, 0), AXIS, HALF_W))
    worst = math.max(worst, math.abs(r - r0))
  end
  check("+/-3 m of lateral drift does not move the range", worst < 1e-9,
    string.format("largest change %.2e m", worst))
  -- THE RANGE IS SIGNED, and this is the check that used to assert the opposite.
  --
  -- It was floored at zero, which is right for the last half metre of an approach and wrong
  -- for everything else: the negative half-space is the whole hemisphere behind the mouth
  -- plane -- alongside the machine, behind it, anywhere on the far side of a sixteen-metre
  -- cannon. A driver hunting for the entrance is in it almost the entire time, and the floor
  -- pinned the range channel at zero throughout: the pulse ran at its contact rate and the
  -- readout said "mouth 0.0 feet" from thirty feet away on the wrong side.
  local rPast = select(1, measure(v3(0, 0, 0), AT, v3(0, -2, 0), AXIS, HALF_W))
  check("behind the mouth plane the range goes NEGATIVE, and by the true amount",
    approx(rPast, -2.0, 1e-9), string.format("%+.2f m", rPast))
  check("the old floor WOULD have reported arrival from there",
    math.max(0, rPast) == 0, "0.00 m, i.e. 'mouth 0.0 feet' while two metres past it")

  -- THE FEED CEILING IS THE IN-PLANE DISTANCE, not the along-range. With the range signed,
  -- testing it directly would leave the entire negative half-space permanently under the
  -- ceiling; and on the approach side it would call a mouth fifteen metres away "ten", because
  -- ten of those metres are along the axis and eight are across it.
  local RAMP_REPORT_M = readConstFrom("bng_mod/lua/ge/extensions/implementProximity.lua",
    "RAMP_REPORT_M")
  local function reach(r, l) return math.sqrt(r * r + l * l) end
  local far = RAMP_REPORT_M + 10.0
  local rBehind, lBehind = measure(v3(0, 0, 0), AT, v3(0, -far, 0), AXIS, HALF_W)
  check("a mouth far BEHIND you is outside the feed ceiling",
    reach(rBehind, lBehind) > RAMP_REPORT_M,
    string.format("in-plane %.1f m vs ceiling %.1f m", reach(rBehind, lBehind), RAMP_REPORT_M))
  check("...where the along-range alone would have kept it inside",
    math.max(0, rBehind) <= RAMP_REPORT_M, "floored range reads 0, i.e. right on top of it")
  -- The ceiling has to clear the machine itself by a wide margin, or standing beside a
  -- sixteen-metre cannon at the barrel end puts the mouth outside the feed -- which is exactly
  -- how the instrument came to report "nothing in range" about a machine three metres away.
  check("the feed ceiling clears a machine-length with room to spare",
    RAMP_REPORT_M > 16.0 * 2, string.format("%.0f m against a ~16 m machine", RAMP_REPORT_M))
  -- ...and past it the mod names the distance instead of going quiet. A bare DOCKCLEAR wipes
  -- the reason as well as the readout on the Python side, so the tap can only answer "nothing
  -- in range" -- false, and identical to what a dead instrument says.
  local prox = assert(io.open("bng_mod/lua/ge/extensions/implementProximity.lua", "r")):read("*a")
  local rampBody = prox:match("local function sendRampDockLine.-\nend")
  -- The CALL, not the word: the block explains at length why it does not send one, so a bare
  -- substring test passes or fails on the comment rather than on the code.
  check("ramp mode never sends a bare DOCKCLEAR",
    rampBody ~= nil and not rampBody:find('send%("DOCKCLEAR"%)'),
    "out of range is a named DOCKFAIL carrying the distance")
  check("...and that failure names how far away the mouth is",
    rampBody ~= nil and rampBody:find("ramp mouth") ~= nil)
  local rWide, lWide = measure(v3(0, 0, 0), AT, v3(-8.0, 10.0, 0), AXIS, HALF_W)
  check("in front but well off to one side is measured as the real distance",
    approx(reach(rWide, lWide), math.sqrt(164), 1e-9),
    string.format("%.2f m, not the %.2f m the projection alone gives", reach(rWide, lWide), rWide))

  -- MARGIN USES THE WORSE SIDE of an off-centre vehicle origin.
  local ext = {minR = -0.8, maxR = 1.1}
  local half = math.max(math.abs(ext.minR), math.abs(ext.maxR))
  check("half-width takes the wider side of an asymmetric vehicle", approx(half, 1.1, 1e-9),
    string.format("%.2f m from minR %.1f / maxR %.1f", half, ext.minR, ext.maxR))
  local _, _, _, marg = measure(v3(0, 0, 0), AT, v3(-0.5, 6, 0), AXIS, HALF_W, half)
  check("margin subtracts offset and half-width from the mouth",
    approx(marg, HALF_W - 0.5 - 1.1, 1e-9), string.format("%.3f m", marg))
  local _, _, _, unk = measure(v3(0, 0, 0), AT, v3(-0.5, 6, 0), AXIS, HALF_W, nil)
  check("an unmeasured vehicle reports -1, never 0", unk == -1.0,
    "0 would read as exactly touching both walls")
end
print()

print("10. the mod's feed ceiling is above what audio will ask for")
do
  -- audio.py fades the instrument out at its own configured range; if the mod stops sending
  -- first, the instrument cuts out mid-fade with no explanation. The invariant existed only as
  -- a comment until now.
  local PROX = "bng_mod/lua/ge/extensions/implementProximity.lua"
  local RAMP_REPORT_M = readConstFrom(PROX, "RAMP_REPORT_M")
  local REPORT_M      = readConstFrom(PROX, "REPORT_M")
  local fh = assert(io.open("audio.py", "r"))
  local py = fh:read("*a")
  fh:close()
  local function pyConst(name)
    local v = py:match("\n" .. name .. "%s*=%s*([%-%d%.]+)")
    assert(v, "could not find " .. name .. " in audio.py")
    return tonumber(v)
  end
  check("audio's implement range fits inside the mod's",
    pyConst("DOCK_MAX_RANGE_M") <= REPORT_M,
    string.format("%.1f <= %.1f", pyConst("DOCK_MAX_RANGE_M"), REPORT_M))
  check("audio's ramp range fits inside the mod's",
    pyConst("DOCK_RAMP_MAX_RANGE_M") <= RAMP_REPORT_M,
    string.format("%.1f <= %.1f", pyConst("DOCK_RAMP_MAX_RANGE_M"), RAMP_REPORT_M))
  check("the ramp beat channel fades in inside the ramp range",
    pyConst("DOCK_RAMP_BEAT_RANGE_M") < pyConst("DOCK_RAMP_MAX_RANGE_M"))
end
print()

print("11. the lateral vector agrees with the rest of the mod")
do
  -- Positive lateral is the driver's LEFT everywhere in this project, and a sign error here
  -- reads as perfectly plausible speech. Both this file and the two it must agree with build
  -- the vector as up:cross(fwd); the negated form must not appear.
  local function usesLateral(path, form)
    local fh = assert(io.open(path, "r"), "cannot open " .. path)
    local found = false
    for line in fh:lines() do
      if not line:match("^%s*%-%-") and line:find(form, 1, true) then found = true end
    end
    fh:close()
    return found
  end
  check("rampGeometry.lua builds up:cross(fwd)", usesLateral(SRC, "up:cross(fwd)"))
  check("...and never builds fwd:cross(up)", not usesLateral(SRC, "fwd:cross(up)"),
    "the negated form mirrors every lateral reading")
  check("the mouth frame's left is vec3(0,0,1):cross(axis)",
    usesLateral(SRC, "vec3(0, 0, 1):cross(axis)"),
    "same construction as implementProximity's dock readout")
end
print("12. the resolve state machine: a slow VM, a sleeping VM, and a genuinely stale reply")
do
  -- Drives the real module rather than a copy of it, with the two engine globals it touches
  -- stubbed. Everything here is about the difference between "this machine has no ramp" and
  -- "this machine has not answered yet", which is the distinction the whole file turns on and
  -- the one that fails silently in game.
  local rg = assert(loadfile(SRC))()
  local queued, active = {}, true
  local fakeVeh = {
    queueLuaCommand = function(_, str) queued[#queued + 1] = str end,
    getActive = function() return active end,
  }
  local savedLog, savedTree = _G.log, _G.scenetree
  _G.log = function() end
  _G.scenetree = {findObjectById = function() return fakeVeh end}
  local function lastEpoch()
    return tonumber((queued[#queued] or ""):match("local EPOCH = (%d+)"))
  end
  -- NINE meta fields: halfW, span, floorU, nNodes, wallUsed, naiveHalfW, axisTier, isCannon,
  -- floorTrusted. The seventh tells a caller whether the mouth end was DERIVED from the ramp's
  -- displacement, ASSUMED to be the rear of the machine, or DECLARED outright; the eighth says
  -- whether the machine carries large_cannon's own controller, which is a different question
  -- from whether it has a ramp at all (scenario 17); the ninth says whether the five chosen
  -- nodes are collision surface, i.e. whether the pitch and lip height derived from them mean
  -- anything. onRampGeometry rejects a reply of the wrong arity as malformed -- deliberately,
  -- since a VM answering with the wrong shape will keep doing so, and that guard is what makes
  -- a half-updated install fail loudly instead of reading field N as field N+1.
  local GOOD_CIDS, GOOD_META = "11,12,13,14,15", "2.148,5.792,0.5,30,2,2.785,1,1,1"

  -- A vehicle VM that takes longer than RESOLVE_TIMEOUT_S to run its queued chunk -- a cannon
  -- spawning with the level -- answers the FIRST chunk after the retry has issued a second one.
  -- Dropping that reply because it is not the chunk we are currently waiting on discards a
  -- successful resolve, three times over, and the vehicle then sits in `failed` for the rest of
  -- the session having in fact resolved perfectly every time.
  rg.request(7)
  local e1 = lastEpoch()
  check("the first request issues a chunk", e1 ~= nil, tostring(rg.stateOf(7)))
  rg.onUpdate(RESOLVE_TIMEOUT_S + 0.5)
  local e2 = lastEpoch()
  check("a timeout issues a fresh chunk", e2 ~= nil and e2 ~= e1,
    string.format("epoch %s then %s", tostring(e1), tostring(e2)))
  rg.onRampGeometry(7, e1, GOOD_CIDS, GOOD_META, "")
  check("the superseded chunk's late reply is ACCEPTED", rg.get(7) ~= nil, rg.stateOf(7))

  -- ...but the guard the epoch actually exists for has to survive: a reply describing a part
  -- configuration that has since been reset must never be installed.
  rg.invalidate(7)
  check("invalidate clears the cache", rg.get(7) == nil, rg.stateOf(7))
  rg.onRampGeometry(7, e1, GOOD_CIDS, GOOD_META, "")
  check("a reply predating the invalidation is still rejected", rg.get(7) == nil,
    rg.stateOf(7) .. " - this is what the epoch is for")

  -- Three silent timeouts is a give-up, and it has to SAY so rather than reading the same as a
  -- vehicle that answered "no ramp here".
  queued = {}
  rg.request(8)
  for _ = 1, MAX_TRIES + 1 do rg.onUpdate(RESOLVE_TIMEOUT_S + 0.5) end
  check("a silent VM eventually gives up", (rg.stateOf(8)):find("GAVE UP") ~= nil, rg.stateOf(8))
  check("...and the state names the cause", (rg.stateOf(8)):find("never answered") ~= nil,
    rg.stateOf(8))
  rg.retry(8)
  check("retry() re-arms it by hand", (rg.stateOf(8)):find("PENDING") ~= nil, rg.stateOf(8))

  -- A pooled-out vehicle cannot run a queued chunk at all, so asking it is not an attempt and
  -- must not spend the budget. The vehicle most likely to be inactive is a large map prop at
  -- the far end of the level -- exactly the machine this file exists for.
  active = false
  queued = {}
  rg.invalidate(9)
  rg.request(9)
  check("no chunk is sent to a sleeping VM", #queued == 0, rg.stateOf(9))
  check("...and it says why", (rg.stateOf(9)):find("inactive") ~= nil, rg.stateOf(9))

  -- One that gave up while asleep must be reachable again once it wakes, or the give-up is
  -- permanent for a reason that has nothing to do with the vehicle.
  active = true
  rg.request(9)
  for _ = 1, MAX_TRIES + 1 do rg.onUpdate(RESOLVE_TIMEOUT_S + 0.5) end
  check("it can still give up once awake", (rg.stateOf(9)):find("GAVE UP") ~= nil, rg.stateOf(9))
  rg.onVehicleActiveChanged(9, true)
  check("becoming active again re-arms the resolve",
    (rg.stateOf(9)):find("GAVE UP") == nil, rg.stateOf(9))

  -- ...while a vehicle that is already resolved is left alone: re-resolving on every pooling
  -- boundary crossing would put the per-session chunk budget back to unbounded.
  rg.request(9)
  rg.onRampGeometry(9, lastEpoch(), GOOD_CIDS, GOOD_META, "")
  local before = rg.get(9)
  rg.onVehicleActiveChanged(9, true)
  check("a resolved vehicle is not disturbed by an activity change", rg.get(9) == before,
    rg.stateOf(9))

  -- "It answered and it has no ramp" is a settled fact about the vehicle; "it never answered"
  -- is a fact about the attempt. Both used to render as GAVE UP, which is how a stock Old
  -- Cannon parked five metres away -- a machine with genuinely no drive-in ramp, correctly
  -- resolved -- read as a broken resolve rather than as an answer.
  rg.invalidate(10)
  rg.request(10)
  rg.onRampGeometry(10, lastEpoch(), "", "", "only 0 ramp nodes")
  check("a vehicle that answered 'no ramp' is not reported as a failure",
    (rg.stateOf(10)):find("NO RAMP ON IT") ~= nil, rg.stateOf(10))
  check("...and it still names what the vehicle said",
    (rg.stateOf(10)):find("only 0 ramp nodes") ~= nil, rg.stateOf(10))
  check("...while the silent VM keeps the GAVE UP wording",
    (rg.stateOf(8)):find("GAVE UP") ~= nil, rg.stateOf(8))

  -- The spoken form is a clause, not a diagnosis: F9+I is pressed mid-manoeuvre.
  check("the speech form of each state stays short",
    rg.shortStateOf(10) == "no ramp on it" and rg.shortStateOf(9) == "ramp found"
      and rg.shortStateOf(8) == "not answering",
    string.format("'%s' / '%s' / '%s'",
      rg.shortStateOf(10), rg.shortStateOf(9), rg.shortStateOf(8)))

  _G.log, _G.scenetree = savedLog, savedTree
end
print()

print("13. the exact-epoch drop is gone from the source")
do
  local body = assert(io.open(SRC, "r")):read("*a")
  check("onRampGeometry no longer drops on 'not the chunk I am waiting on'",
    body:find("p%.epoch ~= epoch") == nil,
    "that test discarded a slow VM's perfectly good answer")
  check("...and rejects on the invalidation mark instead",
    body:find("if epoch <= staleFloor%(vehID%) then") ~= nil)
  check("a stuck resolve is recoverable from the console",
    body:find("function M%.retry") ~= nil and body:find("function M%.diag") ~= nil,
    "extensions.rampGeometry.retry(id) / .diag()")
end

print()

-- =================================================================================================
--  A synthetic tilt-deck trailer.
--
--  Not real jbeam coordinates -- unlike the cannon, whose numbers are read out of the shipped
--  file -- but the SHAPE is what these checks are about: a drive-on deck that is most of the
--  trailer, so its centroid sits on the machine's and the displacement rule has nothing to
--  read. Roughly a 22 ft deck: 6.7 m long, 2.5 m wide, on a chassis of about the same length.
-- =================================================================================================
local function deckNodes(part)
  local n = {}
  local lat = {-1.25, -0.6, 0.0, 0.6, 1.25}
  -- Deck rows from the rear (f -3.35, the end you drive onto) forward to f +3.35.
  for row = 0, 6 do
    local f = -3.35 + row * (6.7 / 6)
    for _, x in ipairs(lat) do
      local e = N("b" .. row .. (x < 0 and "r" or "l"), x, f, 0.9)
      e.part = part
      n[#n + 1] = e
    end
  end
  return n
end

-- The chassis under it: the same length, so the two centroids coincide and the displacement
-- the first axis tier depends on is numerical residue.
local function deckChassis()
  local n = {}
  for i = 1, 40 do
    n[#n + 1] = N("chassis_" .. i, ((i % 5) - 2) * 0.5, -3.3 + (i % 7) * 1.1, 0.45)
  end
  return n
end

local function fullTiltdeck(part)
  local n = deckChassis()
  for _, e in ipairs(deckNodes(part)) do n[#n + 1] = e end
  return n
end

print("14. the PART tier, which is what every ramp that is not large_cannon's needs")
do
  -- The scan of all 124 stock vehicle zips is the premise of this whole tier: node NAMES
  -- beginning ramp_ exist on exactly one drivable machine. If the allowlist ever loses these
  -- entries the mod silently goes back to seeing only the cannon, with no other symptom.
  local r = resolve(fullTiltdeck("tiltdeck_deck_22ft"))
  check("a tilt deck resolves by partOrigin", r ~= nil,
    "its nodes are b0rr/b1r/b11rr -- nothing a name tier can match")
  check("...through the part tier, not the name tier", r and r.tierName == "part",
    r and r.tierName or "no resolve")

  -- Boundary-aware in both directions: the suffix must be separated, and the word must not be
  -- allowed to end mid-token.
  check("the allowlist word may carry a slot suffix", partIsRamp("tiltdeck_deck_40ft"))
  check("...but not run into an alphanumeric", not partIsRamp("tiltdeck_deck22ft"),
    "an unbounded match is what lets a substring test hit anything")

  -- The DENYLIST is not hypothetical: every entry is a near-miss the zip scan turned up.
  check("a rollback's ram piston is not a ramp", not partIsRamp("tiltframe_rampiston"),
    "it is the hydraulic ram that TILTS the deck -- structure, not surface")
  check("...nor its ram cylinder", not partIsRamp("tiltframe_ramcylinder"))
  check("a tailgate is not a ramp", not partIsRamp("cargotrailer_tailgate"))
  check("a dump truck's bed sides are not a deck", not partIsRamp("dumptruck_deck_L"))
  check("...and a bare search for the word would take the ram piston",
    ("tiltframe_rampiston"):find("ramp", 1, true) ~= nil,
    "which is why the rule is an allowlist plus a denylist, not a search")

  -- group is jbeam data and comes in either shape.
  local byGroup = fullTiltdeck(nil)
  for _, nd in ipairs(byGroup) do
    if nd.name:find("^b") then nd.group = "tiltdeck_deck" end
  end
  check("group as a string works", resolve(byGroup) ~= nil)
  local byArray = fullTiltdeck(nil)
  for _, nd in ipairs(byArray) do
    if nd.name:find("^b") then nd.group = {"paint", "tiltdeck_deck"} end
  end
  check("group as an ARRAY works too", resolve(byArray) ~= nil,
    "a node can belong to several groups; assuming a string drops those vehicles")

  -- The property that makes this tier safe to add at all.
  local cannon = resolve(fullCannon(true))
  check("the cannon still answers through the NAME tier",
    cannon and cannon.tierName == "node name", cannon and cannon.tierName or "no resolve")
  check("...with the half-width it always gave", cannon and approx(cannon.halfW, 2.148, 1e-9),
    cannon and string.format("%.3f m", cannon.halfW) or "no resolve")
end

print()

print("15. the DECK axis tier: a mouth on a machine with no displaced ramp")
do
  local deck = resolve(fullTiltdeck("tiltdeck_deck_22ft"))
  check("it resolves", deck ~= nil)
  check("...through the deck tier", deck and deck.axisTier == 2,
    "the deck IS the trailer, so there is no centroid displacement to read")
  check("the axis is the machine's own fore/aft", deck and deck.axisName == "f")
  -- The one fact the deck tier supplies that the cloud cannot: which end is the mouth. You
  -- drive onto a trailer from behind, so the mouth is the REAR, and the sign is forced negative
  -- to say so. Getting this backwards aims the align teleport at the headboard.
  check("the mouth is the REAR of the machine", deck and deck.sgn == -1,
    "you drive onto a trailer from behind")

  -- The guard. large_spinner's nodes are literally ramp_0..ramp_5b, so it clears the NAME tier
  -- today; it is a spinning wall, and its shape is what rejects it.
  local wall = {}
  for i = 1, 20 do
    local e = N("w" .. i, (i % 2 == 0) and 0.1 or -0.1, -1.0 + (i % 10) * 0.22, 0.3 + i * 0.15)
    e.part = "tiltdeck_deck"
    wall[#wall + 1] = e
  end
  for i = 1, 40 do wall[#wall + 1] = N("body_" .. i, (i % 5) - 2, (i % 9) - 4, 0.5) end
  local wr, wwhy = resolve(wall)
  check("a tall narrow wall is rejected by the deck tier", wr == nil,
    wwhy or "it resolved, which it must not")

  -- The ordering property: the deck tier must never get a say where the displacement rule
  -- already had one, or it could move the cannon's mouth to the wrong end of the machine.
  local cannon = resolve(fullCannon(true))
  check("the cannon never reaches the deck tier", cannon and cannon.axisTier == 1,
    "its ramp is displaced 6.9x, so the first tier answers")
  -- On the cannon the two tiers happen to AGREE -- its ramp hangs off the rear, which is where
  -- the deck tier assumes a mouth is anyway. That agreement is why the ordering has to be
  -- demonstrated on a machine where they differ, and the stock fleet has one: us_semi_ramplow
  -- and md_series_ramplow sit in a *_bumper_F slot, i.e. the ramp is on the FRONT. There the
  -- displacement tier derives the mouth correctly and the deck tier's "you drive on from
  -- behind" assumption picks the wrong end of the ramp entirely.
  local front = {}
  for _, nd in ipairs(fullCannon(true)) do
    front[#front + 1] = {cid = nd.cid, name = nd.name, f = -nd.f, r = nd.r, u = nd.u}
  end
  local frontOK = resolve(front)
  local frontForced = resolve(front, {noDisplacement = true})
  check("a FRONT-mounted ramp is resolved correctly by the displacement tier",
    frontOK ~= nil and frontOK.sgn == 1 and frontOK.axisTier == 1,
    frontOK and string.format("sgn %d, tier %d", frontOK.sgn, frontOK.axisTier) or "no resolve")
  -- It does NOT quietly produce a plausible wrong answer here: the cannon's ramp cloud is
  -- 6.898 m wide against 5.792 m long, so the deck tier's length-dominance guard declines it.
  -- That is the shape of failure this tier is meant to have -- decline, or disagree visibly --
  -- and it is only safe because the displacement tier gets first refusal.
  check("...and the deck tier would not answer for it at all",
    frontForced == nil,
    frontForced and ("it picked " .. frontForced.names[1]) or "declined, as it must")
end

print()

print("16. the ramp align teleport")
do
  local IP = "bng_mod/lua/ge/extensions/implementProximity.lua"
  local body = assert(io.open(IP, "r")):read("*a")
  local standoff = readConstFrom(IP, "RAMP_ALIGN_STANDOFF_M")
  check("the standoff is 20 feet", approx(standoff, 6.096, 1e-6),
    string.format("%.3f m", standoff))

  -- Nose-referenced, not origin-referenced. The reference node is metres from the front bumper
  -- on a semi, so an origin-referenced standoff promises a gap the driver does not get.
  check("the standoff is measured from the vehicle's NOSE",
    body:find("RAMP_ALIGN_STANDOFF_M %+ nose") ~= nil and body:find("entry%.ext%.maxF") ~= nil,
    "vehicleGeometry's trimmed forward extent is the nose")
  check("...with a fallback for a vehicle geometry has not measured yet",
    body:find("RAMP_ALIGN_NOSE_FALLBACK_M") ~= nil,
    "over-estimating parks you further back; under-estimating parks you in the mouth")

  -- Direction. mouth.axis points INTO the ramp, so the teleport SUBTRACTS it to stand off and
  -- faces ALONG it. The coupler align does the opposite on both counts, because that manoeuvre
  -- is reversed into -- and swapping either one reads as perfectly reasonable in isolation.
  check("it stands off BACK from the mouth",
    body:find("mouth%.centre %- mouth%.axis %* back") ~= nil,
    "adding the axis would put the vehicle inside the ramp")
  check("...and faces INTO the ramp",
    body:find("quatFromDir%(mouth%.axis, vec3%(0, 0, 1%)%)") ~= nil,
    "negating this faces the driver away from the ramp")
  local VS = "bng_mod/lua/ge/extensions/vehicleScanner.lua"
  local vsBody = assert(io.open(VS, "r")):read("*a")
  check("...which is genuinely the opposite of the coupler align",
    vsBody:find("quatFromDir%(awayDir, vec3%(0, 0, 1%)%)") ~= nil,
    "awayDir points away from the trailer; mouth.axis points into the ramp")

  -- Lateral placement. The align CENTRES THE BODY, not the reference node, and those are not
  -- the same point: an etk800 wagon measures minR -0.680 / maxR +1.340, so its node sits 0.33 m
  -- off its own centreline. Placing the node on the axis put the body a third of a metre over
  -- and left -0.05 m on one flank of a mouth the car clears by 0.28 m a side -- and the readout
  -- then said "you do not fit", correctly about where it had just parked the car and wrongly
  -- about the car. Measured after the fix: flanks at -1.010 and +1.010 in a +-1.292 mouth.
  check("the align centres the vehicle's BODY on the ramp axis",
    body:find("pos %- mouth%.left %* bodyMid") ~= nil,
    "placing the reference node on the axis puts an off-centre body off-centre")
  check("...and then quotes the SYMMETRIC half-span as the margin",
    body:find("local hw = bodyHalf or playerHalfWidth") ~= nil,
    "once the body is centred the clearance is equal both sides")
  -- ...while the LIVE readout keeps the worse-side rule, because there the driver is wherever
  -- they are and the margin has to come from the flank that hits first.
  check("the live readout still measures from the worse side",
    body:find("rampMeasure%(origin, mouth, playerHalfWidth") ~= nil
      and body:find("math%.max%(math%.abs%(entry%.ext%.minR%), math%.abs%(entry%.ext%.maxR%)%)") ~= nil,
    "the align chooses its lateral position; the driver's own approach does not")

  -- The deployment check. The align is geometrically perfect against a STOWED ramp, which is
  -- how a driver ends up parked twenty feet in front of the back of a truck. Measured on a
  -- us_semi rollback: 1.30 m lip home and level, 0.95 m on tilt alone, 0.05 m fully deployed.
  check("the align measures how high the lip is off the ground",
    body:find("RAMP_ALIGN_LIP_SAY_M") ~= nil and body:find("mouth%.floorZ %- g") ~= nil,
    "neither pitch nor extension answers 'can a car get onto this' on its own")
  check("...and reports failure as NA rather than a zero reading as 'on the ground'",
    body:find('local lipStr = "NA"') ~= nil,
    "getSurfaceHeightBelow fails as a huge negative, so this is a magnitude band")

  -- The command socket. setsockname returns nil plus a message rather than throwing, so a
  -- pcall around it reports success on a socket bound to nothing: the extension goes deaf with
  -- nothing logged, while still sending normally because a sender needs no bind.
  check("the command bind checks setsockname's RETURN, not just for a throw",
    body:find("local bound, berr = udpCmd:setsockname") ~= nil,
    "setsockname does not throw; a bare pcall cannot see it fail")
  check("...closes its sockets on unload, so a reload does not leak the port",
    body:find("function M%.onExtensionUnloaded") ~= nil,
    "a reloaded module's locals are nil, so setupSockets closes nothing")
  check("...and retries a failed bind rather than staying deaf for the session",
    body:find("CMD_BIND_RETRY_S") ~= nil,
    "the usual cause is a leaked socket the GC frees moments later")

  -- The teleport call itself, and the bug the scanner already paid for once.
  -- checkOnlyStatics and visibilityPoint stay nil: a boolean visibilityPoint throws inside
  -- LuaVec3.__sub and the teleport silently never fires. The TRAILING false is resetVehicle,
  -- which defaults to TRUE and would repair the very damage the driver came to keep -- this
  -- check originally asserted the bare three-argument call and was not updated when that rule
  -- landed, so it failed against correct code. vehicle_geometry_sim.lua scenario 13 owns the
  -- rule mod-wide; this asserts this one call site agrees.
  check("it uses spawn.safeTeleport with explicit nils and resetVehicle = false",
    body:find("spawn%.safeTeleport%(player, pos, rot, nil, nil, nil, false, false%)") ~= nil,
    "a boolean visibilityPoint throws inside LuaVec3.__sub; resetVehicle defaults to true")

  -- Ground reference. A tilt deck's mouth floor is a metre in the air until the deck is down,
  -- so it is the wrong Z to copy; the player is standing on the ground by definition.
  check("ground height comes from the player, not the mouth floor",
    body:find("pos%.z = player:getPosition%(%)%.z %+ 0%.3") ~= nil
      and body:find("pos%.z = mouth%.floorZ") == nil,
    "a raised deck would teleport the vehicle into the air")

  -- The gate, enforced in the mod as well as in Python: a RAMPALIGN arriving with the
  -- instrument off is a version skew, not a request to teleport somebody onto a machine they
  -- were trying to tow.
  check("the mod enforces the docking gate itself",
    body:find("if not dockActive then return rampAlignFail") ~= nil)
  -- ...and the MODE gate with it. The instrument decides implement-vs-ramp per tick from
  -- whether an implement is fitted; the align has to use the same rule, or the key can teleport
  -- you to something the readout was never talking about.
  check("...and refuses when the instrument is in implement mode",
    body:find("if implCids then") ~= nil
      and body:find("implement fitted, so the instrument is not in ramp mode") ~= nil)
  check("it is reachable from the command port",
    body:find('elseif cmd == "RAMPALIGN" then') ~= nil)

  -- Python must not silently fall back to the coupler align.
  local py = assert(io.open("beamtel.py", "r")):read("*a")
  check("Python routes Shift+V by the docking instrument",
    py:find("if dock_mode_active:") ~= nil
      and py:find('_send_implement_cmd%("RAMPALIGN"%)') ~= nil)
  check("...and speaks the mod's reason rather than falling through",
    py:find("RAMPALIGN:") ~= nil and py:find('startswith%("FAIL,"%)') ~= nil)
end

print()

-- THE FAILURE REPORT USED TO BE HERE AS WELL AS AT THE FOOT OF THE FILE, and that is a harness
-- bug worth naming rather than quietly deleting. Everything below it ran, and its checks were
-- recorded in `failures`, but nothing ever read that list again -- the file ended with a bare
-- print("all checks passed"). So every scenario from 17 onward could fail and the sim would
-- still exit 0 saying it passed, which is the one failure mode a test harness must not have.
-- It was hiding a real one: the blanket rg.has grep above had been failing since RAMPSELF was
-- added. One report, at the end, after every check has run.
print("17. isCannon is a different question from has")
do
  -- The regression this exists for. beamtel's firing readout keyed off has(), which meant "this
  -- machine has a drive-in ramp" -- a sound proxy only while large_cannon was the only vehicle
  -- whose ramp resolved at all. rampGeometry's part tiers (scenario 14) exist precisely so a
  -- rollback, a tilt deck and a dry van resolve too, and every one of them then latched CANNON.
  -- Measured in a us_semi tc82s_rollback: F9 I answered "Inclination 100 percent, strength
  -- unknown", the inclination being the truck's own engine RPM over a thousand -- pegged
  -- because its hydraulic pump raises idle to 1500 -- and the strength a gear string that is
  -- not a percentage. It also masked the ramp readout that machine should have got.
  local rg = assert(loadfile(SRC))()
  local active = true
  local fakeVeh = {
    queueLuaCommand = function() end,
    getActive = function() return active end,
  }
  local savedLog, savedTree = _G.log, _G.scenetree
  _G.log = function() end
  _G.scenetree = {findObjectById = function() return fakeVeh end}

  local CIDS = "11,12,13,14,15"
  -- Same geometry both times. The ONLY difference is the eighth meta field, which is the point:
  -- nothing about the mouth distinguishes a cannon from a trailer, so nothing geometric could
  -- ever have made this call.
  rg.request(21)
  rg.onRampGeometry(21, 1, CIDS, "2.148,5.792,0.5,30,2,2.785,1,0,1", "")
  rg.request(22)
  rg.onRampGeometry(22, 2, CIDS, "2.148,5.792,0.5,30,2,2.785,1,1,1", "")

  check("a rollback resolves as a ramp", rg.has(21), rg.stateOf(21))
  check("...and is NOT reported as a cannon", not rg.isCannon(21),
    "this is the bug: has() said yes, and the firing readout believed it")
  check("the cannon resolves as a ramp too", rg.has(22), rg.stateOf(22))
  check("...and IS reported as a cannon", rg.isCannon(22), rg.stateOf(22))
  check("the old form cannot tell them apart", rg.has(21) == rg.has(22),
    "which is exactly why the readout fired on a truck")
  check("the state line names the cannon", (rg.stateOf(22)):find("large_cannon") ~= nil,
    rg.stateOf(22))
  check("...and does not name it on the rollback",
    (rg.stateOf(21)):find("large_cannon") == nil, rg.stateOf(21))

  -- Conservative on anything unresolved: the readout it gates reads live electrics that mean
  -- nothing on an ordinary vehicle, so "not known yet" must answer no rather than maybe.
  check("an unresolved vehicle is not a cannon", not rg.isCannon(23), rg.stateOf(23))

  -- The 7-field reply is what a half-updated install sends, and bng_mod/ is a live junction, so
  -- it genuinely happens. It must fail loudly rather than land with isCannon nil.
  rg.request(24)
  -- A deliberately high epoch, not a guessed one: epochCounter is module-global and every
  -- request above (isCannon self-arms one too) advanced it, so a hardcoded number quietly makes
  -- the reply a superseded chunk's -- which is held, not rejected, and the check then passes
  -- for the wrong reason.
  rg.onRampGeometry(24, 999, CIDS, "2.148,5.792,0.5,30,2,2.785,1", "")
  check("a 7-field reply is rejected as malformed", rg.get(24) == nil, rg.stateOf(24))
  check("...and says so", (rg.stateOf(24)):find("malformed") ~= nil, rg.stateOf(24))

  _G.log, _G.scenetree = savedLog, savedTree
end

print()

do
  -- The consumer. Nothing but a grep enforces that implementProximity asks the right question,
  -- and the two calls differ by one word.
  local fh = assert(io.open("bng_mod/lua/ge/extensions/implementProximity.lua", "r"))
  local src = fh:read("*a")
  fh:close()
  check("implementProximity derives the cannon kind from isCannon",
    src:find("rg.isCannon", 1, true) ~= nil)
  -- Scoped to the CANNON readout, not to the whole file. The blanket form banned rg.has
  -- outright, which was right while the cannon block was the only caller and became wrong the
  -- moment RAMPSELF arrived -- that one asks "does the machine I am driving have a ramp", which
  -- is exactly what has() means and exactly what it should use. The blanket check had in fact
  -- been failing ever since, unnoticed, for the harness reason recorded at the foot of this
  -- file. Anchored on the branch itself so it still catches the regression it exists for.
  local ci = src:find('if kind == "0"', 1, true)
  local cannonBranch = ci and src:sub(ci, ci + 400) or nil
  check("...and the cannon branch never asks rg.has",
    cannonBranch ~= nil and cannonBranch:find("rg.has", 1, true) == nil,
    "has() means 'you can drive into this', not 'this can be fired': " .. tostring(cannonBranch))
end

print()

print("18. a us_semi rollback deck: a mouth row that is not one plane")
do
  -- The 72 real us_semi_rollback_deck node positions, read out of a running game with the deck
  -- at its home pose (no tilt, no extension) and projected onto the machine's own fwd/rgt/up --
  -- i.e. exactly what the chunk computes. Coordinates are metres, r positive to the driver's
  -- LEFT. Kept verbatim rather than idealised because the whole finding is a property of the
  -- real cloud: the deck is FOUR rails at TWO structural levels, an understructure at u
  -- 0.07-0.14 and the drivable surface at u 0.49-0.60, and no synthetic ramp would have had
  -- that shape.
  --
  -- Ground truth from the same dump: lateral runs +1.732 to -0.859, so the deck is 2.591 m wide
  -- and its half-width is 1.296 m. Note it is NOT symmetric about the vehicle origin -- the
  -- deck centreline sits at +0.44 -- which is its own reason MIN_WALL_LATERAL_M cannot be read
  -- as "inboard of the mouth".
  local function D(name, r, f, u) return N(name, r, f, u) end
  local deck = {
    D("cf0ll",1.665,-9.12,0.495), D("cf0l",0.99,-9.12,0.495), D("cf0rr",-0.786,-9.122,0.492),
    D("cf0r",-0.11,-9.121,0.493), D("cf0",0.44,-9.121,0.494), D("cf10ll",1.665,-9.12,0.545),
    D("cf10l",1.02,-9.121,0.545), D("cf10rr",-0.786,-9.122,0.542), D("cf10r",-0.14,-9.122,0.543),
    D("cf10",0.44,-9.121,0.544), D("cf11ll",1.731,-8.41,0.526), D("cf11l",1.019,-8.411,0.525),
    D("cf11rr",-0.853,-8.412,0.522), D("cf11r",-0.141,-8.411,0.524), D("cf12ll",1.73,-6.532,0.541),
    D("cf12l",1.018,-6.533,0.54), D("cf12rr",-0.854,-6.534,0.537), D("cf12r",-0.142,-6.533,0.538),
    D("cf13ll",1.729,-4.39,0.558), D("cf13l",1.017,-4.39,0.557), D("cf13rr",-0.856,-4.391,0.555),
    D("cf13r",-0.144,-4.391,0.556), D("cf14ll",1.727,-2.15,0.577), D("cf14l",1.015,-2.15,0.576),
    D("cf14rr",-0.857,-2.151,0.574), D("cf14r",-0.145,-2.151,0.575), D("cf15ll",1.726,-0.48,0.591),
    D("cf15l",1.014,-1.13,0.585), D("cf15rr",-0.858,-0.481,0.588), D("cf15r",-0.146,-1.131,0.583),
    D("cf16ll",1.726,0.12,0.597), D("cf16l",1.014,0.12,0.596), D("cf16rr",-0.859,0.118,0.594),
    D("cf16r",-0.146,0.119,0.595), D("cf1ll",1.732,-8.409,0.374), D("cf1l",1.02,-8.408,0.068),
    D("cf1rr",-0.853,-8.411,0.37), D("cf1r",-0.14,-8.409,0.067), D("cf2ll",1.73,-6.531,0.346),
    D("cf2l",1.019,-6.53,0.083), D("cf2rr",-0.854,-6.533,0.342), D("cf2r",-0.141,-6.531,0.081),
    D("cf3ll",1.729,-4.388,0.363), D("cf3l",1.017,-4.387,0.1), D("cf3rr",-0.855,-4.39,0.36),
    D("cf3r",-0.143,-4.388,0.099), D("cf4ll",1.727,-2.148,0.382), D("cf4l",1.015,-2.147,0.119),
    D("cf4rr",-0.857,-2.15,0.379), D("cf4r",-0.144,-2.148,0.118), D("cf5ll",1.726,-0.478,0.396),
    D("cf5l",1.015,-1.127,0.127), D("cf5rr",-0.858,-0.48,0.393), D("cf5r",-0.145,-1.128,0.126),
    D("cf6ll",1.726,0.122,0.402), D("cf6l",1.014,0.09,0.139), D("cf6rr",-0.858,0.12,0.399),
    D("cf6r",-0.146,0.09,0.138), D("ci0ll",1.731,-7.929,0.53), D("ci0rr",-0.853,-7.931,0.526),
    D("ci1ll",1.731,-7.229,0.535), D("ci1rr",-0.854,-7.231,0.532), D("ci2ll",1.73,-5.779,0.547),
    D("ci2rr",-0.855,-5.781,0.543), D("ci3ll",1.729,-5.079,0.552), D("ci3rr",-0.855,-5.081,0.549),
    D("ci4ll",1.728,-3.629,0.564), D("ci4rr",-0.856,-3.631,0.561), D("ci5ll",1.728,-2.879,0.57),
    D("ci5rr",-0.857,-2.881,0.567), D("ci6ll",1.727,-1.319,0.584), D("ci6rr",-0.858,-1.321,0.581),
  }
  for _, nd in ipairs(deck) do nd.part = "us_semi_rollback_deck" end

  -- The rest of the truck, so there is a machine centroid for the displacement tier to measure
  -- against. Shape is irrelevant and deliberately crude; all that matters is that the mass of
  -- the vehicle sits FORWARD of the deck, which on a cabover semi it emphatically does.
  local machine = {}
  for _, nd in ipairs(deck) do machine[#machine + 1] = nd end
  for i = 1, 60 do
    machine[#machine + 1] = N("cab_" .. i, ((i % 5) - 2) * 0.5, -3.0 + (i % 12) * 0.55, 1.0)
  end

  local r = resolve(machine)
  check("the rollback deck resolves", r ~= nil, r == nil and "no resolve" or "")
  check("...through the PART tier", r and r.tierName == "part",
    "its nodes are cf0ll/ci3rr, nothing a ramp_ name tier could see")
  check("...and by displacement, mouth at the REAR", r and r.axisTier == 1 and r.sgn == -1,
    r and string.format("tier %d, sgn %d", r.axisTier, r.sgn) or "")

  -- THE FINDING. The row is two levels, so there is no single floor to measure wall height
  -- from, and the wall rule must decline rather than answer.
  check("the mouth row is recognised as NOT one plane", r and r.mouthCoherent == false,
    "an understructure 0.45 m below the deck surface is not a floor and a wall")
  check("...so neither side used the wall rule", r and r.wallUsed == 0,
    r and string.format("%d of 2", r.wallUsed) or "")
  check("the half-width is the deck's real half-width", r and approx(r.halfW, 1.296, 0.01),
    r and string.format("%.3f m against a measured 1.296 m", r.halfW) or "")

  -- ...and the negative control, which is the shipped behaviour this replaces. Measured in
  -- game before the fix: halfW 0.93566, naiveHalfW 1.29281, wallUsed 2.
  local bug = resolve(machine, {noCoherence = true, uncappedBand = true})
  check("believing the fit picks an INNER rail as the wall", bug and bug.wallUsed == 2,
    bug and string.format("%d of 2", bug.wallUsed) or "")
  check("...and under-reports the deck by 0.36 m per side",
    bug and approx(bug.halfW, 0.936, 0.01),
    bug and string.format("%.3f m -- the game reported 0.93566", bug.halfW) or "")
  check("...while its own naive figure was right all along",
    bug and approx(bug.naiveHalfW, 1.293, 0.01),
    bug and string.format("%.3f m -- the game reported 1.29281", bug.naiveHalfW) or "")
  check("the two disagree about whether a 2.2 m car fits",
    bug and r and (bug.halfW * 2 < 2.2) and (r.halfW * 2 > 2.2),
    "which is the whole cost of it: RAMPALIGN refusing a car that fits easily")

  -- The mouth pair must be a ROW. The deck's side rail runs at the same lateral offset through
  -- every station in the band, so the extremes are a tie to the millimetre and a strict
  -- comparison settles it by table order. Measured in game: markers 1.45 m apart along the ramp,
  -- a live 3-D half-width of 1.42 m against a meta figure of 1.29 m from the same resolve, and
  -- an axis skewed by the difference. The lateral answer was already correct throughout, which
  -- is exactly why nothing in the meta could show it.
  check("the two mouth markers sit on one station", r and r.edgeAlongGap < 0.05,
    r and string.format("%.3f m apart along the ramp", r.edgeAlongGap) or "")
  local tied = resolve(machine, {orderTies = true})
  check("...where table order would spread them across the band",
    tied and tied.edgeAlongGap > 0.5,
    tied and string.format("%.3f m apart", tied.edgeAlongGap) or "")
  check("...while reporting the same lateral half-width either way",
    tied and r and approx(tied.halfW, r.halfW, 0.02),
    "which is why the meta could never reveal it")

  -- The band cap, separately. The deck is 9.24 m home and about 12 m with the bed run out, so
  -- the pure fraction makes the "mouth row" a slab metres deep and the two wall picks land at
  -- different stations along the ramp -- measured at 1.49 m apart, which is not a row.
  check("the row band is capped", r and approx(r.rowBand, ROW_BAND_MAX_M, 1e-9),
    r and string.format("%.3f m", r.rowBand) or "")
  local uncapped = resolve(machine, {uncappedBand = true})
  check("...where the pure fraction would have given a slab",
    uncapped and uncapped.rowBand > ROW_BAND_MAX_M,
    uncapped and string.format("%.3f m of a %.2f m deck", uncapped.rowBand, uncapped.span) or "")

  -- The property that makes all of this safe to add: large_cannon's mouth row IS one plane, so
  -- it never reaches any of the new code, and the cap does not change which nodes are in its
  -- band (2.03 m uncapped against a 1.264 m row spread).
  local cannon = resolve(fullCannon(true))
  check("the cannon's mouth row is still one plane", cannon and cannon.mouthCoherent == true,
    "a single ramp floor fits its own line to near zero")
  check("...still answers by the wall rule, both sides", cannon and cannon.wallUsed == 2,
    cannon and string.format("%d of 2", cannon.wallUsed) or "")
  check("...with the half-width it has always given", cannon and approx(cannon.halfW, 2.148, 1e-9),
    cannon and string.format("%.3f m", cannon.halfW) or "")
end

print()

print("19. declared mouths, two of them, and a pitch the resolve refuses to publish")
do
  -- Three shipped props defeat the inference outright, and the hamster wheel defeats it twice
  -- over: it carries NO node groups at all and one partOrigin covering both drive-in ramps AND
  -- the whole A-frame, so no subset the tiers can name is a mouth -- and it has TWO mouths, one
  -- at each end of its axle, which a single centroid displacement cannot point at even given a
  -- perfect cloud. Hence a declared mouth: five node NAMES per mouth, in the cache's own order.
  local rg = assert(loadfile(SRC))()
  local savedLog, savedTree, savedPV = _G.log, _G.scenetree, _G.getPlayerVehicle
  local savedVec3 = _G.vec3

  -- mouthFrame is the first thing in this file to be driven directly, so it is the first to
  -- need the engine's vec3. Only the handful of operations it actually performs are provided;
  -- anything else should fail loudly rather than return a plausible number.
  local VM = {}
  VM.__index = VM
  local function V(x, y, z) return setmetatable({x = x, y = y, z = z}, VM) end
  VM.__add = function(a, b) return V(a.x + b.x, a.y + b.y, a.z + b.z) end
  VM.__sub = function(a, b) return V(a.x - b.x, a.y - b.y, a.z - b.z) end
  VM.__mul = function(a, k) return V(a.x * k, a.y * k, a.z * k) end
  function VM:length() return math.sqrt(self.x ^ 2 + self.y ^ 2 + self.z ^ 2) end
  function VM:distance(o)
    return math.sqrt((self.x - o.x) ^ 2 + (self.y - o.y) ^ 2 + (self.z - o.z) ^ 2)
  end
  function VM:normalized()
    local l = self:length()
    if l < 1e-12 then return V(0, 0, 0) end
    return V(self.x / l, self.y / l, self.z / l)
  end
  function VM:dot(o) return self.x * o.x + self.y * o.y + self.z * o.z end
  function VM:cross(o)
    return V(self.y * o.z - self.z * o.y,
             self.z * o.x - self.x * o.z,
             self.x * o.y - self.y * o.x)
  end
  _G.vec3 = function(a, b, c)
    if type(a) == "table" then return V(a.x or 0, a.y or 0, a.z or 0) end
    return V(a or 0, b or 0, c or 0)
  end

  -- Two mouths 23.5 m apart on the x axis, mirroring the real rig: mouth 1 at x +11.74 facing
  -- back toward the drum, mouth 2 at x -11.74 facing the other way. Cids 1..5 are mouth 1,
  -- 6..10 are mouth 2.
  local NODEPOS = {
    [1] = {0, -3.37, 0}, [2] = {0, 0, 0}, [3] = {0, 3.37, 0},      -- mouth 1 row, at f +11.74
    [4] = {0, -3.37, 0}, [5] = {0, 3.37, 0},                        -- mouth 1 inner, at f +5.30
    [6] = {0, 3.37, 0},  [7] = {0, 0, 0},  [8] = {0, -3.37, 0},     -- mouth 2 row, at f -11.74
    [9] = {0, 3.37, 0},  [10] = {0, -3.37, 0},                      -- mouth 2 inner, at f -5.30
  }
  local ALONG = {[1] = 11.74, [2] = 11.74, [3] = 11.74, [4] = 5.30, [5] = 5.30,
                 [6] = -11.74, [7] = -11.74, [8] = -11.74, [9] = -5.30, [10] = -5.30}
  local fakeVeh = {
    queueLuaCommand = function() end,
    getActive = function() return true end,
    getPosition = function() return {x = 0, y = 0, z = 0} end,
    getNodePosition = function(_, cid)
      local p = NODEPOS[cid]
      return {x = ALONG[cid], y = p[2], z = p[3]}
    end,
  }
  local playerAt = {x = 0, y = 0, z = 0}
  _G.log = function() end
  _G.scenetree = {findObjectById = function() return fakeVeh end}
  _G.getPlayerVehicle = function() return {getPosition = function() return playerAt end} end

  local CIDS = "1,2,3,4,5,6,7,8,9,10"
  -- axisTier 3 is "declared", and the ninth field says the chosen nodes are collision surface.
  rg.request(41)
  rg.onRampGeometry(41, 1, CIDS, "3.37,6.44,0,10,2,3.37,3,0,1", "declared mouth, 2 on this machine")
  check("a two-mouth declaration resolves", rg.has(41), rg.stateOf(41))
  check("...and says so rather than claiming a derivation",
    rg.stateOf(41):find("DECLARED") ~= nil and rg.stateOf(41):find("2 mouths") ~= nil,
    rg.stateOf(41))

  -- WHICH mouth is a fact about where the driver is, not about the machine. Standing at the +x
  -- end must not send them round to the -x one.
  playerAt = {x = 40, y = 0, z = 0}
  local near = rg.mouthFrame(41)
  check("the nearer mouth is the one answered", near and near.mouthIndex == 1,
    near and string.format("mouth %d of %d at x %.2f", near.mouthIndex, near.mouthCount,
      near.centre.x) or "no frame")
  check("...and its axis points INTO the machine from that side",
    near and near.axis.x < -0.95,
    near and string.format("axis (%.2f, %.2f)", near.axis.x, near.axis.y) or "")
  playerAt = {x = -40, y = 0, z = 0}
  local far = rg.mouthFrame(41)
  check("standing at the other end answers the other mouth", far and far.mouthIndex == 2,
    far and string.format("mouth %d of %d at x %.2f", far.mouthIndex, far.mouthCount,
      far.centre.x) or "no frame")
  check("...with the axis reversed to match", far and far.axis.x > 0.95,
    far and string.format("axis (%.2f, %.2f)", far.axis.x, far.axis.y) or "")
  check("a fixed-mouth form would have sent the driver round the machine",
    near and far and math.abs(near.centre.x - far.centre.x) > 20,
    "the two mouths are 23.5 m apart -- answering one of them always is a walk, not a rounding")

  -- The floor-trust flag. The Wheel Roller tilt ramp resolves through the onramp group, whose
  -- inner row is collision:false and hangs 0.76 m UNDER the deck -- so the plane through the
  -- five chosen nodes is structure, and the ramp announced "ramp down 33 degrees" and "ramp not
  -- down, lip 1.9 feet up" while sitting dead level. The mouth itself was right throughout,
  -- which is why the answer is to withhold two fields rather than reject the resolve.
  rg.request(42)
  rg.onRampGeometry(42, 1, CIDS, "3.37,6.44,0,10,2,3.37,3,0,0", "declared, untrusted floor")
  local un = rg.mouthFrame(42)
  check("an untrusted floor withholds the pitch", un and un.pitchDeg == nil,
    un and tostring(un.pitchDeg) or "no frame")
  check("...and says so in the state line",
    rg.stateOf(42):find("PITCH WITHHELD") ~= nil, rg.stateOf(42))
  check("...while still publishing a usable mouth",
    un and un.centre ~= nil and un.axis ~= nil and un.halfW > 3.0,
    un and string.format("half-width %.2f m", un.halfW) or "")
  check("the OLD form would have reported it as perfectly level",
    (un and un.pitchDeg or 0.0) == 0.0,
    "`pitch or 0.0` is why this needed a sentinel rather than a nil -- zero is a claim")

  -- The arity guard is what makes a half-updated install fail loudly. bng_mod/ is a live
  -- junction into the game folder, so the two halves genuinely do go out of step.
  rg.request(43)
  rg.onRampGeometry(43, 1, CIDS, "3.37,6.44,0,10,2,3.37,3,0", "eight fields, an older mod half")
  check("an old eight-field reply is refused rather than misread", not rg.has(43), rg.stateOf(43))
  -- ...and a cid list that is not a whole number of mouths is equally malformed.
  rg.request(44)
  rg.onRampGeometry(44, 1, "1,2,3,4,5,6", "3.37,6.44,0,10,2,3.37,3,0,1", "six cids")
  check("a partial mouth is refused too", not rg.has(44), rg.stateOf(44))

  -- The declared table has to name nodes that exist, and it is the one thing here that no fake
  -- can check -- so the source is grepped for the two entries and for the collision reasoning.
  local src = assert(io.open(SRC, "r")):read("*a")
  check("the hamster wheel is declared with both its ramps",
    src:find("large_hamster_wheel%s*=%s*{") ~= nil
      and src:find('"r08ll"') ~= nil and src:find('"r02rr"') ~= nil,
    "five names per mouth, mirrored between the two")
  -- Scoped to the TABLE, because the comment above it has to spell out the very node names it
  -- is banning -- the trap vehicle_geometry_sim scenario 12 fell into with `setsockname`, and
  -- the one the no-percent comment in rampGeometry fell into twice.
  local di = src:find("local DECLARED_MOUTHS = {", 1, true)
  local declTable = di and src:sub(di, di + 700) or nil
  check("...using collision nodes, not the outer toe",
    declTable ~= nil and declTable:find("_2") == nil,
    "the _2 toe nodes are collision = false; a floor through them is a fiction")
  check("the wheel roller is deliberately NOT declared",
    src:find("WHY THE WHEEL ROLLER IS NOT IN THAT TABLE") ~= nil,
    "its only candidate mouth is the wheel track, so the width margin would lie")

  _G.log, _G.scenetree, _G.getPlayerVehicle, _G.vec3 =
    savedLog, savedTree, savedPV, savedVec3
end

print()

if #failures > 0 then
  print(string.format("%d FAILURE(S): %s", #failures, table.concat(failures, ", ")))
  os.exit(1)
end
print("all checks passed")
