local M = {}

local TAG = 'bnvdaAutoSpawner'

local _initialSpawnDone = false

function M.onExtensionLoaded()
  log('I', TAG, 'bnvdaAutoSpawner extension loaded.')
  setExtensionUnloadMode(M, "manual")
end

-- Main menu: no layout loads, so onUILayoutLoaded won't fire.
-- ensureAppVisible is safe here since no layout change follows.
function M.onUiReady()
  if _initialSpawnDone then return end
  _initialSpawnDone = true
  log('I', TAG, 'onUiReady fired, spawning bnvdaHook for main menu.')
  guihooks.trigger('appContainer:ensureAppVisible', '{"appName":"bnvdaHook"}')
end

-- After any layout finishes loading (freeroam, campaign, etc.), re-inject
-- the app.  addApp avoids setting layoutDirty.
function M.onUILayoutLoaded(layoutType)
  log('I', TAG, 'onUILayoutLoaded: ' .. tostring(layoutType) .. ', adding bnvdaHook.')
  guihooks.trigger('appContainer:addApp', 'bnvdaHook')
end

return M
