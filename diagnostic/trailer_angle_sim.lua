-- Replay trailerAngle.lua's yaw measurement against synthetic poses with known ground truth.
--
--     lua diagnostic/trailer_angle_sim.lua
--
-- This feature drives a tone and nothing else, which means every way of getting it wrong
-- produces a sound that is perfectly plausible and simply false. A mirrored sign says the
-- trailer is swinging left while it swings right, which is worse than no instrument at all --
-- it is an instrument that actively steers you into the jackknife. An unflattened dot product
-- reports several degrees of swing for a trailer that is dead in line but nose-up on a kerb,
-- so the silence the whole feature is built around never arrives. Neither has an in-game
-- symptom that names itself; there is only a tone that feels a bit off.
--
-- Three of the checks are NEGATIVE: they assert that the obvious implementation returns a
-- specific wrong answer, so the check cannot pass for free if someone later "simplifies" the
-- rule back to it.
--
-- Constants are parsed out of the sources rather than copied, so retuning there cannot
-- silently invalidate these checks. Only the *logic* is duplicated here.

local SRC    = "bng_mod/lua/ge/extensions/trailerAngle.lua"
local AUDIO  = "audio.py"
local BEAMTEL = "beamtel.py"

local function slurp(path)
  local fh = assert(io.open(path, "r"), "cannot open " .. path)
  local body = fh:read("*a")
  fh:close()
  return body
end

local function readConstFrom(path, name, pattern)
  local body = slurp(path)
  local val = body:match(pattern or ("\nlocal " .. name .. "%s*=%s*([%-%d%.]+)"))
  assert(val, "could not find " .. name .. " in " .. path)
  return tonumber(val)
end

local TICK_HZ            = readConstFrom(SRC, "TICK_HZ")
local SEND_EPSILON_DEG   = readConstFrom(SRC, "SEND_EPSILON_DEG")
local HEARTBEAT_S        = readConstFrom(SRC, "HEARTBEAT_S")
local TRAILER_ANGLE_SIGN = readConstFrom(SRC, "TRAILER_ANGLE_SIGN")

-- The three Python-side numbers the tone is actually built from.
local TRAILER_FULL_DEG_PY = readConstFrom(BEAMTEL, "TRAILER_FULL_DEG",
  "\nTRAILER_FULL_DEG%s*=%s*([%-%d%.]+)")
local TRAILER_STALE_SEC   = readConstFrom(BEAMTEL, "TRAILER_STALE_SEC",
  "\nTRAILER_STALE_SEC%s*=%s*([%-%d%.]+)")
local TRAILER_FULL_DEG_AU = readConstFrom(AUDIO, "TRAILER_FULL_DEG",
  "\nTRAILER_FULL_DEG%s*=%s*([%-%d%.]+)")
local HYDRO_STEER_DEADZONE = readConstFrom(AUDIO, "HYDRO_STEER_DEADZONE",
  "\nHYDRO_STEER_DEADZONE%s*=%s*([%-%d%.]+)")
local HYDRO_STEER_FULL = readConstFrom(AUDIO, "HYDRO_STEER_FULL",
  "\nHYDRO_STEER_FULL%s*=%s*([%-%d%.]+)")

-- =================================================================================================
--  Minimal vec3, matching the engine's semantics for the three operations used
-- =================================================================================================
local vec3 = {}
vec3.__index = vec3
local function V(x, y, z) return setmetatable({x = x, y = y, z = z}, vec3) end
function vec3.__sub(a, b) return V(a.x - b.x, a.y - b.y, a.z - b.z) end
function vec3:dot(o) return self.x * o.x + self.y * o.y + self.z * o.z end
function vec3:length() return math.sqrt(self:dot(self)) end
function vec3:normalized()
  local l = self:length()
  if l < 1e-12 then return V(0, 0, 0) end
  return V(self.x / l, self.y / l, self.z / l)
end
function vec3:cross(o)
  return V(self.y * o.z - self.z * o.y,
           self.z * o.x - self.x * o.z,
           self.x * o.y - self.y * o.x)
end

-- =================================================================================================
--  The logic under test, mirroring trailerAngle.lua's flatFwd + trailerYawDeg
-- =================================================================================================

local function flatFwd(f)
  local v = V(f.x, f.y, 0)
  if v:length() < 1e-3 then return nil end
  return v:normalized()
