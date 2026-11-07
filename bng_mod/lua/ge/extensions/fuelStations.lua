-- =================================================================================================
--
--  Fuel Stations for BeamNG.drive Accessibility Mod (GE Extension)
--
--  Finds every place on the loaded map where a car can be refuelled or recharged, lets Python
--  set one as the big-map route, announces arrival, and on request has the vehicle VM park the
--  car at a free pump (beamtelFuelPark.lua) and hand it back.
--
--  Catalogue, built when the world becomes ready and spread over frames:
--    * official stations: freeroam_facilities' gasStations, the same data the stock refuel
--      prompt is driven by. Each pump is a BeamNGGameplayArea, a thin oriented box beside the
--      pump body; the stock prompt opens when the vehicle's OOBB overlaps it.
--    * unofficial pumps: pump/charger TSStatic shapes not near any official pump, clustered.
--      Listed so the player knows they exist; the game cannot refuel there.
--
--  Commands (UDP on CMD_LISTEN_PORT, one per datagram):
--    LIST          -> FS_WAIT while building, then FS_BEGIN:<n> / FS:<json> ... / FS_END
--    TARGET:<id>   -> route to the station; parks straight away if already there
--    PARK          -> park at a free pump of the target station
--    PARK_CANCEL   -> stop parking, keep the target
--    CANCEL        -> stop parking and forget the target
--    STATUS        -> FS_EVENT status
--  Events go out as FS_EVENT:<json> with a `kind`.
--
--  See docs/lua-fuel-stations.md.
--
-- =================================================================================================

local M = {}

local PYTHON_HOST = "127.0.0.1"
local DATA_PORT = 4486       -- rows and events to Python
local CMD_LISTEN_PORT = 4488 -- commands from Python

local IN_RANGE_M = 40        -- beyond the station's own radius
local LEAVE_RANGE_M = 70
local PARK_REACH_M = 60      -- PARK is refused further out than this past the radius
local UNOFFICIAL_MERGE_M = 15
local CLUSTER_M = 25
local STATICS_PER_TICK = 2000
local TANK_TIMEOUT_S = 1.0
local LEAD_IN_M = 10
local PRE_LEAD_M = 8

local udpSend, udpCmd = nil, nil
local function fsLog(level, msg) log(level, 'FuelStations', msg) end
local function send(line)
  if udpSend then pcall(function() udpSend:send(line) end) end
end
local function event(kind, data)
  data = data or {}
  data.kind = kind
  send("FS_EVENT:" .. jsonEncode(data))
end

-- ---------------------------------------------------------------------------------------------
-- Catalogue
-- ---------------------------------------------------------------------------------------------

local stations, byId = {}, {}
local build = nil          -- {phase, names, i, hits}
local ready = false
local listWanted = false   -- a LIST arrived while building
local target = nil         -- station id
local inRange = false
local watchAcc = 0
local park = nil
local tankRequest = nil    -- {vid, age, result, done, callbacks}
local lastDiag = {}

local function flat(v) return vec3(v.x, v.y, 0) end

local function prettify(text)
  text = tostring(text or ""):gsub("^fuel_", ""):gsub("[_%-]+", " ")
  return (text:gsub("(%a)([%w']*)", function(a, b) return a:upper() .. b end))
end

local function translated(key, fallbackId)
  local text
  if core_locales and core_locales.translateWithOrWithoutContext then
    local ok, value = pcall(core_locales.translateWithOrWithoutContext, key)
    if ok and type(value) == "string" then text = value end
  end
  -- An untranslated key comes back unchanged; a modded map without a locale file is the
  -- common case, so fall back to something speakable rather than "levels dot ...".
  if not text or text == "" or text == key or text:find("^levels%.") then
    local segment = type(key) == "string" and key:match("([^%.]+)%.title$")
    text = prettify(segment or fallbackId or "Fuel station")
  end
  return text
end

local function groundZ(pos)
  local origin = vec3(pos.x, pos.y, pos.z + 2)
  local hit = castRayStatic(origin, vec3(0, 0, -1), 6)
  if hit and hit < 6 then return origin.z - hit end
  if core_terrain and core_terrain.getTerrainHeight then
    local h = core_terrain.getTerrainHeight(pos)
    if h and h > -1e6 then return h end
  end
  return pos.z
end

