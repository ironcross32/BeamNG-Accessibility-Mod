-- =================================================================================================
--
--  Terrain Scanner for BeamNG.drive Accessibility Mod (GE Extension)
--
--  Description: One-shot snapshot of the landscape in front of the player, sampled on a polar
--               grid and shipped to Python, which renders it as a granular cloud (pitch =
--               elevation, time = distance, pan = bearing). See the "Terrain sonification
--               scanner" block in audio.py for the instrument itself.
--
--               This file only gathers. It makes no decision about how anything sounds.
--
--  Loaded by: scripts/bng_screenreader_mod/modScript.lua
--
--  Water objects are resolved once per scan and tested spatially. River segments provide
--  their bed elevation directly; flat water gets a second, budgeted ray when the first ray
--  lands on its surface.
--
-- =================================================================================================

local M = {}

local PYTHON_HOST       = "127.0.0.1"
local PYTHON_PORT_DATA  = 4471   -- send scan snapshots to Python
local CMD_LISTEN_PORT   = 4472   -- receive SCAN / NEAREST_POI from Python

-- MUST match SCAN_MAX_RANGE_M in audio.py -- Python's time axis is scaled by the reach we
-- report, so the two only agree because the ceiling is the same number in both places.
-- diagnostic/terrain_scan_sim.py greps this line and asserts it.
local SCAN_MAX_RANGE_M  = 200.0
local SCAN_BEARINGS     = 24     -- spokes across the forward 180 degrees, endpoints included
local SCAN_RINGS        = 25     -- rings per spoke, ring 1 sitting ON the vehicle
-- Every sample is raycast first so a hidden TerrainBlock cannot mask visible static meshes.
-- Water-bed probes share this same budget. At 100 rays per frame the ordinary 600-cell scan
-- collects over roughly six frames without hitching the GE update.
local SCAN_RAYS_PER_TICK = 100
local SCAN_MIN_REACH_M  = 20.0   -- a floor, so a tiny map still produces a usable time axis
local SCAN_RAY_TOP_Z    = 100000.0
local WATER_PROBE_EPS_M = 0.05

-- Surface families. The scan carries what the ground IS as a one-character suffix on the
-- cell token; audio.py turns that into a timbre. Omission means paved, which is ALSO what an
-- unresolved material means, so a map with no TerrainBlock and an older Python half both
-- render exactly as they always did.
--
-- Classified on COLLISIONTYPE, not on the groundmodel name, because collisiontype is already
-- the game's own answer to "what does this surface sound like" (docs/level-generation.md) and
-- it collapses the aliases for free -- measured live, ASPHALT, ASPHALT_OLD, ASPHALT_PREPPED,
-- KICKPLATE and VOID are one collisiontype between them, as are GRAVEL and GRAVEL_WET, ICE
-- and FRICTIONLESS, SNOW and SNOWBANK.
--
-- The list below is therefore REPRESENTATIVE NAMES, resolved to collisiontype ids once per
-- scan and applied by id. That is what makes it more than a transcribed allowlist: every
-- other groundmodel sharing one of those ids classifies without being named, a modded map's
-- own surfaces included. The same capability-over-a-list argument rampGeometry.isCannon()
-- makes.
--
-- Two things about the API, both established by probing a running game and neither guessable
-- from the engine's own Lua. `be:getGroundModelByID(i).data` is USERDATA, not a table, and
-- its `.name` field is **nil on every one of the 60 registrations** -- so the registry cannot
-- be walked and keyed by name, which is what the obvious implementation tries to do (and why
-- the stock dumpGroundModels writes `gms[gm.name or i]`). The way in is
-- `be:getGroundModelIDByName(name)`, which is case-insensitive and answers -1 for a name it
-- does not know. And `.collisiontype` is an INTEGER id, not a string: the index space is the
-- fixed material list at the top of lua/common/particles.json.
local FAMILY_NAMES = {
  d = {"DIRT", "DIRT_DUSTY", "MUD"},
  v = {"GRAVEL", "SAND", "ROCK"},
  g = {"GRASS", "BRANCHES_STRONG", "LEAVES_THIN"},
  -- SNOW rides with ICE. They do not drive alike, but both mean NO GRIP, which is the only
  -- thing a surface sonification is for -- and snow is one of the surfaces with no tyre-sound
  -- branch at all, so the scan is the only way to know it is there.
  i = {"ICE", "SNOW"},
  -- Everything else, ASPHALT and friends included, resolves to nothing: no suffix, and
  -- therefore today's tone.
}
-- A ray that lands more than this above or below the heightmap found something that is not
-- the terrain -- a bridge deck, a building roof, a static mesh. Its material is therefore not
-- the terrain's material, so the cell reports unknown rather than the riverbed underneath it.
local MAT_TERRAIN_AGREE_M = 1.0

-- Road overlay. getMaterialIdxWs answers for the TERRAIN LAYER only, so an asphalt DecalRoad
-- laid over grass -- which is how nearly every stock map builds a road -- reports grass, and
-- "paved is unchanged" would be false in exactly the place it matters most. The navigation
-- graph is the one thing that knows a road is there. Built one-shot per scan, local to this
-- file: roadDetector.lua owns the same index but rebuilds it on its own schedule and can be
-- switched off entirely, so calling into it would make the scan depend on another feature's
-- state. Its shape is mirrored (buckets, candidates, projection), not imported.
local ROAD_BUCKET_SIZE_M   = 50.0
local ROAD_Z_TOLERANCE_M   = 3.0   -- tighter than roadDetector's 6.0: compared against the
                                   -- cell's own visible surface, not the vehicle's z
