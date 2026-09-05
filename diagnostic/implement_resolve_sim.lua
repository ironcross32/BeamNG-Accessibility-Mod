-- Replay the implement node-resolution tiers against synthetic v.data tables.
--
--     lua diagnostic/implement_resolve_sim.lua
--
-- This exists because the first version of resolveImplementNodes shipped a bug that no
-- syntax check could catch: a tier proposed a part name, the name collected zero nodes, and
-- because the tier had already committed, the later tier that WOULD have found the bucket
-- never ran. The machine then silently reported no implement. Every case below is built
-- from the real WL-40 jbeam names.
--
-- The keyword lists are parsed out of the protocol file rather than copied, so retuning them
-- there cannot silently invalidate these checks. Only the tier *logic* is duplicated.

local SRC = "bng_mod/lua/vehicle/protocols/796F6C6F313035.lua"

local function readWordList(name)
  local fh = assert(io.open(SRC, "r"), "cannot open " .. SRC)
  local body = fh:read("*a")
  fh:close()
  local list = body:match("local " .. name .. "%s*=%s*{(.-)}")
  assert(list, "could not find " .. name .. " in " .. SRC)
  local out = {}
  for w in list:gmatch('"(.-)"') do out[#out + 1] = w end
  assert(#out > 0, name .. " parsed empty")
  return out
end

local IMPL_SLOT_WORDS = readWordList("IMPL_SLOT_WORDS")
local IMPL_PART_WORDS = readWordList("IMPL_PART_WORDS")

local function readNumber(name)
  local fh = assert(io.open(SRC, "r"), "cannot open " .. SRC)
  local body = fh:read("*a")
  fh:close()
  local n = body:match("local " .. name .. "%s*=%s*(%d+)")
  assert(n, "could not find " .. name .. " in " .. SRC)
  return tonumber(n)
end

local IMPL_MIN_ATTACH_NODES = readNumber("IMPL_MIN_ATTACH_NODES")

-- Boundary-aware, mirroring the protocol file. Lowered copy for the search, ORIGINAL for the
-- camel hump: "wl40_liftarm_blockForks" only matches "fork" through the hump.
local function matchesAnyWord(s, words)
  if type(s) ~= "string" then return false end
  local low = s:lower()
  for _, w in ipairs(words) do
    local from = 1
    while true do
      local i = low:find(w, from, true)
      if not i then break end
      local prev = (i > 1) and low:sub(i - 1, i - 1) or nil
      local atBoundary = (prev == nil) or prev:match("%W") ~= nil or prev:match("%d") ~= nil
      local atHump = s:sub(i, i):match("%u") ~= nil
      if atBoundary or atHump then return true end
      from = i + 1
    end
  end
  return false
end

-- The pre-fix matcher, kept so the false-positive scenarios can assert that the old form
-- really did accept a roof scoop and a rollback ramp. A check that cannot fail is not a check.
local function matchesAnyWordSubstring(s, words)
  if type(s) ~= "string" then return false end
  s = s:lower()
  for _, w in ipairs(words) do
    if s:find(w, 1, true) then return true end
  end
  return false
end

-- The tier logic, mirroring resolveImplementNodes. `opts.actuated` is the actuation gate --
-- whether the machine has implement lift/tilt hydraulic cylinders -- and defaults to true so
-- the loader scenarios read the way they always did. `opts.firstMatch` replays the pre-fix
-- slot tiers, which took whichever candidate pairs() offered first with no size floor.
local function resolve(data, opts)
  opts = opts or {}
  local actuated = (opts.actuated ~= false)
  local match = opts.substring and matchesAnyWordSubstring or matchesAnyWord
  local partName, cids = nil, {}

  -- The gate, first: nothing below it runs on a machine whose rams do not move an implement.
  if not actuated then return nil, 0 end

  local function collect(candidate)
    if type(candidate) ~= "string" or candidate == "" then return nil end
    local out = {}
    for _, nd in pairs(data.nodes) do
      if nd.partOrigin == candidate and nd.pos then out[#out + 1] = nd.cid end
    end
    if #out < 4 then return nil end
    return out
  end

  local function pickCandidate(names)
    local namedN, namedC, bigN, bigC = nil, nil, nil, nil
    for name in pairs(names) do
      local got = collect(name)
      if got then
        if match(name, IMPL_PART_WORDS) then
          if not namedC or #got > #namedC or (#got == #namedC and name < namedN) then
            namedN, namedC = name, got
          end
        elseif #got >= IMPL_MIN_ATTACH_NODES then
          if not bigC or #got > #bigC or (#got == #bigC and name < bigN) then
            bigN, bigC = name, got
          end
        end
      end
    end
    if namedN then return namedN, namedC end
    return bigN, bigC
  end

  local slotNames = {}
  for _, nd in pairs(data.nodes) do
    if nd.partOrigin and nd.partOrigin ~= ""
        and match(nd.partPath, IMPL_SLOT_WORDS) then
      if opts.firstMatch then
        local got = collect(nd.partOrigin)
        if got and not partName then partName, cids = nd.partOrigin, got end
      else
        slotNames[nd.partOrigin] = true
      end
    end
  end
  if not opts.firstMatch then
    local n, got = pickCandidate(slotNames)
    if n then partName, cids = n, got end
  end

  if not partName then
    for _, nd in pairs(data.nodes) do
      if match(nd.partOrigin, IMPL_PART_WORDS) then
        local got = collect(nd.partOrigin)
        if got then partName, cids = nd.partOrigin, got break end
      end
    end
  end

  if not partName then
    local pathNames = {}
    for path, name in pairs(data.activeParts or {}) do
      if match(path, IMPL_SLOT_WORDS) and type(name) == "string" then
        if opts.firstMatch then
          local got = collect(name)
          if got and not partName then partName, cids = name, got end
        else
          pathNames[name] = true
        end
      end
    end
    if not opts.firstMatch then
      local n, got = pickCandidate(pathNames)
      if n then partName, cids = n, got end
    end
  end

  return partName, #cids
end

-- Node-table builder. Nodes are keyed by cid, the way v.data.nodes is.
local function build(specs)
  local nodes, cid = {}, 0
  for _, spec in ipairs(specs) do
    for _ = 1, spec.count do
      nodes[cid] = {
        cid = cid,
        pos = {x = 0, y = 0, z = 0},
        partOrigin = spec.origin,
        partPath = spec.path,
      }
      cid = cid + 1
    end
  end
  return nodes
end

local failures = 0
local function check(label, ok, detail)
  print(string.format("   %s: %s%s", label, ok and "OK" or "FAIL",
    detail and (" - " .. detail) or ""))
  if not ok then failures = failures + 1 end
end

local MACHINE = {
  count = 60, origin = "wl40", path = "/wl40",
}
local LIFTARM = {
  count = 20, origin = "wl40_liftarm",
  path = "/wl40/wl40_loader_liftarm/wl40_liftarm",
}

print("1. WL-40 with the default bucket")
local n, c = resolve({nodes = build({
  MACHINE, LIFTARM,
  {count = 40, origin = "wl40_bucket",
   path = "/wl40/wl40_loader_liftarm/wl40_liftarm_attachment/wl40_bucket"},
})})
check("resolves the bucket", n == "wl40_bucket", tostring(n) .. " / " .. c .. " nodes")

print("2. WL-40 with block handler forks")
n, c = resolve({nodes = build({
  MACHINE, LIFTARM,
  {count = 36, origin = "wl40_liftarm_blockForks",
   path = "/wl40/wl40_loader_liftarm/wl40_liftarm_attachment/wl40_liftarm_blockForks"},
})})
check("resolves the forks", n == "wl40_liftarm_blockForks", tostring(n) .. " / " .. c .. " nodes")

print("3. WL-40 with the sorting grapple (keyword tier only, no 'attachment' in path)")
n, c = resolve({nodes = build({
  MACHINE, LIFTARM,
  {count = 50, origin = "wl40_loader_log_grapple_sorting",
   path = "/wl40/wl40_loader_liftarm/wl40_grapple"},
})})
check("resolves the grapple", n == "wl40_loader_log_grapple_sorting", tostring(n))

print("4. THE REGRESSION: activeParts names a part with no nodes")
-- This is the shape that broke it in the game. The slot-path tier matches and proposes a
-- name that collects nothing; the keyword tier must still get its turn.
n, c = resolve({
  nodes = build({
    MACHINE, LIFTARM,
    {count = 40, origin = "wl40_bucket", path = "/wl40/wl40_liftarm/wl40_bucket"},
  }),
  activeParts = {["/wl40/wl40_liftarm_attachment/skin_wl40_implement"] = "skin_wl40_implement_primary"},
})
check("falls through to the keyword tier", n == "wl40_bucket", tostring(n) .. " / " .. c .. " nodes")

print("5. a blanked partOrigin must never win")
-- slotSystem resets slot options to the empty STRING, which is truthy in Lua.
n, c = resolve({nodes = build({
  MACHINE,
  {count = 30, origin = "", path = "/wl40/wl40_liftarm_attachment/whatever"},
  {count = 40, origin = "wl40_bucket",
   path = "/wl40/wl40_liftarm_attachment/wl40_bucket"},
})})
check("skips the blanks and finds the bucket", n == "wl40_bucket", tostring(n) .. " / " .. c .. " nodes")

print("6. an ordinary car resolves nothing")
n, c = resolve({nodes = build({
  {count = 200, origin = "pickup", path = "/pickup"},
  {count = 40, origin = "pickup_bed", path = "/pickup/pickup_bed"},
  {count = 20, origin = "pickup_engine_i6", path = "/pickup/pickup_engine/pickup_engine_i6"},
})})
check("no implement", n == nil, tostring(n))

print("7. a WL-40 tug config with no attachment resolves nothing")
n, c = resolve({nodes = build({MACHINE, LIFTARM})})
check("no implement", n == nil, tostring(n))

print("8. an implement with too few nodes is rejected, not half-accepted")
n, c = resolve({nodes = build({
  MACHINE,
  {count = 2, origin = "wl40_bucket",
   path = "/wl40/wl40_liftarm_attachment/wl40_bucket"},
})})
check("no implement", n == nil, tostring(n))

print("9. activeParts tier still works when it is the only signal")
n, c = resolve({
  nodes = build({
    MACHINE,
    {count = 40, origin = "wl40_mystery_tool", path = "/wl40/wl40_tool"},
  }),
  activeParts = {["/wl40/wl40_liftarm_attachment/wl40_mystery_tool"] = "wl40_mystery_tool"},
})
check("resolves via activeParts", n == "wl40_mystery_tool", tostring(n) .. " / " .. c .. " nodes")


-- =========================================================================================
-- Sample-point geometry.
--
-- The five sample cids are not just "some nodes on the implement": the GE side derives the
-- implement's HEADING from edgeL -> edgeR, so that pair has to be the genuine lateral
-- extremes of the leading edge. The first version scored "most forward and lowest" per
-- side, which on a bucket with centre teeth put both picks within centimetres of the
-- centreline; the heading then came from a ~10 cm baseline of two jittering nodes and the
-- scanner snapped between left and right on a parked machine.
-- =========================================================================================

local function readNum(name)
  local fh = assert(io.open(SRC, "r")); local b = fh:read("*a"); fh:close()
  local val = tonumber(b:match(name .. "%s*=%s*([%d%.]+)"))
  assert(val, "could not parse " .. name)
  return val
end

local IMPL_EDGE_BAND  = readNum("IMPL_EDGE_BAND")
local IMPL_FLOOR_BAND = readNum("IMPL_FLOOR_BAND")

-- Mirrors the three-stage selection in resolveImplementNodes: fore/aft band, then the FLOOR
-- band, then the lateral extremes within what is left.
--
-- `legacy` replays the version without the floor constraint and with the centre pick taken
-- nearest the vehicle centreline, so the defects it caused are demonstrated rather than
-- merely asserted away.
local function samplePoints(nodes, legacy)
  local minF, maxF = math.huge, -math.huge
  local minZ, maxZ = math.huge, -math.huge
  for _, p in ipairs(nodes) do
    if p.y < minF then minF = p.y end
    if p.y > maxF then maxF = p.y end
    if p.z < minZ then minZ = p.z end
    if p.z > maxZ then maxZ = p.z end
  end
  local span = maxF - minF
  local frontCut, rearCut = maxF - span * IMPL_EDGE_BAND, minF + span * IMPL_EDGE_BAND
  local floorCut = minZ + (maxZ - minZ) * IMPL_FLOOR_BAND

  local function bandPicks(test)
    local function gather(lowOnly)
      local out = {}
      for _, p in ipairs(nodes) do
        if test(p.y) and ((not lowOnly) or p.z <= floorCut) then out[#out + 1] = p end
      end
      return out
    end
    local pool = gather(not legacy)
    if #pool < 2 then pool = gather(false) end

    local l, r = nil, nil
    local lx, rx = -math.huge, math.huge
    for _, p in ipairs(pool) do
      if p.x > lx then l, lx = p, p.x end
      if p.x < rx then r, rx = p, p.x end
    end
    if not (l and r) then return nil end

    -- New: nearest the midpoint of that pair. Legacy: nearest the vehicle centreline.
    local aim = legacy and 0.0 or ((lx + rx) * 0.5)
    local c, cd, cz = nil, math.huge, math.huge
    for _, p in ipairs(pool) do
      local d = math.abs(p.x - aim)
      if d < cd or (d == cd and p.z < cz) then c, cd, cz = p, d, p.z end
    end
    return l, r, c, (lx - rx)
  end

  local eL, eR, eC, width = bandPicks(function(f) return f >= frontCut end)
  local hL, hR = bandPicks(function(f) return f <= rearCut end)
  return eL, eR, eC, hL, hR, width
end

-- The floor-plane pitch the tilt readout and the entry gate both measure: edgeL/edgeR
-- midpoint to heelL/heelR midpoint. Both halves of the mod use this same two-point axis.
local function floorPitchDeg(eL, eR, hL, hR)
  local ay, az = (eL.y + eR.y) * 0.5, (eL.z + eR.z) * 0.5
  local by, bz = (hL.y + hR.y) * 0.5, (hL.z + hR.z) * 0.5
  local len = math.sqrt((ay - by) ^ 2 + (az - bz) ^ 2)
  if len < 1e-6 then return 0 end
  return math.deg(math.asin(math.max(-1, math.min(1, (az - bz) / len))))
end

-- The old scoring, kept so the regression is demonstrated rather than asserted.
local function oldEdgePair(nodes)
  local function pick(side)
    local best, bs = nil, -math.huge
    for _, p in ipairs(nodes) do
      if side(p.x) then
        local s = p.y * 2.0 - p.z
        if s > bs then best, bs = p, s end
      end
    end
    return best
  end
  local l = pick(function(x) return x > 0.05 end)
  local r = pick(function(x) return x < -0.05 end)
  if not (l and r) then return 0 end
  return l.x - r.x
end

-- A 1.8 m bucket with a row of centre teeth that reach slightly further forward and lower
-- than the corners, which is what a real bucket looks like.
local bucket = {}
for i = -9, 9 do
  bucket[#bucket + 1] = {x = i * 0.1, y = 1.20, z = 0.00}   -- cutting edge
end
for _, tx in ipairs({-0.06, 0.06}) do
  bucket[#bucket + 1] = {x = tx, y = 1.26, z = -0.03}       -- centre teeth: more forward, lower
end
for i = -9, 9 do
  bucket[#bucket + 1] = {x = i * 0.1, y = 0.10, z = 0.35}   -- heel / back of the bowl
end

print("10. sample geometry: the leading-edge pair must be the LATERAL extremes")
local eL, eR, eC, hL, hR, width = samplePoints(bucket)
check("edgeL is the far left corner", math.abs(eL.x - 0.9) < 1e-9, string.format("x=%.2f", eL.x))
check("edgeR is the far right corner", math.abs(eR.x + 0.9) < 1e-9, string.format("x=%.2f", eR.x))
check("edge baseline spans the bucket", width > 1.5, string.format("%.2f m", width))
check("edgeC sits on the centreline", math.abs(eC.x) < 0.07, string.format("x=%.2f", eC.x))
check("heel pair is also spread", (hL.x - hR.x) > 1.5, string.format("%.2f m", hL.x - hR.x))

print("11. THE SNAP BUG: the old scoring collapsed that pair onto the centreline")
local oldWidth = oldEdgePair(bucket)
check(
  "old scoring produced a uselessly short baseline",
  oldWidth < 0.2,
  string.format("%.2f m - heading from this is pure node jitter", oldWidth)
)
check(
  "new selection is more than 5x wider",
  width > oldWidth * 5,
  string.format("%.2f m vs %.2f m", width, oldWidth)
)

print("12. a narrow implement (forks) still yields a usable baseline")
local forks = {}
for _, fx in ipairs({-0.55, 0.55}) do
  for i = 0, 4 do
    forks[#forks + 1] = {x = fx, y = 1.4 - i * 0.05, z = 0.0}   -- two tines
    forks[#forks + 1] = {x = fx, y = 0.1 + i * 0.05, z = 0.30}  -- carriage
  end
end
local _, _, _, _, _, fw = samplePoints(forks)
check("fork tine spacing is used as the baseline", fw > 1.0, string.format("%.2f m", fw))

-- =========================================================================================
-- The floor plane.
--
-- The heel picks are the lateral extremes of the REAR band, and until the floor constraint
-- was added there was no height condition on them at all. On the block-handler forks that
-- lands them on the top of the backplate, so edgeMid -> heelMid is a diagonal across the
-- attachment rather than the plane the load rides on -- and the tilt readout, which is that
-- axis's world pitch, then reported level at an angle the tines cannot enter anything at.
-- =========================================================================================

-- Block-handler forks: two tines lying flat at z = 0, a tall backplate behind them, and a
-- slightly wider rail across the top of it -- which is what makes the old unconstrained pick
-- land up there unambiguously.
local blockForks = {}
for _, fx in ipairs({-0.55, 0.55}) do
  for _, fy in ipairs({0.0, 0.3, 0.6, 0.9, 1.2}) do
    blockForks[#blockForks + 1] = {x = fx, y = fy, z = 0.00}          -- tine underside
    blockForks[#blockForks + 1] = {x = fx * 0.91, y = fy, z = 0.08}   -- tine top, chamfered in
  end
end
for _, bz in ipairs({0.00, 0.05, 0.55, 0.95, 1.40}) do
  for _, bx in ipairs({-0.60, 0.60}) do
    blockForks[#blockForks + 1] = {x = bx, y = -0.05, z = bz}         -- backplate
  end
end
for _, bx in ipairs({-0.62, 0.62}) do
  blockForks[#blockForks + 1] = {x = bx, y = -0.05, z = 1.40}         -- top rail
end

print("13. TALL BACKPLATE: the heel picks must land on the floor, not on top of the plate")
local bL, bR, bC, bhL, bhR = samplePoints(blockForks)
local newPitch = floorPitchDeg(bL, bR, bhL, bhR)
check("heel picks are in the low band", math.max(bhL.z, bhR.z) < 0.10,
  string.format("heel z %.2f / %.2f", bhL.z, bhR.z))
check("resolved axis is the tine plane", math.abs(newPitch) < 3.0,
  string.format("%+.1f deg from level", newPitch))

local oL, oR, _, ohL, ohR = samplePoints(blockForks, true)
local oldPitch = floorPitchDeg(oL, oR, ohL, ohR)
check("the OLD unconstrained pick was on the backplate top",
  math.min(ohL.z, ohR.z) > 1.0, string.format("heel z %.2f / %.2f", ohL.z, ohR.z))
check("...and its axis was more than 30 degrees off the tine plane",
  math.abs(oldPitch - newPitch) > 30.0,
  string.format("%+.1f deg vs %+.1f deg - 'level' meant %.0f degrees nose-up",
    oldPitch, newPitch, math.abs(oldPitch)))

print("14. edgeC on an implement with nothing in the middle")
-- A side-shifted carriage is what separates the two rules: with the implement centred they
-- pick the same node, so the centreline rule looks correct right up until it isn't. Tines at
-- x = 0.20 and 1.30, i.e. the whole attachment offset to the driver's left.
local offsetForks = {}
for _, fx in ipairs({0.20, 1.30}) do
  for _, fy in ipairs({0.0, 0.4, 0.8, 1.2}) do
    offsetForks[#offsetForks + 1] = {x = fx, y = fy, z = 0.00}
  end
end
for _, rx in ipairs({0.70, 0.80}) do   -- ribs across the middle of the carriage
  offsetForks[#offsetForks + 1] = {x = rx, y = 1.05, z = 0.02}
end
for _, bx in ipairs({0.15, 1.35}) do
  offsetForks[#offsetForks + 1] = {x = bx, y = -0.05, z = 0.00}
  offsetForks[#offsetForks + 1] = {x = bx, y = -0.05, z = 0.90}
end
local sL, sR, sC = samplePoints(offsetForks)
local trueMid = (sL.x + sR.x) * 0.5
local oldSL, oldSR, oldSC = samplePoints(offsetForks, true)
check("new edgeC sits near the edge pair's midpoint", math.abs(sC.x - trueMid) < 0.10,
  string.format("x %.2f vs midpoint %.2f", sC.x, trueMid))
check("old edgeC was dragged toward the vehicle centreline",
  math.abs(oldSC.x - trueMid) > 0.40,
  string.format("x %.2f vs midpoint %.2f", oldSC.x, trueMid))
check("the old three-point mean carried the bias into the docking origin",
  math.abs((oldSL.x + oldSC.x + oldSR.x) / 3 - trueMid) > 0.15,
  string.format("%.2f vs %.2f", (oldSL.x + oldSC.x + oldSR.x) / 3, trueMid))
check("the origin is now the edge pair's midpoint, so it carries none of it",
  math.abs(((sL.x + sR.x) * 0.5) - trueMid) < 1e-9,
  "edgeC is a contact point and no longer feeds the origin at all")

-- =========================================================================================
-- The entry gate.
--
-- Every other axis can be nulled perfectly and the tines still not go in: a tilted implement
-- climbs through the band's whole thickness after a few centimetres of travel and hits the
-- far side. Mirrors resolveEntry in implementProximity.lua.
-- =========================================================================================

local PROX = "bng_mod/lua/ge/extensions/implementProximity.lua"
local function readProxNum(name)
  local fh = assert(io.open(PROX, "r"), "cannot open " .. PROX)
  local b = fh:read("*a"); fh:close()
  local val = tonumber(b:match(name .. "%s*=%s*([%d%.]+)"))
  assert(val, "could not parse " .. name)
  return val
end
local ENTRY_MIN   = readProxNum("IMPL_ENTRY_MIN_DEPTH_M")
local ENTRY_EXIT  = readProxNum("IMPL_ENTRY_EXIT_DEPTH_M")
local ENTRY_LEVEL = readProxNum("IMPL_ENTRY_LEVEL_DEG")

local entryOK = false
local function resolveEntry(theta, L, T)
  local depth
  if math.abs(theta) < ENTRY_LEVEL then
    depth = L
  else
    depth = math.min(L, T / math.sin(math.rad(math.abs(theta))))
  end
  local threshold = entryOK and ENTRY_EXIT or ENTRY_MIN
  entryOK = depth >= threshold
  return depth, entryOK
end

print("15. entry depth: how far the tines actually go in")
entryOK = false
local d1, ok1 = resolveEntry(0.0, 1.20, 0.11)
check("level tines enter their full length", math.abs(d1 - 1.20) < 1e-9 and ok1,
  string.format("%.2f m", d1))
entryOK = false
local d2, ok2 = resolveEntry(36.0, 1.20, 0.11)
check("36 degrees into a 0.11 m band is rejected", (not ok2) and d2 < 0.20,
  string.format("%.3f m - the tine is out the far side after 19 cm", d2))
entryOK = false
local d3 = resolveEntry(5.0, 1.20, 0.11)
check("depth never exceeds the tine length, however shallow the angle",
  math.abs(d3 - 1.20) < 1e-9,
  string.format("%.2f m; the raw trig says %.2f m, which is more tine than exists",
    d3, 0.11 / math.sin(math.rad(5.0))))
entryOK = false
local d4, ok4 = resolveEntry(8.0, 1.20, 0.30)
check("a thicker band is enterable at the same angle", ok4, string.format("%.2f m", d4))

-- Hysteresis, the same shape as the slam gate's: a machine idling on its suspension at the
-- threshold must not chatter, because this drives a one-shot earcon.
local function wobble200(startEnterable)
  entryOK = false
  if startEnterable then resolveEntry(0.0, 1.20, 1.20) end  -- decisively enterable first
  local flips, prev = 0, entryOK
  for i = 1, 200 do
    -- Band thickness jittering so the computed depth lands between the two thresholds --
    -- exactly where a version without hysteresis would toggle every tick.
    local theta = 10.0
    local T = ((ENTRY_MIN + ENTRY_EXIT) * 0.5 + 0.002 * math.sin(i * 0.7))
              * math.sin(math.rad(theta))
    local _, ok = resolveEntry(theta, 1.20, T)
    if ok ~= prev then flips = flips + 1; prev = ok end
  end
  return flips, prev
end
local flipsUp, heldUp = wobble200(true)
check("no chatter across 200 ticks of threshold wobble", flipsUp == 0 and heldUp,
  string.format("%d state changes, held enterable", flipsUp))
local flipsDown, heldDown = wobble200(false)
check("...and the gate is equally sticky from the other side",
  flipsDown == 0 and not heldDown,
  string.format("%d state changes, held closed", flipsDown))

-- =========================================================================================
-- FALSE POSITIVES: vehicles that must never be thought to have an implement.
--
-- These matter more than they look. The GE side's ONLY test for "does this vehicle have a
-- bucket" is whether the vehicle VM pushed it a cid list, so a resolution here is what makes
-- the docking instrument measure from a seat frame or a tow ball, makes the scanner aim from
-- it, and locks out ramp mode -- whose whole premise is a vehicle with NO implement, i.e.
-- exactly the ordinary cars these names come from. Every part name below is real, taken out
-- of the stock 0.39 vehicle jbeams.

print("16. bucket SEATS on an ordinary car")
n, c = resolve({nodes = build({
  {count = 200, origin = "pickup", path = "/pickup"},
  {count = 30, origin = "pickup_seat_F_bucket",
   path = "/pickup/pickup_interior/pickup_seat_F_bucket"},
})}, {actuated = false})
check("no implement", n == nil, tostring(n))
-- ...and show what is doing the work: the name matching alone still accepts it, so the
-- actuation gate is the whole defence here, not a nicety on top of one.
n, c = resolve({nodes = build({
  {count = 200, origin = "pickup", path = "/pickup"},
  {count = 30, origin = "pickup_seat_F_bucket",
   path = "/pickup/pickup_interior/pickup_seat_F_bucket"},
})}, {actuated = true})
check("the name tier on its own would have taken the seat", n == "pickup_seat_F_bucket",
  tostring(n) .. " - a car has no lift or tilt rams, which is why this never gets asked")

print("17. roof scoops and a rollback loading ramp, on a machine that HAS rams")
-- us_semi's rollback bed is moved by cylinders named tilt1/tilt2, so this one clears the
-- actuation gate on its own merits. The word boundary is the only thing left standing
-- between "us_semi_ramplow" and a tow truck that believes it has a plow.
local ROLLBACK = {
  {count = 300, origin = "us_semi", path = "/us_semi"},
  {count = 40, origin = "us_semi_ramplow", path = "/us_semi/us_semi_rollback/us_semi_ramplow"},
  {count = 20, origin = "covet_roofscoop", path = "/covet/covet_roofscoop"},
}
n, c = resolve({nodes = build(ROLLBACK)}, {actuated = true})
check("no implement", n == nil, tostring(n))
n, c = resolve({nodes = build(ROLLBACK)}, {actuated = true, substring = true})
check("the OLD substring matcher took the loading ramp", n == "us_semi_ramplow",
  tostring(n) .. " - 'plow' inside 'ramplow'")

print("18. a tow hitch in an attachment slot, on a machine that HAS rams")
-- Every stock car carries a towhitch_receiver_attachment slot and the fitted hitch is five
-- nodes -- over the four-node floor, with a partPath the slot tier matches outright.
local HITCH = {
  {count = 300, origin = "van", path = "/van"},
  {count = 5, origin = "tow_hitch_drop_2",
   path = "/van/van_frame/towhitch_receiver_attachment/tow_hitch_drop_2"},
}
n, c = resolve({nodes = build(HITCH)}, {actuated = true})
check("no implement", n == nil, tostring(n))
n, c = resolve({nodes = build(HITCH)}, {actuated = true, firstMatch = true, substring = true})
check("the OLD slot tier made five nodes of tow ball the implement",
  n == "tow_hitch_drop_2", tostring(n) .. " / " .. c .. " nodes")

print("19. two filled attachment slots: the answer must be the same every run")
-- A loader can wear a hitch plate as well as a bucket. The old tier took whichever node
-- pairs() reached first, so which of the two became "the implement" was luck of the table
-- layout -- and it could differ between sessions on identical geometry.
local BOTH = {
  MACHINE, LIFTARM,
  {count = 40, origin = "wl40_bucket",
   path = "/wl40/wl40_loader_liftarm/wl40_liftarm_attachment/wl40_bucket"},
  {count = 5, origin = "tow_hitch_drop_2",
   path = "/wl40/wl40_frame_R/towhitch_receiver_attachment/tow_hitch_drop_2"},
}
local stable = true
for _ = 1, 50 do
  local nn = resolve({nodes = build(BOTH)}, {actuated = true})
  if nn ~= "wl40_bucket" then stable = false end
end
check("the bucket wins, 50 runs out of 50", stable, "named candidates beat unnamed ones")

print("20. the camel hump must survive the boundary rule")
-- The obvious way to write the boundary test -- lower the string, then require a separator
-- before the keyword -- rejects the WL-40's own block handler forks, because lowering
-- "blockForks" puts "fork" after a "k". That would swap one silent failure for another.
local function naiveBoundary(s, words)
  local low = s:lower()
  for _, w in ipairs(words) do
    local i = low:find(w, 1, true)
    if i and (i == 1 or low:sub(i - 1, i - 1):match("%W")) then return true end
  end
  return false
end
check("blockForks still reads as an implement",
  matchesAnyWord("wl40_liftarm_blockForks", IMPL_PART_WORDS),
  "matched through the capital F")
check("the lowercase-only rule would have lost it",
  not naiveBoundary("wl40_liftarm_blockForks", IMPL_PART_WORDS),
  "'fork' after a 'k' once lowered")
check("and it still rejects ramplow / roofscoop",
  not matchesAnyWord("us_semi_ramplow", IMPL_PART_WORDS)
    and not matchesAnyWord("covet_roofscoop", IMPL_PART_WORDS)
    and not matchesAnyWord("sunburst2_hoodscoop", IMPL_PART_WORDS),
  "no boundary, no hump")
check("...while keeping the separated forms that really are implements",
  matchesAnyWord("wl40_bucket", IMPL_PART_WORDS)
    and matchesAnyWord("bucketTipRail", IMPL_PART_WORDS)
    and matchesAnyWord("snow_plow_blade", IMPL_PART_WORDS),
  "underscore, start of string, camel hump")

print()
if failures > 0 then
  print(failures .. " FAILURE(S)")
  os.exit(1)
end
print("all checks passed")
