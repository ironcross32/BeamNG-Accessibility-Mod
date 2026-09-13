# Accessible BeamNG Map Editor: Findings and Design Notes

Research date: September 5, 2026.

This records the discussion about a screen-reader-accessible map editor that
would be a separate application from BeamTel. It is a feasibility assessment
and proposed design, not an implementation specification or commitment to build.
The investigation reviewed the existing map generator, the installed BeamNG Lua
tools and selected shipped level scenes, and official BeamNG documentation.
No new in-game validation was performed for this investigation.

## Overall feasibility

The proposed editor is feasible. New, Open, Save, Save As, terrain shaping,
surface painting, water placement, and export to a playable BeamNG level are
realistic capabilities. The largest design challenge is providing enough spatial
feedback for a screen-reader user to understand and confidently edit a landscape.

The existing generator already produces terrain, material definitions, scene
objects, water, roads, spawn points, and level metadata without the game running.
It provides useful groundwork, but its layout is specific to Proving Grounds.
A general editor would need its own project model, editing operations, interface,
undo system, validation, and export workflow.

The editor could operate independently of BeamTel. A separate, optional game
connection could later provide testing, position queries, and runtime validation.
Generated levels need not require the BeamTel application to load.

## How terrain works

A terrain block is a large sheet of ground divided into a grid, rather than a
collection of solid cubes. Heights describe the ground surface, and a material
layer identifies surfaces such as grass, dirt, asphalt, or mud. Water, buildings,
bridges, and other scene objects are separate from the terrain.

Raising heights makes hills; lowering them makes depressions. A heightfield stores
one ground height at each horizontal location, so it cannot by itself describe
both the floor and ceiling of a cave, an overhang, or a bridge above another road.
Those features require additional geometry. Terrain holes can remove surface and
collision where that geometry needs an opening.

BeamNG uses square terrain grids with power-of-two resolutions. The documented
heightmap import range is 128 through 8192 samples per side. Physical map size
depends on both sample count and spacing: a 2048 grid at one metre spacing covers
roughly two kilometres per side; at four metre spacing it covers roughly eight.
Increasing spacing increases coverage while reducing the ability to represent
small features. Increasing sample count increases memory and processing costs.

The existing Proving Grounds generator uses 2048 samples per side at 2.5 metre
spacing, covering approximately 5120 metres per side. Its terrain height encoding
range is 2100 metres; this is not a ceiling on vehicles or scene objects.

A rectangular playable area can be arranged within square terrain. The installed
terrain editor enumerates multiple TerrainBlock objects, but a multiple-block
workflow would require dedicated seam, collision, and tool-compatibility testing.
One block is a sensible first-version scope.

## New Map dialog

Suggested fields:

- Map name and physical size in metres or kilometres.
- Starting surface, such as grass, dirt, asphalt, or sand.
- Starting landscape: flat, gently rolling, mountainous, or imported heightmap.
- Terrain detail presets, with sample spacing and resolution available as
  advanced settings.
- Starting elevation and sufficient vertical range for excavation and hills,
  potentially selected automatically from the landscape preset.
- Boundary style, either here or in a separate Map Boundary dialog.

The interface should explain physical dimensions separately from terrain detail.
Users should not have to understand the binary format to choose useful settings.

## Editing capabilities and limits

| Capability | Assessment |
| --- | --- |
| New, Open, Save, Save As, autosave, undo, redo | Straightforward application features. |
| Raise, lower, flatten, smooth, roughen | Direct operations on the terrain height grid. |
| Hills, valleys, bowls, trenches, embankments, plateaus | Well suited to tools with dimensions, slopes, and shape controls. |
| Ponds and flooded depressions | Combine a shaped basin with water at a chosen elevation. |
| Surface painting | Feasible; appearance, traction, roughness, depth behavior, and tire sound need coordinated presets. |
| Roads and banked sections | Feasible; junctions, blending, and navigation add complexity. |
| Bridges, tunnels, caves, overhangs, vertical cliffs | Possible using separate geometry and terrain holes where needed. |
| Reopen the editor's own projects | A core feature. |
| Import arbitrary existing BeamNG levels | A later, substantially larger task; terrain reading alone is not complete level editing. |