local function addStation(st)
  local center, count = vec3(0, 0, 0), 0
  for _, p in ipairs(st.pumps) do center = center + p.pos; count = count + 1 end
  if count == 0 then return end
  center = center / count
  local radius = 0
  for _, p in ipairs(st.pumps) do radius = math.max(radius, (p.pos - center):length()) end
  st.center, st.radius = center, radius
  stations[#stations + 1] = st
  byId[st.id] = st
end

local function buildOfficial()
  local level = getCurrentLevelIdentifier and getCurrentLevelIdentifier()
  if not (level and freeroam_facilities and freeroam_facilities.getFacilities) then return end
  local ok, facilities = pcall(freeroam_facilities.getFacilities, level)
  if not ok or type(facilities) ~= "table" then
    fsLog('W', "facilities unavailable: " .. tostring(facilities))
    return
  end
  for index, f in ipairs(facilities.gasStations or {}) do
    local st = {id = "o:" .. tostring(f.id or index), facilityId = f.id, official = true,
      name = translated(f.name, f.id), energy = {}, pumps = {}}
    for _, e in ipairs(f.energyTypes or {"any"}) do
      if e ~= "unknown" then st.energy[#st.energy + 1] = e end
    end
    for _, pair in ipairs(f.pumps or {}) do
      local area = type(pair) == "table" and scenetree.findObject(pair[1])
      if area then
        local pos, rot, scl = area:getPosition(), quat(area:getRotation()), area:getScale()
        local xv, yv, zv = rot * vec3(1, 0, 0) * scl.x, rot * vec3(0, 1, 0) * scl.y, rot * vec3(0, 0, 1) * scl.z
        -- The long side is whichever is longer: Italy's Norte station has its areas turned.
        local long, lat = xv, yv
        if yv:length() > xv:length() then long, lat = yv, xv end
        st.pumps[#st.pumps + 1] = {pos = vec3(pos), xVec = xv, yVec = yv, zVec = zv,
          long = flat(long):normalized(), lat = flat(lat):normalized(),
          halfLong = long:length(), halfLat = lat:length()}
      end
    end
    addStation(st)
  end
end

-- Pump bodies and chargers, by shape file name. "pump" alone is far too broad (water pumps,
-- oil pumpjacks, pumpkins), so it must come with a fuel word; the excludes are the false
-- positives seen in the stock levels (tank farms, gas terminals, airport refuelling props).
local EXCLUDE = {"tank", "terminal", "pallet", "airport", "number", "sign", "jack", "water", "pumpkin"}
local FUEL_WORDS = {"fuel", "gas", "petrol", "diesel", "station"}

local function classifyShape(shapeName)
  if type(shapeName) ~= "string" or shapeName == "" then return nil end
  local base = shapeName:lower():match("([^/\\]+)$") or ""
  base = base:gsub("%.[^.]+$", "")
  for _, word in ipairs(EXCLUDE) do
    if base:find(word, 1, true) then return nil end
  end
  if base:find("charging_station", 1, true) or base:find("ev_charger", 1, true)
      or base:find("evcharger", 1, true) then
    return "charger"
  end
  if base:find("pump", 1, true) then
    for _, word in ipairs(FUEL_WORDS) do
      if base:find(word, 1, true) then return "pump" end
    end
  end
  return nil
end
M.classifyShape = classifyShape

local function nearestPoiName(pos)
  if not (gameplay_rawPois and gameplay_rawPois.getRawPoiListByLevel) then return nil end
  local ok, list = pcall(gameplay_rawPois.getRawPoiListByLevel, getCurrentLevelIdentifier())
  if not ok or type(list) ~= "table" then return nil end
  local best, bestD
  for _, element in ipairs(list) do
    local marker = element and element.markerInfo and element.markerInfo.bigmapMarker
    if marker and marker.pos and marker.name then
      local d = (flat(vec3(marker.pos)) - flat(pos)):length()
      if d <= 300 and (not bestD or d < bestD) then best, bestD = marker.name, d end
    end
  end
  return best and translated(best, nil) or nil
end

local function finishUnofficial(hits)
  local official = {}
  for _, st in ipairs(stations) do
    for _, p in ipairs(st.pumps) do official[#official + 1] = p.pos end
  end
  local kept = {}
  for _, hit in ipairs(hits) do
    local near = false
    for _, pos in ipairs(official) do
      if (pos - hit.pos):length() <= UNOFFICIAL_MERGE_M then near = true; break end
    end
    if not near then kept[#kept + 1] = hit end
  end
  -- Single-link clustering: a forecourt is a chain of pumps a few metres apart.
  local cluster = {}
  local clusters = {}
  for i = 1, #kept do
    if not cluster[i] then
      local members, queue = {}, {i}
      cluster[i] = true
      while #queue > 0 do
        local j = table.remove(queue)
        members[#members + 1] = kept[j]
        for k = 1, #kept do
          if not cluster[k] and (kept[k].pos - kept[j].pos):length() <= CLUSTER_M then
            cluster[k] = true
            queue[#queue + 1] = k
          end
        end
      end
      clusters[#clusters + 1] = members
    end
  end
  for index, members in ipairs(clusters) do
    local hasPump, hasCharger = false, false
    local pumps = {}
    for _, m in ipairs(members) do
      if m.kind == "charger" then hasCharger = true else hasPump = true end
      pumps[#pumps + 1] = {pos = m.pos, unofficial = true}
    end
    local energy = {}
    if hasPump then energy[#energy + 1] = "fuel" end
    if hasCharger then energy[#energy + 1] = "electricEnergy" end
    local st = {id = "u:" .. index, official = false, energy = energy, pumps = pumps}
    addStation(st)
    local what = hasPump and "fuel pumps" or "chargers"
    local near = nearestPoiName(st.center)
    st.name = near and ("Unmarked " .. what .. " near " .. near) or ("Unmarked " .. what)
  end
end

local function startBuild()
  stations, byId = {}, {}
  ready, inRange, target = false, false, nil
  local ok, err = pcall(buildOfficial)
  if not ok then fsLog('E', "official stations failed: " .. tostring(err)) end
  local names = scenetree.findClassObjects and scenetree.findClassObjects('TSStatic') or {}
  build = {names = names, i = 1, hits = {}, started = os.clock()}
end

local function stepBuild()
  local names, last = build.names, math.min(#build.names, build.i + STATICS_PER_TICK - 1)
  for i = build.i, last do
    local o = scenetree.findObject(names[i])
    local kind = o and classifyShape(o.shapeName)
    if kind then build.hits[#build.hits + 1] = {pos = vec3(o:getPosition()), kind = kind} end
  end
  build.i = last + 1
  if build.i <= #names then return end
  local hits = build.hits
  build = nil
  local ok, err = pcall(finishUnofficial, hits)
  if not ok then fsLog('E', "unofficial pumps failed: " .. tostring(err)) end
  ready = true
  local official = 0
  for _, st in ipairs(stations) do if st.official then official = official + 1 end end
  lastDiag.official, lastDiag.unofficial, lastDiag.shapeHits = official, #stations - official, #hits
  fsLog('I', string.format("%d official, %d unofficial stations (%d pump shapes)",
    official, #stations - official, #hits))
  event("scan_done", {count = #stations})
end

-- ---------------------------------------------------------------------------------------------
-- The player's tanks, for "not for your vehicle"
-- ---------------------------------------------------------------------------------------------

local MAIN_TYPES = {gasoline = true, diesel = true, kerosine = true, electricEnergy = true}

local function requestTanks(veh, callback)
  if tankRequest and tankRequest.vid == veh:getID() and not tankRequest.done then
    table.insert(tankRequest.callbacks, callback)
    return
  end
  local request = {vid = veh:getID(), age = 0, callbacks = {callback}}
  tankRequest = request
  local ok = pcall(core_vehicleBridge.requestValue, veh, function(ret)
    if tankRequest ~= request or request.done then return end
    local types = {}
    for _, tank in ipairs((ret and ret[1]) or {}) do
      local kind = tank.energyType == "kerosene" and "kerosine" or tank.energyType
      if MAIN_TYPES[kind] then types[kind] = true end
    end
    request.result = types
  end, 'energyStorage')
  if not ok then request.age = TANK_TIMEOUT_S end
end

local function stepTanks(dt)
  local request = tankRequest
  if not request or request.done then return end
  request.age = request.age + dt
  if request.result or request.age >= TANK_TIMEOUT_S then
    request.done = true
    for _, cb in ipairs(request.callbacks) do pcall(cb, request.result) end
  end
end

-- true / false, or nil when it cannot be said (unknown tanks, unofficial station).
local function compatible(st, tanks)
  if not st.official or not tanks or next(tanks) == nil then return nil end
  local offers = {}
  for _, e in ipairs(st.energy) do offers[e] = true end
  if offers.any then return true end
  for kind in pairs(tanks) do if offers[kind] then return true end end
  return false
end

-- ---------------------------------------------------------------------------------------------
-- Geometry
-- ---------------------------------------------------------------------------------------------

local UP = vec3(0, 0, 1)

local function playerFrame(veh)
  local fwd = flat(vec3(veh:getDirectionVector()))
  if fwd:length() < 1e-4 then fwd = vec3(0, 1, 0) end
  fwd = fwd:normalized()
  local left = UP:cross(fwd):normalized()
  return vec3(veh:getPosition()), fwd, left
end

local function relative(veh, pos)
  local origin, fwd, left = playerFrame(veh)
  local rel = flat(pos - origin)
  local distance = rel:length()
  local bearing = distance < 1e-6 and 0 or math.deg(math.atan2(rel:dot(left), rel:dot(fwd)))
  return distance, bearing
end

local function oobb(vid)
  local c = vec3(be:getObjectOOBBCenterXYZ(vid))
  return c, vec3(be:getObjectOOBBHalfAxisXYZ(vid, 0)), vec3(be:getObjectOOBBHalfAxisXYZ(vid, 1)),
    vec3(be:getObjectOOBBHalfAxisXYZ(vid, 2))
end

local function extentAlong(h0, h1, h2, axis)
  return math.abs(h0:dot(axis)) + math.abs(h1:dot(axis)) + math.abs(h2:dot(axis))
end

local function overlapsPump(vid, pump)
  if not pump.xVec then return false end
  local c, h0, h1, h2 = oobb(vid)
  return overlapsOBB_OBB(c, h0, h1, h2, pump.pos, pump.xVec, pump.yVec, pump.zVec) and true or false
end

-- Whether a straight run between two ground points is free at a given height. The last metre
-- is not judged: the stop point sits against the island by design, and stopBeside has already
-- cleared it sideways; a ray grazing the kerb end there is not a wall in the way.
local function clearRun(a, b, height)
  local from = vec3(a.x, a.y, a.z + height)
  local to = vec3(b.x, b.y, b.z + height)
  local dir = to - from
  local len = dir:length()
  if len < 1.0 then return true end
  local hit = castRayStatic(from, dir / len, len)
  return not hit or hit >= len - 1.0
end

-- The same run along both edges of the car as well as its centre line.
-- The heights every ray in this file stands at, from vehicleGeometry.rayProfile for the car
-- being parked: set at the start of each plan. The same figures go to the vehicle VM for its
-- look-ahead and are what obstacleDetector uses, so one vehicle is judged one way everywhere.
local planRays = {highLift = 0.6, step = 0.15}

local function clearLeg(a, b, halfWidth)
  local along = flat(b - a)
  if along:length() < 1.0 then return true end
  local side = UP:cross(along:normalized()) * (halfWidth * 0.8)
  local h = planRays.highLift
  return clearRun(a, b, h) and clearRun(a + side, b + side, h) and clearRun(a - side, b - side, h)
end

-- A way from the car to a lead-in point: straight if it is open, otherwise through one staging
-- point on rings round the station (a forecourt is islands with lanes between them, and the
-- straight line from where the car stopped usually crosses one). Shortest open route wins.
-- How sharply a path turns from one heading onto the next, in degrees.
local function turnDeg(a, b)
  a, b = flat(a), flat(b)
  if a:length() < 0.3 or b:length() < 0.3 then return 0 end
  return math.deg(math.acos(math.max(-1, math.min(1, a:normalized():dot(b:normalized())))))
end

-- A car cannot follow a hairpin: one run went round a staging point that turned it back about
-- 150 degrees onto the approach, cut the corner by 3 m and hit the forecourt at walking pace.
-- Routes with a corner sharper than this are not offered; gentler ones are preferred.
local MAX_TURN_DEG = 100

local function routeTo(start, goal, halfWidth, st, nextDir)
  local function cost(points)
    local len, worst, prev, heading = 0, 0, start, nil
    for _, p in ipairs(points) do
      local leg = p - prev
      if heading then worst = math.max(worst, turnDeg(heading, leg)) end
      len, heading, prev = len + flat(leg):length(), leg, p
    end
    worst = math.max(worst, turnDeg(heading, nextDir))
    if worst > MAX_TURN_DEG then return nil end
    return len + worst / 6
  end
  local best, bestCost
  if clearLeg(start, goal, halfWidth) then
    best, bestCost = {goal}, cost({goal})
    if not bestCost then best = nil end
  end
  for _, ring in ipairs({8, 14, 20}) do
    for k = 0, 15 do
      local angle = k * math.pi / 8
      local mid = st.center + vec3(math.cos(angle), math.sin(angle), 0) * (st.radius + ring)
      mid.z = groundZ(mid)
      local c = cost({mid, goal})
      if c and (not bestCost or c < bestCost) and clearLeg(start, mid, halfWidth)
          and clearLeg(mid, goal, halfWidth) then
        best, bestCost = {mid, goal}, c
      end
    end
  end
  return best
end

local function otherVehicleNear(pos, radius, playerId)
  for i = 0, be:getObjectCount() - 1 do
    local v = be:getObject(i)
    if v and v:getID() ~= playerId then
      local c = vec3(be:getObjectOOBBCenterXYZ(v:getID()))
      if (flat(c) - flat(pos)):length() <= radius then return true end
    end
  end
  return false
end

-- Where a car can stand at an official pump is MEASURED, not inferred from the area's shape.
-- The first guess (car along the area's long axis, beside it) was wrong at Italy's Apex Airport:
-- there the pump is a thin slab lying along the long axis, the strip runs out from it into both
-- lanes, and a car alongside the pump lies ACROSS the strip. So: a height grid round the pump,
-- a car-sized footprint slid out from the pump in both orientations and on both sides until it
-- clears everything taller than a kerb, and only the placements whose box overlaps the stock
-- area are kept -- the stock prompt opens on exactly that overlap.
local GRID_M = 0.25
local GRID_REACH_M = 5.0
local FIT_M = 0.2         -- cells further above the plane than this are left out of its fit
local FOOT_MARGIN_M = 0.15

local function heightGrid(pump)
  if pump.grid then return pump.grid end
  local n = math.floor(GRID_REACH_M / GRID_M)
  local cells, samples = {}, {}
  -- Rays start 3 m up: above the pumps (2.8 m at Italy) but under any canopy. Started higher,
  -- a low canopy is read as the ground over the whole grid and the pumps beneath it vanish.
  -- A ray that starts inside something (a hit at once) is that something: solid.
  local top = pump.pos.z + 3
  for i = -n, n do
    cells[i] = {}
    for j = -n, n do
      local q = pump.pos + pump.long * (i * GRID_M) + pump.lat * (j * GRID_M)
      local hit = castRayStatic(vec3(q.x, q.y, top), vec3(0, 0, -1), 10)
      local h = (hit and hit < 10) and (top - hit) or nil
      if hit and hit < 0.05 then h = top end
      cells[i][j] = h
      if h then samples[#samples + 1] = {i, j, h} end
    end
  end
  -- The forecourt's own plane, fitted to the cells that are not standing up out of it: a
  -- median alone calls a gentle slope a wall at one edge of the grid.
  local a, b, c = 0, 0, pump.pos.z
  if #samples > 0 then
    local hs = {}
    for k, s in ipairs(samples) do hs[k] = s[3] end
    table.sort(hs)
    c = hs[math.max(1, math.floor(#hs / 2))]
    for _ = 1, 2 do
      local sx, sy, sz, sxx, syy, sxy, sxz, syz, m = 0, 0, 0, 0, 0, 0, 0, 0, 0
      for _, s in ipairs(samples) do
        local x, y, z = s[1] * GRID_M, s[2] * GRID_M, s[3]
        if z - (a * x + b * y + c) < FIT_M then
          sx, sy, sz = sx + x, sy + y, sz + z
          sxx, syy, sxy = sxx + x * x, syy + y * y, sxy + x * y
          sxz, syz, m = sxz + x * z, syz + y * z, m + 1
        end
      end
      if m >= 10 then
        -- Least squares for z = a x + b y + c, grid centred so the cross terms are small.
        local mx, my, mz = sx / m, sy / m, sz / m
        local vxx, vyy, vxy = sxx / m - mx * mx, syy / m - my * my, sxy / m - mx * my
        local vxz, vyz = sxz / m - mx * mz, syz / m - my * mz
        local det = vxx * vyy - vxy * vxy
        if math.abs(det) > 1e-9 then
          a = (vxz * vyy - vyz * vxy) / det
          b = (vyz * vxx - vxz * vxy) / det
          c = mz - a * mx - b * my
        end
      end
    end
  end
  pump.grid = {cells = cells, n = n, a = a, b = b, c = c}
  return pump.grid
end

local function solidAt(grid, i, j)
  if i < -grid.n or i > grid.n or j < -grid.n or j > grid.n then return false end
  local h = grid.cells[i][j]
  if not h then return false end
  -- Solid is whatever this vehicle cannot roll over or overhang.
  return h - (grid.a * i * GRID_M + grid.b * j * GRID_M + grid.c) > planRays.step
end

-- Whether a car footprint (half sizes along two flat unit axes) at `centre` is clear.
local function footprintClear(pump, grid, centre, axisA, halfA, axisB, halfB)
  local rel = flat(centre - pump.pos)
  local ca, cb = rel:dot(pump.long), rel:dot(pump.lat)
  local ea = halfA + FOOT_MARGIN_M
  local eb = halfB + FOOT_MARGIN_M
  for u = -ea, ea + 1e-6, GRID_M do
    for v = -eb, eb + 1e-6, GRID_M do
      local w = axisA * u + axisB * v
      local i = math.floor((ca + w:dot(pump.long)) / GRID_M + 0.5)
      local j = math.floor((cb + w:dot(pump.lat)) / GRID_M + 0.5)
      if solidAt(grid, i, j) then return false end
    end
  end
  return true
end

-- Every placement beside one pump that is clear and that the stock prompt would accept.
local function placements(pump, halfLength, halfWidth, halfHeight)
  local grid = heightGrid(pump)
  local found = {}
  for _, axes in ipairs({{pump.long, pump.lat}, {pump.lat, pump.long}}) do
    local along, across = axes[1], axes[2]
    for _, side in ipairs({1, -1}) do
      for d = 0, 4.0, 0.1 do
        local centre = pump.pos + across * (side * d)
        if footprintClear(pump, grid, centre, along, halfLength, across, halfWidth) then
          local rel = flat(centre - pump.pos)
          local ground = grid.a * rel:dot(pump.long) + grid.b * rel:dot(pump.lat) + grid.c
          local stop = vec3(centre.x, centre.y, ground)
          local box = vec3(centre.x, centre.y, ground + halfHeight)
          local reaches = overlapsOBB_OBB(box, along * halfLength, across * halfWidth,
            vec3(0, 0, halfHeight), pump.pos, pump.xVec, pump.yVec, pump.zVec)
          if reaches then
            found[#found + 1] = {stop = stop, along = along, across = across * side, gap = d}
          end
          break
        end
      end
    end
  end
  return found
end

local function vehicleRays(vid)
  local vg = extensions.vehicleGeometry
  if vg and vg.request then vg.request(vid) end
  local front = vg and vg.rayProfile and vg.rayProfile(vid, 1) or {lowLift = 0.35, highLift = 0.8, step = 0.15}
  local rear = vg and vg.rayProfile and vg.rayProfile(vid, -1) or front
  return front, rear
end

local function planPark(st, veh)
  local vid = veh:getID()
  local front, rear = vehicleRays(vid)
  -- The footprint stands on everything under the car, so the lower of the two ends decides.
  planRays = {highLift = math.min(front.highLift, rear.highLift), step = math.min(front.step, rear.step),
    front = front, rear = rear}
  local origin, fwd = playerFrame(veh)
  local _, h0, h1, h2 = oobb(vid)
  local left = UP:cross(fwd):normalized()
  local halfWidth = extentAlong(h0, h1, h2, left)
  local halfLength = extentAlong(h0, h1, h2, fwd)
  local halfHeight = extentAlong(h0, h1, h2, UP)
  local centre = vec3(be:getObjectOOBBCenterXYZ(vid))
  local start = vec3(centre.x, centre.y, groundZ(centre))
  local candidates = {}

  for index, pump in ipairs(st.pumps) do
    local options = {}
    if pump.unofficial then
      local away = flat(centre - pump.pos)
      if away:length() < 0.5 then away = -fwd end
      away = away:normalized()
      local stop = pump.pos + away * (halfLength + 1.0)
      stop.z = groundZ(stop)
      options[#options + 1] = {stop = stop, dir = -away, side = 0}
    else
      for _, place in ipairs(placements(pump, halfLength, halfWidth, halfHeight)) do
        for _, sign in ipairs({1, -1}) do
          options[#options + 1] = {stop = place.stop, dir = place.along * sign, side = 1}
        end
      end
      if #options == 0 then
        candidates[#candidates + 1] = {reason = "noRoom", index = index}
      end
    end
    for _, o in ipairs(options) do
      o.pump, o.index = pump, index
      local toStop = flat(o.stop - centre)
      local distance = toStop:length()
      local leadIn = o.stop - o.dir * LEAD_IN_M
      leadIn.z = groundZ(leadIn)
      -- Already lined up a short way back: drive straight in rather than circling back.
      local behind = -flat(centre - o.stop):dot(o.dir)
      local lateral = math.abs(flat(centre - o.stop):dot(UP:cross(o.dir)))
      local direct = behind > 0.5 and behind <= LEAD_IN_M + 2 and lateral < 1.5
          and fwd:dot(o.dir) > 0.8
      o.reason = nil
      if otherVehicleNear(o.stop, math.max(3, halfLength), vid) then
        o.reason = "occupied"
      elseif direct then
        o.points = {o.stop}
      elseif not clearRun(leadIn, o.stop, planRays.highLift) then
        o.reason = "blocked"
      else
        -- A car turning onto the approach line needs room to straighten before the pump: a
        -- first run went round a staging point and arrived 3 m wide of a 10 m approach. Join
        -- the line further back when that stretch is open too.
        local pre = o.stop - o.dir * (LEAD_IN_M + PRE_LEAD_M)
        pre.z = groundZ(pre)
        local entry = clearLeg(pre, leadIn, halfWidth) and pre or leadIn
        o.points = routeTo(start, entry, halfWidth, st, o.dir)
        if o.points then
          if entry == pre then o.points[#o.points + 1] = leadIn end
          o.points[#o.points + 1] = o.stop
        else
          o.reason = "blocked"
        end
      end
      if o.points then
        local length = 0
        local prev = centre
        for _, p in ipairs(o.points) do length = length + flat(p - prev):length(); prev = p end
        -- A heading against the car's means turning round first.
        local first = flat(o.points[1] - centre)
        local turn = first:length() > 0.5 and first:normalized():dot(fwd) or 1
        o.score = length + (turn < 0 and 15 or 0) + (fwd:dot(o.dir) < 0 and 8 or 0) + distance * 0.1
      end
      candidates[#candidates + 1] = o
    end
  end

  local best, reasons = nil, {}
  for _, o in ipairs(candidates) do
    if o.reason then
      reasons[o.reason] = (reasons[o.reason] or 0) + 1
    elseif not best or o.score < best.score then
      best = o
    end
  end
  lastDiag.candidates, lastDiag.reasons = #candidates, reasons
  if not best then
    if (reasons.occupied or 0) > 0 and (reasons.blocked or 0) == 0 then
      return nil, "Every pump is taken"
    end
    if (reasons.noRoom or 0) == #candidates then
      return nil, "Could not find a place beside these pumps where the game would let this vehicle refuel"
    end
    return nil, "No clear way to a pump from here. Drive a little closer and try again"
  end
  local pumpLeft = best.side ~= 0 and flat(best.pump.pos - best.stop):dot(UP:cross(best.dir)) > 0
  best.pumpSide = best.side == 0 and "ahead" or (pumpLeft and "left" or "right")
  -- One more point past the stop gives the vehicle VM's steering a line to hold to the end.
  best.beyond = best.stop + best.dir * 5
  return best
end

-- ---------------------------------------------------------------------------------------------
-- Parking session
-- ---------------------------------------------------------------------------------------------

local tokenCounter = 0

local function pointLiteral(p)
  return string.format("{x=%.3f,y=%.3f,z=%.3f}", p.x, p.y, p.z)
end

-- Only a route that ends at this station is ours to clear; anything else the player set stays.
local function clearOwnRoute(st)
  local gm = core_groundMarkers
  if not (gm and gm.currentlyHasTarget and gm.currentlyHasTarget()) then return end
  local path = gm.routePlanner and gm.routePlanner.path
  local last = path and path[#path]
  if last and last.pos and (flat(vec3(last.pos)) - flat(st.center)):length() < 3 then
    if freeroam_bigMapMode and freeroam_bigMapMode.setNavFocus then
      pcall(freeroam_bigMapMode.setNavFocus, nil)
    else
      pcall(gm.setPath, nil)
    end
  end
end

local function endPark(kind, data)
  local session = park
  if not session then return end
  park = nil
  local veh = scenetree.findObjectById(session.vid)
  if veh then
    veh:queueLuaCommand(string.format(
      'if extensions.beamtelFuelPark then extensions.beamtelFuelPark.stop(%q) end', session.token))
  end
  data = data or {}
  data.station = session.station.name
  lastDiag.lastEnd = kind .. (data.message and (": " .. data.message) or "")
  fsLog('I', "parking ended: " .. lastDiag.lastEnd)
  event(kind, data)
end

local function stationById(id) return id and byId[id] end

local function beginPark(st, veh, tanks)
  if park then return end
  if compatible(st, tanks) == false then
    event("park_refused", {message = st.name .. " has no fuel for your vehicle"})
    return
  end
  local plan, reason = planPark(st, veh)
  if not plan then
    event("park_refused", {message = reason})
    return
  end
  -- The route to the station has done its job. Left in place, the game says "arrived at
  -- destination" halfway into the parking manoeuvre, and a player rightly brakes for it.
  clearOwnRoute(st)
  -- The driving assistant holds the steering source; it must let go before this takes it.
  local okA, assistant = pcall(require, 'ge/extensions/drivingAssistant')
  if okA and assistant and assistant.owns and assistant.owns() then
    pcall(assistant.off, 'Parking at the pump. Steering assistance off')
  end
  tokenCounter = tokenCounter + 1
  local token = string.format("fp%d_%d", os.time(), tokenCounter)
  local pts = {}
  for _, p in ipairs(plan.points) do pts[#pts + 1] = pointLiteral(p) end
  local function raysLiteral(r)
    return string.format("{lowLift=%.3f,highLift=%.3f,step=%.3f}", r.lowLift, r.highLift, r.step)
  end
  local chunk = string.format(
    'extensions.load("beamtelFuelPark") extensions.beamtelFuelPark.start(%q, {%s}, %s, {front=%s,rear=%s})',
    token, table.concat(pts, ","), pointLiteral(plan.beyond), raysLiteral(planRays.front),
    raysLiteral(planRays.rear))
  veh:queueLuaCommand(chunk)
  park = {token = token, vid = veh:getID(), station = st, plan = plan, age = 0, alive = 0,
    heartbeat = 0}
  local start = vec3(veh:getPosition())
  local path = {}
  for _, p in ipairs(plan.points) do path[#path + 1] = {math.floor(p.x * 10) / 10, math.floor(p.y * 10) / 10} end
  lastDiag.plan = {pump = plan.index, side = plan.pumpSide, points = #plan.points,
    stop = {plan.stop.x, plan.stop.y, plan.stop.z}, dir = {plan.dir.x, plan.dir.y}, path = path,
    start = {start.x, start.y}, startFwd = {veh:getDirectionVector().x, veh:getDirectionVector().y}}
  event("parking", {pump = plan.index, pumpCount = #st.pumps, side = plan.pumpSide,
    official = st.official, distance = (flat(plan.stop) - flat(veh:getPosition())):length()})
end

local function requestPark(st, veh)
  if park then
    event("park_refused", {message = "Already parking"})
    return
  end
  requestTanks(veh, function(tanks)
    local v = getPlayerVehicle(0)
    if not v or v:getID() ~= veh:getID() then return end
    local ok, err = pcall(beginPark, st, v, tanks)
    if not ok then
      fsLog('E', "park failed to start: " .. tostring(err))
      event("park_refused", {message = "Parking could not start"})
    end
  end)
end

-- Called from the vehicle VM.
function M.onParkEvent(vid, token, kind, message)
  if not park or park.token ~= token or park.vid ~= vid then return end
  if kind == 'alive' then
    park.alive = 0
  elseif kind == 'stopped' then
    local session = park
    local plan = session.plan
    local veh = scenetree.findObjectById(vid)
    local overlap = veh and not plan.pump.unofficial and overlapsPump(vid, plan.pump)
    -- The stock prompt only opens on the transition into parking speed, which happened while
    -- this was still driving; ask it to look again now the car is still.
    if gameplay_markerInteraction and gameplay_markerInteraction.setForceReevaluateOpenPrompt then
      pcall(gameplay_markerInteraction.setForceReevaluateOpenPrompt)
    end
    endPark("parked", {pump = plan.index, side = plan.pumpSide, official = session.station.official,
      inZone = overlap == true,
      electric = (function()
        for _, e in ipairs(session.station.energy) do if e == "electricEnergy" then return true end end
        return false end)()})
  elseif kind == 'failed' then
    endPark("park_failed", {message = message})
  end
end

-- ---------------------------------------------------------------------------------------------
-- Commands
-- ---------------------------------------------------------------------------------------------

local function stationRow(st, veh, tanks)
  local distance, bearing = 0, 0
  if veh then distance, bearing = relative(veh, st.center) end
  return {id = st.id, name = st.name, official = st.official, energy = st.energy,
    distance = math.max(0, distance - st.radius), bearing = bearing, pumps = #st.pumps,
    compatible = compatible(st, tanks), target = st.id == target}
end

local function sendList()
  local veh = getPlayerVehicle(0)
  local function emit(tanks)
    local rows = {}
    for _, st in ipairs(stations) do rows[#rows + 1] = stationRow(st, veh, tanks) end
    table.sort(rows, function(a, b) return a.distance < b.distance end)
    send("FS_BEGIN:" .. #rows)
    for _, row in ipairs(rows) do send("FS:" .. jsonEncode(row)) end
    send("FS_END")
  end
  if veh then requestTanks(veh, emit) else emit(nil) end
end

local function distanceTo(st, veh)
  return math.max(0, (flat(vec3(veh:getPosition())) - flat(st.center)):length() - st.radius)
end

local function setTarget(id)
  local st = stationById(id)
  if not st then event("error", {message = "That station is no longer on this map"}); return end
  local veh = getPlayerVehicle(0)
  if not veh then event("error", {message = "No vehicle"}); return end
  if park then endPark("park_cancelled", {}) end
  target, inRange, watchAcc = st.id, false, 0
  local here = distanceTo(st, veh) <= IN_RANGE_M
  if not here then
    local destination = vec3(st.center.x, st.center.y, groundZ(st.center))
    local ok = freeroam_bigMapMode and freeroam_bigMapMode.setNavFocus
      and pcall(freeroam_bigMapMode.setNavFocus, destination)
    if not ok and core_groundMarkers then
      pcall(core_groundMarkers.setPath, destination, {clearPathOnReachingTarget = true})
    end
  end
  event("target", {id = st.id, station = st.name, official = st.official, here = here,
    distance = distanceTo(st, veh)})
  if here then
    inRange = true
    requestPark(st, veh)
  end
end

local function cancel()
  local st = stationById(target)
  if park then endPark("park_cancelled", {}) end
  if st then clearOwnRoute(st) end
  target, inRange = nil, false
  event("cancelled", {station = st and st.name or nil})
end

local function status()
  local st = stationById(target)
  local veh = getPlayerVehicle(0)
  event("status", {ready = ready, stations = #stations, target = st and st.name or nil,
    targetId = target, inRange = inRange, parking = park ~= nil,
    distance = (st and veh) and distanceTo(st, veh) or nil})
end

local function handle(line)
  local cmd, arg = line:match("^(%u+):?(.*)$")
  if not cmd then return end
  if cmd == "LIST" then
    if ready then sendList() else listWanted = true; send("FS_WAIT") end
  elseif cmd == "TARGET" then
    if ready then setTarget(arg) else send("FS_WAIT") end
  elseif cmd == "PARK" then
    local st = stationById(target)
    local veh = getPlayerVehicle(0)
    if not st then
      event("park_refused", {message = "No fuel station chosen"})
    elseif not veh then
      event("park_refused", {message = "No vehicle"})
    elseif distanceTo(st, veh) > PARK_REACH_M then
      event("park_refused", {message = "Too far from " .. st.name .. ". Get closer first"})
    else
      requestPark(st, veh)
    end
  elseif cmd == "PARK_CANCEL" then
    if park then endPark("park_cancelled", {}) end
  elseif cmd == "CANCEL" then
    cancel()
  elseif cmd == "STATUS" then
    status()
  end
end

-- ---------------------------------------------------------------------------------------------
-- Per frame
-- ---------------------------------------------------------------------------------------------

local function watch(dt)
  local st = stationById(target)
  if not st then return end
  watchAcc = watchAcc + dt
  if watchAcc < 0.5 then return end
  watchAcc = 0
  local veh = getPlayerVehicle(0)
  if not veh then return end
  local d = distanceTo(st, veh)
  if not inRange and d <= IN_RANGE_M then
    inRange = true
    event("in_range", {station = st.name, official = st.official})
  elseif inRange and d > LEAVE_RANGE_M and not park then
    inRange = false
    event("left_range", {station = st.name})
  end
end

local function stepPark(dt)
  park.age, park.alive, park.heartbeat = park.age + dt, park.alive + dt, park.heartbeat + dt
  local veh = scenetree.findObjectById(park.vid)
  local player = getPlayerVehicle(0)
  if not veh or not player or player:getID() ~= park.vid then
    endPark("park_failed", {message = "Vehicle changed"})
    return
  end
  if park.alive > 3 then
    endPark("park_failed", {message = "The vehicle stopped answering. Brake now"})
    return
  end
  if park.heartbeat >= 0.5 then
    park.heartbeat = 0
    veh:queueLuaCommand(string.format(
      'if extensions.beamtelFuelPark then extensions.beamtelFuelPark.heartbeat(%q) end', park.token))
  end
end

-- ---------------------------------------------------------------------------------------------
-- Sockets and lifecycle
-- ---------------------------------------------------------------------------------------------

local function setupSockets()
  if udpSend then pcall(function() udpSend:close() end); udpSend = nil end
  if udpCmd then pcall(function() udpCmd:close() end); udpCmd = nil end
  udpSend = socket.udp()
  if udpSend then
    udpSend:setpeername(PYTHON_HOST, DATA_PORT)
    udpSend:settimeout(0)
  end
  local ok, err = pcall(function()
    udpCmd = socket.udp()
    -- setsockname RETURNS nil plus a message; it does not throw. See uiToggle.lua.
    local bound, berr = udpCmd:setsockname("127.0.0.1", CMD_LISTEN_PORT)
    if not bound then error(tostring(berr), 0) end
    udpCmd:settimeout(0)
  end)
  if not (ok and udpCmd) then
    fsLog('E', "Failed to bind command socket on port " .. CMD_LISTEN_PORT .. ": " .. tostring(err))
    if udpCmd then pcall(function() udpCmd:close() end) end
    udpCmd = nil
  end
end

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
  if ok and udpCmd then fsLog('I', "Command socket bound on port " .. CMD_LISTEN_PORT .. " after retry.") end
end

function M.onExtensionUnloaded()
  if park then pcall(endPark, "park_failed", {message = "Fuel station assistance unloaded"}) end
  if udpSend then pcall(function() udpSend:close() end); udpSend = nil end
  if udpCmd then pcall(function() udpCmd:close() end); udpCmd = nil end
end

function M.onExtensionLoaded()
  setExtensionUnloadMode(M, "manual")
  setupSockets()
  -- Loaded into a live world (Ctrl+L, or the mod enabled mid-session): build now.
  if getCurrentLevelIdentifier and getCurrentLevelIdentifier() then pcall(startBuild) end
  fsLog('I', "Fuel stations loaded.")
end

function M.onWorldReadyState(state)
  if state == 2 then
    if park then pcall(endPark, "park_failed", {message = "Level changed"}) end
    setupSockets()
    pcall(startBuild)
  end
end

function M.onClientEndMission()
  if park then pcall(endPark, "park_failed", {message = "Level changed"}) end
  stations, byId, build, ready, target, inRange = {}, {}, nil, false, nil, false
end

function M.onVehicleSwitched()
  if park then pcall(endPark, "park_failed", {message = "Vehicle changed"}) end
end

function M.onVehicleResetted(vid)
  if park and park.vid == vid then pcall(endPark, "park_failed", {message = "Vehicle reset"}) end
end

function M.onUpdate(dtReal, dtSim, dtRaw)
  retryCmdBind(dtReal)
  if udpCmd then
    local data = udpCmd:receive()
    if data then
      local line = data:match("^%s*(.-)%s*$")
      local ok, err = pcall(handle, line)
      if not ok then
        fsLog('E', "command failed: " .. tostring(err))
        event("error", {message = "Fuel station command failed"})
      end
    end
  end
  -- The GE onUpdate chain has no pcall: a throw here would stop every later extension.
  local ok, err = pcall(function()
    if build then
      stepBuild()
      if ready and listWanted then listWanted = false; sendList() end
    end
    stepTanks(dtReal or 0)
    watch(dtReal or 0)
    if park then stepPark(dtReal or 0) end
  end)
  if not ok then
    fsLog('E', "update failed: " .. tostring(err))
    build = nil
    ready = true
    if park then pcall(endPark, "park_failed", {message = "Parking failed"}) end
  end
end

function M.diag()
  local st = stationById(target)
  fsLog('I', string.format("ready %s | %d stations (%s official, %s unofficial, %s shape hits) | target %s"
    .. " | inRange %s | parking %s | last plan %s | candidates %s %s",
    tostring(ready), #stations, tostring(lastDiag.official), tostring(lastDiag.unofficial),
    tostring(lastDiag.shapeHits), st and st.name or "none", tostring(inRange), tostring(park ~= nil),
    lastDiag.plan and jsonEncode(lastDiag.plan) or "none", tostring(lastDiag.candidates),
    lastDiag.reasons and jsonEncode(lastDiag.reasons) or "") .. " | last end " .. tostring(lastDiag.lastEnd))
  return lastDiag
end

M.stations = function() return stations end
M.planPark = planPark
M.placements = placements

return M
