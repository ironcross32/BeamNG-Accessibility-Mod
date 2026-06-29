# bake_hrtf.py — One-time build tool: extract the horizontal-plane HRIRs from the
# MIT KEMAR SOFA file into a small compressed .npz so the shipped build no longer
# needs h5py + the ~6.5 MB HDF5 native libraries (or the 1.1 MB .sofa file).
#
# Run with an environment that HAS h5py installed (e.g. the .venv):
#     .venv/Scripts/python bake_hrtf.py
#
# Output: hrtf_kemar_horizontal.npz  (azimuths, irs_left, irs_right, samplerate)
# This mirrors exactly the data HRTFSet used to pull out of the SOFA file.

import sys
import numpy as np
import h5py

SOFA_PATH = "mit_kemar_normal_pinna.sofa"
OUT_PATH = "hrtf_kemar_horizontal.npz"


def main():
    with h5py.File(SOFA_PATH, "r") as f:
        positions = f["SourcePosition"][:]        # (M, 3): azimuth, elevation, distance
        irs = f["Data.IR"][:]                     # (M, R, N)
        sr = f["Data.SamplingRate"][()]
        sr = float(sr[0]) if hasattr(sr, "__len__") else float(sr)

    # Keep only the horizontal plane (elevation ~= 0), exactly as HRTFSet did.
    elev = positions[:, 1]
    horiz_mask = np.abs(elev) < 1.0
    horiz_pos = positions[horiz_mask]
    horiz_irs = irs[horiz_mask]

    azimuths = horiz_pos[:, 0] % 360.0
    order = np.argsort(azimuths)
    azimuths = azimuths[order].astype(np.float64)
    irs_left = horiz_irs[order, 0, :].astype(np.float32)   # receiver 0 = left
    irs_right = horiz_irs[order, 1, :].astype(np.float32)  # receiver 1 = right

    np.savez_compressed(
        OUT_PATH,
        azimuths=azimuths,
        irs_left=irs_left,
        irs_right=irs_right,
        samplerate=np.float64(sr),
    )

    print(
        f"Wrote {OUT_PATH}: {len(azimuths)} positions, "
        f"{irs_left.shape[1]} samples/IR at {int(sr)} Hz"
    )


if __name__ == "__main__":
    sys.exit(main())
