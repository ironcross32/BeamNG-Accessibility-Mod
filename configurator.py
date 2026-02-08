
# configurator.py

# nuitka-project: --onefile
# nuitka-project: --assume-yes-for-downloads
# nuitka-project: --windows-disable-console
# nuitka-project: --include-package=comtypes
# nuitka-project: --include-package-data=comtypes
# nuitka-project: --include-package=numpy
# nuitka-project: --include-package=sounddevice

import json
import os
import sys
import traceback
import wx

# --- Audio Test Dependencies ---
AUDIO_TEST_OK = False
try:
    import numpy as np
    import sounddevice as sd
    AUDIO_TEST_OK = True
except ImportError:
    pass # Will be handled by disabling the test button

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
    "preferred_hostapi": "wasapi",
    "audio_poll_interval_sec": 2.0,
}


def _setup_comtypes_cache():
    try:
        cache_dir = os.path.join(CONFIG_DIR, "comtypes_cache")
        os.makedirs(cache_dir, exist_ok=True)
        os.environ.setdefault("COMTYPES_CACHE_DIR", cache_dir)
    except Exception:
        pass


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
    """Speak a synchronous test with the exact display name (case-insensitive)."""
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
    """Generates and plays a short triangle wave for testing."""
    if not AUDIO_TEST_OK:
        wx.MessageBox(
            "Audio test unavailable. Please ensure 'numpy' and 'sounddevice' are installed.",
            "Audio Error", wx.OK | wx.ICON_ERROR
        )
        return
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

    except Exception as e:
        wx.MessageBox(f"Failed to play audio: {e}", "Audio Error", wx.OK | wx.ICON_ERROR)


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
        _coerce("follow_default_audio_device", bool, True)
        _coerce("preferred_hostapi", str, "wasapi")
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


class LabelAccessible(wx.Accessible):
    """MSAA accessible object that returns a fixed name for screen readers."""
    def __init__(self, win, name):
        super().__init__(win)
        self._name = name

    def GetName(self, childId):
        return (wx.ACC_OK, self._name)


def _label_spin_double(ctrl, name):
    """Set MSAA Name on a SpinCtrlDouble and its inner TextCtrl for NVDA."""
    ctrl.SetAccessible(LabelAccessible(ctrl, name))
    for child in ctrl.GetChildren():
        if isinstance(child, wx.TextCtrl):
            child.SetAccessible(LabelAccessible(child, name))
            return