local ROAD_MIN_DRIVABILITY = 0.75

-- POIs. Capped and deduped because a busy map clusters a dozen mission starts on one spot,
-- and a dozen doublets from one bearing buries the terrain bed the scan exists to draw.
local POI_MAX = 12

local udpSend = nil
local udpCmd  = nil
local scan    = nil   -- the in-progress snapshot, or nil when idle
local nextId  = 1
local lastScanDiag = nil

local function tsLog(level, msg) log(level, 'TerrainScanner', msg) end

local function send(line)
  if udpSend then pcall(function() udpSend:send(line) end) end
end

-- -------------------------------------------------------------------------------------------
-- Surface height, in two tiers.
--
-- The ray is authoritative because it sees the visible collision surface: terrain, static
-- meshes, bridges, and GroundPlanes. The TerrainBlock heightmap is only a fallback for a ray
-- that finds nothing. be:getSurfaceHeightBelow reports failure as -1e20 rather than nil,
-- hence the magnitude guard.
-- -------------------------------------------------------------------------------------------
local function heightmapAt(x, y, hintZ)
  if not (core_terrain and core_terrain.getTerrainHeight) then return nil end
  local ok, h = pcall(core_terrain.getTerrainHeight, vec3(x, y, hintZ))
  if ok and type(h) == "number" then return h end
  return nil
end

local function rayAt(x, y, startZ)
  local ok, h = pcall(function() return be:getSurfaceHeightBelow(vec3(x, y, startZ)) end)
  if ok and type(h) == "number" and h > -1e10 then return h end
  return nil
end

-- -------------------------------------------------------------------------------------------
-- Water descriptors. Resolve scene objects once at scan start; per-cell work is then an XY
-- bounds test, plus River.containsPoint for the small set of plausible river candidates.
-- -------------------------------------------------------------------------------------------
local function worldBounds(obj)
  local ok, box = pcall(function() return obj:getWorldBox() end)
  if not (ok and box and box.minExtents and box.maxExtents) then return nil end
  return {
    minX = box.minExtents.x, maxX = box.maxExtents.x,
    minY = box.minExtents.y, maxY = box.maxExtents.y,
    minZ = box.minExtents.z, maxZ = box.maxExtents.z,
  }
end

local function inXY(bounds, x, y)
  return bounds == nil
      or (x >= bounds.minX and x <= bounds.maxX
          and y >= bounds.minY and y <= bounds.maxY)
end

