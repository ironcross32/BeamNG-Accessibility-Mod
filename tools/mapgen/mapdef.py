"""Layout and terrain synthesis for the Proving Grounds level.

The whole map is a flat plateau at BASE_Z with features CUT INTO or RAISED OUT
of it, which is what keeps it navigable without sight: everything that is not a
named feature is dead level, so any slope you feel is information.

Coordinate system
-----------------
World XY is centred on the origin and spans +/- WORLD_SPAN/2. The terrain block
sits at (-WORLD_SPAN/2, -WORLD_SPAN/2, 0) and covers the whole square. Heights
are stored as uint16 over [0, MAX_HEIGHT], giving MAX_HEIGHT/65535 ~= 32 mm of
vertical resolution.  The deliberately generous range leaves room for future
high terrain while still resolving the smallest features on this map.

Why asphalt everywhere and gravel only at the edges
---------------------------------------------------
GRAVEL has roughnessCoefficient 0.44 against ASPHALT's 0.0, so the tyre noise
changes audibly the moment a wheel crosses between them. That is a free
boundary cue for a blind driver and it costs nothing.

The first version spent that cue on the wrong thing: gravel was the DEFAULT and
asphalt was painted on the roads, so 92 % of the map rumbled and the cue fired
once, on leaving a road, after which everything sounded the same again. Now
asphalt is the driving surface -- the roads and the whole plain between them --
and gravel is a MARGIN, laid only around things there is a reason to stay inside:
each road, the yard, the climb road within its berms, each section, and the map
perimeter. A rumble therefore always means the same thing, "you are leaving the
thing you are on", which is the property that makes it learnable.

A margin is a BAND and not a fill, so crossing it and continuing puts you back on
asphalt. That asymmetry is deliberate: the width of the rumble tells you whether
you clipped an edge or drove out of a section, and a margin that never ended
would be indistinguishable from the old scheme.

Rumble strips mean something ELSE, and that is on purpose
---------------------------------------------------------
Gravel says "you are leaving the thing you are on". A rumble strip marks a
drivable boundary before that margin: the edges of the hill-climb asphalt and
the lane boundaries in the suspension straights. Two cues for two questions
beats one cue used for both -- and the strips are the only surface on the map
that a wheel can be on while still being somewhere legitimate.
"""

import math

import numpy as np

# ---------------------------------------------------------------- world scale
GRID = 2048                 # heightmap edge; must be a power of two, <= 8192
METRES_PER_CELL = 2.5       # TerrainBlock squareSize
WORLD_SPAN = GRID * METRES_PER_CELL          # 5120 m
ORIGIN = -WORLD_SPAN / 2.0                   # -2560
MAX_HEIGHT = 2100.0         # uint16 full scale, in metres
BASE_Z = 50.0               # the plateau. Everything else is relative to this.

# ------------------------------------------------------------------ materials
# Index order IS the .ter material table. Names are TerrainMaterial internalName.
MATERIALS = [
    ("ground_gravel", "GRAVEL"),
    ("road_asphalt", "ASPHALT"),
    ("ground_dirt", "DIRT"),
    ("ground_mud", "MUD"),
    ("ground_rock", "ROCK"),
    ("ground_rumble", "RUMBLE_STRIP"),
]
M_GRAVEL, M_ASPHALT, M_DIRT, M_MUD, M_ROCK, M_RUMBLE = range(6)

# RUMBLE_STRIP is a SOUND AND A SHAKE THE ENGINE SYNTHESISES, not geometry, and
# that is what makes it usable here at all. Its groundmodel carries
# roughnessCoefficient 0, so nothing in the data suggests it does anything -- but
# sounds.lua drives `event:>Surfaces>roll_rumblestrip` from `wd.peakForce` and
# `obj:getPeakPeriod()`, both C++ side, and the shipped strips prove the point:
# hirochi_raceway's is a FLAT decal-road material carrying `groundType:
# "RUMBLE_STRIP"` with no bumps modelled anywhere. (On a mesh material the field
# is `groundType`; on a TerrainMaterial it is `groundmodelName`. Same value.)
#
# That matters because a real rumble strip CANNOT be built out of this
# heightmap -- see SUSP_RIPPLE_MIN_M below. Painting the material is the only
# route to the effect, and it happens to be the correct one.
#
# Appended rather than inserted: index order IS the .ter material table, so a new
# entry at the end leaves every existing index meaning what it meant.

# ------------------------------------------------------------------ hill climb
# grade(s) = G0 + (G1 - G0) * (s/L)^P -- gentle at the bottom, brutal at the top.
# P = 1.5 brings the severe grades forward without losing the progressive run-up:
# 40 percent of the climb is at or above 30 percent grade, versus 32 percent with
# the old P = 2 profile.
CLIMB_X = 0.0
CLIMB_Y0 = -780.0
CLIMB_LEN = 3000.0
CLIMB_G0 = 0.04             # 4 % at the foot
CLIMB_G1 = 0.60             # 60 % (31 deg) at the top
CLIMB_P = 1.5
# CLIMB_HALF_W and BERM_OUTER are deliberately exact multiples of
# METRES_PER_CELL. A threshold that falls mid-cell is resolved by a heightmap
# node that is neither road nor berm, which is precisely how the berm acquired a
# ramp (see below); on the grid, the road edge and the wall foot are the same
# node and there is nothing in between.
CLIMB_HALF_W = 12.5         # 25 m of drivable road; 5 cells either side
BERM_OUTER = 22.5           # flat berm top ends here; the mountain falls away
BERM_H = 12.0               # height of the berm above the road
CLIMB_SHOULDER_W = 5.0      # rumble + gravel inside the berms
CLIMB_RUMBLE_W = 2.5        # one cell between asphalt and the outer gravel
CLIMB_ASPHALT_W = 2.0 * (CLIMB_HALF_W - CLIMB_SHOULDER_W)
CLIMB_ROAD_NODE_STEP = 50.0 # vertical fidelity for the AI/navigation road

