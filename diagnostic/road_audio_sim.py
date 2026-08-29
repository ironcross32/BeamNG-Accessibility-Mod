"""Deterministic road cue state, panning, rate, and queue diagnostics."""

from __future__ import annotations

import logging
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

import numpy as np  # noqa: E402

import audio  # noqa: E402


class FakeClock:
    def __init__(self):
        self.value = 100.0

    def __call__(self):
        return self.value


class FakeHRTF:
    def __init__(self):
        self.bearings = []

    def get_hrir(self, bearing):
        self.bearings.append(bearing)
        return np.array([1.0, 0.5], dtype=np.float32), np.array(
            [0.25, 0.125], dtype=np.float32
        )


def main():
    controller = audio.AudioController(logging.getLogger("road_audio_sim"))
    assert controller._is_enabled, "audio dependencies are unavailable"
    controller.apply_config(
        {
            "road_beep_volume_db": -14.0,
            "road_correction_volume_db": -24.0,
            "road_junction_volume_db": -14.0,
            "hrtf_enabled": False,
        }
    )
    controller.set_road_mode(True)
    assert controller._road_correction_amp < controller._road_amp

    # Aligned driving is the dead zone: no correction and no pending acquisition pulse.
    controller.update_road_guidance(
        "onRoad", None, {"active": False, "bearing": 0, "severity": 0}
    )
    assert not controller._road_correction_active
    assert controller._road_playback_pos < 0

    controller.update_road_guidance("offRoad", None, None)
    assert not controller._road_beacon_available
    controller.update_road_guidance(
        "onRoad", None, {"active": False, "bearing": 0, "severity": 0}
    )

    assert audio.road_beacon_rate_hz(distance=2) > audio.road_beacon_rate_hz(
        distance=100
    )

    clean_ratio, clean_index = audio.road_correction_timbre(0.0)
    dirty_ratio, dirty_index = audio.road_correction_timbre(1.0)
    assert clean_ratio == 5.0 and clean_index == 1.2
    assert dirty_ratio > clean_ratio and not dirty_ratio.is_integer()
    assert abs(dirty_index - 1.4) < 1e-9

    # Positive bearings are left. Stereo fallback and HRTF both preserve that contract.
    controller._hrtf = None
    controller._hrtf_user_enabled = False
    left, right = controller._render_directional_pulse(
        np.ones(16, dtype=np.float32), 45
    )
    assert np.max(np.abs(left)) > np.max(np.abs(right))

    controller._hrtf = FakeHRTF()
    controller._hrtf_user_enabled = True
    left, right = controller._render_directional_pulse(
        np.ones(16, dtype=np.float32), 45
    )
    assert len(left) == 17 and len(right) == 17
    assert np.max(np.abs(left)) > np.max(np.abs(right))

    # Correction is a solid triangle/FM voice, not a scheduled acquisition pulse.
    controller._hrtf = None
    controller._hrtf_user_enabled = False
    controller.update_road_guidance(
        "onRoad", None, {"active": True, "bearing": 90, "severity": 0.25}
    )
    correction = controller._render_road_correction_block(512, True, 90, 0.25)
    assert correction is not None
    assert np.max(np.abs(correction[0])) > np.max(np.abs(correction[1]))
    assert controller._road_correction_render_bearing == 35.0
    assert controller._road_playback_pos < 0

    # HRTF lookup never leaves the requested 35-degree frontal arc.
    fake_hrtf = FakeHRTF()
    controller._hrtf = fake_hrtf
    controller._hrtf_user_enabled = True
    controller._render_road_correction_block(512, True, -90, 1.0)
    assert fake_hrtf.bearings[-1] == 325.0

    # Alignment releases to silence; leaving the road cuts the voice and tail now.
    released = controller._render_road_correction_block(512, False, 0, 0)
    assert released is not None
    for _ in range(100):
        released = controller._render_road_correction_block(512, False, 0, 0)
        if released is None:
            break
    assert released is None and controller._road_correction_env == 0.0
    controller._render_road_correction_block(512, True, 10, 0.5)
    controller.update_road_guidance(
        "offRoad", {"bearing": 20, "distance": 8}, None
    )
    assert controller._road_correction_env == 0.0
    assert controller._road_correction_overlap_L is None

    fake_clock = FakeClock()
    original_monotonic = audio.time.monotonic
    audio.time.monotonic = fake_clock
    try:
        controller._road_chime_next_allowed_time = 0
        controller.trigger_road_orientation_chime(12)
        assert len(controller._road_chime_queue) == 1  # one-way legal direction only
        fake_clock.value += 2
        controller.trigger_road_orientation_chime(12, -168)
        assert len(controller._road_chime_queue) == 2
    finally:
        audio.time.monotonic = original_monotonic

    controller.trigger_road_junction_earcon()
    assert len(controller._road_junction_queue) == 2
    first_delays = [entry["delay"] for entry in controller._road_junction_queue]
    controller.trigger_road_junction_earcon()
    assert len(controller._road_junction_queue) == 2
    assert [entry["delay"] for entry in controller._road_junction_queue] == first_delays
    controller.trigger_road_junction_entry_earcon()
    assert len(controller._road_junction_queue) == 1
    assert controller._road_junction_queue[0]["L"] is controller.ROAD_JUNCTION_ENTRY_WAVEFORM
    assert not np.array_equal(
        controller.ROAD_JUNCTION_ENTRY_WAVEFORM,
        controller.ROAD_JUNCTION_WAVEFORM,
    )

    controller.update_road_guidance(
        "onRoad", None, {"active": True, "bearing": -35, "severity": 0.7}
    )
    assert controller._road_correction_active
    assert controller._road_correction_bearing == -35
    controller.clear_road_audio()
    assert not controller._road_correction_active
    assert not controller._road_chime_queue
    assert not controller._road_junction_queue

    print("road_audio_sim: all diagnostics passed")


if __name__ == "__main__":
    main()
