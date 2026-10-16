-- =================================================================================================
--
--  Engine sound parameters, vehicle side, for audioDebugAccessible.lua (GE). The accessible
--  counterpart of the World Editor's Engine Audio Debug window (Window > Audio), which is ImGui
--  and has no accessibility tree.
--
--  Loaded on demand by the GE extension (extensions.load("beamtelAudioDebug")), the same way
--  drivingAssistant loads beamtelAssistant, so the cross-VM chunks stay one-line calls and never
--  carry the string.format templates the stock window ships across.
--
--  Every reply goes back through obj:queueGameEngineLua to
--  extensions.audioDebugAccessible.onVehicleData(payload). A SET is answered with a fresh
--  report, which is what Python takes as the confirmation.
--
--  Three things the stock window gets wrong, and this does not:
--    * It shows main_gain / muffled (the aggregates, which include the exhaust part's offsets)
--      and writes them back as base_gain / base_muffled with every offset zeroed, so touching
--      any control wipes the offset an exhaust part applied. Here the base is read, and the
--      FULL current parameter table is sent with one field replaced.
--    * electricMotor's setEngineSoundParameterList does not derive main_gain / muffled from the
--      base values the way combustionEngine's does, so on an EV the stock main gain and
--      muffling controls do nothing. Here an EV is written main_gain / muffled directly.
--    * electricMotor reads its fundamental frequency from device.fundamentalFrequencyRPMCoef,
--      not from the parameter table, so the cylinder control is not offered there at all.
--
-- =================================================================================================

local M = {}

-- Per sound, the parameter table as it was before this module first saw it, so RESTORE puts back
-- the vehicle's own values rather than the stock window's fixed "Reset All to 0" set (which is
-- not zero either: it writes eq_e_gain = 1). Keyed by soundKey. A vehicle reload rebuilds this VM
-- and so empties it, which is right: the sounds are rebuilt from jbeam at the same moment.
local originals = {}
local modified = {}

local function soundKey(device, ref)
  return tostring(device.name) .. ":" .. tostring(ref)
end

local function copyParams(p)
  local out = {}
  for k, v in pairs(p or {}) do out[k] = v end
  return out
end

local function isElectric(device)
  return device.type == "electricMotor"
end

-- Every engine sound on the vehicle, in a fixed order. sounds.getEngineSoundData walks the sound
-- configuration with pairs(), so its order is not stable between calls -- a list the user steps
-- through with left/right cannot be built on that.
local function collect()
  local list = {}
  local engines = powertrain.getDevicesByCategory("engine") or {}
  for engineIndex, device in ipairs(engines) do
    local cfg = device.getSoundConfiguration and device:getSoundConfiguration()
    if type(cfg) == "table" then
      local refs = {}
      for ref, data in pairs(cfg) do
        if type(data) == "table" and type(data.params) == "table" and data.soundID then
          refs[#refs + 1] = ref
        end
      end
      table.sort(refs)
      for _, ref in ipairs(refs) do
        list[#list + 1] = {device = device, ref = ref, data = cfg[ref], engineIndex = engineIndex}
      end
    end
  end
  return list
end

local function find(id)
  for _, s in ipairs(collect()) do
    if soundKey(s.device, s.ref) == id then return s end
  end
  return nil
end

local function captureOriginal(s)
  local key = soundKey(s.device, s.ref)
  if not originals[key] then originals[key] = copyParams(s.data.params) end
end

-- The user-facing value set for one sound. Frequencies stay on FMOD's 0-1000 scale: that is
-- what the browser steps in (Hz is spoken, converted in GE), because a fixed step in Hz is far
-- too coarse at 50 Hz and pointlessly fine at 10 kHz. A key absent here is not offered.
local function values(s)
  local p = s.data.params
  local electric = isElectric(s.device)
  local out = {}
  if electric then
    out.mainGain = p.main_gain
    out.muffling = p.muffled
  else
    out.mainGain = p.base_gain
    out.muffling = p.base_muffled
  end
  out.onLoadGain = p.onLoadGain
  out.offLoadGain = p.offLoadGain
  out.lowShelfFreq = p.eq_a_freq
  out.lowShelfGain = p.eq_a_gain
  out.highShelfFreq = p.eq_b_freq
  out.highShelfGain = p.eq_b_gain
  out.eqLowFreq = p.eq_c_freq
  out.eqLowGain = p.eq_c_gain
  out.eqLowWidth = p.eq_c_reso
  out.eqHighFreq = p.eq_d_freq
  out.eqHighGain = p.eq_d_gain
  out.eqHighWidth = p.eq_d_reso
  out.eqFundamentalGain = p.eq_e_gain
  if not electric and p.fundamentalFrequencyRPMCoef then
    out.cylinders = math.floor(p.fundamentalFrequencyRPMCoef * 120 + 0.5)
  end
  return out
end

-- user key -> parameter name. mainGain and muffling are resolved per device type in apply().
local PARAM = {
  onLoadGain = "onLoadGain", offLoadGain = "offLoadGain",
  lowShelfFreq = "eq_a_freq", lowShelfGain = "eq_a_gain",
  highShelfFreq = "eq_b_freq", highShelfGain = "eq_b_gain",
  eqLowFreq = "eq_c_freq", eqLowGain = "eq_c_gain", eqLowWidth = "eq_c_reso",
  eqHighFreq = "eq_d_freq", eqHighGain = "eq_d_gain", eqHighWidth = "eq_d_reso",
  eqFundamentalGain = "eq_e_gain",
}

local function report(errorText)
  local sounds_ = {}
  for _, s in ipairs(collect()) do
    captureOriginal(s)
    local key = soundKey(s.device, s.ref)
    local blend = tostring(s.data.blendFile or "")
    blend = blend:match("([^/]+)$") or blend
    blend = blend:gsub("%.sfxBlend2D%.json$", "")
    local entry = {
      id = key,
      device = tostring(s.device.name),
      ref = tostring(s.ref),
      deviceType = tostring(s.device.type),
      blend = blend,
      values = values(s),
      modified = modified[key] and true or false,
    }
    -- The exhaust's base gain is jbeam mainGain PLUS the gain change the fitted exhaust part
    -- adds (combustionEngine: main_gain = exhaustMainGain + maxExhaustAudioGain). The jbeam
    -- export needs mainGain alone, so the part's share rides along.
    if s.ref == "exhaust" and s.device.exhaustMainGain and originals[key] and originals[key].base_gain then
      entry.exhaustPartGain = originals[key].base_gain - s.device.exhaustMainGain
    end
    sounds_[#sounds_ + 1] = entry
  end
  local payload = {vehId = obj:getID(), sounds = sounds_, error = errorText}
  obj:queueGameEngineLua("if extensions.audioDebugAccessible then extensions.audioDebugAccessible.onVehicleData("
    .. serialize(payload) .. ") end")
end

local function apply(s, params)
  s.device:setEngineSoundParameterList(s.data.soundID, params, s.ref)
end

function M.report()
  report(nil)
end

function M.set(id, key, value)
  local s = find(id)
  if not s then return report("That sound is no longer on this vehicle") end
  value = tonumber(value)
  if not value then return report("Not a number") end
  captureOriginal(s)

  local electric = isElectric(s.device)
  local params = copyParams(s.data.params)
  if key == "mainGain" then
    if electric then params.main_gain = value else params.base_gain = value end
  elseif key == "muffling" then
    if electric then params.muffled = value else params.base_muffled = value end
  elseif key == "cylinders" then
    if electric then return report("Electric motors take their frequency from the motor, not this setting") end
    params.fundamentalFrequencyRPMCoef = value / 120
  elseif PARAM[key] then
    params[PARAM[key]] = value
  else
    return report("Unknown setting " .. tostring(key))
  end

  local ok, err = pcall(apply, s, params)
  if not ok then return report("Could not apply: " .. tostring(err)) end
  modified[soundKey(s.device, s.ref)] = true
  report(nil)
end

-- Only the fields this module edits are put back. The rest of the table is live: the rev limiter
-- and the exhaust part rewrite gainOffset / mufflingOffset* while the engine runs, so restoring
-- the whole snapshot would pin them at whatever they were when it was taken.
local EDITABLE = {"base_gain", "base_muffled", "main_gain", "muffled", "fundamentalFrequencyRPMCoef"}
for _, name in pairs(PARAM) do EDITABLE[#EDITABLE + 1] = name end

function M.restore()
  local restored = 0
  for _, s in ipairs(collect()) do
    local key = soundKey(s.device, s.ref)
    if modified[key] and originals[key] then
      local params = copyParams(s.data.params)
      for _, name in ipairs(EDITABLE) do
        if originals[key][name] ~= nil then params[name] = originals[key][name] end
      end
      local ok = pcall(apply, s, params)
      if ok then
        modified[key] = nil
        restored = restored + 1
      end
    end
  end
  report(restored == 0 and "Nothing to restore" or nil)
end

return M
