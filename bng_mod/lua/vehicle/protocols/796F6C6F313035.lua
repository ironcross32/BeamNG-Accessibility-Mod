-- This Source Code Form is subject to the terms of the bCDDL, v. 1.1.
-- If a copy of the bCDDL was not distributed with this
-- file, You can obtain one at http://beamng.com/bCDDL-1.1-bCDDL.txt

local M = {}
local hasShiftLights = false

-- Air pressure fallback: cache the *_pressureRelative key for vehicles that
-- don't expose a pressureTank controller (e.g. Gavril T-Series).
local _airPressureKey = nil   -- cached electrics key, e.g. "mainAirTank_pressureRelative"
local _airKeySearched = false -- true once we have committed to a result

-- Coupler mode detection: wrap extensions.couplings hooks to notify the GE extension
-- when the player toggles couplers on/off with the L key.
--
-- This local is ONLY a per-tick short-circuit for the retry harness in fillStruct; it is
-- NOT the guard against double-wrapping. See COUPLER_HOOK_MARK.
local _couplerHooksWrapped = false

-- The real guard, and it lives on the table being patched rather than in this module.
-- `extensions.couplings` is the GAME's module: it survives everything that resets our
-- locals, so a module-local flag cannot protect it. reset() used to clear the local and
-- re-wrap, which read back the PREVIOUS wrapper and wrapped that -- one layer per re-init,
-- each layer sending its own notification. Measured in game: a single hook call produced
-- five "Couplers on" announcements, and the depth grew during one session. Marking the
-- couplings table itself gives the flag the same lifetime as the function it protects, so
-- re-init is a no-op while a genuinely fresh vehicle VM (fresh table, no mark) wraps once.
local COUPLER_HOOK_MARK = "__beamScreenreaderCouplerHookWrapped"

-- Articulation / hydraulic steering: cache of the steering hydraulic cylinders.
-- Machines like the WL-40 wheel loader steer by bending the frame with a pair of
-- opposed rams instead of turning the wheels, so releasing the input does NOT
-- straighten them. fillStruct runs at 60Hz, so the powertrain walk is done once.
local _steerCylinders = nil    -- array of hydraulicCylinder consumer objects
local _steerCylScanned = false -- true once we have committed to a result
local _steerCylOpposed = false -- true when the set contains both +1 and -1 directions
local _steerCylTries = 0       -- retry budget, in case we run before powertrain is built
local STEER_CYL_MAX_TRIES = 120 -- ~2s at 60Hz

-- Which way round the ram extensions map to the world. The project convention is
-- POSITIVE = LEFT everywhere; the jbeam's own direction flags only say which ram opposes
-- which, not which one is the left turn. Confirmed by ear on the WL-40: the raw
-- (steerL - steerR) difference is positive when articulated RIGHT, hence the -1.
local STEER_ARTIC_SIGN = -1

-- Electrics names that a steering cylinder's directional control valve reads from.
local STEER_VALVE_ELECTRICS = {
  steering_joystick = true,
  steering_input    = true,
  steering          = true,
}

-- hydraulicCylinders are not top-level powertrain devices -- powertrain.lua blacklists
-- them from the device factory and hydraulicPump instantiates them into its own
-- connectedConsumers list instead. So powertrain.getDevice("steerR") always returns nil
-- and we have to walk the pumps to reach them.
local function findSteerCylinders()
  if _steerCylScanned then return end

  -- Give up after a budget rather than on the first miss: this can run before the
  -- powertrain has finished building, and a premature miss would latch forever.
  _steerCylTries = _steerCylTries + 1
  if _steerCylTries >= STEER_CYL_MAX_TRIES then _steerCylScanned = true end

  if not powertrain or not powertrain.getDevicesByCategory then return end
  local pumps = powertrain.getDevicesByCategory("hydraulicPowerSource")
  if not pumps or #pumps == 0 then return end

  local found = {}
  local sawPositive, sawNegative = false, false
  for _, pump in pairs(pumps) do
    for _, cyl in ipairs(pump.connectedConsumers or {}) do
      local name = tostring(cyl.name or "")
      local isSteer = STEER_VALVE_ELECTRICS[cyl.directionElectricsName] or name:find("^steer") ~= nil
      if isSteer and cyl.currentExtendPercent then
        found[#found + 1] = cyl
        if (cyl.direction or 1) >= 0 then sawPositive = true else sawNegative = true end
      end
    end
  end

  if #found > 0 then
    _steerCylinders = found
    _steerCylOpposed = sawPositive and sawNegative
    _steerCylScanned = true
  end
end

-- Returns actual frame articulation in -1..1, POSITIVE = LEFT (project-wide convention),
-- or nil when this vehicle has no hydraulic steering cylinders.
local function getCylinderArticulation()
  findSteerCylinders()
  if not _steerCylinders then return nil end

  if _steerCylOpposed then
    -- Opposed pair (WL-40: steerL direction=1, steerR direction=-1). The *difference*
    -- of the two extensions is exactly zero when the frame is straight, whatever the
    -- rams' travel range is -- so this needs no centre calibration constant and
    -- survives an asymmetric minExtend/maxExtend.
    -- Average each side separately rather than taking one mean over all of them, so an
    -- uneven number of rams per side doesn't bias the zero point off straight.
    local posSum, posN, negSum, negN = 0, 0, 0, 0
    for _, cyl in ipairs(_steerCylinders) do
      if (cyl.direction or 1) >= 0 then
        posSum = posSum + cyl.currentExtendPercent
        posN = posN + 1
      else
        negSum = negSum + cyl.currentExtendPercent
        negN = negN + 1
      end
    end
    return ((posSum / posN) - (negSum / negN)) * STEER_ARTIC_SIGN
  end

  -- Single-ram steering: no difference to take, so fall back on the ram's own midpoint.
  -- This one does assume the travel range is symmetric about straight.
  local cyl = _steerCylinders[1]
  return ((cyl.currentExtendPercent * 2) - 1) * (cyl.direction or 1) * STEER_ARTIC_SIGN
end

-- ---------------------------------------------------------------------------------------
-- Implement (loader bucket / forks)
-- ---------------------------------------------------------------------------------------
-- Everything below is silent on any vehicle without hydraulic implement cylinders, exactly
-- the way actualSteering is: the cylinder walk finds nothing, IMPL_FLAG_PRESENT never gets
-- set, and the Python side's status metrics and tones stay hidden. There is no per-vehicle
-- check anywhere in here.

local IMPL_FLAG_PRESENT      = 1
local IMPL_FLAG_ARTIC_VALID  = 2
local IMPL_FLAG_GROUND_VALID = 4
local IMPL_FLAG_JACKING      = 8
local IMPL_FLAG_DETACHED     = 16

-- Lua patterns have no alternation, so keyword sets are tables walked by matchesAnyWord.
local IMPL_LIFT_WORDS = {"lift", "boom", "hoist"}
local IMPL_TILT_WORDS = {"tilt", "curl", "dump"}
local IMPL_SLOT_WORDS = {"attachment", "implement"}
local IMPL_PART_WORDS = {"bucket", "fork", "grapple", "blade", "plow", "tine", "scoop"}

local IMPL_CYL_MAX_TRIES  = 120   -- ~2s at 60Hz, same budget as the steering scan
local IMPL_NODE_MAX_TRIES = 120
local IMPL_TICK_INTERVAL  = 0.05  -- 20Hz; the Python tones' smoothing absorbs the rest
local IMPL_SANE_GROUND_M  = 25.0  -- reject a surface reading further than this
-- obj:getSurfaceHeightBelow does NOT return nil when there is no surface -- it returns a
-- huge negative number (the game itself clamps it with max(-1e6, ...)). So the guard is a
-- sanity band on the computed distance, not a nil check.
local IMPL_SANE_GROUND_MIN = -1.0
local IMPL_DETACH_SLOP_M  = 0.6   -- implement drifting this far past its design offset
-- Fraction of the implement's fore/aft extent that counts as "the leading edge" (and, at
-- the other end, "the heel"). The lateral extremes are then taken from within that band.
local IMPL_EDGE_BAND      = 0.30
-- Fraction of the implement's design-space VERTICAL extent that counts as its floor. The
-- edge and heel picks are restricted to it so the edge->heel axis is the implement's floor
-- plane -- the tine underside on forks, the bowl floor on a bucket -- rather than a diagonal
-- across it. Without this, the forks' heel picks land on the top of the backplate (the
-- lateral extremes of the rear band with no height constraint), and the resulting axis sits
-- ~38 degrees off the tine plane. That is what made the tilt readout report level at an
-- angle nothing can be driven into.
local IMPL_FLOOR_BAND     = 0.35
local IMPL_EDGE_MIN_WIDTH = 0.30  -- narrower than this and the derived heading gets noisy
-- How many nodes a part sitting in an attachment slot has to have before that slot alone is
-- taken as evidence it is an implement. A slot named "...attachment" says nothing about what
-- was fitted into it: EVERY stock car has a towhitch_receiver_attachment slot, and a fitted
-- ball hitch is five nodes carrying a matching partPath -- which is exactly the shape the
-- slot tier accepts. The smallest stock implement is the block handler forks at 36 nodes and
-- the tow hitches are 5, so this sits an order of magnitude clear of both. It applies ONLY to
-- the slot tiers: a part whose own name says bucket/fork/grapple has named itself and keeps
-- the ordinary four-node floor.
local IMPL_MIN_ATTACH_NODES = 12
local IMPL_ACT_RATE_FULL  = 0.30  -- ram travel fraction per second that counts as full
-- Soft-body rams never sit perfectly still, so the rate term needs a floor it must clear
-- before it counts as movement at all. Real operation runs near 0.5/s, so this costs
-- nothing in practice and is what stops the Python tones pumping on a parked machine.
local IMPL_ACT_RATE_DEADBAND = 0.05
local IMPL_ACT_RATE_TAU   = 0.15  -- seconds, smoothing on the measured travel rate
local IMPL_STALL_RATE     = 0.02  -- ram considered stalled below this travel fraction/s
local IMPL_STALL_CMD      = 0.6   -- ...while commanded harder than this
local IMPL_REST_TAU       = 1.5   -- seconds, EMA on the resting front-axle clearance
local IMPL_ARTIC_FALLBACK_DEG = 40.0 -- used only if the axle geometry cannot be measured

