local M = {}

local TAG = 'bnvdaAutoSpawner'

local function ensureApp()
  guihooks.trigger('appContainer:ensureAppVisible', '{"appName":"bnvdaHook"}')
  log('I', TAG, 'Requested ensureAppVisible for bnvdaHook')
end

local function onUILayoutLoaded(layoutType)
  log('I', TAG, 'onUILayoutLoaded fired, layoutType=' .. tostring(layoutType))
  ensureApp()
end

local function onUiReady()
  log('I', TAG, 'onUiReady fired')
  ensureApp()
end

local function onExtensionLoaded()
  log('I', TAG, 'bnvdaAutoSpawner extension loaded.')
  setExtensionUnloadMode(M, "manual")
end

M.onExtensionLoaded = onExtensionLoaded
M.onUILayoutLoaded = onUILayoutLoaded
M.onUiReady = onUiReady

return M
