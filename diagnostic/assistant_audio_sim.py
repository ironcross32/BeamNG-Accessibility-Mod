"""Verify spatial turn slots and the independent disengagement alarm."""
import logging
import sys
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from audio import AudioController


def main():
    audio = AudioController(logging.getLogger("assistant_audio_sim"))
    audio.apply_config({"hrtf_enabled": True})
    audio.load_hrtf(str(Path(__file__).resolve().parents[1] / "hrtf_kemar_horizontal.npz"))
    assert audio._hrtf is not None
    sample_rate = audio.samplerate

    def render():
        blocks_l, blocks_r = [], []
        while audio._assistant_cue_queue:
            left, right = np.zeros(512, dtype=np.float32), np.zeros(512, dtype=np.float32)
            audio._mix_assistant_cues(left, right, len(left))
            blocks_l.append(left)
            blocks_r.append(right)
        return np.concatenate(blocks_l), np.concatenate(blocks_r)

    for choices in (("left", "right"), ("straight", "right"), ("left", "straight", "right")):
        audio.trigger_assistant_cue("junction", choices)
        assert len(audio._assistant_cue_queue) == len(choices)
        left, right = render()
        for index, direction in enumerate(("left", "straight", "right")):
            start, end = int(index * 0.24 * sample_rate), int((index * 0.24 + 0.20) * sample_rate)
            l_energy = np.sum(left[start:end] ** 2)
            r_energy = np.sum(right[start:end] ** 2)
            if direction not in choices:
                assert l_energy + r_energy == 0, "Missing exits must be silent"
            else:
                assert l_energy + r_energy > 0
                if direction == "left":
                    assert l_energy > r_energy
                elif direction == "right":
                    assert r_energy > l_energy

    audio.trigger_assistant_cue("junction", ("left", "right"))
    audio.trigger_assistant_cue("off")
    audio.set_road_mode(False)
    left, right = render()
    assert np.array_equal(left, right), "Off must cancel spatial turn cues"
    assert np.any(left[:int(sample_rate * 0.23)])
    assert not np.any(left[int(sample_rate * 0.23):int(sample_rate * 0.31)])
    assert np.any(left[int(sample_rate * 0.31):])
    print("assistant_audio_sim: all diagnostics passed with real HRTF data")


if __name__ == "__main__":
    main()
