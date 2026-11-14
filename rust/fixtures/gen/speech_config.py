"""Golden cases for beam_base::speech_config, from speech.migrate_config."""

from __future__ import annotations

import json

from _common import write_fixture

import speech

CASES = [
    {},
    {"units": "metric"},
    {"force_sapi": True},
    {"force_sapi": False},
    {"force_sapi": "no", "speech_backend": "NVDA"},
    {"force_sapi": [], "speech_backend": "NVDA"},
    {"a": 1, "sapi_voice_name": "David", "b": 2},
    {"sapi_voice_name": "", "speech_voice_name": "Zira"},
    {"sapi_voice_name": "David", "speech_voice_name": "Zira"},
    {"sapi_voice_name": None},
    {"sapi_voice_name": 5},
    {"sapi_rate": 0}, {"sapi_rate": 3}, {"sapi_rate": -20}, {"sapi_rate": 15},
    {"sapi_rate": 3.9}, {"sapi_rate": -3.9}, {"sapi_rate": "4"}, {"sapi_rate": " -2 "},
    {"sapi_rate": "x"}, {"sapi_rate": "4.5"}, {"sapi_rate": None}, {"sapi_rate": True},
    {"sapi_rate": float("nan")}, {"sapi_rate": float("inf")}, {"sapi_rate": 10**30},
    {"sapi_rate": -(10**30)}, {"sapi_rate": [1]},
    {"sapi_rate": 5, "speech_rate": 80},
    {"sapi_volume": 50}, {"sapi_volume": 150}, {"sapi_volume": -4}, {"sapi_volume": "70"},
    {"sapi_volume": "loud"}, {"sapi_volume": 99.9}, {"sapi_volume": float("-inf")},
    {"sapi_volume": 10**30}, {"sapi_volume": 30, "speech_volume": 10},
    {"z": 0, "force_sapi": 1, "sapi_voice_name": "V", "sapi_rate": 10, "sapi_volume": 100, "y": 1},
]


def main():
    out = []
    for c in CASES:
        before = json.dumps(c)
        changed = speech.migrate_config(c)
        out.append({"before": before, "after": json.dumps(c), "changed": changed})
    write_fixture("speech_config", {"AUTO_BACKEND": speech.AUTO_BACKEND, "cases": out})


if __name__ == "__main__":
    main()
