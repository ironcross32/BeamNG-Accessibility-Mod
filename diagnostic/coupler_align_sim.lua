-- Replay vehicleScanner.lua's coupler ALIGN geometry against the real T-series / tanker
-- measurements, plus the greps that keep the deleted auto-coupling path deleted.
--
--     lua diagnostic/coupler_align_sim.lua
--
-- The align is a teleport, so every way of getting it wrong produces a vehicle sitting
-- somewhere plausible with a confident announcement attached. There is no symptom from the
-- seat that says "the standoff was computed against the wrong axis"; there is only a truck
-- that will not couple, which is exactly how the fifth-wheel path shipped broken for the
-- whole life of the feature.
--
-- Several checks are NEGATIVE: they assert the previous implementation gets a specific
-- wrong answer, so the check cannot pass for free if someone later "simplifies" the rule
-- back to it. The wrong answers below are the ones measured in game, not invented.
--
-- Tuning constants are parsed out of the source rather than copied, so retuning there
-- cannot silently invalidate these checks. Only the *logic* is duplicated here.

local SCANNER_SRC = "bng_mod/lua/ge/extensions/vehicleScanner.lua"
local PROTO_SRC   = "bng_mod/lua/vehicle/protocols/796F6C6F313035.lua"
local GEOSIM_SRC  = "diagnostic/vehicle_geometry_sim.lua"
local AUDIO_SRC   = "audio.py"

local function slurp(path)
  local fh = assert(io.open(path, "r"), "cannot open " .. path)
  local body = fh:read("*a")
  fh:close()
  return body
end

local function readConstFrom(path, name)
  local body = slurp(path)
  local val = body:match("\nlocal " .. name .. "%s*=%s*([%-%d%.]+)")
  assert(val, "could not find " .. name .. " in " .. path)
  return tonumber(val)
end

local FW_ALIGN_CLEARANCE_M = readConstFrom(SCANNER_SRC, "FW_ALIGN_CLEARANCE_M")
local ALIGN_OVERHANG_MAX_M = readConstFrom(SCANNER_SRC, "ALIGN_OVERHANG_MAX_M")
local ALIGN_DISPLACE_SAY_M = readConstFrom(SCANNER_SRC, "ALIGN_DISPLACE_SAY_M")
local COUPLER_RANGE_M      = readConstFrom(SCANNER_SRC, "COUPLER_RANGE_M")

-- =================================================================================================
--  Minimal vector arithmetic (the sim runs under plain Lua, with no engine globals)
-- =================================================================================================

local V = {}
V.__index = V
local function vec(x, y, z) return setmetatable({x = x, y = y, z = z or 0}, V) end
function V.__add(a, b) return vec(a.x + b.x, a.y + b.y, a.z + b.z) end
function V.__sub(a, b) return vec(a.x - b.x, a.y - b.y, a.z - b.z) end
function V.__unm(a) return vec(-a.x, -a.y, -a.z) end
function V.__mul(a, s) return vec(a.x * s, a.y * s, a.z * s) end
function V:dot(b) return self.x * b.x + self.y * b.y + self.z * b.z end
function V:length() return math.sqrt(self:dot(self)) end
function V:normalized()
  local l = self:length()
  if l < 1e-9 then return vec(0, 0, 0) end
  return vec(self.x / l, self.y / l, self.z / l)
end
function V:cross(b)
  return vec(self.y * b.z - self.z * b.y,
             self.z * b.x - self.x * b.z,
             self.x * b.y - self.y * b.x)
end
local function flat(a) return vec(a.x, a.y, 0) end

-- =================================================================================================
--  Ground truth, measured in game on us_semi (t83_longhaul) + tanker (diesel_2)
-- =================================================================================================

