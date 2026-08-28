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
local CMD_LISTEN_PORT   = 4472   -- receive SCAN from Python

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
  scan = {
    id      = nextId,
    origin  = origin,
    fwd     = fwd,
    left    = left,
    refZ    = nil,         -- first budgeted ray establishes visible ground below the vehicle
    waters  = waters,
    waterCounts = waterCounts,
    reach   = resolveReach(),
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
  local parts = {
    string.format("SCAN,%d,%d,%d,%.2f,%.2f",
      scan.id, SCAN_BEARINGS, SCAN_RINGS, scan.reach, scan.refZ)
  }
  for _, row in ipairs(scan.rows) do parts[#parts + 1] = row end
  for _, row in ipairs(objs) do parts[#parts + 1] = row end
  parts[#parts + 1] = "END"
  send(table.concat(parts, "\n"))
  lastScanDiag = {
    id = scan.id, refZ = scan.refZ,
    surfaceMin = scan.surfaceMin, surfaceMax = scan.surfaceMax,
    waterCells = scan.waterCells, dryCells = scan.dryCells,
    missingCells = scan.missingCells, rayCount = scan.rayCount,
  }
  tsLog('D', string.format(
    "scan %d sent: reach %.1f m, refZ %.2f, surface %s..%s, water/dry/missing %d/%d/%d, rays %d, %d objects",
    scan.id, scan.reach, scan.refZ, tostring(scan.surfaceMin), tostring(scan.surfaceMax),
    scan.waterCells, scan.dryCells, scan.missingCells, scan.rayCount, #objs))
  scan = nil
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
            appendCell(string.format("%.1f_%.1f", bedZ - scan.refZ, depth), "water")
          else
            appendCell(string.format("%.1f", h - scan.refZ), "dry")
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
    last = string.format(
      "id %d | ref z %.2f | visible surface %s..%s | water/dry/missing %d/%d/%d | rays %d",
      lastScanDiag.id, lastScanDiag.refZ, tostring(lastScanDiag.surfaceMin),
      tostring(lastScanDiag.surfaceMax), lastScanDiag.waterCells, lastScanDiag.dryCells,
      lastScanDiag.missingCells, lastScanDiag.rayCount)
  end
  tsLog('I', string.format(
    "water objects: %s | reach %.1f m | grid %dx%d = %d samples | last scan: %s",
    waterSummary, reach, SCAN_BEARINGS, SCAN_RINGS, SCAN_BEARINGS * SCAN_RINGS, last))
end

return M
