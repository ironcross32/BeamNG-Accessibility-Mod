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
-- ...and the PART tier, which is what every ramp that is not large_cannon's needs.
--
-- A scan of all 124 stock vehicle zips says the node-name tier above resolves exactly one
-- drivable thing: large_cannon. Every other stock ramp names its nodes opaquely -- the tilt
-- deck's are b0rr/b1r/b11rr, the dry van's cd1rr, the rollback's tf01r -- and carries its
-- identity only in the jbeam PART it came from. So a second tier matches nd.partOrigin (the
-- part NAME, proven present and a plain string; the implement resolver in 796F6C6F313035.lua
-- already rests on it) and, failing that, nd.group.
--
-- An ALLOWLIST rather than a substring test for "ramp", because the substring test is wrong in
-- both directions on the shipped data. It misses every entry below except the cannon's, and it
-- hits tiltframe_rampiston and tiltframe_ramcylinder, which are the hydraulic ram that TILTS a
-- rollback deck -- structure you must not drive at.
--
-- Verified against the shipped jbeam, one line per source file:
--   cannon_ramp            large_cannon/large_cannon_ramp.jbeam        (also tier 1)
--   tsfb_ramp              tsfb/tsfb_ramp.jbeam                        tandem semi flatbed
--   dryvan_ramp            dryvan/dryvan_ramp.jbeam                    dry van
--   dryvan_rampextension   dryvan/dryvan_ramp.jbeam                    ...its fold-out tip
--   us_semi_rollback_deck  us_semi/rollback/us_semi_rollback_deck.jbeam
--   tiltdeck_deck          tiltdeck/tiltdeck_deck_{22,30,40}ft.jbeam   tilt deck trailer
--   us_semi_ramplow        us_semi/us_semi_ramplow.jbeam
--   md_series_ramplow      md_series/md_series_ramplow{,_small}.jbeam
--
-- The last two sit in a *_bumper_F slot rather than a rear one, so they are the entries most
-- likely to be wrong; they are also the first to drop if they ever misresolve in game.
--
-- `onramp` is the one GROUP-tier entry, and it is the drive-on end of the Wheel Roller's tilt
-- ramp (testroller/testroller_tiltramp.jbeam, groups onramp_R and onramp_L). It earns its place
-- by naming the way IN: the same jbeam also has offramp_R/offramp_L at the other end, which is
-- deliberately NOT allowed -- admitting it would put the cloud's centroid back on the machine
-- centroid, and the displacement rule that derives WHICH END is the mouth would collapse into
-- residue. A scan of all 124 stock vehicle zips finds `onramp` in exactly one vehicle and
-- `offramp` in the same one, so neither word can collide with anything shipped.
local RAMP_PART_WORDS = {
  "cannon_ramp", "tsfb_ramp", "dryvan_ramp", "dryvan_rampextension",
  "us_semi_rollback_deck", "tiltdeck_deck", "us_semi_ramplow", "md_series_ramplow",
  "onramp",
}
-- Checked BEFORE the allowlist, and as plain substrings rather than whole words, because each
-- of these is a near-miss the zip scan actually turned up rather than a hypothetical:
--   tailgate / door   cargotrailer, boxutility, frameless_dump -- hinged, not drivable
--   dumptruck_deck    the bed SIDES of a dump truck, despite the name
--   tiltframe_ram     the rollback's ram cylinder and piston, i.e. what moves the deck
--   spinner_wall      large_spinner's nodes are literally ramp_0..ramp_5b, so it passes the
--                     NAME tier today. It is a spinning wall. This closes a standing false
--                     positive rather than guarding against a new one.
local RAMP_PART_DENY = {
  "tailgate", "door", "dumptruck_deck", "tiltframe_ram", "spinner_wall",
}
-- DECLARED MOUTHS: machines whose way in cannot be INFERRED at all, named outright.
--
-- Everything above derives a mouth from a node cloud, and that only works where the cloud says
-- which end you drive into. Three shipped props defeat it in three different ways, and each was
-- found by asking rampTruth() next to the machine rather than by reading jbeam:
--
--   large_hamster_wheel  carries NO node groups whatsoever and exactly two partOrigin values,
--                        one of which (large_hamster_wheel_frame) is the drive-in ramps AND the
--                        whole A-frame together. There is no subset the tiers can name. Worse,
--                        it has TWO mouths -- a ramp at each end of its axle -- and a centroid
--                        displacement cannot point at both, so the one-mouth model cannot
--                        express the machine even given a perfect cloud.
--   testroller ramp      resolves through the `onramp` group above, and needs no entry here.
--   testroller multi     has no mouth at all: it is a pair of frictionless cradles you drive
--                        over. Deliberately NOT declared -- see the note below the table.
--
-- Five node NAMES per mouth, in the order mouthL, mouthC, mouthR, innerL, innerR, which is
-- exactly what cache[].cids holds, so a declaration bypasses the inference and changes nothing
-- downstream. L and R are the driver's own left and right ON THE WAY IN (the mod-wide
-- positive-is-LEFT convention), which for the hamster wheel's two ramps means the two entries
-- are mirrored rather than copied.
--
-- Node names are chosen for COLLISION, not for being furthest out. The hamster wheel's outermost
-- toe nodes are r02ll_2 and friends, one metre further out and 0.1 m lower -- and every one of
-- them carries collision = false, so they are not a surface anything can drive on and a floor
-- fitted through them would be a fiction. r02ll/r05ll/r08ll sit at z = 0 with collision on, so
-- both rows are flat, the derived pitch is 0, and the mouth is a true 6.74 m wide.
local DECLARED_MOUTHS = {
  large_hamster_wheel = {
    -- +X ramp: driving in runs toward -X, so the driver's left is -Y.
    { "r08ll", "r05ll", "r02ll", "r08l", "r02l" },
    -- -X ramp: the mirror. Driving in runs toward +X, so the driver's left is +Y.
    { "r02rr", "r05rr", "r08rr", "r02r", "r08r" },
  },
}
-- WHY THE WHEEL ROLLER IS NOT IN THAT TABLE, since it is the obvious third entry. Its only
-- candidate mouth is the pair of roller stations the car's wheels sit in, so the half-width a
-- declaration would publish is half the TRACK WIDTH -- against which every car is too wide, and
-- the align would announce "you do not fit" about a rig the car fits perfectly. The width margin
-- is not an optional part of the readout, so a declaration here would buy a square placement at
-- the cost of a confident wrong answer on the number beside it. It stays findable through the
-- vehicle scanner, which needs no mouth.

