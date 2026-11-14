"""Golden waveforms for beam_audio::waveforms, from a real AudioController.

The controller is built with a logger and no stream, as the diagnostic sims do, given a
config through apply_config, then every _generate_* method is called at the sample rate
under test. Writes three files so each stays small:

- audio_waveforms_default.json: default config at 48 kHz.
- audio_waveforms_custom.json:  non-default levels at 44.1 kHz.
- audio_slip_tables.json:       _slip_voice_tables at 48 kHz (noise loops sampled).

Waveforms are base64 little-endian float32. The level each generator read is stored
with it, so the Rust calls it with the identical value.
"""

from __future__ import annotations

import base64
import logging

import numpy as np

from _common import fbits, write_fixture

import audio


def b64(arr, dtype):
    return base64.b64encode(np.ascontiguousarray(arr, dtype=dtype).tobytes()).decode("ascii")


def wave_entry(name, arr, **params):
    e = {"name": name, "params": {k: fbits(v) if isinstance(v, float) else v for k, v in params.items()}}
    if arr is None:
        e["none"] = True
    else:
        e["dtype"] = np.asarray(arr).dtype.str
        e["out"] = b64(arr, "<f4")
    return e


def generate_all(ctrl, sr):
    ctrl.samplerate = sr
    c = ctrl
    out = []
    amp = c._compass_click_amp
    out.append(wave_entry("click", c._generate_click(), amp=amp))
    out.append(wave_entry("highlight_click", c._generate_highlight_click(), amp=amp))
    out.append(wave_entry("cam_click", c._generate_cam_click(), amp=amp))
    out.append(wave_entry("cam_highlight_click", c._generate_cam_highlight_click(), amp=amp))
    for i, w in enumerate(c._generate_placement_pings()):
        out.append(wave_entry(f"placement_ping_{i}", w, amp=c._ping_amp))
    out.append(wave_entry("center_click", c._generate_center_click()))
    out.append(wave_entry("dock_lock", c._generate_dock_lock()))
    for k in ("clear", "over", "committed"):
        out.append(wave_entry(f"slam_{k}", c._generate_slam_cue(k), kind=k))
    out.append(wave_entry("entry_cue", c._generate_entry_cue()))
    out.append(wave_entry("dock_approach_cue", c._generate_dock_approach_cue()))
    out.append(wave_entry("tc_tone", c._generate_tc_tone()))
    out.append(wave_entry("check_engine_buzzer", c._generate_check_engine_buzzer(c.buzzer_amp), amp=c.buzzer_amp))
    out.append(wave_entry("describe_error_buzz", c._generate_describe_error_buzz()))
    out.append(wave_entry("oil_chime", c._generate_oil_chime(c.chime_amp), amp=c.chime_amp))
    out.append(wave_entry("scanner_beep", c._generate_scanner_beep()))
    out.append(wave_entry("scanner_aligned_beep", c._generate_scanner_aligned_beep()))
    obs = c._generate_obstacle_buzz()
    out.append(wave_entry("obstacle_buzz", obs, amp=c._obstacle_amp,
                          harmonic_peak=float(c._obstacle_harmonic_peak)))
    out.append(wave_entry("dropoff_sweep", c._generate_dropoff_sweep()))
    out.append(wave_entry("hill_sweep", c._generate_hill_sweep()))
    out.append(wave_entry("road_beep", c._generate_road_beep(), amp=c._road_amp))
    out.append(wave_entry("route_beacon_pulse", c._generate_route_beacon_pulse(), amp=c._route_beacon_amp))
    out.append(wave_entry("road_chime_fwd", c._generate_road_chime_tone(audio.ROAD_CHIME_FWD_FREQ_HZ),
                          freq=audio.ROAD_CHIME_FWD_FREQ_HZ))
    out.append(wave_entry("road_chime_back", c._generate_road_chime_tone(audio.ROAD_CHIME_BACK_FREQ_HZ),
                          freq=audio.ROAD_CHIME_BACK_FREQ_HZ))
    out.append(wave_entry("road_junction_pip", c._generate_road_junction_pip(), amp=c._road_junction_amp))
    out.append(wave_entry("road_junction_entry", c._generate_road_junction_entry(), amp=c._road_junction_amp))
    out.append(wave_entry("road_correction_settled", c._generate_road_correction_settled(),
                          amp=c._road_correction_amp))
    out.append(wave_entry("guidance_tone", c._generate_guidance_tone()))
    out.append(wave_entry("lowspeed_click", c._generate_lowspeed_click(), db=c._ls_click_amp_db))
    out.append(wave_entry("lowspeed_stop_tone", c._generate_lowspeed_stop_tone(), db=c._ls_stop_amp_db))
    out.append(wave_entry("clickspot_beep", c._generate_clickspot_beep()))
    out.append(wave_entry("clickspot_beep_reverse", c._generate_clickspot_beep_reverse()))
    out.append(wave_entry("node_beep", c._generate_node_beep()))
    out.append(wave_entry("node_beep_reverse", c._generate_node_beep_reverse()))
    return out


def controller(cfg):
    log = logging.getLogger("waveform-fixture")
    log.addHandler(logging.NullHandler())
    log.propagate = False
    ctrl = audio.AudioController(log)
    ctrl.apply_config(cfg)
    return ctrl


CUSTOM_CFG = {
    "compass_click_level_dbfs": -9.5,
    "placement_ping_volume_db": -20.0,
    "obstacle_buzz_volume_db": -25.0,
    "road_beep_volume_db": -8.0,
    "route_beacon_volume_db": -22.0,
    "road_junction_volume_db": -10.0,
    "lowspeed_click_level_dbfs": -20.0,
    "lowspeed_stop_tone_level_dbfs": -9.0,
    "check_engine_buzzer_level_dbfs": -6.0,
    "oil_chime_level_dbfs": -18.0,
    "road_correction_volume_db": -30.0,
}


def main():
    d = controller({})
    write_fixture("audio_waveforms_default", {"sr": fbits(48000.0), "config": {},
                                              "waves": generate_all(d, 48000.0)})
    c = controller(CUSTOM_CFG)
    write_fixture("audio_waveforms_custom", {"sr": fbits(44100.0), "config": CUSTOM_CFG,
                                             "waves": generate_all(c, 44100.0)})

    s = controller({})
    s.samplerate = 48000.0
    spin, lock, lo, hi = s._slip_voice_tables()
    write_fixture("audio_slip_tables", {
        "sr": fbits(48000.0),
        "spin": b64(spin, "<f4"), "lock": b64(lock, "<f4"),
        "noise_len": int(len(lo)),
        "noise_lo_sampled": b64(lo[::11], "<f4"), "noise_hi_sampled": b64(hi[::11], "<f4"),
        "noise_lo_rms": fbits(float(np.sqrt(np.mean(lo.astype(np.float64) ** 2)))),
        "noise_hi_rms": fbits(float(np.sqrt(np.mean(hi.astype(np.float64) ** 2)))),
        "dtypes": [a.dtype.str for a in (spin, lock, lo, hi)],
    })


if __name__ == "__main__":
    main()
