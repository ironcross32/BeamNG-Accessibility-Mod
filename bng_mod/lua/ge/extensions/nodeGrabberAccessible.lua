-- =================================================================================================
--
--  Accessible Node Grabber for BeamNG.drive Accessibility Mod (GE Extension)
--
--  Description: Detects vehicle nodes under the mouse cursor (ray-sphere intersection)
--               and sends node metadata to the Python backend via UDP.
--               Supports SNAP command to warp cursor to the nearest node to camera center.
--               UNPIN releases every pinned node on every vehicle.
--               Receives ON/OFF/SCAN_ON/SCAN_OFF/SNAP/UNPIN commands from Python.
--               Always reports the game's controller node grabber mode (PADMODE:1/0); while
--               the accessible grabber is on and controller mode is active, scans the screen
--               centre instead of the cursor and reports GRAB:1/0 and PINNED.
--
--  Loaded by: scripts/bng_screenreader_mod/modScript.lua
--
--  Version:     1.1
--  Target Game: BeamNG.drive 0.39+ (controller mode); 0.37+ otherwise
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

-- The game's controller node grabber (core_nodegrabberGamepad, 0.39+). It aims at the centre of
-- the screen rather than the mouse, so while it is on the scan follows the camera instead.
local PAD_PICK_TAN      = 0.08    -- how far off the centre ray a node may sit (tangent, ~4.6 deg)
local lastPadActive     = false
local lastPadGrabbing   = false
local wrappedPad        = nil     -- the gamepad module table whose fixCurrentNode we wrapped

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

-- Controller mode has no cursor: the target is whatever sits nearest the centre of the screen.
-- A blind player aims by ear, so a crosshair that must land inside a node's tiny hit sphere
-- would almost never report anything; instead take the node closest to the centre ray within
-- a narrow cone, preferring nearer nodes when several line up.
local function findCenterNode()
  if not nodeCache or #nodeCache == 0 then return nil end

  local player = be:getPlayerVehicle(0)
  if not player then return nil end

  local camPos = core_camera.getPosition()
  local camFwd = core_camera.getForward()
  if not camPos or not camFwd then return nil end
  local camPosV = vec3(camPos.x, camPos.y, camPos.z)
  local camFwdN = vec3(camFwd.x, camFwd.y, camFwd.z):normalized()

  local objPos = player:getPosition()
  local bestScore = math.huge
  local bestNode = nil

  for _, nd in ipairs(nodeCache) do
    local toNode = objPos + vec3(player:getNodePosition(nd.cid)) - camPosV
    local along = toNode:dot(camFwdN)
    if along > 0.05 and along < CULL_DISTANCE then
      local off = toNode:cross(camFwdN):length() / along
      if off <= PAD_PICK_TAN then
        local score = off + along * 0.001
        if score < bestScore then
          bestScore = score
          bestNode = nd
        end
      end
    end
  end

  return bestNode
end

-- =================================================================================================
--  Controller node grabber (core_nodegrabberGamepad)
-- =================================================================================================

-- rawget, never the plain global: extensions.lua loads a module on first access, and reading
-- this every frame must not load the game's grabber into a session that never asked for it.
local function getPad()
  local pad = rawget(_G, "core_nodegrabberGamepad")
  if type(pad) ~= "table" or type(pad.state) ~= "table" then return nil end
  return pad
end