class ConfigFrame(wx.Frame):
    def __init__(self, parent=None, id=wx.ID_ANY, title="BeamTel Configurator"):
        super().__init__(parent, id, title, size=(600, 940))
        self.SetMinSize((580, 940))

        self.panel = wx.Panel(self)
        vbox = wx.BoxSizer(wx.VERTICAL)

        # --- Speech group ---
        sb_speech = wx.StaticBox(self.panel, label="Speech")
        speech = wx.StaticBoxSizer(sb_speech, wx.VERTICAL)
        self.chk_force_sapi = wx.CheckBox(self.panel, label="&Force SAPI (skip NVDA detection)")
        self.chk_force_sapi.SetToolTip("When enabled, the telemetry app will use SAPI only and skip NVDA.")
        self.chk_force_sapi.SetName("Force SAPI")
        speech.Add(self.chk_force_sapi, 0, wx.ALL, 6)
        grid = wx.FlexGridSizer(0, 2, 6, 8)
        grid.AddGrowableCol(1, 1)
        lbl_voice = wx.StaticText(self.panel, label="&SAPI Voice:")
        self.choice_voice = wx.Choice(self.panel)
        self.choice_voice.SetToolTip("Select the SAPI voice to use (by display name).")
        self.choice_voice.SetName("SAPI Voice")
        grid.Add(lbl_voice, 0, wx.ALIGN_CENTER_VERTICAL)
        grid.Add(self.choice_voice, 1, wx.EXPAND)
        lbl_rate = wx.StaticText(self.panel, label="SAPI &Rate (-10 to 10):")
        self.spin_rate = wx.SpinCtrl(self.panel, min=-10, max=10)
        self.spin_rate.SetToolTip("SAPI rate: -10 (slow) to 10 (fast).")
        self.spin_rate.SetName("SAPI Rate")
        grid.Add(lbl_rate, 0, wx.ALIGN_CENTER_VERTICAL)
        grid.Add(self.spin_rate, 1, wx.EXPAND)
        lbl_vol = wx.StaticText(self.panel, label="SAPI &Volume (0 to 100):")
        self.spin_volume = wx.SpinCtrl(self.panel, min=0, max=100)
        self.spin_volume.SetToolTip("SAPI volume: 0 to 100.")
        self.spin_volume.SetName("SAPI Volume")
        grid.Add(lbl_vol, 0, wx.ALIGN_CENTER_VERTICAL)
        grid.Add(self.spin_volume, 1, wx.EXPAND)
        speech.Add(grid, 0, wx.LEFT | wx.RIGHT | wx.TOP | wx.EXPAND, 6)
        speech_btn_sizer = wx.BoxSizer(wx.HORIZONTAL)
        btn_refresh = wx.Button(self.panel, label="Re&fresh")
        btn_refresh.SetToolTip("Refresh the list of installed SAPI voices.")
        btn_refresh.SetName("Refresh Voices")
        btn_test = wx.Button(self.panel, label="&Test Voice")
        btn_test.SetToolTip("Speak a short test line with the selected voice, rate, and volume.")
        btn_test.SetName("Test Voice")
        speech_btn_sizer.AddStretchSpacer()
        speech_btn_sizer.Add(btn_refresh, 0, wx.RIGHT, 5)
        speech_btn_sizer.Add(btn_test)
        speech.Add(speech_btn_sizer, 0, wx.ALL | wx.EXPAND, 6)
        vbox.Add(speech, 0, wx.ALL | wx.EXPAND, 10)

        # --- General group ---
        sb_gen = wx.StaticBox(self.panel, label="General")
        gen = wx.StaticBoxSizer(sb_gen, wx.VERTICAL)
        gen_grid = wx.FlexGridSizer(0, 2, 6, 8)
        gen_grid.AddGrowableCol(1, 1)
        self.rb_units = wx.RadioBox(
            self.panel, label="&Display Units", choices=["Imperial (mph, °F, psi)", "Metric (km/h, °C, bar)"],
            majorDimension=1, style=wx.RA_SPECIFY_ROWS)
        self.rb_units.SetName("Display Units")
        self.rb_units.SetToolTip("Choose how speeds, temps, and pressures are spoken.")
        gen.Add(self.rb_units, 0, wx.ALL | wx.EXPAND, 6)
        self.rb_proto = wx.RadioBox(
            self.panel, label="&Telemetry Protocol", choices=["Extended", "OutGauge"],
            majorDimension=1, style=wx.RA_SPECIFY_ROWS)
        self.rb_proto.SetName("Telemetry Protocol")
        self.rb_proto.SetToolTip("Must match the protocol selected in the BeamNG.drive UI app.")
        gen.Add(self.rb_proto, 0, wx.LEFT|wx.RIGHT|wx.BOTTOM| wx.EXPAND, 6)
        lbl_dwell = wx.StaticText(self.panel, label="&Neutral dwell (ms):")
        self.spin_dwell = wx.SpinCtrl(self.panel, min=0, max=5000)
        self.spin_dwell.SetToolTip("How long to remain in neutral before speaking 'neutral' (debounce).")
        self.spin_dwell.SetName("Neutral dwell time")
        gen_grid.Add(lbl_dwell, 0, wx.ALIGN_CENTER_VERTICAL)
        gen_grid.Add(self.spin_dwell, 0, wx.EXPAND)
        gen.Add(gen_grid, 0, wx.ALL | wx.EXPAND, 6)
        vbox.Add(gen, 0, wx.LEFT | wx.RIGHT | wx.BOTTOM | wx.EXPAND, 10)

        # Compass Clicks Group
        sb_compass = wx.StaticBox(self.panel, label="Compass Clicks")
        compass_sizer = wx.StaticBoxSizer(sb_compass, wx.VERTICAL)
        compass_grid = wx.FlexGridSizer(0, 2, 6, 8)
        compass_grid.AddGrowableCol(1, 1)
        
        lbl_compass_interval = wx.StaticText(self.panel, label="Click Interval (degrees):")
        self.spin_compass_interval = wx.SpinCtrl(self.panel, min=1, max=90)
        self.spin_compass_interval.SetToolTip("The number of degrees of rotation before a compass click is heard (1-90).")
        self.spin_compass_interval.SetName("Compass Click Interval")
        compass_grid.Add(lbl_compass_interval, 0, wx.ALIGN_CENTER_VERTICAL)
        compass_grid.Add(self.spin_compass_interval, 0, wx.EXPAND)
        
        # MODIFIED: Control and label for highlight Nth click
        self.chk_compass_highlight = wx.CheckBox(self.panel, label="Highlight Every:")
        self.chk_compass_highlight.SetToolTip("Play a distinct sound on a certain click.")
        self.chk_compass_highlight.SetName("Enable Compass Highlight")
        
        self.spin_compass_highlight_nth = wx.SpinCtrl(self.panel, min=2, max=100)
        self.spin_compass_highlight_nth.SetToolTip("Play the highlight sound on every Nth click (e.g., 4 for quadrants).")
        self.spin_compass_highlight_nth.SetName("Compass Highlight Nth Click")

        compass_grid.Add(self.chk_compass_highlight, 0, wx.ALIGN_CENTER_VERTICAL)
        
        # MODIFIED: Add a horizontal sizer for the spin control and its label "clicks"
        highlight_row = wx.BoxSizer(wx.HORIZONTAL)
        highlight_row.Add(self.spin_compass_highlight_nth, 1, wx.EXPAND)
        highlight_row.Add(wx.StaticText(self.panel, label=" clicks"), 0, wx.ALIGN_CENTER_VERTICAL | wx.LEFT, 4)
        compass_grid.Add(highlight_row, 1, wx.EXPAND)
        
        lbl_compass_click_level = wx.StaticText(self.panel, label="Compass Click &Volume (dBFS):")
        self.spin_compass_click_level = wx.SpinCtrlDouble(self.panel, min=-120.0, max=0.0, inc=1.0)
        self.spin_compass_click_level.SetDigits(1)
        self.spin_compass_click_level.SetToolTip("Volume level for compass clicks. 0 is loudest, negatives are quieter.")
        self.spin_compass_click_level.SetName("Compass Click Volume")
        _label_spin_double(self.spin_compass_click_level, "Compass Click Volume")
        compass_grid.Add(lbl_compass_click_level, 0, wx.ALIGN_CENTER_VERTICAL)
        compass_grid.Add(self.spin_compass_click_level, 0, wx.EXPAND)

        lbl_lowspeed_click_level = wx.StaticText(self.panel, label="Low Speed Click Volu&me (dBFS):")
        self.spin_lowspeed_click_level = wx.SpinCtrlDouble(self.panel, min=-120.0, max=0.0, inc=1.0)
        self.spin_lowspeed_click_level.SetDigits(1)
        self.spin_lowspeed_click_level.SetToolTip("Volume level for low speed detection clicks.")
        self.spin_lowspeed_click_level.SetName("Low Speed Click Volume")
        _label_spin_double(self.spin_lowspeed_click_level, "Low Speed Click Volume")
        compass_grid.Add(lbl_lowspeed_click_level, 0, wx.ALIGN_CENTER_VERTICAL)
        compass_grid.Add(self.spin_lowspeed_click_level, 0, wx.EXPAND)

        compass_sizer.Add(compass_grid, 0, wx.ALL | wx.EXPAND, 6)

        # HRTF controls
        self.chk_hrtf_enabled = wx.CheckBox(self.panel, label="H&RTF Binaural Processing")
        self.chk_hrtf_enabled.SetToolTip("Enable HRTF for 3D spatial audio on compass and low speed clicks. Requires SOFA file.")
        self.chk_hrtf_enabled.SetName("Enable HRTF")
        compass_sizer.Add(self.chk_hrtf_enabled, 0, wx.LEFT | wx.RIGHT, 6)

        self.hrtf_grid = wx.FlexGridSizer(0, 2, 6, 8)
        self.hrtf_grid.AddGrowableCol(1, 1)

        lbl_hrtf_emphasis = wx.StaticText(self.panel, label="HRTF Front &Emphasis (dB):")
        self.spin_hrtf_front_emphasis = wx.SpinCtrlDouble(self.panel, min=-24.0, max=0.0, inc=1.0)
        self.spin_hrtf_front_emphasis.SetDigits(1)
        self.spin_hrtf_front_emphasis.SetToolTip("Attenuation applied to sounds behind you. More negative = rear sounds quieter relative to front.")
        self.spin_hrtf_front_emphasis.SetName("HRTF Front Emphasis")
        _label_spin_double(self.spin_hrtf_front_emphasis, "HRTF Front Emphasis")
        self.hrtf_grid.Add(lbl_hrtf_emphasis, 0, wx.ALIGN_CENTER_VERTICAL)
        self.hrtf_grid.Add(self.spin_hrtf_front_emphasis, 0, wx.EXPAND)

        lbl_hrtf_distance = wx.StaticText(self.panel, label="HRTF &Distance Gain (dB):")
        self.spin_hrtf_distance_gain = wx.SpinCtrlDouble(self.panel, min=-24.0, max=6.0, inc=1.0)
        self.spin_hrtf_distance_gain.SetDigits(1)
        self.spin_hrtf_distance_gain.SetToolTip("Additional gain after HRTF processing. Negative values make sounds appear farther away.")
        self.spin_hrtf_distance_gain.SetName("HRTF Distance Gain")
        _label_spin_double(self.spin_hrtf_distance_gain, "HRTF Distance Gain")
        self.hrtf_grid.Add(lbl_hrtf_distance, 0, wx.ALIGN_CENTER_VERTICAL)
        self.hrtf_grid.Add(self.spin_hrtf_distance_gain, 0, wx.EXPAND)

        compass_sizer.Add(self.hrtf_grid, 0, wx.ALL | wx.EXPAND, 6)
        vbox.Add(compass_sizer, 0, wx.LEFT | wx.RIGHT | wx.BOTTOM | wx.EXPAND, 10)

        # --- Pitch & Roll Group ---
        sb_pitchroll = wx.StaticBox(self.panel, label="Pitch && Roll")
        pitchroll_sizer = wx.StaticBoxSizer(sb_pitchroll, wx.VERTICAL)

        self.chk_pitch_roll_enabled = wx.CheckBox(self.panel, label="&Pitch and Roll Tones")
        self.chk_pitch_roll_enabled.SetToolTip("Enable continuous tones that indicate vehicle pitch and roll angle.")
        self.chk_pitch_roll_enabled.SetName("Enable Pitch and Roll Tones")
        pitchroll_sizer.Add(self.chk_pitch_roll_enabled, 0, wx.ALL, 6)

        pitchroll_grid = wx.FlexGridSizer(0, 2, 6, 8)
        pitchroll_grid.AddGrowableCol(1, 1)

        lbl_pr_max = wx.StaticText(self.panel, label="Ma&ximum Volume (dBFS):")
        self.spin_pitch_roll_max = wx.SpinCtrlDouble(self.panel, min=-120.0, max=0.0, inc=1.0)
        self.spin_pitch_roll_max.SetDigits(1)
        self.spin_pitch_roll_max.SetToolTip("Volume level at maximum tilt angle.")
        self.spin_pitch_roll_max.SetName("Pitch Roll Maximum Volume")
        _label_spin_double(self.spin_pitch_roll_max, "Pitch Roll Maximum Volume")
        pitchroll_grid.Add(lbl_pr_max, 0, wx.ALIGN_CENTER_VERTICAL)
        pitchroll_grid.Add(self.spin_pitch_roll_max, 0, wx.EXPAND)

        lbl_pr_min = wx.StaticText(self.panel, label="Sta&bilized Volume (dBFS):")
        self.spin_pitch_roll_min = wx.SpinCtrlDouble(self.panel, min=-120.0, max=0.0, inc=1.0)
        self.spin_pitch_roll_min.SetDigits(1)
        self.spin_pitch_roll_min.SetToolTip("Volume level when the vehicle is stable (faded down).")
        self.spin_pitch_roll_min.SetName("Pitch Roll Stabilized Volume")
        _label_spin_double(self.spin_pitch_roll_min, "Pitch Roll Stabilized Volume")
        pitchroll_grid.Add(lbl_pr_min, 0, wx.ALIGN_CENTER_VERTICAL)
        pitchroll_grid.Add(self.spin_pitch_roll_min, 0, wx.EXPAND)

        pitchroll_sizer.Add(pitchroll_grid, 0, wx.LEFT | wx.RIGHT | wx.BOTTOM | wx.EXPAND, 6)
        vbox.Add(pitchroll_sizer, 0, wx.LEFT | wx.RIGHT | wx.BOTTOM | wx.EXPAND, 10)

        # --- Shift Tone Group ---
        sb_shift = wx.StaticBox(self.panel, label="Shift Tone")
        shift_sizer = wx.StaticBoxSizer(sb_shift, wx.VERTICAL)
        shift_grid = wx.FlexGridSizer(0, 3, 6, 8) 
        shift_grid.AddGrowableCol(1, 1)
        lbl_shift_freq = wx.StaticText(self.panel, label="Shift Tone &Frequency (Hz):")
        self.spin_freq = wx.SpinCtrlDouble(self.panel, min=20.0, max=20000.0, inc=1.0)
        self.spin_freq.SetDigits(1)
        self.spin_freq.SetToolTip("Triangle tone frequency when shift light is on.")
        self.spin_freq.SetName("Shift Tone Frequency")
        _label_spin_double(self.spin_freq, "Shift Tone Frequency")
        shift_grid.Add(lbl_shift_freq, 0, wx.ALIGN_CENTER_VERTICAL)
        shift_grid.Add(self.spin_freq, 1, wx.EXPAND)
        shift_grid.Add((0,0))
        lbl_shift_level = wx.StaticText(self.panel, label="Shift Tone Le&vel (dBFS):")
        self.spin_level = wx.SpinCtrlDouble(self.panel, min=-120.0, max=0.0, inc=1.0)
        self.spin_level.SetDigits(1)
        self.spin_level.SetToolTip("Tone level; 0 is loudest, negatives are quieter.")
        self.spin_level.SetName("Shift Tone Level")
        _label_spin_double(self.spin_level, "Shift Tone Level")
        shift_grid.Add(lbl_shift_level, 0, wx.ALIGN_CENTER_VERTICAL)
        shift_grid.Add(self.spin_level, 1, wx.EXPAND)
        self.btn_test_tone = wx.Button(self.panel, label="Test Shift &Tone")
        self.btn_test_tone.SetToolTip("Plays the shift tone using the current frequency and level settings.")
        self.btn_test_tone.SetName("Test Shift Tone")
        self.btn_test_tone.Enable(AUDIO_TEST_OK)
        shift_grid.Add(self.btn_test_tone, 0)
        shift_sizer.Add(shift_grid, 0, wx.ALL | wx.EXPAND, 6)
        vbox.Add(shift_sizer, 0, wx.LEFT | wx.RIGHT | wx.BOTTOM | wx.EXPAND, 10)

        # --- Warning Sounds Group ---
        sb_warnings = wx.StaticBox(self.panel, label="Warning Sounds")
        warnings_sizer = wx.StaticBoxSizer(sb_warnings, wx.VERTICAL)

        self.chk_oil_chime_enabled = wx.CheckBox(self.panel, label="&Oil Overheating Alert")
        self.chk_oil_chime_enabled.SetToolTip("Enable the oil warning chime when the oil pressure warning light is on.")
        self.chk_oil_chime_enabled.SetName("Oil Overheating Alert")
        warnings_sizer.Add(self.chk_oil_chime_enabled, 0, wx.ALL, 6)

        self.chk_tc_clicks_enabled = wx.CheckBox(self.panel, label="T&C (Traction Control) Clicks")
        self.chk_tc_clicks_enabled.SetToolTip("Enable the traction control activation click sound.")
        self.chk_tc_clicks_enabled.SetName("TC Clicks")
        warnings_sizer.Add(self.chk_tc_clicks_enabled, 0, wx.LEFT | wx.RIGHT | wx.BOTTOM, 6)

        warnings_grid = wx.FlexGridSizer(0, 2, 6, 8)
        warnings_grid.AddGrowableCol(1, 1)

        lbl_buzzer = wx.StaticText(self.panel, label="&Check Engine Buzzer Level (dBFS):")
        self.spin_buzzer_level = wx.SpinCtrlDouble(self.panel, min=-120.0, max=0.0, inc=1.0)
        self.spin_buzzer_level.SetDigits(1)
        self.spin_buzzer_level.SetToolTip("Level for the check engine warning sound.")
        self.spin_buzzer_level.SetName("Check Engine Buzzer Level")
        _label_spin_double(self.spin_buzzer_level, "Check Engine Buzzer Level")
        warnings_grid.Add(lbl_buzzer, 0, wx.ALIGN_CENTER_VERTICAL)
        warnings_grid.Add(self.spin_buzzer_level, 0, wx.EXPAND)

        lbl_chime = wx.StaticText(self.panel, label="&Oil Warning Chime Level (dBFS):")
        self.spin_chime_level = wx.SpinCtrlDouble(self.panel, min=-120.0, max=0.0, inc=1.0)
        self.spin_chime_level.SetDigits(1)
        self.spin_chime_level.SetToolTip("Level for the oil pressure warning chime.")
        self.spin_chime_level.SetName("Oil Warning Chime Level")
        _label_spin_double(self.spin_chime_level, "Oil Warning Chime Level")
        warnings_grid.Add(lbl_chime, 0, wx.ALIGN_CENTER_VERTICAL)
        warnings_grid.Add(self.spin_chime_level, 0, wx.EXPAND)

        warnings_sizer.Add(warnings_grid, 0, wx.EXPAND | wx.ALL, 6)

        vbox.Add(warnings_sizer, 0, wx.LEFT | wx.RIGHT | wx.BOTTOM | wx.EXPAND, 10)

        # --- Audio Device Group ---
        sb_audio = wx.StaticBox(self.panel, label="Audio Device")
        audio_sizer = wx.StaticBoxSizer(sb_audio, wx.VERTICAL)
        self.chk_follow_device = wx.CheckBox(self.panel, label="F&ollow Default Audio Device")
        self.chk_follow_device.SetName("Follow Default Audio Device")
        self.chk_follow_device.SetToolTip("Automatically switch the audio output when the OS default device changes (recommended).")
        audio_sizer.Add(self.chk_follow_device, 0, wx.ALL, 6)
        
        hostapi_row = wx.FlexGridSizer(0, 2, 6, 8)
        hostapi_row.AddGrowableCol(1, 1)
        lbl_hostapi = wx.StaticText(self.panel, label="Preferred &Host API:")
        self.txt_hostapi = wx.TextCtrl(self.panel)
        self.txt_hostapi.SetName("Preferred Host API")
        self.txt_hostapi.SetToolTip("Audio backend for device switching. 'WASAPI' is best for modern Windows.")
        hostapi_row.Add(lbl_hostapi, 0, wx.ALIGN_CENTER_VERTICAL)
        hostapi_row.Add(self.txt_hostapi, 1, wx.EXPAND)

        lbl_poll = wx.StaticText(self.panel, label="Device &Poll Interval (sec):")
        self.spin_poll = wx.SpinCtrlDouble(self.panel, min=0.1, max=10.0, inc=0.1)
        self.spin_poll.SetDigits(1)
        self.spin_poll.SetName("Device Poll Interval")
        _label_spin_double(self.spin_poll, "Device Poll Interval")
        self.spin_poll.SetToolTip("How often to check for default audio device changes (in seconds).")
        hostapi_row.Add(lbl_poll, 0, wx.ALIGN_CENTER_VERTICAL)
        hostapi_row.Add(self.spin_poll, 1, wx.EXPAND)
        audio_sizer.Add(hostapi_row, 0, wx.EXPAND | wx.LEFT | wx.RIGHT | wx.BOTTOM, 6)
        
        vbox.Add(audio_sizer, 0, wx.LEFT | wx.RIGHT | wx.BOTTOM | wx.EXPAND, 10)

        # --- Buttons ---
        btn_sizer = wx.BoxSizer(wx.HORIZONTAL)
        self.btn_save = wx.Button(self.panel, label="&Save")
        self.btn_reload = wx.Button(self.panel, label="Re&load")
        self.btn_reset = wx.Button(self.panel, label="&Reset to Defaults")
        self.btn_close = wx.Button(self.panel, label="E&xit")
        for b in (self.btn_save, self.btn_reload, self.btn_reset, self.btn_close):
            btn_sizer.Add(b, 0, wx.ALL, 5)
        vbox.Add(btn_sizer, 0, wx.ALIGN_RIGHT | wx.RIGHT | wx.BOTTOM, 6)

        self.panel.SetSizer(vbox)

        # Events
        self.chk_compass_highlight.Bind(wx.EVT_CHECKBOX, self.on_toggle_highlight)
        self.chk_hrtf_enabled.Bind(wx.EVT_CHECKBOX, self.on_toggle_hrtf)
        btn_refresh.Bind(wx.EVT_BUTTON, self.on_refresh_voices)
        btn_test.Bind(wx.EVT_BUTTON, self.on_test_voice)
        self.btn_test_tone.Bind(wx.EVT_BUTTON, self.on_test_tone)
        self.btn_save.Bind(wx.EVT_BUTTON, self.on_save)
        self.btn_reload.Bind(wx.EVT_BUTTON, self.on_reload)
        self.btn_reset.Bind(wx.EVT_BUTTON, self.on_reset)
        self.btn_close.Bind(wx.EVT_BUTTON, lambda evt: self.Close())

        # Populate
        self.voices = []
        self.load_into_controls(load_config())
        self.on_toggle_highlight(None) # Set initial visibility
        self.on_toggle_hrtf(None) # Set initial HRTF visibility
        self.on_refresh_voices(None)

        # Accessibility names
        sb_speech.SetName("Speech")
        sb_gen.SetName("General")
        sb_compass.SetName("Compass Clicks")
        sb_pitchroll.SetName("Pitch and Roll")
        sb_shift.SetName("Shift Tone")
        sb_warnings.SetName("Warning Sounds")
        sb_audio.SetName("Audio Device")

        self.Centre()

    def on_toggle_highlight(self, evt):
        is_enabled = self.chk_compass_highlight.IsChecked()
        # Find the sizer item for the spin control and its label to hide/show them
        # This assumes the sizer is the parent of the spin control
        sizer_item = self.spin_compass_highlight_nth.GetContainingSizer()
        if sizer_item:
            sizer_item.Show(is_enabled)
            self.spin_compass_highlight_nth.Enable(is_enabled)
        self.panel.Layout()

    def on_toggle_hrtf(self, evt):
        is_enabled = self.chk_hrtf_enabled.IsChecked()
        self.hrtf_grid.ShowItems(is_enabled)
        self.spin_hrtf_front_emphasis.Enable(is_enabled)
        self.spin_hrtf_distance_gain.Enable(is_enabled)
        self.panel.Layout()

    def load_into_controls(self, cfg):
        self.cur_cfg = cfg
        self.chk_force_sapi.SetValue(cfg.get("force_sapi", False))
        self.rb_units.SetSelection(1 if str(cfg.get("units", "imperial")).lower().startswith("m") else 0)
        self.rb_proto.SetSelection(1 if str(cfg.get("telemetry_protocol", "extended")).lower() == "outgauge" else 0)
        self.spin_rate.SetValue(cfg.get("sapi_rate", 0))
        self.spin_volume.SetValue(cfg.get("sapi_volume", 100))
        self.spin_freq.SetValue(cfg.get("shift_tone_frequency_hz", 880.0))
        self.spin_level.SetValue(cfg.get("shift_tone_level_dbfs", -12.0))
        self.spin_buzzer_level.SetValue(cfg.get("check_engine_buzzer_level_dbfs", -12.0))
        self.spin_chime_level.SetValue(cfg.get("oil_chime_level_dbfs", -12.0))
        self.spin_dwell.SetValue(cfg.get("neutral_dwell_ms", 300))
        self.spin_compass_interval.SetValue(cfg.get("compass_click_interval", 15))
        self.chk_compass_highlight.SetValue(cfg.get("compass_highlight_enabled", False))
        self.spin_compass_highlight_nth.SetValue(cfg.get("compass_highlight_nth_click", 6)) # MODIFIED
        self.spin_compass_click_level.SetValue(cfg.get("compass_click_level_dbfs", -6.0))
        self.spin_lowspeed_click_level.SetValue(cfg.get("lowspeed_click_level_dbfs", -14.0))
        self.chk_hrtf_enabled.SetValue(cfg.get("hrtf_enabled", True))
        self.spin_hrtf_front_emphasis.SetValue(cfg.get("hrtf_front_emphasis_db", -6.0))
        self.spin_hrtf_distance_gain.SetValue(cfg.get("hrtf_distance_gain_db", 0.0))
        self.chk_pitch_roll_enabled.SetValue(cfg.get("pitch_roll_tones_enabled", True))
        self.spin_pitch_roll_max.SetValue(cfg.get("pitch_roll_max_dbfs", -24.0))
        self.spin_pitch_roll_min.SetValue(cfg.get("pitch_roll_min_dbfs", -36.0))
        self.chk_oil_chime_enabled.SetValue(cfg.get("oil_chime_enabled", True))
        self.chk_tc_clicks_enabled.SetValue(cfg.get("tc_clicks_enabled", True))
        self.chk_follow_device.SetValue(cfg.get("follow_default_audio_device", True))
        self.txt_hostapi.SetValue(cfg.get("preferred_hostapi", "wasapi"))
        self.spin_poll.SetValue(cfg.get("audio_poll_interval_sec", 2.0))

    def controls_to_config(self):
        cfg = self.cur_cfg.copy()
        cfg["force_sapi"] = self.chk_force_sapi.GetValue()
        cfg["units"] = "metric" if self.rb_units.GetSelection() == 1 else "imperial"
        cfg["telemetry_protocol"] = "outgauge" if self.rb_proto.GetSelection() == 1 else "extended"
        cfg["sapi_rate"] = self.spin_rate.GetValue()
        cfg["sapi_volume"] = self.spin_volume.GetValue()
        cfg["shift_tone_frequency_hz"] = self.spin_freq.GetValue()
        cfg["shift_tone_level_dbfs"] = self.spin_level.GetValue()
        cfg["check_engine_buzzer_level_dbfs"] = self.spin_buzzer_level.GetValue()
        cfg["oil_chime_level_dbfs"] = self.spin_chime_level.GetValue()
        cfg["neutral_dwell_ms"] = self.spin_dwell.GetValue()
        cfg["compass_click_interval"] = self.spin_compass_interval.GetValue()
        cfg["compass_highlight_enabled"] = self.chk_compass_highlight.GetValue()
        cfg["compass_highlight_nth_click"] = self.spin_compass_highlight_nth.GetValue() # MODIFIED
        cfg["compass_click_level_dbfs"] = self.spin_compass_click_level.GetValue()
        cfg["lowspeed_click_level_dbfs"] = self.spin_lowspeed_click_level.GetValue()
        cfg["hrtf_enabled"] = self.chk_hrtf_enabled.GetValue()
        cfg["hrtf_front_emphasis_db"] = self.spin_hrtf_front_emphasis.GetValue()
        cfg["hrtf_distance_gain_db"] = self.spin_hrtf_distance_gain.GetValue()
        cfg["pitch_roll_tones_enabled"] = self.chk_pitch_roll_enabled.GetValue()
        cfg["pitch_roll_max_dbfs"] = self.spin_pitch_roll_max.GetValue()
        cfg["pitch_roll_min_dbfs"] = self.spin_pitch_roll_min.GetValue()
        cfg["oil_chime_enabled"] = self.chk_oil_chime_enabled.GetValue()
        cfg["tc_clicks_enabled"] = self.chk_tc_clicks_enabled.GetValue()
        cfg["follow_default_audio_device"] = self.chk_follow_device.GetValue()
        cfg["preferred_hostapi"] = self.txt_hostapi.GetValue().strip()
        cfg["audio_poll_interval_sec"] = self.spin_poll.GetValue()
        
        sel = self.choice_voice.GetSelection()
        cfg["sapi_voice_name"] = self.choice_voice.GetString(sel) if sel != wx.NOT_FOUND else ""
        return cfg

    def on_refresh_voices(self, evt):
        names = list_sapi_voices()
        self.voices = names[:]
        self.choice_voice.Clear()
        if names:
            self.choice_voice.AppendItems(names)
            want = (self.cur_cfg.get("sapi_voice_name", "") or "").strip().lower()
            sel_idx = 0
            if want:
                for i, nm in enumerate(names):
                    if nm.strip().lower() == want:
                        sel_idx = i
                        break
            self.choice_voice.SetSelection(sel_idx)
        else:
            self.choice_voice.Append("No SAPI voices found")
            self.choice_voice.SetSelection(0)

    def on_test_voice(self, evt):
        cfg = self.controls_to_config()
        ok, err = test_sapi_voice(
            cfg.get("sapi_voice_name", ""), cfg.get("sapi_rate", 0), cfg.get("sapi_volume", 100)
        )
        if not ok and err:
            wx.MessageBox(err, "SAPI Test", wx.OK | wx.ICON_ERROR)
            
    def on_test_tone(self, evt):
        freq = self.spin_freq.GetValue()
        level = self.spin_level.GetValue()
        play_test_tone(freq, level)

    def on_save(self, evt):
        try:
            cfg = self.controls_to_config()
            _write_config(CONFIG_PATH, cfg)
            self.load_into_controls(load_config())

            wx.MessageBox(f"Saved settings to:\n{CONFIG_PATH}", "Saved", wx.OK | wx.ICON_INFORMATION)
        except Exception as e:
            wx.MessageBox(f"Failed to save:\n{e}", "Error", wx.OK | wx.ICON_ERROR)

    def on_reload(self, evt):
        try:
            cfg = load_config()
            self.load_into_controls(cfg)
            self.on_refresh_voices(None)
            wx.MessageBox("Reloaded current configuration.", "Reloaded", wx.OK | wx.ICON_INFORMATION)
        except Exception as e:
            wx.MessageBox(f"Failed to reload:\n{e}", "Error", wx.OK | wx.ICON_ERROR)

    def on_reset(self, evt):
        try:
            self.load_into_controls(DEFAULT_CONFIG.copy())
            self.on_refresh_voices(None)
            wx.MessageBox("Reset controls to default values. Press Save to commit.", "Reset", wx.OK | wx.ICON_INFORMATION)
        except Exception as e:
            wx.MessageBox(f"Failed to reset:\n{e}", "Error", wx.OK | wx.ICON_ERROR)


def main():
    try:
        app = wx.App(False)
        frm = ConfigFrame()
        frm.Show()
        app.MainLoop()
    except Exception:
        print("--- WX UI FAILED TO INITIALIZE ---")
        print(traceback.format_exc())


if __name__ == "__main__":
    main()