end

-- The shipped rule.
local function yawDeg(pFwd, tFwd)
  local pf, tf = flatFwd(pFwd), flatFwd(tFwd)
  if not pf or not tf then return nil end
  local d = pf:dot(tf)
  if d > 1 then d = 1 elseif d < -1 then d = -1 end
  local mag = math.deg(math.acos(d))
  local left = V(0, 0, 1):cross(pf)
  local sign = (tf:dot(left) >= 0) and 1 or -1
  return mag * sign * TRAILER_ANGLE_SIGN
end

-- NEGATIVE CONTROL 1: the same rule without flattening. Reads pitch as swing.
local function yawDegUnflattened(pFwd, tFwd)
  local pf, tf = pFwd:normalized(), tFwd:normalized()
  local d = pf:dot(tf)
  if d > 1 then d = 1 elseif d < -1 then d = -1 end
  return math.deg(math.acos(d))
end

-- NEGATIVE CONTROL 2: the sign taken from fwd:cross(up) instead of up:cross(fwd). Both read
-- as entirely reasonable in the source; they differ by a negation, i.e. by left and right.
local function yawDegMirrored(pFwd, tFwd)
  local pf, tf = flatFwd(pFwd), flatFwd(tFwd)
  if not pf or not tf then return nil end
  local d = pf:dot(tf)
  if d > 1 then d = 1 elseif d < -1 then d = -1 end
  local mag = math.deg(math.acos(d))
  local right = pf:cross(V(0, 0, 1))
  local sign = (tf:dot(right) >= 0) and 1 or -1
  return mag * sign * TRAILER_ANGLE_SIGN
end

-- Python's _trailer_artic_norm, minus the staleness clock.
local function articNorm(deg)
  local n = deg / TRAILER_FULL_DEG_PY
  if n > 1 then n = 1 elseif n < -1 then n = -1 end
  return n
end

-- A heading `deg` degrees to the LEFT of +y (counter-clockwise seen from above, z up),
-- optionally pitched nose-up by `pitch` degrees.
local function heading(deg, pitch)
  local a = math.rad(deg)
  local p = math.rad(pitch or 0)
  local horiz = math.cos(p)
  return V(-math.sin(a) * horiz, math.cos(a) * horiz, math.sin(p))
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
  "tuning: tick %.0f Hz  epsilon %.2f deg  heartbeat %.2f s  sign %+.0f  "
  .. "fullScale %.0f deg  stale %.1f s  deadzone %.2f",
  TICK_HZ, SEND_EPSILON_DEG, HEARTBEAT_S, TRAILER_ANGLE_SIGN,
  TRAILER_FULL_DEG_PY, TRAILER_STALE_SEC, HYDRO_STEER_DEADZONE))
print()

print("0. the extension compiles")
do
  local chunk, err = loadfile(SRC)
  check("trailerAngle.lua parses", chunk ~= nil, err or "")
end
print()

print("1. in line is SILENT -- the property the whole feature rests on")
do
  local fwd = heading(0)
  local deg = yawDeg(fwd, fwd)
  check("a trailer dead in line reads 0 deg", approx(deg, 0, 1e-9),
    string.format("%.6f deg", deg))
  check("...and lands inside the tone's deadzone",
    math.abs(articNorm(deg)) < HYDRO_STEER_DEADZONE,
    string.format("|%.4f| < %.2f", articNorm(deg), HYDRO_STEER_DEADZONE))

  -- The deadzone is a band, not a point: a rig rolling straight is never numerically perfect.
  local slack = HYDRO_STEER_DEADZONE * TRAILER_FULL_DEG_PY
  local nearly = yawDeg(fwd, heading(slack * 0.8))
  check(string.format("...and so does %.1f deg of ordinary slop", slack * 0.8),
    math.abs(articNorm(nearly)) < HYDRO_STEER_DEADZONE,
    string.format("silence holds out to %.2f deg", slack))

  local past = yawDeg(fwd, heading(slack * 1.5))
  check("...but a real swing past it breaks the silence",
    math.abs(articNorm(past)) > HYDRO_STEER_DEADZONE,
    string.format("%.1f deg -> |%.3f|", slack * 1.5, articNorm(past)))
end
print()

