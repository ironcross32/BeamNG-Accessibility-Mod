-- Proving Grounds: post-load verification.
--
-- Paste into the accessible console (or run via the MCP lua_exec tool) once the
-- level is loaded. Everything here checks a fact the generator ASSUMED and could
-- not settle from the files alone.
--
--   extensions.load('util_provingGroundsVerify')  -- not needed; this is a chunk
--
-- The one genuinely open question is the spawn heading. The generator applies a
-- 180 degree flip (proven from spawn.lua:974) and picks a matrix convention
-- (SPAWN_MATRIX_COLUMN_MAJOR) that a single shipped sample cannot disambiguate.
-- Check 5 below settles it; if a heading is reversed, flip that one constant in
-- tools/mapgen/build.py and rebuild.

local out = {}
local function say(s) out[#out + 1] = s end

-- 1. the level and its objects -------------------------------------------------
say("mission = " .. tostring(getMissionFilename()))
local tb = scenetree.findObject("theTerrain")
say(string.format("terrain=%s  roads=%d  water=%d  spawns=%d",
  tostring(tb ~= nil),
  #(scenetree.findClassObjects("DecalRoad") or {}),
  #(scenetree.findClassObjects("WaterBlock") or {}),
  #(scenetree.findClassObjects("SpawnSphere") or {})))

-- The terrain material makes the climb visible and audible, but AI and the
-- BeamTel road detector both consume map.getMap(), which is populated from
-- drivable DecalRoad objects. The helper road is deliberately invisible so it
-- cannot cover the asphalt/rumble/gravel surface sequence.
local climbRoad = scenetree.findObject("road_hill_climb")
local navCount = 0
local nav = map and map.getMap and map.getMap() or nil
for id, _ in pairs((nav and nav.nodes) or {}) do
  if tostring(id):find("^road_hill_climb") then navCount = navCount + 1 end
end
if climbRoad then
  say(string.format("hill climb road: nodes=%d width=%.1f drivability=%.1f navNodes=%d %s",
    climbRoad:getNodeCount(), climbRoad:getNodeWidth(0), climbRoad.drivability,
    navCount, navCount > 1 and "OK" or "MISSING FROM NAVGRAPH"))
else
  say("hill climb road MISSING")
end

-- 2. terrain heights at the named sections -------------------------------------
-- getSurfaceHeightBelow is the visible-surface answer; core_terrain is the
-- terrain-only one. They should agree here because nothing is built on top.
local function groundAt(x, y)
  local p = vec3(x, y, 2000)
  local h = core_terrain and core_terrain.getTerrainHeight(p)
  return h
end

local expect = {
  { "staging",      0,     -2200, 50.0 },
  { "mud basin",   -1500,  -900,  49.4 },   -- apron, not in a pit
  { "ford bed",     1300,  -900,  49.55 },  -- BASE_Z - 0.45
  { "sump centre",  1500,   340,  45.5 },   -- BASE_Z - 4.5
  { "climb foot",   0,     -780,  50.0 },
  { "climb summit", 0,      2265, 855.5 },
}
for _, e in ipairs(expect) do
  local h = groundAt(e[2], e[3])
  local d = h and (h - e[4]) or nil
  say(string.format("  %-13s got %8.2f  want %8.2f  %s", e[1],
    h or -1, e[4], (d and math.abs(d) < 1.0) and "OK" or "MISMATCH"))
end

-- 3. the climb actually steepens ------------------------------------------------
-- Sampled as rise over a 50 m run, which is what a car experiences.
say("climb grade by distance along:")
for _, s in ipairs({ 0, 750, 1500, 2250, 2950 }) do
  local y0, y1 = -780 + s, -780 + s + 50
  local h0, h1 = groundAt(0, y0), groundAt(0, y1)
  if h0 and h1 then
    say(string.format("  %5d m: %5.1f %%", s, (h1 - h0) / 50 * 100))
  end
end

-- 4. the berms are actually walls ----------------------------------------------
-- Road half-width is 12.5 m and every node beyond it stands at full BERM_H, so
-- the wall is ONE CELL wide. Both figures matter and the second is the one that
-- was wrong twice: a berm can be 7 m proud and still be driven over if the rise
-- is spread across two nodes, because the outer of the two is then a ramp.
local yMid = -780 + 1500
local hRoad = groundAt(0, yMid)
local hBerm = groundAt(18, yMid)
say(string.format("berm at mid-climb: road %.2f, berm %.2f, proud by %.2f m (want 12.00)",
  hRoad or -1, hBerm or -1, (hBerm and hRoad) and (hBerm - hRoad) or -1))
local prev, worst = groundAt(0, yMid), 0
for x = 2.5, 25, 2.5 do
  local h = groundAt(x, yMid)
  worst = math.max(worst, (h - prev) / 2.5)
  prev = h
end
say(string.format("  steepest single-cell face %.0f %% -- a ramp anywhere in the rise shows up here",
  worst * 100))

-- 5. SPAWN HEADINGS -- the open question ---------------------------------------
-- The summit spawn must face SOUTH (0, -1) so the hill can be driven downward.
local function spawnFwd(name)
  local o = scenetree.findObject(name)
  if not o then return nil end
  -- the same expression core/levels.lua:400 and spawn.lua:974 use to place a car
  local rot = quat(o:getRotation()) * quat(0, 0, 1, 0)
  return rot * vec3(0, 1, 0)
end
for _, n in ipairs({ "spawn_default", "spawn_climb_base", "spawn_climb_summit" }) do
  local f = spawnFwd(n)
  if f then
    say(string.format("  %-20s vehicle would face (%.2f, %.2f)", n, f.x, f.y))
  else
    say("  " .. n .. " MISSING")
  end
end
say("  expected: spawn_default (0,1)N  climb_base (0,1)N  climb_summit (0,-1)S")

-- 5b. the tunnel is a kilometre of collision shell and an audio space --------
local tunnelSegments = {}
for _, id in ipairs(scenetree.findClassObjects("TSStatic") or {}) do
  local o = scenetree.findObject(id)
  if o and tostring(o:getName()):find("^tunnel_segment_") then
    tunnelSegments[#tunnelSegments + 1] = o
  end
end
say("summit vertical curve (grade must fall smoothly to zero):")
for _, y0 in ipairs({ 2220, 2230, 2240, 2250, 2260, 2262.5 }) do
  local y1 = math.min(y0 + 2.5, 2265)
  local h0, h1 = groundAt(0, y0), groundAt(0, y1)
  if h0 and h1 and y1 > y0 then
    say(string.format("  y=%6.1f: %5.1f %%", y0,
      (h1 - h0) / (y1 - y0) * 100))
  end
end
local tunnelAudio = scenetree.findObject("audio_one_kilometre_tunnel")
local tunnelRoad = scenetree.findObject("road_one_kilometre_tunnel")
local tunnelSpawn = scenetree.findObject("spawn_tunnel")
say(string.format("tunnel: segments=%d roadNodes=%d spawn=%s audio=%s ambience=%s",
  #tunnelSegments, tunnelRoad and tunnelRoad:getNodeCount() or 0,
  tostring(tunnelSpawn ~= nil), tostring(tunnelAudio ~= nil),
  tunnelAudio and tostring(tunnelAudio.soundAmbience) or "missing"))
if tunnelAudio then
  local s = tunnelAudio:getScale()
  say(string.format("  reverb volume %.1f x %.1f x %.1f m (want length 1000.0)",
    s.x, s.y, s.z))
end
if #tunnelSegments > 0 then
  -- A world box proves the mesh loaded; rays prove its collision detail loaded.
  -- Both start inside the bore, so the terrain cannot satisfy either ray.
  local sideHit = castRayStatic(vec3(-2000, 750, 53), vec3(1, 0, 0), 20)
  local roofHit = castRayStatic(vec3(-2000, 750, 53), vec3(0, 0, 1), 20)
  say(string.format("  collision rays: wall %.2f m, roof %.2f m %s",
    sideHit, roofHit, (sideHit < 20 and roofHit < 20) and "OK" or "MISSING COLLISION"))
  local southOutside = groundAt(-2000, 495)
  local southInside = groundAt(-2000, 505)
  local northInside = groundAt(-2000, 1495)
  local northOutside = groundAt(-2000, 1505)
  say(string.format("  portal terrain joins: south %.3f m, north %.3f m (want 0.000)",
    math.abs((southInside or 0) - (southOutside or 0)),
    math.abs((northInside or 0) - (northOutside or 0))))
end

-- 6. surface materials underfoot ------------------------------------------------
-- groundmodel is what makes mud behave like mud; a wrong internalName in the
-- .ter shows up as the terrain having no surface at all.
if tb then
  local function matAt(x, y)
    local ok, m = pcall(function() return tb:getMaterialName(x, y) end)
    return ok and tostring(m) or "n/a"
  end
  say("materials: road=" .. matAt(0, 0) ..
      "  plain=" .. matAt(600, 0) ..
      "  mud=" .. matAt(-1610, -1010))
end

-- 7. water surface height -------------------------------------------------------
-- A WaterBlock's position.z IS the surface; scale.z is depth DOWNWARD from it.
-- The first version of this check read p.z + s.z, which is the same wrong model
-- the generator was using, so it reported 50.00 about two pools sitting two
-- metres underground. A check derived from the code it checks cannot fail, so
-- the surface is compared against the TERRAIN BED instead -- an independent
-- measurement, and the one that says whether there is water over the cut.
for _, n in ipairs({ "water_ford", "water_sump" }) do
  local o = scenetree.findObject(n)
  if o then
    -- The waterline is position.z. NOT the top of getWorldBox: that box is the
    -- collision volume and straddles the surface, and reading its top face is
    -- how this check was wrong the second time. Geometry alone cannot settle
    -- which face is the water, so the real test is check 8 below -- this one
    -- only says whether the numbers are self-consistent.
    local p, s = o:getPosition(), o:getScale()
    local bed = groundAt(p.x, p.y)
    local d = bed and (p.z - bed) or -1
    say(string.format("  %-11s waterline %.2f, bed %.2f, depth %.2f m %s  footprint %.0f x %.0f",
      n, p.z, bed or -1, d, d > 0.05 and "OK" or "DRY -- WATERLINE IS UNDER THE TERRAIN",
      s.x, s.y))
    -- A valid volume can still look like bare dirt. This exact regression had
    -- all three textures and working underwater fog, but its clear surface was
    -- visually indistinguishable from the ford bed. Keep a graphics-quality-
    -- independent fallback reflection and enough surface colour to read.
    local cubemap = o:getField("cubemap", 0)
    local clarity = tonumber(o:getField("clarity", 0)) or 1
    local fresnelBias = tonumber(o:getField("fresnelBias", 0)) or 0
    local visibleStyle = cubemap ~= "" and clarity <= 0.25 and fresnelBias >= 0.4
    say(string.format("    visible style: cubemap=%s clarity=%.2f fresnelBias=%.2f %s",
      cubemap ~= "" and cubemap or "MISSING", clarity, fresnelBias,
      visibleStyle and "OK" or "TOO CLEAR -- MAY LOOK LIKE BARE DIRT"))
  else
    say("  " .. n .. " MISSING")
  end
end

-- 8b. suspension straights ------------------------------------------------------
-- The three lanes must differ from each other by a wide margin and each must be
-- FLAT ACROSS ITS OWN WIDTH: the profile varies along the lane only, so a car
-- gets the same input at both wheels and a run is repeatable. A lateral tilt
-- inside a lane means the blend weights are wrong and the section is measuring
-- roll instead of heave.
--
-- The material layout is deliberately NOT checked here, because it cannot be:
-- TerrainBlock has no position-to-material query at all (getMaterialName(i) is a
-- lookup into the material TABLE and ignores anything you pass as a second
-- argument -- it reads exactly like a coordinate query and is not one). The only
-- instrument for that is wheels.wheels[i].contactMaterialID1 from a vehicle VM:
-- 10 asphalt, 15 dirt, 19 gravel, 29 rumble strip.
local SUSP_X, SUSP_Y0, SUSP_LEN = -1200, 300, 1000
local lanes = { { -20, "gentle", 0.46 }, { 0, "medium", 0.91 }, { 20, "harsh", 1.56 } }
say("suspension straights:")
for _, L in ipairs(lanes) do
  local lo, hi, worst, prev = 1e9, -1e9, 0, nil
  for d = 0, SUSP_LEN, 2.5 do
    local h = groundAt(SUSP_X + L[1], SUSP_Y0 + d)
    if h then
      lo, hi = math.min(lo, h), math.max(hi, h)
      if prev then worst = math.max(worst, math.abs(h - prev) / 2.5) end
      prev = h
    end
  end
  -- flatness across the lane, at a point where the profile is well away from zero
  local yProbe = SUSP_Y0 + SUSP_LEN / 2
  local cl, cr = groundAt(SUSP_X + L[1] - 6, yProbe), groundAt(SUSP_X + L[1] + 6, yProbe)
  say(string.format("  %-7s peak-to-peak %.2f m (want %.2f)  steepest %5.1f %%  cross-tilt %.3f m %s",
    L[2], hi - lo, L[3], worst * 100, math.abs((cl or 0) - (cr or 0)),
    (math.abs(hi - lo - L[3]) < 0.10 and math.abs((cl or 0) - (cr or 0)) < 0.02)
      and "OK" or "MISMATCH"))
end
-- the apron in front of the lanes must be dead flat: it is the baseline you set
-- the car up on, and the reference the first bump is felt against
local apronLo, apronHi = 1e9, -1e9
for d = -110, -10, 5 do
  for o = -30, 30, 5 do
    local h = groundAt(SUSP_X + o, SUSP_Y0 + d)
    if h then apronLo, apronHi = math.min(apronLo, h), math.max(apronHi, h) end
  end
end
say(string.format("  apron spread %.3f m %s", apronHi - apronLo,
  (apronHi - apronLo) < 0.02 and "OK" or "NOT FLAT"))

-- 8c. the sound stage rigs -----------------------------------------------------
-- These are the only objects on this map the generator does not write into the
-- scene: a level scene cannot hold a vehicle, so mainLevel.lua spawns them a
-- couple of seconds into the mission. That makes "are they there" a real
-- question rather than a formality -- a spawn that is declined (vehicle cap,
-- unknown config name, a game update renaming a prop) fails silently and leaves
-- an empty pad that looks exactly like a pad you have not driven to yet.
--
-- The HEADING is the half worth checking. Every rig here is entered heading
-- north, but only one of the three is faced north to achieve that: the hamster
-- wheel is faced east because you drive in across its axle, and the tilt ramp is
-- faced south because its jbeam puts the onramp at local +Y. Each of those is a
-- reading of somebody else's jbeam, and getting one backwards parks the driver
-- at a wall with no way to tell that from a rig they have simply missed.
local RIGS = {
  { "testroller",          300, -2200,  0, 1, "Wheel Roller (rolling road)" },
  { "large_hamster_wheel", 400, -2200,  1, 0, "Hamster Wheel" },
  { "testroller",          500, -2200,  0, 1, "Tilt Ramp" },
}
say("sound stage rigs:")
for _, R in ipairs(RIGS) do
  local found, fwd, gap = nil, nil, 1e9
  for i = 0, be:getObjectCount() - 1 do
    local o = be:getObject(i)
    if o and o:getJBeamFilename() == R[1] then
      local p = vec3(o:getPosition())
      local d = math.sqrt((p.x - R[2]) ^ 2 + (p.y - R[3]) ^ 2)
      if d < gap then gap, found, fwd = d, o, vec3(o:getDirectionVector()) end
    end
  end
  if found and gap < 8 then
    local dot = fwd.x * R[4] + fwd.y * R[5]
    say(string.format("  %-28s at %.1f m, facing (%.2f, %.2f) want (%d, %d) %s",
      R[6], gap, fwd.x, fwd.y, R[4], R[5],
      dot > 0.95 and "OK" or "WRONG HEADING -- entry faces the wrong way"))
  else
    say("  " .. R[6] .. " MISSING -- mainLevel.lua did not spawn it")
  end
end

-- ...and the one thing a facing alone does not settle: which way the DOCKING INSTRUMENT
-- thinks you drive in. mouth.axis points INTO the ramp, so on this pad it must be north.
-- The prop facing and the mouth axis are derived by completely separate machinery (a spawn
-- rotation here, a node-cloud centroid displacement in rampGeometry), and the tilt ramp is
-- exactly the case where they disagreed: its refNodes put +Y at the REAR, so facing it by
-- the jbeam group names alone produced a mouth entered heading south.
local rg = extensions.rampGeometry
if rg and rg.mouthFrame then
  local okF, mf = pcall(rg.mouthFrame, (function()
    for i = 0, be:getObjectCount() - 1 do
      local o = be:getObject(i)
      if o and o:getJBeamFilename() == "testroller" then
        local p = vec3(o:getPosition())
        if math.abs(p.x - 500) < 8 and math.abs(p.y + 2200) < 8 then return o:getID() end
      end
    end
    return -1
  end)())
  if okF and mf then
    say(string.format("  tilt ramp mouth: centre (%.1f, %.1f, %.2f) axis (%.2f, %.2f) "
      .. "half-width %.2f m %s", mf.centre.x, mf.centre.y, mf.centre.z,
      mf.axis.x, mf.axis.y, mf.halfW or -1,
      mf.axis.y > 0.95 and "OK" or "AXIS NOT NORTH -- entered from the wrong side"))
    -- KNOWN WRONG on this rig, and stated rather than hidden: the onramp group's inner row
    -- is on3r/on3l, which are collision:false and sit 0.76 m UNDER the deck, so the fitted
    -- floor plane is a strut and the pitch is about -33 degrees on a level ramp.
    say(string.format("  tilt ramp pitch %.1f deg (expect ~0; a large value here is the known "
      .. "non-collision inner-row defect, not a tilted ramp)", mf.pitchDeg or -999))
  else
    say("  tilt ramp mouth: UNRESOLVED -- docking instrument has nothing to align to")
  end
end

-- 8. THE WATER IS ACTUALLY WET --------------------------------------------------
-- The only check here that asks the SIMULATION rather than the scene, and the
-- only one that could have caught a pool which renders correctly and does not
-- exist to the physics -- which is exactly what shipped: the ford looked right
-- from every angle, and driving through it made dirt noises.
--
-- Water sounds, wading drag and hydrolocking all read the same obj:inWater(cid)
-- this counts (sounds.lua:434, wheels.lua:187, combustionEngine.lua:337), so if
-- this says dry, the section does not work however convincing it looks. No
-- amount of geometry can substitute: getWorldBox reports the collision volume,
-- which straddles the waterline, and nothing in it says which face is the water.
--
-- It teleports the player, so it runs last, and resetVehicle stays false (see
-- "No teleport in this mod may reset the vehicle"). safeTeleport settles the
-- cluster on a later frame. Queueing the vehicle command immediately races that
-- move and can report zero wet nodes from the OLD position; this happened with
-- all 757 nodes present and looked exactly like broken water physics. The job
-- waits before crossing into the vehicle VM, and the count then lands another
-- frame or two later in the GE globals. Read it with a second call.
--
--   > extensions.provingGroundsWetness -- not a thing; just re-run this chunk
--   > return provingGroundsWet, provingGroundsNodes
--
-- NEVER measure this after moving a WaterBlock at runtime. setPosition plus
-- postApply updates the render and leaves the physics registration stale, and it
-- then reports the HIGHEST nodes wet and the lowest dry -- wrong in a way that
-- reads as a subtle offset rather than as garbage. Reload the level first.
local function wetnessAt(pos)
  local veh = be:getPlayerVehicle(0)
  if not veh then return "no player vehicle" end
  local vehId = veh:getID()
  provingGroundsWet, provingGroundsNodes = nil, nil
  spawn.safeTeleport(veh, pos, quatFromEuler(0, 0, 0), nil, nil, nil, false, false)
  core_jobsystem.create(function(job)
    job.sleep(0.5)
    local settledVeh = be:getObjectByID(vehId)
    if not settledVeh then return end
    settledVeh:queueLuaCommand([==[
      local wet, n = 0, 0
      for i = 0, #v.data.nodes do
        local nd = v.data.nodes[i]
        if nd then n = n + 1 if obj:inWater(nd.cid) then wet = wet + 1 end end
      end
      obj:queueGameEngineLua('provingGroundsWet = ' .. wet .. '; provingGroundsNodes = ' .. n)
    ]==])
  end, 1)
  return "teleported; read provingGroundsWet / provingGroundsNodes after one second"
end
say("wetness test (ford, 0.45 m): " .. tostring(wetnessAt(vec3(1300, -900, 50.2))))

return table.concat(out, "\n")
