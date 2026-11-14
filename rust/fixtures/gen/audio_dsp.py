"""Golden cases for beam_audio::dsp (the numpy idioms) and the module-level rate helpers
of audio.py that live in beam_audio::consts.

Arrays of floats are stored as lists of IEEE-754 bit patterns (u64) to keep the file
small; scalars use fbits.
"""

from __future__ import annotations

import struct

import numpy as np

from _common import fbits, write_fixture

import audio


def bits(arr):
    return [struct.unpack("<Q", struct.pack("<d", float(v)))[0] for v in np.asarray(arr, dtype=np.float64).ravel()]


def cbits(arr):
    arr = np.asarray(arr)
    return [[a, b] for a, b in zip(bits(arr.real), bits(arr.imag))]


def main():
    rng = np.random.default_rng(12345)

    lin = []
    for start, stop, num, endpoint in [
        (0.0, 1.0, 5, True), (0.0, 0.02, 960, False), (1.0, 0.0, 48, True),
        (0.0, np.pi, 384, True), (1.0, 0.3, 96, True), (0.3, 1.0, 96, True),
        (5.0, 1.0, 264, True), (2.0, 2.0, 7, True), (3.0, 7.0, 1, True),
        (3.0, 7.0, 1, False), (3.0, 7.0, 0, True), (-1.5, 2.25, 13, False),
        (0.0, 0.055, 2641, False), (0.4, 0.4, 1056, True),
    ]:
        lin.append({"start": fbits(start), "stop": fbits(stop), "num": num, "endpoint": endpoint,
                    "out": bits(np.linspace(start, stop, num, endpoint=endpoint))})

    freqs = [{"n": n, "d": fbits(d), "out": bits(np.fft.rfftfreq(n, d))}
             for n, d in [(768, 1 / 48000), (4096, 1 / 44100), (9601, 1 / 48000), (7, 0.5), (1, 1.0)]]

    sums = []
    for n in [0, 1, 5, 8, 9, 127, 128, 129, 1000, 4097]:
        x = rng.standard_normal(n)
        sums.append({"x": bits(x), "sum": fbits(np.sum(x)),
                     "mean": fbits(np.mean(x)) if n else None,
                     "rms": fbits(float(np.sqrt(np.mean(x * x)))) if n else None,
                     "cumsum": bits(np.cumsum(x)) if n <= 1000 else None})

    conv = []
    for na, nv in [(1, 1), (5, 3), (3, 5), (64, 40), (200, 120)]:
        a, v = rng.standard_normal(na), rng.standard_normal(nv)
        a32, v32 = a.astype(np.float32), v.astype(np.float32)
        conv.append({"a": bits(a), "v": bits(v), "full": bits(np.convolve(a, v, mode="full")),
                     "full_f32": bits(np.convolve(a32, v32, mode="full").astype(np.float64))})

    ss = []
    srt = np.sort(rng.uniform(0, 360, 50))
    srt = np.concatenate([srt, srt[10:13]])
    srt.sort()
    for v in [-1.0, 0.0, float(srt[11]), 180.0, 359.99, 400.0, float(srt[0])]:
        ss.append({"v": fbits(v), "left": int(np.searchsorted(srt, v, side="left")),
                   "right": int(np.searchsorted(srt, v, side="right"))})

    ffts = []
    for nx, n in [(16, 16), (15, 15), (512, 4096), (300, 256), (128, 129), (96, 103)]:
        x = rng.standard_normal(nx)
        X = np.fft.rfft(x, n=n)
        ffts.append({"x": bits(x), "n": n, "rfft": cbits(X)})
    iffts = []
    for nspec, n in [(9, 16), (8, 15), (1025, 2048), (65, 128), (2049, 557), (257, 600)]:
        spec = rng.standard_normal(nspec) + 1j * rng.standard_normal(nspec)
        iffts.append({"spec": cbits(spec), "n": n, "irfft": bits(np.fft.irfft(spec, n))})

    # numpy's default_rng streams.
    g = np.random.default_rng(audio.SCAN_RNG_SEED)
    jit = [int(g.integers(-1200, 1201)) for _ in range(1000)]
    g = np.random.default_rng(audio.SCAN_RNG_SEED)
    jit_small = [int(g.integers(-1, 2)) for _ in range(500)]
    g = np.random.default_rng(0xC0FFEE)
    uni = g.uniform(0, 2.0 * np.pi, 385)
    g = np.random.default_rng(0x511DE)
    normals_head = g.standard_normal(1000)
    g = np.random.default_rng(0x511DE)
    big = g.standard_normal(2 * 48001)
    g = np.random.default_rng(7)
    raw64 = [int(v) for v in g.bit_generator.random_raw(20)]
    g = np.random.default_rng(2 ** 70 + 99)
    raw_big_seed = [int(v) for v in g.bit_generator.random_raw(8)]
    g = np.random.default_rng(0)
    mixed = [int(g.integers(0, 10)), float(g.uniform(-3.0, 3.0)), int(g.integers(-5, 6)),
             int(g.integers(0, 2 ** 40)), float(g.standard_normal())]

    rng_fx = {
        "scan_jitter_1200": jit,
        "scan_jitter_1": jit_small,
        "uniform_c0ffee": bits(uni),
        "normal_511de_head": bits(normals_head),
        "normal_511de_big_len": len(big),
        "normal_511de_big_sum": fbits(np.sum(big)),
        "normal_511de_big_every_997": bits(big[::4999]),
        "raw_seed7": raw64,
        "raw_seed_2_70_plus_99": raw_big_seed,
        "mixed_seed0": [mixed[0], fbits(mixed[1]), mixed[2], mixed[3], fbits(mixed[4])],
    }

    # Rate helpers.
    vals = [-1.0, 0.0, 0.5, 1.0, 12.5, 20.0, 100.0, 169.0, 170.0, 200.0, 255.0, 300.0, 400.0,
            2000.0, 1e9, float("nan"), float("inf"), -0.0]
    rates = []
    for v in vals:
        rates.append({"x": fbits(v),
                      "road": fbits(audio.road_beacon_rate_hz(v)),
                      "route": fbits(audio.route_beacon_rate_hz(v)),
                      "correction": fbits(audio.road_correction_rate_hz(v))})
    obstacle = []
    for state in [0, 1, 2, 3, 4, -1]:
        for u in vals:
            obstacle.append({"state": state, "urgency": fbits(u),
                             "rate": fbits(audio.obstacle_cue_rate_hz(state, u)),
                             "duck": fbits(audio.obstacle_duck_gain(state))})
    stop = []
    for d in [-5.0, -2.5, -2.4, -0.1, 0.0, 0.5, 0.6, 1.4, 1.5, 1.6, 3.0, 10.0, 40.0, 100.0]:
        for sp in [0.0, 0.1, 0.2, 0.3, 1.0, 5.0, 20.0]:
            for stopped in [None, True, False]:
                for on_bar in [False, True]:
                    name, period = audio.stop_bar_state(d, sp, stopped, on_bar)
                    stop.append({"d": fbits(d), "speed": fbits(sp), "stopped": stopped,
                                 "on_bar": on_bar, "name": name,
                                 "period": None if period is None else fbits(period)})

    write_fixture("audio_dsp", {
        "linspace": lin, "rfftfreq": freqs, "sums": sums, "convolve": conv,
        "searchsorted": {"a": bits(srt), "cases": ss},
        "rfft": ffts, "irfft": iffts, "rng": rng_fx,
        "rates": rates, "obstacle": obstacle, "stop_bar": stop,
    })


if __name__ == "__main__":
    main()