print("2. positive is LEFT, and the mirrored form is caught")
do
  local fwd = heading(0)
  local left = yawDeg(fwd, heading(20))
  local right = yawDeg(fwd, heading(-20))
  check("a trailer swung LEFT reads positive", left > 0, string.format("%+.2f deg", left))
  check("a trailer swung right reads negative", right < 0, string.format("%+.2f deg", right))
  check("magnitudes match the pose", approx(math.abs(left), 20, 1e-9)
    and approx(math.abs(right), 20, 1e-9),
    string.format("%.4f / %.4f", math.abs(left), math.abs(right)))

  -- NEGATIVE CONTROL: fwd:cross(up) instead of up:cross(fwd).
  local mirroredLeft = yawDegMirrored(fwd, heading(20))
  check("...and the fwd:cross(up) form gets it exactly backwards",
    mirroredLeft < 0 and approx(mirroredLeft, -left, 1e-9),
    string.format("%+.2f deg where the truth is %+.2f", mirroredLeft, left))

  -- The convention is shared across four files now and nothing but a grep enforces it.
  local body = slurp(SRC)
  check("the source builds its left vector as vec3(0,0,1):cross(fwd)",
    body:find("vec3(0, 0, 1):cross(pf)", 1, true) ~= nil,
    "positive-is-LEFT, shared with vehicleGeometry / implementProximity / terrainScanner")
end
print()

print("3. the yaw is FLATTENED, so pitch is not read as swing")
do
  local fwd = heading(0)
  -- A trailer dead in line, but nose-up 12 degrees: cresting a hump, on a kerb, or simply
  -- squatting on its suspension under load.
  local pitched = heading(0, 12)
  local deg = yawDeg(fwd, pitched)
  check("a nose-up trailer in line still reads 0 deg", approx(deg, 0, 1e-9),
    string.format("%.6f deg at 12 deg of pitch", deg))
  check("...and is still silent", math.abs(articNorm(deg)) < HYDRO_STEER_DEADZONE,
    "the silence survives the terrain")

  -- NEGATIVE CONTROL: the unflattened dot product.
  local naive = yawDegUnflattened(fwd, pitched)
  check("...where the unflattened form invents swing out of nothing",
    naive > 10,
    string.format("%.2f deg of phantom yaw, i.e. |%.3f| of tone", naive, articNorm(naive)))
  check("...enough to break the silence on its own",
    math.abs(articNorm(naive)) > HYDRO_STEER_DEADZONE,
    "a level, in-line trailer would sound")

  -- And a real swing measured on a slope is still the real swing.
  local swungOnSlope = yawDeg(heading(0, 8), heading(25, -6))
  check("a genuine 25 deg swing survives both bodies being pitched",
    approx(math.abs(swungOnSlope), 25, 1e-9),
    string.format("%.4f deg", math.abs(swungOnSlope)))
end
print()

print("4. degrees map onto the WL-40 tone's scale, and clamp")
do
  check("full scale reaches the top of the pitch ramp",
    articNorm(TRAILER_FULL_DEG_PY) >= HYDRO_STEER_FULL,
    string.format("%.0f deg -> %.2f, ramp tops out at %.2f",
      TRAILER_FULL_DEG_PY, articNorm(TRAILER_FULL_DEG_PY), HYDRO_STEER_FULL))
  check("a jackknife past full scale PINS rather than wrapping",
    approx(articNorm(TRAILER_FULL_DEG_PY * 3), 1.0, 1e-12)
    and approx(articNorm(-TRAILER_FULL_DEG_PY * 3), -1.0, 1e-12),
    "135 deg -> +1.00 / -1.00")
  check("the mapping is monotonic through the useful range",
    articNorm(5) < articNorm(15) and articNorm(15) < articNorm(30)
    and articNorm(30) < articNorm(40),
    "5 < 15 < 30 < 40 deg")

  -- The two halves of one number, in two languages, in two files.
  check("TRAILER_FULL_DEG agrees between beamtel.py and audio.py",
    approx(TRAILER_FULL_DEG_PY, TRAILER_FULL_DEG_AU, 1e-9),
    string.format("%.1f / %.1f", TRAILER_FULL_DEG_PY, TRAILER_FULL_DEG_AU))
end
print()

