-- =================================================================================================
--
--  Vehicle Naming Helper for BeamNG.drive Accessibility Mod (GE Extension)
--
--  Builds richer human-friendly identifiers for in-world vehicles than the bare JBeam name.
--  Used by vehicleSlots.lua and vehicleSpawnerAccessible.lua so that screen-reader announcements
--  can distinguish e.g. "ETK 800-Series 856ti, blue" from another ETK config or color.
--
--  All lookups are GE-side (no async cross-VM round trips):
--    - brand + friendly name come from core_vehicles.getVehicleList()
--    - configuration name is the basename of vehicle.partConfig
--    - color is mapped from vehicle.color to the nearest entry in a small named palette
--
-- =================================================================================================

local M = {}

local _modelInfo = nil   -- modelKey -> {name, brand}

local function buildModelInfo()
  _modelInfo = {}
  local ok, list = pcall(core_vehicles.getVehicleList)
  if not ok or not list or not list.vehicles then return end
  for _, e in pairs(list.vehicles) do
    local key = e.modelName or (e.model and e.model.key)
    if key and not _modelInfo[key] then
      local m = e.model or {}
      _modelInfo[key] = {
        name  = m.Name or m.name or key,
        brand = m.Brand or m.brand or "",
      }
    end
  end
end

local function getModelInfo(modelKey)
  if not _modelInfo then buildModelInfo() end
  return _modelInfo and _modelInfo[modelKey] or nil
end

-- Small named palette for nearest-match color naming. Keep entries broadly
-- distinguishable by ear; the goal is that the user can tell two cars apart,
-- not that we recover an exact paint code.
local _palette = {
  {"black",      0.02, 0.02, 0.02},
  {"white",      0.95, 0.95, 0.95},
  {"silver",     0.75, 0.75, 0.78},
  {"gray",       0.45, 0.45, 0.45},
  {"red",        0.80, 0.05, 0.05},
  {"dark red",   0.35, 0.03, 0.03},
  {"orange",     0.95, 0.45, 0.05},
  {"yellow",     0.95, 0.90, 0.10},
  {"green",      0.10, 0.55, 0.10},
  {"dark green", 0.05, 0.25, 0.05},
  {"blue",       0.05, 0.20, 0.75},
  {"light blue", 0.40, 0.65, 0.90},
  {"navy",       0.05, 0.05, 0.30},
  {"purple",     0.40, 0.05, 0.55},
  {"pink",       0.95, 0.55, 0.70},
  {"brown",      0.32, 0.18, 0.08},
  {"tan",        0.70, 0.60, 0.40},
  {"cyan",       0.10, 0.75, 0.85},
}

local function nearestColorName(r, g, b)
  local best, bestD = nil, math.huge
  for _, p in ipairs(_palette) do
    local dr, dg, db = r - p[2], g - p[3], b - p[4]
    local d = dr*dr + dg*dg + db*db
    if d < bestD then bestD = d; best = p[1] end
  end
  return best
end

local function vehicleColorComponents(v)
  local c = nil
  pcall(function() c = v.color end)
  if not c then pcall(function() c = v:getColor() end) end
  if not c then return nil end
  local r, g, b
  -- Color may come back as a Point4F (x,y,z,w), ColorF (r,g,b,a), or array.
  pcall(function() r = c.r or c.x or c[1] end)
  pcall(function() g = c.g or c.y or c[2] end)
  pcall(function() b = c.b or c.z or c[3] end)
  if type(r) ~= "number" or type(g) ~= "number" or type(b) ~= "number" then
    return nil
  end
  -- Some BeamNG paths return 0..255; clamp to 0..1 if they look like 0..255.
  if r > 1.5 or g > 1.5 or b > 1.5 then
    r, g, b = r / 255.0, g / 255.0, b / 255.0
  end
  return r, g, b
end

local function configNameFromPath(pc)
  if type(pc) ~= "string" or pc == "" then return nil end
  -- Some mods (e.g. some Bastion configurations) overwrite vehicle.partConfig
  -- with a serialized Lua table fragment instead of a path. Reject anything
  -- that contains table-syntax characters or is unreasonably long, otherwise
  -- the malformed name propagates into UDP packets and the per-tick
  -- string work amplifies into noticeable frame hitches.
  if #pc > 96 or pc:find("[{}%[%]=\"]") then return nil end
  local base = pc:match("([^/\\]+)$") or pc
  base = base:gsub("%.pc$", ""):gsub("%.json$", "")
  if base == "" then return nil end
  return base
end

local function modelKeyFromVehicle(v)
  local f = ""
  pcall(function() f = v:getJBeamFilename() or "" end)
  return f:match("([^/\\]+)%.jbeam$") or f:match("([^/\\]+)$") or ""
end

-- Returns a single rich display string, e.g. "ETK 800-Series 856ti, blue".
-- Falls back to the JBeam basename when richer fields are unavailable.
function M.describe(vehicle)
  if not vehicle then return "Unknown" end

  local modelKey = modelKeyFromVehicle(vehicle)
  if modelKey == "" then modelKey = "vehicle" end

  local info = getModelInfo(modelKey)
  local nameParts = {}
  if info then
    if info.brand and info.brand ~= "" then nameParts[#nameParts + 1] = info.brand end
    nameParts[#nameParts + 1] = info.name or modelKey
  else
    nameParts[#nameParts + 1] = modelKey
  end

  local pc = nil
  pcall(function() pc = vehicle.partConfig end)
  local cfg = configNameFromPath(pc)
  -- Suppress config when it's just the model key repeated or a generic default.
  if cfg and cfg ~= modelKey and cfg:lower() ~= "default" then
    nameParts[#nameParts + 1] = cfg
  end

  local out = table.concat(nameParts, " ")

  local r, g, b = vehicleColorComponents(vehicle)
  if r then
    local cname = nearestColorName(r, g, b)
    if cname then out = out .. ", " .. cname end
  end

  -- Sanitize protocol separators ('|' segments SLOTS, ';' segments ACTIVE_VEHICLES).
  out = out:gsub("[|;]", " ")
  return out
end

function M.invalidateCache()
  _modelInfo = nil
end

-- =================================================================================================
--  GE Extension Hooks
-- =================================================================================================

function M.onExtensionLoaded()
  log('I', 'VehicleNaming', "Vehicle naming helper loaded.")
end

function M.onModManagerReady()
  -- Mods may have added or removed vehicles; rebuild the lookup lazily.
  M.invalidateCache()
end

function M.onClientStartMission()
  M.invalidateCache()
end

return M
