-- =================================================================================================
--
--  Vehicle Scanner for BeamNG.drive Accessibility Mod (GE Extension)
--
--  Description: Scans for the nearest vehicle and sends bearing/distance data to the Python
--               backend via UDP as a plain "bearing,distance" text string.
--               Receives ON/OFF commands via UDP (no file I/O).
--
--  Loaded by: scripts/bng_screenreader_mod/modScript.lua
--
--  Version:     7.0 (GE extension with modScript loader, UDP commands)
--  Target Game: BeamNG.drive 0.37+
--
-- =================================================================================================

local M = {}

-- Configuration
local PYTHON_HOST         = "127.0.0.1"
local PYTHON_PORT_SCANNER = 4445   -- send scan data to Python on this port
local CMD_LISTEN_PORT     = 4448   -- receive ON/OFF commands from Python on this port
local SCAN_INTERVAL       = 0.1

-- Internal State
local udpSend           = nil
local udpCmd            = nil      -- command listener socket
local isScanModeActive  = false
local currentTargetID   = nil
local currentTargetDist = math.huge
local scanTimer         = 0
local lastPlayerID      = nil      -- for vehicle-switch detection

-- Alignment State
local alignPending      = false
local alignTimeout      = 0
local ALIGN_TIMEOUT_SEC = 3.0

-- Delayed fifth-wheel auto-coupling (wait for physics to settle after teleport)
local _fwCouplePending  = false
local _fwCoupleTimer    = 0
local _fwCoupleRetries  = 0
local FW_COUPLE_DELAY   = 0.5    -- seconds to wait before first attempt
local FW_COUPLE_MAX_RETRIES = 6  -- retry coupling several times
local FW_COUPLE_RETRY_INTERVAL = 0.3
local _fwCouplePlayerTag = ""
local _fwCoupleTargetTag = ""

-- Coupler Tracking State
local couplerTrackActive  = false
local _playerCouplerCid   = nil
local _targetCouplerCid   = nil
local _couplerTargetID    = nil
local _pendingPlayerInfo  = nil   -- nil=waiting, {cid=N,tag=S,nx,ny,nz} or false (no coupler)
local _pendingTargetInfo  = nil   -- nil=waiting, {cid,tag,cx,cy,cz,ox,oy,oz} or false (o=target center)
local COUPLER_RANGE_M     = 1.5
local COUPLER_TRACK_HZ    = 20
local couplerTrackTimer   = 0

-- Coupler Distance Mode State (speech callouts every 2.5s, survives vehicle switches)
local couplerDistMode       = false
local couplerDistReady      = false
local couplerDistTimer      = 0
local COUPLER_DIST_INTERVAL = 2.5
local _cdPlayerCid          = nil
local _cdTargetCid          = nil
local _cdTargetID           = nil
local _cdPlayerID           = nil
local _cdDiscovering        = false
local _cdPendingPlayer      = nil   -- nil=not yet, false=no coupler, number=cid
local _cdPendingTarget      = nil
local _cdPlayerOverhang     = 0     -- how far truck body extends behind its coupler (meters)
local _cdTargetOverhang     = 0     -- how far trailer body extends past its coupler toward truck (meters)

-- Coupler tag compatibility table
local COUPLER_COMPAT = {
  tow_hitch  = "tow_bar",
  tow_bar    = "tow_hitch",
  fifthwheel = "fifthwheel",
}

-- Logging helper
local function scannerLog(level, msg)
  log(level, 'VehicleScanner', msg)
end

local function areCouplerTagsCompatible(tag1, tag2)
  if tag1 == "" or tag2 == "" then return false end
  local t1, t2 = tag1:lower(), tag2:lower()
  if t1 == t2 then return true end
  -- Exact match in compat table
  if COUPLER_COMPAT[t1] == t2 then return true end
  -- Pattern-based matching for tags with extra characters
  -- e.g. "tow_hitch_heavy" should still match with "tow_bar"
  local function classify(tag)
    if tag:find("fifthwheel") or tag:find("fifth_wheel") then return "fifthwheel" end
    if tag:find("tow_hitch") then return "tow_hitch" end
    if tag:find("tow_bar") then return "tow_bar" end
    return tag
  end
  local c1, c2 = classify(t1), classify(t2)
  if c1 == c2 then return true end
  return COUPLER_COMPAT[c1] == c2
end

local function _tryCouplerDistReady()
  if _cdPendingPlayer == nil or _cdPendingTarget == nil then return end
  _cdDiscovering = false
  if _cdPendingPlayer == false then
    if udpSend then udpSend:send("COUPLER_DIST_FAIL:No coupler on your vehicle") end
    return
  end
  if _cdPendingTarget == false then
    if udpSend then udpSend:send("COUPLER_DIST_FAIL:No coupler on target") end
    return
  end
  couplerDistReady = true
  couplerDistTimer = COUPLER_DIST_INTERVAL  -- fire immediately on next tick
  scannerLog('info', string.format(
    "Coupler distance mode ready. Player cid=%d rearOverhang=%.2fm, Target cid=%d frontOverhang=%.2fm",
    _cdPlayerCid, _cdPlayerOverhang, _cdTargetCid, _cdTargetOverhang))
end