Small bumps and narrow boundaries are limited by terrain spacing. At 2.5 metre
spacing, sub-metre washboard cannot be faithfully sculpted into the heightfield.
Mesh geometry or appropriate ground effects would be needed for finer features.

Surface appearance, physical behavior, and sound are related but distinct. The
existing research documents stock surfaces that drive differently without having
distinct tire sounds. An accessible editor should describe these differences
instead of assuming every material offers a useful audible cue.

### Water

A Create Pond tool could ask for length, width, maximum depth, bank slope, and
water elevation, then generate the basin and water together. It could calculate
depths and flag where the chosen water level reaches unintended surrounding land.

This is an editor calculation, not a promise of a fluid simulation that pours
water into a hole, automatically overflows, or erodes the ground. BeamNG supplies
bounded WaterBlock objects, large water planes, and rivers defined by paths.
Irregular shorelines and connected low areas require careful placement and checks.

Existing generator research found that WaterBlock's waterline is position.z,
not the top of its collision box. Texture and reflection setup also affect whether
water is visible. Moving a live water block previously left stale physics state;
fresh-load wetness tests are necessary before trusting changes.

### Roads and structures

Road tools could expose length, heading, curve radius, width, grade, banking,
shoulders, and surface. They could shape the underlying terrain automatically.
BeamNG distinguishes projected DecalRoad surfaces from MeshRoad geometry used for
bridges and elevated sections. Painting asphalt alone does not create an AI road;
navigation data must be generated and checked separately.

Reusable ramps, bridges, barriers, buildings, and tunnel modules could have known
dimensions, entry directions, and attachment points. The installed Road Architect
tunnel code combines procedural meshes with terrain holes, supporting the
feasibility of a later accessible tunnel tool. Asset availability and collision
must be checked for each supported item; do not assume every stock asset behaves
identically or is suitable for arbitrary scaling.

## Screen-reader interaction design

Named features and spatial relationships should be central. A user should be able
to select North Hill from a list, change its height, and inspect the consequences.
Three complementary interaction methods are proposed:

1. A feature list or tree for hills, ponds, roads, structures, and spawn points,
   with rename, move, duplicate, group, and property-editing actions.
2. Keyboard exploration with north/south/east/west movement, adjustable step size,
   and on-demand reports of elevation, slope, surface, water depth, and nearby
   features.
3. Relative placement, such as a spawn at a hill's base, a bridge between banks,
   or trees beside a road. Absolute coordinates remain available for precision.

Example operation, represented by ordinary labelled dialog fields:

> Create a hill 200 metres north of the starting point, 150 metres wide and
> 30 metres high, with a rounded summit.

Example inspection report:

> East bank of Lower Pond. Ground elevation 48 metres; water depth 2 metres.
> Shoreline 18 metres west. Access road 40 metres east.

Support both feature dialogs and a keyboard terrain brush with radius, strength,
and edge-softness controls. Optional tones could convey rising and falling ground,
with speech on demand. Avoid overwhelming users with continuous announcements.

Native accessible Windows controls are a plausible foundation. BeamTel's wxPython
configuration code contains useful experience with accessible labels and focus,
but the new editor should remain a separate application. Actual workflows need
testing with screen-reader users; speaking control labels alone does not establish
that spatial editing is usable.

## Map boundaries

Terrain extent and playable extent are different. A terrain block should not be
treated as an automatically sealed enclosure. Behavior beyond its edge depends
on other level geometry and scripts; this investigation did not establish a
universal current engine behavior outside every terrain block.