# THE BERM HAS NO LATERAL RAMP AT ALL. Every node outside CLIMB_HALF_W stands at
# full BERM_H, so the face is one cell wide by construction and cannot be widened
# by a constant that happens not to be a multiple of the cell size.
#
# That is the third version of this wall, and each failure was less visible than
# the last. The first ramped 3.5 m linearly across the whole 8 m shoulder and
# measured 31-44 % in game -- shallower than the 60 % at the top of the climb it
# was meant to contain. The second raised it to 7 m over "one cell" of lateral
# run, which reads as a wall and was still driven over: with CLIMB_HALF_W at 12.0
# and BERM_RISE at 2.5 the ramp spanned two nodes, and measured mid-climb the
# outer of the two was 84 % -- a launch ramp with a 7 m lip, sitting exactly where
# a car deflected by the 196 % inner face arrives. A wall that is steep for its
# first metre and climbable for its second is not a wall, and the defect is
# invisible in the constants: BERM_RISE was one cell, and it still produced two.
#
# Terrain cannot be vertical -- at METRES_PER_CELL the steepest face expressible
# is BERM_H / METRES_PER_CELL -- so steepness is bought with HEIGHT and with
# nothing else. 12 m over one 2.5 m cell is 480 % (78 deg), and the height is
# what stops a car that gets airborne off the face from clearing the top, which
# 7 m did not.
FLANK_SLOPE = 0.45          # how the mountain falls away outside the berms
SUMMIT_PAD = 90.0           # flat turn-around beyond the top of the climb
PAD_WALL_W = 7.5            # wall band around that turn-around (3 cells)

# ASPHALT's staticFrictionCoefficient is 0.98, so a 60 % grade is comfortably
# inside the traction limit -- which is deliberate. It makes the climb a test of
# POWER (and of the speed carried into it), not a test of grip. Dirt and gravel
# variants belong in later climbs; that is why the profile is parameterised.


def climb_grade(s):
    """Instantaneous grade (rise/run) s metres along the climb."""
    u = np.clip(s, 0.0, CLIMB_LEN) / CLIMB_LEN
    return CLIMB_G0 + (CLIMB_G1 - CLIMB_G0) * u ** CLIMB_P


def climb_height(s):
    """Height above BASE_Z after s metres along the climb (integral of grade)."""
    s = np.clip(s, 0.0, CLIMB_LEN)
    u = s / CLIMB_LEN
    return (CLIMB_G0 * s
            + (CLIMB_G1 - CLIMB_G0) * CLIMB_LEN * u ** (CLIMB_P + 1) / (CLIMB_P + 1))


CLIMB_PROFILE_TOP_Z = BASE_Z + float(climb_height(CLIMB_LEN))
CLIMB_Y1 = CLIMB_Y0 + CLIMB_LEN
SUMMIT_Y = CLIMB_Y1 + SUMMIT_PAD

# The old profile reached its full 60 percent grade at CLIMB_Y1 and then became
# flat in one heightmap cell. Position was continuous but slope was not, so the
# join felt like a hard hump. Preserve the whole 3000 m climb, including its 60
# percent finish, then use the first 45 m of the summit pad as a vertical curve.
CLIMB_CREST_START_S = CLIMB_LEN
CLIMB_CREST_START_Y = CLIMB_Y0 + CLIMB_CREST_START_S
CLIMB_CREST_END_Y = CLIMB_Y1 + SUMMIT_PAD / 2.0
CLIMB_CREST_LENGTH = CLIMB_CREST_END_Y - CLIMB_CREST_START_Y
# A linear reduction in grade from 60 percent to zero gains average-grade times
# distance: 0.30 * 45 = 13.5 m. Raising the final pad is the only way to round
# the crest without weakening the climb or introducing a dip.
CLIMB_TOP_Z = (CLIMB_PROFILE_TOP_Z
               + 0.5 * float(climb_grade(CLIMB_LEN)) * CLIMB_CREST_LENGTH)


def climb_surface_height(world_y):
    """Absolute road height including the smooth transition onto the summit."""
    world_y = np.asarray(world_y, dtype=np.float64)
    s = np.clip(world_y - CLIMB_Y0, 0.0, CLIMB_LEN)
    original = BASE_Z + climb_height(s)

    t = np.clip(
        (world_y - CLIMB_CREST_START_Y) / CLIMB_CREST_LENGTH, 0.0, 1.0)
    h0 = CLIMB_PROFILE_TOP_Z
    h1 = CLIMB_TOP_Z
    tangent0 = float(climb_grade(CLIMB_CREST_START_S)) * CLIMB_CREST_LENGTH
    # Cubic Hermite basis: preserve the incoming grade and end at zero grade.
    curved = ((2.0 * t ** 3 - 3.0 * t ** 2 + 1.0) * h0
              + (t ** 3 - 2.0 * t ** 2 + t) * tangent0
              + (-2.0 * t ** 3 + 3.0 * t ** 2) * h1)
    return np.where(world_y >= CLIMB_CREST_START_Y, curved, original)

# --------------------------------------------------------------------- layout
# Each of these is a POI on the big map, in the order a driver would take them.
STAGING = (0.0, -2200.0)
MUD_BASIN = (-1500.0, -900.0)
FORD = (1300.0, -900.0)
SUMP = (1500.0, 340.0)
CLIMB_BASE = (CLIMB_X, CLIMB_Y0 - 40.0)
CLIMB_SUMMIT = (CLIMB_X, (CLIMB_Y1 + SUMMIT_Y) / 2.0)