local _implCylScanned  = false
local _implCylTries    = 0
local _implCylDiagLogged = false -- the cylinder-scan failure dump has been written once
local _implLiftCyls    = nil
local _implTiltCyls    = nil

local _implNodesScanned = false
local _implNodeTries    = 0
local _implEdgeCids     = nil  -- {left, centre, right} of the cutting edge / tine tips
local _implHeelCids     = nil  -- {left, right} of the implement's rear-bottom
local _implTiltZeroDeg  = 0.0  -- design-pose pitch of the floor plane; LOGGED ONLY, see below
local _implPushed       = false -- the cid list has been handed to the GE proximity extension
local _implClock        = 0     -- seconds of sim time, for the push heartbeat
local _implPushAt       = -1e9  -- when the cid list was last sent
local _implPushName     = ""    -- ...and what was sent, so it can be repeated verbatim
local _implPushCids     = nil
local IMPL_PUSH_HEARTBEAT_S = 4.0
local _implDiagLogged   = false -- the "why did resolution fail" dump has been written once
local _implRefCid       = nil  -- a machine-body node, for the detach test
local _implRefDist      = 0.0  -- its design-space distance to the centre edge node

local _implFrontWheels = nil   -- wheel entries on the implement end of the machine
local _implRearWheels  = nil
local _implWheelsScanned = false

local _implAccum       = 0.0
local _implPrevExtend  = nil   -- cylinder -> last currentExtendPercent
local _implRateSmooth  = nil   -- cylinder -> EMA of |d(extendPercent)/dt|
local _implRestClear   = nil   -- EMA of front-axle height above the surface, wheels loaded

-- Cached outputs, refilled at IMPL_TICK_INTERVAL and re-sent on every 60Hz packet.
local _implFlags     = 0
local _implEdgeH     = -1.0
local _implMinClear  = -1.0
local _implTiltDeg   = 0.0
local _implTiltPct   = 0.0
local _implLift      = 0.0
local _implActivity  = 0.0
local _implArticDeg  = 0.0

