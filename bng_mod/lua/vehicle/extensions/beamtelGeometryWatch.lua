-- =================================================================================================
--
--  Break watcher, vehicle side, for vehicleGeometry.lua (GE).
--
--  Loaded by vehicleGeometry's own resolve chunk the first time it runs in this VM, so it exists
--  exactly on the vehicles something has asked for geometry about.
--
--  Why it exists: the geometry cache is resolved once and then trusted until a reset. Nothing
--  about a crash told it to look again, so a car that lost its bumper AFTER the first resolve
--  kept that bumper's cids as its front band -- and the scanner then measured from wherever the
--  bumper landed. Measured on a Vivace: the front band sat 80 m ahead of the car, on top of the
--  car it had hit, and the scanner read 0 ft to a target 400 ft away.
--
--  The game raises onBeamBroke for every beam that breaks (lua/vehicle/main.lua) and
--  onSlidenodeDetached when a node comes off its rail. A crash breaks hundreds of beams over a
--  second or so, so this waits for the breaking to stop (QUIET_S) before asking GE for one fresh
--  resolve, with MAX_WAIT_S as a ceiling so a vehicle that keeps shedding parts down a hillside
--  is still refreshed while it does.
--
-- =================================================================================================

local M = {}

local QUIET_S    = 0.5  -- no breaks for this long ends the burst
local MAX_WAIT_S = 2.0  -- refresh at least this often while a burst goes on

local quiet = nil  -- seconds left until the burst counts as over; nil = nothing pending
local waited = 0

local function arm()
  if not quiet then waited = 0 end
  quiet = QUIET_S
end

function M.onBeamBroke(id, energy) arm() end
function M.onSlidenodeDetached(nodeId, railId, energy) arm() end

function M.updateGFX(dt)
  if not quiet then return end
  quiet = quiet - dt
  waited = waited + dt
  if quiet > 0 and waited < MAX_WAIT_S then return end
  quiet = nil
  obj:queueGameEngineLua("if extensions.vehicleGeometry and extensions.vehicleGeometry.onVehicleBroke then "
    .. "extensions.vehicleGeometry.onVehicleBroke(" .. obj:getID() .. ") end")
end

-- A reset repairs every beam, and GE drops its cache on onVehicleResetted anyway.
function M.onReset() quiet = nil end

return M