# -------------------------------------------------------------- 1 km tunnel
# A straight, level tunnel on the otherwise unused north-west plateau.  Its
# shell is assembled from the game's 64 m Utah tunnel module by build.py; the
# 16 modules are shortened uniformly to 62.5 m so the portals are exactly one
# kilometre apart rather than the tempting-but-wrong 1024 m.
TUNNEL_X = -2000.0
TUNNEL_Y0 = 500.0
TUNNEL_LENGTH = 1000.0
TUNNEL_Y1 = TUNNEL_Y0 + TUNNEL_LENGTH
TUNNEL_SEGMENTS = 16
TUNNEL_SEGMENT_LENGTH = TUNNEL_LENGTH / TUNNEL_SEGMENTS
TUNNEL_MODULE_LENGTH = 64.0
TUNNEL_SEGMENT_SCALE_Y = TUNNEL_SEGMENT_LENGTH / TUNNEL_MODULE_LENGTH
# The module's measured pavement is 0.20 m above its object origin. Sink the
# shell by that amount so both portals meet the plateau without a square-edged
# 20 cm lip.
TUNNEL_MODULE_ROAD_Z = 0.20
TUNNEL_ROAD_HW = 4.5
TUNNEL_APPROACH = 60.0
TUNNEL_SPAWN = (TUNNEL_X, TUNNEL_Y0 - 40.0)

WATER_SURFACE_Z = BASE_Z            # both pools fill exactly to the plateau
FORD_DEPTH = 0.45                   # drive-through
SUMP_DEPTH = 4.5                    # engine hydrolocks: see combustionEngine.lua

ROAD_HW = 10.0                      # spine road half-width
BRANCH_HW = 8.0
YARD_HW = 110.0

PERIMETER_INSET = 70.0              # wall around the map edge so you cannot leave
PERIMETER_H = 9.0

SPINE_Y0 = -2320.0

# --------------------------------------------------------------- sound stage
# A flat asphalt pad east of the Staging Yard holding three of the game's own
# test rigs, for listening to an engine work rather than for driving anywhere.
#
# NONE OF THIS CAN BE TERRAIN, and that is worth stating because it is the first
# thing anyone would try. A .ter is a heightmap plus one material byte per cell,
# and nothing in the ground model catalogue moves -- groundmodelName picks
# friction, roughness and tyre sound and never motion. A rolling surface cannot
# be a surface at all; it has to be an object. All three rigs below are
# `"Type": "Prop"` vehicles that ship with BeamNG, spawned on mission start by
# the level's own mainLevel.lua (build.py writes it).
#
# So the pad itself is a MATERIAL feature only -- the plain here is already dead
# level asphalt, which is exactly what a rig wants -- and the only thing actually
# built is the gravel margin. That is the point of it: the margin makes the stage
# findable and bounded by ear, under the same rule as everything else on this map
# ("you are leaving the thing you are on").
SOUND_STAGE = (400.0, -2200.0)
STAGE_HW = 150.0            # pad half-width; rigs sit 100 m apart inside it
STAGE_HH = 110.0
STAGE_RIG_Y = SOUND_STAGE[1]
# How far SOUTH of its rig each POI puts you down. MUST stay inside rampGeometry's
# RAMP_SEARCH_M (70 m), which is the radius the docking instrument will even look in --
# at the first value tried, 90 m, rampTruth() answered "no ramp among 0 objects within
# 70 metres" while parked in clear sight of all three rigs, which reads exactly like an
# instrument that does not support them. Still a real run-up, and well clear of the
# hamster wheel frame's own 12.7 m half-length.
STAGE_APPROACH_M = 45.0

# EVERY RIG ON THIS PAD IS ENTERED HEADING NORTH, and that single rule is why the
# facings below are all different. A prop's facing is where its own +Y points,
# which is NOT the direction you drive into it -- for two of these three it is
# not even parallel to it, and each was read out of the shipped jbeam rather than
# guessed:
#
#   Wheel Roller   autoAdjust.lua sizes the rig to the nearest vehicle's track
#                  width (local X) and wheelbase (local Y), so its +Y MUST lie
#                  along the car's length. Facing north is the one that matters.
#   Hamster Wheel  the drum's circle is in the local Y-Z plane (radius 7.17 m)
#                  and its axle runs along local X, with a drive-in ramp at each
#                  END of that axle -- so you enter ACROSS the drum and then turn
#                  90 degrees to drive around the inside of it. Faced east so the
#                  two ramps face north and south; you drive straight up the
#                  south one, then turn either way to spin it.
#   Tilt Ramp      faced NORTH, and the reason is a trap this project has hit before.
#                  The jbeam names its node groups -- `onramp_R`/`onramp_L` at local
#                  +Y, `offramp_*` at local -Y -- which invites the reading that +Y is
#                  the front and the prop should be turned around to be entered
#                  northbound. It is not: `refNodes` declares `back:` as `rb3r` at
#                  local y +8.41, so +Y is the prop's REAR and getDirectionVector()
#                  points along local -Y. The onramp is at the BACK. Exactly the
#                  inversion large_cannon carries (its refNodes declare back toward +y
#                  while the muzzle is at +y), and exactly as invisible in the jbeam.
#                  Settled by measurement, not reasoning: faced south, rampGeometry
#                  resolved the mouth axis as (0, -1) -- entered heading SOUTH, against
#                  this pad's whole rule -- and faced north it is (0, +1).
#
# A rig's spawn position is its REFERENCE NODE, which is not its centre. Measured in game, the
# hamster wheel's drum sits 7.22 m from the node the spawner places -- so spawning it at x = 400
# put the drum, both its ramps and therefore the resolved mouth at x = 392.78, while the POI kept
# dropping the driver at 400. The mouth is only 6.74 m wide, so that is the driver arriving 7.2 m
# to one side of a ramp they were told they were lined up on: exactly the failure the POI exists
# to prevent. FWD_OFFSET_M cancels it, applied ALONG THE PROP'S OWN FACING rather than in world
# X, so the correction survives someone turning the rig round.
#
# (key, label, model, config, x offset, prop facing, forward offset, icon, blurb)
SOUND_STAGE_RIGS = [
    ("roller", "Wheel Roller", "testroller", "multi", -100.0, (0.0, 1.0), 0.0,
     "carToWheels",
     "Rolling road. Drive on heading north and the wheels spin in frictionless "
     "cradles while the car stays put. It resizes itself to your car once a "
     "second, so just park near it. Note there is almost no LOAD once the wheels "
     "are up to speed, so this is a free rev at an indicated speed, not the "
     "sound of an engine pulling."),
    ("hamster", "Hamster Wheel", "large_hamster_wheel", "unpowered", 0.0, (1.0, 0.0), 7.22,
     "d_rotation",
     "A 14 metre drum on an axle, spun by driving inside it. Drive north up the "
     "ramp, then turn east or west and drive. The only rig here that loads the "
     "engine properly, because you are climbing the inside of it and never get "
     "to the top."),
    ("tiltramp", "Tilt Ramp", "testroller", "ramp", 100.0, (0.0, 1.0), 0.0,
     "AWD",
     "Adjustable tilting roller ramp, the traction demo rig. Drive on heading "
     "north; one front wheel and the opposite rear wheel end up on rollers."),
]


