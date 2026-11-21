-- Physical gates on the road network, found from the level's own static meshes.
--
-- BeamNG's "gated" flag is not a gate: an authored gatedRoad becomes
-- type = 'private' (traffic does not spawn there) and map.lua then puts a
-- gatedRoad flag on the link where private meets public as a routing cost.
-- Neither says a gate stands there; an audit of every stock level found one
-- near only about 5 percent of marked roads (docs/road-privacy-and-gates.md).
-- So "gate" is said only where a gate mesh sits on the carriageway.
--
-- A plain module required by roadDetector, not an extension: it owns no socket.
local M = {}

local STATICS_PER_TICK = 2000
M.ROAD_M = 6        -- gate origin to the link's centreline; roadside fences sit further out
M.DZ_M = 4
M.MERGE_M = 8       -- a left and right leaf, or a boom and its partner, are one gate
M.AT_START_M = 15   -- a gate this close past a junction is that road's entrance
M.FENCE_RUN_M = 15
M.FENCE_RUN_MAX = 3 -- more of one gate mesh than this inside FENCE_RUN_M is a fence built of them

-- Shape-name fragments of meshes that close a road. A guardhouse or control
-- booth stands beside the lane, and police barriers and closure signs are not
-- gates, so none of those count.
M.WORDS = {"boomgate", "boom_gate", "pole_boom", "metal_gate", "fence_gate",
  "swing_gate", "sliding_gate", "weight_station_barrier"}
M.EXCLUDE = {"pillar", "sign", "corrugated", "boom_lift"}

local scan = nil

-- The mesh's base name when it is a gate, else nil.
function M.classify(shapeName)
  if type(shapeName) ~= "string" or shapeName == "" then return nil end
  local base = shapeName:lower():match("([^/\\]+)$") or ""
  base = base:gsub("%.[^.]+$", "")
  for _, word in ipairs(M.EXCLUDE) do
    if base:find(word, 1, true) then return nil end
  end
  for _, word in ipairs(M.WORDS) do
    if base:find(word, 1, true) then return base end
  end
  return nil
end

-- Johnson Valley's settlement fence is built from gate panels. A run of the
-- same gate mesh is a fence, not a way in.
function M.dropFenceRuns(found)
  local kept = {}
  local runSq = M.FENCE_RUN_M * M.FENCE_RUN_M
  for _, a in ipairs(found) do
    local same = 0
    for _, b in ipairs(found) do
      if b.base == a.base then
        local dx, dy = a.pos.x - b.pos.x, a.pos.y - b.pos.y
        if dx * dx + dy * dy <= runSq then same = same + 1 end
      end
    end
    if same <= M.FENCE_RUN_MAX then kept[#kept + 1] = a end
  end
  return kept
end

function M.begin()
  local names = scenetree and scenetree.findClassObjects and scenetree.findClassObjects('TSStatic') or {}
  scan = {names = names, i = 1, found = {}}
end

-- Advances the amortised scan; returns the gate list once, on the tick it ends.
function M.step()
  if not scan then return nil end
  local names = scan.names
  local last = math.min(#names, scan.i + STATICS_PER_TICK - 1)
  for i = scan.i, last do
    local o = scenetree.findObject(names[i])
    local base = o and M.classify(o.shapeName)
    if base then
      local p = o:getPosition()
      scan.found[#scan.found + 1] = {pos = vec3(p.x, p.y, p.z), base = base}
    end
  end
  scan.i = last + 1
  if scan.i <= #names then return nil end
  local gates = M.dropFenceRuns(scan.found)
  scan = nil
  return gates
end

-- Puts each gate on the one link whose centreline it stands on, as
-- edge.gates = {{along = metres from inNode, id = ...}}. `near(pos)` lists the
-- links to try and `project` is roadDetector's projectOnEdge.
function M.attach(gates, near, project)
  local placed = 0
  for _, gate in ipairs(gates) do
    local best, bestP
    for _, edge in ipairs(near(gate.pos)) do
      local p = project(gate.pos, edge)
      if p.distance <= M.ROAD_M and math.abs(p.dz) <= M.DZ_M
        and (not bestP or p.distance < bestP.distance) then
        best, bestP = edge, p
      end
    end
    if best then
      local along = bestP.t * best.length
      best.gates = best.gates or {}
      local merged = false
      for _, other in ipairs(best.gates) do
        if math.abs(other.along - along) <= M.MERGE_M then merged = true; break end
      end
      if not merged then
        best.gates[#best.gates + 1] = {along = along, id = best.id .. '@' .. tostring(math.floor(along))}
        placed = placed + 1
      end
    end
  end
  return placed
end

-- The first gate on `record`'s link at least `fromAlong` metres along it in
-- the direction of travel: returns its distance from fromAlong and the gate.
function M.ahead(record, fromAlong)
  local gates = record.edge.gates
  if not gates then return nil end
  local length = record.edge.length
  local bestDistance, bestGate
  for _, gate in ipairs(gates) do
    local along = record.forward and gate.along or (length - gate.along)
    local d = along - (fromAlong or 0)
    if d >= 0 and (not bestDistance or d < bestDistance) then bestDistance, bestGate = d, gate end
  end
  return bestDistance, bestGate
end

return M