Selected shipped-scene inspection found GroundPlane objects in Small Grid, while
Gridmap v2, West Coast USA, and Utah contain TerrainBlock objects. The installed
tutorial code implements explicit out-of-bounds warnings and timeout callbacks.
That recovery behavior is gameplay logic, not an inherent wall at a terrain edge.

For the proposed editor, place the playable boundary inside the terrain extent,
leaving room behind it for scenery and containment.

### Terrain can be extremely steep, but not vertical

A 100 percent gradient means one metre up per metre forward: 45 degrees.
Vertical means 90 degrees. Adjacent terrain samples are connected by sloping
faces, so gradient equals height difference divided by horizontal distance.

At the generator's 2.5 metre sample spacing:

| Rise across one cell | Gradient | Approximate angle |
| --- | --- | --- |
| 5 metres | 200% | 63 degrees |
| 12 metres | 480% | 78 degrees |
| 25 metres | 1000% | 84 degrees |

The current hill-climb berm settings use a 12 metre rise over one cell. The
separate map perimeter raises an outer band to at least nine metres above the
base plain, with a gravel warning band inside it. That minimum absolute elevation
does not guarantee a nine metre obstacle above every adjacent feature. Code
inspection alone does not establish that either boundary contains every vehicle.

### Boundary options

1. **Mountainous perimeter:** a continuous ridge with a steep inward face and
   varied peaks behind it. Check for low passes, shallow diagonal routes, and
   unintended ramps. Natural variation must not introduce escape paths.
2. **Vertical physical barrier:** wall, retaining-wall, or cliff meshes with
   dedicated collision geometry. These can be vertical independently of terrain
   spacing. Fit their bases to the ground and check seams and corners.
3. **Combined boundary:** mountains or cliffs for the landscape, a continuous
   collision barrier integrated into the face, and optional recovery logic beyond
   it. This provides stronger containment than terrain steepness alone.

No single slope guarantees containment for every vehicle and approach speed.
Momentum, nearby launch ramps, unusually capable vehicles, and weak sections can
defeat a ridge. A smooth transition at the foot may become a launch ramp itself.
Barrier height, approach shape, continuity, and run-up all matter. Aircraft,
teleportation, and unusual mod vehicles require separate expectations.

Suggested Map Boundary dialog options:

- Style: mountain ridge, rock cliff, concrete wall, or open edge.
- Height, border width, and inward slope where applicable.
- Warning surface, such as gravel or rumble strips.
- Optional warning and recovery behavior beyond the physical boundary.

Example geometric validation report:

> Mountain boundary surrounds all four sides. Minimum inward slope: 78 degrees.
> One low pass found on the northeast corner.

This is a proposed report, not a result measured on an existing map. Geometry
checks must be followed by driving tests for collision, launches, and escape paths.
A useful default would be a mountain or cliff perimeter with a gravel warning
band and optional recovery beyond it.

## Additional possible features

- Audible driving boundaries using gravel verges and rumble strips.
- Seeded terrain generation, forests, rock fields, and islands, with exclusions
  around roads, water, and spawn points.
- Measurements of distance, elevation difference, steepest slope, road profile,
  bridge clearance, and water depth.
- Reports of disconnected roads, submerged spawns, missing assets, floating
  objects, abrupt transitions, and boundary weaknesses.
- Spawn locations and headings, destinations, checkpoints, time trials, and later
  traffic configuration.
- Time of day, fog, lighting, ambient sound, and tunnel reverb.
- Export packaging, preview images, and an optional Test From Here workflow.

These are candidate capabilities; they are not all necessary for a first version.

## Project storage and export

Save an editable project separately from the exported BeamNG level. The project
should retain named features, parameters, relationships, and the ordered terrain
operations that produced the result, together with any imported or directly
edited terrain data needed to reproduce it.

The exported heightmap contains resulting heights, not the original semantic
features. It cannot reliably reveal that a particular area was created by a
rounded-hill tool or recover that tool's settings. Imported existing levels would
therefore have a different editing experience unless meaningful features are
explicitly reconstructed.