-- A floor, not a fit. The real large_cannon mouth row alone holds around seventeen nodes;
-- anything under eight is not a ramp and is not worth trying to find two end rows in.
local MIN_RAMP_NODES = 8
-- ...and a lower one for the part/group tier, because the two tiers are guarded differently.
-- Tier 1 is a PATTERN (anything called ramp_something), so its floor is the only thing standing
-- between the resolve and a handful of coincidentally-named nodes. Tier 2 is an ALLOWLIST of
-- part and group names verified against the shipped jbeam, so a match there is authored
-- evidence and needs far less protection from noise.
--
-- Six is not arbitrary: it is the least that still supports the two-row analysis everything
-- downstream is built on -- a mouth row of four and an inner row of two. The tilt ramp is
-- exactly that (on1r/on1rr/on1l/on1ll at the lip, on3r/on3l behind them) and it is six rather
-- than eight only because firstGroup drops the two nodes that are in roller_RR as well.
local MIN_RAMP_NODES_PART = 6
-- How many distinct partOrigin values a "no ramp here" reply carries back with it. The
-- allowlist above is a data question that only in-game testing settles, so the resolve has to
-- say what it DID see -- otherwise identifying a machine the list missed means a log dive on a
-- line that is emitted once, at D level, from another VM. lastReason is already rendered
-- verbatim by M.stateOf and by implementProximity.rampTruth(), so this costs no new plumbing.
local REASON_PARTS_MAX = 10
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
-- ...and an ABSOLUTE ceiling on what that fraction is allowed to produce.
--
-- A mouth is a mouth-sized thing. The fraction was tuned on large_cannon, whose ramp is a fixed
-- 5.792 m, so 0.35 of it is 2.03 m and the question of a cap never came up. It comes up the
-- moment the ramp is part of a machine that changes its own length: a us_semi rollback deck is
-- 9.24 m home and grows to about 12 m with the bed run out, which makes the "mouth row" a 4.2 m
-- slab. Measured on that vehicle at full extension, the two wall picks came out 1.49 m apart
-- ALONG the ramp -- not a row at all, and the live half-width then disagreed with the resolve's
-- own meta (1.168 m against 0.94 m) because one is a 3-D distance between two nodes at
-- different stations and the other is a pure lateral difference.
--
-- 2.0 m rather than something tighter because large_cannon's three mouth rows span 1.264 m and
-- must all stay in the band; at 2.0 the cannon's membership is unchanged (its uncapped band is
-- 2.03 m and there is nothing between 1.264 and 2.03 to admit or drop), so this cannot regress
-- the vehicle the rule was built on.
local ROW_BAND_MAX_M = 2.0
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
-- The along-ramp spread the centreline reference nodes must cover before their slope is
-- believed. Two nodes a centimetre apart define a line, but not one worth extrapolating a metre
-- up-ramp from: a modelling seam between them would be read as the whole ramp's pitch. Below
-- this the floor falls back to the flat lowest-z plane, which is what every vehicle used before.
-- large_cannon's mouth row spreads its centreline nodes over 1.0 m, i.e. 4x this.
local FLOOR_FIT_MIN_SPAN_M = 0.25
-- How far the centreline reference nodes may lie off their own fitted line before the fit is
-- disbelieved -- i.e. before this row is declared NOT ONE PLANE.
--
-- Deliberately equal to WALL_MIN_H, and that is the entire argument: a wall is a node more than
-- WALL_MIN_H above the floor, so if the floor itself is uncertain by more than WALL_MIN_H then
-- every wall this row could report is inside the fit's own error bar. Believing it anyway is
-- how the rule invents walls that are not there.
--
-- Which is exactly what it did. A us_semi rollback deck is TWO structural levels -- an
-- understructure at u 0.07-0.14 and the drivable surface at u 0.49-0.55 -- and the uncapped row
-- band above swept both into one row. The line then fitted through the middle of them, landing
-- the "floor" near u 0.30 where nothing physically exists, so every node of the real deck
-- surface read as 0.22 m of wall and the rule took the smallest lateral: the INNER rail at
-- 1.017 instead of the outer edge at 1.732. Measured half-width 0.936 m against a true 1.296 m,
-- at the HOME pose, with no tilt and no extension involved. The residual there is 0.23 m
-- against this 0.15 m, so the guard trips with margin; large_cannon's mouth row is a single
-- ramp floor and fits it to near zero.
local FLOOR_FIT_MAX_RESIDUAL_M = WALL_MIN_H
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
--
-- ...and when that displacement does not exist, the resolve falls to a DECK tier rather than
-- being rejected. See the axis block in the chunk below for why that is a second tier and not
-- a relaxation of this one.
local AXIS_DOMINANCE = 3.0
local MIN_AXIS_DISP_M = 1.0  -- ...and it must be a real displacement, not numerical residue
-- The deck tier's own guard: a drive-on deck is long and narrow, so its along-machine span must
-- beat its lateral span by this factor. large_spinner's wall -- the one thing besides the
-- cannon that clears the NAME tier -- is the shape this rejects. 1.5 rather than
-- AXIS_DOMINANCE's 3.0 because a 22 ft tilt deck is only about 2.7x, and a 6.7 m rollback deck
-- on a 2.5 m body is 2.7x too; 3.0 would reject both of the vehicles the tier exists for.
local DECK_LENGTH_DOMINANCE = 1.5
-- Degenerate baseline guard, mirroring the implement resolver's MIN_EDGE_WIDTH_M and there for
-- the same reason: a two-point baseline shorter than this points wherever soft-body jitter
-- says it does, and every angle derived from it inherits that.
local MIN_MOUTH_WIDTH_M = 0.30
-- Two nodes this close laterally are the SAME EDGE, so the station they sit at decides between
-- them rather than whichever pairs() reached first.
--
-- A row band holds several stations by design, and a deck's side rail runs through all of them
-- at the same lateral offset. The extremes are then a tie to within a millimetre, and a strict
-- comparison resolves it by table order -- which is how the mouth pair came out 1.45 m apart
-- ALONG the ramp on a rollback whose lateral half-width was by then perfectly correct. Nothing
-- in the meta could show it: halfW there is a pure lateral difference, while mouthFrame
-- measures the two nodes in 3-D, so the same resolve read 1.29 m on one side of the wire and
-- 1.42 m on the other, and the derived axis and centre were skewed with it.
--
-- The mouth row breaks ties toward the mouth end, the inner row toward the inside. Both are the
-- same rule: take the marker from the end of the ramp the row is supposed to represent.
local EDGE_TIE_M = 0.02

-- Copied from vehicleGeometry deliberately, so there is one retry cadence across the mod.
local RESOLVE_TIMEOUT_S = 3.0
local MAX_TRIES         = 3

-- cache[vehID] = {cids = {mouthL, mouthC, mouthR, innerL, innerR, ...more mouths...},
--                 halfW, alongSpan, floorU, nNodes, wallUsed, naiveHalfW, axisTier, isCannon,
--                 mouthCount, floorTrusted}
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
-- Why each vehicle is in the state it is in, kept for M.diag() and for the "no ramp near you"
-- readout. This resolve's failure mode is that it lands on nothing and says so once at D level,
-- in a log nobody is reading from the driver's seat; the difference between "that car has no
-- ramp", "the cannon's VM never answered" and "the mouth resolved but is 80 m away" is the
-- whole diagnosis, and none of it was reachable in game.
local lastReason = {}
-- ...and WHICH KIND of answer it is, which is not the same question. "It answered, and it has
-- no ramp" is a settled fact about the vehicle; "it never answered" is a fact about the attempt
-- and may not be true a minute from now. Collapsing both into `failed` and rendering both as
-- "GAVE UP" is the same class of confusion this file keeps running into: two states that want
-- different reactions from the driver, reported in identical words.
--   resolved | pending | none | silent | malformed | inactive | unasked
local stateKind = {}
-- Epoch below which a reply for this vehicle is genuinely stale, i.e. it was issued before the
-- last invalidation. NOT the same thing as "not the chunk we are currently waiting on" -- see
-- M.onRampGeometry.
local staleBefore = {}
local staleBeforeAll = 0
local epochCounter = 0

local function markStale(vehID)
  if vehID == nil then staleBeforeAll = epochCounter else staleBefore[vehID] = epochCounter end
end

local function staleFloor(vehID)
  local v = staleBefore[vehID] or 0
  if staleBeforeAll > v then v = staleBeforeAll end
  return v
end

-- A vehicle whose VM is not running cannot answer a queueLuaCommand at all. Asking anyway
-- spends the retry budget on silence and lands the vehicle in `failed` permanently, which is
-- indistinguishable from "this machine has no ramp" -- and the vehicle most likely to be
-- inactive is a big map prop like a cannon sitting at the far end of the map, i.e. exactly the
-- one this file exists for.
local function vehIsActive(veh)
  if not veh or not veh.getActive then return true end
  local ok, active = pcall(function() return veh:getActive() end)
  if not ok then return true end
  return active and active ~= 0
end

local function rgLog(level, msg) log(level, 'rampGeometry', msg) end

-- Renders a Lua list literal for embedding in VEH_SCRIPT. The chunk is built by concatenation
-- for the reason documented below -- it is itself the subject of an outer string.format -- so
-- the word lists have to travel as source text rather than as upvalues.
local function luaList(t)
  local parts = {}
  for i, w in ipairs(t) do parts[i] = '"' .. w .. '"' end
  return "{" .. table.concat(parts, ",") .. "}"
end

