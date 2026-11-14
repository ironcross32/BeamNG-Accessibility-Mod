"""Golden cases for beam_audio::scanner_hrtf_diag, from scanner_hrtf_diag.py."""

from __future__ import annotations

import numpy as np

from _common import fbits, write_fixture

import scanner_hrtf_diag as d

FMT = "%.3f,%.3f,%.4f,%.3f,%d,%.4f,%.4f,%d,%.3f,%.3f,%.3f,%.6f,%.6f\n"


def main():
    rng = np.random.default_rng(5)
    rms_cases = []
    for n, dtype in [(0, np.float32), (1, np.float32), (7, np.float32), (512, np.float32),
                     (538, np.float64), (1024, np.float64)]:
        a = (rng.standard_normal(n) * 0.1).astype(dtype)
        rms_cases.append({"dtype": np.dtype(dtype).str, "x": [fbits(float(v)) for v in a],
                          "rms": fbits(d._rms(a))})
    rms_cases.append({"dtype": "none", "x": [], "rms": fbits(d._rms(None))})

    ild = []
    for l, r in [(0.0, 0.0), (1e-10, 5e-10), (1e-9, 0.0), (0.1, 0.1), (0.2, 0.1), (0.01, 0.3),
                 (1.0, 1e-12), (float("nan"), 0.1)]:
        ild.append({"l": fbits(l), "r": fbits(r), "out": fbits(d._ild_db(l, r))})

    rows = []
    for row in [
        (12345.6789, 0.0, -0.00005, 12.3456, 1, 0.5, 1.41421356, 1, -3.14159, 0.0, -1.23456, 0.012345678, 0.0),
        (0.0005, 10.0005, 179.99995, 0.0, 0, 1.0, 2.0, 0, 0.0, float("nan"), float("inf"), 1e-7, 5e-7),
        (-0.0, -2.5, -90.00005, 1e6, 1, -0.00004, 0.99995, 1, 359.9995, -0.0005, 0.0005, 2.5e-7, 1.0),
    ]:
        rows.append({"row": [fbits(v) if isinstance(v, float) else v for v in row], "line": FMT % row})

    write_fixture("scanner_hrtf_diag", {"header": d.CSV_HEADER, "rms": rms_cases, "ild": ild,
                                        "rows": rows})


if __name__ == "__main__":
    main()
