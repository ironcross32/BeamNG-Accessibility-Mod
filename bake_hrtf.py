# bake_hrtf.py — Build tool: extract the measurement rings near the horizontal
# plane from each bundled SOFA file into a small compressed .npz, so the shipped
# build does not need h5py + the ~6.5 MB HDF5 native libraries (or the SOFA files,
# which run to tens of MB).
#
# Run with an environment that HAS h5py installed (e.g. the .venv):
#     .venv/Scripts/python bake_hrtf.py            # every source whose .sofa is present
#     .venv/Scripts/python bake_hrtf.py thk_ku100  # just the named set(s)
#
# The .sofa files are gitignored; the .npz outputs are committed. Download URLs are
# below. Licences and attribution: HRTF_DATA_LICENSES.md. The set ids and output
# names must match hrtf_sets.BUNDLED_SETS.
#
# Output keys: azimuths, elevations, irs_left, irs_right (one row per position),
# samplerate. Every ring from hrtf_sets.ELEVATION_MIN_DEG to ELEVATION_MAX_DEG is
# kept, plus the nearest one beyond each end, so any elevation the setting allows
# has a measured ring either side. Angles follow SOFA: degrees, azimuth 0 = front,
# 90 = left, counter-clockwise. The measurements are stored as measured; all the
# processing (ITD split, interpolation, symmetry, equalisation, level matching) is
# hrtf.HRTFSet's, at load time, so it applies equally to any set.

import sys

import h5py
import numpy as np

import hrtf_sets

SOURCES = {
    # MIT Media Lab KEMAR, normal pinna, 5 degree steps, 44.1 kHz.
    # https://sofacoustics.org/data/database/mit/mit_kemar_normal_pinna.sofa
    "mit_kemar": "mit_kemar_normal_pinna.sofa",
    # TH Koeln Neumann KU100, full sphere on a 2 degree grid, 48 kHz. (The
    # same collection's HRIR_CIRC360 is horizontal-only, so it cannot give elevation.)
    # https://zenodo.org/records/3928297/files/HRIR_FULL2DEG.sofa
    "thk_ku100": "HRIR_FULL2DEG.sofa",
    # SADIE II subject D2 (KEMAR), diffuse-field equalised, 48 kHz, 256 taps.
    # https://zenodo.org/records/12092466/files/D2_HRIR_SOFA.zip
    # -> D2_HRIR_SOFA/D2_48K_24bit_256tap_FIR_SOFA.sofa
    "sadie_kemar": "D2_48K_24bit_256tap_FIR_SOFA.sofa",
}

# How far off the horizontal plane a ring may sit and still count as on it.
HORIZONTAL_TOLERANCE_DEG = 1.0

# Fewer points than this at one elevation is a scattered grid, not a ring.
MIN_RING_POINTS = 36


def _spherical_positions(f):
    """SourcePosition as (azimuth deg, elevation deg, distance), converting cartesian files."""
    pos = f["SourcePosition"][:].astype(np.float64)
    kind = f["SourcePosition"].attrs.get("Type", b"spherical")
    if isinstance(kind, bytes):
        kind = kind.decode("ascii", "replace")
    if str(kind).lower().startswith("cartesian"):
        x, y, z = pos[:, 0], pos[:, 1], pos[:, 2]
        dist = np.sqrt(x * x + y * y + z * z)
        az = np.degrees(np.arctan2(y, x))
        el = np.degrees(np.arcsin(np.clip(z / np.maximum(dist, 1e-12), -1.0, 1.0)))
        pos = np.stack([az, el, dist], axis=1)
    return pos