-- Same job for DECLARED_MOUTHS, which is a map of model name to a list of five-name mouths.
-- Rendered by concatenation rather than passed as a second format slot, because the chunk is
-- documented as having exactly ONE -- string.format with one argument and two slots is an error
-- rather than a repeat, and the failure would land in another VM.
local function luaDeclTable(t)
  local parts = {}
  for model, mouths in pairs(t) do
    local ms = {}
    for i, m in ipairs(mouths) do ms[i] = luaList(m) end
    parts[#parts + 1] = '["' .. model .. '"]={' .. table.concat(ms, ",") .. "}"
  end
  return "{" .. table.concat(parts, ",") .. "}"
end

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
-- ------------------------------------------------------------------------------------------
-- Part matching. NOT ONE PERCENT SIGN may appear anywhere below -- not in code, not in a
-- string, not in a comment. That rules out string.format AND every Lua character class, so no
-- alphanumeric class, no escaped hyphen, no gsub pattern of any kind. This whole chunk is the
-- subject of an outer string.format and a stray percent breaks the resolve at load time, in
-- another VM, silently -- which is precisely what the first draft of this comment did, by
-- spelling out the very classes it was warning against. Every test here is therefore a plain
-- find (the `true` fourth argument) or a byte comparison.
-- ------------------------------------------------------------------------------------------
local ALLOW = ]] .. luaList(RAMP_PART_WORDS) .. [[
local DENY  = ]] .. luaList(RAMP_PART_DENY) .. [[
local DECL_ALL = ]] .. luaDeclTable(DECLARED_MOUTHS) .. [[

local function isAlnum(b)
  if not b then return false end
  return (b >= 48 and b <= 57) or (b >= 65 and b <= 90) or (b >= 97 and b <= 122)
end

-- Boundary-aware, the same rule the implement resolver applies to its own keywords and for the
-- same reason: a bare substring test is what lets ramplow match a bumper and rampiston match a
-- hydraulic ram. A word counts only where it is bounded at both ends by a separator or by the
-- end of the string, so tiltdeck_deck matches tiltdeck_deck_22ft and does not match
-- tiltdeck_deck22ft. Unlike the implement resolver there is no camelCase-hump clause, because
-- every verified part name is snake_case; a camelCase ramp part would need one adding.
local function partIsRamp(s)
  if type(s) ~= "string" or s == "" then return false end
  local l = s:lower()
  for _, d in ipairs(DENY) do
    if l:find(d, 1, true) then return false end
  end
  for _, w in ipairs(ALLOW) do
    local from = 1
    while true do
      local a, b = l:find(w, from, true)
      if not a then break end
      if (not isAlnum(l:byte(a - 1))) and (not isAlnum(l:byte(b + 1))) then return true end
      from = a + 1
    end
  end
  return false
end

-- THERE IS NO nd.group AT RUNTIME, and the tier that read one could never fire.
--
-- The jbeam source really does write {"group":"onramp_R"} section headers, so a node table
-- carrying nd.group as a string or an array of them is the obvious reading -- and it is wrong.
-- The loader resolves group names to INDICES: v.data.groups is a name-to-index map and the
-- node carries nd.firstGroup, a number. Dumped live off a Wheel Roller tilt ramp, the node
-- fields are cid, firstGroup, fixed, frictionCoef, name, nodeMaterial, nodeWeight, partName,
-- partOrigin, partPath, pos, slotType -- no group of any kind. So tier 3 was dead code on
-- every vehicle in the game, silently: it cost one nil test per node and could only ever
-- return false, which reads in a log exactly like a machine that has no ramp.
--
-- firstGroup is nil for an ungrouped node rather than 0, which matters because 0 is a valid
-- index (`lift` holds it on this very vehicle) -- so the reverse map needs no sentinel.
--
-- It is FIRST group, though: a node declared in two groups reports only one, so a node in
-- ["roller_RR", "onramp_R"] answers roller_RR and drops out of the ramp cloud. That is why
-- the tilt ramp's mouth is six nodes and not eight, and it is a limit of the engine's data
-- rather than of this scan.
local GROUPNAME = {}
if v and v.data and type(v.data.groups) == "table" then
  for gname, gidx in pairs(v.data.groups) do
    if type(gname) == "string" and type(gidx) == "number" then GROUPNAME[gidx] = gname end
  end
end

-- partOrigin first, group second.
local function nodeIsRampPart(nd)
  if partIsRamp(nd.partOrigin) then return true end
  local gi = nd.firstGroup
  if type(gi) == "number" then
    local gname = GROUPNAME[gi]
    if gname and partIsRamp(gname) then return true end
  end
  return false
end

-- Sanitised for the reason string, which travels inside single quotes through a queueLuaCommand
-- and must not carry a quote, a comma or a percent. Byte-wise for the same no-percent reason.
local function safeName(s)
  local out = {}
  for i = 1, #s do
    local b = s:byte(i)
    if isAlnum(b) or b == 95 or b == 45 then
      out[#out + 1] = s:sub(i, i)
    else
      out[#out + 1] = "-"
    end
  end
  return table.concat(out)
end

local ok, err = pcall(function()
  if not (v.data and v.data.nodes) then return reply("", "", "no node data") end

  -- POSITIVE EVIDENCE THAT THIS MACHINE IS THE STOCK large_cannon, and it is not the same
  -- question as "does it have a drive-in ramp".
  --
  -- beamtel's firing readout used to key off M.has, on the reasoning that the only vehicle
  -- naming its nodes ramp_ WAS the cannon. The part tiers above ended that: they exist so a
  -- rollback, a tilt deck and a dry van resolve too, and every one of them then latched
  -- CANNON. Measured in a us_semi tc82s_rollback, F9 I answered "Inclination 100 percent,
  -- strength unknown" -- because the two aiming figures are read off electrics that only
  -- large_cannon's controller hijacks, so on a real truck the inclination is its ENGINE RPM
  -- over a thousand (the rollback's hydraulicsCombustionEngineControl raises idle to 1500,
  -- i.e. a pegged 100 percent the moment the pump engages) and the strength is a gear string
  -- that is not a percentage. It also made the rollback's own ramp readout unreachable,
  -- because the cannon branch of the alignment key wins before _dock_phrase_ramp is consulted.
  --
  -- The honest test is the controller that PUBLISHES those two values: large_cannon.jbeam
  -- declares ["large_cannon", {}], so the machine that can be aimed is exactly the machine
  -- carrying that controller. Same shape of argument as cannonGeometry demanding a beamType 7
  -- launcher beam rather than trusting a filename -- a capability check on the source of the
  -- readout, not a jbeam allowlist that needs maintaining. It costs nothing: v.data is already
  -- in hand and this rides back in the same reply.
  local isCannon = 0
  for _, c in pairs(v.data.controller or {}) do
    if type(c) == "table" and tostring(c.fileName) == "large_cannon" then isCannon = 1 end
  end

  -- ------------------------------------------------------------------------------------
  -- DECLARED MOUTHS come first and win outright, because they are the answer for machines
  -- whose cloud cannot produce one. Nothing below can reach a vehicle listed here, which is
  -- the point: the inference would return a confident wrong mouth rather than nothing.
  -- ------------------------------------------------------------------------------------
  local DECLARED = DECL_ALL[tostring(v.data.model)]
  if DECLARED then
    local byName = {}
    for _, nd in pairs(v.data.nodes) do
      if nd.name then byName[nd.name] = nd end
    end
    local cidList, trusted, missing = {}, 1, nil
    for _, m in ipairs(DECLARED) do
      for _, nm in ipairs(m) do
        local nd = byName[nm]
        if not nd then missing = nm break end
        cidList[#cidList + 1] = nd.cid
        -- A node you cannot touch is not a floor. Recorded rather than rejected, so a
        -- declaration that lands on non-collision structure degrades to "pitch unknown"
        -- instead of publishing an invented one.
        if nd.collision == false then trusted = 0 end
      end
      if missing then break end
    end
    if missing then
      -- A part configuration that does not carry a declared node is a real possibility and
      -- must not read as "this machine has no ramp": it names the node so the table can be
      -- fixed, which is the same argument the parts-seen list makes on the failure path.
      return reply("", "", "declared mouth names a node this configuration lacks: "
        .. safeName(missing))
    end
    -- NOTE THE SPACES, AND DO NOT REMOVE THEM. This whole chunk is a Lua long string, so two
    -- adjacent close-square-brackets END it wherever they appear -- and a nested subscript such
    -- as byName of DECLARED sub 1 sub 1 writes exactly that pair. The file then fails to compile
    -- hundreds of lines away, in the middle of unrelated code, with nothing pointing at the
    -- subscript that caused it. Same class of trap as the no-percent rule above, and it bites
    -- the same way: the first draft of THIS comment spelled the bracket pair out and broke the
    -- file it was warning about, exactly as the percent comment once did. Split the subscript
    -- into a local, and never write the pair -- not in code, not in a comment.
    local first = DECLARED[1]
    local mL = byName[ first[1] ].pos
    local mR = byName[ first[3] ].pos
    local iL = byName[ first[4] ].pos
    local iR = byName[ first[5] ].pos
    local cx, cy, cz = (mL.x + mR.x) * 0.5, (mL.y + mR.y) * 0.5, (mL.z + mR.z) * 0.5
    local ix, iy, iz = (iL.x + iR.x) * 0.5, (iL.y + iR.y) * 0.5, (iL.z + iR.z) * 0.5
    local dhw = math.sqrt((mL.x - mR.x) ^ 2 + (mL.y - mR.y) ^ 2 + (mL.z - mR.z) ^ 2) * 0.5
    local dsp = math.sqrt((ix - cx) ^ 2 + (iy - cy) ^ 2 + (iz - cz) ^ 2)
    -- wallUsed 2 and naiveHalfW equal to halfW: a declared mouth has no wall rule to report
    -- and no naive alternative to compare against, and saying so beats inventing a figure.
    local dmeta = tostring(dhw) .. "," .. tostring(dsp) .. ",0," .. tostring(#cidList)
      .. ",2," .. tostring(dhw) .. ",3," .. tostring(isCannon) .. "," .. tostring(trusted)
    return reply(table.concat(cidList, ","), dmeta,
      "declared mouth, " .. tostring(#DECLARED) .. " on this machine")
  end

  local fwd = vec3(obj:getDirectionVector())
  local up  = vec3(obj:getDirectionVectorUp())
  -- MUST be up:cross(fwd), never the negation. implementProximity projects onto its own
  -- lateral vector and compares the result against values measured along this one; if the two
  -- disagree every lateral reading mirrors, and nothing but a grep enforces it.
  local rgt = up:cross(fwd)

  -- Two candidate sets, filled in ONE pass. The node-name tier is tried first and wins
  -- outright when it is populated, so large_cannon resolves through exactly the code it always
  -- did and the part tier can never change an answer that already worked.
  local T = {
    {n = 0, c = {}, f = {}, r = {}, u = {}, k = {}, sf = 0, sr = 0, su = 0},  -- 1: node name
    {n = 0, c = {}, f = {}, r = {}, u = {}, k = {}, sf = 0, sr = 0, su = 0},  -- 2: part / group
  }
  -- `k` carries each kept node's collision flag alongside its projections, so the five chosen
  -- cids can be tested for being real surface at the end without a second pass over v.data.
  local function keep(t, cid, f, r, u, coll)
    local i = t.n + 1
    t.n = i
    t.c[i], t.f[i], t.r[i], t.u[i], t.k[i] = cid, f, r, u, coll
    t.sf, t.sr, t.su = t.sf + f, t.sr + r, t.su + u
  end

  local mn, mf, mr, mu = 0, 0, 0, 0
  local partSeen, partOrder = {}, {}
  for _, nd in pairs(v.data.nodes) do
    local lp = vec3(obj:getNodePosition(nd.cid))
    local f, r, u = fwd:dot(lp), rgt:dot(lp), up:dot(lp)
    mn = mn + 1
    mf, mr, mu = mf + f, mr + r, mu + u
    if nd.name and nd.name:find("]] .. RAMP_NODE_PAT .. [[") then
      keep(T[1], nd.cid, f, r, u, nd.collision)
    end
    if nodeIsRampPart(nd) then
      keep(T[2], nd.cid, f, r, u, nd.collision)
    end
    -- Collected unconditionally, because it is only ever read on the FAILURE path and that is
    -- precisely the path that has nothing else to say.
    if type(nd.partOrigin) == "string" and nd.partOrigin ~= ""
       and not partSeen[nd.partOrigin] and #partOrder < ]] .. REASON_PARTS_MAX .. [[ then
      partSeen[nd.partOrigin] = true
      partOrder[#partOrder + 1] = safeName(nd.partOrigin)
    end
  end
  if mn < 1 then return reply("", "", "no nodes") end

  local pick, tierName = nil, ""
  if T[1].n >= ]] .. MIN_RAMP_NODES .. [[ then
    pick, tierName = T[1], "node name"
  elseif T[2].n >= ]] .. MIN_RAMP_NODES_PART .. [[ then
    pick, tierName = T[2], "part"
  end
  if not pick then
    return reply("", "", "only " .. T[1].n .. " named and " .. T[2].n
      .. " part-matched ramp nodes; parts seen: " .. table.concat(partOrder, " "))
  end

  local rn = pick.n
  local rc, xf, xr, xu, rcoll = pick.c, pick.f, pick.r, pick.u, pick.k
  local sf, sr, su = pick.sf, pick.sr, pick.su

  mf, mr, mu = mf / mn, mr / mn, mu / mn
  sf, sr, su = sf / rn, sr / rn, su / rn
  local df, dr, du = sf - mf, sr - mr, su - mu
  local af, ar, au = math.abs(df), math.abs(dr), math.abs(du)

  -- Dominant axis of the centroid displacement, and TWO separate guards that used to be one.
  --
  -- Guard one: vertical is rejected outright. A node set displaced from the machine centroid
  -- mostly in Z is not a ramp we understand, and guessing would be worse than saying so. That
  -- is enforced structurally by the branch conditions below -- if au is the largest of the
  -- three, neither branch fires, disp stays nil, and we fall to the deck tier.
  --
  -- Guard two: `oth` is the OTHER HORIZONTAL component, never max(other, vertical). The axis
  -- this ratio protects is horizontal -- mouthFrame flattens it to vec3(raw.x, raw.y, 0) and
  -- the vertical survives only as the scalar pitchDeg -- so weighing it against a vertical
  -- quantity tests the answer against something the answer does not contain. Worse, vertical
  -- displacement carries no information about whether the along-axis is well determined: a
  -- ramp hangs low and a machine is tall, so every ramp on a tall machine has one. And on a
  -- machine whose ramp is DESIGNED to move in Z it is not even a constant. large_cannon tilts
  -- its whole assembly on the ramL/ramR cylinders; at full inclination the ramp swings down,
  -- au grows to 2.85 m against a displacement of 8.46 m, and the old form's ratio of 2.966
  -- missed the 3.0 threshold by about three centimetres. The cannon then resolved only while
  -- its barrel was near level -- true at spawn, so nothing ever saw it, but a reset, a part
  -- swap, a level reload or M.retry at elevation killed ramp mode for the session and
  -- reported NO RAMP ON IT. A raised tilt deck and a rollback with the bed up fail the same
  -- way. ramp_resolve_sim.lua scenario 8 asserts both halves against a rotated pose, and
  -- asserts the old form fails at an angle the machine can actually reach.
  local along, lat, disp, oth, axisTier
  if af >= ar and af >= au then
    along, lat, disp, oth = xf, xr, df, ar
  elseif ar >= af and ar >= au then
    along, lat, disp, oth = xr, xf, dr, af
  end
  local dm = 0
  if disp then dm = math.abs(disp) end
  if disp and dm >= ]] .. MIN_AXIS_DISP_M .. [[ and dm >= ]] .. AXIS_DOMINANCE .. [[ * oth then
    axisTier = 1
  else
    -- ==========================================================================================
    -- THE DECK TIER, and it is a SECOND tier rather than a relaxation of the first.
    --
    -- The displacement rule works because a bolt-on ramp hangs off one end of a much larger
    -- machine, so the ramp cloud's centroid is metres away from the machine's. A tilt deck is
    -- the exact opposite: the deck IS the trailer, its centroid sits on the machine's, dm is
    -- numerical residue, and the rule above rejects it. Loosening the rule to admit that would
    -- also admit every cloud whose displacement is residue for the ordinary reason -- it isn't
    -- a ramp -- and would put the cannon's correctness at risk to fix a case the cannon is not.
    --
    -- So: when there is no displacement to read, do not read one. Fall back to the two facts
    -- that hold for a deck and cannot be derived from the cloud alone -- the machine's own
    -- fore/aft axis, and the fact that you drive onto a trailer FROM BEHIND. Forcing the sign
    -- negative below is what makes the rearward end the mouth. That is right for the tilt deck,
    -- the rollback deck, the dry van ramp and the tsfb ramp alike.
    --
    -- No string.format anywhere in this chunk, not even in a comment: this text is itself the
    -- subject of an outer string.format, so a percent sign here breaks the resolve at load
    -- time, in another VM, silently. That is what the sim's format check is for.
    -- ==========================================================================================
    local fLo, fHi, rLo, rHi = math.huge, -math.huge, math.huge, -math.huge
    for i = 1, rn do
      if xf[i] < fLo then fLo = xf[i] end
      if xf[i] > fHi then fHi = xf[i] end
      if xr[i] < rLo then rLo = xr[i] end
      if xr[i] > rHi then rHi = xr[i] end
    end
    local fSpan, rSpan = fHi - fLo, rHi - rLo
    if fSpan < ]] .. MIN_AXIS_DISP_M .. [[
       or fSpan < ]] .. DECK_LENGTH_DOMINANCE .. [[ * rSpan then
      -- Every component by name, not a bare pair of numbers. The old form printed the winning
      -- displacement against `oth` with nothing to say which axis `oth` came from, and reading
      -- 8.46 vs 2.85 off a tilted cannon told you the ratio missed without telling you that the
      -- 2.85 was VERTICAL -- which is the entire diagnosis. dm is not printed because it is
      -- whichever of the first two won, and on the vertical-fallthrough path it is 0.
      return reply("", "", "along-axis not dominant (fore/aft " .. tostring(af)
        .. " m, lateral " .. tostring(ar) .. " m, vertical " .. tostring(au)
        .. " m; deck span " .. tostring(fSpan)
        .. " m vs " .. tostring(rSpan) .. " m wide)")
    end
    -- disp is forced negative rather than measured: the mouth is the REAR of the machine.
    along, lat, disp, oth, axisTier = xf, xr, -1, 0, 2
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
  -- Capped absolutely. A fraction alone tracks the length of whatever the ramp happens to be
  -- right now, and a rollback deck runs its own length out by half.
  local rowBand = span * ]] .. ROW_BAND .. [[
  if rowBand > ]] .. ROW_BAND_MAX_M .. [[ then rowBand = ]] .. ROW_BAND_MAX_M .. [[ end

  local mouth, inner = {}, {}
  for i = 1, rn do
    if t[i] >= tmax - rowBand then mouth[#mouth + 1] = i end
    if t[i] <= tmin + rowBand then inner[#inner + 1] = i end
  end
  if #mouth < 3 or #inner < 2 then
    return reply("", "", "end rows too sparse (" .. #mouth .. "/" .. #inner .. ")")
  end

  -- ===========================================================================================
  -- Floor of each row, as a LINE ALONG THE RAMP rather than a single lowest z. Every pick below
  -- classifies a node by its height above this, and getting that wrong picks different nodes.
  --
  -- The lowest-z form was pose-dependent, and badly. A row is a BAND, not a plane: the mouth
  -- row spans 1.26 m of a 5.79 m ramp -- the toe on the ground, the floor lip, the raised wall.
  -- Tilt the assembly and that along-extent turns into vertical extent, so "height above the
  -- lowest node in the row" starts reporting the ramp's PITCH as though it were wall height.
  -- On large_cannon the floor lip is 1.0 m up-ramp of the toe and 0.1 m above it, so past
  -- 2.9 degrees of tilt the lip alone clears WALL_MIN_H, and the wall rule then picks
  -- ramp_L_1a at 0.546 m instead of ramp_L_6a at 2.148 m. That is a mouth reported as a
  -- quarter of its true width -- the same class of confidently-wrong clearance the wall rule
  -- exists to prevent, arriving through the back door.
  --
  -- Fitting a line absorbs the pitch exactly, because a rotation maps a straight row to a
  -- straight row. The reference nodes are the ones within MIN_WALL_LATERAL_M of the centreline:
  -- the wall rule already refuses to call those walls, so treating them as floor is the same
  -- judgement, not a new one. Nodes more than WALL_MAX_H above the row's lowest are excluded --
  -- that is overhead structure by this file's own definition, and large_cannon has exactly one,
  -- ramp_M_0, sitting 2 m up on the centreline where it would otherwise drag the fit with it.
  --
  -- With too few reference nodes, or too little along-spread to define a slope, this falls back
  -- to the lowest-z plane, which is the behaviour every vehicle had before.
  -- ===========================================================================================
  -- Returns a fourth value now: whether the reference nodes actually LIE on the line that was
  -- fitted to them. A row can be two structural levels rather than one plane, and a
  -- least-squares line through both is a floor at a height where the machine has no floor.
  -- Everything below classifies nodes by their height above that line, so an incoherent fit
  -- does not degrade the answer, it inverts it -- see FLOOR_FIT_MAX_RESIDUAL_M.
  local function floorFitOf(row)
    local lo = math.huge
    for _, i in ipairs(row) do if xu[i] < lo then lo = xu[i] end end
    local n, st, su, stt, stu = 0, 0, 0, 0, 0
    local tLo, tHi = math.huge, -math.huge
    local ref = {}
    for _, i in ipairs(row) do
      if math.abs(xr[i]) <= ]] .. MIN_WALL_LATERAL_M .. [[
         and (xu[i] - lo) <= ]] .. WALL_MAX_H .. [[ then
        n = n + 1
        ref[n] = i
        st, su = st + t[i], su + xu[i]
        stt, stu = stt + t[i] * t[i], stu + t[i] * xu[i]
        if t[i] < tLo then tLo = t[i] end
        if t[i] > tHi then tHi = t[i] end
      end
    end
    local den = n * stt - st * st
    if n >= 2 and (tHi - tLo) >= ]] .. FLOOR_FIT_MIN_SPAN_M .. [[
       and math.abs(den) > 1e-9 then
      local k = (n * stu - st * su) / den
      local a = (su - k * st) / n
      -- COUNT the nodes that miss the line, do not take the worst one. A single reference node
      -- off the fit is an ordinary feature -- a spine down the middle of a trough, a modelling
      -- seam -- and condemning the whole row for it would send large_cannon down the two-level
      -- path over one rib. Two structural levels are not one outlier: they put a substantial
      -- SHARE of the reference set off the line, because both levels run the length of the row.
      -- On the rollback that share is 2 of 4; the ribbed cannon is 1 of 4 and stays coherent.
      local off = 0
      for j = 1, n do
        local i = ref[j]
        local d = xu[i] - (a + k * t[i])
        if d < 0 then d = -d end
        if d > ]] .. FLOOR_FIT_MAX_RESIDUAL_M .. [[ then off = off + 1 end
      end
      return a, k, lo, not (off >= 2 and off * 3 >= n)
    end
    -- The flat lowest-z plane, which is what every vehicle used before the fit existed. Reported
    -- as coherent because it is not a fit and has no residual to disbelieve; falling back here
    -- must keep behaving exactly as it always did.
    return lo, 0, lo, true
  end
  local mouthFitA, mouthFitK, mouthFloorU, mouthCoherent = floorFitOf(mouth)
  local innerFitA, innerFitK, innerFloorU, innerCoherent = floorFitOf(inner)
  -- Height above the fitted floor. mouthFloorU/innerFloorU stay the plain lowest z, because the
  -- naive half-width is the floor-band rule reproduced literally as a negative control and the
  -- meta field is a reported measurement, not a classifier.
  local function mouthH(i) return xu[i] - (mouthFitA + mouthFitK * t[i]) end
  local function innerH(i) return xu[i] - (innerFitA + innerFitK * t[i]) end

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
  -- The top of the mouth row, which is the surface you DRIVE ON when the row turns out not to
  -- be one plane. A deck is a plate with structure hanging under it; you drive on the plate.
  --
  -- Overhead structure is excluded by the file's existing definition of it -- more than
  -- WALL_MAX_H above the row's lowest node -- and that exclusion is load-bearing, not tidiness.
  -- large_cannon's ramp_M_0 sits 2.1 m up on the centreline, so a bare maximum would make the
  -- "top surface" a single node in mid-air and collapse the mouth to nothing.
  local mouthLoU, mouthTopU = math.huge, -math.huge
  for _, i in ipairs(mouth) do if xu[i] < mouthLoU then mouthLoU = xu[i] end end
  for _, i in ipairs(mouth) do
    if (xu[i] - mouthLoU) <= ]] .. WALL_MAX_H .. [[ and xu[i] > mouthTopU then
      mouthTopU = xu[i]
    end
  end
  local function mouthIsFloor(i)
    if mouthCoherent then return mouthH(i) <= ]] .. WALL_MIN_H .. [[ end
    return (mouthTopU - xu[i]) <= ]] .. WALL_MIN_H .. [[
  end

  local wallL, wallR, floorL, floorR = nil, nil, nil, nil
  local floorLatHi, floorLatLo = -math.huge, math.huge
  -- Lateral extreme first, and the station only where the lateral is a tie. See EDGE_TIE_M.
  local function takeMouthEdge(i)
    if (not floorL) or xr[i] > floorLatHi + ]] .. EDGE_TIE_M .. [[
       or (xr[i] > floorLatHi - ]] .. EDGE_TIE_M .. [[ and t[i] > t[floorL]) then
      floorL = i
    end
    if xr[i] > floorLatHi then floorLatHi = xr[i] end
    if (not floorR) or xr[i] < floorLatLo - ]] .. EDGE_TIE_M .. [[
       or (xr[i] < floorLatLo + ]] .. EDGE_TIE_M .. [[ and t[i] > t[floorR]) then
      floorR = i
    end
    if xr[i] < floorLatLo then floorLatLo = xr[i] end
  end
  local wallUsed = 0
  if mouthCoherent then
    for _, i in ipairs(mouth) do
      local h = mouthH(i)
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
        takeMouthEdge(i)
      end
    end
    if wallL then wallUsed = wallUsed + 1 else wallL = floorL end
    if wallR then wallUsed = wallUsed + 1 else wallR = floorR end
  else
    -- THE ROW IS NOT ONE PLANE, so the wall rule does not run on it at all.
    --
    -- Not a degraded version of the rule -- its input is missing. Every one of its tests is
    -- "how far above the floor is this node", and there is no single floor here to be above.
    -- Running it anyway is precisely what reported a us_semi rollback deck as 1.87 m wide when
    -- it is 2.59 m: the fit landed between the deck's two levels and the drivable surface
    -- itself came out as wall, so the rule -- which conservatively takes the SMALLEST
    -- qualifying lateral -- chose an inner rail 0.36 m inboard of the real edge on each side.
    --
    -- The answer for this shape is the lateral extremes of the top surface, and that is not a
    -- fallback so much as the right rule for the machine: a deck has no side walls, so there is
    -- nothing between the driver and its edges. wallUsed stays 0, which is already how the rest
    -- of the mod reads "the wall rule did not answer here", and the meta carries the naive
    -- figure alongside so the log line shows both.
    --
    -- The trade this makes, stated plainly: on a machine with real walls AND a multi-level
    -- floor this over-reports clearance where the old form under-reported it. That is the right
    -- direction for a deck, which has nothing to hit, and it is the direction that stops
    -- RAMPALIGN warning that a car will not fit through a mouth it fits through easily.
    for _, i in ipairs(mouth) do
      if mouthIsFloor(i) then takeMouthEdge(i) end
    end
    wallL, wallR = floorL, floorR
  end
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
    if mouthIsFloor(i) then
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
  local innerLoU, innerTopU = math.huge, -math.huge
  for _, i in ipairs(inner) do if xu[i] < innerLoU then innerLoU = xu[i] end end
  for _, i in ipairs(inner) do
    if (xu[i] - innerLoU) <= ]] .. WALL_MAX_H .. [[ and xu[i] > innerTopU then
      innerTopU = xu[i]
    end
  end
  local function innerIsFloor(i)
    if innerCoherent then return innerH(i) <= ]] .. WALL_MIN_H .. [[ end
    return (innerTopU - xu[i]) <= ]] .. WALL_MIN_H .. [[
  end
  for _, i in ipairs(inner) do
    if innerIsFloor(i) then
      -- Ties toward the INSIDE here (smaller t), the mirror of the mouth row's rule: the inner
      -- row's only job is to give the axis a second point, and that point wants to be as far
      -- from the mouth as the row goes.
      if (not innerL) or xr[i] > iHi + ]] .. EDGE_TIE_M .. [[
         or (xr[i] > iHi - ]] .. EDGE_TIE_M .. [[ and t[i] < t[innerL]) then
        innerL = i
      end
      if xr[i] > iHi then iHi = xr[i] end
      if (not innerR) or xr[i] < iLo - ]] .. EDGE_TIE_M .. [[
         or (xr[i] < iLo + ]] .. EDGE_TIE_M .. [[ and t[i] < t[innerR]) then
        innerR = i
      end
      if xr[i] < iLo then iLo = xr[i] end
    end
  end
  if not (innerL and innerR) then
    return reply("", "", "inner row has no floor band")
  end

  local cids = rc[wallL] .. "," .. rc[mouthC] .. "," .. rc[wallR] .. ","
            .. rc[innerL] .. "," .. rc[innerR]

  -- IS THE FITTED FLOOR MADE OF SURFACE, OR OF STRUCTURE? A node with collision off is not
  -- something a wheel can rest on, so a plane through one is not a floor and the pitch and the
  -- lip height derived from it are fiction.
  --
  -- Found on the Wheel Roller tilt ramp, which resolves through the onramp group: that group's
  -- inner row is on3r/on3l, collision = false, hanging 0.76 m UNDER the deck. The mouth row, the
  -- axis and the half-width were all correct, and the ramp still announced "ramp down 33
  -- degrees" and "ramp not down, lip 1.9 feet up" while sitting dead level -- the exact shape of
  -- error this file exists to refuse, arriving through a field nobody was checking.
  --
  -- Reported rather than rejected. Throwing the resolve away would take a correct mouth with it
  -- and lose the alignment the whole instrument is for; publishing the flag lets the readout
  -- decline the two numbers it cannot stand behind and keep the three it can.
  local cidTrusted = 1
  for _, i in ipairs({ wallL, mouthC, wallR, innerL, innerR }) do
    if rcoll[i] == false then cidTrusted = 0 end
  end

  local meta = tostring(halfW) .. "," .. tostring(span) .. "," .. tostring(mouthFloorU) .. ","
            .. tostring(rn) .. "," .. tostring(wallUsed) .. "," .. tostring(naiveHalfW) .. ","
            .. tostring(axisTier) .. "," .. tostring(isCannon) .. "," .. tostring(cidTrusted)
  -- The reason field is used on the SUCCESS path too, purely to carry which tiers fired. Both
  -- are name-driven choices that produce confident, plausible numbers when they land on the
  -- wrong thing, so which one answered is the first line worth reading when a half-width or a
  -- mouth end looks wrong.
  reply(cids, meta, "by " .. tierName .. " tier, axis tier " .. tostring(axisTier))
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
  epoch = tonumber(epoch) or 0
  -- Stale means "issued before the last invalidation", NOT "not the chunk I am waiting on".
  -- The old test was the second one, and it threw away the very reply this file needs most: a
  -- vehicle VM that takes longer than RESOLVE_TIMEOUT_S to run its queued command -- a cannon
  -- spawning with a level, on a loaded frame -- answers the FIRST chunk after the retry has
  -- already issued a second. Every such answer was dropped as stale, three times over, and the
  -- vehicle then landed in `failed` for the rest of the session with the resolve having in fact
  -- succeeded every single time. Nothing above D level said so.
  --
  -- The guard that actually matters is still intact, because it is the one the epoch was for:
  -- a reply describing a part configuration that has since been reset or swapped carries an
  -- epoch below the invalidation mark and is still rejected.
  if epoch <= staleFloor(vehID) then
    rgLog('D', string.format("stale ramp reply for %s (epoch %s, superseded by an invalidation)",
      tostring(vehID), tostring(epoch)))
    return
  end
  local p = pending[vehID]
  -- Whether this reply is the newest chunk's decides only whether a FAILURE is terminal: a
  -- success is a success whenever it arrives, but giving up on the strength of a superseded
  -- chunk's "nothing here" would discard an answer that is still in flight.
  local newest = (p == nil) or (epoch >= p.epoch)

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
    lastReason[vehID] = why ~= "" and why or "no reason given"
    stateKind[vehID] = "none"
    if not newest then
      -- A superseded chunk said no. The newest one has not answered yet, so this decides
      -- nothing; leave `pending` alone and wait for it.
      rgLog('D', string.format("ignoring superseded 'no ramp' reply for %s: %s",
        tostring(vehID), lastReason[vehID]))
      return
    end
    pending[vehID] = nil
    if why == "no node data" or why == "no nodes" then
      local n = (notReady[vehID] or 0) + 1
      notReady[vehID] = n
      if n < MAX_TRIES then
        rgLog('D', string.format(
          "vehicle %s was not ready to answer the ramp resolve (%s), attempt %d of %d",
          tostring(vehID), why, n, MAX_TRIES))
        stateKind[vehID] = "pending"
        return  -- pending is already cleared, so the next M.request re-issues
      end
    end
    failed[vehID] = true
    rgLog('D', string.format("vehicle %s has no usable ramp: %s",
      tostring(vehID), lastReason[vehID]))
    return
  end
  -- A VM answering with the wrong shape will keep answering with the wrong shape, so this is
  -- terminal too -- the same guard, and the same reasoning, as vehicleGeometry's #ext ~= 6.
  -- FIVE cids per mouth, and a machine may declare more than one -- the hamster wheel has a
  -- ramp at each end of its axle. So the test is "a whole number of mouths, at least one",
  -- never a fixed length. meta went 8 -> 9 with the floor-trust flag and the guard moves with
  -- it, which is what makes a half-updated install fail loudly instead of mis-reading a field.
  if #cids < 5 or (#cids % 5) ~= 0 or #meta ~= 9 then
    lastReason[vehID] = string.format("malformed reply: %d cids, %d meta", #cids, #meta)
    stateKind[vehID] = "malformed"
    if not newest then return end
    pending[vehID] = nil
    failed[vehID] = true
    rgLog('W', string.format("vehicle %s returned %d cids and %d meta, expected 5 and 8",
      tostring(vehID), #cids, #meta))
    return
  end

  -- A usable answer, whatever chunk asked for it.
  pending[vehID] = nil
  failed[vehID], notReady[vehID] = nil, nil
  lastReason[vehID] = "resolved"
  stateKind[vehID] = "resolved"
  cache[vehID] = {
    cids       = cids,
    halfW      = meta[1],
    alongSpan  = meta[2],
    floorU     = meta[3],
    nNodes     = meta[4],
    wallUsed   = meta[5],
    naiveHalfW = meta[6],
    -- 1 = the ramp cloud's displacement from the machine centroid named the axis and the mouth
    -- end. 2 = it did not, and the machine's own fore/aft axis was used with the REAR taken as
    -- the mouth. Carried rather than discarded because the two tiers can disagree about which
    -- end of a machine you drive into, and nothing else in a readout would reveal which one
    -- answered.
    axisTier   = meta[7],
    -- Whether this machine carries large_cannon's controller. Kept as a separate fact from
    -- "it has a ramp" because the two came apart the moment the part tiers admitted trailers;
    -- see the chunk for what reading one as the other did to the firing readout.
    isCannon   = meta[8] == 1,
    -- How many five-cid mouths cids holds. mouthFrame picks between them by proximity.
    mouthCount = #cids / 5,
    -- Whether the five chosen nodes are collision surface. When false the mouth's POSITION,
    -- AXIS and WIDTH are still good -- they come from the lateral spread and the row centroids,
    -- which structure below the deck does not distort -- but the floor plane through them is
    -- not a floor, so pitch and lip height are withheld rather than published wrong.
    floorTrusted = meta[9] == 1,
  }
  -- The naive half-width is printed even when the wall rule worked. It is the one line that
  -- turns "the clearance feels wrong" into an answer, and it costs nothing to carry.
  rgLog('I', string.format(
    "ramp on vehicle %d: %d nodes, span %.2f m along, mouth half-width %.3f m (%s; "
    .. "naive floor-band pick would have said %.3f m); %s; axis %s",
    vehID, meta[4], meta[2], meta[1],
    (meta[5] >= 2) and "wall rule both sides"
      or string.format("wall rule on %d of 2 sides, floor extreme on the rest", meta[5]),
    meta[6],
    tostring(reason or ""),
    (meta[7] == 2) and "by machine fore/aft, mouth at the rear (deck)"
      or "by centroid displacement"))
end

-- Fire a resolve for this vehicle unless one is cached, in flight, or known hopeless. Cheap to
-- call every tick; callers are not expected to track state.
function M.request(vehID)
  if not vehID or cache[vehID] or pending[vehID] or failed[vehID] then return end
  local veh = scenetree.findObjectById(vehID)
  if not veh then return end
  -- Asking a sleeping VM is not a resolve attempt, it is a message into a queue nobody is
  -- draining. Say so and come back later rather than spending a try on it.
  if not vehIsActive(veh) then
    lastReason[vehID] = "vehicle VM is inactive (pooled out)"
    stateKind[vehID] = "inactive"
    return
  end
  epochCounter = epochCounter + 1
  pending[vehID] = {epoch = epochCounter, timer = 0, tries = 1}
  lastReason[vehID] = "asked, waiting for the vehicle VM"
  stateKind[vehID] = "pending"
  pcall(function() veh:queueLuaCommand(string.format(VEH_SCRIPT, epochCounter)) end)
end

-- One line per vehicle this extension has an opinion about. The whole point is that "no ramp
-- near you" has half a dozen causes that sound identical from the seat, and until this existed
-- none of them could be told apart without reading the log at D level.
-- The one classification both renderings read from, so the console line and the spoken line can
-- never disagree about what state a vehicle is in.
local function classify(vehID)
  if cache[vehID] then return "resolved" end
  if pending[vehID] then return "pending" end
  if failed[vehID] then return stateKind[vehID] or "silent" end
  return stateKind[vehID] or "unasked"
end

function M.stateOf(vehID)
  local kind = classify(vehID)
  local why = tostring(lastReason[vehID] or "no reason recorded")
  if kind == "resolved" then
    local e = cache[vehID]
    -- The axis tier is named here rather than left in the log, because "it resolved" and "it
    -- resolved by guessing the mouth is at the rear" are different amounts of trust, and this
    -- string is what rampTruth() and the spoken reason both render.
    -- Tier 3 has to name itself, or a declared mouth renders as "by displacement" -- which is
    -- the one claim it is not. Whether the mouth was derived or handed over is the first thing
    -- worth knowing when one looks wrong, since only one of the two can be fixed by tuning.
    local axisWord = "by displacement"
    if e.axisTier == 2 then
      axisWord = "by machine fore/aft, mouth at rear"
    elseif e.axisTier == 3 then
      axisWord = "DECLARED, not inferred"
    end
    return string.format("RESOLVED (%d nodes, half-width %.2f m, axis %s%s%s)%s",
      e.nNodes, e.halfW, axisWord,
      (e.mouthCount or 1) > 1 and string.format(", %d mouths", e.mouthCount) or "",
      e.floorTrusted == false and ", PITCH WITHHELD (nodes are not collision surface)" or "",
      e.isCannon and " [large_cannon]" or "")
  elseif kind == "pending" then
    local p = pending[vehID]
    if p then
      return string.format("PENDING (attempt %d of %d, %.1fs)", p.tries, MAX_TRIES, p.timer)
    end
    return "PENDING (asked, waiting for the vehicle VM)"
  elseif kind == "none" then
    -- Deliberately NOT "gave up". The vehicle answered; this is the answer. An ordinary car
    -- and a cannon with no drive-in ramp both land here and both are correct.
    return "NO RAMP ON IT (asked and answered): " .. why
  elseif kind == "inactive" then
    return "WAITING: " .. why
  elseif kind == "unasked" then
    return lastReason[vehID] and ("WAITING: " .. why) or "never asked"
  end
  return "GAVE UP: " .. why
end

-- Speech-sized. The spoken readout gets one clause, not a diagnosis: the point from the seat is
-- whether to keep waiting, look elsewhere, or stop looking at this machine altogether.
function M.shortStateOf(vehID)
  local kind = classify(vehID)
  if kind == "resolved" then return "ramp found" end
  if kind == "pending" then return "still checking" end
  if kind == "none" then return "no ramp on it" end
  if kind == "inactive" then return "asleep" end
  if kind == "unasked" then return "not checked yet" end
  return "not answering"
end

function M.diag()
  local out = {}
  local ids = {}
  local seen = {}
  for _, t in ipairs({cache, pending, failed, notReady, lastReason}) do
    for id in pairs(t) do
      if not seen[id] then seen[id] = true; ids[#ids + 1] = id end
    end
  end
  table.sort(ids)
  if #ids == 0 then return "rampGeometry: nothing asked yet" end
  for _, id in ipairs(ids) do
    local veh = scenetree.findObjectById(id)
    local name = "gone"
    if veh then
      local ok, n = pcall(function() return veh.JBeam or veh:getField("name", 0) end)
      name = (ok and n and n ~= "") and tostring(n) or "?"
    end
    out[#out + 1] = string.format("%s [%s]: %s", name, tostring(id), M.stateOf(id))
  end
  return table.concat(out, "\n")
end

-- Forget everything about a vehicle and ask again. The manual escape hatch for a resolve that
-- gave up for a reason that has since gone away -- the VM was asleep, the level was still
-- loading -- without having to reset the vehicle or reload the level.
-- Goes through M.invalidate rather than the local dropVehicle/dropAll, which are declared
-- further down the file and would resolve to a nil GLOBAL from up here.
function M.retry(vehID)
  if vehID == nil then
    M.invalidate(nil)
    return "rampGeometry: cleared everything; resolves will re-arm on the next query"
  end
  M.invalidate(vehID)
  M.request(vehID)
  return string.format("rampGeometry: vehicle %s re-armed -- %s",
    tostring(vehID), M.stateOf(vehID))
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

-- Is this machine the stock large_cannon? A DIFFERENT question from M.has, and the separation
-- is the whole point: has() means "you can drive into this", isCannon() means "this can be
-- aimed and fired". They were the same predicate only for as long as large_cannon was the only
-- vehicle whose ramp resolved at all, and the part tiers ended that -- a rollback, a tilt deck
-- and a dry van all answer yes to the first and no to the second. Resolved from the presence
-- of large_cannon's own controller, in the vehicle VM, in the same round trip; see the chunk.
--
-- Self-arming like has(), so beamtel's per-tick poll needs no request of its own. False for
-- anything unresolved, which is the conservative way round: the readout it gates reads live
-- electrics that mean nothing on an ordinary vehicle.
function M.isCannon(vehID)
  M.request(vehID)
  local e = cache[vehID]
  return (e ~= nil) and e.isCannon == true
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

    -- A MACHINE MAY HAVE MORE THAN ONE MOUTH, and which one is "the" mouth is a fact about
    -- where the driver is, not about the machine. The hamster wheel has a drive-in ramp at each
    -- end of its axle: they are equally valid, and answering with a fixed one would send a
    -- driver standing at the near ramp all the way round the machine to the far one.
    --
    -- So the choice is made HERE, per query, by proximity to the player -- the same reasoning
    -- that keeps the frame itself live rather than remembered. A single-mouth machine takes the
    -- identical path with one candidate and cannot be affected.
    local nMouths = entry.mouthCount or 1
    local best = 0
    if nMouths > 1 then
      local ref = nil
      local pv = getPlayerVehicle(0)
      if pv then ref = vec3(pv:getPosition()) end
      if ref then
        local bestD = math.huge
        for k = 0, nMouths - 1 do
          local a1 = base + vec3(veh:getNodePosition(entry.cids[k * 5 + 1]))
          local a3 = base + vec3(veh:getNodePosition(entry.cids[k * 5 + 3]))
          local d = ((a1 + a3) * 0.5):distance(ref)
          if d < bestD then bestD, best = d, k end
        end
      end
    end

    local p = {}
    for i = 1, 5 do
      p[i] = base + vec3(veh:getNodePosition(entry.cids[best * 5 + i]))
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
      -- NIL, not zero, when the five nodes are not collision surface. Zero would read as "this
      -- ramp is dead level", which is a claim, and the whole point is that we cannot make one.
      -- Every consumer treats a missing pitch as silence, the same contract the DOCK line's
      -- optional tail already uses.
      pitchDeg = entry.floorTrusted ~= false and pitchDeg or nil,
      -- Carried so the lip height derived from floorZ can be withheld on the same grounds:
      -- floorZ is the minimum of the same three mouth nodes.
      floorTrusted = entry.floorTrusted ~= false,
      mouthIndex = best + 1,
      mouthCount = nMouths,
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
  lastReason[vehID] = nil
  stateKind[vehID] = nil
  -- Everything already in flight described the part configuration that just went away, so it
  -- must not be installed when it lands. This is the ONLY thing the epoch is for.
  markStale(vehID)
end

local function dropAll()
  cache, pending, failed, notReady, lastReason, stateKind = {}, {}, {}, {}, {}, {}
  staleBefore = {}
  markStale(nil)
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

-- A vehicle coming back from the active pool is the one event that can turn a hopeless resolve
-- into a workable one without anything about the vehicle changing: an inactive VM cannot run a
-- queued chunk, so everything it "failed" is an artefact of it having been asleep. Only the
-- give-up state is cleared -- a cached ramp is still a ramp, and dropping it here would re-issue
-- a chunk every time a machine crossed the pooling boundary.
function M.onVehicleActiveChanged(vehId, active)
  if not active or active == 0 then return end
  if cache[vehId] or pending[vehId] then return end
  if failed[vehId] or notReady[vehId] then
    failed[vehId], notReady[vehId] = nil, nil
    lastReason[vehId] = "re-armed: vehicle VM became active again"
    stateKind[vehId] = "unasked"
    rgLog('D', string.format("vehicle %s became active; re-arming the ramp resolve",
      tostring(vehId)))
  end
end

function M.onUpdate(dtReal, dtSim, dtRaw)
  -- Retry genuinely stalled resolves only. A vehicle VM that is still spawning cannot answer,
  -- and without this the entry would stay pending forever. A vehicle that answered "no ramp" is
  -- already in `failed` and never reaches this loop.
  for vehID, p in pairs(pending) do
    p.timer = p.timer + dtReal
    if p.timer >= RESOLVE_TIMEOUT_S then
      local veh0 = scenetree.findObjectById(vehID)
      if veh0 and not vehIsActive(veh0) then
        -- Time spent asleep is not a failed attempt. Hold the entry open and keep waiting;
        -- onVehicleActiveChanged re-arms properly once the VM is running again.
        p.timer = 0
        lastReason[vehID] = "vehicle VM is inactive (pooled out)"
        stateKind[vehID] = "inactive"
      elseif p.tries >= MAX_TRIES then
        -- Flagged before clearing `pending`, so this is logged exactly once and the give-up is
        -- genuinely permanent rather than being re-armed by the next M.request.
        failed[vehID] = true
        lastReason[vehID] = string.format(
          "vehicle VM never answered %d chunks over %.0fs", MAX_TRIES,
          MAX_TRIES * RESOLVE_TIMEOUT_S)
        stateKind[vehID] = "silent"
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
