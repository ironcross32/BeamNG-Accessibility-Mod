# Generated Levels: Ice Pool (`tools/mapgen/icepool.py`)

Engine and file-format facts shared by every generated level are in [level-generation.md](level-generation.md).


`python tools/mapgen/icepool.py` builds a second level, `ice_pool` (mod folder
`beam_ice_pool`), reusing `terfile.py`, `textures.py` and `build.py`'s `face()` /
`write_items()` but none of the Proving Grounds layout. An elliptical floor of
ICE, 327 m north-south by 250 m east-west; a 16 m by 287 m strip of water 20 cm
deep down the long axis, ending 20 m short of the wall at each end; and a
two-lane (8 m) ice circuit whose centreline is a concentric ellipse, crossing
those 20 m gaps as its turns. One spawn, in the south-east quadrant halfway
between the water and the east wall, facing north.

- **1 m cells, not 2.5 m.** The map is only 512 m square, and the steepest face
  a heightmap can express is rise per cell, so the finer grid is what lets the
  wall finish near vertical (84 degrees) rather than merely steep.
- **The wall is a function of true distance from the floor ellipse.** Grade
  runs `3 * (d / 30)^3`, so the first 10 m is an apron under 11 % and it curls
  to 72 degrees at 30 m, then a 10-per-metre face to 50 m. Distance is solved
  exactly by iteration; the usual `f / |grad f|` approximation stretches the
  apron at the ends of a 1.3:1 ellipse. Every surface a car can reach is ICE,
  so the apron also slides you back down. The road differs from the floor in
  texture only.
- **The strip's width is set by the TURNS.** A `WaterBlock` is a box, so its
  square corners are what the road has to clear inside the 20 m gap. At 30 m
  wide the road edge passed 1.7 m from them; 16 m gives 4.4 m. The build
  measures both clearances (road to water, road to wall toe) on every run and
  refuses to install if either goes to zero.
- **Time of day is clock-linear**: `time = minutes / 1440 - 0.5` (mod 1), from
  `core/solarTimeOfDay.lua`'s `timeFromMinutes`, so 14:00 is 0.0833.
- **Ground wind cannot live in the scene.** `core_environment` keeps it in a
  module local, zeroes it on `onClientEndMission`, and re-applies it to each
  vehicle on spawn and reset. The generated `mainLevel.lua` calls
  `core_environment.setGroundWind` once the player's vehicle exists. The vector
  is `(speed * sin(heading), speed * cos(heading))` with +X east and +Y north,
  which is how the environment UI itself builds it. The breeze is 4.5 m/s
  blowing TOWARD the north-east. The clouds drift the same way.
- Cloud cover is 2.4 on the UI's 0-3 scale. Shipped levels sit around 1.2.