def grid_axes():
    """World X and Y for every heightmap NODE, as (rows=Y, cols=X) arrays.

    Sample i is at `TerrainBlock.position + i * squareSize` -- a corner grid, not
    cell centres. The first version added the half-cell (`+ 0.5`) a raster image
    would want, which put the whole world 1.25 m out of position against its own
    layout constants and, worse, landed a heightmap node halfway up the berm face
    where the design intended a single vertical step. Measured in game: the ford's
    hard edge, specified at x = 1450, resolved at 1447.5.
    """
    c = ORIGIN + np.arange(GRID) * METRES_PER_CELL
    return c[:, None], c[None, :]


def _rect(x, y, cx, cy, hw, hh):
    return (np.abs(x - cx) <= hw) & (np.abs(y - cy) <= hh)


def _disc(x, y, cx, cy, r):
    return ((x - cx) ** 2 + (y - cy) ** 2) <= r * r


MUD_PITS = [
    (-1610.0, -1010.0, 70.0, 0.55),
    (-1420.0, -940.0, 52.0, 0.70),
    (-1530.0, -790.0, 60.0, 0.60),
    (-1660.0, -860.0, 44.0, 0.45),
    (-1380.0, -1080.0, 48.0, 0.65),
]

FORD_HW, FORD_HH = 150.0, 45.0
SUMP_R = 120.0
MUD_APRON_R = 300.0

# ------------------------------------------- suspension straights (washboard)
# Three parallel lanes running due NORTH, each undulating harder than the last,
# separated -- and bounded on the outside -- by rumble strips.
#
# Cardinal on purpose. Every other long feature on this map runs north (the
# spine road, the climb), so "drive north" is the one heading that needs no
# reference, and a lane you can only stay in by holding a heading is not a
# suspension test.
#
# WHAT THE HEIGHTMAP CAN AND CANNOT EXPRESS. At METRES_PER_CELL of 2.5 the
# Nyquist limit is a 5 m wavelength, and 4 samples per cycle (10 m) is the
# shortest that still reconstructs as a wave rather than as a triangle of
# arbitrary amplitude. So a TRUE washboard -- the 0.3 to 1 m ripple that makes a
# gravel road roar -- is not buildable here at any amplitude, and pretending
# otherwise would produce a lane whose profile depends on where its crests
# happened to land between nodes. What IS buildable is the whoop/swell band,
# 10 m and up, which at 50 to 100 km/h excites 1.4 to 2.8 Hz: body heave and
# pitch, which is where dampers actually work. The rumble strips supply the
# high-frequency end instead, and they can, because the engine synthesises that
# from the ground model rather than from geometry.
SUSP_RIPPLE_MIN_M = 4 * METRES_PER_CELL     # 10 m; shortest honest wavelength
#
# EVERY WAVELENGTH IS AN EXACT MULTIPLE OF METRES_PER_CELL, and that is not
# tidiness. A wavelength that does not divide by the cell size beats against the
# node grid: the sampled crest drifts in and out of phase, so amplitude varies by
# up to 30 % along the lane and no two cycles are the same bump. A lane whose
# severity changes along its own length cannot be compared with the lane beside
# it, which is the entire point of having three. Locked to the grid, every cycle
# is sampled identically and the lane is repeatable end to end.
SUSP_X = -1200.0            # section centre line, well west of the spine road
SUSP_Y0 = 300.0             # undulation starts here
SUSP_LEN = 1000.0
SUSP_Y1 = SUSP_Y0 + SUSP_LEN
SUSP_APRON_LEN = 120.0      # flat run-up at the south end, in front of the lanes
SUSP_LANE_HW = 7.5          # 15 m lane: 3 cells either side of its centre
SUSP_SEP_W = 5.0            # rumble strip width: 2 cells
SUSP_PITCH = 2 * SUSP_LANE_HW + SUSP_SEP_W   # 20 m between lane centres
SUSP_HALF_W = SUSP_PITCH + SUSP_LANE_HW + SUSP_SEP_W   # 32.5 m
SUSP_TAPER_M = 50.0         # amplitude fades in and out over this