Importing arbitrary levels also requires handling scene objects, materials,
dependencies, and custom scripts without losing unsupported content. Terrain
reading is an achievable early import feature; complete lossless level editing
is a substantially larger scope.

Export should use a staging and replacement strategy appropriate to user-created
projects. The current Proving Grounds build deletes and recreates its generated
mod folder; it is not a suitable general-purpose project-saving design.

## Suggested first milestone

A standalone Windows application with one terrain block, native accessible
controls, project saving and reopening, undo/redo, named terrain features, surface
painting, ponds, simple roads, spawn points, a basic boundary tool, and export.

The first end-to-end demonstration should let a screen-reader user create a map,
add a hill and pond, connect them with a road, configure its boundary, save and
reopen the project, export it, and drive it in BeamNG.

Later stages could add complex road networks, mesh structures, vegetation,
existing-map import, multiple terrain blocks, and a live game connection.

## Verification and outstanding questions

- Validate water wetness, material behavior and sound, spawn headings, road
  navigation, structure collision, and map boundaries in BeamNG.
- Test boundaries with ordinary cars, capable off-road vehicles, and high-speed
  approaches; include corners, low passes, and adjacent terrain features.
- Test accessibility with actual screen readers and users, including large
  feature lists, editing feedback, focus restoration, undo, and error reports.
- Benchmark terrain sizes and operation history before setting detail defaults.
- Determine the supported import/export versions and material workflow. The
  existing generator uses classic terrain materials; current documentation also
  describes a newer texture-set-based material workflow.
- Reconcile terrain height conversion before precise editor measurements: local
  generator documentation uses a divisor of 65535, whereas current official
  terrain documentation describes engine scaling by 65536. This investigation
  did not resolve the discrepancy or change the generator.
- Confirm individual stock asset dependencies in exported levels. Older local
  notes about level-local asset availability conflict with later successful
  tunnel-asset checks; avoid applying a blanket assumption.
- When testing reloads, BeamNG must have focus before treating the command as
  executed. Confirm a fresh load in beamng.log and use probes that distinguish
  the new build from the previous one.

## Sources and useful code

Repository sources:

- [Level generation research](docs/level-generation.md)
- [Terrain binary reader and writer](tools/mapgen/terfile.py)
- [Terrain synthesis and boundary settings](tools/mapgen/mapdef.py)
- [Level assembly and export](tools/mapgen/build.py)
- [In-game verification](tools/mapgen/verify.lua)
- [Accessible configuration controls](config_ui.py)

Installed game root inspected: `D:\Steam\steamapps\common\BeamNG.drive`.
Relevant files relative to that root:

- `lua/ge/extensions/editor/terrainEditor.lua`: real terrain import and block
  handling. The similarly named `editor/api/terrain.lua` contains many TODO stubs
  and must not be mistaken for a complete callable editing API.
- `lua/ge/extensions/editor/riverEditor.lua` and `meshEditor.lua`: river editing.
- `lua/ge/extensions/editor/tech/roadArchitect/tunnelMesh.lua`: procedural tunnel
  creation and terrain-hole edits.
- `lua/ge/extensions/gameplay/tutorial/bounds.lua`: explicit out-of-bounds logic.
- `content/levels/*.zip`: selected scene objects were read without modifying or
  extracting over the game installation.

Official references consulted:

- [Terrain file format and limits](https://documentation.beamng.com/modding/levels/level_formats/terrain/)
- [Terrain, heightmaps, materials, and water](https://documentation.beamng.com/modding/levels/level_creation/section2/)
- [River Editor](https://documentation.beamng.com/world_editor/tools/river_editor/)
- [Roads and drivable surfaces](https://documentation.beamng.com/modding/levels/level_creation/section3/)
- [TSStatic and collision geometry](https://documentation.beamng.com/modding/levels/level_classes/tsstatic/)
- [Level testing and validation](https://documentation.beamng.com/modding/levels/level_creation/section9/)