-- Fifth wheel offset from the truck's reference node, in the truck's own frame.
-- Negative forward: the plate sits BEHIND the reference node.
local FW_FWD          = -2.726
local FW_LAT          =  0.434   -- off its own centreline; the whole point of the lateral fix
-- Body reach behind the fifth wheel, and the trailer's reach ahead of its king pin.
local TRUCK_REAR_OH   =  1.680
local TANKER_FRONT_OH =  0.866
-- Oriented-box half extents (spawn.lua's isIntersecting is a box test, not a body test).
local TRUCK_HALF_LEN  =  4.907
local TANKER_HALF_LEN =  6.425
-- King pin, forward of the tanker's reference node.
local KINGPIN_FWD     =  5.221
-- What the shipped build actually did, measured twice, identically.
local SHIPPED_SHIFT_M =  4.615

-- =================================================================================================
--  The align under test, and the form it replaced
-- =================================================================================================

-- Returns the truck's reference-node position for a given trailer pose.
--   couplerPos  world position of the king pin
--   awayDir     unit vector pointing from the trailer out to where the truck goes
local function alignRefPos(couplerPos, awayDir, opts)
  local gap
  if opts.zeroGap then
    gap = 0                                    -- the shipped fifth-wheel form
  else
    gap = math.min(opts.rearOH, ALIGN_OVERHANG_MAX_M)
        + math.min(opts.frontOH, ALIGN_OVERHANG_MAX_M)
        + FW_ALIGN_CLEARANCE_M
  end
  local alignDist = math.abs(opts.fwdOffset) + gap
  local pos = couplerPos + awayDir * alignDist
  if opts.lateral then
    -- quatFromDir(awayDir, up) sets the truck's forward TO awayDir, so its left is
    -- vec3(0,0,1):cross(awayDir) and the coupler's own left-offset subtracts.
    local alignLeft = vec(0, 0, 1):cross(awayDir)
    pos = pos - alignLeft * opts.lateral
  end
  return pos, alignDist, alignDist - math.abs(opts.fwdOffset)
end

-- Where the coupler itself ends up, given the truck reference position and heading.
local function couplerWorld(refPos, awayDir, fwdOffset, latOffset)
  local left = vec(0, 0, 1):cross(awayDir)
  return refPos + awayDir * fwdOffset + left * latOffset
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

print(string.format(
  "tuning: clearance %.2f m  overhang cap %.1f m  displace-say %.2f m  coupler range %.2f m",
  FW_ALIGN_CLEARANCE_M, ALIGN_OVERHANG_MAX_M, ALIGN_DISPLACE_SAY_M, COUPLER_RANGE_M))
print("")

-- -------------------------------------------------------------------------------------------------
print("0. the standoff clears the oriented-box test spawn.lua actually applies")
do
  -- The boxes are what matters, not the bodies: a fifth wheel works precisely BECAUSE the
  -- trailer nose overhangs the tractor frame and they never touch. Only the align cares,
  -- because only the align goes through isIntersecting.
  local kingpin = vec(0, 0, 0)
  local away = vec(0, -1, 0)
  local _, _, gap = alignRefPos(kingpin, away, {
    fwdOffset = FW_FWD, rearOH = TRUCK_REAR_OH, frontOH = TANKER_FRONT_OH, lateral = FW_LAT})

  local touchAt = TRUCK_REAR_OH + TANKER_FRONT_OH
  check("boxes touch at the measured 2.546 m", approx(touchAt, 2.546, 0.001),
    string.format("%.3f m", touchAt))
  check("computed gap clears it", gap > touchAt,
    string.format("gap %.3f m vs touch %.3f m", gap, touchAt))
  check("margin is real, not a rounding hair", (gap - touchAt) >= 0.5,
    string.format("%.3f m of margin", gap - touchAt))

  -- NEGATIVE CONTROL: the shipped zero-gap form overlaps, which is what made safeTeleport
  -- relocate the vehicle instead of placing it.
  local _, _, badGap = alignRefPos(kingpin, away, {
    fwdOffset = FW_FWD, zeroGap = true, lateral = FW_LAT})
  check("NEGATIVE: shipped zero-gap form overlaps the boxes", badGap < touchAt,
    string.format("gap %.3f m vs touch %.3f m -- overlap %.3f m", badGap, touchAt,
      touchAt - badGap))

  -- And the ball-hitch fixed gap would ALSO have overlapped for this pair, which is why the
  -- standoff has to be derived rather than borrowed.
  check("NEGATIVE: a fixed 1.5 m gap would also overlap", 1.5 < touchAt,
    string.format("1.500 m vs touch %.3f m", touchAt))
end

-- -------------------------------------------------------------------------------------------------
print("1. the COUPLER lands on the target axis, at every trailer heading")
do
  local kingpin = vec(12.5, -3.25, 0)
  local worstGood, worstNone, worstFlip = 0, 0, 0

  for deg = 0, 350, 10 do
    local a = math.rad(deg)
    local away = vec(math.cos(a), math.sin(a), 0):normalized()
    local axisLeft = vec(0, 0, 1):cross(away)

    -- Corrected form.
    local refPos = alignRefPos(kingpin, away, {
      fwdOffset = FW_FWD, rearOH = TRUCK_REAR_OH, frontOH = TANKER_FRONT_OH, lateral = FW_LAT})
    local cw = couplerWorld(refPos, away, FW_FWD, FW_LAT)
    worstGood = math.max(worstGood, math.abs((cw - kingpin):dot(axisLeft)))

    -- NEGATIVE CONTROL A: no compensation at all (the shipped form).
    local refNone = alignRefPos(kingpin, away, {
      fwdOffset = FW_FWD, rearOH = TRUCK_REAR_OH, frontOH = TANKER_FRONT_OH})
    local cwNone = couplerWorld(refNone, away, FW_FWD, FW_LAT)
    worstNone = math.max(worstNone, math.abs((cwNone - kingpin):dot(axisLeft)))

    -- NEGATIVE CONTROL B: the sign flipped. This is the one that matters -- quatFromDir's
    -- convention is a native call and is not derivable from source, and a sign error here
    -- looks identical from the seat to no correction at all.
    local refFlip = alignRefPos(kingpin, away, {
      fwdOffset = FW_FWD, rearOH = TRUCK_REAR_OH, frontOH = TANKER_FRONT_OH,
      lateral = -FW_LAT})
    local cwFlip = couplerWorld(refFlip, away, FW_FWD, FW_LAT)
    worstFlip = math.max(worstFlip, math.abs((cwFlip - kingpin):dot(axisLeft)))
  end

  -- The real attach radius is couplerRadius * 0.8 = 0.08 m, so "on the axis" has to mean
  -- centimetres, not decimetres.
  check("corrected form lands within 1 cm at every heading", worstGood < 0.01,
    string.format("worst %.4f m", worstGood))
  check("NEGATIVE: no compensation leaves the measured 0.434 m",
    approx(worstNone, FW_LAT, 0.001), string.format("worst %.4f m", worstNone))
  check("NEGATIVE: flipped sign DOUBLES the error rather than removing it",
    approx(worstFlip, FW_LAT * 2, 0.001), string.format("worst %.4f m", worstFlip))
  check("both wrong forms exceed the 0.08 m attach radius",
    worstNone > 0.08 and worstFlip > 0.08)
end

-- -------------------------------------------------------------------------------------------------
print("1b. the coupler is chosen by ROLE, not by tag")
do
  -- `fifthwheel_v2` is a coupling STANDARD carried by both halves of the joint, so it
  -- cannot say which half a node is. A log_trailer has both: its own king pin at the front
  -- and a fifth wheel at the back for pulling a second trailer. The measured jbeam offsets,
  -- read out of a running game.
  local LOG_TRAILER = {
    {name = "fwk2",   couplerTag = "fifthwheel_v2", fwd = -3.075},  -- its own plate, at the REAR
    {name = "kingp3", tag        = "fifthwheel_v2", fwd =  7.193},  -- the king pin, at the FRONT
  }
  local TANKER = {
    {name = "kingp3", tag = "fifthwheel_v2", fwd = 5.221},          -- no couplerTag node at all
  }

  -- The shipped ladder, reproduced exactly, including its strict `>`.
  local function tagLadder(nodes)
    local best, bpri = nil, 0
    for _, nd in ipairs(nodes) do
      local p = 0
      if nd.couplerTag then
        p = 1
        if nd.couplerTag:lower():find("fifthwheel") then p = 3 end
      end
      if p == 0 and nd.tag and nd.tag:lower():find("fifthwheel") then p = 3 end
      if p > bpri then best = nd; bpri = p end
    end
    return best
  end
  -- The rule now in the mod: the node the couplings/kingpin controller names wins outright.
  local function roleRule(nodes, roleName)
    local best = tagLadder(nodes)
    for _, nd in ipairs(nodes) do
      if roleName and nd.name == roleName then best = nd end
    end
    return best
  end

  check("role rule takes the king pin on a log trailer",
    roleRule(LOG_TRAILER, "kingp3").name == "kingp3")
  check("...and still takes it on a tanker, which has only one candidate",
    roleRule(TANKER, "kingp3").name == "kingp3")
  -- NEGATIVE CONTROL: the tag ladder scores both log-trailer nodes 3, so `pairs()` order
  -- decides. In game it handed back the rear plate first and the align aimed the truck
  -- 10.27 m behind the real king pin -- i.e. reversed it into the trailer.
  check("NEGATIVE: the tag ladder takes the trailer's OWN fifth wheel",
    tagLadder(LOG_TRAILER).name == "fwk2")
  check("NEGATIVE: ...which is 10.27 m from the king pin",
    approx(LOG_TRAILER[2].fwd - LOG_TRAILER[1].fwd, 10.268, 0.01),
    string.format("%.3f m of error", LOG_TRAILER[2].fwd - LOG_TRAILER[1].fwd))
  -- ...and the reason a tanker never showed the bug: nothing to be ambiguous about.
  check("NEGATIVE: the tanker cannot reveal it -- tag ladder agrees there",
    tagLadder(TANKER).name == "kingp3",
    "one candidate, so the tag ladder is accidentally right")

  -- A ball-hitch trailer has neither controller, so the ladder is left untouched.
  local BALL = {{name = "hitch", couplerTag = "tow_bar", fwd = 2.0}}
  check("with no coupling controller the ladder is unchanged",
    roleRule(BALL, nil).name == tagLadder(BALL).name)

  -- The same role split, spelled with two tags instead of one shared one. A tow_bar is the
  -- drawbar you hook up TO; a tow_hitch tows something else. Both ladders used to rank
  -- tow_hitch above tow_bar -- right for the truck, backwards for the trailer.
  local function ladderFor(nodes, towedEnd)
    local best, bpri = nil, 0
    for _, nd in ipairs(nodes) do
      local p, cl = 0, (nd.couplerTag or ""):lower()
      if nd.couplerTag then
        p = 1
        if towedEnd then
          if cl:find("tow_hitch") then p = 2 end
          if cl:find("fifthwheel") or cl == "tow_bar" then p = 3 end
        else
          if cl == "tow_bar" then p = 2 end
          if cl:find("fifthwheel") or cl:find("tow_hitch") then p = 3 end
        end
      end
      if p > bpri then best = nd; bpri = p end
    end
    return best
  end
  -- A trailer with its own drawbar AND a rear hitch for double-towing.
  local DOUBLE = {
    {name = "hitch_rear", couplerTag = "tow_hitch", fwd = -4.0},
    {name = "drawbar",    couplerTag = "tow_bar",   fwd =  3.0},
  }
  check("target side takes the drawbar, not the rear hitch",
    ladderFor(DOUBLE, true).name == "drawbar")
  check("NEGATIVE: the old (towing-end) ranking takes the rear hitch",
    ladderFor(DOUBLE, false).name == "hitch_rear",
    "7 m of error, the ball-hitch spelling of the log-trailer bug")
  -- The truck keeps the towing-end ranking: its hitch, never a drawbar it happens to carry.
  local TRUCK = {
    {name = "ball",  couplerTag = "tow_hitch", fwd = -2.5},
    {name = "eye",   couplerTag = "tow_bar",   fwd =  2.5},
  }
  check("player side still takes the hitch", ladderFor(TRUCK, false).name == "ball")
  -- An ordinary single-purpose trailer is unaffected either way.
  local PLAIN = {{name = "drawbar", couplerTag = "tow_bar", fwd = 3.0}}
  check("a plain trailer resolves the same under both rankings",
    ladderFor(PLAIN, true).name == ladderFor(PLAIN, false).name)

  local body = slurp(SCANNER_SRC)
  check("the player chunk asks the fifthwheel controller for its node",
    body:find('couplings/fifthwheel", 1, true) and cc.fifthwheelNode', 1, true) ~= nil)
  check("the target chunk asks the kingpin controller for its node",
    body:find('couplings/kingpin", 1, true) and cc.kingpinNode', 1, true) ~= nil)
end

print("2. the lateral offset must be PROJECTED, not the raw world x")
do
  -- The field this replaced was `np.x`, the world-axis x of the node offset. It is the
  -- lateral offset only when the vehicle happens to be axis-aligned -- and nothing ever
  -- read it, so it sat in the protocol looking exactly like a usable number.
  local projSpread, rawSpread = {}, {}
  for deg = 0, 350, 10 do
    local a = math.rad(deg)
    local fwd = vec(math.cos(a), math.sin(a), 0):normalized()
    local left = vec(0, 0, 1):cross(fwd)
    -- The node's world-relative offset for a coupler at (FW_FWD, FW_LAT) in body frame.
    local np = fwd * FW_FWD + left * FW_LAT
    projSpread[#projSpread + 1] = np:dot(left)
    rawSpread[#rawSpread + 1] = np.x
  end
  local function spread(t)
    local lo, hi = math.huge, -math.huge
    for _, v in ipairs(t) do lo = math.min(lo, v); hi = math.max(hi, v) end
    return hi - lo
  end
  check("projected form is heading-invariant", spread(projSpread) < 1e-9,
    string.format("spread %.2e", spread(projSpread)))
  check("NEGATIVE: raw world x swings by more than the truck is wide",
    spread(rawSpread) > 5.0, string.format("spread %.3f m", spread(rawSpread)))
end

-- -------------------------------------------------------------------------------------------------
print("3. the coupler-tracking bearing must be flattened")
do
  -- Both plates sit about a metre up. Unflattened, any height difference between them is
  -- reported as horizontal steering error -- and the sign comes off an almost-vertical
  -- vector, so it flips per tick, in the last half metre of the manoeuvre.
  local function bearing(dx, dz, flatten)
    local playerFwd = vec(0, 1, 0)
    local rear = -playerFwd
    local to = vec(dx, -0.30, dz)     -- 0.30 m behind, dx across, dz up
    if flatten then rear = flat(rear):normalized(); to = flat(to):normalized()
    else rear = rear:normalized(); to = to:normalized() end
    local c = math.max(-1, math.min(1, rear:dot(to)))
    return math.deg(math.acos(c))
  end

  local flatB = bearing(0.0, 0.15, true)
  local rawB  = bearing(0.0, 0.15, false)
  check("flattened: dead astern reads ~0 despite 0.15 m of height", flatB < 1.0,
    string.format("%.2f deg", flatB))
  check("NEGATIVE: unflattened reports it as >20 deg of steering error", rawB > 20.0,
    string.format("%.2f deg", rawB))

  -- ...and the sign is unstable under a millimetre of lateral jitter.
  local function signOf(dx)
    local playerFwd = vec(0, 1, 0)
    local left = vec(0, 0, 1):cross(playerFwd)
    local to = vec(dx, -0.30, 0.15):normalized()
    return left:dot(to) < 0 and -1 or 1
  end
  check("NEGATIVE: sign flips across a 1 mm lateral wobble",
    signOf(0.0005) ~= signOf(-0.0005))
end

-- -------------------------------------------------------------------------------------------------
print("4. the overhang cap bounds a damaged vehicle without changing a healthy one")
do
  -- The overhang is a max over every node and a detached part stays in the cloud, so a
  -- wreck could otherwise ask to be parked in the next county. Over-estimating only parks
  -- you further back -- the safe direction -- so a loose ceiling is enough.
  local _, _, healthy = alignRefPos(vec(0, 0, 0), vec(0, -1, 0), {
    fwdOffset = FW_FWD, rearOH = TRUCK_REAR_OH, frontOH = TANKER_FRONT_OH, lateral = FW_LAT})
  local _, _, wrecked = alignRefPos(vec(0, 0, 0), vec(0, -1, 0), {
    fwdOffset = FW_FWD, rearOH = 400.0, frontOH = TANKER_FRONT_OH, lateral = FW_LAT})
  check("healthy pair is unaffected by the cap",
    approx(healthy, TRUCK_REAR_OH + TANKER_FRONT_OH + FW_ALIGN_CLEARANCE_M, 1e-9),
    string.format("%.3f m", healthy))
  check("a 400 m outlier is clamped", wrecked <= ALIGN_OVERHANG_MAX_M + TANKER_FRONT_OH
    + FW_ALIGN_CLEARANCE_M + 1e-9, string.format("%.3f m", wrecked))
  check("clamped standoff still errs LONG, never short", wrecked > healthy)
end

-- -------------------------------------------------------------------------------------------------
print("5. the displacement guard would have caught the shipped bug")
do
  check("measured shipped displacement exceeds the threshold",
    SHIPPED_SHIFT_M > ALIGN_DISPLACE_SAY_M,
    string.format("%.3f m vs %.2f m", SHIPPED_SHIFT_M, ALIGN_DISPLACE_SAY_M))
  -- There is no gradient to lean on: once isIntersecting fails, every retry candidate is
  -- built at getHalfExtents() * 2, so the search is for a spot clear of a box eight times
  -- the truck's volume. A near-miss is not a small displacement.
  check("threshold is well under half a vehicle length",
    ALIGN_DISPLACE_SAY_M < TRUCK_HALF_LEN,
    string.format("%.2f m vs %.3f m", ALIGN_DISPLACE_SAY_M, TRUCK_HALF_LEN))
end

-- =================================================================================================
--  Source greps. These are the checks that keep the deleted path deleted.
-- =================================================================================================

-- Comment lines are skipped throughout: the prose explaining why auto-coupling is gone has
-- to write `beamstate.activateAutoCoupling()` itself. Same rule vehicle_geometry_sim.lua
-- scenario 13 applies to its safeTeleport grep.
local function codeLines(path)
  local out = {}
  for line in slurp(path):gmatch("[^\n]*") do
    local trimmed = line:match("^%s*(.-)%s*$")
    if trimmed ~= "" and not trimmed:match("^%-%-") and not trimmed:match("^#") then
      out[#out + 1] = line
    end
  end
  return out
end

local function grepCode(path, needle)
  for _, line in ipairs(codeLines(path)) do
    if line:find(needle, 1, true) then return line end
  end
  return nil
end

print("5b. the placement is two-phase, and the landing check waits a tick")
do
  local body = slurp(SCANNER_SRC)
  -- safeTeleport converts the requested REFERENCE position to a box centre with the
  -- vehicle's current rotation and back with the new one, so a heading change lands the
  -- reference node off by (I - diffRot) * (centre - ref). Measured on a T-series: that
  -- offset is 0.658 m, a 37 degree turn misses by 0.3985 m, and re-issuing once the heading
  -- matches lands within 0.0175 m.
  local REF_TO_CENTRE = 0.658
  local function artifact(deg) return 2 * math.sin(math.rad(deg) / 2) * REF_TO_CENTRE end
  check("a 37 deg heading change predicts the measured miss",
    approx(artifact(37), 0.3985, 0.03), string.format("%.4f m predicted", artifact(37)))
  check("...and it exceeds the attach radius many times over", artifact(37) > 0.08 * 4)
  -- NEGATIVE CONTROL: this is exactly why the bug hid. The obvious test aligns from the
  -- pose the previous align left, i.e. already on the target heading, where it vanishes.
  check("NEGATIVE: aligning from the target heading shows nothing", artifact(0) < 0.001,
    string.format("%.6f m -- an already-square truck cannot reveal this", artifact(0)))

  check("the mod re-issues the placement after settling",
    body:find("st%.phase == 1") ~= nil and body:find("st%.phase = 2") ~= nil)
  check("...and only announces from phase 2",
    body:find('udpSend:send%(string%.format%("COUPLER_START:') ~= nil
      and body:find("local shifted = %(vec3%(landed%.x") ~= nil)
  check("...and the landing check reads the position on a LATER tick",
    body:find("ALIGN_SETTLE_S") ~= nil,
    "getPosition() does not reflect a teleport within the same frame")
  -- The pending placement must not survive the things that invalidate it.
  local cleared = select(2, body:gsub("_alignSettle%s*=%s*nil", ""))
  check("the pending placement is cleared on reset, switch, OFF and both exits",
    cleared >= 5, string.format("%d clear sites", cleared))
end

print("5c. a coupling run outlives the scanner toggle")
do
  local body = slurp(SCANNER_SRC)
  -- Observed on the first real coupling: aligned, scanner toggled off/on 4 s later, coupled
  -- 19 s after that -- with no homing tone for the reverse and no "Coupled" afterwards,
  -- because the OFF handler cleared the attach monitor and switching the scanner back on
  -- re-armed nothing. Silencing the scanner's periodic callouts is not a request to abandon
  -- a coupling in progress; it is what you do BECAUSE you are concentrating on one.
  -- Comment lines are stripped first: the prose explaining that these three SURVIVE has
  -- to name all three, and would otherwise trip every check below. Same rule scenario 6
  -- applies, and the reason it is spelled string.char(10) is that this file must not
  -- carry a newline escape through the tooling that generates it.
  local codeOnly = table.concat(codeLines(SCANNER_SRC), string.char(10))
  local offBlock = codeOnly:match('elseif cmd == "OFF" and isScanModeActive then(.-)elseif')
  check("the OFF handler exists", offBlock ~= nil)
  if offBlock then
    check("...and no longer clears the tracking", not offBlock:find("couplerTrackActive"))
    check("...nor the attach monitor", not offBlock:find("couplerAttachMonitor"))
    check("...nor a pending placement", not offBlock:find("_alignSettle"))
  end

  -- Tracking has to run above the scan-mode gate, or clearing the OFF handler achieves
  -- nothing: the block would simply never be reached with the scanner off.
  local gateAt  = body:find("if not isScanModeActive then return end", 1, true)
  local trackAt = body:find("-- 2d. Coupler tracking", 1, true)
  check("tracking sits ABOVE the scan-mode early return",
    trackAt ~= nil and gateAt ~= nil and trackAt < gateAt,
    string.format("track@%s gate@%s", tostring(trackAt), tostring(gateAt)))

  -- ...and Python must not kill the tone either, since the Lua feed is still live.
  local py = slurp("beamtel.py")
  check("Python guards the toggle with an active-run flag",
    py:find("if not scan_mode_active and not coupler_run_active:", 1, true) ~= nil)
  check("...and clears that flag only when the run really ends",
    select(2, py:gsub("coupler_run_active = False", "")) >= 2)
end

print("6. the auto-coupling path stays deleted")
do
  local scannerHits = codeLines(SCANNER_SRC)
  check("grep found real source to search", #scannerHits > 400,
    string.format("%d code lines", #scannerHits))

  check("no beamstate.activateAutoCoupling call remains",
    grepCode(SCANNER_SRC, "activateAutoCoupling") == nil,
    grepCode(SCANNER_SRC, "activateAutoCoupling"))
  check("no _fwCouple state remains", grepCode(SCANNER_SRC, "_fwCouple") == nil)
  check("no FW_COUPLE_ constants remain", grepCode(SCANNER_SRC, "FW_COUPLE_") == nil)
  check("COUPLER_ATTACHED is gone from the Lua",
    grepCode(SCANNER_SRC, "COUPLER_ATTACHED") == nil)
  check("COUPLER_ATTACHED is gone from the Python too",
    grepCode("beamtel.py", "COUPLER_ATTACHED") == nil)

  -- The align must still be a placement, never a repair. vehicle_geometry_sim.lua scenario
  -- 13 owns this rule mod-wide; asserted here as well because this change rewrote the call
  -- site's surroundings and a refactor could quietly drop the tail.
  local tele = grepCode(SCANNER_SRC, "spawn.safeTeleport(")
  check("safeTeleport still carries its two explicit falses",
    tele ~= nil and tele:find("false, false", 1, true) ~= nil, tele)
end

print("7. the lateral convention agrees across files")
do
  -- The offset is MEASURED in the vehicle's body frame and CANCELLED in a world frame.
  -- Those agree only because the teleport puts the truck level and facing awayDir -- which
  -- is the whole reason the body frame is the right one to measure in: a ground frame
  -- (flattened forward + world up) is roll-sensitive, and a semi's fifth-wheel plate sits
  -- about 1.1 m up, so ten degrees of roll swings it 0.19 m. Measured off a still-rocking
  -- truck with the ground frame: 0.224 m off axis, against 0.001 m once settled.
  check("vehicleScanner measures the offset in the BODY frame (up:cross(fwd))",
    grepCode(SCANNER_SRC, "up:cross(fwd)") ~= nil)
  check("vehicleScanner cancels it with cross(awayDir)",
    grepCode(SCANNER_SRC, "vec3(0, 0, 1):cross(awayDir)") ~= nil)
  -- vehicle_geometry_sim.lua scenario 11 polices this convention across the mod. Its file
  -- list is hard-coded, so a new lateral site has to be added there or the agreement is
  -- enforced for every file except the one that steers a 20-tonne teleport.
  -- Specific enough that it cannot pass merely because vehicleScanner is named elsewhere in
  -- that file (scenarios 8 and 10 both mention it).
  local geo = slurp(GEOSIM_SRC)
  check("vehicle_geometry_sim scenario 11 greps vehicleScanner for both crosses",
    geo:find('usesLateral(SCANNER_SRC, "up:cross(fwd)")', 1, true) ~= nil
      and geo:find('usesLateral(SCANNER_SRC, "vec3(0, 0, 1):cross(awayDir)")', 1, true) ~= nil)
end

print("8. the coupler hook guard lives on the table it protects")
do
  local body = slurp(PROTO_SRC)
  check("a mark is set on the game's couplings table",
    body:find("couplings%[COUPLER_HOOK_MARK%]%s*=%s*true") ~= nil)
  check("and it is CHECKED before wrapping",
    body:find("if%s+couplings%[COUPLER_HOOK_MARK%]%s+then") ~= nil)
  -- NEGATIVE CONTROL: the module-local alone must not be the thing standing between us and
  -- a second wrapper. reset() clears it by design, to re-arm the retry harness.
  check("NEGATIVE: reset() still clears the local (so the mark is load-bearing)",
    body:find("_couplerHooksWrapped = false") ~= nil)
  check("the GE handler filters by vehicle id",
    slurp(SCANNER_SRC):find("function M.onCouplerModeChange(vehID, isActive)", 1, true) ~= nil)
end

print("9. COUPLER_RANGE_M agrees between the Lua and the audio engine")
do
  -- The inRange flag is computed in Lua; the beep-rate ramp is scaled by the Python copy.
  -- A divergence produces a rate curve that saturates or never engages, silently. Same
  -- class of drift terrain_scan_sim.py scenario 11 greps for on SCAN_MAX_RANGE_M.
  local py = slurp(AUDIO_SRC):match("COUPLER_RANGE_M%s*=%s*([%d%.]+)")
    or slurp(AUDIO_SRC):match("COUPLER_IN_RANGE_M%s*=%s*([%d%.]+)")
  if py then
    check("audio.py matches vehicleScanner.lua",
      approx(tonumber(py), COUPLER_RANGE_M, 1e-9),
      string.format("lua %.3f vs python %s", COUPLER_RANGE_M, py))
  else
    check("audio.py exposes a named coupler range constant to compare", false,
      "no COUPLER_RANGE_M / COUPLER_IN_RANGE_M found in audio.py")
  end
end

print("")
if #failures == 0 then
  print("ALL CHECKS PASSED")
else
  print(string.format("%d CHECK(S) FAILED:", #failures))
  for _, f in ipairs(failures) do print("  - " .. f) end
  os.exit(1)
end