# (lateral offset, name, swell wavelength/amplitude, ripple wavelength/amplitude)
# Two components per lane on purpose. The long swell is a body-control test; the
# short ripple is a wheel-control test; a single sine is only one of those and
# would let a car that fails the other pass. Harshness is raised by BOTH
# shortening the wavelengths and growing the amplitudes, because it is their
# ratio -- the slope -- that a suspension actually feels.
# NO LANE'S SWELL WAVELENGTH IS AN INTEGER MULTIPLE OF ITS RIPPLE'S. That locks
# the two components into a fixed phase relationship for the whole lane, and
# which relationship you get is luck: the gentle lane was first specified at
# 60 m over 20 m, exactly 3:1, which puts the ripple at its trough on every swell
# crest and cancelled 29 % of the amplitude -- 0.33 m peak-to-peak against the
# 0.46 m the two amplitudes were chosen to give. That is invisible in the
# constants and reads exactly like the node grid having eaten the wave, which it
# had not: the continuous profile measures 0.326 too. 55 m over 20 m is 2.75:1,
# still both exact cell multiples, and the pattern repeats every 220 m.
SUSP_LANES = [
    (-SUSP_PITCH, "gentle", 55.0, 0.18, 20.0, 0.05),
    (0.0, "medium", 40.0, 0.35, 15.0, 0.11),
    (SUSP_PITCH, "harsh", 25.0, 0.60, 10.0, 0.20),
]

# The gentlest lane is the WEST one, so the three read left-to-right in
# increasing severity from the spawn -- and the spawn sits in front of it.
SUSP_START = (SUSP_X + SUSP_LANES[0][0], SUSP_Y0 - 60.0)


def susp_profile(s, swell_l, swell_a, rip_l, rip_a):
    """Height above BASE_Z, s metres into a lane.

    Windowed with a raised cosine at both ends so the lane starts and finishes at
    zero height AND zero slope. Without it the entry is a step: a sine is at its
    steepest as it crosses zero, so an untapered lane would begin with its worst
    gradient at the exact moment the car arrives at speed.
    """
    u = np.clip(s / SUSP_TAPER_M, 0.0, 1.0)
    v = np.clip((SUSP_LEN - s) / SUSP_TAPER_M, 0.0, 1.0)
    window = 0.25 * (1.0 - np.cos(np.pi * u)) * (1.0 - np.cos(np.pi * v))
    return window * (swell_a * np.sin(2.0 * np.pi * s / swell_l)
                     + rip_a * np.sin(2.0 * np.pi * s / rip_l))


def _cells(x, lo, hi):
    """Half-open [lo, hi) -- the mask to use when painting MATERIAL, not height.

    The two layers of a .ter are not the same kind of grid. The heightmap is a
    grid of NODES (sample i sits exactly at position + i * squareSize), so an
    inclusive mask is right for it: both edge nodes belong to the thing. The
    material layer is a raster of CELLS, one per square, so an inclusive mask
    claims an extra column at each end and neighbouring regions fight over the
    boundary -- whoever paints last wins one cell of the other.

    That is not cosmetic here. With inclusive masks a lane took the node at
    exactly cx +/- LANE_HW at BOTH ends, so the 5 m gap between two lanes was
    left holding a SINGLE cell of rumble strip while the outer strips, with no
    lane on the far side to eat into them, kept both of theirs. The dividers
    would have been 2.5 m in the middle of the section and 5 m at its edges --
    two different cues for one boundary, on the one feature whose whole job is
    to tell you which lane you are in.
    """
    return (x >= lo) & (x < hi)


def _lane_weight(x, cx):
    """1.0 across a lane, falling LINEARLY to 0 over SUSP_SEP_W beyond it.

    The lanes have different profiles, so their edges sit at different heights --
    up to a metre apart on the harsh lane. Left as a step that is a kerb you hit
    sideways at speed. Because the falloff width equals the separator width, two
    neighbouring weights sum to exactly 1 across the strip between them, so the
    rumble strip IS the linear interpolation from one lane to the next: banked,
    continuous, and telling you which way you drifted.
    """
    return np.clip((SUSP_LANE_HW + SUSP_SEP_W - np.abs(x - cx)) / SUSP_SEP_W,
                   0.0, 1.0)


# ------------------------------------------------------------------- margins
# Width of the gravel band laid around a thing worth staying inside. Wide enough
# that a wheel cannot cross it between physics ticks at any speed this map can
# produce, and wide enough to be several cells (METRES_PER_CELL is 2.5) so it
# survives the heightmap quantisation the berm was caught by.
MARGIN_W = 10.0
PERIMETER_MARGIN_W = 25.0   # wider: there is no road here, only the map edge


def _band(inner, outer):
    """The margin between two masks: inside the grown one, outside the tight one."""
    return outer & ~inner