print("5. the feed cannot leave a tone stuck on")
do
  local body = slurp(SRC)
  check("the mod sends an explicit CLEAR, not just silence",
    body:find('send("TRAILER:CLEAR")', 1, true) ~= nil,
    "uncoupling is a message, not a timeout")
  check("...and every giving-up path routes through it",
    select(2, body:gsub("sendClear%(%)", "")) >= 6,
    "no player, no frame, no trailer, no trailer frame, no angle")

  local py = slurp(BEAMTEL)
  check("Python ages the value out as well",
    py:find("TRAILER_STALE_SEC", 1, true) ~= nil and TRAILER_STALE_SEC > 0,
    string.format("%.1f s -- covers a crashed extension or a mod/Python skew",
      TRAILER_STALE_SEC))
  check("...and the heartbeat is fast enough that it cannot expire a live reading",
    HEARTBEAT_S < TRAILER_STALE_SEC * 0.5,
    string.format("%.2f s heartbeat under a %.1f s timeout", HEARTBEAT_S, TRAILER_STALE_SEC))
  check("the tick rate can actually deliver that heartbeat",
    (1.0 / TICK_HZ) < HEARTBEAT_S,
    string.format("%.3f s per tick", 1.0 / TICK_HZ))
end
print()

print("6. the tone is SHARED with the WL-40, never duplicated")
do
  local au = slurp(AUDIO)
  check("audio.py selects a source rather than adding a second voice",
    au:find("hydro_from_trailer = hydro_actual == 0.0", 1, true) ~= nil,
    "one channel, two possible sources, never both")
  check("...with the frame bend winning where it exists",
    au:find("if hydro_from_trailer:%s*\n%s*hydro_actual = self%._trailer_artic") ~= nil
    or au:find("hydro_actual = self._trailer_artic", 1, true) ~= nil,
    "a WL-40 towing something reports its own hinge")
  check("the forward duck exists and is negative",
    au:find("TRAILER_FWD_DUCK_DB", 1, true) ~= nil,
    "quieter forward, full volume in reverse")
  check("...and is applied only through the ducked-source envelope",
    au:find("TRAILER_FWD_DUCK_DB * trailer_duck_env", 1, true) ~= nil,
    "so the term is identically zero for the WL-40")
  check("the centre click disarms on a source change",
    au:find("hydro_from_trailer != self._hydro_was_from_trailer", 1, true) ~= nil,
    "uncoupling at an angle must not announce 'straight'")
end
print()

print("7. only ONE trailer is reported, and it is the one behind you")
do
  -- A truck hooked at both ends: something towing it from in front, something it tows behind.
  local pFwd = heading(0)
  local pPos = V(0, 0, 0)
  local ahead = V(0, 9, 0)    -- +9 m along forward
  local behind = V(0, -9, 0)  -- -9 m, i.e. astern
  local dAhead = (ahead - pPos):dot(pFwd)
  local dBehind = (behind - pPos):dot(pFwd)
  check("the astern partner scores more negative", dBehind < dAhead,
    string.format("%.1f m vs %+.1f m", dBehind, dAhead))
  check("...so the 'most negative wins' rule picks it",
    math.min(dAhead, dBehind) == dBehind,
    "you jackknife what you are pulling, not what is pulling you")

  local body = slurp(SRC)
  check("the registry is read, not hooked",
    body:find("core_vehicles.attachedCouplers", 1, true) ~= nil,
    "onCouplerAttached fires once and misses every coupling it did not arm for")
  check("...with the directed registry preferred where available",
    body:find("core_trailerRespawn", 1, true) ~= nil,
    "tractor -> trailer without any geometry")
  -- Skipping comment lines, because the prose at the top of that file has to write
  -- `setsockname` itself in order to explain why it is absent. vehicle_geometry_sim.lua's
  -- safeTeleport grep hit exactly this and solves it the same way.
  local codeOnly = {}
  for line in (body .. "\n"):gmatch("(.-)\n") do
    if not line:match("^%s*%-%-") then codeOnly[#codeOnly + 1] = line end
  end
  codeOnly = table.concat(codeOnly, "\n")
  check("this extension binds no listening socket",
    codeOnly:find("setsockname", 1, true) == nil,
    "send-only, so the retryCmdBind contract does not apply -- cf. cannonShot")
end
print()

if #failures > 0 then
  print(string.format("%d CHECK(S) FAILED:", #failures))
  for _, f in ipairs(failures) do print("  - " .. f) end
  os.exit(1)
end

print("all checks passed")
