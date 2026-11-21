# hrtf_sets.py — the HRTF datasets the user can choose between.
#
# Deliberately numpy-free: configurator.py imports this to fill its dropdown and
# validate the saved choice, and the configurator build does not need the audio stack.
#
# Every set is a pre-baked .npz of the measurement rings near ear level (see
# bake_hrtf.py; hrtf.HRTFSet turns them into one processed ring) shipped next to
# the executable. A future "import your own .sofa" would bake into the user's config
# folder and append its entries in available_sets(); nothing else here would change.

from collections import namedtuple

HRTFSetInfo = namedtuple("HRTFSetInfo", "set_id label filename")

# Order is the dropdown order. Licences and attribution: HRTF_DATA_LICENSES.md.
BUNDLED_SETS = (
    HRTFSetInfo(
        "mit_kemar",
        "MIT KEMAR",
        "hrtf_mit_kemar.npz",
    ),
    HRTFSetInfo(
        "thk_ku100",
        "Neumann KU100 (TH Köln)",
        "hrtf_thk_ku100.npz",
    ),
    HRTFSetInfo(
        "sadie_kemar",
        "KEMAR (SADIE II, University of York)",
        "hrtf_sadie_kemar.npz",
    ),
)

# Unchanged from before the choice existed.
DEFAULT_HRTF_SET = "mit_kemar"

# The elevation every spatialised cue is rendered at, degrees, negative = below
# ear level. The baked sets carry the measured rings from ELEVATION_MIN_DEG to
# ELEVATION_MAX_DEG (plus one either side where the grid needs it to bracket), and
# hrtf.HRTFSet interpolates between them. -3 is roughly the road 20 m ahead from a
# driver's ear; it is well inside the ~10 degree vertical blur of human hearing
# near straight ahead, so expect it to change timbre slightly rather than to sound
# lower. Larger values are there to experiment with.
ELEVATION_MIN_DEG = -20.0
ELEVATION_MAX_DEG = 10.0
DEFAULT_ELEVATION_DEG = -3.0

# Equalise out the tone colour the measurements share in every direction.
DEFAULT_EQUALISE = True


def clamp_elevation(value):
    try:
        value = float(value)
    except (TypeError, ValueError):
        return DEFAULT_ELEVATION_DEG
    if value != value:  # NaN
        return DEFAULT_ELEVATION_DEG
    return max(ELEVATION_MIN_DEG, min(ELEVATION_MAX_DEG, value))


def available_sets():
    """Every selectable set, in display order."""
    return list(BUNDLED_SETS)


def resolve(set_id):
    """The set for set_id, or the default when the id is unknown (a stale or hand-edited config)."""
    sets = available_sets()
    for info in sets:
        if info.set_id == set_id:
            return info
    for info in sets:
        if info.set_id == DEFAULT_HRTF_SET:
            return info
    return sets[0]