def build():
    """Return (height_metres float32 [GRID, GRID], material_idx uint8 [GRID, GRID])."""
    y, x = grid_axes()
    z = np.full((GRID, GRID), BASE_Z, dtype=np.float64)
    mat = np.full((GRID, GRID), M_ASPHALT, dtype=np.uint8)

    # ------------------------------------------------------------ hill climb
    road_z = np.broadcast_to(climb_surface_height(y), (GRID, GRID))

    ax = np.abs(x - CLIMB_X)
    d_lon = np.maximum(0.0, np.maximum(CLIMB_Y0 - y, y - SUMMIT_Y))
    d_lat = np.maximum(0.0, ax - BERM_OUTER)
    d_out = np.hypot(d_lat, d_lon)

    crest = road_z + np.where(ax <= CLIMB_HALF_W, 0.0, BERM_H)
    # Beyond the berm the mountain falls away to the plateau. max() rather than
    # assignment so the hill can never cut a trench into the surrounding plain.
    z = np.maximum(z, np.maximum(BASE_Z, crest - FLANK_SLOPE * d_out))

    onClimb = (y >= CLIMB_Y0 - 60.0) & (y <= SUMMIT_Y)
    corridor = (ax <= BERM_OUTER) & onClimb
    mat[corridor] = M_ROCK
    # Preserve the existing asphalt width and split its 5 m shoulder into a
    # one-cell rumble strip followed by one cell of gravel on each side. These
    # are material CELLS, so use half-open masks: the five bands then meet
    # exactly without one side stealing a boundary cell from its neighbour.
    # The rumble gives an immediate lane-edge cue; gravel says the car continued
    # outward; the rock berm is the structural backstop.
    asphalt_hw = CLIMB_HALF_W - CLIMB_SHOULDER_W
    rumble_outer = asphalt_hw + CLIMB_RUMBLE_W
    climb_gravel = _cells(x, CLIMB_X - CLIMB_HALF_W,
                          CLIMB_X + CLIMB_HALF_W) & onClimb
    climb_rumble = (
        _cells(x, CLIMB_X - rumble_outer, CLIMB_X - asphalt_hw)
        | _cells(x, CLIMB_X + asphalt_hw, CLIMB_X + rumble_outer)
    ) & onClimb
    climb_asphalt = _cells(x, CLIMB_X - asphalt_hw,
                           CLIMB_X + asphalt_hw) & onClimb
    mat[climb_gravel] = M_GRAVEL
    mat[climb_rumble] = M_RUMBLE
    mat[climb_asphalt] = M_ASPHALT

    # The summit pad is a WALLED BOWL with one gap, not an open platform. The
    # pad is 90 m across and the berms are only 45 m apart, so assigning the pad
    # height erases them -- which left a flat asphalt square 340 m above the
    # plain, unfenced on three sides, at the one place on the map where a car
    # arrives at full throttle and has to stop. The wall is the same one-cell
    # BERM_H step as the corridor, and the gap is exactly the road.
    pad_hw = SUMMIT_PAD / 2.0
    pad = _rect(x, y, CLIMB_X, CLIMB_SUMMIT[1], pad_hw, pad_hw)
    ring = _rect(x, y, CLIMB_X, CLIMB_SUMMIT[1],
                 pad_hw + PAD_WALL_W, pad_hw + PAD_WALL_W)
    entry = (ax <= CLIMB_HALF_W) & (y < CLIMB_SUMMIT[1])
    # The south half of the pad carries the end of the vertical curve; the
    # north half is flat. Assigning the whole pad to CLIMB_TOP_Z created the
    # abrupt slope break the driver felt at the top.
    z[pad] = road_z[pad]
    mat[pad] = M_ASPHALT
    wall = ring & ~pad & ~entry
    z[wall] = road_z[wall] + BERM_H
    mat[wall] = M_ROCK

    # --------------------------------------------------------- road network
    spine = _rect(x, y, 0.0, (SPINE_Y0 + CLIMB_Y0) / 2.0,
                  ROAD_HW, (CLIMB_Y0 - SPINE_Y0) / 2.0)
    west = _rect(x, y, MUD_BASIN[0] / 2.0, -900.0, abs(MUD_BASIN[0]) / 2.0, BRANCH_HW)
    east = _rect(x, y, SUMP[0] / 2.0, -900.0, SUMP[0] / 2.0, BRANCH_HW)
    spur = _rect(x, y, SUMP[0], (-900.0 + SUMP[1]) / 2.0, BRANCH_HW, (SUMP[1] + 900.0) / 2.0)
    yard = _rect(x, y, STAGING[0], STAGING[1], YARD_HW, YARD_HW)

    # The verge is computed against the UNION of the network, never per road, or
    # every junction gets a gravel bar straight across it -- each road would lay
    # its margin over its neighbour's carriageway. Grow all of them, subtract all
    # of them, and the band survives only where nothing else is a road.
    def grown(w):
        return (_rect(x, y, 0.0, (SPINE_Y0 + CLIMB_Y0) / 2.0,
                      ROAD_HW + w, (CLIMB_Y0 - SPINE_Y0) / 2.0 + w)
                | _rect(x, y, MUD_BASIN[0] / 2.0, -900.0,
                        abs(MUD_BASIN[0]) / 2.0 + w, BRANCH_HW + w)
                | _rect(x, y, SUMP[0] / 2.0, -900.0, SUMP[0] / 2.0 + w, BRANCH_HW + w)
                | _rect(x, y, SUMP[0], (-900.0 + SUMP[1]) / 2.0,
                        BRANCH_HW + w, (SUMP[1] + 900.0) / 2.0 + w)
                | _rect(x, y, STAGING[0], STAGING[1], YARD_HW + w, YARD_HW + w))

    network = spine | west | east | spur | yard
    mat[_band(network, grown(MARGIN_W))] = M_GRAVEL
    mat[network] = M_ASPHALT

    # The road-network verge extends MARGIN_W beyond the north end of the spine,
    # across the foot of the climb. Restore the climb cross-section over that
    # overlap so its rumble strips start with the incline and remain continuous
    # all the way to the summit pad.
    climb_run = (y >= CLIMB_Y0) & (y < CLIMB_Y1)
    mat[climb_gravel & climb_run] = M_GRAVEL
    mat[climb_rumble & climb_run] = M_RUMBLE
    mat[climb_asphalt & climb_run] = M_ASPHALT

    # ----------------------------------------------------------- sound stage
    # No height change at all: the plain is already BASE_Z asphalt and a rig
    # wants exactly that. The gravel band is the whole feature -- it is what
    # makes the pad findable and bounded without sight. Painted after the road
    # network so its margin cannot be eaten by the verge computation, and it
    # overlaps nothing later (mud, ford, sump, straights, perimeter are all
    # elsewhere), so nothing overwrites it either.
    stage = _rect(x, y, SOUND_STAGE[0], SOUND_STAGE[1], STAGE_HW, STAGE_HH)
    mat[_band(stage, _rect(x, y, SOUND_STAGE[0], SOUND_STAGE[1],
                           STAGE_HW + MARGIN_W, STAGE_HH + MARGIN_W))] = M_GRAVEL
    mat[stage] = M_ASPHALT

    # --------------------------------------------------------- 1 km tunnel
    # The tunnel shell is a scene object, but its floor is still terrain.  Give
    # the approaches a gravel verge and keep the entire opening asphalt so the
    # transition into the stock tunnel module has no material or height seam.
    tunnel_road = _rect(
        x, y, TUNNEL_X, (TUNNEL_Y0 + TUNNEL_Y1) / 2.0,
        TUNNEL_ROAD_HW, TUNNEL_LENGTH / 2.0 + TUNNEL_APPROACH)
    tunnel_outer = _rect(
        x, y, TUNNEL_X, (TUNNEL_Y0 + TUNNEL_Y1) / 2.0,
        TUNNEL_ROAD_HW + MARGIN_W,
        TUNNEL_LENGTH / 2.0 + TUNNEL_APPROACH + MARGIN_W)
    mat[_band(tunnel_road, tunnel_outer)] = M_GRAVEL
    mat[tunnel_road] = M_ASPHALT

    # ------------------------------------------------------------ mud basin
    apron = _disc(x, y, MUD_BASIN[0], MUD_BASIN[1], MUD_APRON_R)
    mat[_band(apron, _disc(x, y, MUD_BASIN[0], MUD_BASIN[1],
                           MUD_APRON_R + MARGIN_W))] = M_GRAVEL
    mat[apron] = M_DIRT
    # Pits are shallow on purpose: MUD's groundmodel supplies the drag (fluid
    # density 7000, shear strength 4000), so the depression only has to hold you
    # in it. A deep hole would just be a hole, and a trap rather than a section.
    for px, py, pr, pd in MUD_PITS:
        d = np.sqrt((x - px) ** 2 + (y - py) ** 2)
        inside = d <= pr
        z -= np.where(inside, pd * (1.0 - (d / pr) ** 2), 0.0)
        mat[inside] = M_MUD

    # ------------------------------------------------- shallow ford, the sump
    ford = _rect(x, y, FORD[0], FORD[1], FORD_HW, FORD_HH)
    mat[_band(ford, _rect(x, y, FORD[0], FORD[1],
                          FORD_HW + MARGIN_W, FORD_HH + MARGIN_W))] = M_GRAVEL
    z[ford] = BASE_Z - FORD_DEPTH
    mat[ford] = M_DIRT

    d = np.sqrt((x - SUMP[0]) ** 2 + (y - SUMP[1]) ** 2)
    sump = d <= SUMP_R
    mat[_band(sump, d <= SUMP_R + MARGIN_W)] = M_GRAVEL
    # Dished, so the entry is a ramp you commit to rather than a ledge you fall
    # off. Drowning the engine should be a decision, not an ambush.
    z -= np.where(sump, SUMP_DEPTH * np.clip(1.0 - (d / SUMP_R) ** 2, 0.0, 1.0), 0.0)
    mat[sump] = M_DIRT

    # ----------------------------------------------- suspension straights
    # The gravel margin goes down FIRST and is then overwritten wherever the
    # section itself paints, which is the same order every other feature uses.
    susp_cy = (SUSP_Y0 - SUSP_APRON_LEN + SUSP_Y1) / 2.0
    susp_hh = (SUSP_LEN + SUSP_APRON_LEN) / 2.0
    whole = _rect(x, y, SUSP_X, susp_cy, SUSP_HALF_W, susp_hh)
    # Half-open in X for every MATERIAL mask here, the margin included. Mixing
    # the two conventions leaves a one-cell sliver of whatever was underneath
    # between the outer rumble strip and its gravel margin -- present on one
    # flank only, because the inclusive and half-open edges disagree at just one
    # end. Two-and-a-half metres of stray asphalt at the section boundary reads
    # from the seat as the strip having a gap in it.
    cells = _cells(x, SUSP_X - SUSP_HALF_W, SUSP_X + SUSP_HALF_W) & whole
    grown = (_cells(x, SUSP_X - SUSP_HALF_W - MARGIN_W,
                    SUSP_X + SUSP_HALF_W + MARGIN_W)
             & _rect(x, y, SUSP_X, susp_cy, SUSP_HALF_W + MARGIN_W,
                     susp_hh + MARGIN_W))
    mat[_band(cells, grown)] = M_GRAVEL

    s_along = np.clip(y - SUSP_Y0, 0.0, SUSP_LEN)
    undulating = whole & (y >= SUSP_Y0)
    apron = whole & (y < SUSP_Y0)

    dz = np.zeros((GRID, GRID), dtype=np.float64)
    for off, _name, swl, swa, ripl, ripa in SUSP_LANES:
        dz += _lane_weight(x, SUSP_X + off) * susp_profile(s_along, swl, swa,
                                                           ripl, ripa)
    z = np.where(undulating, z + dz, z)

    # Rumble first, lanes over the top: the strips are simply what is left
    # between them, including the two outer edges. Bounding the outermost lanes
    # as well as separating them is what makes a lane identifiable by feel --
    # drift either way and you are told, rather than only drifting inwards.
    mat[cells] = M_RUMBLE
    for off, _name, _swl, _swa, _ripl, _ripa in SUSP_LANES:
        lane = _cells(x, SUSP_X + off - SUSP_LANE_HW,
                      SUSP_X + off + SUSP_LANE_HW) & cells
        # The strips run through the APRON too, so the lane you want can be
        # found and lined up on while stationary -- count the strips across.
        # An apron that was plain asphalt would make you start the test to
        # discover which lane you were in.
        mat[lane & apron] = M_ASPHALT
        mat[lane & undulating] = M_DIRT

    # -------------------------------------------------------- perimeter wall
    edge = ((np.abs(x) > WORLD_SPAN / 2.0 - PERIMETER_INSET)
            | (np.abs(y) > WORLD_SPAN / 2.0 - PERIMETER_INSET))
    warn = ((np.abs(x) > WORLD_SPAN / 2.0 - PERIMETER_INSET - PERIMETER_MARGIN_W)
            | (np.abs(y) > WORLD_SPAN / 2.0 - PERIMETER_INSET - PERIMETER_MARGIN_W))
    mat[_band(edge, warn)] = M_GRAVEL
    z = np.where(edge, np.maximum(z, BASE_Z + PERIMETER_H), z)
    mat[edge] = M_ROCK

    return z.astype(np.float32), mat


