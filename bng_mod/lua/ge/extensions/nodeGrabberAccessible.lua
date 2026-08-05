-- =================================================================================================
--
--  Accessible Node Grabber for BeamNG.drive Accessibility Mod (GE Extension)
--
--  Description: Detects vehicle nodes under the mouse cursor (ray-sphere intersection)
--               and sends node metadata to the Python backend via UDP.
--               Supports SNAP command to warp cursor to the nearest node to camera center.
--               Receives ON/OFF/SCAN_ON/SCAN_OFF/SNAP commands from Python.
--
--  Loaded by: scripts/bng_screenreader_mod/modScript.lua
--
--  Version:     1.0
--  Target Game: BeamNG.drive 0.37+
--
-- =================================================================================================

local M = {}

-- Configuration
local PYTHON_HOST       = "127.0.0.1"
local PYTHON_PORT_DATA  = 4454   -- send node data to Python on this port
local CMD_LISTEN_PORT   = 4455   -- receive commands from Python on this port
local NODE_SIZE_FACTOR  = 0.012  -- ray-sphere hit radius factor (matches meshEditor)
local SCAN_INTERVAL     = 0      -- every frame while scanning (mouse movement is per-frame)
local CULL_DISTANCE     = 50.0   -- ignore nodes farther than this from camera (meters)

-- Internal State
local udpSend           = nil
local udpCmd            = nil
local isActive          = false   -- master toggle
local isScanning        = false   -- true while CTRL held
local nodeCache         = nil     -- table of {cid, name, groups, localPos} from vehicle VM
local nodeCacheVehID    = nil     -- vehicle ID the cache belongs to (set only on successful cache)
local pendingCacheVehID = nil     -- vehicle ID we're trying to cache (async in-flight)
local cacheRetryTimer   = 0
local cacheRetryCount   = 0
local CACHE_RETRY_INTERVAL = 0.5
local MAX_CACHE_RETRIES = 10
local lastHoveredCid    = -1      -- avoid re-sending same node
local scanTimer         = 0

-- Logging helper
local function ngLog(level, msg)
  log(level, 'NodeGrabAccess', msg)
end

-- =================================================================================================
--  Node Metadata Cache (cross-VM call to vehicle)
-- =================================================================================================

local function buildNodeCache()
  local player = be:getPlayerVehicle(0)
  if not player then
    ngLog('warn', "No player vehicle for node cache.")
    nodeCache = {}
    nodeCacheVehID = nil
    pendingCacheVehID = nil
    return
  end

  local vehID = player:getID()
  if nodeCacheVehID == vehID and nodeCache and #nodeCache > 0 then
    pendingCacheVehID = nil
    return  -- already cached with data for this vehicle
  end

  ngLog('info', "Building node cache for vehicle ID " .. vehID .. " (attempt " .. cacheRetryCount .. ")")
  nodeCache = {}
  -- Don't set nodeCacheVehID here — wait for onNodeCacheReady callback
  pendingCacheVehID = vehID

  -- Query the vehicle VM for node data
  player:queueLuaCommand([[
    local results = {}
    if v.data and v.data.nodes then
      -- Get vehicle direction vectors for position classification
      local vehUp = vec3(obj:getDirectionVectorUp())
      local vehFwd = vec3(obj:getDirectionVector())
      local vehRight = vehUp:cross(vehFwd)

      -- First pass: find bounding box extents along vehicle axes
      local minFwd, maxFwd = math.huge, -math.huge
      local minRight, maxRight = math.huge, -math.huge
      local minUp, maxUp = math.huge, -math.huge

      for _, nd in pairs(v.data.nodes) do
        local lp = vec3(obj:getNodePosition(nd.cid))
        local f = vehFwd:dot(lp)
        local r = vehRight:dot(lp)
        local u = vehUp:dot(lp)
        if f < minFwd then minFwd = f end
        if f > maxFwd then maxFwd = f end
        if r < minRight then minRight = r end
        if r > maxRight then maxRight = r end
        if u < minUp then minUp = u end
        if u > maxUp then maxUp = u end
      end

      local fwdRange = maxFwd - minFwd
      local rightRange = maxRight - minRight
      local upRange = maxUp - minUp

      -- Second pass: classify each node
      for _, nd in pairs(v.data.nodes) do
        local lp = vec3(obj:getNodePosition(nd.cid))
        local f = vehFwd:dot(lp)
        local r = vehRight:dot(lp)
        local u = vehUp:dot(lp)

        -- Normalize to 0-1 range within bounding box
        local fNorm = fwdRange > 0.01 and ((f - minFwd) / fwdRange) or 0.5
        local rNorm = rightRange > 0.01 and ((r - minRight) / rightRange) or 0.5
        local uNorm = upRange > 0.01 and ((u - minUp) / upRange) or 0.5

        -- Classify forward/back
        local fbLabel
        if fNorm > 0.66 then fbLabel = "front"
        elseif fNorm < 0.33 then fbLabel = "rear"
        else fbLabel = "middle"
        end

        -- Classify left/right
        local lrLabel
        if rNorm > 0.66 then lrLabel = "right"
        elseif rNorm < 0.33 then lrLabel = "left"
        else lrLabel = "center"
        end

        -- Classify up/down
        local udLabel
        if uNorm > 0.66 then udLabel = "top"
        elseif uNorm < 0.33 then udLabel = "underside"
        else udLabel = "middle"
        end

        -- Build location string
        local location = fbLabel .. " " .. lrLabel
        if udLabel ~= "middle" then
          location = location .. " " .. udLabel
        end

        -- Node name
        local nodeName = nd.name or ("node_" .. nd.cid)

        -- Group membership
        local groupStr = ""
        if nd.group then
          if type(nd.group) == "table" then
            groupStr = table.concat(nd.group, "+")
          else
            groupStr = tostring(nd.group)
          end
        end

        -- Build CSV line: cid,name,groups,location,heightNorm
        table.insert(results, string.format("%d,%s,%s,%s,%.3f",
          nd.cid, nodeName, groupStr, location, uNorm))
      end
    end

    -- Send back to GE as batched data
    local payload = table.concat(results, "|")
    obj:queueGameEngineLua(string.format(
      "extensions.nodeGrabberAccessible.onNodeCacheReady(%d, %q)",
      obj:getID(), payload
    ))
  ]])