-- The pin button (nodegrabberAction: middle mouse, or the pad's action button) goes through
-- core_nodegrabberGamepad.fixCurrentNode in both modes, and the action map resolves that field
-- at press time -- so wrapping it is the one place a pin is seen rather than guessed from a
-- mouse click. Re-wrapped whenever the module table changes (it is rebuilt on a Lua reload).
local pinOriginal, pinWrapper = nil, nil

local function ensurePinWrapped(pad)
  if not pad or wrappedPad == pad then return end
  local original = pad.fixCurrentNode
  if type(original) ~= "function" then return end
  pinOriginal = original
  pinWrapper = function(...)
    if isActive and udpSend then udpSend:send("PINNED") end
    return original(...)
  end
  pad.fixCurrentNode = pinWrapper
  wrappedPad = pad
end

-- Put the game's function back, or a reload of this extension would wrap the old wrapper and
-- every pin would be reported twice.
local function restorePinWrap()
  if wrappedPad and wrappedPad.fixCurrentNode == pinWrapper then
    wrappedPad.fixCurrentNode = pinOriginal
  end
  wrappedPad, pinOriginal, pinWrapper = nil, nil, nil
end

-- Sent whether or not the accessible grabber is on: the game toggles controller mode silently,
-- and also drops out of it on its own (a camera change, opening a menu, resetting the car).
local function pollPad()
  local pad = getPad()
  local active = pad and pad.state.active and true or false
  local grabbing = pad and pad.state.grabbing and true or false
  if pad then ensurePinWrapped(pad) end

  if active ~= lastPadActive then
    lastPadActive = active
    lastHoveredCid = -1
    if udpSend then udpSend:send(active and "PADMODE:1" or "PADMODE:0") end
  end
  if grabbing ~= lastPadGrabbing then
    lastPadGrabbing = grabbing
    if isActive and udpSend then udpSend:send(grabbing and "GRAB:1" or "GRAB:0") end
  end
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
    -- Viewport size travels with the point; see the note in clickspotAccessible.
    local packet = string.format("SNAP:%d,%d,%d,%d,%d,%s,%s,%s,%.3f",
      screenX, screenY, screenW, screenH,
      bestNode.cid, bestNode.name, bestNode.location, bestNode.groups, bestNode.heightNorm)
    udpSend:send(packet)
    ngLog('info', string.format("SNAP to node %d (%s) at screen %d,%d", bestNode.cid, bestNode.name, screenX, screenY))
  end
end

-- =================================================================================================
--  UNPIN: release every pinned (fixed) node
-- =================================================================================================

-- A pinned node is invisible to the ray scan, so finding it again with the mouse is guesswork.
-- This clears them all through the same call the game's own recovery uses
-- (lua/vehicle/recovery.lua). It runs on every vehicle, not just the player's: a node pinned
-- on a car you have since switched away from would otherwise stay fixed with no way back to it.
local function handleUnpin()
  local grabber = be and be.nodeGrabber
  local cleared = 0
  if grabber then
    for i = 0, be:getObjectCount() - 1 do
      local obj = be:getObject(i)
      if obj then
        local ok = pcall(function() grabber:clearVehicleFixedNodes(obj:getID()) end)
        if ok then cleared = cleared + 1 end
      end
    end
  else
    ngLog('warn', "be.nodeGrabber unavailable; cannot unpin nodes.")
  end
  ngLog('info', "UNPIN cleared fixed nodes on " .. cleared .. " vehicle(s)")
  if udpSend then
    udpSend:send(grabber and ("UNPINNED:" .. cleared) or "UNPIN_FAILED")
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
    -- setsockname RETURNS nil plus a message; it does not THROW. A pcall around it reports
    -- success on a socket bound to nothing, and the extension then goes deaf with nothing in
    -- the log -- it still sends normally, because a UDP sender needs no bind.
    local bound, berr = udpCmd:setsockname("127.0.0.1", CMD_LISTEN_PORT)
    if not bound then error(tostring(berr), 0) end
    udpCmd:settimeout(0)
  end)
  if ok and udpCmd then
    ngLog('info', "UDP command socket listening on port " .. CMD_LISTEN_PORT)
  else
    ngLog('error', "Failed to create UDP command socket: " .. tostring(err))
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
    ngLog('info', "UDP command socket bound on port " .. CMD_LISTEN_PORT .. " after retry.")
  end
end

-- setupSockets closes the sockets held by THIS module instance, and extensions.reload builds a
-- fresh instance whose locals are nil -- so it closes nothing and the outgoing instance keeps
-- the port, leaving the reloaded copy permanently deaf. Hence this hook.
function M.onExtensionUnloaded()
  restorePinWrap()
  if udpSend then pcall(function() udpSend:close() end); udpSend = nil end
  if udpCmd  then pcall(function() udpCmd:close()  end); udpCmd  = nil end
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
  retryCmdBind(dtReal)
  pollPad()
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
          -- Quiet resync: Python may have started after the last PADMODE change.
          if udpSend then udpSend:send(lastPadActive and "PADSYNC:1" or "PADSYNC:0") end
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
        elseif cmd == "UNPIN" and isActive then
          handleUnpin()
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

  if not (isScanning or lastPadActive) then return end
  if not udpSend then return end

  -- While a node is held its neighbours swing past the aim point; keep reporting the held one.
  if lastPadGrabbing then return end

  -- 4. Detect hovered node (screen centre in controller mode, mouse cursor otherwise)
  local hovered
  if lastPadActive then hovered = findCenterNode() else hovered = findHoveredNode() end

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