# ------------------------------------------------------- fluid depth per surface
# Metres of FLUID above the solid floor, per material index. This is not
# cosmetic: without the depth map beside the .ter the engine logs
#   BeamNGCollision::addTerrainBlock| depth image map not existing: ...
#   all fluids will be disabled as ground type
# and MUD's whole reason for being here -- fluidDensity 7000, shearStrength 4000
# -- is inert. The pits would still be shallow depressions with high friction,
# which is a section that looks right and does not behave like mud at all.
#
# CALIBRATION IS INFERRED, NOT PROVEN. The map is 8-bit greyscale at the
# heightmap's own resolution; 255 is solid at the surface and lower is deeper.
# Every shipped level agrees on the top of the range and lands close to the same
# floor -- derby 225, east_coast_usa 225, small_island 226, Utah 240 -- which is
# 30 steps of usable range, and Utah's floor of exactly 240 is exactly MUD's own
# groundmodel `defaultDepth` of 0.15 m at one centimetre per step. That is the
# reading used here. If it is wrong it is wrong in scale only: any value below
# 255 enables the fluid, so the pits behave like mud either way.
FLUID_DEPTH_M = {
    M_MUD: 0.30,        # deepest any shipped level goes
    M_DIRT: 0.05,       # a skim of loose material
}
DEPTH_SOLID = 255
DEPTH_CM_PER_STEP = 0.01


