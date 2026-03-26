-- This Source Code Form is subject to the terms of the bCDDL, v. 1.1.
-- If a copy of the bCDDL was not distributed with this
-- file, You can obtain one at http://beamng.com/bCDDL-1.1-bCDDL.txt

local M = {}
local hasShiftLights = false

-- Electrics dump: Python sends "DUMP" to port 4446, we send key=value lines to port 4447.
local DUMP_CMD_PORT  = 4446
local DUMP_RESP_PORT = 4447
local _cmdSock  = nil   -- receives "DUMP" commands
local _dumpSock = nil   -- sends dump lines back

-- Air pressure fallback: cache the *_pressureRelative key for vehicles that
-- don't expose a pressureTank controller (e.g. Gavril T-Series).
local _airPressureKey = nil   -- cached electrics key, e.g. "mainAirTank_pressureRelative"
local _airKeySearched = false -- true once we have committed to a result

local function init()
  local shiftLightControllers = controller.getControllersByType("shiftLights")
  hasShiftLights = shiftLightControllers and #shiftLightControllers > 0

  local ok
  ok = pcall(function()
    _cmdSock = socket.udp()
    _cmdSock:setsockname("127.0.0.1", DUMP_CMD_PORT)
    _cmdSock:settimeout(0)
  end)
  if not ok then _cmdSock = nil end

  ok = pcall(function()
    _dumpSock = socket.udp()
    _dumpSock:setpeername("127.0.0.1", DUMP_RESP_PORT)
    _dumpSock:settimeout(0)
  end)
  if not ok then _dumpSock = nil end
end

local function destroy()
  if _cmdSock then pcall(function() _cmdSock:close() end); _cmdSock = nil end
  if _dumpSock then pcall(function() _dumpSock:close() end); _dumpSock = nil end
end

local function reset() end
local function getAddress()        return "127.0.0.1" end
local function getPort()           return "4444" end
local function getMaxUpdateRate()  return 60 end

local function isPhysicsStepUsed()
  return false
end

local function getStructDefinition()
  return [[
    // --- Basic Data ---
    unsigned short flags;
    char           gear[4];
    char           plid;
    float          speed;
    float          rpm;
    float          rpmMax;
    float          turbo;
    float          turboMax;
    float          engTemp;
    float          fuel;
    float          oilPressure;
    float          oilTemp;
    unsigned       dashLights;
    unsigned       showLights;

    // --- Inputs ---
    float          throttle;
    float          brake;
    float          clutch;
    float          steering;
    float          actualSteering;
    float          steeringInput;    // electrics.values.steering_input (raw driver input)

    // --- Pneumatics ---
    float          airPressure;
    float          airPressureMax;
    
    // --- Expanded Telemetry ---
    float          clutchTemperature; // C
    float          g_lat;             // Lateral G-Force
    float          g_lon;             // Longitudinal G-Force

    // --- Tire Pressures (PSI) ---
    float          tirepressure_FL;
    float          tirepressure_FR;
    float          tirepressure_RL;
    float          tirepressure_RR;

    // --- Tire Temperatures (C) ---
    float          tiretemp_FL;
    float          tiretemp_FR;
    float          tiretemp_RL;
    float          tiretemp_RR;

    // --- Brake Temperatures (C) ---
    float          braketemp_FL;
    float          braketemp_FR;
    float          braketemp_RL;
    float          braketemp_RR;

    // --- Signal Inputs (0 or 1) ---
    float          signal_left_input;
    float          signal_right_input;
    float          hazard_enabled;
  ]]
end

-- OG_x and DL_x constants...
local OG_TURBO =  8192
local OG_KM    = 16384
local OG_BAR   = 32768
local DL_SHIFT        = 2 ^ 0
local DL_FULLBEAM     = 2 ^ 1
local DL_HANDBRAKE    = 2 ^ 2
local DL_TC           = 2 ^ 4
local DL_SIGNAL_L     = 2 ^ 5
local DL_SIGNAL_R     = 2 ^ 6
local DL_CHECK        = 2 ^ 7
local DL_OILWARN      = 2 ^ 8
local DL_BATTERY      = 2 ^ 9
local DL_ABS          = 2 ^ 10
local DL_LOWBEAM      = 2 ^ 11