def bake(sofa_path, out_path):
    with h5py.File(sofa_path, "r") as f:
        conventions = f.attrs.get("SOFAConventions", b"")
        if isinstance(conventions, bytes):
            conventions = conventions.decode("ascii", "replace")
        if conventions and conventions != "SimpleFreeFieldHRIR":
            print(f"  warning: {sofa_path} is {conventions}, not SimpleFreeFieldHRIR")
        positions = _spherical_positions(f)       # (M, 3): azimuth, elevation, distance
        irs = f["Data.IR"][:]                     # (M, R, N)
        sr = f["Data.SamplingRate"][()]
        sr = float(np.ravel(sr)[0])

    if irs.shape[1] != 2:
        raise ValueError(f"{sofa_path}: expected 2 receivers (ears), found {irs.shape[1]}")

    ring_of = np.round(positions[:, 1], 1)
    rings, counts = np.unique(ring_of, return_counts=True)
    # A ring is an elevation measured all the way round; a Lebedev-style grid's
    # scattered points are not one.
    rings = rings[counts >= MIN_RING_POINTS]
    kept = _rings_covering(rings, hrtf_sets.ELEVATION_MIN_DEG, hrtf_sets.ELEVATION_MAX_DEG)
    if not np.any(np.abs(kept) < HORIZONTAL_TOLERANCE_DEG):
        raise ValueError(f"{sofa_path}: no complete ring on the horizontal plane")

    all_rows, all_az, all_el, report = [], [], [], []
    for ring_el in kept:
        sel = np.flatnonzero(ring_of == ring_el)
        # One measurement per azimuth: where a grid repeats one, keep the first.
        by_az = {}
        for i in sel:
            by_az.setdefault(round(float(positions[i, 0]) % 360.0, 3), i)
        azimuths = np.array(sorted(by_az), dtype=np.float64)
        widest_gap = float(np.max(np.diff(np.concatenate([azimuths, [azimuths[0] + 360.0]]))))
        if widest_gap > 30.0:
            raise ValueError(
                f"{sofa_path}: a {widest_gap:.0f} degree hole in the {ring_el:g} degree ring"
            )
        all_rows.extend(by_az[a] for a in azimuths)
        all_az.append(azimuths)
        all_el.append(np.full(len(azimuths), float(ring_el)))
        report.append(f"{ring_el:+g} ({len(azimuths)})")

    rows = np.array(all_rows)
    irs_left = irs[rows, 0, :].astype(np.float32)   # receiver 0 = left
    irs_right = irs[rows, 1, :].astype(np.float32)  # receiver 1 = right

    np.savez_compressed(
        out_path,
        azimuths=np.concatenate(all_az),
        elevations=np.concatenate(all_el),
        irs_left=irs_left,
        irs_right=irs_right,
        samplerate=np.float64(sr),
    )
    print(
        f"Wrote {out_path}: rings {', '.join(report)}, "
        f"{irs_left.shape[1]} samples/IR at {int(sr)} Hz"
    )


def _rings_covering(rings, lo, hi):
    """The rings inside [lo, hi], plus the nearest one outside each end, so any
    elevation in range has a measured ring on both sides of it."""
    rings = np.sort(rings)
    keep = (rings >= lo) & (rings <= hi)
    below = rings[rings < lo]
    above = rings[rings > hi]
    # A grid that stops short of lo or hi just keeps what it has; HRTFSet clamps.
    if below.size and not np.any(np.isclose(rings, lo)):
        keep |= rings == below[-1]
    if above.size and not np.any(np.isclose(rings, hi)):
        keep |= rings == above[0]
    return rings[keep]


def main(argv):
    import os

    wanted = argv or list(SOURCES)
    baked = 0
    for set_id in wanted:
        if set_id not in SOURCES:
            print(f"Unknown set '{set_id}'. Known: {', '.join(SOURCES)}")
            return 2
        sofa = SOURCES[set_id]
        out = hrtf_sets.resolve(set_id).filename
        if not os.path.exists(sofa):
            print(f"Skipping {set_id}: {sofa} not found (see the download URL in bake_hrtf.py)")
            continue
        bake(sofa, out)
        baked += 1
    return 0 if baked else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
