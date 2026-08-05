-- Reliable UI/Python relay. The UI-to-GE hop uses BeamNG's supported
-- engineLua bridge; only GE Lua opens a loopback socket.
local M = {}

function M.onExtensionLoaded()
  setExtensionUnloadMode(M, "manual")
end

local socket = require('socket')
local host, port = '127.0.0.1', 8766
local retryDelay = 2
local connectTimeout = 1
local maxQueuedFrames = 256

local client = nil
local state = 'disconnected'
local retryAt = 0
local connectDeadline = 0
local lastRetryLog = 0
local receiveBuffer = ''
local outboundQueue = {}
local currentFrame = nil

local function clearOutbound()
  outboundQueue = {}
  currentFrame = nil
end

local function scheduleRetry(reason, wasConnected)
  if client then client:close() end
  client = nil
  state = 'disconnected'
  receiveBuffer = ''
  -- Never carry a partial JSON frame or stale speech into a new connection.
  clearOutbound()
  retryAt = socket.gettime() + retryDelay

  if wasConnected then
    log('W', 'bnvdaBridge', 'Lua TCP transport disconnected: ' .. tostring(reason))
  elseif socket.gettime() - lastRetryLog >= 10 then
    lastRetryLog = socket.gettime()
    log('I', 'bnvdaBridge', 'Python TCP relay unavailable; retrying every 2 seconds: ' .. tostring(reason))
  end
end

local function enqueue(payload)
  if #outboundQueue >= maxQueuedFrames then
    table.remove(outboundQueue, 1)
  end
  table.insert(outboundQueue, payload .. '\n')
end

local function markConnected()
  state = 'connected'
  connectDeadline = 0
  receiveBuffer = ''
  log('I', 'bnvdaBridge', 'Lua TCP socket connected to 127.0.0.1:8766; awaiting pong')
  enqueue('{"type":"transport_ping","transport":"lua-tcp"}')
end

local function startConnect()
  local now = socket.gettime()
  if client or now < retryAt then return end

  client = socket.tcp()
  client:settimeout(0)
  local ok, err = client:connect(host, port)
  if ok or err == 'already connected' then
    markConnected()
  elseif err == 'timeout' or err == 'Operation already in progress' then
    state = 'connecting'
    connectDeadline = now + connectTimeout
  else
    scheduleRetry(err, false)
  end
end

local function finishConnect()
  local now = socket.gettime()
  if now >= connectDeadline then
    scheduleRetry('connect timeout', false)
    return
  end

  local ok, _, writable = pcall(socket.select, {}, {client}, 0)
  if not ok then
    scheduleRetry('socket.select failed', false)
    return
  end
  if #writable == 0 then return end

  -- On Windows a failed nonblocking connect may also become writable. Verify
  -- that it has a peer rather than calling connect() on the same socket again.
  local peer, peerErr = client:getpeername()
  if peer then
    markConnected()
  else
    scheduleRetry(peerErr or 'connection refused', false)
  end
end

-- Called from CEF as extensions.bnvdaBridge.sendFromUI(<serialized string>).
function M.sendFromUI(payload)
  if type(payload) ~= 'string' then return end
  -- Messages produced while Python is down are stale UI state; do not replay
  -- them as a speech burst when the relay returns.
  if state ~= 'connected' then return end
  enqueue(payload)
end

local function pumpWrites()
  while client and state == 'connected' do
    if not currentFrame then
      if #outboundQueue == 0 then return end
      currentFrame = table.remove(outboundQueue, 1)
    end

    local sent, err, last = client:send(currentFrame)
    local count = sent or last or 0
    if count > 0 then currentFrame = currentFrame:sub(count + 1) end
    if currentFrame == '' then currentFrame = nil end

    if err == 'timeout' then return end
    if err then scheduleRetry(err, true); return end
  end
end

local function deliver(line)
  local ok, data = pcall(jsonDecode, line)
  if not ok or type(data) ~= 'table' then
    log('W', 'bnvdaBridge', 'Invalid JSON from Python relay')
    return
  end

  if data.type == 'transport_pong' then
    log('I', 'bnvdaBridge', 'Lua TCP transport ready; Python pong received')
  end
  guihooks.trigger('BNVDATransportMessage', data)
end

local function pumpReads()
  while client and state == 'connected' do
    local line, err, partial = client:receive('*l')
    if line then
      deliver(receiveBuffer .. line)
      receiveBuffer = ''
    elseif partial and #partial > 0 then
      receiveBuffer = receiveBuffer .. partial
    end
    if err == 'timeout' then return end
    if err then scheduleRetry(err, true); return end
  end
end

function M.onUpdate()
  startConnect()
  if not client then return end
  if state == 'connecting' then finishConnect() end
  if state ~= 'connected' then return end
  pumpWrites()
  if client then pumpReads() end
end

function M.onExtensionUnloaded()
  if client then client:close() end
  client = nil
  state = 'disconnected'
  clearOutbound()
end

return M