local function readRiverNodes(obj)
  local nodes = {}
  local ok, count = pcall(function() return obj:getNodeCount() end)
  if not (ok and type(count) == "number") then return nodes end
  for i = 0, count - 1 do
    local okNode, pos, depth = pcall(function()
      return vec3(obj:getNodePosition(i)), obj:getNodeDepth(i)
    end)
    if okNode and pos and type(depth) == "number" then
      nodes[#nodes + 1] = {pos = pos, depth = math.max(0, depth)}
    end
  end
  return nodes
end

local function resolveWaterObjects()
  local waters = {}
  local counts = {River = 0, WaterBlock = 0, WaterPlane = 0}
  for _, class in ipairs({"River", "WaterBlock", "WaterPlane"}) do
    local ok, names = pcall(function() return scenetree.findClassObjects(class) end)
    if ok and names then
      for _, n in pairs(names) do
        local obj = scenetree.findObject(n)
        if obj then
          local desc = {class = class, name = tostring(n), obj = obj}
          desc.bounds = worldBounds(obj)
          if class == "River" then
            desc.nodes = readRiverNodes(obj)
          else
            local okPos, pos = pcall(function() return vec3(obj:getPosition()) end)
            if okPos and pos then desc.z = pos.z end
            -- WaterPlane is intentionally unbounded in XY. Its Z still belongs only to this
            -- descriptor; it is never folded into a single global water elevation.
            if class == "WaterPlane" then desc.bounds = nil end
          end
          if class == "River" or type(desc.z) == "number" then
            waters[#waters + 1] = desc
            counts[class] = counts[class] + 1
          end
        end
      end
    end
  end
  return waters, counts
end

local function riverBed(desc, p, segment)
  local i = math.floor(segment) + 1 -- engine segment 0 is Lua node 1 -> node 2
  local a, b = desc.nodes[i], desc.nodes[i + 1]
  if not (a and b) then return nil, nil end
  local vx, vy = b.pos.x - a.pos.x, b.pos.y - a.pos.y
  local denom = vx * vx + vy * vy
  local t = 0
  if denom > 1e-9 then
    t = ((p.x - a.pos.x) * vx + (p.y - a.pos.y) * vy) / denom
    t = math.max(0, math.min(1, t))
  end
  local surfaceZ = a.pos.z + (b.pos.z - a.pos.z) * t
  local depth = math.max(0, a.depth + (b.depth - a.depth) * t)
  return surfaceZ - depth, depth
end

-- Returns bedZ, depth, descriptor, needsBedRay. A point immediately below the sampled
-- visible surface is used for River containment, so a bridge above the volume remains dry.
local function waterAt(waters, p, surfaceZ)
  for _, w in ipairs(waters) do
    if w.class == "River" and inXY(w.bounds, p.x, p.y) then
      local ok, segment = pcall(function()
        return w.obj:containsPoint(vec3(p.x, p.y, surfaceZ - WATER_PROBE_EPS_M))
      end)
      if ok and type(segment) == "number" and segment >= 0 then
        local bedZ, depth = riverBed(w, p, segment)
        if bedZ ~= nil then return bedZ, depth, w, false end
      end
    end
  end

  local best = nil
  for _, w in ipairs(waters) do
    if w.class ~= "River" and inXY(w.bounds, p.x, p.y)
        and surfaceZ <= w.z + WATER_PROBE_EPS_M then
      -- Prefer the closest water surface above the hit if descriptors overlap.
      local gap = w.z - surfaceZ
      if best == nil or gap < best.gap then best = {desc = w, gap = gap} end
    end
  end
  if best then
    local w = best.desc
    if surfaceZ < w.z - WATER_PROBE_EPS_M then
      return surfaceZ, math.max(0, w.z - surfaceZ), w, false
    end
    return surfaceZ, 0, w, true
  end
  return nil, nil, nil, false
end

-- -------------------------------------------------------------------------------------------
-- Surface material.
--
-- NOTE, because the whole rest of this file is ray-budgeted and a reader will assume
-- otherwise: getMaterialIdxWs is an XY lookup into the terrain's own material layer, NOT a
-- raycast. It costs nothing against SCAN_RAYS_PER_TICK, which is why the three budgeted ray
-- sites are still three. It takes z = 0 for the same reason -- the z component is ignored.
-- -------------------------------------------------------------------------------------------
local function collisionTypeOf(name)
  if not (be.getGroundModelIDByName and be.getGroundModelByID) then return nil end
  local okId, id = pcall(function() return be:getGroundModelIDByName(name) end)
  if not okId or type(id) ~= "number" or id < 0 then return nil end
  local okGm, gm = pcall(function() return be:getGroundModelByID(id) end)
  if not okGm or not gm then return nil end
  local okCt, ct = pcall(function() return gm.data and gm.data.collisiontype end)
  if not okCt or type(ct) ~= "number" then return nil end
  return ct
end

-- collisiontype id -> family code, built once per scan from the representative names above.
-- Empty when the API is missing, in which case every cell reports no family and the scan
-- sounds exactly as it did before this feature existed.
local function resolveGroundFamilies()
  local byCollision = {}
  for fam, names in pairs(FAMILY_NAMES) do
    for _, name in ipairs(names) do
      local ct = collisionTypeOf(name)
      if ct then byCollision[ct] = fam end
    end
  end
  return byCollision
end

local function materialFamilyAt(x, y, surfaceZ)
  local terrain = scan.terrain
  if not terrain then return nil end
  -- The ray is authoritative for HEIGHT but the material layer is flat, so a cell whose ray
  -- landed on a bridge or a roof would otherwise be painted with whatever is under it.
  local groundZ = heightmapAt(x, y, surfaceZ)
  if groundZ == nil or math.abs(surfaceZ - groundZ) > MAT_TERRAIN_AGREE_M then return nil end
  local okIdx, idx = pcall(function() return terrain:getMaterialIdxWs(vec3(x, y, 0)) end)
  if not okIdx or type(idx) ~= "number" then return nil end
  local cached = scan.matCache[idx]
  if cached ~= nil then
    if cached == false then return nil end
    return cached
  end
  local name = nil
  local okMat, mtl = pcall(function() return terrain:getMaterial(idx) end)
  if okMat and mtl and mtl.getGroundmodelName then
    local okGm, gm = pcall(function() return mtl:getGroundmodelName() end)
    if okGm and type(gm) == "string" and gm ~= "" then name = gm end
  end
  if name == nil and terrain.getMaterialName then
    local okNm, nm = pcall(function() return terrain:getMaterialName(idx) end)
    if okNm and type(nm) == "string" and nm ~= "" then name = nm end
  end
  local fam = nil
  if name then
    local ct = collisionTypeOf(name)
    if ct then fam = scan.groundFamilies[ct] end
  end
  scan.matCache[idx] = fam or false
  return fam
end

-- -------------------------------------------------------------------------------------------
-- Road overlay. Pure maths against a prebuilt index; spends no rays either.
-- -------------------------------------------------------------------------------------------
local function bucketKey(bx, by)
  return tostring(bx) .. ":" .. tostring(by)
end

local function buildRoadIndex(origin, reach)
  local index, edgeCount = {}, 0
  local mapData = map and map.getMap and map.getMap() or nil
  local nodes = mapData and mapData.nodes or nil
  if not nodes then return index, 0 end
  local seen = {}
  for sourceId, sourceNode in pairs(nodes) do
    local links = sourceNode.links
    if type(links) == "table" then
      for targetId, data in pairs(links) do
        local targetNode = nodes[targetId]
        if type(data) == "table" and targetNode then
          local a, b = tostring(sourceId), tostring(targetId)
          local key = a < b and (a .. "|" .. b) or (b .. "|" .. a)
          if not seen[key] then
            seen[key] = true
            local inPos = data.inPos or sourceNode.pos
            local outPos = data.outPos or targetNode.pos
            local drivability = tonumber(data.drivability) or 1.0
            if inPos and outPos and drivability >= ROAD_MIN_DRIVABILITY then
              local ip = vec3(inPos.x, inPos.y, inPos.z)
              local op = vec3(outPos.x, outPos.y, outPos.z)
              local ir = tonumber(data.inRadius) or tonumber(sourceNode.radius) or 4.0
              local orr = tonumber(data.outRadius) or tonumber(targetNode.radius) or 4.0
              local rad = math.max(ir, orr)
              -- Only edges that can reach the scan disc, so a big map buckets a few hundred
              -- rather than tens of thousands.
              local near = math.min(
                math.sqrt((ip.x - origin.x) ^ 2 + (ip.y - origin.y) ^ 2),
                math.sqrt((op.x - origin.x) ^ 2 + (op.y - origin.y) ^ 2))
              if near <= reach + rad + ip:distance(op) then
                local edge = {a = ip, b = op, ra = ir, rb = orr}
                edgeCount = edgeCount + 1
                local minX = math.floor((math.min(ip.x, op.x) - rad) / ROAD_BUCKET_SIZE_M)
                local maxX = math.floor((math.max(ip.x, op.x) + rad) / ROAD_BUCKET_SIZE_M)
                local minY = math.floor((math.min(ip.y, op.y) - rad) / ROAD_BUCKET_SIZE_M)
                local maxY = math.floor((math.max(ip.y, op.y) + rad) / ROAD_BUCKET_SIZE_M)
                for bx = minX, maxX do
                  for by = minY, maxY do
                    local k = bucketKey(bx, by)
                    index[k] = index[k] or {}
                    table.insert(index[k], edge)
                  end
                end
              end
            end
          end
        end
      end
    end
  end
  return index, edgeCount
end

local function onRoad(index, x, y, surfaceZ)
  local bucket = index[bucketKey(math.floor(x / ROAD_BUCKET_SIZE_M),
                                math.floor(y / ROAD_BUCKET_SIZE_M))]
  if not bucket then return false end
  for _, e in ipairs(bucket) do
    local dx, dy = e.b.x - e.a.x, e.b.y - e.a.y
    local len2 = dx * dx + dy * dy
    local t = 0.0
    if len2 > 1e-9 then
      t = ((x - e.a.x) * dx + (y - e.a.y) * dy) / len2
      if t < 0 then t = 0 elseif t > 1 then t = 1 end
    end
    local px, py = e.a.x + dx * t, e.a.y + dy * t
    local pz = e.a.z + (e.b.z - e.a.z) * t
    local radius = e.ra + (e.rb - e.ra) * t
    local dist2 = (x - px) ^ 2 + (y - py) ^ 2
    -- The z test is what stops an overpass paving the ground beneath it.
    if dist2 <= radius * radius and math.abs(surfaceZ - pz) <= ROAD_Z_TOLERANCE_M then
      return true
    end
  end
  return false
end

-- The game exposes no render/draw-distance scalar to Lua at all -- only per-view farClip
-- values and $pref::Terrain::lodScale, neither of them queryable from here. The honest
-- analogue of "or the rendering distance, whichever comes first" is the terrain's own size:
-- past its edge there is nothing to sonify. Deliberately NOT the distance to the nearest
-- edge -- shrinking the whole scan because one flank runs out of map would compress the time
-- axis in every other direction too. Samples that fall off the map report as "no surface"
-- and render as silence, which is the right answer for them individually.
local function resolveReach()
  local reach = SCAN_MAX_RANGE_M
  if core_terrain and core_terrain.getTerrain then
    local ok, t = pcall(core_terrain.getTerrain)
    if ok and t then
      local ok2, box = pcall(function() return t:getWorldBox() end)
      if ok2 and box and box.minExtents and box.maxExtents then
        local dx = math.abs(box.maxExtents.x - box.minExtents.x)
        local dy = math.abs(box.maxExtents.y - box.minExtents.y)
        local span = math.max(dx, dy)
        if span > 0 and span < reach then reach = span end
      end
    end
  end
  return math.max(SCAN_MIN_REACH_M, reach)
end

-- -------------------------------------------------------------------------------------------
-- Objects. be:getObject enumerates spawned objects, which in BeamNG is what a "prop" is --
-- cones, barriers, haybales and pallets are all vehicle objects. Map-placed TSStatic is
-- deliberately NOT included even though scenetree.findClassObjects("TSStatic") would
-- enumerate it: on a stock map that is thousands of trees and building shells, and burying
-- the terrain bed under a wall of pings would make the instrument useless.
-- -------------------------------------------------------------------------------------------
local PROP_WORDS = {"cone", "barrier", "haybale", "pallet", "tirewall", "cardboard",
                    "barrel", "roadsign", "bollard", "christmas"}

local function objectKind(veh)
  local ok, f = pcall(function() return veh:getJBeamFilename() end)
  if ok and type(f) == "string" then
    local low = f:lower()
    for _, w in ipairs(PROP_WORDS) do
      if low:find(w, 1, true) then return "p" end
    end
  end
  return "v"
end

-- Map POIs -- garages, fuel, dealerships, mission starts, and whatever a level publishes
-- through its own onGetRawPoiListForLevel (the generated Proving Grounds markers included).
-- gameplay_rawPois is the same aggregation the big map itself uses, so the scan and the map
-- can never disagree about what is out there. Costs no rays, like gatherObjects.
local function gatherPois(origin, fwd, left, refZ, reach)
  local rows, seen, kept = {}, {}, 0
  if not (gameplay_rawPois and gameplay_rawPois.getRawPoiListByLevel) then return rows end
  local levelId = getCurrentLevelIdentifier and getCurrentLevelIdentifier() or nil
  if not levelId then return rows end
  local ok, list = pcall(gameplay_rawPois.getRawPoiListByLevel, levelId)
  if not ok or type(list) ~= "table" then return rows end
  for _, element in ipairs(list) do
    local marker = element and element.markerInfo and element.markerInfo.bigmapMarker
    local pos = marker and marker.pos or nil
    if pos then
      local rel = vec3(pos.x, pos.y, pos.z) - origin
      local f = rel:dot(fwd)
      local l = rel:dot(left)
      local d = math.sqrt(f * f + l * l)
      -- Past the reach there is no time slot for it, and pinning it to the last ring would
      -- state a distance that is not true. Dropped, like an off-map terrain cell.
      if f >= 0 and d <= reach and kept < POI_MAX then
        local bearing = math.deg(math.atan2(l, f))
        -- A bigmap marker's z is frequently a DISPLAY height floating above the ground, and
        -- the pitch is an elevation readout, so re-ground it where the terrain can say so.
        local groundZ = heightmapAt(pos.x, pos.y, pos.z) or pos.z
        local key = string.format("%.0f|%.0f", bearing, d)
        if not seen[key] then
          seen[key] = true
          kept = kept + 1
          rows[#rows + 1] = string.format("P,%.1f,%.1f,%.1f", bearing, d, groundZ - refZ)
        end
      end
    end
  end
  return rows
end

-- Translate both ordinary string keys and the {txt=..., ctx=...} values used by some stock
-- activities. Python still strips the small amount of HTML found in descriptions because
-- presentation cleanup belongs at the speech boundary.
local function poiText(value)
  if core_locales and core_locales.translateWithOrWithoutContext then
    local ok, translated = pcall(core_locales.translateWithOrWithoutContext, value)
    if ok and type(translated) == "string" and translated ~= "" then return translated end
  end
  if type(value) == "table" and value.txt then return tostring(value.txt) end
  if type(value) == "string" then return value end
  return ""
end

-- One result rather than a list: this is the named readout, not the scan's POI sound layer.
-- Ties prefer a marker with a description, which collapses the common pairing of a richly
-- described level marker and a bare spawn marker at exactly the same coordinates.
local function nearestPoi()
  local player = be:getPlayerVehicle(0)
  if not player then return nil, "no vehicle" end
  if not (gameplay_rawPois and gameplay_rawPois.getRawPoiListByLevel) then
    return nil, "point of interest service unavailable"
  end
  local levelId = getCurrentLevelIdentifier and getCurrentLevelIdentifier() or nil
  if not levelId then return nil, "no level" end

  if gameplay_rawPois.clear then pcall(gameplay_rawPois.clear) end
  local ok, list = pcall(gameplay_rawPois.getRawPoiListByLevel, levelId)
  if not ok or type(list) ~= "table" then return nil, "could not read points of interest" end

  local origin = vec3(player:getPosition())
  local fwd = vec3(player:getDirectionVector())
  fwd.z = 0
  if fwd:length() < 1e-4 then return nil, "no heading" end
  fwd = fwd:normalized()
  local left = vec3(0, 0, 1):cross(fwd):normalized()
  local best = nil
  for _, element in ipairs(list) do
    local marker = element and element.markerInfo and element.markerInfo.bigmapMarker
    local pos = marker and marker.pos or nil
    if pos then
      local dx, dy = pos.x - origin.x, pos.y - origin.y
      local distance = math.sqrt(dx * dx + dy * dy)
      local description = poiText(marker.description
        or (element.data and element.data.description))
      local richer = best and description ~= "" and best.description == ""
      if best == nil or distance < best.distance - 0.25
          or (math.abs(distance - best.distance) <= 0.25 and richer) then
        local rel = vec3(dx, dy, 0)
        best = {
          name = poiText(marker.name or (element.data and element.data.name)),
          description = description,
          distance = distance,
          bearing = distance < 1e-6 and 0 or math.deg(math.atan2(rel:dot(left), rel:dot(fwd))),
          radius = math.max(0, tonumber(marker.radius) or 0),
        }
      end
    end
  end
  if not best then return nil, "no points of interest found" end
  if best.name == "" then best.name = "Point of interest" end
  return best
end

local function sendNearestPoi()
  local result, reason = nearestPoi()
  if not result then
    send("POI_FAIL:" .. tostring(reason or "unknown error"))
    return
  end
  send("POI:" .. jsonEncode(result))
end

local function gatherObjects(origin, fwd, left, refZ, reach)
  local rows = {}
  local playerID = -1
  local p = be:getPlayerVehicle(0)
  if p then playerID = p:getID() end
  for i = 0, be:getObjectCount() - 1 do
    local obj = be:getObject(i)
    if obj and obj:getID() ~= playerID then
      local ok, pos = pcall(function() return vec3(obj:getPosition()) end)
      if ok and pos then
        local rel = pos - origin
        local f = rel:dot(fwd)
        local l = rel:dot(left)
        local d = math.sqrt(f * f + l * l)
        if f >= 0 and d <= reach then
          rows[#rows + 1] = string.format("O,%.1f,%.1f,%.1f,%s",
            math.deg(math.atan2(l, f)), d, pos.z - refZ, objectKind(obj))
        end
      end
    end
  end
  return rows
end

-- -------------------------------------------------------------------------------------------
-- The scan itself.
-- -------------------------------------------------------------------------------------------
local function beginScan()
  local player = be:getPlayerVehicle(0)
  if not player then
    send("FAIL,no vehicle")
    return
  end
  local origin = vec3(player:getPosition())
  local fwd = vec3(player:getDirectionVector())
  fwd.z = 0
  if fwd:length() < 1e-4 then
    send("FAIL,no heading")
    return
  end
  fwd = fwd:normalized()
  -- up X fwd, the mod-wide convention: positive bearing is the driver's LEFT.
  local left = vec3(0, 0, 1):cross(fwd):normalized()

  local waters, waterCounts = resolveWaterObjects()
  local reach = resolveReach()
  local okTerrain, terrain = pcall(function()
    return core_terrain and core_terrain.getTerrain and core_terrain.getTerrain() or nil
  end)
  local roadIndex, roadEdges = buildRoadIndex(origin, reach)
  scan = {
    id      = nextId,
    origin  = origin,
    fwd     = fwd,
    left    = left,
    refZ    = nil,         -- first budgeted ray establishes visible ground below the vehicle
    waters  = waters,
    waterCounts = waterCounts,
    reach   = reach,
    -- nil on smallgrid and gridmap, which have no TerrainBlock at all. Every cell then
    -- reports no family and the scan sounds exactly as it did before this feature existed.
    terrain = okTerrain and terrain or nil,
    groundFamilies = resolveGroundFamilies(),
    matCache = {},         -- material index -> family code, or false for "resolved to none"
    roadIndex = roadIndex,
    roadEdges = roadEdges,
    roadCells = 0,         -- how many cells the overlay overrode; see M.diag
    famCounts = {},
    s       = 0,           -- spoke index being filled
    r       = 0,           -- ring index within it
    cur     = nil,         -- the row under construction
    rows    = {},
    pendingBed = nil,
    rayCount = 0,
    surfaceMin = nil,
    surfaceMax = nil,
    waterCells = 0,
    dryCells = 0,
    missingCells = 0,
  }
  nextId = nextId + 1
end

local function finishScan()
  local objs = gatherObjects(scan.origin, scan.fwd, scan.left, scan.refZ, scan.reach)
  local pois = gatherPois(scan.origin, scan.fwd, scan.left, scan.refZ, scan.reach)
  local parts = {
    string.format("SCAN,%d,%d,%d,%.2f,%.2f",
      scan.id, SCAN_BEARINGS, SCAN_RINGS, scan.reach, scan.refZ)
  }
  for _, row in ipairs(scan.rows) do parts[#parts + 1] = row end
  for _, row in ipairs(objs) do parts[#parts + 1] = row end
  for _, row in ipairs(pois) do parts[#parts + 1] = row end
  parts[#parts + 1] = "END"
  send(table.concat(parts, "\n"))
  lastScanDiag = {
    id = scan.id, refZ = scan.refZ,
    surfaceMin = scan.surfaceMin, surfaceMax = scan.surfaceMax,
    waterCells = scan.waterCells, dryCells = scan.dryCells,
    missingCells = scan.missingCells, rayCount = scan.rayCount,
    famCounts = scan.famCounts, roadCells = scan.roadCells, roadEdges = scan.roadEdges,
    hasTerrain = scan.terrain ~= nil, poiCount = #pois,
  }
  tsLog('D', string.format(
    "scan %d sent: reach %.1f m, refZ %.2f, surface %s..%s, water/dry/missing %d/%d/%d, rays %d, %d objects",
    scan.id, scan.reach, scan.refZ, tostring(scan.surfaceMin), tostring(scan.surfaceMax),
    scan.waterCells, scan.dryCells, scan.missingCells, scan.rayCount, #objs))
  scan = nil
end

-- The suffix is OPTIONAL and its absence means paved -- which is also what an unresolved
-- material means, so the two halves going out of step degrades to today's tone rather than
-- to an error. The road overlay wins over the terrain layer and reports itself as "r", a
-- separate code that audio.py renders identically to paved: a radius-against-the-road-graph
-- rule is a heuristic, and a heuristic whose effect cannot be counted is one nobody can
-- debug. M.diag counts it.
local function familySuffix(p, surfaceZ)
  local fam = materialFamilyAt(p.x, p.y, surfaceZ)
  if onRoad(scan.roadIndex, p.x, p.y, surfaceZ) then
    scan.roadCells = scan.roadCells + 1
    fam = "r"
  end
  if fam == nil then return "" end
  scan.famCounts[fam] = (scan.famCounts[fam] or 0) + 1
  return ":" .. fam
end

local function appendCell(cell, kind)
  scan.cur[#scan.cur + 1] = cell
  scan.r = scan.r + 1
  if kind == "water" then
    scan.waterCells = scan.waterCells + 1
  elseif kind == "dry" then
    scan.dryCells = scan.dryCells + 1
  else
    scan.missingCells = scan.missingCells + 1
  end
end

local function stepScan()
  local rays = 0
  local ringStep = scan.reach / math.max(1, SCAN_RINGS - 1)
  local bearStep = 180.0 / math.max(1, SCAN_BEARINGS - 1)

  if scan.refZ == nil then
    if rays >= SCAN_RAYS_PER_TICK then return end
    rays = rays + 1
    scan.rayCount = scan.rayCount + 1
    -- Unlike the map samples, the reference ray begins just above the vehicle: it asks for
    -- the visible supporting surface beneath the vehicle, not the roof of an overpass above.
    scan.refZ = rayAt(scan.origin.x, scan.origin.y, scan.origin.z + 2)
                or heightmapAt(scan.origin.x, scan.origin.y, scan.origin.z)
                or scan.origin.z
  end

  while scan.s < SCAN_BEARINGS do
    local bearing = -90.0 + scan.s * bearStep
    if scan.cur == nil then
      local rad = math.rad(bearing)
      scan.dir = scan.fwd * math.cos(rad) + scan.left * math.sin(rad)
      scan.cur = {string.format("S,%.1f", bearing)}
      scan.r = 0
    end

    while scan.r < SCAN_RINGS do
      if rays >= SCAN_RAYS_PER_TICK then return end  -- resume next frame

      if scan.pendingBed then
        local pending = scan.pendingBed
        rays = rays + 1
        scan.rayCount = scan.rayCount + 1
        local bedZ = rayAt(pending.p.x, pending.p.y,
                           pending.water.z - WATER_PROBE_EPS_M)
                     or heightmapAt(pending.p.x, pending.p.y,
                                    pending.water.z - WATER_PROBE_EPS_M)
        if bedZ == nil or bedZ > pending.water.z + WATER_PROBE_EPS_M then
          bedZ = pending.surfaceZ
        end
        appendCell(string.format("%.1f_%.1f", bedZ - scan.refZ,
          math.max(0, pending.water.z - bedZ)), "water")
        scan.pendingBed = nil
      else
        local dist = scan.r * ringStep
        local p = scan.origin + scan.dir * dist
        rays = rays + 1
        scan.rayCount = scan.rayCount + 1
        local h = rayAt(p.x, p.y, SCAN_RAY_TOP_Z)
        if h == nil then h = heightmapAt(p.x, p.y, scan.origin.z) end
        if h == nil then
          -- No surface here. This is NOT zero: zero is level ground, and reporting a plateau
          -- where the map simply ends is a lie the listener has no way to catch.
          appendCell("~", "missing")
        else
          scan.surfaceMin = scan.surfaceMin == nil and h or math.min(scan.surfaceMin, h)
          scan.surfaceMax = scan.surfaceMax == nil and h or math.max(scan.surfaceMax, h)
          local bedZ, depth, water, needsBedRay = waterAt(scan.waters, p, h)
          if water and needsBedRay then
            scan.pendingBed = {p = p, surfaceZ = h, water = water}
          elseif water then
            -- Water carries no family: depth is authoritative, and the material under a lake
            -- is not the thing being looked at.
            appendCell(string.format("%.1f_%.1f", bedZ - scan.refZ, depth), "water")
          else
            appendCell(string.format("%.1f", h - scan.refZ) .. familySuffix(p, h), "dry")
          end
        end
      end
    end

    scan.rows[#scan.rows + 1] = table.concat(scan.cur, ",")
    scan.cur = nil
    scan.s = scan.s + 1
  end

  finishScan()
end

-- -------------------------------------------------------------------------------------------
local function setupSockets()
  if udpSend then pcall(function() udpSend:close() end); udpSend = nil end
  if udpCmd  then pcall(function() udpCmd:close()  end); udpCmd  = nil end

  udpSend = socket.udp()
  if udpSend then
    udpSend:setpeername(PYTHON_HOST, PYTHON_PORT_DATA)
    udpSend:settimeout(0)
  else
    tsLog('E', "Failed to create UDP send socket.")
  end

  local ok, err = pcall(function()
    udpCmd = socket.udp()
    -- setsockname RETURNS nil plus a message; it does not THROW. A pcall around it reports
    -- success on a socket bound to nothing, and the extension then goes deaf with nothing in
    -- the log -- it still sends normally, because a UDP sender needs no bind.
    local bound, berr = udpCmd:setsockname("127.0.0.1", CMD_LISTEN_PORT)
    if not bound then error(tostring(berr), 0) end
    udpCmd:settimeout(0)
  end)
  if not (ok and udpCmd) then
    tsLog('E', "Failed to bind command socket on port " .. tostring(CMD_LISTEN_PORT)
      .. ": " .. tostring(err))
    if udpCmd then pcall(function() udpCmd:close() end) end
    udpCmd = nil
  end
end

-- A failed bind is otherwise permanent for the session, so re-arm it. This is the recovery
-- path, not a precaution, and it has been watched doing the job: the first reload of the
-- patched files leaked eight ports, because the OUTGOING code had no unload hook yet. The
-- retry could not take them while the old module tables were still referenced -- a socket held
-- that way is not one the collector is about to free -- and ticked uselessly for two minutes.
-- The Ctrl+L that followed did NOT re-load these extensions (no load line for any of them in
-- the log at that timestamp, so setupSockets never ran again); all thirteen ports came back
-- through THIS function instead, within one frame of each other, the moment those tables went
-- away. Without it the mod would have stayed deaf until the game was restarted.
local CMD_BIND_RETRY_S = 3.0
local cmdBindRetry = 0

local function retryCmdBind(dtReal)
  if udpCmd then return end
  cmdBindRetry = cmdBindRetry + (dtReal or 0)
  if cmdBindRetry < CMD_BIND_RETRY_S then return end
  cmdBindRetry = 0
  local ok = pcall(function()
    local sk = socket.udp()
    local bound = sk:setsockname("127.0.0.1", CMD_LISTEN_PORT)
    if not bound then sk:close(); error("still in use", 0) end
    sk:settimeout(0)
    udpCmd = sk
  end)
  if ok and udpCmd then
    tsLog('I', "UDP command socket bound on port " .. CMD_LISTEN_PORT .. " after retry.")
  end
end

-- setupSockets closes the sockets held by THIS module instance, and extensions.reload builds a
-- fresh instance whose locals are nil -- so it closes nothing and the outgoing instance keeps
-- the port, leaving the reloaded copy permanently deaf. Hence this hook.
function M.onExtensionUnloaded()
  if udpSend then pcall(function() udpSend:close() end); udpSend = nil end
  if udpCmd  then pcall(function() udpCmd:close()  end); udpCmd  = nil end
end

function M.onExtensionLoaded()
  setExtensionUnloadMode(M, "manual")
  setupSockets()
  tsLog('I', "Terrain scanner loaded.")
end

function M.onWorldReadyState(state)
  if state == 2 then
    scan = nil
    setupSockets()
  end
end

function M.onUpdate(dtReal, dtSim, dtRaw)
  retryCmdBind(dtReal)
  if udpCmd then
    local data = udpCmd:receive()
    if data then
      local cmd = data:match("^%s*(.-)%s*$"):upper()
      if cmd == "SCAN" then
        -- A new request supersedes one still gathering. Finishing the old one first would
        -- send a snapshot of where the vehicle used to be.
        scan = nil
        local ok, err = pcall(beginScan)
        if not ok then
          scan = nil
          tsLog('E', "beginScan failed: " .. tostring(err))
          send("FAIL,scan error")
        end
      elseif cmd == "NEAREST_POI" then
        local ok, err = pcall(sendNearestPoi)
        if not ok then
          tsLog('E', "nearest POI failed: " .. tostring(err))
          send("POI_FAIL:lookup error")
        end
      end
    end
  end

  if scan then
    -- The GE onUpdate chain is dispatched WITHOUT pcall, so a throw in here would silently
    -- stop every extension loaded after this one in modScript.lua rather than just breaking
    -- the scanner.
    local ok, err = pcall(stepScan)
    if not ok then
      scan = nil
      tsLog('E', "stepScan failed: " .. tostring(err))
      send("FAIL,scan error")
    end
  end
end

-- Diagnostics: the failure mode of a name-driven or geometry-driven resolve is a confident
-- wrong number, which no amount of listening will identify. This prints what one scan
-- actually decided.
function M.diag()
  local waters, counts = resolveWaterObjects()
  local reach = resolveReach()
  local names = {River = {}, WaterBlock = {}, WaterPlane = {}}
  for _, w in ipairs(waters) do names[w.class][#names[w.class] + 1] = w.name end
  local waterSummary = string.format("River %d [%s], WaterBlock %d [%s], WaterPlane %d [%s]",
    counts.River, table.concat(names.River, ","),
    counts.WaterBlock, table.concat(names.WaterBlock, ","),
    counts.WaterPlane, table.concat(names.WaterPlane, ","))
  local last = "none"
  if lastScanDiag then
    -- The family histogram, the overlay count and the POI count are here for the reason the
    -- water summary already is: the failure mode of a name-driven resolve is a confident
    -- WRONG number, and no amount of listening will identify it. "0 edges bucketed" and
    -- "every cell unknown" sound identical from the seat and want different fixes.
    local fam = {}
    for _, code in ipairs({"d", "v", "g", "i", "r"}) do
      fam[#fam + 1] = code .. "=" .. tostring((lastScanDiag.famCounts or {})[code] or 0)
    end
    last = string.format(
      "id %d | ref z %.2f | visible surface %s..%s | water/dry/missing %d/%d/%d | rays %d"
      .. " | terrain %s | families %s | road overlay %d cells from %d edges | %d POIs",
      lastScanDiag.id, lastScanDiag.refZ, tostring(lastScanDiag.surfaceMin),
      tostring(lastScanDiag.surfaceMax), lastScanDiag.waterCells, lastScanDiag.dryCells,
      lastScanDiag.missingCells, lastScanDiag.rayCount,
      lastScanDiag.hasTerrain and "yes" or "NO (every cell unknown, i.e. today's tone)",
      table.concat(fam, ","), lastScanDiag.roadCells or 0, lastScanDiag.roadEdges or 0,
      lastScanDiag.poiCount or 0)
  end
  tsLog('I', string.format(
    "water objects: %s | reach %.1f m | grid %dx%d = %d samples | last scan: %s",
    waterSummary, reach, SCAN_BEARINGS, SCAN_RINGS, SCAN_BEARINGS * SCAN_RINGS, last))
end

-- Console-accessible and useful to diagnostics without putting a datagram on Python's port.
M.nearestPoi = nearestPoi

return M
