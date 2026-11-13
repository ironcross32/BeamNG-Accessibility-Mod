"""Golden cases for beam_base::hrtf_sets, from hrtf_sets.py."""

from __future__ import annotations

from _common import fbits, write_fixture

import hrtf_sets


def main():
    nan, inf = float("nan"), float("inf")
    values = [
        -3.0, 0, 5, -20, 10, -20.0001, 10.0001, -100, 100, -0.0, 0.5, True, False, None,
        nan, inf, -inf, "4.5", " -7 ", "abc", "", "nan", "inf", "-inf", "1_0", [], {},
        [1], 2 ** 53, -(2 ** 53),
    ]
    elevations = []
    for v in values:
        elevations.append({"value": fbits(v) if isinstance(v, float) else v,
                           "is_float": isinstance(v, float),
                           "out": fbits(hrtf_sets.clamp_elevation(v))})

    resolve = []
    for sid in ["mit_kemar", "thk_ku100", "sadie_kemar", "", "MIT_KEMAR", "unknown",
                "mit_kemar ", "hrtf_mit_kemar.npz"]:
        info = hrtf_sets.resolve(sid)
        resolve.append({"set_id": sid, "out": info.set_id})

    write_fixture("hrtf_sets", {
        "bundled": [list(info) for info in hrtf_sets.BUNDLED_SETS],
        "available": [list(info) for info in hrtf_sets.available_sets()],
        "DEFAULT_HRTF_SET": hrtf_sets.DEFAULT_HRTF_SET,
        "ELEVATION_MIN_DEG": fbits(hrtf_sets.ELEVATION_MIN_DEG),
        "ELEVATION_MAX_DEG": fbits(hrtf_sets.ELEVATION_MAX_DEG),
        "DEFAULT_ELEVATION_DEG": fbits(hrtf_sets.DEFAULT_ELEVATION_DEG),
        "DEFAULT_EQUALISE": hrtf_sets.DEFAULT_EQUALISE,
        "clamp_elevation": elevations,
        "resolve": resolve,
    })


if __name__ == "__main__":
    main()