-- Keyword matching, boundary-aware. A keyword only counts where it STARTS a word: after a
-- separator or a digit, at the start of the string, or at a camelCase hump. Plain substring
-- matching is what let stock parts claim to be implements -- "us_semi_ramplow" (the rollback
-- bed's loading ramp) matched "plow", "covet_roofscoop" and "sunburst2_hoodscoop" matched
-- "scoop" -- and every one of those is a false implement on a vehicle that has none.
--
-- The hump test must run against the ORIGINAL string, not the lowered copy: lowering
-- "wl40_liftarm_blockForks" makes "fork" follow a "k", so a boundary rule that only looked
-- at the lowered form would reject the WL-40's block handler forks, which are a real
-- implement. Lowered for the search, original for the boundary -- both, or neither works.
local function matchesAnyWord(s, words)
  if type(s) ~= "string" then return false end
  local low = s:lower()
  for _, w in ipairs(words) do
    local from = 1
    while true do
      local i = low:find(w, from, true)
      if not i then break end
      -- %W is "not alphanumeric", which counts "_" and "/" as separators, as intended.
      local prev = (i > 1) and low:sub(i - 1, i - 1) or nil
      local atBoundary = (prev == nil) or prev:match("%W") ~= nil or prev:match("%d") ~= nil
      local atHump = s:sub(i, i):match("%u") ~= nil
      if atBoundary or atHump then return true end
      from = i + 1
    end
  end
  return false
end

-- Same pump walk as findSteerCylinders. Steering is classified FIRST so a cylinder driven
-- by steering_input can never be mistaken for a lift ram on a machine that names them oddly.
local function findImplementCylinders()
  if _implCylScanned then return end

  _implCylTries = _implCylTries + 1
  if _implCylTries >= IMPL_CYL_MAX_TRIES then _implCylScanned = true end

  if not powertrain or not powertrain.getDevicesByCategory then return end
  local pumps = powertrain.getDevicesByCategory("hydraulicPowerSource")
  if not pumps or #pumps == 0 then return end

  local lifts, tilts = {}, {}
  for _, pump in pairs(pumps) do
    for _, cyl in ipairs(pump.connectedConsumers or {}) do
      if cyl.currentExtendPercent then
        local name = tostring(cyl.name or "")
        local dirName = cyl.directionElectricsName
        local isSteer = STEER_VALVE_ELECTRICS[dirName] or name:find("^steer") ~= nil
        if not isSteer then
          local hay = name .. " " .. tostring(dirName or "")
          if matchesAnyWord(hay, IMPL_TILT_WORDS) then
            tilts[#tilts + 1] = cyl
          elseif matchesAnyWord(hay, IMPL_LIFT_WORDS) then
            lifts[#lifts + 1] = cyl
          end
        end
      end
    end
  end

  if #lifts > 0 or #tilts > 0 then
    _implLiftCyls = (#lifts > 0) and lifts or nil
    _implTiltCyls = (#tilts > 0) and tilts or nil
    _implCylScanned = true
    log('I', 'beamtel.implement', string.format(
      "implement cylinders: %d lift, %d tilt", #lifts, #tilts))
  elseif _implCylScanned and not _implCylDiagLogged then
    -- Budget exhausted with nothing classified. Same reasoning as the node dump: this is
    -- pure name matching, so list what the pumps actually offered.
    _implCylDiagLogged = true
    local seen = {}
    for _, pump in pairs(pumps) do
      for _, cyl in ipairs(pump.connectedConsumers or {}) do
        seen[#seen + 1] = string.format("%s[%s]",
          tostring(cyl.name), tostring(cyl.directionElectricsName))
      end
    end
    log('W', 'beamtel.implement',
      "no implement cylinders classified; consumers seen: " .. table.concat(seen, ", "))
  end
end

-- Resolve the implement's node set by NAME, never by geometry. A "everything forward of the
-- tilt ram" rule looks tempting and is wrong: the WL-40's own lift arm reaches forward of the
-- tilt ram, so it would be swept in and would corrupt the tilt axis and the clearance.
-- Hand the resolved implement to the GE side. implementProximity.lua needs exactly this
-- node set to test the bucket against nearby vehicles, and re-deriving the name matching
-- over there would be two copies of the same fragile thing. Pushed rather than pulled
-- because the resolution only ever changes when it re-runs, and it re-runs on reset --
-- so GE can treat the newest push as authoritative and never has to guard against
-- reading stale cids.
-- Re-sent periodically rather than once, which is a correction. The game-engine side drops
-- its cids on any event that could invalidate them -- a Lua reload, a vehicle switch -- but
-- it has no way to ASK for them back, and this VM's one-shot latch meant it never volunteered
-- them either. Switching out of the loader and back in therefore killed every implement
-- feature until the vehicle was reset, which is a routine thing to do and left no error
-- anywhere. A heartbeat costs one queueGameEngineLua every few seconds and makes the whole
-- handoff self-healing: whatever GE has lost, it gets back within IMPL_PUSH_HEARTBEAT_S.
--
-- Only machines that HAVE an implement heartbeat. A vehicle with none says so once; repeating
-- it would let whichever vehicle spoke last overwrite the loader's cids while you are sitting
-- in the other one.
local function pushImplementToGE(partName, friendly, cids)
  _implPushed = true
  _implPushAt = _implClock
  _implPushName = friendly or partName or ""
  _implPushCids = cids
  local list = {}
  for _, cid in ipairs(cids or {}) do list[#list + 1] = tostring(cid) end
  -- The five sample nodes go over separately, in a FIXED order that the GE side relies on:
  -- edgeL, edgeC, edgeR, heelL, heelR. Edge-mid minus heel-mid is the implement's own
  -- forward axis, which is what lets the vehicle scanner aim from the bucket rather than
  -- from the cab on an articulated machine.
  local sample = {}
  if _implEdgeCids and _implHeelCids then
    for _, cid in ipairs(_implEdgeCids) do sample[#sample + 1] = tostring(cid) end
    for _, cid in ipairs(_implHeelCids) do sample[#sample + 1] = tostring(cid) end
  end
  obj:queueGameEngineLua(string.format(
    "if extensions.implementProximity then extensions.implementProximity.onImplementCids(%d, %q, %q, %q) end",
    obj:getID(), tostring(friendly or partName or ""),
    table.concat(sample, ","), table.concat(list, ",")))
end

-- ==================================================================================================
--  Ramp hydraulics
-- ==================================================================================================
-- What the deck of a ramp machine is actually doing: how far each hydraulic group has travelled
-- out of its own stroke. Pushed to the GE side, where implementProximity pairs it with the ramp's
-- live pitch and sends the pair on to the F9+I readout.
--
-- Deliberately NOT derived from geometry, which is the obvious route and is wrong in exactly the
-- pose the readout exists for. Measuring the mouth's displacement from the machine's reference
-- node along the deck axis is exact at rest and picks the TILT up as false extension, because the
-- reference node does not lie on the axis the deck rotates about: measured on a us_semi rollback,
-- a pure 10.2 degree tilt with the bed fully home slid the mouth 0.123 m along its own axis, so
-- the readout would have announced four inches of extension on a deck that had not moved, growing
-- with the angle. The ram's own extension carries no such term -- against that same vehicle it
-- tracked the geometrically measured deck travel to within 1 cm over a 4.88 m stroke.
--
-- Grouped by directionElectricsName rather than classified against a word list. That field is the
-- vehicle's OWN vocabulary -- the very string its Special Vehicle Keys bindings drive -- so the
-- readout names each control the way the machine's own controls are named, and a machine nobody
-- has thought about reports correctly with no entry anywhere. Grouped rather than listed because
-- a deck is run out by a PAIR of rams reporting the same figure to six decimal places, and
-- reading that out twice is noise.
local RAMP_HYD_PUSH_S       = 0.2   -- 5 Hz: this feeds a key-press readout, not a tone
local RAMP_HYD_MIN_TRAVEL_M = 0.02  -- below this a "group" is a linkage detail, not a control
-- Re-sent on a heartbeat as well as on change, for the reason IMPL_PUSH_HEARTBEAT_S already
-- records: a latch that lives in THIS VM cannot be cleared by the other side restarting, and
-- the other side drops what it holds on a GE Lua reload, a vehicle switch or an extension
-- reload. Change-only, a machine sitting still never volunteers its position again -- measured:
-- reloading implementProximity emptied its table, the change test here then suppressed every
-- push, and the deck readout stayed silent until the bed was physically moved. Four seconds of
-- one small cross-VM call is what makes the handoff self-healing.
local RAMP_HYD_HEARTBEAT_S  = 4.0
local _rampHydAt   = -1e9
local _rampHydSent = -1e9
local _rampHydLast = nil

local function pushRampHydraulics()
  if (_implClock - _rampHydAt) < RAMP_HYD_PUSH_S then return end
  _rampHydAt = _implClock
  if not (powertrain and powertrain.getDevicesByCategory) then return end
  local pumps = powertrain.getDevicesByCategory("hydraulicPowerSource")
  if not pumps then return end

  local order, byName = {}, {}
  for _, pump in pairs(pumps) do
    for _, cyl in ipairs(pump.connectedConsumers or {}) do
      local dirName = cyl.directionElectricsName
      local pct = tonumber(cyl.currentExtendPercent)
      local lo, hi = tonumber(cyl.minExtend), tonumber(cyl.maxExtend)
      -- Steering valves are excluded by the same table the articulation tone uses. A bent
      -- frame is not a ramp, and it already has a continuous tone of its own.
      if pct and lo and hi and type(dirName) == "string" and dirName ~= ""
         and not STEER_VALVE_ELECTRICS[dirName]
         and (hi - lo) >= RAMP_HYD_MIN_TRAVEL_M then
        local g = byName[dirName]
        if not g then
          g = {n = 0, sum = 0, travel = hi - lo}
          byName[dirName] = g
          order[#order + 1] = dirName
        end
        g.n, g.sum = g.n + 1, g.sum + pct
      end
    end
  end

  -- Rounded to a whole percent and a centimetre BEFORE the change test. A deck sitting on its
  -- own springs jitters in the sixth decimal place, so an unrounded comparison would re-push at
  -- the full rate forever on a machine that is doing nothing.
  local parts = {}
  for _, dirName in ipairs(order) do
    local g = byName[dirName]
    parts[#parts + 1] = dirName .. ":"
      .. tostring(math.floor(g.sum / g.n * 100 + 0.5)) .. ":"
      .. tostring(math.floor(g.travel * 100 + 0.5))
  end
  local payload = table.concat(parts, ";")
  if payload == _rampHydLast and (_implClock - _rampHydSent) < RAMP_HYD_HEARTBEAT_S then
    return
  end
  _rampHydLast, _rampHydSent = payload, _implClock
  -- An EMPTY payload is pushed too, and is not the same as never pushing: it is this VM saying
  -- "I have no hydraulics", which is what stops the GE side waiting for an answer that is never
  -- coming on an ordinary car. Same argument rampGeometry's chunk makes for always replying.
  obj:queueGameEngineLua(string.format(
    "if extensions.implementProximity then extensions.implementProximity.onRampHydraulics(%d, %q) end",
    obj:getID(), payload))
end

local function resolveImplementNodes()
  if _implNodesScanned then
    -- Budget exhausted with nothing found: tell GE so it stops waiting.
    if not _implPushed then pushImplementToGE(nil, "", nil) end
    return
  end

  -- ACTUATION GATE. An implement is not a part with a suggestive name -- it is the thing the
  -- machine's lift and tilt rams move. Resolving nodes before that is established is what let
  -- ordinary vehicles claim one: the tiers below are pure name matching, and the game is full
  -- of names that satisfy them (bucket SEATS on half the fleet, hood and roof SCOOPS, a tow
  -- hitch in an "...attachment" slot). Every one of those pushed a cid list to the GE side,
  -- which is the docking instrument's ONLY test for "does this vehicle have an implement" --
  -- so the readout measured from a seat frame, the scanner aimed from a tow ball, and ramp
  -- mode, whose whole premise is a vehicle with no implement, could never be reached on the
  -- cars that need it.
  --
  -- This is not a new rule, it is the rule the rest of the block already used: IMPL_FLAG_PRESENT
  -- has always required (_implEdgeCids and (_implLiftCyls or _implTiltCyls)), so on a car that
  -- name-matched, the tones and status metrics stayed correctly silent and only the GE push
  -- escaped -- one half of this file's own definition of an implement leaking out past the
  -- other. Everything downstream of the cylinders needs them anyway (activity, jacking, the
  -- stall test are all read off currentExtendPercent), so a machine that fails this gate could
  -- never have produced a working readout even if its node set were the genuine article.
  --
  -- Gating on the cylinders costs nothing on a real loader (they classify on the first tick
  -- the powertrain is built) and is silent by construction everywhere else: an ordinary car
  -- has no hydraulicPowerSource at all, so the scan finds nothing and this returns for the
  -- rest of the session. Wait for the scan to COMMIT rather than failing on the first miss,
  -- and do not spend node tries while waiting -- both scans run off the same 20 Hz tick and
  -- the powertrain may not be built yet.
  if not (_implLiftCyls or _implTiltCyls) then
    if not _implCylScanned then return end
    _implNodesScanned = true
    if not _implPushed then
      log('I', 'beamtel.implement',
        "no implement lift/tilt cylinders on this vehicle; implement features stay off")
      pushImplementToGE(nil, "", nil)
    end
    return
  end

  _implNodeTries = _implNodeTries + 1
  if _implNodeTries >= IMPL_NODE_MAX_TRIES then _implNodesScanned = true end

  if not (v and v.data and v.data.nodes) then return end

  -- Resolution is entirely name-driven, across three tiers. Each tier proposes a candidate
  -- part name and is only ACCEPTED if that name actually collects enough nodes; a tier that
  -- proposes a name matching nothing falls through to the next one. Committing to the first
  -- proposal instead is what made the original version fail on the WL-40: the activeParts
  -- tier matched the attachment slot path, handed back a name that collected zero nodes, and
  -- the keyword tier that would have found "wl40_bucket" never ran.
  --
  -- Tiers 1 and 2 key off node options that jbeam/slotSystem.lua writes side by side
  -- (partOrigin = the part NAME, partPath = the full slot path), so a node carrying one
  -- generally carries the other.
  local partName, cids = nil, {}

  local function collect(candidate)
    if type(candidate) ~= "string" or candidate == "" then return nil end
    local out = {}
    for _, nd in pairs(v.data.nodes) do
      if nd.partOrigin == candidate and nd.pos then
        out[#out + 1] = nd.cid
      end
    end
    if #out < 4 then return nil end
    return out
  end

  -- Choose between the candidates a slot tier proposes, rather than taking whichever one
  -- pairs() happened to hand over first. Two reasons, and the second is the bug:
  --   * pairs() order is arbitrary, so on a machine with two filled attachment slots the
  --     resolution was nondeterministic between runs.
  --   * a slot named "...attachment" is not evidence about what is IN it. A ball hitch in a
  --     towhitch_receiver_attachment slot matched the tier and won, and five nodes of tow
  --     ball then became "the implement" -- on any vehicle that got past the actuation gate,
  --     e.g. a rollback tow truck, whose bed rams are named tilt1/tilt2.
  -- So: a part whose own name says implement wins outright; otherwise the largest node set
  -- wins and has to clear IMPL_MIN_ATTACH_NODES. Ties break on the name so the answer is
  -- stable across sessions.
  local function pickCandidate(names)
    local namedN, namedC, bigN, bigC = nil, nil, nil, nil
    for name in pairs(names) do
      local got = collect(name)
      if got then
        if matchesAnyWord(name, IMPL_PART_WORDS) then
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

  -- Tier 1: a node whose partPath runs through an attachment/implement slot.
  -- Note the explicit ~= "": slotSystem appends a reset row that blanks every slot option
  -- to the EMPTY STRING, not to nil, and "" is truthy in Lua. Without this guard a node
  -- carrying a blanked partOrigin could win the match and then collect every other blanked
  -- node in the vehicle as "the implement".
  local slotNames = {}
  for _, nd in pairs(v.data.nodes) do
    if nd.partOrigin and nd.partOrigin ~= ""
        and matchesAnyWord(nd.partPath, IMPL_SLOT_WORDS) then
      slotNames[nd.partOrigin] = true
    end
  end
  do
    local n, got = pickCandidate(slotNames)
    if n then partName, cids = n, got end
  end

  -- Tier 2: a partOrigin that names itself as an implement (wl40_bucket,
  -- wl40_liftarm_blockForks, ...).
  if not partName then
    for _, nd in pairs(v.data.nodes) do
      if matchesAnyWord(nd.partOrigin, IMPL_PART_WORDS) then
        local got = collect(nd.partOrigin)
        if got then partName, cids = nd.partOrigin, got break end
      end
    end
  end

  -- Tier 3: the slot-path table, if beamstate happens to have populated it. Last because it
  -- is not guaranteed to exist here and its values are not guaranteed to be plain names.
  if not partName then
    local pathNames = {}
    for path, name in pairs(v.data.activeParts or {}) do
      if matchesAnyWord(path, IMPL_SLOT_WORDS) and type(name) == "string" then
        pathNames[name] = true
      end
    end
    local n, got = pickCandidate(pathNames)
    if n then partName, cids = n, got end
  end

  if not partName then
    -- Say why, once, when the retry budget finally runs out. Every tier above matches on
    -- names, so when a machine reports nothing this list is the whole diagnosis: it shows
    -- whether the node options are present at all and what they actually say.
    if _implNodesScanned and not _implDiagLogged then
      _implDiagLogged = true
      local seen, order, nOrigin, nPath, nNodes = {}, {}, 0, 0, 0
      local samplePath = nil
      for _, nd in pairs(v.data.nodes) do
        nNodes = nNodes + 1
        if nd.partOrigin and nd.partOrigin ~= "" then
          nOrigin = nOrigin + 1
          if not seen[nd.partOrigin] then
            seen[nd.partOrigin] = true
            order[#order + 1] = tostring(nd.partOrigin)
          end
        end
        if nd.partPath and nd.partPath ~= "" then
          nPath = nPath + 1
          samplePath = samplePath or tostring(nd.partPath)
        end
      end
      table.sort(order)
      log('W', 'beamtel.implement', string.format(
        "no implement resolved: %d nodes, %d with partOrigin, %d with partPath, "
        .. "activeParts=%s, matched part=%s, cids=%d",
        nNodes, nOrigin, nPath, tostring(v.data.activeParts ~= nil),
        tostring(partName), #cids))
      log('W', 'beamtel.implement', "sample partPath: " .. tostring(samplePath))
      log('W', 'beamtel.implement', "partOrigin values seen: " .. table.concat(order, ", "))
    end
    return
  end

  -- Which way the implement sits relative to the machine, derived rather than assumed:
  -- the fore/aft sign is whichever direction takes you from the whole-vehicle centroid to
  -- the implement centroid. This avoids having to know the jbeam's axis convention.
  local allY, allN = 0, 0
  for _, nd in pairs(v.data.nodes) do
    if nd.pos then allY = allY + nd.pos.y; allN = allN + 1 end
  end
  local implY = 0
  for _, cid in ipairs(cids) do implY = implY + v.data.nodes[cid].pos.y end
  local fwdSign = ((implY / #cids) - (allY / math.max(1, allN))) >= 0 and 1 or -1

  -- Five fixed sample points, chosen once from the free design-space positions. Live node
  -- reads then cost five engine calls a tick instead of re-deriving an AABB from ~120.
  --
  -- Select in two stages: take a BAND at each end of the implement's fore/aft extent, then
  -- within that band take the LATERAL EXTREMES. Scoring "most forward and lowest" per side
  -- and calling the winners left and right does not work: nothing in that score rewards
  -- being far out, so on a bucket whose centre teeth sit slightly more forward and lower
  -- than its corners, both picks land within a few centimetres of the centreline. The GE
  -- side derives the implement's heading from edgeL -> edgeR, and a 10 cm baseline made of
  -- two jittering soft-body nodes points in a random direction -- which showed up as the
  -- scanner refusing to settle on centre and snapping left and right on a parked machine.
  local minF, maxF = math.huge, -math.huge
  for _, cid in ipairs(cids) do
    local f = v.data.nodes[cid].pos.y * fwdSign
    if f < minF then minF = f end
    if f > maxF then maxF = f end
  end
  local span = maxF - minF
  local frontCut = maxF - span * IMPL_EDGE_BAND
  local rearCut  = minF + span * IMPL_EDGE_BAND

  -- ...and the third stage: keep only what sits in the implement's FLOOR band. The band
  -- picks above are lateral extremes with no height constraint, which on the block-handler
  -- forks puts the heel pair on top of the backplate. edgeMid -> heelMid is then a diagonal
  -- across the attachment rather than the plane the load actually rides on, and every angle
  -- derived from it -- the tilt readout, the entry gate -- is wrong by however tall the
  -- implement is. Not a per-vehicle table: the floor of a bucket and the underside of a
  -- tine are both "the low part of this thing", which is all this measures.
  local minZ, maxZ = math.huge, -math.huge
  for _, cid in ipairs(cids) do
    local z = v.data.nodes[cid].pos.z
    if z < minZ then minZ = z end
    if z > maxZ then maxZ = z end
  end
  local floorCut = minZ + (maxZ - minZ) * IMPL_FLOOR_BAND

  -- Widest pair in a band, plus the one nearest the MIDPOINT of that pair (lowest wins ties,
  -- since that is the point that touches the ground first).
  local function bandPicks(inBand)
    local function gather(lowOnly)
      local out = {}
      for _, cid in ipairs(cids) do
        local p = v.data.nodes[cid].pos
        if inBand(p.y * fwdSign) and ((not lowOnly) or p.z <= floorCut) then
          out[#out + 1] = cid
        end
      end
      return out
    end
    -- Fall back to the unconstrained set if the floor band holds fewer than a pair, so a
    -- flat implement with no vertical spread still resolves rather than refusing outright.
    local pool = gather(true)
    if #pool < 2 then pool = gather(false) end

    local l, r = nil, nil
    local lx, rx = -math.huge, math.huge
    local lowZ, highZ = math.huge, -math.huge
    for _, cid in ipairs(pool) do
      local p = v.data.nodes[cid].pos
      if p.x > lx then l, lx = cid, p.x end
      if p.x < rx then r, rx = cid, p.x end
      if p.z < lowZ then lowZ = p.z end
      if p.z > highZ then highZ = p.z end
    end
    if not (l and r) then return nil end

    -- Nearest the midpoint of that pair, NOT nearest the vehicle centreline. Forks have
    -- nothing in the middle, so the centreline rule lands on an inner node of the left tine
    -- -- 0.6 m off centre and well behind the tips -- and drags mean(edgeL, edgeC, edgeR)
    -- with it, biasing every lateral reading the docking instrument produces.
    local midX = (lx + rx) * 0.5
    local c, cd, cz = nil, math.huge, math.huge
    for _, cid in ipairs(pool) do
      local p = v.data.nodes[cid].pos
      local d = math.abs(p.x - midX)
      if d < cd or (d == cd and p.z < cz) then c, cd, cz = cid, d, p.z end
    end
    return l, r, c, (lx - rx), (highZ - lowZ)
  end

  local edgeL, edgeR, edgeC, edgeWidth = bandPicks(function(f) return f >= frontCut end)
  local heelL, heelR, _, _, heelZSpread = bandPicks(function(f) return f <= rearCut end)
  if not (edgeC and edgeL and edgeR and heelL and heelR) then return end
  -- A band that collapsed to a single point gives the GE side no usable heading; it falls
  -- back to the along-axis there, so just note it rather than refusing the implement.
  if edgeWidth < IMPL_EDGE_MIN_WIDTH then
    log('W', 'beamtel.implement', string.format(
      "implement leading edge is only %.2f m wide; heading will be less stable", edgeWidth))
  end

  _implEdgeCids = {edgeL, edgeC, edgeR}
  _implHeelCids = {heelL, heelR}

  -- Design-pose pitch of the floor plane. This USED to be subtracted from the live pitch, so
  -- that 0 degrees meant "as modelled". With a genuine floor axis that offset is not wanted:
  -- world pitch already IS the angle from level, which is the only thing the operator can
  -- act on -- an implement modelled nose-down reads nose-down, as it should. It is still
  -- computed and logged, because a large value now means the floor band resolved onto
  -- something that is not the floor, and that should be visible rather than silent.
  local function designMid(list)
    local x, y, z = 0, 0, 0
    for _, cid in ipairs(list) do
      local p = v.data.nodes[cid].pos
      x, y, z = x + p.x, y + p.y, z + p.z
    end
    local n = #list
    return vec3(x / n, y / n, z / n)
  end
  -- Same two-point edge axis the live tilt uses, so the logged design pitch is comparable
  -- with what the readout reports rather than being a different measurement.
  local dA, dB = designMid({edgeL, edgeR}), designMid(_implHeelCids)
  local dLen = dA:distance(dB)
  if dLen > 1e-4 then
    _implTiltZeroDeg = math.deg(math.asin(math.max(-1, math.min(1, (dA.z - dB.z) / dLen))))
  end

  -- Reference node on the machine body for the detach test: nearest the vehicle centroid,
  -- i.e. as far from anything breakable as we can cheaply get.
  local cx, cy, cz, cn = 0, 0, 0, 0
  for _, nd in pairs(v.data.nodes) do
    if nd.pos and nd.partOrigin ~= partName then
      cx, cy, cz, cn = cx + nd.pos.x, cy + nd.pos.y, cz + nd.pos.z, cn + 1
    end
  end
  if cn > 0 then
    local centre = vec3(cx / cn, cy / cn, cz / cn)
    local best, bestD = nil, math.huge
    for _, nd in pairs(v.data.nodes) do
      if nd.pos and nd.partOrigin ~= partName then
        local d = centre:distance(vec3(nd.pos.x, nd.pos.y, nd.pos.z))
        if d < bestD then best, bestD = nd.cid, d end
      end
    end
    if best then
      local ep = v.data.nodes[edgeC].pos
      _implRefCid = best
      _implRefDist = vec3(v.data.nodes[best].pos.x, v.data.nodes[best].pos.y, v.data.nodes[best].pos.z)
                       :distance(vec3(ep.x, ep.y, ep.z))
    end
  end

  _implNodesScanned = true

  local friendly = partName
  local pd = v.data.activePartsData and v.data.activePartsData[partName]
  if pd and pd.information and type(pd.information.name) == "string" then
    friendly = pd.information.name
  end
  -- Resolution is entirely name-driven, so say out loud what it landed on. If a machine
  -- ever reports nothing, this line is the first thing to look at.
  log('I', 'beamtel.implement', string.format(
    "implement '%s' (%s): %d nodes, edge cids %s/%s/%s, heel %s/%s, heel band z spread "
    .. "%.2f m, floor-plane design pitch %.1f deg (diagnostic only; live tilt is world pitch)",
    tostring(partName), tostring(friendly), #cids, tostring(edgeL), tostring(edgeC),
    tostring(edgeR), tostring(heelL), tostring(heelR), heelZSpread or -1.0, _implTiltZeroDeg))
  pushImplementToGE(partName, friendly, cids)
end

-- Split the wheels into the pair on the implement end and the pair behind it. Used both for
-- the jacking test and for measuring the articulation angle.
local function resolveImplementWheels()
  if _implWheelsScanned then return end
  _implWheelsScanned = true
  if not (wheels and wheels.wheels and v and v.data and v.data.nodes) then return end

  local entries = {}
  for _, wd in pairs(wheels.wheels) do
    local n1 = wd.node1 and v.data.nodes[wd.node1]
    if n1 and n1.pos then entries[#entries + 1] = {wd = wd, y = n1.pos.y} end
  end
  if #entries < 4 then return end

  local sum = 0
  for _, e in ipairs(entries) do sum = sum + e.y end
  local mid = sum / #entries

  -- "Front" here means the implement end, whichever sign of y that is on this jbeam.
  local implFwd = 1
  if _implEdgeCids then
    local ep = v.data.nodes[_implEdgeCids[2]].pos
    implFwd = (ep.y >= mid) and 1 or -1
  end

  local front, rear = {}, {}
  for _, e in ipairs(entries) do
    if (e.y - mid) * implFwd > 0 then front[#front + 1] = e.wd else rear[#rear + 1] = e.wd end
  end
  if #front > 0 and #rear > 0 then
    _implFrontWheels, _implRearWheels = front, rear
  end
end

-- World position of a node. obj:getNodePosition returns an offset from the vehicle origin
-- expressed in WORLD axes, so this is a plain addition, not a rotation.
local function implNodeWorld(vehPos, cid)
  return vehPos + vec3(obj:getNodePosition(cid))
end

-- Height of a point above whatever is under it, or nil when nothing sane is found. Two
-- independent sources so a map with no terrain block still reports off its static geometry.
local function implSurfaceDrop(p)
  local best = nil
  local h = obj:getSurfaceHeightBelow(p)
  if h then
    local d = p.z - h
    if d >= IMPL_SANE_GROUND_MIN and d <= IMPL_SANE_GROUND_M then best = d end
  end
  local ray = obj:castRayStatic(p, vec3(0, 0, -1), IMPL_SANE_GROUND_M)
  if ray and ray > 0 and ray < IMPL_SANE_GROUND_M then
    if not best or ray < best then best = ray end
  end
  return best
end

-- Magnitude of the frame articulation in degrees, measured as the yaw between the front and
-- rear axle axes. The SIGN is taken from the already-trusted normalised articulation rather
-- than from a cross product, so this can never disagree with the steering tone.
local function implArticulationDeg(articNorm)
  resolveImplementWheels()
  local mag = nil
  if _implFrontWheels and _implRearWheels then
    local up = vec3(obj:getDirectionVectorUp())
    local function axleAxis(list)
      local acc, n = vec3(0, 0, 0), 0
      local ref = nil
      for _, wd in ipairs(list) do
        if wd.node1 and wd.node2 then
          local a = vec3(obj:getNodePosition(wd.node2)) - vec3(obj:getNodePosition(wd.node1))
          a = a - up * a:dot(up)
          if a:length() > 1e-4 then
            a = a:normalized()
            -- node1->node2 runs inboard/outboard per wheel, so flip each axis onto a
            -- common side before averaging or the pair cancels to nothing.
            if not ref then ref = a elseif a:dot(ref) < 0 then a = -a end
            acc = acc + a
            n = n + 1
          end
        end
      end
      if n == 0 or acc:length() < 1e-4 then return nil end
      return acc:normalized()
    end
    local fa, ra = axleAxis(_implFrontWheels), axleAxis(_implRearWheels)
    if fa and ra then
      mag = math.deg(math.acos(math.max(-1, math.min(1, math.abs(fa:dot(ra))))))
    end
  end
  if not mag then mag = math.abs(articNorm) * IMPL_ARTIC_FALLBACK_DEG end
  return (articNorm >= 0) and mag or -mag
end

-- Refill the cached implement outputs. Called at IMPL_TICK_INTERVAL, inside a pcall.
local function updateImplement(dt, articulation)
  local flags = 0
  if articulation then flags = flags + IMPL_FLAG_ARTIC_VALID end
  _implArticDeg = articulation and implArticulationDeg(articulation) or 0.0

  findImplementCylinders()
  resolveImplementNodes()

  if not (_implEdgeCids and (_implLiftCyls or _implTiltCyls)) then
    _implFlags = flags
    _implEdgeH, _implMinClear = -1.0, -1.0
    _implTiltDeg, _implTiltPct, _implLift, _implActivity = 0.0, 0.0, 0.0, 0.0
    return
  end
  flags = flags + IMPL_FLAG_PRESENT

  local vehPos = vec3(obj:getPosition())

  -- Ground clearance. Two numbers on purpose: the edge height is what a driver wants read
  -- out, the minimum over all five points is what the tone must react to, because the corner
  -- about to dig in is not necessarily the cutting edge.
  local edgeSum, edgeN, minClear = 0, 0, nil
  for _, cid in ipairs(_implEdgeCids) do
    local d = implSurfaceDrop(implNodeWorld(vehPos, cid))
    if d then
      edgeSum, edgeN = edgeSum + d, edgeN + 1
      if not minClear or d < minClear then minClear = d end
    end
  end
  for _, cid in ipairs(_implHeelCids) do
    local d = implSurfaceDrop(implNodeWorld(vehPos, cid))
    if d and (not minClear or d < minClear) then minClear = d end
  end
  if edgeN > 0 and minClear then
    _implEdgeH, _implMinClear = edgeSum / edgeN, minClear
    flags = flags + IMPL_FLAG_GROUND_VALID
  else
    _implEdgeH, _implMinClear = -1.0, -1.0
  end

  -- Tilt, measured in world space so it stays right on a slope -- which is the case that
  -- decides whether a load spills. edgeMid -> heelMid is the implement's FLOOR plane (see
  -- the floor-band constraint in resolveImplementNodes), so its world pitch is directly the
  -- angle from level and needs no design-pose offset: 0 means level tines, positive means
  -- the edge is above the heel, i.e. racked back.
  local function worldMid(list)
    local acc = vec3(0, 0, 0)
    for _, cid in ipairs(list) do acc = acc + implNodeWorld(vehPos, cid) end
    return acc / #list
  end
  -- edgeL/edgeR only, NOT all three edge picks. The GE-side entry gate measures this same
  -- axis from getImplementFrame, whose origin is the edgeL/edgeR midpoint, and two halves of
  -- one instrument reporting angles that differ by a degree or two is the kind of
  -- disagreement that costs an afternoon to chase. edgeC is a contact point, not a centre.
  local A, B = worldMid({_implEdgeCids[1], _implEdgeCids[3]}), worldMid(_implHeelCids)
  local len = A:distance(B)
  if len > 1e-4 then
    _implTiltDeg = math.deg(math.asin(math.max(-1, math.min(1, (A.z - B.z) / len))))
  end

  if _implTiltCyls then
    local s, n = 0, 0
    for _, cyl in ipairs(_implTiltCyls) do s, n = s + cyl.currentExtendPercent, n + 1 end
    _implTiltPct = s / n
  end

  -- Detach test: the implement is coupler-welded to the lift arm and can come off as a unit,
  -- at which point every number above is describing something lying in the dirt.
  if _implRefCid then
    local live = implNodeWorld(vehPos, _implRefCid):distance(implNodeWorld(vehPos, _implEdgeCids[2]))
    if live > _implRefDist + IMPL_DETACH_SLOP_M then flags = flags + IMPL_FLAG_DETACHED end
  end

  -- Activity and ram-stall, in one walk. The rate term is what makes gravity droop and load
  -- settling count as movement, and it is what carries the tone gate across a momentary
  -- joystick release without needing a hold timer.
  _implPrevExtend = _implPrevExtend or {}
  _implRateSmooth = _implRateSmooth or {}
  local act, stalled = 0.0, false
  local function walk(list)
    for _, cyl in ipairs(list) do
      local cmd = math.abs(electrics.values[cyl.directionElectricsName] or 0)
      if cmd > act then act = cmd end
      local prev = _implPrevExtend[cyl]
      local rate = 0
      if prev and dt > 0 then rate = math.abs(cyl.currentExtendPercent - prev) / dt end
      _implPrevExtend[cyl] = cyl.currentExtendPercent
      -- currentExtendPercent is derived from live beam lengths, so on a soft-body machine
      -- it is NEVER perfectly still: the rams breathe with engine vibration and load
      -- settling even when parked. Smooth the rate and then take a deadband out of it, or
      -- that permanent jitter reads as "being operated" and the tones pump in and out
      -- continuously while the loader sits there. Full-speed travel is around 0.5/s, so a
      -- deadband at IMPL_ACT_RATE_DEADBAND is a small fraction of any real movement.
      local sm = _implRateSmooth[cyl] or rate
      sm = sm + (1.0 - math.exp(-dt / IMPL_ACT_RATE_TAU)) * (rate - sm)
      _implRateSmooth[cyl] = sm
      local rateAct = (sm - IMPL_ACT_RATE_DEADBAND)
                      / (IMPL_ACT_RATE_FULL - IMPL_ACT_RATE_DEADBAND)
      if rateAct > act then act = math.min(1.0, math.max(0.0, rateAct)) end
      -- A ram that refuses to move while it is being commanded hard is, by definition,
      -- against a load. This is the reliable jacking signal; wheel force is not, because a
      -- loader's front axle load swings several-fold between an empty and a full bucket.
      if cmd > IMPL_STALL_CMD and prev and rate < IMPL_STALL_RATE then stalled = true end
    end
  end
  if _implLiftCyls then walk(_implLiftCyls) end
  if _implTiltCyls then walk(_implTiltCyls) end
  _implActivity = math.min(1.0, act)

  -- Jacking height. Measured off the surface machinery rather than off force: track the
  -- front axle's resting height while the wheels are loaded, then report anything above it.
  -- A cold start already mid-jack reads 0 until the machine has sat on its wheels once.
  resolveImplementWheels()
  _implLift = 0.0
  if _implFrontWheels then
    local zSum, zN, loaded = 0, 0, true
    for _, wd in ipairs(_implFrontWheels) do
      if wd.node1 then
        zSum, zN = zSum + implNodeWorld(vehPos, wd.node1).z, zN + 1
        if (wd.downForce or 0) <= 0 then loaded = false end
      end
    end
    if zN > 0 then
      local axleZ = zSum / zN
      local mid = vec3(0, 0, 0)
      for _, wd in ipairs(_implFrontWheels) do
        if wd.node1 then mid = mid + implNodeWorld(vehPos, wd.node1) end
      end
      mid = mid / zN
      local drop = implSurfaceDrop(mid)
      if drop then
        local clear = axleZ - (mid.z - drop)
        if loaded then
          local beta = 1.0 - math.exp(-dt / IMPL_REST_TAU)
          _implRestClear = _implRestClear and (_implRestClear + beta * (clear - _implRestClear)) or clear
        elseif _implRestClear and stalled then
          _implLift = math.max(0.0, clear - _implRestClear)
          if _implLift > 0.01 then flags = flags + IMPL_FLAG_JACKING end
        end
      end
    end
  end

  _implFlags = flags
end

local function tryCouplerHookWrap()
  if _couplerHooksWrapped then return end
  if not extensions or not extensions.couplings then return end
  local couplings = extensions.couplings
  -- Already wrapped by an earlier incarnation of this module: adopt it, don't re-wrap.
  if couplings[COUPLER_HOOK_MARK] then
    _couplerHooksWrapped = true
    return
  end
  -- The vehicle id is carried so the GE side can ignore vehicles the driver is not in.
  -- Every spawned VM runs this protocol and wraps its own couplings table, so without it
  -- a trailer's own auto-coupling would announce as though it were the player's.
  local notify = "if extensions.vehicleScanner then extensions.vehicleScanner.onCouplerModeChange(%s, "
  local origActivate = couplings.onBeamstateActivateAutoCoupling
  couplings.onBeamstateActivateAutoCoupling = function(...)
    if origActivate then origActivate(...) end
    obj:queueGameEngineLua(string.format(notify .. "true) end", objectId))
  end
  local origDisable = couplings.onBeamstateDisableAutoLatching
  couplings.onBeamstateDisableAutoLatching = function(...)
    if origDisable then origDisable(...) end
    obj:queueGameEngineLua(string.format(notify .. "false) end", objectId))
  end
  couplings[COUPLER_HOOK_MARK] = true
  _couplerHooksWrapped = true
end

local function init()
  local shiftLightControllers = controller.getControllersByType("shiftLights")
  hasShiftLights = shiftLightControllers and #shiftLightControllers > 0
  tryCouplerHookWrap()
end

local function destroy() end

local function reset()
  -- Clearing the LOCAL only re-arms the retry harness; the mark on extensions.couplings
  -- is what stops this becoming a second wrapper. See COUPLER_HOOK_MARK.
  _couplerHooksWrapped = false
  tryCouplerHookWrap()
  -- Parts (and therefore the hydraulic cylinders) can change across a reset.
  _steerCylinders = nil
  _steerCylScanned = false
  _steerCylOpposed = false
  _steerCylTries = 0
  -- Same reasoning for the implement: a part swap and a coupler break both arrive here, and
  -- both invalidate the cached cids, the tilt zero and the resting-clearance calibration.
  _implCylScanned = false
  _implCylTries = 0
  _implCylDiagLogged = false
  _implLiftCyls = nil
  _implTiltCyls = nil
  _implNodesScanned = false
  _implNodeTries = 0
  _implPushed = false
  _implPushAt = -1e9
  _implPushName = ""
  _implPushCids = nil
  _implDiagLogged = false
  _implEdgeCids = nil
  _implHeelCids = nil
  _implTiltZeroDeg = 0.0
  _implRefCid = nil
  _implRefDist = 0.0
  _implFrontWheels = nil
  _implRearWheels = nil
  _implWheelsScanned = false
  _implAccum = 0.0
  _implPrevExtend = nil
  _implRateSmooth = nil
  _implRestClear = nil
  _implFlags = 0
  _implEdgeH = -1.0
  _implMinClear = -1.0
  _implTiltDeg = 0.0
  _implTiltPct = 0.0
  _implLift = 0.0
  _implActivity = 0.0
  _implArticDeg = 0.0
end
local function getAddress()        return "127.0.0.1" end
local function getPort()           return "4444" end
local function getMaxUpdateRate()  return 60 end

local function isPhysicsStepUsed()
  return false
end

local function getStructDefinition()
  return [[
    // --- Basic Data ---
    unsigned short flags;
    char           gear[4];
    char           plid;
    float          speed;
    float          rpm;
    float          rpmMax;
    float          turbo;
    float          turboMax;
    float          engTemp;
    float          fuel;
    float          oilPressure;
    float          oilTemp;
    unsigned       dashLights;
    unsigned       showLights;

    // --- Inputs ---
    float          throttle;
    float          brake;
    float          clutch;
    float          steering;
    float          actualSteering;
    float          steeringInput;    // electrics.values.steering_input (raw driver input)

    // --- Pneumatics ---
    float          airPressure;
    float          airPressureMax;
    
    // --- Expanded Telemetry ---
    float          clutchTemperature; // C
    float          g_lat;             // Lateral G-Force
    float          g_lon;             // Longitudinal G-Force

    // --- Tire Pressures (PSI) ---
    float          tirepressure_FL;
    float          tirepressure_FR;
    float          tirepressure_RL;
    float          tirepressure_RR;

    // --- Tire Temperatures (C) ---
    float          tiretemp_FL;
    float          tiretemp_FR;
    float          tiretemp_RL;
    float          tiretemp_RR;

    // --- Brake Temperatures (C) ---
    float          braketemp_FL;
    float          braketemp_FR;
    float          braketemp_RL;
    float          braketemp_RR;

    // --- Signal Inputs (0 or 1) ---
    float          signal_left_input;
    float          signal_right_input;
    float          hazard_enabled;

    // --- Lightbar (0=off, 1=lights, 2=lights+siren) ---
    float          lightbar;

    // --- Fog Lights (0=off, 1=on) ---
    float          fog;

    // --- Loader implement (bucket / forks). All zero/-1 on ordinary vehicles. ---
    float          implementFlags;        // bitmask: 1 present, 2 artic valid,
                                          // 4 ground valid, 8 jacking, 16 detached
    float          implementEdgeHeight;   // m, cutting edge above the surface (-1 unknown)
    float          implementMinClearance; // m, nearest of 5 sample points (-1 unknown)
    float          implementTiltDeg;      // degrees from level, + = curled back / up
    float          implementTiltPercent;  // 0..1 of tilt ram travel
    float          implementLift;         // m the machine has been jacked off its wheels
    float          implementActivity;     // 0..1, drives the Python tone fade gate
    float          articulationDeg;       // degrees of frame bend, + = LEFT

    // --- Centred wheels and wheel layout (appended for packet compatibility) ---
    float          tirepressure_F;
    float          tirepressure_R;
    float          tiretemp_F;
    float          tiretemp_R;
    float          braketemp_F;
    float          braketemp_R;
    unsigned       telemetryPresence;     // FL=1, FR=2, RL=4, RR=8, F=16, R=32,
                                          // clutch temperature valid=64
  ]]
end

-- OG_x and DL_x constants...
local OG_TURBO =  8192
local OG_KM    = 16384
local OG_BAR   = 32768
local DL_SHIFT        = 2 ^ 0
local DL_FULLBEAM     = 2 ^ 1
local DL_HANDBRAKE    = 2 ^ 2
local DL_TC           = 2 ^ 4
local DL_SIGNAL_L     = 2 ^ 5
local DL_SIGNAL_R     = 2 ^ 6
local DL_CHECK        = 2 ^ 7
local DL_OILWARN      = 2 ^ 8
local DL_BATTERY      = 2 ^ 9
local DL_ABS          = 2 ^ 10
local DL_LOWBEAM      = 2 ^ 11

local WHEEL_POSITION_FLAGS = {
  FL = 1,
  FR = 2,
  RL = 4,
  RR = 8,
  F  = 16,
  R  = 32,
}

local function fillStruct(o, dtSim)
  if not _couplerHooksWrapped then tryCouplerHookWrap() end
  -- Only emit telemetry from the active player vehicle. Otherwise every
  -- spawned vehicle (trailers, traffic, AI cars) runs this 60Hz pipeline and
  -- floods the Python listener on port 4444 with overlapping packets.
  -- `be` is a Game Engine global and is not available in the vehicle VM, so
  -- use the per-vehicle `playerInfo` table populated by BeamNG instead.
  if not (playerInfo and playerInfo.firstPlayerSeated) then
    return false
  end
  if not electrics.values.watertemp then
    return false
  end

  o.flags = OG_KM + OG_BAR + (electrics.values.turboBoost and OG_TURBO or 0)
  o.gear = string.sub(electrics.values.gear .. "\0\0\0", 1, 4)
  o.plid = 0
  o.speed = electrics.values.wheelspeed or electrics.values.airspeed or 0
  o.rpm = electrics.values.rpm or 0
  o.rpmMax = electrics.values.maxrpm or 0
  o.turbo = (electrics.values.turboBoost or 0) / 14.504 -- BAR
  o.turboMax = (electrics.values.turboBoostMax or 0) / 14.504 -- BAR
  o.oilPressure = (powertrain.engine and powertrain.engine.oilPressure or 0) / 14.504 -- PSI to BAR
  o.engTemp = electrics.values.watertemp or 0
  o.fuel = electrics.values.fuel or 0
  o.oilTemp = electrics.values.oiltemp or 0

  o.dashLights = 0
  o.showLights = 0
  o.dashLights = bit.bor(o.dashLights, DL_FULLBEAM ) if electrics.values.highbeam      ~= 0 then o.showLights = bit.bor(o.showLights, DL_FULLBEAM ) end
  o.dashLights = bit.bor(o.dashLights, DL_HANDBRAKE) if electrics.values.parkingbrake  ~= 0 then o.showLights = bit.bor(o.showLights, DL_HANDBRAKE) end
  o.dashLights = bit.bor(o.dashLights, DL_SIGNAL_L ) if electrics.values.signal_L      ~= 0 then o.showLights = bit.bor(o.showLights, DL_SIGNAL_L ) end
  o.dashLights = bit.bor(o.dashLights, DL_SIGNAL_R ) if electrics.values.signal_R      ~= 0 then o.showLights = bit.bor(o.showLights, DL_SIGNAL_R ) end
  o.dashLights = bit.bor(o.dashLights, DL_CHECK ) if electrics.values.checkengine ~= false then o.showLights = bit.bor(o.showLights, DL_CHECK ) end
  if electrics.values.hasABS then
    o.dashLights = bit.bor(o.dashLights, DL_ABS    ) if electrics.values.absActive     ~= 0 then o.showLights = bit.bor(o.showLights, DL_ABS      ) end
  end
  o.dashLights = bit.bor(o.dashLights, DL_OILWARN  ) if electrics.values.oil           ~= 0 then o.showLights = bit.bor(o.showLights, DL_OILWARN  ) end
  o.dashLights = bit.bor(o.dashLights, DL_BATTERY  ) if electrics.values.engineRunning == 0 then o.showLights = bit.bor(o.showLights, DL_BATTERY  ) end
  if electrics.values.hasESC then
    o.dashLights = bit.bor(o.dashLights, DL_TC     ) if electrics.values.esc           ~= 0 or electrics.values.tcs ~= 0 then o.showLights = bit.bor(o.showLights, DL_TC       ) end
  end
  if hasShiftLights then
    o.dashLights = bit.bor(o.dashLights, DL_SHIFT  ) if electrics.values.shouldShift        then o.showLights = bit.bor(o.showLights, DL_SHIFT    ) end
  end
  o.dashLights = bit.bor(o.dashLights, DL_LOWBEAM ) if electrics.values.lowbeam      ~= 0 then o.showLights = bit.bor(o.showLights, DL_LOWBEAM ) end

  o.throttle = electrics.values.throttle or 0
  o.brake = electrics.values.brake or 0
  o.clutch = electrics.values.clutch or 0
  o.steering      = electrics.values.steering
  o.steeringInput = electrics.values.steering_input or 0
  -- Where the wheels/frame actually are, as opposed to where the driver is asking them
  -- to go. Non-zero only on vehicles whose steering does not self-centre, which is what
  -- lets the Python side auto-detect them. -1..1, POSITIVE = LEFT.
  local articulation = getCylinderArticulation()
  -- Whatever articulation source won, remembered for the implement block. Distinct from
  -- actualSteering being 0, which on an ordinary car means "no such thing" rather than
  -- "straight" -- the Python side needs to tell those two apart to know whether to offer
  -- the Frame Articulation readout at all.
  local articValid = articulation

  if articulation then
      o.actualSteering = articulation
  elseif hydros and hydros.hydros then
      -- Fallback: a classic hydro-based steering actuator that never returns to centre.
      o.actualSteering = 0
      for _, h in pairs(hydros.hydros) do
        -- Must be an explicit false: input.lua treats nil as autocentering ENABLED,
        -- so a nil here means an ordinary car whose wheels do self-centre.
        if h.inputSource == "steering_input" and h.steeringAutocenterEnabled == false then
          local range = math.max(math.abs(h.inLimit or 1), math.abs(h.outLimit or 1))
          o.actualSteering = (h.state or 0) / range  -- normalize to -1..1
          articValid = o.actualSteering
          break
        end
      end
  else
      o.actualSteering = 0
  end
  
  local currentPa = 0
  local maxPa = 0
  local primaryTankName = nil
  local tanks = controller.getControllersByType("pressureTank")
  if tanks then
    for _, tank in pairs(tanks) do
      if tank.storage and tank.storage.isPrimarySupply then
        primaryTankName = tank.storage.name
        maxPa = tank.storage.maxWorkingPressure or 0
        break
      end
    end
    if primaryTankName then
      local electrics_key = primaryTankName .. "_pressureRelative"
      currentPa = electrics.values[electrics_key] or 0
    end
  end

  -- Fallback for vehicles without a pressureTank controller (e.g. T-Series):
  -- scan electrics.values once, prefer keys that contain "tank", cache the result.
  if currentPa == 0 and not _airKeySearched then
    if next(electrics.values) then          -- wait until electrics is populated
      _airKeySearched = true
      for k, v in pairs(electrics.values) do
        if type(k) == "string" and k:match("_pressureRelative$")
            and type(v) == "number" and v > 0 then
          if k:lower():find("tank") then
            _airPressureKey = k
            break                           -- "tank" key is best, stop immediately
          elseif not _airPressureKey then
            _airPressureKey = k             -- keep first match as fallback
          end
        end
      end
    end
  end
  if currentPa == 0 and _airPressureKey then
    currentPa = electrics.values[_airPressureKey] or 0
  end

  o.airPressure    = math.max(0, currentPa / 6894.76)  -- PSI, non-negative
  o.airPressureMax = maxPa / 6894.76                    -- PSI (0 = unknown)

  -- Get data from direct, reliable sources
  -- Friction and centrifugal clutches keep their thermal state on the device itself as
  -- clutchTemperature; there is no powertrain.clutch.temperature aggregate. Ignore
  -- clutch-category devices such as hydraulic pumps which expose no thermal field. If a
  -- vehicle has multiple thermal clutches, the hottest one is the actionable reading.
  local clutchTemperature = nil
  if powertrain and powertrain.getDevicesByCategory then
    for _, device in ipairs(powertrain.getDevicesByCategory('clutch')) do
      local value = device.clutchTemperature
      if type(value) == 'number'
          and (clutchTemperature == nil or value > clutchTemperature) then
        clutchTemperature = value
      end
    end
  end
  o.clutchTemperature = clutchTemperature or 0
  o.g_lat = sensors.gy2 or 0 -- Lateral Gs
  o.g_lon = sensors.gx2 or 0 -- Longitudinal Gs

  -- Get Tire and Brake data by iterating through the wheels
  local pressures = {}
  local tireTemps = {}
  local brakeTemps = {}
  local telemetryPresence = clutchTemperature ~= nil and 64 or 0
  if wheels and wheels.wheels then
    for _, wd in pairs(wheels.wheels) do
      local pressurePa = 0
      if wd.pressureGroup and v.data.pressureGroups and v.data.pressureGroups[wd.pressureGroup] then
        pressurePa = obj:getGroupPressure(v.data.pressureGroups[wd.pressureGroup]) - (powertrain.currentEnvPressure or 101325)
      end
      pressures[wd.name] = (pressurePa > 0 and pressurePa or 0) * 0.000145038 -- PSI
      tireTemps[wd.name] = (wd.thermals and wd.thermals.tireTemperature) or 0
      -- BeamNG exposes separate surface and core temperatures. Surface temperature is
      -- the live, driver-relevant brake reading used by the stock racing display.
      brakeTemps[wd.name] = wd.brakeSurfaceTemperature or 0
      local positionFlag = WHEEL_POSITION_FLAGS[wd.name]
      if positionFlag then telemetryPresence = bit.bor(telemetryPresence, positionFlag) end
    end
  end
  
  o.tirepressure_FL = pressures.FL or 0
  o.tirepressure_FR = pressures.FR or 0
  o.tirepressure_RL = pressures.RL or 0
  o.tirepressure_RR = pressures.RR or 0

  o.tiretemp_FL = tireTemps.FL or 0
  o.tiretemp_FR = tireTemps.FR or 0
  o.tiretemp_RL = tireTemps.RL or 0
  o.tiretemp_RR = tireTemps.RR or 0
  
  o.braketemp_FL = brakeTemps.FL or 0
  o.braketemp_FR = brakeTemps.FR or 0
  o.braketemp_RL = brakeTemps.RL or 0
  o.braketemp_RR = brakeTemps.RR or 0

  o.tirepressure_F = pressures.F or 0
  o.tirepressure_R = pressures.R or 0
  o.tiretemp_F = tireTemps.F or 0
  o.tiretemp_R = tireTemps.R or 0
  o.braketemp_F = brakeTemps.F or 0
  o.braketemp_R = brakeTemps.R or 0
  o.telemetryPresence = telemetryPresence

  o.signal_left_input  = electrics.values.signal_left_input or 0
  o.signal_right_input = electrics.values.signal_right_input or 0
  o.hazard_enabled     = electrics.values.hazard_enabled or 0
  o.lightbar           = electrics.values.lightbar or 0
  o.fog                = electrics.values.fog or 0

  -- Implement block. Throttled to IMPL_TICK_INTERVAL because it raycasts, and wrapped in a
  -- pcall because this runs on the physics thread: a throw here would take the whole
  -- telemetry protocol down, not just the loader features.
  _implAccum = _implAccum + (dtSim or 0)
  _implClock = _implClock + (dtSim or 0)
  -- Heartbeat the cid list so a game-engine reload or a vehicle switch self-heals. Only
  -- once something was actually found: see pushImplementToGE.
  if _implPushCids and (_implClock - _implPushAt) >= IMPL_PUSH_HEARTBEAT_S then
    pcall(pushImplementToGE, nil, _implPushName, _implPushCids)
  end
  -- Outside the IMPL_TICK_INTERVAL block on purpose: that one is throttled because it
  -- RAYCASTS, and this walks a handful of already-resolved device tables. It has its own,
  -- looser throttle and its own change test, so on a machine that is not moving a ram it
  -- costs one table walk every 200 ms and sends nothing at all.
  pcall(pushRampHydraulics)
  if _implAccum >= IMPL_TICK_INTERVAL then
    local dt = _implAccum
    _implAccum = 0
    local ok, err = pcall(updateImplement, dt, articValid)
    if not ok then
      log('E', 'beamtel.implement', 'implement update failed: ' .. tostring(err))
      -- Latch it off rather than throwing every tick; a reset re-arms the scan.
      _implEdgeCids = nil
      _implNodesScanned = true
      _implFlags = 0
    end
  end

  o.implementFlags        = _implFlags
  o.implementEdgeHeight   = _implEdgeH
  o.implementMinClearance = _implMinClear
  o.implementTiltDeg      = _implTiltDeg
  o.implementTiltPercent  = _implTiltPct
  o.implementLift         = _implLift
  o.implementActivity     = _implActivity
  o.articulationDeg       = _implArticDeg
  return true
end

M.init = init
M.destroy = destroy
M.reset = reset
M.getAddress = getAddress
M.getPort = getPort
M.getMaxUpdateRate = getMaxUpdateRate
M.getStructDefinition = getStructDefinition
M.fillStruct = fillStruct
M.isPhysicsStepUsed = isPhysicsStepUsed

return M
