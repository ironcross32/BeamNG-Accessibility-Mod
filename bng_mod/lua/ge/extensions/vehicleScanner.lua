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

-- Logging helper
local function scannerLog(level, msg)
  log(level, 'VehicleScanner', msg)
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
        scannerLog('info', "Scan mode deactivated via UDP.")
      elseif cmd == "NEXT" then
        cycleTarget(1)
      elseif cmd == "PREV" then
        cycleTarget(-1)
      elseif cmd == "ALIGN" then
        scannerLog('info', "ALIGN command received.")
        if not currentTargetID then
          udpSend:send("ALIGN_FAIL:No vehicle target locked")
        else
          local targetVeh = scenetree.findObjectById(currentTargetID)
          if not targetVeh then
            udpSend:send("ALIGN_FAIL:Target vehicle not found")
          else
            local player = be:getPlayerVehicle(0)
            if not player then
              udpSend:send("ALIGN_FAIL:No player vehicle")
            else
              alignPending = true
              alignTimeout = 0
              local tid = currentTargetID
              targetVeh:queueLuaCommand(string.format([[
                local bestNode = nil
                local bestTag = ""
                local bestPriority = 0
                for _, nd in pairs(v.data.nodes) do
                  local pri = 0
                  local t = ""
                  if nd.tag and type(nd.tag) == "string" then
                    local tl = nd.tag:lower()
                    if tl:find("fifthwheel") or tl:find("fifth_wheel") or tl:find("tow_hitch") then
                      pri = 3
                      t = nd.tag
                    end
                  end
                  if pri == 0 and nd.couplerTag then
                    local cl = nd.couplerTag:lower()
                    if cl == "tow_bar" then
                      pri = 2
                      t = nd.couplerTag
                    elseif pri == 0 then
                      pri = 1
                      t = nd.couplerTag
                    end
                  end
                  if pri > bestPriority then
                    bestNode = nd
                    bestTag = t
                    bestPriority = pri
                  end
                end
                if bestNode then
                  local np = vec3(obj:getNodePosition(bestNode.cid))
                  local wp = vec3(obj:getPosition()) + np
                  local dv = vec3(obj:getDirectionVector())
                  obj:queueGameEngineLua(string.format(
                    "extensions.vehicleScanner.onCouplerFound(%d, %%.4f, %%.4f, %%.4f, %%.4f, %%.4f, %%.4f, '%%s')",
                    wp.x, wp.y, wp.z, dv.x, dv.y, dv.z, bestTag
                  ))
                else
                  obj:queueGameEngineLua("extensions.vehicleScanner.onCouplerNotFound(%d)")
                end
              ]], tid, tid))
            end
          end
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
        udpSend:send("ALIGN_FAIL:Alignment timed out")
      end
      scannerLog('warn', "Alignment callback timed out.")
    end
  end

  -- 2b. Detect player vehicle switch by polling
  local player = be:getPlayerVehicle(0)
  local playerID = player and player:getID() or nil
  if playerID ~= lastPlayerID then
    lastPlayerID      = playerID
    currentTargetID   = nil
    currentTargetDist = math.huge
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
end

-- =================================================================================================
--  Trailer Alignment Callbacks (called from vehicle VM via queueGameEngineLua)
-- =================================================================================================

function M.onCouplerFound(trailerID, cx, cy, cz, dx, dy, dz, tag)
  alignPending = false
  alignTimeout = 0
  scannerLog('info', "Coupler found on vehicle " .. trailerID .. " (tag: " .. tostring(tag) .. ")")

  local player = be:getPlayerVehicle(0)
  if not player then
    if udpSend then udpSend:send("ALIGN_FAIL:No player vehicle") end
    return
  end

  local couplerPos = vec3(cx, cy, cz)

  -- Trailer forward direction projected to horizontal plane
  local trailerDir = vec3(dx, dy, dz)
  trailerDir.z = 0
  trailerDir = trailerDir:normalized()

  -- Place truck 5m ahead of coupler along trailer's forward direction
  local alignPos = couplerPos + trailerDir * 5.0
  alignPos.z = alignPos.z + 0.3  -- lift slightly to avoid ground clip

  -- Truck faces same direction as trailer (rear points toward coupler)
  -- Player reverses to close the gap and couple
  local rot = quatFromDir(trailerDir, vec3(0, 0, 1))
  spawn.safeTeleport(player, alignPos, rot, nil, true)

  if udpSend then
    udpSend:send("ALIGN_OK")
  end
  scannerLog('info', "Player teleported to alignment position.")
end

function M.onCouplerNotFound(trailerID)
  alignPending = false
  alignTimeout = 0
  scannerLog('warn', "No coupler found on vehicle " .. trailerID)
  if udpSend then
    udpSend:send("ALIGN_FAIL:No coupler found on target vehicle")
  end
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