end

-- Callback from vehicle VM with node data
function M.onNodeCacheReady(vehID, payload)
  if vehID ~= pendingCacheVehID then return end

  nodeCache = {}
  if payload == "" then
    ngLog('info', "Node cache ready: 0 nodes (empty payload)")
    -- Don't mark as cached — might need retry
    return
  end

  for entry in payload:gmatch("[^|]+") do
    local cid, name, groups, location, heightNorm = entry:match("^(%d+),([^,]*),([^,]*),([^,]*),([%d%.]+)$")
    if cid then
      nodeCache[#nodeCache + 1] = {
        cid = tonumber(cid),
        name = name,
        groups = groups,
        location = location,
        heightNorm = tonumber(heightNorm) or 0.5,
      }
    end
  end

  if #nodeCache > 0 then
    -- Success — mark as cached and clear retry state
    nodeCacheVehID = vehID
    pendingCacheVehID = nil
    cacheRetryCount = 0
    ngLog('info', "Node cache ready: " .. #nodeCache .. " nodes")
  else
    ngLog('info', "Node cache callback returned 0 nodes, will retry")
  end
end

-- =================================================================================================
--  Ray-Sphere Node Detection
-- =================================================================================================

local function findHoveredNode()
  if not nodeCache or #nodeCache == 0 then return nil end

  local ray = getCameraMouseRay()
  if not ray then return nil end

  local camPos = vec3(ray.pos.x, ray.pos.y, ray.pos.z)
  local rayDir = vec3(ray.dir.x, ray.dir.y, ray.dir.z)

  local player = be:getPlayerVehicle(0)
  if not player then return nil end

  local objPos = player:getPosition()
  local minDist = math.huge
  local bestNode = nil

  for _, nd in ipairs(nodeCache) do
    -- World position of this node
    local nodeWorldPos = objPos + vec3(player:getNodePosition(nd.cid))

    -- Distance from camera to node (for culling and sphere radius)
    local distToCam = (nodeWorldPos - camPos):length()
    if distToCam < CULL_DISTANCE then
      -- Ray-sphere intersection: distance from node to ray line
      local toNode = nodeWorldPos - camPos
      local crossProd = toNode:cross(rayDir)
      local nodeRayDist = crossProd:length() / rayDir:length()
      local sphereRadius = distToCam * NODE_SIZE_FACTOR

      if nodeRayDist <= sphereRadius and distToCam < minDist then
        minDist = distToCam
        bestNode = nd
      end
    end
  end

  return bestNode
end

-- =================================================================================================
--  SNAP: Find closest node to camera center and project to screen coords
-- =================================================================================================

local function handleSnap()
  if not nodeCache or #nodeCache == 0 then return end

  local player = be:getPlayerVehicle(0)
  if not player then return end

  local camPos = core_camera.getPosition()
  local camFwd = core_camera.getForward()
  if not camPos or not camFwd then return end

  local objPos = player:getPosition()
  local camFwdN = vec3(camFwd.x, camFwd.y, camFwd.z):normalized()
  local camPosV = vec3(camPos.x, camPos.y, camPos.z)

  -- Find node closest to the camera's forward ray (center of screen)
  local minRayDist = math.huge
  local bestNode = nil
  local bestWorldPos = nil

  for _, nd in ipairs(nodeCache) do
    local nodeWorldPos = objPos + vec3(player:getNodePosition(nd.cid))
    local distToCam = (nodeWorldPos - camPosV):length()
    if distToCam < CULL_DISTANCE then
      local toNode = nodeWorldPos - camPosV
      local crossProd = toNode:cross(camFwdN)
      local nodeRayDist = crossProd:length() / camFwdN:length()
      -- Weight by distance too: prefer closer nodes when ray distance is similar
      local score = nodeRayDist + distToCam * 0.01
      if score < minRayDist then
        minRayDist = score
        bestNode = nd
        bestWorldPos = nodeWorldPos
      end
    end
  end

  if not bestNode or not bestWorldPos then return end

  -- Project world position to screen coordinates using manual perspective projection
  local camRight = core_camera.getRight()
  local camUp = core_camera.getUp()
  if not camRight or not camUp then return end

  local fovDeg = core_camera.getFovDeg()
  local vm = GFXDevice.getVideoMode()
  local screenW = vm.width
  local screenH = vm.height

  -- Vector from camera to node
  local toNode = bestWorldPos - camPosV

  -- Project onto camera axes
  local fwdDot = toNode:dot(camFwdN)
  if fwdDot <= 0.01 then return end  -- behind camera

  local rightDot = toNode:dot(vec3(camRight.x, camRight.y, camRight.z):normalized())
  local upDot = toNode:dot(vec3(camUp.x, camUp.y, camUp.z):normalized())

  -- Perspective divide
  local fovRad = math.rad(fovDeg)
  local tanHalfFov = math.tan(fovRad / 2.0)
  local aspect = screenW / screenH

  -- NDC coordinates (-1 to 1)
  local ndcX = (rightDot / fwdDot) / (tanHalfFov * aspect)
  local ndcY = (upDot / fwdDot) / tanHalfFov

  -- Screen coordinates
  local screenX = math.floor((ndcX + 1.0) * 0.5 * screenW)
  local screenY = math.floor((1.0 - ndcY) * 0.5 * screenH)  -- Y is inverted

  -- Clamp to screen bounds
  screenX = math.max(0, math.min(screenW - 1, screenX))
  screenY = math.max(0, math.min(screenH - 1, screenY))

  -- Send SNAP packet to Python
  if udpSend then
    local packet = string.format("SNAP:%d,%d,%d,%s,%s,%s,%.3f",
      screenX, screenY, bestNode.cid, bestNode.name, bestNode.location, bestNode.groups, bestNode.heightNorm)
    udpSend:send(packet)
    ngLog('info', string.format("SNAP to node %d (%s) at screen %d,%d", bestNode.cid, bestNode.name, screenX, screenY))
  end
end

-- =================================================================================================
--  GE Extension Hooks
-- =================================================================================================

local function setupSockets()
  -- Close existing sockets (no-op on first call; matters on map reload / Ctrl+L)
  if udpSend then pcall(function() udpSend:close() end); udpSend = nil end
  if udpCmd  then pcall(function() udpCmd:close()  end); udpCmd  = nil end

  -- Create send socket
  udpSend = socket.udp()
  if udpSend then
    udpSend:setpeername(PYTHON_HOST, PYTHON_PORT_DATA)
    udpSend:settimeout(0)
    ngLog('info', "UDP send socket targeting " .. PYTHON_HOST .. ":" .. PYTHON_PORT_DATA)
  else
    ngLog('error', "Failed to create UDP send socket.")
  end

  -- Create command receive socket
  local ok, err = pcall(function()
    udpCmd = socket.udp()
    udpCmd:setsockname("127.0.0.1", CMD_LISTEN_PORT)
    udpCmd:settimeout(0)
  end)
  if ok and udpCmd then
    ngLog('info', "UDP command socket listening on port " .. CMD_LISTEN_PORT)
  else
    ngLog('error', "Failed to create UDP command socket: " .. tostring(err))
    udpCmd = nil
  end
end

function M.onExtensionLoaded()
  setExtensionUnloadMode(M, "manual")
  ngLog('info', "Accessible node grabber extension loaded.")
  -- Bind sockets here so Ctrl+L Lua reload re-opens them; onWorldReadyState
  -- doesn't fire on a hot reload.
  setupSockets()
end

function M.onVehicleSwitched(oldId, newId, player)
  if not isActive then return end
  if player ~= 0 then return end
  ngLog('info', "Vehicle switched from " .. tostring(oldId) .. " to " .. tostring(newId))
  nodeCache = {}
  nodeCacheVehID = nil
  lastHoveredCid = -1
  pendingCacheVehID = newId
  cacheRetryTimer = 0
  cacheRetryCount = 0
end

function M.onWorldReadyState(state)
  if state == 2 then
    ngLog('info', "World ready. Initializing accessible node grabber.")

    -- Reset state on map (re)load
    isActive = false
    isScanning = false
    nodeCache = nil
    nodeCacheVehID = nil
    pendingCacheVehID = nil
    cacheRetryTimer = 0
    cacheRetryCount = 0
    lastHoveredCid = -1
    scanTimer = 0

    -- Re-bind sockets in case the previous bind was torn down
    setupSockets()
  end
end

function M.onUpdate(dtReal, dtSim, dtRaw)
  -- 1. Poll for commands from Python (drain all queued packets)
  if udpCmd then
    repeat
      local data = udpCmd:receive()
      if data then
        local cmd = data:match("^%s*(.-)%s*$"):upper()
        if cmd == "ON" and not isActive then
          isActive = true
          nodeCacheVehID = nil
          pendingCacheVehID = nil
          cacheRetryCount = 0
          cacheRetryTimer = 0
          ngLog('info', "Accessible node grabber activated.")
          buildNodeCache()
        elseif cmd == "OFF" and isActive then
          isActive = false
          isScanning = false
          lastHoveredCid = -1
          ngLog('info', "Accessible node grabber deactivated.")
        elseif cmd == "SCAN_ON" and isActive then
          isScanning = true
          lastHoveredCid = -1
        elseif cmd == "SCAN_OFF" then
          isScanning = false
          lastHoveredCid = -1
        elseif cmd == "SNAP" and isActive then
          handleSnap()
        elseif cmd == "RECACHE" and isActive then
          nodeCacheVehID = nil
          pendingCacheVehID = nil
          cacheRetryCount = 0
          buildNodeCache()
        end
      end
    until not data
  end

  if not isActive then return end

  -- 2. Check if vehicle changed (player switched vehicles)
  local player = be:getPlayerVehicle(0)
  if player then
    local currentID = player:getID()
    if currentID ~= nodeCacheVehID and currentID ~= pendingCacheVehID then
      pendingCacheVehID = currentID
      cacheRetryTimer = 0
      cacheRetryCount = 0
      nodeCache = {}
      nodeCacheVehID = nil
      lastHoveredCid = -1
    end
  end

  -- 3. Retry cache building if pending
  if pendingCacheVehID then
    cacheRetryTimer = cacheRetryTimer + dtReal
    if cacheRetryTimer >= CACHE_RETRY_INTERVAL then
      cacheRetryTimer = 0
      cacheRetryCount = cacheRetryCount + 1
      if cacheRetryCount <= MAX_CACHE_RETRIES then
        buildNodeCache()
      else
        -- Give up
        ngLog('warn', "Gave up building node cache after " .. MAX_CACHE_RETRIES .. " retries")
        pendingCacheVehID = nil
      end
    end
  end

  if not isScanning then return end
  if not udpSend then return end

  -- 4. Detect hovered node
  local hovered = findHoveredNode()

  if hovered then
    if hovered.cid ~= lastHoveredCid then
      lastHoveredCid = hovered.cid
      -- Send NODE packet: "NODE:<cid>,<name>,<location>,<groups>,<heightNorm>"
      local packet = string.format("NODE:%d,%s,%s,%s,%.3f",
        hovered.cid, hovered.name, hovered.location, hovered.groups, hovered.heightNorm)
      udpSend:send(packet)
    end
  else
    if lastHoveredCid ~= -1 then
      lastHoveredCid = -1
      udpSend:send("NODE_CLEAR")
    end
  end
end

return M