def fluid_depth_map(mat):
    """The .ter.depth.png payload: uint8, 255 = solid floor at the surface."""
    depth = np.full(mat.shape, DEPTH_SOLID, dtype=np.int32)
    for idx, metres in FLUID_DEPTH_M.items():
        steps = int(round(metres / DEPTH_CM_PER_STEP))
        depth[mat == idx] = DEPTH_SOLID - steps
    return np.clip(depth, 0, 255).astype(np.uint8)


def to_uint16(z):
    """Quantise metres to the .ter uint16 range."""
    raw = np.clip(np.asarray(z, dtype=np.float64) / MAX_HEIGHT, 0.0, 1.0) * 65535.0
    return np.round(raw).astype("<u2")


def describe_susp(z=None):
    """Per-lane profile, measured off the SAMPLED heightmap when one is given.

    Given z, the peak-to-peak and steepest gradient are read back out of the
    array the game will load, after the node grid and the uint16 quantisation
    have both had their say -- so a wave that did not survive the grid shows up
    here as a lane that is flatter than it was specified to be, rather than as a
    disappointment from the driving seat.
    """
    rows = []
    for off, name, swl, swa, ripl, ripa in SUSP_LANES:
        # The target is the CONTINUOUS profile, densely sampled -- not
        # 2 * (swa + ripa), which assumes the two crests coincide and they
        # generally do not. With the naive bound the comparison tests this
        # function's arithmetic instead of testing whether the wave survived the
        # node grid, which is the only thing it exists to find out.
        fine = np.arange(0.0, SUSP_LEN, 0.05)
        ideal = (swa * np.sin(2.0 * np.pi * fine / swl)
                 + ripa * np.sin(2.0 * np.pi * fine / ripl))
        want_pp = float(ideal.max() - ideal.min())
        if z is None:
            rows.append((name, swl, swa, ripl, ripa, want_pp, None, None))
            continue
        col = int(round((SUSP_X + off - ORIGIN) / METRES_PER_CELL))
        r0 = int(round((SUSP_Y0 - ORIGIN) / METRES_PER_CELL))
        r1 = int(round((SUSP_Y1 - ORIGIN) / METRES_PER_CELL))
        lane = np.asarray(z[r0:r1 + 1, col], dtype=np.float64)
        got_pp = float(lane.max() - lane.min())
        grade = float(np.abs(np.diff(lane)).max() / METRES_PER_CELL)
        rows.append((name, swl, swa, ripl, ripa, want_pp, got_pp, grade))
    return rows


def describe_climb():
    """Human-readable profile, for the build log and for tuning."""
    rows = []
    for f in (0.0, 0.25, 0.5, 0.75, 1.0):
        s = f * CLIMB_LEN
        g = float(climb_grade(s))
        rows.append((s, g * 100.0, math.degrees(math.atan(g)),
                     BASE_Z + float(climb_height(s))))
    return rows
