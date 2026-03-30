
# configurator.py
# Configuration backend for BeamTel.
# Handles config loading, saving, validation, SAPI voice enumeration, and audio tests.
# UI code lives in config_ui.py.

# nuitka-project: --onefile
# nuitka-project: --assume-yes-for-downloads
# nuitka-project: --windows-disable-console
# nuitka-project: --include-package=comtypes
# nuitka-project: --include-package-data=comtypes
# nuitka-project: --include-package=numpy
# nuitka-project: --include-package=sounddevice
# nuitka-project: --include-module=config_ui

import json
import os
import sys

# --- Audio Test Dependencies ---
AUDIO_TEST_OK = False
try:
    import numpy as np
    import sounddevice as sd
    AUDIO_TEST_OK = True
except ImportError:
    pass

FROZEN = getattr(sys, "frozen", False)

# --- Use %localappdata% for configuration ---
def _get_config_dir():
    """Gets the configuration directory, falling back if LOCALAPPDATA is not set."""
    base_path = os.getenv("LOCALAPPDATA")
    if base_path is None:
        base_path = os.path.expanduser("~")
    return os.path.join(base_path, "beamtel")

CONFIG_DIR = _get_config_dir()
os.makedirs(CONFIG_DIR, exist_ok=True)
CONFIG_PATH = os.path.join(CONFIG_DIR, "beamtel_config.json")


# --- Sync DEFAULT_CONFIG with beamtel.py ---
DEFAULT_CONFIG = {
    "force_sapi": False,
    "sapi_voice_name": "",
    "sapi_rate": 0,
    "sapi_volume": 100,
    "units": "imperial",
    "shift_tone_frequency_hz": 880.0,
    "shift_tone_level_dbfs": -12.0,
    "check_engine_buzzer_level_dbfs": -12.0,
    "oil_chime_level_dbfs": -12.0,
    "oil_chime_enabled": True,
    "tc_clicks_enabled": True,
    "pitch_roll_tones_enabled": True,
    "pitch_roll_max_dbfs": -24.0,
    "pitch_roll_min_dbfs": -36.0,
    "compass_click_level_dbfs": -6.0,
    "lowspeed_click_level_dbfs": -14.0,
    "neutral_dwell_ms": 300,
    "telemetry_protocol": "extended",
    "compass_click_interval": 15,
    "compass_highlight_enabled": True,
    # MODIFIED: Changed from degrees to a counter
    "compass_highlight_nth_click": 6,
    "hrtf_enabled": True,
    "hrtf_front_emphasis_db": -6.0,
    "hrtf_distance_gain_db": 0.0,
    "follow_default_audio_device": True,
    "preferred_device_name": "",
    "audio_poll_interval_sec": 2.0,
    "launch_beamng": False,
}


def _setup_comtypes_cache():
    try:
        cache_dir = os.path.join(CONFIG_DIR, "comtypes_cache")
        os.makedirs(cache_dir, exist_ok=True)
        os.environ.setdefault("COMTYPES_CACHE_DIR", cache_dir)
    except Exception:
        pass


def list_wasapi_output_devices():
    """Return sorted list of WASAPI output device names available on this system."""
    if not AUDIO_TEST_OK:
        return []
    try:
        apis = sd.query_hostapis()
        wasapi_idx = next(
            (i for i, a in enumerate(apis) if "wasapi" in str(a.get("name", "")).lower()),
            None,
        )
        if wasapi_idx is None:
            return []
        return sorted(
            d["name"]
            for d in sd.query_devices()
            if d["hostapi"] == wasapi_idx and d["max_output_channels"] > 0
        )
    except Exception:
        return []


def list_sapi_voices():
    try:
        _setup_comtypes_cache()
        import comtypes.client as cc

        v = cc.CreateObject("SAPI.SpVoice")
        tokens = v.GetVoices()
        out = []
        for i in range(tokens.Count):
            t = tokens.Item(i)
            try:
                name = str(t.GetDescription())
            except Exception:
                name = f"Voice {i}"
            out.append(name)
        out.sort(key=lambda s: s.lower())
        return out
    except Exception:
        return []


def test_sapi_voice(display_name_exact: str, rate: int, volume: int):
    """Speak a synchronous test with the exact display name (case-insensitive).
    Returns (success: bool, error_message: str | None)."""
    try:
        _setup_comtypes_cache()
        import comtypes.client as cc

        rate = int(max(-10, min(10, rate)))
        volume = int(max(0, min(100, volume)))

        sp = cc.CreateObject("SAPI.SpVoice")
        try:
            toks = sp.GetVoices()
            want = (display_name_exact or "").strip().lower()
            sel = None
            for i in range(toks.Count):
                t = toks.Item(i)
                try:
                    desc = str(t.GetDescription()).strip()
                except Exception:
                    continue
                if desc.lower() == want:
                    sel = t
                    break
            if sel:
                sp.Voice = sel
        except Exception:
            pass

        sp.Rate = rate
        sp.Volume = volume
        sp.Speak("This is a test of the current SAPI configuration.", 0)
        return True, None
    except Exception as e:
        return False, f"SAPI test failed: {e}"