local _cmdRebindCounter = 0

local function fillStruct(o, dtSim)
  -- If we lost (or never got) the command socket, try to reclaim it periodically.
  -- This handles vehicle switches where the old vehicle orphaned the port.
  if not _cmdSock then
    _cmdRebindCounter = _cmdRebindCounter + 1
    if _cmdRebindCounter >= 30 then  -- ~0.5s at 60fps
      _cmdRebindCounter = 0
      local ok = pcall(function()
        _cmdSock = socket.udp()
        _cmdSock:setsockname("127.0.0.1", DUMP_CMD_PORT)
        _cmdSock:settimeout(0)
      end)
      if not ok then _cmdSock = nil end
    end
  end
  if not _dumpSock then
    local ok = pcall(function()
      _dumpSock = socket.udp()
      _dumpSock:setpeername("127.0.0.1", DUMP_RESP_PORT)
      _dumpSock:settimeout(0)
    end)
    if not ok then _dumpSock = nil end
  end

  -- Diagnostic dump commands from beamtel.py
  if _cmdSock and _dumpSock then
    local cmd = _cmdSock:receive()
    if cmd == "DUMP" then
      -- Electrics dump: all electrics.values key=value pairs
      local lines = {}
      for k, v in pairs(electrics.values) do
        lines[#lines + 1] = tostring(k) .. "=" .. tostring(v)
      end
      table.sort(lines)
      for _, line in ipairs(lines) do
        _dumpSock:send(line)
      end
      _dumpSock:send("DONE")

    elseif cmd == "PDUMP" then
      -- Powertrain dump: top-level fields + named device enumeration via getDevice()
      local lines = {}

      -- 1. Top-level powertrain fields (environment scalars, etc.)
      for k, v in pairs(powertrain) do
        local vt = type(v)
        if vt == "number" or vt == "string" or vt == "boolean" then
          lines[#lines + 1] = tostring(k) .. "=" .. tostring(v)
        elseif vt == "table" then
          for sk, sv in pairs(v) do
            local svt = type(sv)
            if svt == "number" or svt == "string" or svt == "boolean" then
              lines[#lines + 1] = tostring(k) .. "." .. tostring(sk) .. "=" .. tostring(sv)
            end
          end
        end
      end

      -- 2. Named device access via powertrain.getDevice()
      if type(powertrain.getDevice) == "function" then
        local candidates = {
          "engine", "gearbox", "transmission", "driveshaft",
          "frontDifferential", "rearDifferential", "differential",
          "clutch", "torqueConverter",
          "steerR", "steerL", "steerF",
          "hydraulicPump", "mainPump",
          "boomLiftLeft", "boomLiftRight", "boomTilt",
          "articulationJoint", "articulation",
        }
        for _, devName in ipairs(candidates) do
          local ok, dev = pcall(function() return powertrain.getDevice(devName) end)
          if ok and dev and type(dev) == "table" then
            for sk, sv in pairs(dev) do
              local svt = type(sv)
              if svt == "number" or svt == "string" or svt == "boolean" then
                lines[#lines + 1] = devName .. "." .. tostring(sk) .. "=" .. tostring(sv)
              end
            end
          end
        end
      end

      table.sort(lines)
      for _, line in ipairs(lines) do
        _dumpSock:send(line)
      end
      _dumpSock:send("DONE")

    elseif cmd == "HDUMP" then
      -- Hydros dump: the hydraulic beams/actuators module
      local lines = {}

      local ok, err = pcall(function()
        if hydros then
          -- First: report what keys exist and their types
          lines[#lines + 1] = "--- hydros top-level keys ---"
          for k, v in pairs(hydros) do
            lines[#lines + 1] = "TYPE:hydros." .. tostring(k) .. "=" .. type(v)
          end

          -- hydros.values contents and types
          if hydros.values then
            lines[#lines + 1] = "--- hydros.values keys ---"
            local count = 0
            for k, v in pairs(hydros.values) do
              count = count + 1
              local vt = type(v)
              lines[#lines + 1] = "TYPE:values." .. tostring(k) .. "=" .. vt
              if vt == "number" or vt == "string" or vt == "boolean" then
                lines[#lines + 1] = "values." .. tostring(k) .. "=" .. tostring(v)
              elseif vt == "table" then
                for sk, sv in pairs(v) do
                  local svt = type(sv)
                  if svt == "number" or svt == "string" or svt == "boolean" then
                    lines[#lines + 1] = "values." .. tostring(k) .. "." .. tostring(sk) .. "=" .. tostring(sv)
                  else
                    lines[#lines + 1] = "TYPE:values." .. tostring(k) .. "." .. tostring(sk) .. "=" .. svt
                  end
                end
              end
            end
            lines[#lines + 1] = "values._count=" .. count
          else
            lines[#lines + 1] = "(hydros.values is nil)"
          end

          -- Dump hydros.hydros (the per-actuator table) with two levels of depth
          if type(hydros.hydros) == "table" then
            lines[#lines + 1] = "--- hydros.hydros entries ---"
            for k, v in pairs(hydros.hydros) do
              local prefix = "hydros." .. tostring(k)
              local vt = type(v)
              if vt == "number" or vt == "string" or vt == "boolean" then
                lines[#lines + 1] = prefix .. "=" .. tostring(v)
              elseif vt == "table" then
                for sk, sv in pairs(v) do
                  local svt = type(sv)
                  if svt == "number" or svt == "string" or svt == "boolean" then
                    lines[#lines + 1] = prefix .. "." .. tostring(sk) .. "=" .. tostring(sv)
                  else
                    lines[#lines + 1] = "TYPE:" .. prefix .. "." .. tostring(sk) .. "=" .. svt
                  end
                end
              end
            end
          end

          -- Dump remaining top-level scalars
          for k, v in pairs(hydros) do
            if k ~= "values" and k ~= "hydros" then
              local vt = type(v)
              if vt == "number" or vt == "string" or vt == "boolean" then
                lines[#lines + 1] = tostring(k) .. "=" .. tostring(v)
              end
            end
          end
        else
          lines[#lines + 1] = "(hydros module not present on this vehicle)"
        end
      end)
      if not ok then
        lines[#lines + 1] = "(error accessing hydros: " .. tostring(err) .. ")"
      end

      table.sort(lines)
      for _, line in ipairs(lines) do
        _dumpSock:send(line)
      end
      _dumpSock:send("DONE")

    elseif cmd == "DAMAGE" then
      -- Damage report: query damageTracker for all known damage conditions.
      -- damageTracker is the central registry that all subsystems report to.
      local found = false
      local ok, err = pcall(function()
        -- Body panels (float deformation values, 0 = undamaged)
        local bodyParts = {"FL", "FR", "ML", "MR", "RL", "RR"}
        local bodyLabels = {FL="Front left", FR="Front right", ML="Middle left", MR="Middle right", RL="Rear left", RR="Rear right"}
        for _, part in ipairs(bodyParts) do
          local val = damageTracker.getDamage("body", part)
          if type(val) == "number" and val > 0.01 then
            local severity = val > 0.5 and "heavy" or (val > 0.2 and "moderate" or "light")
            _dumpSock:send(bodyLabels[part] .. " body " .. severity .. " damage")
            found = true
          end
        end

        -- Engine damage (boolean flags)
        local engineChecks = {
          {"starvedOfOil",         "Oil starvation"},
          {"coolantOverheating",   "Coolant overheating"},
          {"oilOverheating",       "Oil overheating"},
          {"pistonRingsDamaged",   "Piston rings damaged"},
          {"rodBearingsDamaged",   "Rod bearings damaged"},
          {"headGasketDamaged",    "Head gasket damaged"},
          {"turbochargerHot",      "Turbocharger overheating"},
          {"turbochargerDamaged",  "Turbocharger damaged"},
          {"superchargerDamaged",  "Supercharger damaged"},
          {"inductionSystemDamaged", "Induction system damaged"},
          {"engineHydrolocked",    "Engine hydrolocked"},
          {"engineDisabled",       "Engine disabled"},
          {"blockMelted",          "Engine block melted"},
          {"cylinderWallsMelted",  "Cylinder walls melted"},
          {"engineLockedUp",       "Engine locked up"},
          {"radiatorLeak",         "Radiator leaking"},
          {"oilpanLeak",           "Oil pan leaking"},
          {"oilRadiatorLeak",      "Oil radiator leaking"},
          {"oilLevelCritical",     "Oil level critical"},
          {"engineReducedTorque",  "Engine torque reduced"},
        }
        for _, pair in ipairs(engineChecks) do
          local val = damageTracker.getDamage("engine", pair[1])
          if val and val ~= false and val ~= 0 then
            _dumpSock:send(pair[2])
            found = true
          end
        end

        -- Wheels: tires, wheel hubs, brakes
        local corners = {"FL", "FR", "RL", "RR"}
        local cornerLabels = {FL="Front left", FR="Front right", RL="Rear left", RR="Rear right"}
        for _, c in ipairs(corners) do
          local label = cornerLabels[c]
          if damageTracker.getDamage("wheels", "tire" .. c) == true then
            _dumpSock:send(label .. " tire burst"); found = true
          end
          if damageTracker.getDamage("wheels", c) == true then
            _dumpSock:send(label .. " wheel broken"); found = true
          end
          if damageTracker.getDamage("wheels", "brake" .. c) == true then
            _dumpSock:send(label .. " brake damaged"); found = true
          end
          local brakeHeat = damageTracker.getDamage("wheels", "brakeOverHeat" .. c)
          if type(brakeHeat) == "number" and brakeHeat > 0 then
            _dumpSock:send(label .. " brake fading"); found = true
          end
        end

        -- Powertrain devices (axles, driveshaft, engine)
        local ptChecks = {
          {"wheelaxleFL",  "Front left axle broken"},
          {"wheelaxleFR",  "Front right axle broken"},
          {"wheelaxleRL",  "Rear left axle broken"},
          {"wheelaxleRR",  "Rear right axle broken"},
          {"driveshaft",   "Driveshaft broken"},
          {"driveshaft_F", "Front driveshaft broken"},
          {"mainEngine",   "Engine broken"},
        }
        for _, pair in ipairs(ptChecks) do
          local val = damageTracker.getDamage("powertrain", pair[1])
          if val and val ~= false then
            _dumpSock:send(pair[2]); found = true
          end
        end

        -- Energy storage (fuel tank)
        local fuelVal = damageTracker.getDamage("energyStorage", "mainTank")
        if fuelVal and fuelVal ~= false then
          _dumpSock:send("Fuel tank damaged"); found = true
        end

        -- Gearbox
        if damageTracker.getDamage("gearbox", "synchroWear") == true then
          _dumpSock:send("Synchro wear"); found = true
        end

        -- Note: detached parts detection is in vehicleScanner.lua's DAMAGE handler
      end)
      if not ok then
        _dumpSock:send("(error: " .. tostring(err) .. ")")
        found = true
      end
      if not found then
        _dumpSock:send("NONE")
      end
      _dumpSock:send("DONE")
    end
  end

  if not electrics.values.watertemp then
    return false
  end

  o.flags = OG_KM + OG_BAR + (electrics.values.turboBoost and OG_TURBO or 0)
  o.gear = string.sub(electrics.values.gear .. "\0\0\0", 1, 4)
  o.plid = 0
  o.speed = electrics.values.wheelspeed or electrics.values.airspeed or 0
  o.rpm = electrics.values.rpm or 0
  o.rpmMax = electrics.values.maxrpm or 0
  o.turbo = (electrics.values.turboBoost or 0) / 14.504 -- BAR
  o.turboMax = (electrics.values.turboBoostMax or 0) / 14.504 -- BAR
  o.oilPressure = (powertrain.engine and powertrain.engine.oilPressure or 0) / 14.504 -- PSI to BAR
  o.engTemp = electrics.values.watertemp or 0
  o.fuel = electrics.values.fuel or 0
  o.oilTemp = electrics.values.oiltemp or 0

  o.dashLights = 0
  o.showLights = 0
  o.dashLights = bit.bor(o.dashLights, DL_FULLBEAM ) if electrics.values.highbeam      ~= 0 then o.showLights = bit.bor(o.showLights, DL_FULLBEAM ) end
  o.dashLights = bit.bor(o.dashLights, DL_HANDBRAKE) if electrics.values.parkingbrake  ~= 0 then o.showLights = bit.bor(o.showLights, DL_HANDBRAKE) end
  o.dashLights = bit.bor(o.dashLights, DL_SIGNAL_L ) if electrics.values.signal_L      ~= 0 then o.showLights = bit.bor(o.showLights, DL_SIGNAL_L ) end
  o.dashLights = bit.bor(o.dashLights, DL_SIGNAL_R ) if electrics.values.signal_R      ~= 0 then o.showLights = bit.bor(o.showLights, DL_SIGNAL_R ) end
  o.dashLights = bit.bor(o.dashLights, DL_CHECK ) if electrics.values.checkengine ~= false then o.showLights = bit.bor(o.showLights, DL_CHECK ) end
  if electrics.values.hasABS then
    o.dashLights = bit.bor(o.dashLights, DL_ABS    ) if electrics.values.absActive     ~= 0 then o.showLights = bit.bor(o.showLights, DL_ABS      ) end
  end
  o.dashLights = bit.bor(o.dashLights, DL_OILWARN  ) if electrics.values.oil           ~= 0 then o.showLights = bit.bor(o.showLights, DL_OILWARN  ) end
  o.dashLights = bit.bor(o.dashLights, DL_BATTERY  ) if electrics.values.engineRunning == 0 then o.showLights = bit.bor(o.showLights, DL_BATTERY  ) end
  if electrics.values.hasESC then
    o.dashLights = bit.bor(o.dashLights, DL_TC     ) if electrics.values.esc           ~= 0 or electrics.values.tcs ~= 0 then o.showLights = bit.bor(o.showLights, DL_TC       ) end
  end
  if hasShiftLights then
    o.dashLights = bit.bor(o.dashLights, DL_SHIFT  ) if electrics.values.shouldShift        then o.showLights = bit.bor(o.showLights, DL_SHIFT    ) end
  end
  o.dashLights = bit.bor(o.dashLights, DL_LOWBEAM ) if electrics.values.lowbeam      ~= 0 then o.showLights = bit.bor(o.showLights, DL_LOWBEAM ) end

  o.throttle = electrics.values.throttle or 0
  o.brake = electrics.values.brake or 0
  o.clutch = electrics.values.clutch or 0
  o.steering      = electrics.values.steering
  o.steeringInput = electrics.values.steering_input or 0
  local steerDevice = powertrain.getDevice("steerR")

  if steerDevice then
      -- .relExtension is a value from 0 to 1 representing
      -- how far the ram is pushed out.
      -- We transform it to -1.0 to 1.0 to match standard steering telemetry.
      o.actualSteering = (steerDevice.relExtension * 2) - 1.0
  elseif hydros and hydros.hydros then
      -- Fallback: find the steering actuator in the hydros table
      -- (e.g. WL-40 wheel loader uses articulated hydraulic steering)
      o.actualSteering = 0
      for _, h in pairs(hydros.hydros) do
        if h.inputSource == "steering_input" and h.steeringAutocenterEnabled == false then
          local range = math.max(math.abs(h.inLimit or 1), math.abs(h.outLimit or 1))
          o.actualSteering = (h.state or 0) / range  -- normalize to -1..1
          break
        end
      end
  else
      o.actualSteering = 0
  end
  
  local currentPa = 0
  local maxPa = 0
  local primaryTankName = nil
  local tanks = controller.getControllersByType("pressureTank")
  if tanks then
    for _, tank in pairs(tanks) do
      if tank.storage and tank.storage.isPrimarySupply then
        primaryTankName = tank.storage.name
        maxPa = tank.storage.maxWorkingPressure or 0
        break
      end
    end
    if primaryTankName then
      local electrics_key = primaryTankName .. "_pressureRelative"
      currentPa = electrics.values[electrics_key] or 0
    end
  end

  -- Fallback for vehicles without a pressureTank controller (e.g. T-Series):
  -- scan electrics.values once, prefer keys that contain "tank", cache the result.
  if currentPa == 0 and not _airKeySearched then
    if next(electrics.values) then          -- wait until electrics is populated
      _airKeySearched = true
      for k, v in pairs(electrics.values) do
        if type(k) == "string" and k:match("_pressureRelative$")
            and type(v) == "number" and v > 0 then
          if k:lower():find("tank") then
            _airPressureKey = k
            break                           -- "tank" key is best, stop immediately
          elseif not _airPressureKey then
            _airPressureKey = k             -- keep first match as fallback
          end
        end
      end
    end
  end
  if currentPa == 0 and _airPressureKey then
    currentPa = electrics.values[_airPressureKey] or 0
  end

  o.airPressure    = math.max(0, currentPa / 6894.76)  -- PSI, non-negative
  o.airPressureMax = maxPa / 6894.76                    -- PSI (0 = unknown)

  -- Get data from direct, reliable sources
  o.clutchTemperature = (powertrain.clutch and powertrain.clutch.temperature) or 0
  o.g_lat = sensors.gy2 or 0 -- Lateral Gs
  o.g_lon = sensors.gx2 or 0 -- Longitudinal Gs

  -- Get Tire and Brake data by iterating through the wheels
  local pressures = {}
  local tireTemps = {}
  local brakeTemps = {}
  if wheels and wheels.wheels then
    for _, wd in pairs(wheels.wheels) do
      local pressurePa = 0
      if wd.pressureGroup and v.data.pressureGroups and v.data.pressureGroups[wd.pressureGroup] then
        pressurePa = obj:getGroupPressure(v.data.pressureGroups[wd.pressureGroup]) - (powertrain.currentEnvPressure or 101325)
      end
      pressures[wd.name] = (pressurePa > 0 and pressurePa or 0) * 0.000145038 -- PSI
      tireTemps[wd.name] = (wd.thermals and wd.thermals.tireTemperature) or 0
      brakeTemps[wd.name] = wd.brakeTemperature or 0
    end
  end
  
  o.tirepressure_FL = pressures.FL or 0
  o.tirepressure_FR = pressures.FR or 0
  o.tirepressure_RL = pressures.RL or 0
  o.tirepressure_RR = pressures.RR or 0

  o.tiretemp_FL = tireTemps.FL or 0
  o.tiretemp_FR = tireTemps.FR or 0
  o.tiretemp_RL = tireTemps.RL or 0
  o.tiretemp_RR = tireTemps.RR or 0
  
  o.braketemp_FL = brakeTemps.FL or 0
  o.braketemp_FR = brakeTemps.FR or 0
  o.braketemp_RL = brakeTemps.RL or 0
  o.braketemp_RR = brakeTemps.RR or 0

  o.signal_left_input  = electrics.values.signal_left_input or 0
  o.signal_right_input = electrics.values.signal_right_input or 0
  o.hazard_enabled     = electrics.values.hazard_enabled or 0

  return true
end

M.init = init
M.destroy = destroy
M.reset = reset
M.getAddress = getAddress
M.getPort = getPort
M.getMaxUpdateRate = getMaxUpdateRate
M.getStructDefinition = getStructDefinition
M.fillStruct = fillStruct
M.isPhysicsStepUsed = isPhysicsStepUsed

return M