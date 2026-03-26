local M = {}

local TAG = 'bnvdaAutoSpawner'

local _pendingEnsure = false
local _pendingFrames = 0
local DELAY_FRAMES = 30  -- ~0.5s at 60fps

local function ensureApp()
  guihooks.trigger('appContainer:ensureAppVisible', '{"appName":"bnvdaHook"}')
  log('I', TAG, 'Requested ensureAppVisible for bnvdaHook')
end

function M.onExtensionLoaded()
  log('I', TAG, 'bnvdaAutoSpawner extension loaded.')
  setExtensionUnloadMode(M, "manual")
end

function M.onUiReady()
  log('I', TAG, 'onUiReady fired, deferring ensureApp by ' .. DELAY_FRAMES .. ' frames')
  _pendingEnsure = true
  _pendingFrames = 0
end

function M.onUpdate(dtReal, dtSim, dtRaw)
  if _pendingEnsure then
    _pendingFrames = _pendingFrames + 1
    if _pendingFrames >= DELAY_FRAMES then
      _pendingEnsure = false
      ensureApp()
    end
  end
end

return M