local function _tryCompleteCouplerSetup()
  -- Wait until both callbacks have arrived
  if _pendingPlayerInfo == nil or _pendingTargetInfo == nil then return end
  alignPending = false
  alignTimeout = 0

  -- Check if both have couplers
  if _pendingPlayerInfo == false then
    if udpSend then udpSend:send("COUPLER_FAIL:No coupler found on your vehicle") end
    return
  end
  if _pendingTargetInfo == false then
    if udpSend then udpSend:send("COUPLER_FAIL:No coupler found on target vehicle") end
    return
  end

  -- Check compatibility
  local pTag = _pendingPlayerInfo.tag
  local tTag = _pendingTargetInfo.tag
  if not areCouplerTagsCompatible(pTag, tTag) then
    if udpSend then
      udpSend:send("COUPLER_FAIL:Incompatible couplers. Your vehicle: " .. pTag .. ", Target: " .. tTag)
    end
    return
  end

  -- Compatible! Do alignment teleport
  local player = be:getPlayerVehicle(0)
  if not player then
    if udpSend then udpSend:send("COUPLER_FAIL:No player vehicle") end
    return
  end

  local ti = _pendingTargetInfo
  local pi = _pendingPlayerInfo
  local couplerPos = vec3(ti.cx, ti.cy, ti.cz)
  local targetCenter = vec3(ti.ox, ti.oy, ti.oz)

  -- Classify coupling type
  local function classifyCoupler(tag)
    local t = tag:lower()
    if t:find("fifthwheel") or t:find("fifth_wheel") then return "fifthwheel" end
    if t:find("tow_hitch") then return "tow_hitch" end
    if t:find("tow_bar") then return "tow_bar" end
    return t
  end
  local isFifthWheel = classifyCoupler(pTag) == "fifthwheel" and classifyCoupler(tTag) == "fifthwheel"

  -- Compute "away from trailer" direction for alignment.
  local awayDir
  local target = scenetree.findObjectById(currentTargetID)

  if isFifthWheel and target then
    -- Fifth wheel: use the trailer's actual forward direction for a clean, centered axis.
    -- The reference-to-coupler vector can be skewed if the reference node is off-center.
    local tFwd = target:getDirectionVector()
    awayDir = vec3(tFwd.x, tFwd.y, 0)
    -- Ensure it points from trailer body toward king pin (same side as coupler)
    local refToCoupler = couplerPos - targetCenter
    refToCoupler.z = 0
    if awayDir:dot(refToCoupler) < 0 then
      awayDir = -awayDir
    end
  else
    -- Ball hitch / fallback: use reference-to-coupler direction
    awayDir = (couplerPos - targetCenter)
    awayDir.z = 0
    local awayLen = awayDir:length()
    if awayLen < 0.5 and target then
      awayDir = vec3(target:getDirectionVector())
      awayDir.z = 0
    end
  end
  awayDir = awayDir:normalized()

  local alignPos, alignDist

  if isFifthWheel then
    -- Fifth wheel: position truck so its coupler lands on the trailer's king pin.
    -- pi.ny is the forward-projected distance from reference node to coupler.
    -- Use abs() because the truck center must always be on the AWAY side of the king pin —
    -- the fifth wheel (behind the truck center) reaches back to meet the king pin.
    -- truckCenter = kingPin + |offset| * awayDir, fifthWheel = truckCenter - |offset| * awayDir = kingPin
    alignDist = math.abs(pi.ny or 0)
    alignPos = couplerPos + awayDir * alignDist
  else
    -- Ball hitch: leave a 1.5m gap so the player reverses to couple up.
    local couplerForwardOffset = math.abs(pi.ny or 0)
    local COUPLER_GAP_M = 1.5
    alignDist = math.max(couplerForwardOffset + COUPLER_GAP_M, 5.0)
    alignPos = couplerPos + awayDir * alignDist
  end

  -- Use the player's current Z for ground level — the player truck is already on the ground.
  -- Don't use the coupler Z, which may be elevated (e.g. trailer deck height).
  alignPos.z = player:getPosition().z + 0.3

  scannerLog('info', string.format(
    "Align: couplerPos=(%.1f,%.1f,%.1f) targetCenter=(%.1f,%.1f,%.1f) alignDist=%.2f finalPos=(%.1f,%.1f,%.1f) fifthWheel=%s localNy=%.2f",
    couplerPos.x, couplerPos.y, couplerPos.z,
    targetCenter.x, targetCenter.y, targetCenter.z,
    alignDist,
    alignPos.x, alignPos.y, alignPos.z,
    tostring(isFifthWheel), (pi.ny or 0)))

  local rot = quatFromDir(awayDir, vec3(0, 0, 1))
  spawn.safeTeleport(player, alignPos, rot, nil, true)

  if isFifthWheel then
    -- Delay auto-coupling to let physics settle after teleport.
    -- The onUpdate loop will retry coupling until it succeeds or times out.
    _fwCouplePending = true
    _fwCoupleTimer = 0
    _fwCoupleRetries = 0
    _fwCouplePlayerTag = pTag
    _fwCoupleTargetTag = tTag
    scannerLog('info', "Fifth wheel teleport done, scheduling delayed auto-coupling. Player: " .. pTag .. " Target: " .. tTag)
  else
    -- Start coupler tracking for manual reverse-to-couple
    _playerCouplerCid = _pendingPlayerInfo.cid
    _targetCouplerCid = _pendingTargetInfo.cid
    _couplerTargetID = currentTargetID
    couplerTrackActive = true
    couplerTrackTimer = 0

    if udpSend then
      udpSend:send("COUPLER_START:" .. pTag .. "," .. tTag)
    end
    scannerLog('info', "Coupler tracking started. Player: " .. pTag .. " Target: " .. tTag)
  end
end

-- =================================================================================================
--  Target Cycling
-- =================================================================================================

