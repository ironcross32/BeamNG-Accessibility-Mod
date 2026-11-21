"""Golden cases for the Configuration panel's round trip (src-tauri panel.rs).

Builds the real wx ConfigPanel off-screen, feeds each config through
load_into_controls, and records what controls_to_config writes back. The Rust side runs
the same config through control_values then build_saved_config (no disk re-read): the
two halves of the panel together, against the Python's controls.

    uv run python rust/fixtures/gen/config_panel.py

LOCALAPPDATA is pointed at a scratch directory BEFORE configurator is imported, so the
user's config is never read or written. The speech backend, voice and output device are
left out: they depend on what this machine has installed.
"""

from __future__ import annotations

import os
import re
import tempfile

_SCRATCH = tempfile.mkdtemp(prefix="beam_panel_fx_")
os.environ["LOCALAPPDATA"] = _SCRATCH

from _common import fbits, write_fixture  # noqa: E402

import inspect  # noqa: E402

import wx  # noqa: E402

import configurator  # noqa: E402
from config_ui import ConfigPanel  # noqa: E402

MACHINE_KEYS = {"speech_backend", "speech_voice_name", "preferred_device_name"}

CASES = [
    ("defaults", {}),
    (
        "normalised",
        {
            "units": "Metric",
            "telemetry_protocol": "OUTGAUGE",
            "beamng_renderer": "Vulkan",
            "obstacle_warning_sensitivity": "weird",
            "scanner_distance_callout_interval": 30.0,
            "speed_announce_interval": 33,
            "scanner_base_freq_hz": 1002.5,
            "hrtf_set": "nope",
        },
    ),
    (
        "edited",
        {
            "units": "metric",
            "speech_rate": 73,
            "speech_volume": 40,
            "shift_tone_frequency_hz": 660.0,
            "shift_tone_level_dbfs": -9.5,
            "scanner_pitch_offset_oct": 1.25,
            "scanner_base_freq_hz": 999.5,
            "hrtf_elevation_deg": -12.0,
            "hrtf_set": "thk_ku100",
            "hrtf_equalise": False,
            "gear_report_mode": "audio",
            "gear_tone_root": "A#",
            "gear_tone_octave": 3,
            "gear_tone_scale": "mixolydian",
            "gear_tone_level_dbfs": -20.0,
            "compass_click_interval": 30,
            "mcp_server_enabled": True,
            "mcp_server_port": 4500,
            "speed_announce_interval": 75,
            "scanner_distance_callout_interval": 45,
            "obstacle_warning_sensitivity": "late",
            "launch_beamng": True,
            "road_include_private": True,
        },
    ),
    (
        "clamped_backend",
        {
            # Values the backend itself must normalise. Spin range clamps and digit
            # rounding are the frontend's (rust/ui/js/util.js), so none are used here.
            "hrtf_elevation_deg": 50.0,
            "gear_tone_level_dbfs": 5.0,
            "gear_report_mode": "bogus",
        },
    ),
]


def typed(v):
    if isinstance(v, bool):
        return {"bool": v}
    if isinstance(v, int):
        return {"int": v}
    if isinstance(v, float):
        return {"float": fbits(v)}
    if isinstance(v, str):
        return {"str": v}
    raise TypeError(f"unexpected control value {v!r}")


def main():
    app = wx.App(False)
    frame = wx.Frame(None)
    panel = ConfigPanel(frame)
    # Never let the debounced save fire into the scratch config mid-run.
    panel._save_timer.Stop()

    src = inspect.getsource(ConfigPanel.controls_to_config)
    written = sorted(set(re.findall(r'cfg\["(\w+)"\]\s*=', src)))

    cases = []
    for name, overrides in CASES:
        cfg = dict(configurator.DEFAULT_CONFIG)
        cfg.update(overrides)
        panel.load_into_controls(dict(cfg))
        panel._save_timer.Stop()
        out = panel.controls_to_config()
        cases.append(
            {
                "name": name,
                "config": {k: (fbits(v) if isinstance(v, float) else v) for k, v in cfg.items()},
                "saved": {k: typed(out[k]) for k in written if k not in MACHINE_KEYS},
            }
        )

    frame.Destroy()
    app.Destroy()
    write_fixture("config_panel", {"written_keys": written, "cases": cases})


if __name__ == "__main__":
    main()
