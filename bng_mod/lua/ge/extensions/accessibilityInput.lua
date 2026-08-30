-- Controller-facing accessibility actions. BeamNG owns the input bindings;
-- Python owns the status catalog, cursor, formatting, and speech.
local M = {}

M.dependencies = {"core_input_categories"}

local allowedActions = {
  status_up = true,
  status_down = true,
  status_repeat = true,
  next_menu = true,
  previous_menu = true,
  activate = true,
}

local function registerCategory()
  if type(core_input_categories) ~= "table" then
    log("E", "accessibilityInput", "core_input_categories is unavailable")
    return
  end
  if core_input_categories.accessibility == nil then
    core_input_categories.accessibility = {
      order = 0.75,
      icon = "accessibility_new",
      title = "Accessibility",
    }
  end
end

function M.onExtensionLoaded()
  setExtensionUnloadMode(M, "manual")
  registerCategory()
end

function M.trigger(action)
  if not allowedActions[action] then
    log("W", "accessibilityInput", "Ignoring unknown accessibility action: " .. tostring(action))
    return
  end

  local bridge = extensions.bnvdaBridge
  if not bridge or type(bridge.sendFromUI) ~= "function" then return end
  bridge.sendFromUI(jsonEncode({type = "accessibility_action", action = action}))
end

return M