local function cycleTarget(direction)  -- direction: 1 = next, -1 = prev
  if not udpSend then return end
  local player = be:getPlayerVehicle(0)
  if not player then return end

  local playerPos = player:getPosition()
  local playerID  = player:getID()

  -- Build distance-sorted list of non-player vehicles
  local vehicles = {}
  for i = 0, be:getObjectCount() - 1 do
    local obj = be:getObject(i)
    if obj and obj:getID() ~= playerID then
      table.insert(vehicles, { id = obj:getID(), dist = playerPos:distance(obj:getPosition()) })
    end
  end
  if #vehicles == 0 then
    udpSend:send("TARGET_NAME:No other vehicles")
    return
  end
  table.sort(vehicles, function(a, b) return a.dist < b.dist end)

  -- Find current target's index (0 = not in list)
  local currentIdx = 0
  for i, v in ipairs(vehicles) do
    if v.id == currentTargetID then currentIdx = i; break end
  end

  -- Advance/retreat with wrap; if no current target, start at first or last
  local newIdx
  if currentIdx == 0 then
    newIdx = direction == 1 and 1 or #vehicles
  else
    newIdx = ((currentIdx - 1 + direction + #vehicles) % #vehicles) + 1
  end

  local entry = vehicles[newIdx]
  local newVeh = scenetree.findObjectById(entry.id)
  if not newVeh then return end

  currentTargetID   = entry.id
  currentTargetDist = entry.dist
  scannerLog('info', "Target cycled to vehicle ID " .. entry.id)

  local fallback = newVeh:getJBeamFilename() or "unknown"
  newVeh:queueLuaCommand(string.format([[
    local info = (v.data and v.data.information) or {}
    local brand = tostring(info.brand or "")
    local model = tostring(info.name or %q)
    local display = brand ~= "" and (brand .. " " .. model) or model
    obj:queueGameEngineLua(string.format(
      "extensions.vehicleScanner.onTargetNameReady(%%q)", display
    ))
  ]], fallback))
end

-- =================================================================================================
--  Core Scan Logic
-- =================================================================================================

local function scanAndSendVehicleData()
  if not udpSend then return end

  local player = be:getPlayerVehicle(0)
  if not player then return end

  local playerPos        = player:getPosition()
  local playerForwardVec = player:getDirectionVector()
  local playerUpVec      = player:getDirectionVectorUp()
  local playerID         = player:getID()

  -- Auto-select the nearest non-player object only when no target is locked.
  -- Once a target is locked (by auto-select or manual cycling), never override it.
  if not currentTargetID then
    local closestVehID   = nil
    local closestVehDist = math.huge
    for i = 0, be:getObjectCount() - 1 do
      local obj = be:getObject(i)
      if obj and obj:getID() ~= playerID then
        local dist = playerPos:distance(obj:getPosition())
        if dist < closestVehDist then
          closestVehDist = dist
          closestVehID   = obj:getID()
        end
      end
    end
    if not closestVehID then return end
    currentTargetID   = closestVehID
    currentTargetDist = closestVehDist
    -- Look up name for the auto-selected target
    local autoVeh = scenetree.findObjectById(closestVehID)
    if autoVeh then
      local fallback = autoVeh:getJBeamFilename() or "unknown"
      autoVeh:queueLuaCommand(string.format([[
        local info = (v.data and v.data.information) or {}
        local brand = tostring(info.brand or "")
        local model = tostring(info.name or %q)
        local display = brand ~= "" and (brand .. " " .. model) or model
        obj:queueGameEngineLua(string.format(
          "extensions.vehicleScanner.onTargetNameReady(%%q)", display
        ))
      ]], fallback))
    end
  end

  -- Look up the locked target; clear lock if it has disappeared
  local targetVehicle = scenetree.findObjectById(currentTargetID)
  if not targetVehicle then
    currentTargetID = nil
    return
  end

  -- Calculate bearing (positive = right of forward, negative = left) and live distance
  local targetPos      = targetVehicle:getPosition()
  currentTargetDist    = playerPos:distance(targetPos)
  local toTargetVec    = (targetPos - playerPos):normalized()
  local cosAngle       = playerForwardVec:dot(toTargetVec)
  local angleRadians   = math.acos(math.max(-1, math.min(1, cosAngle)))
  local playerRightVec = playerUpVec:cross(playerForwardVec)
  local dot            = playerRightVec:dot(toTargetVec)
  local bearingDegrees = math.deg(angleRadians) * (dot < 0 and -1 or 1)

  -- Approach angle from target's frame: which face of the target is the player nearest to?
  -- 0 = player is in front of target, ±180 = player is behind target,
  -- positive = player is to target's right, negative = player is to target's left
  local targetFwdVec   = targetVehicle:getDirectionVector()
  local targetUpVec    = targetVehicle:getDirectionVectorUp()
  local toPlayerVec    = (playerPos - targetPos):normalized()
  local cosApproach    = targetFwdVec:dot(toPlayerVec)
  local approachRad    = math.acos(math.max(-1, math.min(1, cosApproach)))
  local targetRightVec = targetUpVec:cross(targetFwdVec)
  local approachDeg    = math.deg(approachRad) * (targetRightVec:dot(toPlayerVec) < 0 and -1 or 1)

  -- Send as plain text "bearing,distance,approachDeg" — parsed by Python with split(',')
  local packet = string.format("%.4f,%.4f,%.4f", bearingDegrees, currentTargetDist, approachDeg)
  udpSend:send(packet)
end

-- =================================================================================================
--  GE Extension Hooks (exported via M table)
-- =================================================================================================

function M.onExtensionLoaded()
  scannerLog('info', "Vehicle scanner extension loaded.")
end

function M.onWorldReadyState(state)
  scannerLog('info', "onWorldReadyState triggered with state: " .. tostring(state))

  if state == 2 then
    scannerLog('info', "World is ready. Initializing scanner systems.")

    -- Close existing sockets before re-creating (handles map reload)
    if udpSend then pcall(function() udpSend:close() end); udpSend = nil end
    if udpCmd  then pcall(function() udpCmd:close()  end); udpCmd  = nil end

    -- Reset state for new map
    isScanModeActive  = false
    currentTargetID   = nil
    currentTargetDist = math.huge
    lastPlayerID      = nil
    alignPending      = false
    _fwCouplePending  = false
    couplerTrackActive = false
    _playerCouplerCid  = nil
    _targetCouplerCid  = nil
    _couplerTargetID   = nil
    _pendingPlayerInfo = nil
    _pendingTargetInfo = nil
    couplerDistMode    = false
    couplerDistReady   = false
    _cdDiscovering     = false
    _cdPlayerCid       = nil
    _cdTargetCid       = nil
    _cdTargetID        = nil
    _cdPlayerID        = nil
    _cdPendingPlayer   = nil
    _cdPendingTarget   = nil
    _cdPlayerOverhang  = 0
    _cdTargetOverhang  = 0

    -- Create send socket for scan data
    udpSend = socket.udp()
    if udpSend then
      udpSend:setpeername(PYTHON_HOST, PYTHON_PORT_SCANNER)
      udpSend:settimeout(0)
      scannerLog('info', "UDP send socket created, targeting " .. PYTHON_HOST .. ":" .. PYTHON_PORT_SCANNER)
    else
      scannerLog('error', "Failed to create UDP send socket.")
    end

    -- Create receive socket for ON/OFF commands from Python
    local ok, err = pcall(function()
      udpCmd = socket.udp()
      udpCmd:setsockname("127.0.0.1", CMD_LISTEN_PORT)
      udpCmd:settimeout(0)
    end)
    if ok and udpCmd then
      scannerLog('info', "UDP command socket listening on port " .. CMD_LISTEN_PORT)
    else
      scannerLog('error', "Failed to create UDP command socket: " .. tostring(err))
      udpCmd = nil
    end
  end
end

function M.onUpdate(dtReal, dtSim, dtRaw)
  -- 1. Poll UDP for ON/OFF commands from Python
  if udpCmd then
    local data = udpCmd:receive()
    if data then
      local cmd = data:match("^%s*(.-)%s*$"):upper()
      if cmd == "ON" and not isScanModeActive then
        isScanModeActive  = true
        currentTargetID   = nil
        currentTargetDist = math.huge
        scannerLog('info', "Scan mode activated via UDP.")
      elseif cmd == "OFF" and isScanModeActive then
        isScanModeActive = false
        couplerTrackActive = false
        scannerLog('info', "Scan mode deactivated via UDP.")
      elseif cmd == "NEXT" then
        cycleTarget(1)
      elseif cmd == "PREV" then
        cycleTarget(-1)
      elseif cmd == "DAMAGE" then
        scannerLog('info', "DAMAGE command received, routing to active vehicle.")
        local player = be:getPlayerVehicle(0)
        if not player then
          -- No vehicle — send DONE immediately so Python doesn't hang
          local tmpSock = socket.udp()
          if tmpSock then
            tmpSock:setpeername("127.0.0.1", 4447)
            tmpSock:send("NONE")
            tmpSock:send("DONE")
            tmpSock:close()
          end
        else
          local fallback = player:getJBeamFilename() or "unknown"
          player:queueLuaCommand(string.format([[
            local _ds = require("socket").udp()
            _ds:setpeername("127.0.0.1", 4447)
            -- Send vehicle name first
            local info = (v.data and v.data.information) or {}
            local brand = tostring(info.brand or "")
            local model = tostring(info.name or %q)
            local vehName = brand ~= "" and (brand .. " " .. model) or model
            _ds:send("NAME:" .. vehName)
            local found = false
            local ok, err = pcall(function()
              local bodyParts = {"FL", "FR", "ML", "MR", "RL", "RR"}
              local bodyLabels = {FL="Front left", FR="Front right", ML="Middle left", MR="Middle right", RL="Rear left", RR="Rear right"}
              for _, part in ipairs(bodyParts) do
                local val = damageTracker.getDamage("body", part)
                if type(val) == "number" and val > 0.01 then
                  local severity = val > 0.5 and "heavy" or (val > 0.2 and "moderate" or "light")
                  _ds:send(bodyLabels[part] .. " body " .. severity .. " damage")
                  found = true
                end
              end
              local engineChecks = {
                {"starvedOfOil","Oil starvation"},{"coolantOverheating","Coolant overheating"},
                {"oilOverheating","Oil overheating"},{"pistonRingsDamaged","Piston rings damaged"},
                {"rodBearingsDamaged","Rod bearings damaged"},{"headGasketDamaged","Head gasket damaged"},
                {"turbochargerHot","Turbocharger overheating"},{"turbochargerDamaged","Turbocharger damaged"},
                {"superchargerDamaged","Supercharger damaged"},{"inductionSystemDamaged","Induction system damaged"},
                {"engineHydrolocked","Engine hydrolocked"},{"engineDisabled","Engine disabled"},
                {"blockMelted","Engine block melted"},{"cylinderWallsMelted","Cylinder walls melted"},
                {"engineLockedUp","Engine locked up"},{"radiatorLeak","Radiator leaking"},
                {"oilpanLeak","Oil pan leaking"},{"oilRadiatorLeak","Oil radiator leaking"},
                {"oilLevelCritical","Oil level critical"},{"engineReducedTorque","Engine torque reduced"},
              }
              for _, pair in ipairs(engineChecks) do
                local val = damageTracker.getDamage("engine", pair[1])
                if val and val ~= false and val ~= 0 then
                  _ds:send(pair[2]); found = true
                end
              end
              local corners = {"FL", "FR", "RL", "RR"}
              local cornerLabels = {FL="Front left", FR="Front right", RL="Rear left", RR="Rear right"}
              for _, c in ipairs(corners) do
                local label = cornerLabels[c]
                if damageTracker.getDamage("wheels", "tire" .. c) == true then
                  _ds:send(label .. " tire burst"); found = true
                end
                if damageTracker.getDamage("wheels", c) == true then
                  _ds:send(label .. " wheel broken"); found = true
                end
                if damageTracker.getDamage("wheels", "brake" .. c) == true then
                  _ds:send(label .. " brake damaged"); found = true
                end
                local brakeHeat = damageTracker.getDamage("wheels", "brakeOverHeat" .. c)
                if type(brakeHeat) == "number" and brakeHeat > 0 then
                  _ds:send(label .. " brake fading"); found = true
                end
              end
              local ptChecks = {
                {"wheelaxleFL","Front left axle broken"},{"wheelaxleFR","Front right axle broken"},
                {"wheelaxleRL","Rear left axle broken"},{"wheelaxleRR","Rear right axle broken"},
                {"driveshaft","Driveshaft broken"},{"driveshaft_F","Front driveshaft broken"},
                {"mainEngine","Engine broken"},
              }
              for _, pair in ipairs(ptChecks) do
                local val = damageTracker.getDamage("powertrain", pair[1])
                if val and val ~= false then
                  _ds:send(pair[2]); found = true
                end
              end
              local fuelVal = damageTracker.getDamage("energyStorage", "mainTank")
              if fuelVal and fuelVal ~= false then
                _ds:send("Fuel tank damaged"); found = true
              end
              if damageTracker.getDamage("gearbox", "synchroWear") == true then
                _ds:send("Synchro wear"); found = true
              end
              -- Detect detached parts via break groups
              local bgMap = {}
              if v.data.beams then
                for _, b in pairs(v.data.beams) do
                  if b.breakGroup then
                    local groups = type(b.breakGroup) == "table" and b.breakGroup or {b.breakGroup}
                    for _, g in ipairs(groups) do
                      if not bgMap[g] then
                        bgMap[g] = {cids = {}, partPath = b.partPath}
                      end
                      table.insert(bgMap[g].cids, b.cid)
                    end
                  end
                end
              end
              local detachedParts = {}
              for gName, gData in pairs(bgMap) do
                local allBroken = #gData.cids > 0
                for _, cid in ipairs(gData.cids) do
                  if not obj:beamIsBroken(cid) then
                    allBroken = false
                    break
                  end
                end
                if allBroken and gData.partPath then
                  detachedParts[gData.partPath] = true
                end
              end
              -- Friendly name from partPath: take last segment, strip vehicle prefix, map directions
              local dirSuffixes = {{"_FL"," front left"},{"_FR"," front right"},{"_RL"," rear left"},{"_RR"," rear right"},{"_F"," front"},{"_L"," left"},{"_R"," right"}}
              local function friendlyName(pp)
                local seg = pp:match("([^/]+)$") or pp
                -- Strip vehicle prefix (first word before underscore)
                local stripped = seg:match("^%w-_(.+)$")
                if stripped and #stripped > 0 then seg = stripped end
                -- Map direction suffix
                for _, d in ipairs(dirSuffixes) do
                  if seg:sub(-#d[1]) == d[1] then
                    seg = seg:sub(1, -#d[1]-1) .. d[2]
                    break
                  end
                end
                seg = seg:gsub("_", " ")
                return seg:sub(1,1):upper() .. seg:sub(2)
              end
              local apd = v.data.activePartsData
              for partPath, _ in pairs(detachedParts) do
                local name = nil
                if apd and apd[partPath] and apd[partPath].information and apd[partPath].information.name then
                  name = apd[partPath].information.name
                end
                if not name or name == "" then
                  name = friendlyName(partPath)
                end
                _ds:send(name .. " detached")
                found = true
              end
            end)
            if not ok then _ds:send("(error: " .. tostring(err) .. ")"); found = true end
            if not found then _ds:send("NONE") end
            _ds:send("DONE")
            _ds:close()
          ]], fallback))
        end
      elseif cmd == "ALIGN" then
        scannerLog('info', "ALIGN command received.")
        if not currentTargetID then
          udpSend:send("COUPLER_FAIL:No vehicle target locked")
        else
          local targetVeh = scenetree.findObjectById(currentTargetID)
          if not targetVeh then
            udpSend:send("COUPLER_FAIL:Target vehicle not found")
          else
            local player = be:getPlayerVehicle(0)
            if not player then
              udpSend:send("COUPLER_FAIL:No player vehicle")
            else
              alignPending = true
              alignTimeout = 0
              _pendingPlayerInfo = nil
              _pendingTargetInfo = nil
              local tid = currentTargetID
              -- Query player vehicle for coupler node
              player:queueLuaCommand([[
                local best, btag, bpri = nil, "", 0
                for _, nd in pairs(v.data.nodes) do
                  local p, t = 0, ""
                  if nd.couplerTag then
                    local cl = nd.couplerTag:lower()
                    p = 1; t = nd.couplerTag
                    if cl == "tow_bar" then p = 2 end
                    if cl:find("fifthwheel") or cl:find("tow_hitch") then p = 3 end
                  end
                  if p == 0 and nd.tag and type(nd.tag) == "string" then
                    local tl = nd.tag:lower()
                    if tl:find("fifthwheel") or tl:find("fifth_wheel") or tl:find("tow_hitch") then
                      p = 3; t = nd.couplerTag or nd.tag
                    elseif tl:find("tow_bar") then
                      p = 2; t = nd.couplerTag or nd.tag
                    end
                  end
                  if p == 0 and nd.couplerStrength and nd.couplerStrength > 0 then
                    p = 1; t = nd.couplerTag or nd.tag or "coupler"
                  end
                  if p > bpri then best = nd; btag = t; bpri = p end
                end
                if best then
                  -- Compute local forward offset by projecting node position onto vehicle forward.
                  -- getNodePosition returns world-relative offsets, so we project onto the vehicle's
                  -- actual forward direction to get the orientation-independent forward distance.
                  local np = vec3(obj:getNodePosition(best.cid))
                  local fwd = vec3(obj:getDirectionVector())
                  fwd.z = 0
                  if fwd:length() > 0.01 then fwd = fwd:normalized() end
                  local forwardOffset = np:dot(fwd)
                  obj:queueGameEngineLua(string.format(
                    'extensions.vehicleScanner.onPlayerCouplerInfo(%d, %q, %.4f, %.4f, %.4f)',
                    best.cid, btag, np.x, forwardOffset, np.z))
                else
                  obj:queueGameEngineLua('extensions.vehicleScanner.onPlayerCouplerInfo(-1, "", 0, 0, 0)')
                end
              ]])
              -- Query target vehicle for coupler node (with position/direction for alignment)
              targetVeh:queueLuaCommand(string.format([[
                local best, btag, bpri = nil, "", 0
                for _, nd in pairs(v.data.nodes) do
                  local p, t = 0, ""
                  if nd.couplerTag then
                    local cl = nd.couplerTag:lower()
                    p = 1; t = nd.couplerTag
                    if cl == "tow_bar" then p = 2 end
                    if cl:find("fifthwheel") or cl:find("tow_hitch") then p = 3 end
                  end
                  if p == 0 and nd.tag and type(nd.tag) == "string" then
                    local tl = nd.tag:lower()
                    if tl:find("fifthwheel") or tl:find("fifth_wheel") or tl:find("tow_hitch") then
                      p = 3; t = nd.couplerTag or nd.tag
                    elseif tl:find("tow_bar") then
                      p = 2; t = nd.couplerTag or nd.tag
                    end
                  end
                  if p == 0 and nd.couplerStrength and nd.couplerStrength > 0 then
                    p = 1; t = nd.couplerTag or nd.tag or "coupler"
                  end
                  if p > bpri then best = nd; btag = t; bpri = p end
                end
                if best then
                  local np = vec3(obj:getNodePosition(best.cid))
                  local op = vec3(obj:getPosition())
                  local wp = op + np
                  obj:queueGameEngineLua(string.format(
                    "extensions.vehicleScanner.onTargetCouplerForAlign(%d, %%d, %%.4f, %%.4f, %%.4f, %%.4f, %%.4f, %%.4f, '%%s')",
                    best.cid, wp.x, wp.y, wp.z, op.x, op.y, op.z, btag
                  ))
                else
                  obj:queueGameEngineLua("extensions.vehicleScanner.onTargetCouplerForAlign(%d, -1, 0, 0, 0, 0, 0, 0, '')")
                end
              ]], tid, tid))
            end
          end
        end
      elseif cmd == "COUPLER_DIST" then
        couplerDistMode = not couplerDistMode
        if not couplerDistMode then
          couplerDistReady = false
          _cdDiscovering = false
          _cdPendingPlayer = nil
          _cdPendingTarget = nil
        end
        scannerLog('info', "Coupler distance mode " .. (couplerDistMode and "ON" or "OFF"))
        if udpSend then
          udpSend:send("COUPLER_DIST_MODE:" .. (couplerDistMode and "ON" or "OFF"))
        end
      end
    end
  end

  -- 2. Alignment timeout guard
  if alignPending then
    alignTimeout = alignTimeout + dtReal
    if alignTimeout >= ALIGN_TIMEOUT_SEC then
      alignPending = false
      alignTimeout = 0
      if udpSend then
        udpSend:send("COUPLER_FAIL:Alignment timed out")
      end
      scannerLog('warn', "Alignment callback timed out.")
    end
  end

  -- 2b. Delayed fifth-wheel auto-coupling (retry until attached or timeout)
  if _fwCouplePending then
    _fwCoupleTimer = _fwCoupleTimer + dtReal
    local delay = _fwCoupleRetries == 0 and FW_COUPLE_DELAY or FW_COUPLE_RETRY_INTERVAL
    if _fwCoupleTimer >= delay then
      _fwCoupleTimer = 0
      _fwCoupleRetries = _fwCoupleRetries + 1
      local p = be:getPlayerVehicle(0)
      local t = currentTargetID and scenetree.findObjectById(currentTargetID) or nil
      if p and t then
        p:queueLuaCommand('beamstate.activateAutoCoupling()')
        t:queueLuaCommand('beamstate.activateAutoCoupling()')
        scannerLog('info', "Fifth wheel auto-coupling attempt " .. _fwCoupleRetries)
      end
      if _fwCoupleRetries >= FW_COUPLE_MAX_RETRIES then
        _fwCouplePending = false
        if p and t then
          -- Retract landing gear and release trailer brakes
          t:queueLuaCommand('electrics.values.feet = 1')
          t:queueLuaCommand('electrics.values.parkingbrake = 0')
          t:queueLuaCommand('electrics.values.brake = 0')
        end
        if udpSend then
          udpSend:send("COUPLER_ATTACHED:" .. _fwCouplePlayerTag .. "," .. _fwCoupleTargetTag)
        end
        scannerLog('info', "Fifth wheel coupling sequence complete after " .. _fwCoupleRetries .. " attempts.")
      end
    end
  end

  -- 2c. Detect player vehicle switch by polling
  local player = be:getPlayerVehicle(0)
  local playerID = player and player:getID() or nil
  if playerID ~= lastPlayerID then
    lastPlayerID      = playerID
    currentTargetID   = nil
    currentTargetDist = math.huge
    if couplerTrackActive then
      couplerTrackActive = false
      if udpSend then udpSend:send("COUPLER_LOST") end
    end
    -- Coupler distance mode survives switch but needs re-discovery
    if couplerDistMode then
      couplerDistReady = false
      _cdDiscovering = false
      _cdPendingPlayer = nil
      _cdPendingTarget = nil
    end
    scannerLog('info', "Player vehicle changed; target lock reset.")
    if udpSend and player then
      -- Query brand/model from inside the vehicle VM; falls back to jbeam filename
      local fallback = player:getJBeamFilename() or "unknown"
      player:queueLuaCommand(string.format([[
        local info = (v.data and v.data.information) or {}
        local brand = tostring(info.brand or "")
        local model = tostring(info.name or %q)
        local display = brand ~= "" and (brand .. " " .. model) or model
        obj:queueGameEngineLua(string.format(
          "extensions.vehicleScanner.onVehicleNameReady(%%q)", display
        ))
      ]], fallback))
    end
  end

  if not isScanModeActive then return end

  -- 3. Rate-limit scans to SCAN_INTERVAL seconds
  scanTimer = scanTimer + dtReal
  if scanTimer >= SCAN_INTERVAL then
    scanTimer = 0
    scanAndSendVehicleData()
  end

  -- 4. Coupler tracking (send bearing/distance between coupler nodes)
  if couplerTrackActive then
    couplerTrackTimer = couplerTrackTimer + dtReal
    if couplerTrackTimer >= (1.0 / COUPLER_TRACK_HZ) then
      couplerTrackTimer = 0
      local player = be:getPlayerVehicle(0)
      local target = scenetree.findObjectById(_couplerTargetID)
      if player and target and _playerCouplerCid and _targetCouplerCid then
        local pPos = vec3(player:getPosition()) + vec3(player:getNodePosition(_playerCouplerCid))
        local tPos = vec3(target:getPosition()) + vec3(target:getNodePosition(_targetCouplerCid))
        local dist = pPos:distance(tPos)

        -- Bearing from player's REAR direction to target coupler
        -- 0° = directly behind = aligned for reversing
        local playerFwd = player:getDirectionVector()
        local playerUp  = player:getDirectionVectorUp()
        local rearDir   = vec3(-playerFwd.x, -playerFwd.y, -playerFwd.z)
        local toTarget  = (tPos - pPos):normalized()
        local cosAngle  = math.max(-1, math.min(1, rearDir:dot(toTarget)))
        local angleRad  = math.acos(cosAngle)
        local rightVec  = playerUp:cross(playerFwd)
        local dot       = rightVec:dot(toTarget)
        local bearing   = math.deg(angleRad) * (dot < 0 and -1 or 1)

        local inRange = dist <= COUPLER_RANGE_M and 1 or 0
        udpSend:send(string.format("COUPLER:%.4f,%.4f,%d", bearing, dist, inRange))
      else
        couplerTrackActive = false
        if udpSend then udpSend:send("COUPLER_LOST") end
        scannerLog('warn', "Coupler tracking stopped: vehicle lost.")
      end
    end
  end

  -- 5. Coupler Distance Mode (periodic speech callouts)
  if couplerDistMode and isScanModeActive and currentTargetID then
    local plyr = be:getPlayerVehicle(0)
    local pID = plyr and plyr:getID() or nil

    -- (Re)discover couplers when player or target changes
    if not couplerDistReady and not _cdDiscovering and plyr then
      if _cdTargetID ~= currentTargetID or _cdPlayerID ~= pID or (_cdPlayerCid == nil) then
        _cdDiscovering = true
        _cdPendingPlayer = nil
        _cdPendingTarget = nil
        _cdPlayerID = pID
        _cdTargetID = currentTargetID
        couplerDistReady = false

        -- Query player vehicle for best coupler node CID + rear overhang
        plyr:queueLuaCommand([[
          local best, bpri = nil, 0
          for _, nd in pairs(v.data.nodes) do
            local p = 0
            if nd.couplerTag then
              local cl = nd.couplerTag:lower()
              p = 1
              if cl == "tow_bar" then p = 2 end
              if cl:find("fifthwheel") or cl:find("tow_hitch") then p = 3 end
            end
            if p == 0 and nd.tag and type(nd.tag) == "string" then
              local tl = nd.tag:lower()
              if tl:find("fifthwheel") or tl:find("fifth_wheel") or tl:find("tow_hitch") then
                p = 3
              elseif tl:find("tow_bar") then
                p = 2
              end
            end
            if p == 0 and nd.couplerStrength and nd.couplerStrength > 0 then
              p = 1
            end
            if p > bpri then best = nd; bpri = p end
          end
          if best then
            -- Compute rear overhang: how far the vehicle body extends behind the coupler
            local cPos = vec3(obj:getNodePosition(best.cid))
            local fwd = vec3(obj:getDirectionVector())
            fwd.z = 0
            fwd = fwd:normalized()
            local rearOverhang = 0
            for _, nd in pairs(v.data.nodes) do
              local nPos = vec3(obj:getNodePosition(nd.cid))
              local diff = nPos - cPos
              local behindDist = -fwd:dot(diff)
              if behindDist > rearOverhang then rearOverhang = behindDist end
            end
            obj:queueGameEngineLua(string.format(
              'extensions.vehicleScanner.onCouplerDistPlayerInfo(%d, %.4f)', best.cid, rearOverhang))
          else
            obj:queueGameEngineLua('extensions.vehicleScanner.onCouplerDistPlayerInfo(-1, 0)')
          end
        ]])

        -- Query target vehicle for best coupler node CID + front overhang
        local targetVeh = scenetree.findObjectById(currentTargetID)
        if targetVeh then
          targetVeh:queueLuaCommand([[
            local best, bpri = nil, 0
            for _, nd in pairs(v.data.nodes) do
              local p = 0
              if nd.couplerTag then
                local cl = nd.couplerTag:lower()
                p = 1
                if cl == "tow_bar" then p = 2 end
                if cl:find("fifthwheel") or cl:find("tow_hitch") then p = 3 end
              end
              if p == 0 and nd.tag and type(nd.tag) == "string" then
                local tl = nd.tag:lower()
                if tl:find("fifthwheel") or tl:find("fifth_wheel") or tl:find("tow_hitch") then
                  p = 3
                elseif tl:find("tow_bar") then
                  p = 2
                end
              end
              if p == 0 and nd.couplerStrength and nd.couplerStrength > 0 then
                p = 1
              end
              if p > bpri then best = nd; bpri = p end
            end
            if best then
              -- Compute front overhang: how far the vehicle body extends past the coupler
              -- toward the coupling direction (from vehicle center toward coupler)
              local cPos = vec3(obj:getNodePosition(best.cid))
              local couplerDir = vec3(cPos.x, cPos.y, 0)
              local cdLen = couplerDir:length()
              if cdLen > 0.3 then
                couplerDir = couplerDir:normalized()
              else
                -- Coupler near center; use vehicle forward as fallback
                local fwd = vec3(obj:getDirectionVector())
                couplerDir = vec3(fwd.x, fwd.y, 0):normalized()
              end
              local frontOverhang = 0
              for _, nd in pairs(v.data.nodes) do
                local nPos = vec3(obj:getNodePosition(nd.cid))
                local diff = nPos - cPos
                local aheadDist = couplerDir:dot(diff)
                if aheadDist > frontOverhang then frontOverhang = aheadDist end
              end
              obj:queueGameEngineLua(string.format(
                'extensions.vehicleScanner.onCouplerDistTargetInfo(%d, %.4f)', best.cid, frontOverhang))
            else
              obj:queueGameEngineLua('extensions.vehicleScanner.onCouplerDistTargetInfo(-1, 0)')
            end
          ]])
        else
          _cdPendingTarget = false
          _tryCouplerDistReady()
        end
      end
    end

    -- Send distance/height data at interval when ready
    if couplerDistReady then
      couplerDistTimer = couplerDistTimer + dtReal
      if couplerDistTimer >= COUPLER_DIST_INTERVAL then
        couplerDistTimer = 0
        local p2 = be:getPlayerVehicle(0)
        local t2 = scenetree.findObjectById(_cdTargetID)
        if p2 and t2 and _cdPlayerCid and _cdTargetCid then
          local pPos = vec3(p2:getPosition()) + vec3(p2:getNodePosition(_cdPlayerCid))
          local tPos = vec3(t2:getPosition()) + vec3(t2:getNodePosition(_cdTargetCid))
          local dx = tPos.x - pPos.x
          local dy = tPos.y - pPos.y
          local horizDist = math.sqrt(dx * dx + dy * dy)
          local heightDiff = tPos.z - pPos.z
          -- Subtract body overhangs to report the physical gap, not coupler-to-coupler
          local gap = math.max(0, horizDist - _cdPlayerOverhang - _cdTargetOverhang)
          if udpSend then
            udpSend:send(string.format("COUPLER_DIST:%.3f,%.3f", gap, heightDiff))
          end
        else
          -- Lost a vehicle, need re-discovery next tick
          couplerDistReady = false
          _cdDiscovering = false
        end
      end
    end
  end
end

-- =================================================================================================
--  Coupler / Alignment Callbacks (called from vehicle VM via queueGameEngineLua)
-- =================================================================================================

-- Called from player vehicle VM with coupler info + local node position
function M.onPlayerCouplerInfo(cid, tag, nx, ny, nz)
  scannerLog('info', "Player coupler info: cid=" .. tostring(cid) .. " tag=" .. tostring(tag)
    .. " nodePos=(" .. tostring(nx) .. "," .. tostring(ny) .. "," .. tostring(nz) .. ")")
  if cid == -1 then
    _pendingPlayerInfo = false
  else
    _pendingPlayerInfo = {cid = cid, tag = tag, nx = nx or 0, ny = ny or 0, nz = nz or 0}
  end
  _tryCompleteCouplerSetup()
end

-- Called from target vehicle VM with coupler info + coupler world pos + target center pos
function M.onTargetCouplerForAlign(tid, cid, cx, cy, cz, ox, oy, oz, tag)
  scannerLog('info', "Target coupler info: tid=" .. tid .. " cid=" .. tostring(cid) .. " tag=" .. tostring(tag))
  if cid == -1 then
    _pendingTargetInfo = false
  else
    _pendingTargetInfo = {cid = cid, tag = tag, cx = cx, cy = cy, cz = cz, ox = ox, oy = oy, oz = oz}
  end
  _tryCompleteCouplerSetup()
end

-- Coupler Distance Mode callbacks (called from vehicle VM)
function M.onCouplerDistPlayerInfo(cid, overhang)
  scannerLog('info', "Coupler dist player info: cid=" .. tostring(cid) .. " rearOverhang=" .. tostring(overhang))
  if cid == -1 then
    _cdPendingPlayer = false
  else
    _cdPendingPlayer = cid
    _cdPlayerCid = cid
    _cdPlayerOverhang = overhang or 0
  end
  _tryCouplerDistReady()
end

function M.onCouplerDistTargetInfo(cid, overhang)
  scannerLog('info', "Coupler dist target info: cid=" .. tostring(cid) .. " frontOverhang=" .. tostring(overhang))
  if cid == -1 then
    _cdPendingTarget = false
  else
    _cdPendingTarget = cid
    _cdTargetCid = cid
    _cdTargetOverhang = overhang or 0
  end
  _tryCouplerDistReady()
end

function M.onTargetNameReady(displayName)
  if udpSend then
    udpSend:send("TARGET_NAME:" .. displayName)
  end
end

function M.onVehicleNameReady(displayName)
  if udpSend then
    udpSend:send("SWITCHED:" .. displayName)
  end
end

function M.getCurrentTargetID()
  return currentTargetID
end

return M