def play_test_tone(freq_hz, level_dbfs):
    """Generates and plays a short triangle wave for testing.
    Returns (success: bool, error_message: str | None)."""
    if not AUDIO_TEST_OK:
        return False, "Audio test unavailable. Please ensure 'numpy' and 'sounddevice' are installed."
    try:
        SR = 48000
        DUR_SEC = 0.75
        amp = 10.0 ** (float(level_dbfs) / 20.0)
        phase_inc = float(freq_hz) / SR

        t = (np.arange(int(SR * DUR_SEC)) * phase_inc) % 1.0
        wave = amp * (2.0 * np.abs(2.0 * t - 1.0) - 1.0)

        fade_samples = int(SR * 0.05)
        wave[-fade_samples:] *= np.linspace(1, 0, fade_samples)

        sd.play(wave.astype(np.float32), samplerate=SR, blocking=False)
        return True, None
    except Exception as e:
        return False, f"Failed to play audio: {e}"


def _write_config(path, cfg):
    with open(path, "w", encoding="utf-8") as f:
        json.dump(cfg, f, indent=2, ensure_ascii=False)


def load_config():
    if not os.path.isfile(CONFIG_PATH):
        _write_config(CONFIG_PATH, DEFAULT_CONFIG)
        return DEFAULT_CONFIG.copy()
    try:
        with open(CONFIG_PATH, "r", encoding="utf-8") as f:
            user = json.load(f)
        if not isinstance(user, dict):
            raise ValueError("Config root is not an object")

        merged = DEFAULT_CONFIG.copy()
        merged.update(user)

        def _coerce(key, caster, fallback):
            try:
                merged[key] = caster(merged.get(key, fallback))
            except (ValueError, TypeError):
                merged[key] = fallback

        _coerce("force_sapi", bool, False)
        _coerce("sapi_voice_name", str, "")
        _coerce("sapi_rate", int, 0)
        _coerce("sapi_volume", int, 100)
        _coerce("shift_tone_frequency_hz", float, 880.0)
        _coerce("shift_tone_level_dbfs", float, -12.0)
        _coerce("check_engine_buzzer_level_dbfs", float, -12.0)
        _coerce("oil_chime_level_dbfs", float, -12.0)
        _coerce("oil_chime_enabled", bool, True)
        _coerce("tc_clicks_enabled", bool, True)
        _coerce("pitch_roll_tones_enabled", bool, True)
        _coerce("pitch_roll_max_dbfs", float, -24.0)
        _coerce("pitch_roll_min_dbfs", float, -36.0)
        _coerce("compass_click_level_dbfs", float, -6.0)
        _coerce("lowspeed_click_level_dbfs", float, -14.0)
        _coerce("neutral_dwell_ms", int, 300)
        _coerce("compass_click_interval", int, 15)
        _coerce("compass_highlight_enabled", bool, False)
        _coerce("compass_highlight_nth_click", int, 6) # MODIFIED
        _coerce("hrtf_enabled", bool, True)
        _coerce("hrtf_front_emphasis_db", float, -6.0)
        _coerce("hrtf_distance_gain_db", float, 0.0)
        _coerce("launch_beamng", bool, False)
        _coerce("follow_default_audio_device", bool, True)
        _coerce("preferred_device_name", str, "")
        _coerce("audio_poll_interval_sec", float, 2.0)

        merged["sapi_rate"] = max(-10, min(10, merged["sapi_rate"]))
        merged["sapi_volume"] = max(0, min(100, merged["sapi_volume"]))
        merged["shift_tone_frequency_hz"] = max(20.0, min(20000.0, merged["shift_tone_frequency_hz"]))
        merged["neutral_dwell_ms"] = max(0, min(5000, merged["neutral_dwell_ms"]))
        merged["compass_click_interval"] = max(1, min(90, merged["compass_click_interval"]))
        merged["compass_highlight_nth_click"] = max(2, min(100, merged["compass_highlight_nth_click"])) # MODIFIED
        merged["audio_poll_interval_sec"] = max(0.1, min(10.0, merged["audio_poll_interval_sec"]))

        for key in ["shift_tone_level_dbfs", "check_engine_buzzer_level_dbfs", "oil_chime_level_dbfs",
                    "pitch_roll_max_dbfs", "pitch_roll_min_dbfs", "compass_click_level_dbfs",
                    "lowspeed_click_level_dbfs", "hrtf_front_emphasis_db"]:
            db = merged.get(key, -12.0)
            merged[key] = max(-120.0, min(0.0, db))
        merged["hrtf_distance_gain_db"] = max(-24.0, min(6.0, merged.get("hrtf_distance_gain_db", 0.0)))

        merged["units"] = "metric" if str(merged.get("units", "imperial")).lower().startswith("m") else "imperial"
        merged["telemetry_protocol"] = "outgauge" if str(merged.get("telemetry_protocol", "extended")).lower() == "outgauge" else "extended"

        return merged
    except Exception:
        try:
            os.replace(CONFIG_PATH, CONFIG_PATH + ".bak.cfgtool")
        except Exception:
            pass
        _write_config(CONFIG_PATH, DEFAULT_CONFIG)
        return DEFAULT_CONFIG.copy()


def main():
    """Standalone configurator UI."""
    import traceback
    try:
        import wx
        from config_ui import ConfigPanel
        app = wx.App(False)
        frm = wx.Frame(None, title="BeamTel Configurator", size=(600, 940))
        frm.SetMinSize((580, 940))
        panel = ConfigPanel(frm)
        frm.Centre()
        frm.Show()
        app.MainLoop()
    except Exception:
        print("--- WX UI FAILED TO INITIALIZE ---")
        print(traceback.format_exc())


if __name__ == "__main__":
    main()
