# audio.py

import threading
import math
import time
import logging

try:
    import numpy as np
except ImportError:
    np = None
    logging.warning("numpy module not found. Audio generation will be disabled.")

try:
    import sounddevice as sd
    AUDIO_OK = True
except ImportError:
    AUDIO_OK = False
    logging.warning("sounddevice unavailable – 'pip install sounddevice' to enable audio cues.")

try:
    from scanner_hrtf_diag import ScannerHRTFDiagnostic
except Exception:
    ScannerHRTFDiagnostic = None

DEFAULT_SR = 48000

# =========================
#  Audio Constants
# =========================
# Compass Click
CLICK_DUR_MS = 20

# Attitude Cues
ATT_START_DEG, ATT_STOP_DEG = 8.0, 5.0
ROLL_CAP_DEG, PITCH_CAP_DEG = 60.0, 30.0
ATT_MAX_DBFS, ATT_MIN_DBFS = -24.0, -36.0
ATT_BASE_ROLL_HZ, ATT_BASE_PITCH_HZ = 220.0, 300.0
ATT_SMOOTH_MS = 80.0

# Attitude Fade Constants (Stability Check)
ATT_FADE_MIN_GAIN = 0.04       # Target gain when stable (approx -28dB drop)
ATT_FADE_SENSITIVITY = 50.0    # Sensitivity to variation (Higher = easier to trigger max vol)
ATT_FADE_ATTACK_MS = 20.0      # Time to reach full volume on instability
ATT_FADE_DECAY_MS = 1250.0     # Time to fade out when stable
ATT_HRTF_FAR_DB = -10.0       # dB attenuation at maximum roll (farthest perceived distance)

# Pedal Tones
PEDAL_BASE_FREQ = 300.0
PEDAL_MAX_MULT = 4.0
PEDAL_SMOOTH_MS = 60.0

# Steering Tone
STEER_BASE_FREQ = 100.0
STEER_MAX_FREQ = 400.0
STEER_AMP_DB = -20.0

# NEW: Drift Detection Constants
DRIFT_FREQ_HZ = 180.0
DRIFT_AMP_DB = -15.0

# NEW: Vehicle Scanner Constants
SCANNER_BEEP_DUR_MS = 60       # Duration of a single beep
SCANNER_MAX_RATE_HZ = 10.0     # Beeps per second at close range
SCANNER_MIN_RATE_HZ = 0.5      # Beeps per second at long range
SCANNER_HALF_DIST_BASE_M = 20.0  # Half-distance at standstill (meters)
SCANNER_HALF_DIST_PER_MS = 3.0   # Additional half-distance per m/s of vehicle speed
SCANNER_BEEP_FREQ_HZ = 1000.0  # Intrinsic render frequency of the beep waveform (pitch reference)
SCANNER_ALIGN_THRESHOLD_DEG = 1.5  # Bearing threshold for alignment tone (bright waveform)
# Scanner pitch model: frequency = base_freq * 2^(offset_oct * proximity), where proximity is 1 at
# dead-center and 0 directly behind. base_freq and offset_oct are user-configurable (see apply_config).
# proximity blends a broad linear ramp (back->front) with a sharp alignment term so the pitch climbs
# continuously AND emphasises dead-center. The decay must be wide enough that the climb is trackable
# across a usable band — a narrow spike is only sampled when a beep fires within ~1deg of center.
SCANNER_ALIGN_PITCH_DECAY_DEG = 6.0  # Decay constant of the sharp alignment term (~15deg usable band)
SCANNER_ALIGN_WEIGHT = 0.5           # Share of the pitch rise from the sharp alignment term vs broad ramp
SCANNER_BASE_FREQ_DEFAULT = 1000.0   # Default base (resting) frequency, Hz
SCANNER_OFFSET_OCT_DEFAULT = 1.0     # Default max pitch rise at alignment, octaves
SCANNER_OFFSET_OCT_MIN = 0.5         # Minimum allowed offset (half an octave above base)
SCANNER_OFFSET_OCT_MAX = 2.0         # Maximum allowed offset (two octaves above base)
# Beep rate is driven by CLOSING speed (rate of change of distance), not raw speed, so heading away
# from the target slows the beeps. Half-distance grows when approaching and shrinks when receding;
# clamped to a positive minimum so the exp() denominator stays valid.
SCANNER_HALF_DIST_MIN_M = 4.0
SCANNER_CLOSING_SMOOTH = 0.25  # EMA factor for closing-speed estimate (per scanner packet)
SCANNER_CLOSING_MAX_MS = 60.0  # Clamp implausible distance jumps (e.g. on target switch)
# Beep rate is driven by CLOSING speed (rate of change of distance), not raw speed, so heading away
# from the target slows the beeps. Half-distance grows when approaching and shrinks when receding;
# clamped to a positive minimum so the exp() denominator stays valid.
SCANNER_HALF_DIST_MIN_M = 4.0
SCANNER_CLOSING_SMOOTH = 0.25  # EMA factor for closing-speed estimate (per scanner packet)
SCANNER_CLOSING_MAX_MS = 60.0  # Clamp implausible distance jumps (e.g. on target switch)
# Steering-locked steady tone: while the player is steering (non-zero input), the beep train morphs
# into a continuous HRTF-positioned tone so the target's direction can be locked in even when beeps
# are slow (far away). Returns to normal beeps as steering re-centers.
SCANNER_STEER_TONE_DEADZONE = 0.04  # |steer| below this => pure beeps
SCANNER_STEER_TONE_FULL     = 0.45  # |steer| at/above this => fully steady tone
SCANNER_TONE_LEVEL_SMOOTH   = 0.12  # per-block one-pole smoothing of the morph level (click-free)
SCANNER_TONE_AMP            = 0.12  # peak amplitude of the steady tone

# Coupler Tracking Constants
COUPLER_TONE_FREQ_HZ = 660.0      # Base frequency (E5, distinct from scanner/obstacle)
COUPLER_TONE_AMP_DB = -18.0       # Volume level
COUPLER_MIN_PITCH = 0.8           # Pitch multiplier at max distance
COUPLER_MAX_PITCH = 2.0           # Pitch multiplier at close range
COUPLER_BEEP_PITCH = 2.5          # Pitch multiplier in coupling range
COUPLER_HALF_DIST_M = 10.0        # Distance for half pitch scaling
COUPLER_RANGE_M = 1.5             # Coupling range threshold
COUPLER_BEEP_DUR_MS = 40          # Beep duration when in range
COUPLER_BEEP_MAX_RATE = 20.0      # Max beeps/sec at very close range
COUPLER_BEEP_MIN_RATE = 4.0       # Min beeps/sec at edge of range

# NEW: Obstacle Detection Constants
OBSTACLE_BUZZ_FREQ_HZ = 440.0     # Base frequency of the square wave buzzer
OBSTACLE_BUZZ_DUR_MS = 80         # Duration of each buzz pulse
OBSTACLE_MAX_RATE_HZ = 15.0       # Buzzes/sec at very close range
OBSTACLE_MIN_RATE_HZ = 1.0        # Buzzes/sec at max detection range
OBSTACLE_HALF_DIST_M = 8.0        # Distance for half-rate (exponential curve)
OBSTACLE_AMP_DB = -12.0           # Volume level
OBSTACLE_CONTINUOUS_DIST = 1.0    # Below this distance, play continuous tone
NUM_OBSTACLE_QUADRANTS = 12       # Max simultaneous clustered obstacles (matches NUM_RAYS in lua)

# Terrain Warning Constants
TERRAIN_SWEEP_DUR_MS = 200        # Duration of terrain warning sweep
TERRAIN_SWEEP_AMP_DB = -14.0      # Volume level for terrain sweeps
DROPOFF_FREQ_START = 500.0        # Descending sweep start freq
DROPOFF_FREQ_END = 200.0          # Descending sweep end freq
HILL_FREQ_START = 200.0           # Ascending sweep start freq
HILL_FREQ_END = 500.0             # Ascending sweep end freq

# NEW: Low Speed Detection Constants
LS_CLICK_DUR_MS = 80
LS_CARRIER_HZ = 250.0
LS_MOD_HZ = 375.0
LS_MOD_INDEX_PEAK = 4.0
LS_AMP_DB = -14.0
LS_MIN_RATE_HZ = 1.0
LS_MAX_RATE_HZ = 8.0
LS_DECEL_FOR_MAX_RATE = 5.0
LS_PITCH_AT_0MPH = 0.7
LS_PITCH_AT_25MPH = 1.8
LS_FADE_ATTACK_S = 0.05
LS_FADE_DECAY_S = 0.30
LS_HRTF_AZIMUTH = 0.0

# NEW: Heading Guidance Constants
GUIDANCE_FREQ_HZ = 440.0       # Steady tone frequency
GUIDANCE_DEADZONE_DEG = 0.5    # No sound if error is within +/- this amount
GUIDANCE_FULL_SCALE_DEG = 5.0  # Error amount for max volume/pan
GUIDANCE_MIN_DBFS = -40.0      # Volume just outside deadzone
GUIDANCE_MAX_DBFS = -12.0      # Volume at full scale error

# Hydraulic steering misalignment tone
# Plays when wheels are off-centre (actual_steering ≠ 0) and driver is not actively steering.
# actual_steering = 0.0 for all non-hydraulic vehicles → auto-detects hydraulic vehicles.
HYDRO_STEER_TONE_HZ    = 330.0   # pure sine, low but audible
HYDRO_STEER_AMP_DB     = -18.0   # peak dBFS
HYDRO_STEER_DEADZONE   = 0.05    # |actual_steering| below this = silent (wheels nearly straight)
HYDRO_STEER_FULL       = 0.35    # |actual_steering| at which full amplitude is reached
HYDRO_STEER_INPUT_DEAD = 0.08    # |steering_input| below this = "driver not actively steering"

# Node Grabber Hover Beep
NODE_BEEP_BASE_FREQ_HZ = 800.0   # Base frequency at height 0.5
NODE_BEEP_LOW_FREQ_HZ = 600.0    # Frequency at underside (height 0.0)
NODE_BEEP_HIGH_FREQ_HZ = 1400.0  # Frequency at top (height 1.0)
NODE_BEEP_DUR_MS = 40             # Duration of the beep
NODE_BEEP_AMP_DB = -14.0          # Volume level

# Clickspot Hover Beep (FM click sound)
CLICKSPOT_BEEP_FREQ_HZ = 770.0    # Carrier frequency
CLICKSPOT_BEEP_MOD_HZ = 1540.0    # Modulator frequency (2x carrier for metallic click)
CLICKSPOT_BEEP_MOD_INDEX = 4.0    # FM modulation index (higher = more harmonics)
CLICKSPOT_BEEP_DUR_MS = 18        # Duration
CLICKSPOT_BEEP_AMP_DB = -12.0     # Volume

# Road Detection (off-road guidance beep) — F#5 carrier with FM funk modulation
ROAD_BEEP_FREQ_HZ      = 739.99   # F#5 carrier
ROAD_BEEP_MOD_HZ       = 369.99   # sub-octave modulator (1:2 ratio — warm, harmonic)
ROAD_BEEP_MOD_INDEX    = 1.8      # FM β — moderate warmth, not paint-peeling
ROAD_BEEP_DUR_MS       = 140      # pulse length
ROAD_BEEP_AMP_DB       = -14.0    # default volume (overridable via config)
ROAD_BEEP_MIN_RATE_HZ  = 0.7      # pulses/sec when far from road
ROAD_BEEP_MAX_RATE_HZ  = 4.0      # pulses/sec when right next to road
ROAD_BEEP_HALF_DIST_M  = 20.0     # exponential half-distance for rate scaling

# Road Orientation Chime (one-shot two-tone cue when transitioning onto a road).
# First tone is panned to the smaller-|bearing| travel direction (more "ahead-ish") at the
# higher pitch; second tone is panned to the opposite direction at the lower pitch.
# Chosen as a clean perfect fourth so the chime is recognizable and pleasant.
ROAD_CHIME_FWD_FREQ_HZ  = 783.99   # G5 — played first, panned to forward-ish road direction
ROAD_CHIME_BACK_FREQ_HZ = 523.25   # C5 — played second, panned to backward-ish road direction
ROAD_CHIME_DUR_MS       = 180
ROAD_CHIME_GAP_MS       = 120      # silence between the two tones (raw, before HRTF tail)
ROAD_CHIME_AMP_DB       = -12.0
ROAD_CHIME_REPEAT_GAP_MS = 320     # silence between chime repetitions when it loops
# FM modulator profile: sharp click at the attack, decaying to a low sustained index. The
# modulator runs at the carrier frequency (1:1 ratio → harmonic spectrum). The peak gives the
# tone a percussive "clack" attack; the floor adds a hint of warmth/buzz on the sustain.
ROAD_CHIME_MOD_INDEX_PEAK  = 5.5
ROAD_CHIME_MOD_INDEX_FLOOR = 1.1
ROAD_CHIME_MOD_DECAY       = 90.0  # 1/e decay constant — ~11 ms time constant, ~50 ms to settle

# Coordinate Guidance FM tone
COORD_GUIDE_FC_ONCOURSE_HZ  = 440.0   # Carrier Hz when on course (amp is ~0 anyway)
COORD_GUIDE_FC_OFFCOURSE_HZ = 220.0   # Carrier Hz when 180° off course
COORD_GUIDE_FM_RATIO_MIN    = 0.25    # fm/fc ratio when on course (quasi-harmonic)
COORD_GUIDE_FM_RATIO_MAX    = 1.618   # fm/fc ratio when 180° off (golden ratio → inharmonic)
COORD_GUIDE_MOD_INDEX_MAX   = 3.0     # FM β (modulation index) at 180° off course
COORD_GUIDE_AMP_DB          = -12.0   # Peak amplitude at 180° off course
COORD_GUIDE_DEADZONE_DEG    = 5.0     # Silent within this bearing error

class AudioController:
    def __init__(self, logger):
        self.logger = logger
        if not AUDIO_OK or np is None:
            self.logger.error("Audio disabled due to missing numpy or sounddevice.")
            self._is_enabled = False
            return
        self._is_enabled = True

        # Internal State
        self.lock = threading.Lock()
        self._stop_event = threading.Event()
        
        self.samplerate = DEFAULT_SR

        # Waveforms are now initialized to None.
        self.CLICK_WAVEFORM = None
        self.TC_TONE_WAVEFORM = None
        self.CHECK_ENGINE_BUZZER_WAVEFORM = None
        self.OIL_CHIME_WAVEFORM = None
        self.SCANNER_BEEP_WAVEFORM = None # NEW: Scanner beep waveform
        self.SCANNER_ALIGNED_WAVEFORM = None  # Bright variant for good alignment
        self.OBSTACLE_BUZZ_WAVEFORM = None  # Obstacle detection square wave
        self.DROPOFF_SWEEP_WAVEFORM = None  # Terrain drop-off warning
        self.HILL_SWEEP_WAVEFORM = None     # Steep hill warning
        self.ROAD_BEEP_WAVEFORM = None      # Off-road guidance FM beep
        self.ROAD_CHIME_FWD_WAVEFORM = None # G5 orientation chime tone (forward-ish direction)
        self.ROAD_CHIME_BACK_WAVEFORM = None # C5 orientation chime tone (backward-ish direction)
        self.CAM_CLICK_WAVEFORM = None
        self.CAM_HIGHLIGHT_CLICK_WAVEFORM = None
        self.NODE_BEEP_WAVEFORM = None  # Node grabber hover beep
        self.CLICKSPOT_BEEP_WAVEFORM = None      # Clickspot hover beep (forward)
        self.CLICKSPOT_BEEP_REV_WAVEFORM = None  # Clickspot hover beep (reverse/leaving)
        self.DESCRIBE_ERROR_WAVEFORM = None      # Low FM buzz: AI describer double-press

        # Scanner/HRTF correlation diagnostic (no-op unless BEAM_SCANNER_DIAG is set)
        self._scanner_diag = (
            ScannerHRTFDiagnostic(self.logger) if ScannerHRTFDiagnostic is not None else None
        )

        # HRTF State
        self._hrtf = None
        self._click_conv_L = None
        self._click_conv_R = None
        self._hclick_conv_L = None
        self._hclick_conv_R = None
        self._click_use_hrtf = False
        self._hclick_use_hrtf = False
        self._hrtf_front_emphasis_db = -6.0  # dB attenuation at 180° (behind)
        self._hrtf_user_enabled = True
        self._hrtf_distance_gain = 1.0  # linear gain from hrtf_distance_gain_db
        self._roll_hrtf_overlap_L = None  # overlap-add tail for HRTF roll tone
        self._roll_hrtf_overlap_R = None
        self._pitch_hrtf_overlap_L = None  # overlap-add tail for HRTF pitch tone
        self._pitch_hrtf_overlap_R = None

        # Configurable enable flags
        self._tc_clicks_enabled = True
        self._pitch_roll_enabled = True

        # Configurable volume params
        self._att_max_dbfs = ATT_MAX_DBFS
        self._att_min_dbfs = ATT_MIN_DBFS
        self._compass_click_amp = 0.5  # linear, approx -6 dBFS
        self._ls_click_amp_db = LS_AMP_DB
        self._obstacle_amp = 10.0 ** (OBSTACLE_AMP_DB / 20.0)

        # Playback State
        self._click_playback_pos = -1.0
        self._click_pan_pos = 0.0
        self._click_pitch_mult = 1.0
        self._highlight_click_playback_pos = -1.0
        self._highlight_click_pan_pos = 0.0
        self._highlight_click_pitch_mult = 1.0
        self._tc_playback_pos = -1.0
        self._buzzer_playback_pos = -1.0
        self._chime_playback_pos = -1.0
        self._describe_error_playback_pos = -1.0
        self._click_request = threading.Event()
        self._highlight_click_request = threading.Event()
        self._scanner_playback_pos = -1.0
        self._node_beep_playback_pos = -1.0
        self._node_beep_freq = NODE_BEEP_BASE_FREQ_HZ
        self._node_beep_reverse = False
        self._clickspot_beep_playback_pos = -1.0
        self._clickspot_beep_reverse = False  # True = play waveform in reverse

        # Camera compass click state
        self._cam_click_playback_pos = -1.0
        self._cam_click_pan_pos = 0.0
        self._cam_click_pitch_mult = 1.0
        self._cam_highlight_click_playback_pos = -1.0
        self._cam_highlight_click_pan_pos = 0.0
        self._cam_highlight_click_pitch_mult = 1.0
        self._cam_click_request = threading.Event()
        self._cam_highlight_click_request = threading.Event()
        self._cam_click_conv_L = None
        self._cam_click_conv_R = None
        self._cam_hclick_conv_L = None
        self._cam_hclick_conv_R = None
        self._cam_click_use_hrtf = False
        self._cam_hclick_use_hrtf = False

        # Shared state from main telemetry loop
        self.shift_active = False
        self.pedal_tones_active = False
        self.tc_active = False
        self.last_clutch = 0.0
        self.last_brake = 0.0
        self.last_throttle = 0.0
        self.last_steering = 0.0 # NEW
        self.inverted = False
        self.last_roll_rad = 0.0
        self.last_pitch_rad = 0.0
        
        # NEW: Vehicle Scanner State
        self._scan_mode_active = False
        self._scanner_target_bearing = 0.0
        self._scanner_target_distance = float('inf')
        self._scan_speed_ms = 0.0
        self._scanner_beep_timer = 0.0
        self._scanner_overlap_L = None
        self._scanner_overlap_R = None
        # Closing-speed estimate (positive = approaching) from successive distance samples
        self._scan_closing_ms = 0.0
        self._scan_prev_dist = None
        self._scan_prev_dist_t = 0.0
        # Scanner pitch / steady-tone config (overwritten by apply_config)
        self._scan_base_freq = SCANNER_BASE_FREQ_DEFAULT
        self._scan_offset_oct = SCANNER_OFFSET_OCT_DEFAULT
        self._scan_steer_tone_enabled = True
        # Steering-locked steady tone state
        self._scan_tone_level = 0.0     # smoothed morph factor 0..1 (0=beeps, 1=steady tone)
        self._scan_tone_phase = 0.0     # continuous oscillator phase, cycles
        self._scan_tone_overlap_L = None
        self._scan_tone_overlap_R = None

        # Coupler Tracking State
        self._coupler_active = False
        self._coupler_bearing = 0.0
        self._coupler_distance = float('inf')
        self._coupler_in_range = False
        self._coupler_phase = 0.0
        self._coupler_beep_timer = 0.0
        self._coupler_overlap_L = None
        self._coupler_overlap_R = None

        # NEW: Obstacle Detection State
        self._obstacle_mode_active = False
        # Per-quadrant state: [front-right, rear-right, rear-left, front-left]
        self._obstacle_bearings = [0.0] * NUM_OBSTACLE_QUADRANTS
        self._obstacle_distances = [float('inf')] * NUM_OBSTACLE_QUADRANTS
        self._obstacle_types = [0] * NUM_OBSTACLE_QUADRANTS  # 0=none, 1=static, 2=dropoff, 3=hill
        self._obstacle_buzz_timers = [0.0] * NUM_OBSTACLE_QUADRANTS
        self._obstacle_playback_pos = [-1.0] * NUM_OBSTACLE_QUADRANTS
        # Per-quadrant pre-rendered pulse buffers (HRTF-convolved or stereo-panned)
        self._obstacle_pulse_L = [None] * NUM_OBSTACLE_QUADRANTS
        self._obstacle_pulse_R = [None] * NUM_OBSTACLE_QUADRANTS
        self._terrain_playback_pos = -1.0
        self._terrain_type = 0  # 0=none, 2=dropoff, 3=hill

        # Road Detection State (off-road guidance beep)
        self._road_mode_active = False
        self._road_on_road = True       # last reported state — start "on" so no beeps until told otherwise
        self._road_bearing = 0.0
        self._road_distance = 0.0
        self._road_beep_timer = 0.0
        self._road_playback_pos = -1.0
        self._road_pulse_L = None
        self._road_pulse_R = None
        self._road_amp = 10.0 ** (-14.0 / 20.0)
        # Orientation chime: list of pending pulses {"delay": int_samples, "L": np.ndarray, "R": np.ndarray, "pos": int}
        self._road_chime_queue = []
        # Earliest monotonic time at which a new chime is allowed to start. Used to pace
        # repeated chime triggers from the lua side so successive pairs don't pile up.
        self._road_chime_next_allowed_time = 0.0

        # NEW: Heading Guidance State
        self._guidance_active = False
        self._guidance_error_deg = 0.0
        self._phase_guidance = 0.0
        self._sm_guidance_error = 0.0 # Smoothed error

        # Hydraulic steering misalignment tone state
        self._hydro_actual_steer    = 0.0
        self._hydro_steer_input     = 0.0
        self._hydro_steer_phase     = 0.0
        self._hydro_steer_overlap_L = None
        self._hydro_steer_overlap_R = None

        # Coordinate Guidance FM state
        self._coord_guidance_active = False
        self._coord_guidance_error = 0.0
        self._sm_coord_error = 0.0
        self._coord_phase_carrier = 0.0
        self._coord_phase_mod = 0.0
        self._coord_hrtf_overlap_L = None
        self._coord_hrtf_overlap_R = None
        
        # NEW: Drift Detection State
        self._drift_alert_active = False
        self._drift_pan = 0.0
        self._phase_drift = 0.0
        self._drift_rate = 0.0

        # NEW: Low Speed Detection State
        self._ls_clicks_active = False
        self._ls_speed_mph = 0.0
        self._ls_decel = 0.0
        self._ls_click_timer = 0.0
        self._ls_playback_pos = -1.0
        self._ls_fade_gain = 0.0
        self._ls_conv_L = None
        self._ls_conv_R = None
        self._ls_use_hrtf = False
        self._ls_pitch_mult = 1.0

        # Audio Stream and Device Management
        self._audio_stream = None
        self._device_watcher_thread = None
        self._current_device_index = None
        self._current_device_name = None
        self._preferred_device_name = ""
        self._follow_default_enabled = True
        self._audio_poll_interval = 2.0
        
        # Synthesis state (phases, smoothers)
        self._phase_shift = 0.0
        self._sm_roll_int, self._sm_pitch_int = 0.0, 0.0
        self._phase_roll, self._phase_pitch = 0.0, 0.0
        self._roll_fade_mult, self._pitch_fade_mult = 1.0, 1.0
        self._sm_c, self._sm_b, self._sm_t = 0.0, 0.0, 0.0
        _rng = np.random.default_rng()
        self._detune_c, self._detune_b, self._detune_t = _rng.uniform(-3.0, 3.0, 3)
        self._phase_c, self._phase_b, self._phase_t = 0.0, 0.0, 0.0
        self._phase_steer = 0.0 # NEW
        
        # Configurable params
        self.shift_freq = 880.0
        self.shift_amp = 10.0 ** (-12.0 / 20.0)
        self._phase_inc_shift = self.shift_freq / self.samplerate
        self.buzzer_amp = 1.0 ** (-12.0 / 20.0)
        self.chime_amp = 1.0 ** (-12.0 / 20.0)

    # NEW: Control methods for the vehicle scanner
    def set_scan_mode(self, is_active):
        with self.lock:
            self._scan_mode_active = bool(is_active)
            if not self._scan_mode_active:
                # Reset distance when turned off so it doesn't beep once on reactivation
                self._scanner_target_distance = float('inf')
    
    def update_scanner_target(self, bearing, distance):
        with self.lock:
            self._scanner_target_bearing = float(bearing)
            distance = float(distance)
            self._scanner_target_distance = distance

            # Estimate closing speed (positive = approaching) from the change in distance.
            # This captures actual direction of travel relative to the target — driving away
            # yields a negative value that slows the beep rate, regardless of vehicle speed.
            now = time.perf_counter()
            prev = self._scan_prev_dist
            if prev is not None and math.isfinite(prev) and math.isfinite(distance):
                dt = now - self._scan_prev_dist_t
                if 1e-3 < dt < 1.0:
                    raw = (prev - distance) / dt
                    raw = max(-SCANNER_CLOSING_MAX_MS, min(SCANNER_CLOSING_MAX_MS, raw))
                    self._scan_closing_ms += SCANNER_CLOSING_SMOOTH * (raw - self._scan_closing_ms)
            if not math.isfinite(distance):
                self._scan_closing_ms = 0.0
            self._scan_prev_dist = distance if math.isfinite(distance) else None
            self._scan_prev_dist_t = now

    # Coupler tracking control methods
    def set_coupler_tracking(self, active):
        with self.lock:
            self._coupler_active = bool(active)
            if not active:
                self._coupler_distance = float('inf')
                self._coupler_in_range = False
                self._coupler_phase = 0.0
                self._coupler_beep_timer = 0.0
                self._coupler_overlap_L = None
                self._coupler_overlap_R = None

    def update_coupler_target(self, bearing, distance, in_range):
        with self.lock:
            self._coupler_bearing = float(bearing)
            self._coupler_distance = float(distance)
            self._coupler_in_range = bool(in_range)

    # NEW: Control methods for obstacle detection
    def set_obstacle_mode(self, is_active):
        with self.lock:
            self._obstacle_mode_active = bool(is_active)
            if not self._obstacle_mode_active:
                for i in range(NUM_OBSTACLE_QUADRANTS):
                    self._obstacle_distances[i] = float('inf')
                    self._obstacle_types[i] = 0
                    self._obstacle_playback_pos[i] = -1.0
                self._terrain_playback_pos = -1.0
                self._terrain_type = 0

    def update_obstacle(self, obstacle_type, bearing, urgency, distance):
        """Terrain-only update path (types 2=dropoff, 3=hill). Static obstacles
        come in via update_static_obstacles."""
        with self.lock:
            if obstacle_type in (2, 3):
                self._terrain_type = obstacle_type
                self._terrain_playback_pos = 0.0  # trigger playback

    def update_static_obstacles(self, obstacles):
        """Replace all static-obstacle slots with the given list.
        obstacles is an iterable of (bearing, urgency, distance) triples — one per
        clustered obstacle from the lua sweep. Slots beyond len(obstacles) become inactive.
        Slot identity is preserved by index so a stable cluster keeps the same playback
        state across sweeps (no retrigger glitch)."""
        with self.lock:
            obs_list = list(obstacles)[:NUM_OBSTACLE_QUADRANTS]
            for i in range(NUM_OBSTACLE_QUADRANTS):
                if i < len(obs_list):
                    bearing, _urgency, distance = obs_list[i]
                    self._obstacle_bearings[i] = float(bearing)
                    self._obstacle_distances[i] = float(distance)
                    self._obstacle_types[i] = 1
                else:
                    self._obstacle_distances[i] = float('inf')
                    self._obstacle_types[i] = 0

    def set_road_mode(self, is_active):
        with self.lock:
            self._road_mode_active = bool(is_active)
            if not self._road_mode_active:
                self._road_on_road = True
                self._road_beep_timer = 0.0
                self._road_playback_pos = -1.0
                self._road_pulse_L = None
                self._road_pulse_R = None
                self._road_chime_queue = []
                self._road_chime_next_allowed_time = 0.0

    def _render_directional_pulse(self, base_pulse, bearing_deg):
        """HRTF-convolve (or stereo-pan as fallback) a mono pulse at the given bearing."""
        use_hrtf = self._hrtf is not None and self._hrtf_user_enabled
        if use_hrtf:
            ir_l, ir_r = self._hrtf.get_hrir(bearing_deg % 360.0)
            if ir_l is not None:
                L = np.convolve(base_pulse, ir_l, mode='full').astype(np.float32)
                R = np.convolve(base_pulse, ir_r, mode='full').astype(np.float32)
                return L, R
        pan_pos = -bearing_deg / 90.0
        Lg, Rg = self._pan_gains(pan_pos)
        return (base_pulse * Lg).astype(np.float32), (base_pulse * Rg).astype(np.float32)

    def trigger_road_orientation_chime(self, bearing_first, bearing_second):
        """Schedule (or re-schedule) the two-tone orientation chime: G5 panned to bearing_first,
        then C5 panned to bearing_second after a brief gap. bearing_first is expected to be the
        smaller-|bearing| direction (closer to forward).

        Rate-limited: lua sends this every tick while the chime should be playing, but we only
        actually schedule a new chime once the previous one (plus a brief inter-repeat gap) has
        finished, so successive pairs don't smear into each other."""
        if not self._is_enabled:
            return
        first = self.ROAD_CHIME_FWD_WAVEFORM
        second = self.ROAD_CHIME_BACK_WAVEFORM
        if first is None or second is None:
            return

        now = time.monotonic()
        if now < self._road_chime_next_allowed_time:
            return  # previous chime still in flight; ignore retrigger

        first_L, first_R = self._render_directional_pulse(first, float(bearing_first))
        second_L, second_R = self._render_directional_pulse(second, float(bearing_second))

        # Second pulse starts after the first tone's body + gap (raw timing, not HRTF tail).
        raw_dur_samples = int(self.samplerate * ROAD_CHIME_DUR_MS / 1000.0)
        gap_samples = int(self.samplerate * ROAD_CHIME_GAP_MS / 1000.0)
        second_delay = raw_dur_samples + gap_samples

        # Reserve a window for the full chime + inter-repeat gap before another can start.
        total_ms = (2 * ROAD_CHIME_DUR_MS) + ROAD_CHIME_GAP_MS + ROAD_CHIME_REPEAT_GAP_MS
        self._road_chime_next_allowed_time = now + (total_ms / 1000.0)

        with self.lock:
            self._road_chime_queue = [
                {"delay": 0, "L": first_L, "R": first_R, "pos": 0},
                {"delay": second_delay, "L": second_L, "R": second_R, "pos": 0},
            ]

    def update_road_state(self, on_road, bearing, distance):
        with self.lock:
            was_off = not self._road_on_road
            self._road_on_road = bool(on_road)
            self._road_bearing = float(bearing)
            self._road_distance = float(distance)
            if self._road_on_road and was_off:
                # Just rejoined the road — kill any pending pulse
                self._road_playback_pos = -1.0
                self._road_pulse_L = None
                self._road_pulse_R = None

    def clear_obstacles(self):
        """Clear all obstacle data (no obstacles detected)."""
        with self.lock:
            for i in range(NUM_OBSTACLE_QUADRANTS):
                self._obstacle_distances[i] = float('inf')
                self._obstacle_types[i] = 0

    def apply_config(self, cfg):
        if not self._is_enabled: return

        db = float(cfg.get("shift_tone_level_dbfs", -12.0))
        db = min(0.0, max(-120.0, db))
        self.shift_freq = max(20.0, min(20000.0, float(cfg.get("shift_tone_frequency_hz", 880.0))))
        self.shift_amp = float(10.0 ** (db / 20.0))
        self._phase_inc_shift = self.shift_freq / self.samplerate

        buzzer_db = float(cfg.get("check_engine_buzzer_level_dbfs", -12.0))
        self.buzzer_amp = float(10.0 ** (buzzer_db / 20.0))

        chime_db = float(cfg.get("oil_chime_level_dbfs", -12.0))
        self.chime_amp = float(10.0 ** (chime_db / 20.0))

        self.CHECK_ENGINE_BUZZER_WAVEFORM = self._generate_check_engine_buzzer(self.buzzer_amp)
        self.OIL_CHIME_WAVEFORM = self._generate_oil_chime(self.chime_amp)

        self._hrtf_front_emphasis_db = min(0.0, float(cfg.get("hrtf_front_emphasis_db", -6.0)))
        self._hrtf_user_enabled = bool(cfg.get("hrtf_enabled", True))
        dist_db = float(cfg.get("hrtf_distance_gain_db", 0.0))
        self._hrtf_distance_gain = float(10.0 ** (dist_db / 20.0))

        # Vehicle scanner pitch + steering-locked steady tone
        self._scan_steer_tone_enabled = bool(cfg.get("scanner_steer_tone_enabled", True))
        self._scan_base_freq = max(100.0, min(8000.0,
            float(cfg.get("scanner_base_freq_hz", SCANNER_BASE_FREQ_DEFAULT))))
        self._scan_offset_oct = max(SCANNER_OFFSET_OCT_MIN, min(SCANNER_OFFSET_OCT_MAX,
            float(cfg.get("scanner_pitch_offset_oct", SCANNER_OFFSET_OCT_DEFAULT))))

        # Enable flags
        self._tc_clicks_enabled = bool(cfg.get("tc_clicks_enabled", True))
        self._pitch_roll_enabled = bool(cfg.get("pitch_roll_tones_enabled", True))

        # Attitude tone volume range
        self._att_max_dbfs = float(cfg.get("pitch_roll_max_dbfs", -24.0))
        self._att_min_dbfs = float(cfg.get("pitch_roll_min_dbfs", -36.0))

        # Compass click amplitude
        cc_db = float(cfg.get("compass_click_level_dbfs", -6.0))
        self._compass_click_amp = float(10.0 ** (cc_db / 20.0))

        # Low speed click amplitude
        self._ls_click_amp_db = float(cfg.get("lowspeed_click_level_dbfs", -14.0))

        # Obstacle detection volume
        obs_db = float(cfg.get("obstacle_buzz_volume_db", -12.0))
        self._obstacle_amp = float(10.0 ** (obs_db / 20.0))
        self.OBSTACLE_BUZZ_WAVEFORM = self._generate_obstacle_buzz()
        self.DROPOFF_SWEEP_WAVEFORM = self._generate_dropoff_sweep()
        self.HILL_SWEEP_WAVEFORM = self._generate_hill_sweep()

        # Road detection beep volume
        road_db = float(cfg.get("road_beep_volume_db", ROAD_BEEP_AMP_DB))
        self._road_amp = float(10.0 ** (road_db / 20.0))
        self.ROAD_BEEP_WAVEFORM = self._generate_road_beep()
        self.ROAD_CHIME_FWD_WAVEFORM = self._generate_road_chime_tone(ROAD_CHIME_FWD_FREQ_HZ)
        self.ROAD_CHIME_BACK_WAVEFORM = self._generate_road_chime_tone(ROAD_CHIME_BACK_FREQ_HZ)

        # Regenerate volume-dependent waveforms
        self.CLICK_WAVEFORM = self._generate_click()
        self.HIGHLIGHT_CLICK_WAVEFORM = self._generate_highlight_click()
        self.LS_CLICK_WAVEFORM = self._generate_lowspeed_click()
        self.CAM_CLICK_WAVEFORM = self._generate_cam_click()
        self.CAM_HIGHLIGHT_CLICK_WAVEFORM = self._generate_cam_highlight_click()

        old_follow = self._follow_default_enabled
        old_device = self._preferred_device_name
        self._follow_default_enabled = bool(cfg.get("follow_default_audio_device", True))
        self._preferred_device_name = str(cfg.get("preferred_device_name", "")).strip()
        self._audio_poll_interval = float(cfg.get("audio_poll_interval_sec", 2.0))

        # If the device selection changed while the stream is already running, switch immediately.
        if self._audio_stream is not None and (
                old_follow != self._follow_default_enabled or
                old_device != self._preferred_device_name):
            new_idx = self._find_target_device(verbose=True)
            if new_idx is not None:
                self._restart_audio_stream(new_idx)
            # If the user just switched to "follow default" and the watcher isn't running, start it.
            if self._follow_default_enabled and self._device_watcher_thread is None:
                self._device_watcher_thread = threading.Thread(
                    target=self._device_watcher_loop, daemon=True)
                self._device_watcher_thread.start()

    def load_hrtf(self, hrir_path):
        """Load pre-baked HRIR data (.npz) for binaural compass clicks."""
        if not self._is_enabled:
            return
        try:
            from hrtf import HRTFSet
            hrtf = HRTFSet(hrir_path, self.logger)
            if hrtf.is_loaded:
                hrtf.resample(self.samplerate)
                self._hrtf = hrtf
            else:
                self.logger.warning("HRTF file could not be loaded — falling back to stereo panning.")
        except Exception as e:
            self.logger.warning(f"HRTF initialization failed: {e} — falling back to stereo panning.")

    def update_telemetry_state(self, state):
        with self.lock:
            self.shift_active = state.get('shift_active', self.shift_active)
            self.pedal_tones_active = state.get('pedal_tones_active', self.pedal_tones_active)
            if state.get('tc_active') and not self.tc_active and self._tc_clicks_enabled:
                self._tc_playback_pos = 0.0 # Trigger TC tone on rising edge
            self.tc_active = state.get('tc_active', self.tc_active)
            self.last_clutch = state.get('last_clutch', self.last_clutch)
            self.last_brake = state.get('last_brake', self.last_throttle)
            self.last_throttle = state.get('last_throttle', self.last_throttle)
            self.last_steering = state.get('last_steering', self.last_steering) # NEW
            self.inverted = state.get('inverted', self.inverted)
            self.last_roll_rad = state.get('last_roll_rad', self.last_roll_rad)
            self.last_pitch_rad = state.get('last_pitch_rad', self.last_pitch_rad)
            
            # NEW: Heading Guidance
            self._guidance_active = state.get('guidance_active', self._guidance_active)
            self._guidance_error_deg = state.get('guidance_error_deg', self._guidance_error_deg)

            # Hydraulic steering
            self._hydro_actual_steer = state.get('last_actual_steering', self._hydro_actual_steer)
            self._hydro_steer_input  = state.get('last_steering_input',  self._hydro_steer_input)

            # Coordinate Guidance
            self._coord_guidance_active = state.get('coord_guidance_active', self._coord_guidance_active)
            self._coord_guidance_error = state.get('coord_guidance_error_deg', self._coord_guidance_error)

            # NEW: Drift Detection
            self._drift_alert_active = state.get('drift_alert_active', self._drift_alert_active)
            self._drift_pan = state.get('drift_pan', self._drift_pan)
            self._drift_rate = state.get('drift_rate', self._drift_rate)

            # NEW: Low Speed Detection
            self._ls_clicks_active = state.get('ls_clicks_active', self._ls_clicks_active)
            self._ls_speed_mph = state.get('ls_speed_mph', self._ls_speed_mph)
            self._ls_decel = state.get('ls_decel', self._ls_decel)
            self._scan_speed_ms = state.get('scan_speed_ms', self._scan_speed_ms)

    def _hrtf_emphasis_gain(self, hrtf_az_deg):
        """Compute front-back emphasis gain. 0 dB at front (0°), emphasis_db at back (180°)."""
        if self._hrtf_front_emphasis_db >= 0.0:
            return 1.0
        # Cosine curve: (1 - cos(az)) / 2 gives 0 at 0°, 1 at 180°
        behind_norm = (1.0 - math.cos(math.radians(hrtf_az_deg))) * 0.5
        db = self._hrtf_front_emphasis_db * behind_norm
        return float(10.0 ** (db / 20.0))

    def trigger_compass_click(self, azimuth_deg, pitch_mult):
        with self.lock:
            self._click_request_pitch_mult = pitch_mult
            if self._hrtf is not None and self._hrtf_user_enabled and self.CLICK_WAVEFORM is not None:
                hrtf_az = (360.0 - azimuth_deg) % 360.0
                ir_l, ir_r = self._hrtf.get_hrir(hrtf_az)
                if ir_l is not None:
                    gain = self._hrtf_emphasis_gain(hrtf_az) * self._hrtf_distance_gain
                    self._click_conv_L = (np.convolve(self.CLICK_WAVEFORM, ir_l, mode='full') * gain).astype(np.float32)
                    self._click_conv_R = (np.convolve(self.CLICK_WAVEFORM, ir_r, mode='full') * gain).astype(np.float32)
                    self._click_use_hrtf = True
                else:
                    self._click_use_hrtf = False
                    self._click_request_pan = math.sin(math.radians(azimuth_deg))
            else:
                self._click_use_hrtf = False
                self._click_request_pan = math.sin(math.radians(azimuth_deg))
        self._click_request.set()

    def trigger_compass_highlight(self, azimuth_deg, pitch_mult):
        with self.lock:
            self._highlight_request_pitch_mult = pitch_mult
            if self._hrtf is not None and self._hrtf_user_enabled and self.HIGHLIGHT_CLICK_WAVEFORM is not None:
                hrtf_az = (360.0 - azimuth_deg) % 360.0
                ir_l, ir_r = self._hrtf.get_hrir(hrtf_az)
                if ir_l is not None:
                    gain = self._hrtf_emphasis_gain(hrtf_az) * self._hrtf_distance_gain
                    self._hclick_conv_L = (np.convolve(self.HIGHLIGHT_CLICK_WAVEFORM, ir_l, mode='full') * gain).astype(np.float32)
                    self._hclick_conv_R = (np.convolve(self.HIGHLIGHT_CLICK_WAVEFORM, ir_r, mode='full') * gain).astype(np.float32)
                    self._hclick_use_hrtf = True
                else:
                    self._hclick_use_hrtf = False
                    self._highlight_request_pan = math.sin(math.radians(azimuth_deg))
            else:
                self._hclick_use_hrtf = False
                self._highlight_request_pan = math.sin(math.radians(azimuth_deg))
        self._highlight_click_request.set()

    def trigger_cam_compass_click(self, azimuth_deg, pitch_mult):
        with self.lock:
            self._cam_click_request_pitch_mult = pitch_mult
            if self._hrtf is not None and self._hrtf_user_enabled and self.CAM_CLICK_WAVEFORM is not None:
                hrtf_az = (360.0 - azimuth_deg) % 360.0
                ir_l, ir_r = self._hrtf.get_hrir(hrtf_az)
                if ir_l is not None:
                    gain = self._hrtf_emphasis_gain(hrtf_az) * self._hrtf_distance_gain
                    self._cam_click_conv_L = (np.convolve(self.CAM_CLICK_WAVEFORM, ir_l, mode='full') * gain).astype(np.float32)
                    self._cam_click_conv_R = (np.convolve(self.CAM_CLICK_WAVEFORM, ir_r, mode='full') * gain).astype(np.float32)
                    self._cam_click_use_hrtf = True
                else:
                    self._cam_click_use_hrtf = False
                    self._cam_click_request_pan = math.sin(math.radians(azimuth_deg))
            else:
                self._cam_click_use_hrtf = False
                self._cam_click_request_pan = math.sin(math.radians(azimuth_deg))
        self._cam_click_request.set()

    def trigger_cam_compass_highlight(self, azimuth_deg, pitch_mult):
        with self.lock:
            self._cam_highlight_request_pitch_mult = pitch_mult
            if self._hrtf is not None and self._hrtf_user_enabled and self.CAM_HIGHLIGHT_CLICK_WAVEFORM is not None:
                hrtf_az = (360.0 - azimuth_deg) % 360.0
                ir_l, ir_r = self._hrtf.get_hrir(hrtf_az)
                if ir_l is not None:
                    gain = self._hrtf_emphasis_gain(hrtf_az) * self._hrtf_distance_gain
                    self._cam_hclick_conv_L = (np.convolve(self.CAM_HIGHLIGHT_CLICK_WAVEFORM, ir_l, mode='full') * gain).astype(np.float32)
                    self._cam_hclick_conv_R = (np.convolve(self.CAM_HIGHLIGHT_CLICK_WAVEFORM, ir_r, mode='full') * gain).astype(np.float32)
                    self._cam_hclick_use_hrtf = True
                else:
                    self._cam_hclick_use_hrtf = False
                    self._cam_highlight_request_pan = math.sin(math.radians(azimuth_deg))
            else:
                self._cam_hclick_use_hrtf = False
                self._cam_highlight_request_pan = math.sin(math.radians(azimuth_deg))
        self._cam_highlight_click_request.set()

    def trigger_check_engine_buzzer(self):
        if self._is_enabled:
            self._buzzer_playback_pos = 0.0

    def trigger_oil_chime(self):
        if self._is_enabled:
            self._chime_playback_pos = 0.0

    def trigger_node_hover_beep(self, height_normalized, reverse=False):
        """Trigger a short beep with pitch varying by node height on the vehicle.
        height_normalized: 0.0 (underside) to 1.0 (top). reverse=True plays backwards."""
        if self._is_enabled:
            h = max(0.0, min(1.0, height_normalized))
            self._node_beep_freq = NODE_BEEP_LOW_FREQ_HZ + (NODE_BEEP_HIGH_FREQ_HZ - NODE_BEEP_LOW_FREQ_HZ) * h
            self._node_beep_reverse = reverse
            self._node_beep_playback_pos = 0.0

    def trigger_clickspot_beep(self, reverse=False):
        """Trigger FM click beep. reverse=True plays the waveform backwards (mouse leaving)."""
        if self._is_enabled:
            self._clickspot_beep_reverse = reverse
            self._clickspot_beep_playback_pos = 0.0

    def trigger_describe_error_buzz(self):
        """Low synthetic FM buzz, played when an AI Describe request is rejected
        because one is already in flight (double-press guard)."""
        if self._is_enabled:
            self._describe_error_playback_pos = 0.0

    # --- Private Methods ---
    
    def _clamp(self, v, a=0.0, b=1.0): return a if v < a else b if v > b else v
    def _pan_gains(self, p):
        ang = (self._clamp(p, -1.0, 1.0) + 1.0) * (math.pi / 4.0)
        return math.cos(ang), math.sin(ang)
        
    def _norm_from_angle_deg(self, deg, start=ATT_START_DEG, stop=ATT_STOP_DEG, cap=90.0):
        d = abs(float(deg))
        if d < stop: return 0.0
        if d <= start:
            n = (d - stop) / max(1e-6, (start - stop))
            return max(1e-6, n * 1e-3)
        return self._clamp((d - start) / max(1e-6, (cap - start)), 0.0, 1.0)

    def _amp_att_from_norm(self, n):
        n = self._clamp(n)
        if n <= 0.0: return 0.0
        db = self._att_min_dbfs + (self._att_max_dbfs - self._att_min_dbfs) * (n**0.6)
        return float(10.0 ** (db / 20.0))

    def _amp_from_val(self, v):
        if v <= 0.0: return 0.0
        dB = -30.0 + 12.0 * (v**0.5)
        return float(10.0 ** (dB / 20.0))

    def _regenerate_waveforms(self):
        self.logger.info(f"Generating audio waveforms for {self.samplerate} Hz sample rate.")
        self.CLICK_WAVEFORM = self._generate_click()
        self.HIGHLIGHT_CLICK_WAVEFORM = self._generate_highlight_click()
        self.TC_TONE_WAVEFORM = self._generate_tc_tone()
        self.CHECK_ENGINE_BUZZER_WAVEFORM = self._generate_check_engine_buzzer(self.buzzer_amp)
        self.OIL_CHIME_WAVEFORM = self._generate_oil_chime(self.chime_amp)
        self.SCANNER_BEEP_WAVEFORM = self._generate_scanner_beep() # NEW
        self.SCANNER_ALIGNED_WAVEFORM = self._generate_scanner_aligned_beep()
        self.OBSTACLE_BUZZ_WAVEFORM = self._generate_obstacle_buzz()
        self.DROPOFF_SWEEP_WAVEFORM = self._generate_dropoff_sweep()
        self.HILL_SWEEP_WAVEFORM = self._generate_hill_sweep()
        self.ROAD_BEEP_WAVEFORM = self._generate_road_beep()
        self.ROAD_CHIME_FWD_WAVEFORM = self._generate_road_chime_tone(ROAD_CHIME_FWD_FREQ_HZ)
        self.ROAD_CHIME_BACK_WAVEFORM = self._generate_road_chime_tone(ROAD_CHIME_BACK_FREQ_HZ)
        self.GUIDANCE_WAVEFORM = self._generate_guidance_tone() # NEW
        self.LS_CLICK_WAVEFORM = self._generate_lowspeed_click()
        self.CAM_CLICK_WAVEFORM = self._generate_cam_click()
        self.CAM_HIGHLIGHT_CLICK_WAVEFORM = self._generate_cam_highlight_click()
        self.NODE_BEEP_WAVEFORM = self._generate_node_beep()
        self.NODE_BEEP_REV_WAVEFORM = self._generate_node_beep_reverse()
        self.CLICKSPOT_BEEP_WAVEFORM = self._generate_clickspot_beep()
        self.CLICKSPOT_BEEP_REV_WAVEFORM = self._generate_clickspot_beep_reverse()
        self.DESCRIBE_ERROR_WAVEFORM = self._generate_describe_error_buzz()
        if self._hrtf is not None:
            self._hrtf.resample(self.samplerate)

    def _generate_click(self):
        dur_samples = int(self.samplerate * CLICK_DUR_MS / 1000.0)
        t = np.linspace(0, CLICK_DUR_MS / 1000.0, dur_samples, endpoint=False)
        decay = np.exp(-t * (1.0 / (CLICK_DUR_MS / 2000.0)))
        wave = np.sin(2.0 * np.pi * 1200.0 * t) * decay
        fade_len = int(dur_samples * 0.1)
        if fade_len > 0: wave[-fade_len:] *= np.linspace(1, 0, fade_len)
        return (wave * self._compass_click_amp).astype(np.float32)

    def _generate_highlight_click(self):
        dur_samples = int(self.samplerate * CLICK_DUR_MS / 1000.0)
        t = np.linspace(0, CLICK_DUR_MS / 1000.0, dur_samples, endpoint=False)
        decay = np.exp(-t * (1.0 / (CLICK_DUR_MS / 2000.0)))
        
        # Waveform with fundamental, 3rd, and 5th harmonics
        f0 = 1200.0
        wave = (0.6 * np.sin(2.0 * np.pi * f0 * t) +
                0.3 * np.sin(2.0 * np.pi * f0 * 3 * t) +
                0.1 * np.sin(2.0 * np.pi * f0 * 5 * t))
        
        wave /= np.max(np.abs(wave)) # Normalize
        wave *= decay

        fade_len = int(dur_samples * 0.1)
        if fade_len > 0: wave[-fade_len:] *= np.linspace(1, 0, fade_len)
        return (wave * self._compass_click_amp).astype(np.float32)

    def _generate_cam_click(self):
        dur_samples = int(self.samplerate * CLICK_DUR_MS / 1000.0)
        t = np.linspace(0, CLICK_DUR_MS / 1000.0, dur_samples, endpoint=False)
        decay = np.exp(-t * (1.0 / (CLICK_DUR_MS / 2000.0)))
        # FM synthesis: carrier 900 Hz, modulator 450 Hz (CM ratio 2.0), mod index 1.5
        fc, fm, beta = 900.0, 450.0, 1.5
        wave = np.sin(2.0 * np.pi * fc * t + beta * np.sin(2.0 * np.pi * fm * t)) * decay
        fade_len = int(dur_samples * 0.1)
        if fade_len > 0: wave[-fade_len:] *= np.linspace(1, 0, fade_len)
        return (wave * self._compass_click_amp).astype(np.float32)

    def _generate_cam_highlight_click(self):
        dur_samples = int(self.samplerate * CLICK_DUR_MS / 1000.0)
        t = np.linspace(0, CLICK_DUR_MS / 1000.0, dur_samples, endpoint=False)
        decay = np.exp(-t * (1.0 / (CLICK_DUR_MS / 2000.0)))
        # FM synthesis base + 3rd harmonic on carrier for richer accent
        fc, fm, beta = 900.0, 450.0, 1.5
        wave = (0.7 * np.sin(2.0 * np.pi * fc * t + beta * np.sin(2.0 * np.pi * fm * t)) +
                0.3 * np.sin(2.0 * np.pi * fc * 3 * t + beta * np.sin(2.0 * np.pi * fm * t)))
        wave /= np.max(np.abs(wave))
        wave *= decay
        fade_len = int(dur_samples * 0.1)
        if fade_len > 0: wave[-fade_len:] *= np.linspace(1, 0, fade_len)
        return (wave * self._compass_click_amp).astype(np.float32)

    def _generate_tc_tone(self):
        DUR_SEC, FC, CM_RATIO = 0.032, 500.0, 3.075
        FM, num_samples = FC / CM_RATIO, int(self.samplerate * DUR_SEC)
        t = np.linspace(0, DUR_SEC, num_samples, endpoint=False)
        mod_index_env = 4.0 * np.exp(-t / (DUR_SEC / 3.0))
        amp_env = 0.4 * np.sin(np.pi * t / DUR_SEC)
        modulator = mod_index_env * np.sin(2 * np.pi * FM * t)
        carrier = np.sin(2 * np.pi * FC * t + modulator)
        return (amp_env * carrier).astype(np.float32)

    def _generate_check_engine_buzzer(self, amplitude):
        DUR_SEC, FC, FM = 0.5, 400.0, 150.0
        num_samples = int(self.samplerate * DUR_SEC)
        t = np.linspace(0, DUR_SEC, num_samples, endpoint=False)
        mod_index_env = 8.0 * np.exp(-t / (DUR_SEC / 3.0))
        modulator = mod_index_env * np.sin(2 * np.pi * FM * t)
        carrier = np.sin(2 * np.pi * FC * t + modulator)
        amp_env = np.ones(num_samples)
        fade_start_sample = int(self.samplerate * 0.4)
        fade_samples = num_samples - fade_start_sample
        if fade_samples > 0:
            amp_env[fade_start_sample:] = np.linspace(1, 0, fade_samples) ** 2
        return (amplitude * amp_env * carrier).astype(np.float32)

    def _generate_describe_error_buzz(self):
        # Low, dark FM buzz for the AI describer double-press rejection. Carrier
        # well below the check-engine buzzer with a sub-harmonic modulator and a
        # high, decaying modulation index for a gravelly "buzz" character.
        DUR_SEC, FC, FM = 0.25, 220.0, 90.0
        amplitude = float(10.0 ** (-18.0 / 20.0))
        num_samples = int(self.samplerate * DUR_SEC)
        t = np.linspace(0, DUR_SEC, num_samples, endpoint=False)
        mod_index_env = 7.0 * np.exp(-t / (DUR_SEC / 2.5))
        modulator = mod_index_env * np.sin(2 * np.pi * FM * t)
        carrier = np.sin(2 * np.pi * FC * t + modulator)
        amp_env = np.ones(num_samples)
        attack = int(self.samplerate * 0.005)
        if attack > 0:
            amp_env[:attack] = np.linspace(0, 1, attack)
        fade_start = int(self.samplerate * 0.15)
        fade_samples = num_samples - fade_start
        if fade_samples > 0:
            amp_env[fade_start:] = np.linspace(1, 0, fade_samples) ** 2
        return (amplitude * amp_env * carrier).astype(np.float32)

    def _generate_oil_chime(self, amplitude):
        DUR_SEC, FC = 1.0, 523.25
        num_samples = int(self.samplerate * DUR_SEC)
        t = np.linspace(0, DUR_SEC, num_samples, endpoint=False)
        wave = (np.sin(2*np.pi*FC*t) + 0.5*np.sin(2*np.pi*FC*2*t) + 0.2*np.sin(2*np.pi*FC*3*t))
        wave /= np.max(np.abs(wave))
        amp_env = np.exp(-t * 5.0) * (1 - np.exp(-t * 30))
        amp_env /= np.max(amp_env)
        return (amplitude * amp_env * wave).astype(np.float32)

    # Vehicle scanner beep: triangle wave carrier FM-modulated by a sine
    # Modulator:carrier ratio 2:1, modulation index 1.0
    def _generate_scanner_beep(self):
        dur_samples = int(self.samplerate * SCANNER_BEEP_DUR_MS / 1000.0)
        t = np.linspace(0, SCANNER_BEEP_DUR_MS / 1000.0, dur_samples, endpoint=False)
        envelope = np.exp(-t * (1.0 / (SCANNER_BEEP_DUR_MS / 4000.0)))
        fc = SCANNER_BEEP_FREQ_HZ            # carrier frequency
        fm = 2.0 * fc                         # modulator frequency (2:1 ratio)
        beta = 1.0                            # modulation index
        # FM instantaneous phase: carrier phase + index * sin(modulator)
        phase = 2.0 * np.pi * fc * t + beta * np.sin(2.0 * np.pi * fm * t)
        # Triangle wave from phase via arcsine of sine
        wave = (2.0 / np.pi) * np.arcsin(np.sin(phase))
        return (wave * envelope * 0.25).astype(np.float32)
        
    # Vehicle scanner aligned beep: same base but with added odd harmonics for brightness
    def _generate_scanner_aligned_beep(self):
        dur_samples = int(self.samplerate * SCANNER_BEEP_DUR_MS / 1000.0)
        t = np.linspace(0, SCANNER_BEEP_DUR_MS / 1000.0, dur_samples, endpoint=False)
        envelope = np.exp(-t * (1.0 / (SCANNER_BEEP_DUR_MS / 4000.0)))
        fc = SCANNER_BEEP_FREQ_HZ
        phase = 2.0 * np.pi * fc * t
        # Base sine + odd harmonics (3rd, 5th, 7th) for bright, distinctive tone
        wave = (np.sin(phase)
                + 0.4 * np.sin(3.0 * phase)
                + 0.25 * np.sin(5.0 * phase)
                + 0.15 * np.sin(7.0 * phase))
        wave /= np.max(np.abs(wave))
        return (wave * envelope * 0.25).astype(np.float32)

    # Obstacle detection: harsh square wave buzzer
    def _generate_obstacle_buzz(self):
        dur_samples = int(self.samplerate * OBSTACLE_BUZZ_DUR_MS / 1000.0)
        t = np.linspace(0, OBSTACLE_BUZZ_DUR_MS / 1000.0, dur_samples, endpoint=False)
        amplitude = self._obstacle_amp
        # Square wave via sign(sin)
        wave = np.sign(np.sin(2.0 * np.pi * OBSTACLE_BUZZ_FREQ_HZ * t))
        # Sharp attack, short decay envelope
        envelope = np.ones(dur_samples, dtype=np.float32)
        fade_start = int(dur_samples * 0.7)
        fade_len = dur_samples - fade_start
        if fade_len > 0:
            envelope[fade_start:] = np.linspace(1, 0, fade_len)
        return (wave * envelope * amplitude).astype(np.float32)

    # Terrain drop-off warning: descending pitch sweep
    def _generate_dropoff_sweep(self):
        dur_samples = int(self.samplerate * TERRAIN_SWEEP_DUR_MS / 1000.0)
        t = np.linspace(0, TERRAIN_SWEEP_DUR_MS / 1000.0, dur_samples, endpoint=False)
        amplitude = 10.0 ** (TERRAIN_SWEEP_AMP_DB / 20.0)
        # Exponential frequency sweep from high to low
        freq = DROPOFF_FREQ_START * (DROPOFF_FREQ_END / DROPOFF_FREQ_START) ** (t / (TERRAIN_SWEEP_DUR_MS / 1000.0))
        phase = 2.0 * np.pi * np.cumsum(freq) / self.samplerate
        wave = np.sign(np.sin(phase))  # square wave sweep
        envelope = np.exp(-t * 3.0 / (TERRAIN_SWEEP_DUR_MS / 1000.0))
        return (wave * envelope * amplitude).astype(np.float32)

    # Steep hill warning: ascending pitch sweep
    def _generate_hill_sweep(self):
        dur_samples = int(self.samplerate * TERRAIN_SWEEP_DUR_MS / 1000.0)
        t = np.linspace(0, TERRAIN_SWEEP_DUR_MS / 1000.0, dur_samples, endpoint=False)
        amplitude = 10.0 ** (TERRAIN_SWEEP_AMP_DB / 20.0)
        freq = HILL_FREQ_START * (HILL_FREQ_END / HILL_FREQ_START) ** (t / (TERRAIN_SWEEP_DUR_MS / 1000.0))
        phase = 2.0 * np.pi * np.cumsum(freq) / self.samplerate
        wave = np.sign(np.sin(phase))
        envelope = np.exp(-t * 3.0 / (TERRAIN_SWEEP_DUR_MS / 1000.0))
        return (wave * envelope * amplitude).astype(np.float32)

    def _generate_road_beep(self):
        """Off-road guidance beep: F#5 carrier with sub-octave FM modulator.
        The modulation index decays through the pulse so the attack has a hint of growl
        and the tail relaxes to a clean tone — distinct without being harsh."""
        dur_samples = int(self.samplerate * ROAD_BEEP_DUR_MS / 1000.0)
        if dur_samples <= 0:
            return None
        t = np.linspace(0, ROAD_BEEP_DUR_MS / 1000.0, dur_samples, endpoint=False)
        # Modulator decays exponentially over the pulse (less funk on the tail)
        mod_env = ROAD_BEEP_MOD_INDEX * np.exp(-12.0 * t)
        modulator = mod_env * np.sin(2.0 * np.pi * ROAD_BEEP_MOD_HZ * t)
        wave = np.sin(2.0 * np.pi * ROAD_BEEP_FREQ_HZ * t + modulator)
        # Smooth attack + exponential decay (no clicks at edges)
        attack = 1.0 - np.exp(-200.0 * t)
        decay  = np.exp(-12.0 * t)
        envelope = attack * decay
        return (wave * envelope * self._road_amp).astype(np.float32)

    def _generate_road_chime_tone(self, freq_hz):
        """Orientation chime tone: sine carrier at freq_hz with a 1:1 FM modulator whose index
        starts high (clicky attack) and decays sharply to a low sustained value (gentle warmth).
        Same generator for both chime tones — they differ only in carrier frequency."""
        dur_samples = int(self.samplerate * ROAD_CHIME_DUR_MS / 1000.0)
        if dur_samples <= 0:
            return None
        t = np.linspace(0, ROAD_CHIME_DUR_MS / 1000.0, dur_samples, endpoint=False)
        # Modulation index: peak → floor with sharp exponential decay
        mod_index = ROAD_CHIME_MOD_INDEX_FLOOR + (
            ROAD_CHIME_MOD_INDEX_PEAK - ROAD_CHIME_MOD_INDEX_FLOOR
        ) * np.exp(-ROAD_CHIME_MOD_DECAY * t)
        modulator = mod_index * np.sin(2.0 * np.pi * freq_hz * t)
        wave = np.sin(2.0 * np.pi * freq_hz * t + modulator)
        # Smooth attack (~10 ms), slow exponential decay so the tone has body but doesn't ring forever
        attack = 1.0 - np.exp(-100.0 * t)
        decay = np.exp(-6.0 * t)
        envelope = attack * decay
        # Final 8 ms linear taper to bury any tail
        fade_samples = int(self.samplerate * 0.008)
        if 0 < fade_samples < dur_samples:
            envelope[-fade_samples:] *= np.linspace(1.0, 0.0, fade_samples)
        amp = float(10.0 ** (ROAD_CHIME_AMP_DB / 20.0))
        return (wave * envelope * amp).astype(np.float32)

    def _generate_guidance_tone(self):
        # A simple, clean sine wave. Amplitude modulation happens in the callback.
        # We'll just return a single period or a small buffer, but since we synthesize continuously
        # in the callback using a phase, we technically don't need a buffer here unless we want
        # complex wavetable synthesis.
        # However, for consistency with other methods, let's keep the logic in the callback for continuous tones.
        # This method is here just to satisfy the pattern if we needed pre-calc buffers.
        return None # Not used, we synthesize on the fly

    def _generate_lowspeed_click(self):
        dur_samples = int(self.samplerate * LS_CLICK_DUR_MS / 1000.0)
        t = np.linspace(0, LS_CLICK_DUR_MS / 1000.0, dur_samples, endpoint=False)
        # FM synthesis: carrier = sin(2π·250·t + mod_env·sin(2π·375·t))
        mod_env = LS_MOD_INDEX_PEAK * np.exp(-30.0 * t)
        carrier = np.sin(2.0 * np.pi * LS_CARRIER_HZ * t + mod_env * np.sin(2.0 * np.pi * LS_MOD_HZ * t))
        # Envelope: fast attack (~2ms), moderate decay
        attack = 1.0 - np.exp(-500.0 * t)
        decay = np.exp(-15.0 * t)
        envelope = attack * decay
        amp = float(10.0 ** (self._ls_click_amp_db / 20.0))
        return (amp * envelope * carrier).astype(np.float32)

    def _generate_clickspot_beep(self):
        """Generate an FM synthesis click sound. Stored forward; reversed at playback time."""
        dur_samples = int(self.samplerate * CLICKSPOT_BEEP_DUR_MS / 1000.0)
        t = np.linspace(0, CLICKSPOT_BEEP_DUR_MS / 1000.0, dur_samples, endpoint=False)
        # FM synthesis: carrier modulated by a sine modulator
        # Modulation index decays over time for a "click" attack character
        mod_env = np.exp(-60.0 * t)  # fast decay on modulator intensity
        modulator = CLICKSPOT_BEEP_MOD_INDEX * mod_env * np.sin(2.0 * np.pi * CLICKSPOT_BEEP_MOD_HZ * t)
        wave = np.sin(2.0 * np.pi * CLICKSPOT_BEEP_FREQ_HZ * t + modulator)
        # Amplitude envelope: sharp attack, exponential decay
        envelope = np.exp(-40.0 * t)
        wave *= envelope
        amp = float(10.0 ** (CLICKSPOT_BEEP_AMP_DB / 20.0))
        return (wave * amp).astype(np.float32)

    def _generate_clickspot_beep_reverse(self):
        """Generate the reverse/leaving variant — FM click with rising modulation and attack envelope."""
        dur_samples = int(self.samplerate * CLICKSPOT_BEEP_DUR_MS / 1000.0)
        t = np.linspace(0, CLICKSPOT_BEEP_DUR_MS / 1000.0, dur_samples, endpoint=False)
        t_rev = t[-1] - t  # reversed time
        # FM synthesis with modulation rising (inverse of forward decay)
        mod_env = np.exp(-60.0 * t_rev)
        modulator = CLICKSPOT_BEEP_MOD_INDEX * mod_env * np.sin(2.0 * np.pi * CLICKSPOT_BEEP_MOD_HZ * t)
        wave = np.sin(2.0 * np.pi * CLICKSPOT_BEEP_FREQ_HZ * t + modulator)
        # Amplitude envelope: ramp up then sharp cutoff
        envelope = 1.0 - np.exp(-40.0 * t)  # inverse of forward decay
        # Apply a short fade-out at the very end to avoid click
        fade_samples = int(self.samplerate * 0.003)
        if fade_samples > 0 and fade_samples < dur_samples:
            envelope[-fade_samples:] *= np.linspace(1, 0, fade_samples)
        wave *= envelope
        amp = float(10.0 ** (CLICKSPOT_BEEP_AMP_DB / 20.0))
        return (wave * amp).astype(np.float32)

    def _generate_node_beep(self):
        """Generate a sine beep template at base frequency. Pitch is applied at playback time."""
        dur_samples = int(self.samplerate * NODE_BEEP_DUR_MS / 1000.0)
        t = np.linspace(0, NODE_BEEP_DUR_MS / 1000.0, dur_samples, endpoint=False)
        # Sine wave at 1 Hz — we scale by actual freq at playback using pitch mult
        wave = np.sin(2.0 * np.pi * NODE_BEEP_BASE_FREQ_HZ * t)
        # Exponential decay envelope
        decay_rate = 5.0 / (NODE_BEEP_DUR_MS / 1000.0)
        envelope = np.exp(-decay_rate * t)
        wave *= envelope
        amp = float(10.0 ** (NODE_BEEP_AMP_DB / 20.0))
        return (wave * amp).astype(np.float32)

    def _generate_node_beep_reverse(self):
        """Generate the reverse/leaving variant of the node beep — rising envelope."""
        dur_samples = int(self.samplerate * NODE_BEEP_DUR_MS / 1000.0)
        t = np.linspace(0, NODE_BEEP_DUR_MS / 1000.0, dur_samples, endpoint=False)
        wave = np.sin(2.0 * np.pi * NODE_BEEP_BASE_FREQ_HZ * t)
        # Rising envelope (inverse of forward decay)
        decay_rate = 5.0 / (NODE_BEEP_DUR_MS / 1000.0)
        envelope = 1.0 - np.exp(-decay_rate * t)
        # Short fade-out at end to avoid click
        fade_samples = int(self.samplerate * 0.003)
        if fade_samples > 0 and fade_samples < dur_samples:
            envelope[-fade_samples:] *= np.linspace(1, 0, fade_samples)
        wave *= envelope
        amp = float(10.0 ** (NODE_BEEP_AMP_DB / 20.0))
        return (wave * amp).astype(np.float32)

    def _audio_callback(self, outdata, frames, time_info, status):
        if self._click_request.is_set():
            with self.lock:
                self._click_playback_pos = 0.0
                self._click_pitch_mult = self._click_request_pitch_mult
                if not self._click_use_hrtf:
                    self._click_pan_pos = self._click_request_pan
            self._click_request.clear()

        if self._highlight_click_request.is_set():
            with self.lock:
                self._highlight_click_playback_pos = 0.0
                self._highlight_click_pitch_mult = self._highlight_request_pitch_mult
                if not self._hclick_use_hrtf:
                    self._highlight_click_pan_pos = self._highlight_request_pan
            self._highlight_click_request.clear()

        if self._cam_click_request.is_set():
            with self.lock:
                self._cam_click_playback_pos = 0.0
                self._cam_click_pitch_mult = self._cam_click_request_pitch_mult
                if not self._cam_click_use_hrtf:
                    self._cam_click_pan_pos = self._cam_click_request_pan
            self._cam_click_request.clear()

        if self._cam_highlight_click_request.is_set():
            with self.lock:
                self._cam_highlight_click_playback_pos = 0.0
                self._cam_highlight_click_pitch_mult = self._cam_highlight_request_pitch_mult
                if not self._cam_hclick_use_hrtf:
                    self._cam_highlight_click_pan_pos = self._cam_highlight_request_pan
            self._cam_highlight_click_request.clear()

        with self.lock:
            # Copy all necessary state variables to local scope to minimize time holding the lock
            shift_on, pt_on = self.shift_active, self.pedal_tones_active
            v_clutch, v_brake, v_throt = self.last_clutch, self.last_brake, self.last_throttle
            v_steer = self._hydro_steer_input # normalized -1..1 steering_input
            inv, roll_rad, pitch_rad = self.inverted, self.last_roll_rad, self.last_pitch_rad
            scan_active = self._scan_mode_active
            scan_bearing = self._scanner_target_bearing
            scan_dist = self._scanner_target_distance
            # Coupler tracking snapshot
            coupler_active = self._coupler_active
            coupler_bearing = self._coupler_bearing
            coupler_dist = self._coupler_distance
            coupler_in_range = self._coupler_in_range
            # Obstacle detection snapshot
            obs_active = self._obstacle_mode_active
            obs_bearings = list(self._obstacle_bearings)
            obs_distances = list(self._obstacle_distances)
            obs_types = list(self._obstacle_types)
            terrain_type = self._terrain_type
            # Road detection snapshot
            road_active = self._road_mode_active
            road_on_road = self._road_on_road
            road_bearing = self._road_bearing
            road_distance = self._road_distance
            # NEW: Guidance state
            guide_active = self._guidance_active
            guide_error = self._guidance_error_deg
            # Hydraulic steering
            hydro_actual = self._hydro_actual_steer
            hydro_input  = self._hydro_steer_input

            # Coordinate Guidance
            coord_guide_active = self._coord_guidance_active
            coord_guide_error = self._coord_guidance_error
            # NEW: Low speed state
            ls_active = self._ls_clicks_active
            ls_speed_mph = self._ls_speed_mph
            ls_decel = self._ls_decel
            scan_speed_ms = self._scan_speed_ms
            scan_closing_ms = self._scan_closing_ms
            scan_base_freq = self._scan_base_freq
            scan_offset_oct = self._scan_offset_oct
            scan_steer_tone_enabled = self._scan_steer_tone_enabled

        bufL, bufR = np.zeros(frames, dtype=np.float32), np.zeros(frames, dtype=np.float32)

        playback_states = [
            ('_tc_playback_pos', self.TC_TONE_WAVEFORM),
            ('_buzzer_playback_pos', self.CHECK_ENGINE_BUZZER_WAVEFORM),
            ('_chime_playback_pos', self.OIL_CHIME_WAVEFORM),
            ('_describe_error_playback_pos', self.DESCRIBE_ERROR_WAVEFORM),
        ]
        for pos_attr, waveform in playback_states:
            pos = getattr(self, pos_attr)
            if pos >= 0 and waveform is not None:
                start, wav_len = int(pos), len(waveform)
                num_to_mix = min(frames, wav_len - start)
                if num_to_mix > 0:
                    segment = waveform[start : start + num_to_mix]
                    bufL[:num_to_mix] += segment; bufR[:num_to_mix] += segment
                next_pos = pos + frames
                setattr(self, pos_attr, next_pos if next_pos < wav_len else -1.0)
        
        if self._click_playback_pos >= 0:
            if self._click_use_hrtf and self._click_conv_L is not None:
                click_len = len(self._click_conv_L)
                indices = self._click_playback_pos + np.arange(frames) * self._click_pitch_mult
                valid_mask = indices < (click_len - 1)
                if np.any(valid_mask):
                    valid_indices = indices[valid_mask]
                    idx_floor, fract = np.floor(valid_indices).astype(int), valid_indices - np.floor(valid_indices)
                    bufL[valid_mask] += self._click_conv_L[idx_floor] + (self._click_conv_L[idx_floor + 1] - self._click_conv_L[idx_floor]) * fract
                    bufR[valid_mask] += self._click_conv_R[idx_floor] + (self._click_conv_R[idx_floor + 1] - self._click_conv_R[idx_floor]) * fract
                next_pos = self._click_playback_pos + frames * self._click_pitch_mult
                self._click_playback_pos = next_pos if next_pos < (click_len - 1) else -1.0
            elif self.CLICK_WAVEFORM is not None:
                click_len = len(self.CLICK_WAVEFORM)
                indices = self._click_playback_pos + np.arange(frames) * self._click_pitch_mult
                valid_mask = indices < (click_len - 1)
                if np.any(valid_mask):
                    valid_indices = indices[valid_mask]
                    idx_floor, fract = np.floor(valid_indices).astype(int), valid_indices - np.floor(valid_indices)
                    sample1, sample2 = self.CLICK_WAVEFORM[idx_floor], self.CLICK_WAVEFORM[idx_floor + 1]
                    click_segment = sample1 + (sample2 - sample1) * fract
                    Lg, Rg = self._pan_gains(self._click_pan_pos)
                    bufL[valid_mask] += click_segment * Lg
                    bufR[valid_mask] += click_segment * Rg
                next_pos = self._click_playback_pos + frames * self._click_pitch_mult
                self._click_playback_pos = next_pos if next_pos < (click_len - 1) else -1.0

        if self._highlight_click_playback_pos >= 0:
            if self._hclick_use_hrtf and self._hclick_conv_L is not None:
                click_len = len(self._hclick_conv_L)
                indices = self._highlight_click_playback_pos + np.arange(frames) * self._highlight_click_pitch_mult
                valid_mask = indices < (click_len - 1)
                if np.any(valid_mask):
                    valid_indices = indices[valid_mask]
                    idx_floor, fract = np.floor(valid_indices).astype(int), valid_indices - np.floor(valid_indices)
                    bufL[valid_mask] += self._hclick_conv_L[idx_floor] + (self._hclick_conv_L[idx_floor + 1] - self._hclick_conv_L[idx_floor]) * fract
                    bufR[valid_mask] += self._hclick_conv_R[idx_floor] + (self._hclick_conv_R[idx_floor + 1] - self._hclick_conv_R[idx_floor]) * fract
                next_pos = self._highlight_click_playback_pos + frames * self._highlight_click_pitch_mult
                self._highlight_click_playback_pos = next_pos if next_pos < (click_len - 1) else -1.0
            elif self.HIGHLIGHT_CLICK_WAVEFORM is not None:
                click_len = len(self.HIGHLIGHT_CLICK_WAVEFORM)
                indices = self._highlight_click_playback_pos + np.arange(frames) * self._highlight_click_pitch_mult
                valid_mask = indices < (click_len - 1)
                if np.any(valid_mask):
                    valid_indices = indices[valid_mask]
                    idx_floor, fract = np.floor(valid_indices).astype(int), valid_indices - np.floor(valid_indices)
                    sample1, sample2 = self.HIGHLIGHT_CLICK_WAVEFORM[idx_floor], self.HIGHLIGHT_CLICK_WAVEFORM[idx_floor + 1]
                    click_segment = sample1 + (sample2 - sample1) * fract
                    Lg, Rg = self._pan_gains(self._highlight_click_pan_pos)
                    bufL[valid_mask] += click_segment * Lg
                    bufR[valid_mask] += click_segment * Rg
                next_pos = self._highlight_click_playback_pos + frames * self._highlight_click_pitch_mult
                self._highlight_click_playback_pos = next_pos if next_pos < (click_len - 1) else -1.0

        # Camera compass click playback
        if self._cam_click_playback_pos >= 0:
            if self._cam_click_use_hrtf and self._cam_click_conv_L is not None:
                click_len = len(self._cam_click_conv_L)
                indices = self._cam_click_playback_pos + np.arange(frames) * self._cam_click_pitch_mult
                valid_mask = indices < (click_len - 1)
                if np.any(valid_mask):
                    valid_indices = indices[valid_mask]
                    idx_floor, fract = np.floor(valid_indices).astype(int), valid_indices - np.floor(valid_indices)
                    bufL[valid_mask] += self._cam_click_conv_L[idx_floor] + (self._cam_click_conv_L[idx_floor + 1] - self._cam_click_conv_L[idx_floor]) * fract
                    bufR[valid_mask] += self._cam_click_conv_R[idx_floor] + (self._cam_click_conv_R[idx_floor + 1] - self._cam_click_conv_R[idx_floor]) * fract
                next_pos = self._cam_click_playback_pos + frames * self._cam_click_pitch_mult
                self._cam_click_playback_pos = next_pos if next_pos < (click_len - 1) else -1.0
            elif self.CAM_CLICK_WAVEFORM is not None:
                click_len = len(self.CAM_CLICK_WAVEFORM)
                indices = self._cam_click_playback_pos + np.arange(frames) * self._cam_click_pitch_mult
                valid_mask = indices < (click_len - 1)
                if np.any(valid_mask):
                    valid_indices = indices[valid_mask]
                    idx_floor, fract = np.floor(valid_indices).astype(int), valid_indices - np.floor(valid_indices)
                    sample1, sample2 = self.CAM_CLICK_WAVEFORM[idx_floor], self.CAM_CLICK_WAVEFORM[idx_floor + 1]
                    click_segment = sample1 + (sample2 - sample1) * fract
                    Lg, Rg = self._pan_gains(self._cam_click_pan_pos)
                    bufL[valid_mask] += click_segment * Lg
                    bufR[valid_mask] += click_segment * Rg
                next_pos = self._cam_click_playback_pos + frames * self._cam_click_pitch_mult
                self._cam_click_playback_pos = next_pos if next_pos < (click_len - 1) else -1.0

        # Camera compass highlight click playback
        if self._cam_highlight_click_playback_pos >= 0:
            if self._cam_hclick_use_hrtf and self._cam_hclick_conv_L is not None:
                click_len = len(self._cam_hclick_conv_L)
                indices = self._cam_highlight_click_playback_pos + np.arange(frames) * self._cam_highlight_click_pitch_mult
                valid_mask = indices < (click_len - 1)
                if np.any(valid_mask):
                    valid_indices = indices[valid_mask]
                    idx_floor, fract = np.floor(valid_indices).astype(int), valid_indices - np.floor(valid_indices)
                    bufL[valid_mask] += self._cam_hclick_conv_L[idx_floor] + (self._cam_hclick_conv_L[idx_floor + 1] - self._cam_hclick_conv_L[idx_floor]) * fract
                    bufR[valid_mask] += self._cam_hclick_conv_R[idx_floor] + (self._cam_hclick_conv_R[idx_floor + 1] - self._cam_hclick_conv_R[idx_floor]) * fract
                next_pos = self._cam_highlight_click_playback_pos + frames * self._cam_highlight_click_pitch_mult
                self._cam_highlight_click_playback_pos = next_pos if next_pos < (click_len - 1) else -1.0
            elif self.CAM_HIGHLIGHT_CLICK_WAVEFORM is not None:
                click_len = len(self.CAM_HIGHLIGHT_CLICK_WAVEFORM)
                indices = self._cam_highlight_click_playback_pos + np.arange(frames) * self._cam_highlight_click_pitch_mult
                valid_mask = indices < (click_len - 1)
                if np.any(valid_mask):
                    valid_indices = indices[valid_mask]
                    idx_floor, fract = np.floor(valid_indices).astype(int), valid_indices - np.floor(valid_indices)
                    sample1, sample2 = self.CAM_HIGHLIGHT_CLICK_WAVEFORM[idx_floor], self.CAM_HIGHLIGHT_CLICK_WAVEFORM[idx_floor + 1]
                    click_segment = sample1 + (sample2 - sample1) * fract
                    Lg, Rg = self._pan_gains(self._cam_highlight_click_pan_pos)
                    bufL[valid_mask] += click_segment * Lg
                    bufR[valid_mask] += click_segment * Rg
                next_pos = self._cam_highlight_click_playback_pos + frames * self._cam_highlight_click_pitch_mult
                self._cam_highlight_click_playback_pos = next_pos if next_pos < (click_len - 1) else -1.0

        if shift_on:
            t = (np.arange(frames) * self._phase_inc_shift + self._phase_shift) % 1.0
            s = self.shift_amp * (2.0 * np.abs(2.0 * t - 1.0) - 1.0)
            bufL += s; bufR += s
            if frames > 0: self._phase_shift = (t[-1] + self._phase_inc_shift) % 1.0

        if pt_on:
            tau, dt = PEDAL_SMOOTH_MS/1000.0, frames/self.samplerate
            beta = 1.0 - math.exp(-dt / max(1e-6, tau))
            self._sm_c += beta * (v_clutch - self._sm_c)
            self._sm_b += beta * (v_brake - self._sm_b)
            self._sm_t += beta * (v_throt - self._sm_t)
            
            pans = {'c': -1/3, 'b': 0, 't': 1/3}
            gains = {k: self._pan_gains(p) for k, p in pans.items()}
            
            for k, sm, detune in [('c', self._sm_c, self._detune_c), ('b', self._sm_b, self._detune_b), ('t', self._sm_t, self._detune_t)]:
                f = (PEDAL_BASE_FREQ + detune) * (1.0 + (PEDAL_MAX_MULT - 1.0) * sm)
                inc = f / self.samplerate
                phase_attr = f'_phase_{k}'
                phase = getattr(self, phase_attr)
                t = (np.arange(frames) * inc + phase) % 1.0
                s = np.sin(2.0 * np.pi * t)
                a = self._amp_from_val(sm)
                bufL += (a * s * gains[k][0]).astype(np.float32)
                bufR += (a * s * gains[k][1]).astype(np.float32)
                if frames > 0: setattr(self, phase_attr, (t[-1] + inc) % 1.0)
            
            # Steering Tone (Square Wave)
            # v_steer is normalized steering_input in -1..1
            s_clamped = self._clamp(v_steer, -1.0, 1.0)
            if abs(s_clamped) > 0.0005: # Minimal deadzone (matches old 0.05/100)
                s_norm = abs(s_clamped)
                freq = STEER_BASE_FREQ + (STEER_MAX_FREQ - STEER_BASE_FREQ) * s_norm
                inc = freq / self.samplerate
                t = (np.arange(frames) * inc + self._phase_steer) % 1.0
                
                # Simple Band-Limited Square Approximation (Fundamental + odd harmonics)
                # or just raw sign(sin) for a "true" harsh square
                # Using sign(sin) for requested "square wave"
                sq = np.sign(np.sin(2.0 * np.pi * t))
                
                amp = float(10.0 ** (STEER_AMP_DB / 20.0))
                
                # Pan: FLIPPED per user request
                # Previous: Left if < 0. New: Right if < 0.
                # Previous: Right if > 0. New: Left if > 0.
                pan_left = 1.0 if s_clamped < 0 else 0.0
                pan_right = 1.0 if s_clamped > 0 else 0.0
                
                bufL += (amp * sq * pan_left).astype(np.float32)
                bufR += (amp * sq * pan_right).astype(np.float32)
                
                if frames > 0: self._phase_steer = (t[-1] + inc) % 1.0

        if not inv and self._pitch_roll_enabled:
            r_norm = self._norm_from_angle_deg(math.degrees(roll_rad), cap=ROLL_CAP_DEG)
            p_norm = self._norm_from_angle_deg(math.degrees(pitch_rad), cap=PITCH_CAP_DEG)
            tau, dt = ATT_SMOOTH_MS / 1000.0, frames / self.samplerate
            beta = 1.0 - math.exp(-dt / max(1e-6, tau))
            self._sm_roll_int += beta * (r_norm - self._sm_roll_int)
            self._sm_pitch_int += beta * (p_norm - self._sm_pitch_int)

            # --- Stability Fade Logic ---
            # Calculate instability (lag between instantaneous target and smoothed value)
            roll_instability = abs(r_norm - self._sm_roll_int)
            pitch_instability = abs(p_norm - self._sm_pitch_int)

            # Roll Fade
            target_r_gain = self._clamp(roll_instability * ATT_FADE_SENSITIVITY + ATT_FADE_MIN_GAIN, 0.0, 1.0)
            if target_r_gain > self._roll_fade_mult:
                tau_f = ATT_FADE_ATTACK_MS / 1000.0
            else:
                tau_f = ATT_FADE_DECAY_MS / 1000.0
            beta_f = 1.0 - math.exp(-dt / max(1e-6, tau_f))
            self._roll_fade_mult += beta_f * (target_r_gain - self._roll_fade_mult)

            # Pitch Fade
            target_p_gain = self._clamp(pitch_instability * ATT_FADE_SENSITIVITY + ATT_FADE_MIN_GAIN, 0.0, 1.0)
            if target_p_gain > self._pitch_fade_mult:
                tau_f = ATT_FADE_ATTACK_MS / 1000.0
            else:
                tau_f = ATT_FADE_DECAY_MS / 1000.0
            beta_f = 1.0 - math.exp(-dt / max(1e-6, tau_f))
            self._pitch_fade_mult += beta_f * (target_p_gain - self._pitch_fade_mult)
            # ----------------------------

            if self._sm_roll_int > 1e-3:
                f_roll = ATT_BASE_ROLL_HZ * (1.0 + 3.0 * self._clamp(self._sm_roll_int))
                inc_r = f_roll / self.samplerate
                t_r = (np.arange(frames) * inc_r + self._phase_roll) % 1.0
                pulse = np.where(t_r < 0.35, 1.0, -1.0)
                a_r = self._amp_att_from_norm(self._sm_roll_int) * self._roll_fade_mult
                mono_roll = (a_r * pulse).astype(np.float32)

                if self._hrtf is not None and self._hrtf_user_enabled:
                    # Map roll direction to HRTF azimuth: left roll→90°, right roll→270°
                    pan_pos = self._clamp(math.copysign(1.0, roll_rad) * self._sm_roll_int, -1.0, 1.0)
                    hrtf_az = (-pan_pos * 90.0) % 360.0
                    ir_l, ir_r = self._hrtf.get_hrir(hrtf_az)
                    if ir_l is not None:
                        # Distance: shallow roll=close (0dB), steep roll=far
                        dist_db = ATT_HRTF_FAR_DB * self._clamp(self._sm_roll_int)
                        dist_gain = float(10.0 ** (dist_db / 20.0))
                        # Overlap-add convolution
                        conv_l = np.convolve(mono_roll, ir_l, mode='full') * dist_gain
                        conv_r = np.convolve(mono_roll, ir_r, mode='full') * dist_gain
                        if self._roll_hrtf_overlap_L is not None:
                            ol = min(len(self._roll_hrtf_overlap_L), len(conv_l))
                            conv_l[:ol] += self._roll_hrtf_overlap_L[:ol]
                            conv_r[:ol] += self._roll_hrtf_overlap_R[:ol]
                        bufL += conv_l[:frames].astype(np.float32)
                        bufR += conv_r[:frames].astype(np.float32)
                        self._roll_hrtf_overlap_L = conv_l[frames:].copy()
                        self._roll_hrtf_overlap_R = conv_r[frames:].copy()
                    else:
                        Lg, Rg = self._pan_gains(pan_pos)
                        bufL += (mono_roll * Lg); bufR += (mono_roll * Rg)
                        self._roll_hrtf_overlap_L = None
                        self._roll_hrtf_overlap_R = None
                else:
                    pan_pos = self._clamp(math.copysign(1.0, roll_rad) * self._sm_roll_int, -1.0, 1.0)
                    Lg, Rg = self._pan_gains(pan_pos)
                    bufL += (mono_roll * Lg); bufR += (mono_roll * Rg)
                    self._roll_hrtf_overlap_L = None
                    self._roll_hrtf_overlap_R = None

                if frames > 0: self._phase_roll = (t_r[-1] + inc_r) % 1.0
            else:
                # Drain any residual HRTF overlap when roll tone stops
                if self._roll_hrtf_overlap_L is not None and len(self._roll_hrtf_overlap_L) > 0:
                    ol = min(len(self._roll_hrtf_overlap_L), frames)
                    bufL[:ol] += self._roll_hrtf_overlap_L[:ol].astype(np.float32)
                    bufR[:ol] += self._roll_hrtf_overlap_R[:ol].astype(np.float32)
                    self._roll_hrtf_overlap_L = None
                    self._roll_hrtf_overlap_R = None

            if self._sm_pitch_int > 1e-3:
                f_pitch = ATT_BASE_PITCH_HZ * (1.0 + 3.0 * self._clamp(self._sm_pitch_int))
                inc_p = f_pitch / self.samplerate
                t_p = (np.arange(frames) * inc_p + self._phase_pitch) % 1.0
                tri = 2.0 * np.abs(2.0 * t_p - 1.0) - 1.0
                shape = tri
                if pitch_rad >= 0.0:
                    ang = 2.0 * np.pi * t_p
                    amt = float(self._clamp(self._sm_pitch_int))
                    h3, h5 = 0.35 * amt, 0.15 * amt
                    shape = (tri + h3 * np.sin(3.0 * ang) + h5 * np.sin(5.0 * ang)) / (1.0 + abs(h3) + abs(h5))
                a_p = self._amp_att_from_norm(self._sm_pitch_int) * self._pitch_fade_mult
                mono_pitch = (a_p * shape).astype(np.float32)

                if self._hrtf is not None and self._hrtf_user_enabled:
                    # Nose down (pitch_rad<0) → front (0°), nose up (pitch_rad>=0) → back (180°)
                    hrtf_az = 180.0 if pitch_rad >= 0.0 else 0.0
                    ir_l, ir_r = self._hrtf.get_hrir(hrtf_az)
                    if ir_l is not None:
                        dist_db = ATT_HRTF_FAR_DB * self._clamp(self._sm_pitch_int)
                        dist_gain = float(10.0 ** (dist_db / 20.0))
                        conv_l = np.convolve(mono_pitch, ir_l, mode='full') * dist_gain
                        conv_r = np.convolve(mono_pitch, ir_r, mode='full') * dist_gain
                        if self._pitch_hrtf_overlap_L is not None:
                            ol = min(len(self._pitch_hrtf_overlap_L), len(conv_l))
                            conv_l[:ol] += self._pitch_hrtf_overlap_L[:ol]
                            conv_r[:ol] += self._pitch_hrtf_overlap_R[:ol]
                        bufL += conv_l[:frames].astype(np.float32)
                        bufR += conv_r[:frames].astype(np.float32)
                        self._pitch_hrtf_overlap_L = conv_l[frames:].copy()
                        self._pitch_hrtf_overlap_R = conv_r[frames:].copy()
                    else:
                        Lg, Rg = self._pan_gains(0.0)
                        bufL += (mono_pitch * Lg); bufR += (mono_pitch * Rg)
                        self._pitch_hrtf_overlap_L = None
                        self._pitch_hrtf_overlap_R = None
                else:
                    Lg, Rg = self._pan_gains(0.0)
                    bufL += (mono_pitch * Lg); bufR += (mono_pitch * Rg)
                    self._pitch_hrtf_overlap_L = None
                    self._pitch_hrtf_overlap_R = None

                if frames > 0: self._phase_pitch = (t_p[-1] + inc_p) % 1.0
            else:
                # Drain any residual HRTF overlap when pitch tone stops
                if self._pitch_hrtf_overlap_L is not None and len(self._pitch_hrtf_overlap_L) > 0:
                    ol = min(len(self._pitch_hrtf_overlap_L), frames)
                    bufL[:ol] += self._pitch_hrtf_overlap_L[:ol].astype(np.float32)
                    bufR[:ol] += self._pitch_hrtf_overlap_R[:ol].astype(np.float32)
                    self._pitch_hrtf_overlap_L = None
                    self._pitch_hrtf_overlap_R = None
        else:
            self._sm_roll_int *= 0.5
            self._sm_pitch_int *= 0.5
        
        # NEW: Drift Detection Logic
        # Sine with 7th and 11th harmonics
        if self._drift_alert_active:
            # Calculate pitch multiplier based on drift rate severity
            # Map 0.5 (threshold) -> 1.0x
            # Map 10.0 (max severity) -> 4.0x
            rate = max(0.5, self._drift_rate)
            norm = self._clamp((rate - 0.5) / (10.0 - 0.5), 0.0, 1.0)
            pitch_mult = 1.0 + 3.0 * norm
            
            inc = (DRIFT_FREQ_HZ * pitch_mult) / self.samplerate
            t = (np.arange(frames) * inc + self._phase_drift) % 1.0
            
            # Base + 7th + 11th
            tone = (1.0 * np.sin(2.0 * np.pi * t) + 
                    0.5 * np.sin(2.0 * np.pi * 7.0 * t) + 
                    0.3 * np.sin(2.0 * np.pi * 11.0 * t))
            
            # Normalize peak roughly
            tone *= 0.55
            
            amp = float(10.0 ** (DRIFT_AMP_DB / 20.0))
            tone *= amp
            
            # Hard Pan based on drift direction (-1 Left, 1 Right)
            # Drift Left -> Left Ear, Drift Right -> Right Ear
            # _drift_pan is -1.0 or 1.0
            Lg = 1.0 if self._drift_pan < 0 else 0.0
            Rg = 1.0 if self._drift_pan > 0 else 0.0
            
            bufL += (tone * Lg).astype(np.float32)
            bufR += (tone * Rg).astype(np.float32)

            if frames > 0: self._phase_drift = (t[-1] + inc) % 1.0


        # NEW: Vehicle Scanner audio logic (suppressed when coupler tracking is active)
        scanner_produced_audio = False
        if scan_active and scan_dist != float('inf') and not coupler_active:
            # Beep repetition rate from distance (exponential). Half-distance scales with the CLOSING
            # speed (how fast the distance is shrinking) rather than raw vehicle speed, so heading
            # toward the target speeds the beeps up while heading away slows them down. Clamped to a
            # positive minimum so the exp() denominator stays valid when receding fast.
            half_dist = max(SCANNER_HALF_DIST_MIN_M,
                            SCANNER_HALF_DIST_BASE_M + scan_closing_ms * SCANNER_HALF_DIST_PER_MS)
            rate_norm = math.exp(-scan_dist / half_dist)
            rate_hz = SCANNER_MIN_RATE_HZ + (SCANNER_MAX_RATE_HZ - SCANNER_MIN_RATE_HZ) * rate_norm
            interval_sec = 1.0 / rate_hz

            # Bearing-derived cues, shared by the beep train and the steady tone.
            # Pitch = base_freq * 2^(offset_oct * proximity); proximity blends a broad linear ramp
            # (back->front) with a sharp alignment term so the pitch climbs continuously toward front
            # and peaks at dead-center. base_freq/offset_oct are user-configurable so the top
            # frequency can be tamed. pitch_mult resamples the beep waveform (rendered at BEEP_FREQ).
            aligned = abs(scan_bearing) <= SCANNER_ALIGN_THRESHOLD_DEG
            pitch_norm = 1.0 - (abs(scan_bearing) / 180.0)  # 1.0 front, 0.0 back (broad ramp)
            align_norm = math.exp(-abs(scan_bearing) / SCANNER_ALIGN_PITCH_DECAY_DEG)
            proximity = SCANNER_ALIGN_WEIGHT * align_norm + (1.0 - SCANNER_ALIGN_WEIGHT) * pitch_norm
            scan_freq = scan_base_freq * (2.0 ** (scan_offset_oct * proximity))
            pitch_mult = scan_freq / SCANNER_BEEP_FREQ_HZ
            hrtf_az_scan = scan_bearing % 360.0

            # Steering-locked morph: 0 = pure beeps, 1 = steady tone. Smoothed per block so the
            # transition is click-free. Holding a turn converts the (possibly slow) beeps into a
            # continuous directional tone for locking onto the target; re-centering restores beeps.
            # Disabled entirely when the user opts out (config).
            steer_mag = abs(v_steer) if scan_steer_tone_enabled else 0.0
            tone_target = (steer_mag - SCANNER_STEER_TONE_DEADZONE) / \
                max(1e-6, SCANNER_STEER_TONE_FULL - SCANNER_STEER_TONE_DEADZONE)
            tone_target = min(1.0, max(0.0, tone_target))
            self._scan_tone_level += SCANNER_TONE_LEVEL_SMOOTH * (tone_target - self._scan_tone_level)
            tone_level = self._scan_tone_level
            beep_gain = 1.0 - tone_level

            # Check if it's time to trigger a new beep
            self._scanner_beep_timer += frames / self.samplerate
            if self._scanner_beep_timer >= interval_sec:
                self._scanner_beep_timer = 0
                self._scanner_playback_pos = 0.0

            # Mix the beep if it's currently playing
            if self._scanner_playback_pos >= 0 and self.SCANNER_BEEP_WAVEFORM is not None:
                # Select waveform: bright aligned variant when within threshold
                active_wf = self.SCANNER_ALIGNED_WAVEFORM if (aligned and self.SCANNER_ALIGNED_WAVEFORM is not None) else self.SCANNER_BEEP_WAVEFORM
                beep_len = len(active_wf)

                indices = self._scanner_playback_pos + np.arange(frames) * pitch_mult
                valid_mask = indices < (beep_len - 1)

                if np.any(valid_mask):
                    valid_indices = indices[valid_mask]
                    idx_floor = valid_indices.astype(int)
                    fract = valid_indices - idx_floor
                    sample1, sample2 = active_wf[idx_floor], active_wf[idx_floor + 1]
                    beep_segment = sample1 + (sample2 - sample1) * fract

                    # Build a full-frame mono buffer for this beep (needed for HRTF convolution).
                    # beep_gain crossfades the beep out as the steering-locked steady tone fades in.
                    mono_scan = np.zeros(frames, dtype=np.float32)
                    mono_scan[valid_mask] = beep_segment

                    # Diagnostic capture (only populated when the diag is enabled).
                    diag = self._scanner_diag
                    d_hrtf_used = False
                    d_ir_l = d_ir_r = None
                    d_out_l = d_out_r = None

                    if self._hrtf is not None and self._hrtf_user_enabled:
                        ir_l, ir_r = self._hrtf.get_hrir(hrtf_az_scan)
                        if ir_l is not None:
                            conv_l = np.convolve(mono_scan, ir_l, mode='full')
                            conv_r = np.convolve(mono_scan, ir_r, mode='full')
                            if self._scanner_overlap_L is not None:
                                ol = min(len(self._scanner_overlap_L), len(conv_l))
                                conv_l[:ol] += self._scanner_overlap_L[:ol]
                                conv_r[:ol] += self._scanner_overlap_R[:ol]
                            bufL += (beep_gain * conv_l[:frames]).astype(np.float32)
                            bufR += (beep_gain * conv_r[:frames]).astype(np.float32)
                            self._scanner_overlap_L = conv_l[frames:].copy()
                            self._scanner_overlap_R = conv_r[frames:].copy()
                            scanner_produced_audio = True
                            if diag is not None and diag.enabled:
                                d_hrtf_used = True
                                d_ir_l, d_ir_r = ir_l, ir_r
                                # Record the exact post-overlap samples written this frame.
                                d_out_l, d_out_r = conv_l[:frames], conv_r[:frames]
                        else:
                            # HRTF IR unavailable: proportional stereo pan (negated for correct direction)
                            pan_pos = -scan_bearing / 90.0
                            Lg, Rg = self._pan_gains(pan_pos)
                            bufL[valid_mask] += beep_gain * beep_segment * Lg
                            bufR[valid_mask] += beep_gain * beep_segment * Rg
                            if diag is not None and diag.enabled:
                                d_out_l, d_out_r = beep_segment * Lg, beep_segment * Rg
                    else:
                        # No HRTF: proportional stereo pan (negated for correct direction)
                        pan_pos = -scan_bearing / 90.0
                        Lg, Rg = self._pan_gains(pan_pos)
                        bufL[valid_mask] += beep_gain * beep_segment * Lg
                        bufR[valid_mask] += beep_gain * beep_segment * Rg
                        if diag is not None and diag.enabled:
                            d_out_l, d_out_r = beep_segment * Lg, beep_segment * Rg

                    if diag is not None and diag.enabled:
                        diag.record_beep(
                            bearing_deg=scan_bearing, dist_m=scan_dist, aligned=aligned,
                            pitch_norm=pitch_norm, pitch_mult=pitch_mult,
                            hrtf_used=d_hrtf_used, hrtf_az_deg=hrtf_az_scan,
                            ir_l=d_ir_l, ir_r=d_ir_r, out_l=d_out_l, out_r=d_out_r,
                        )

                next_pos = self._scanner_playback_pos + frames * pitch_mult
                self._scanner_playback_pos = next_pos if next_pos < (beep_len - 1) else -1.0

            # Drain the beep's HRTF tail (crossfaded) when no new beep audio played this frame.
            if not scanner_produced_audio and self._scanner_overlap_L is not None:
                ol = min(len(self._scanner_overlap_L), frames)
                if ol > 0:
                    bufL[:ol] += (beep_gain * self._scanner_overlap_L[:ol]).astype(np.float32)
                    bufR[:ol] += (beep_gain * self._scanner_overlap_R[:ol]).astype(np.float32)
                self._scanner_overlap_L = None
                self._scanner_overlap_R = None

            # Steering-locked steady tone: a continuous, HRTF-positioned triangle whose pitch tracks
            # the same bearing->pitch mapping as the beeps. Fills the gaps between (slow) beeps so the
            # player can hold a turn and hear an unbroken directional lock on the target.
            if tone_level > 1e-3:
                inc = scan_freq / self.samplerate  # cycles/sample (same Hz as the beep at this bearing)
                ph = self._scan_tone_phase + inc * np.arange(frames)
                tri = (2.0 / np.pi) * np.arcsin(np.sin(2.0 * np.pi * ph))
                mono_tone = (tri * (SCANNER_TONE_AMP * tone_level)).astype(np.float32)
                self._scan_tone_phase = float((self._scan_tone_phase + inc * frames) % 1.0)

                used_hrtf_tone = False
                if self._hrtf is not None and self._hrtf_user_enabled:
                    ir_l, ir_r = self._hrtf.get_hrir(hrtf_az_scan)
                    if ir_l is not None:
                        tl = np.convolve(mono_tone, ir_l, mode='full')
                        tr = np.convolve(mono_tone, ir_r, mode='full')
                        if self._scan_tone_overlap_L is not None:
                            ol = min(len(self._scan_tone_overlap_L), len(tl))
                            tl[:ol] += self._scan_tone_overlap_L[:ol]
                            tr[:ol] += self._scan_tone_overlap_R[:ol]
                        bufL += tl[:frames].astype(np.float32)
                        bufR += tr[:frames].astype(np.float32)
                        self._scan_tone_overlap_L = tl[frames:].copy()
                        self._scan_tone_overlap_R = tr[frames:].copy()
                        used_hrtf_tone = True
                if not used_hrtf_tone:
                    pan_pos = -scan_bearing / 90.0
                    Lg, Rg = self._pan_gains(pan_pos)
                    bufL += mono_tone * Lg
                    bufR += mono_tone * Rg
                    self._scan_tone_overlap_L = None
                    self._scan_tone_overlap_R = None
            elif self._scan_tone_overlap_L is not None:
                # Drain/clear the steady tone's HRTF tail as it fades out.
                ol = min(len(self._scan_tone_overlap_L), frames)
                if ol > 0:
                    bufL[:ol] += self._scan_tone_overlap_L[:ol].astype(np.float32)
                    bufR[:ol] += self._scan_tone_overlap_R[:ol].astype(np.float32)
                self._scan_tone_overlap_L = None
                self._scan_tone_overlap_R = None

        else:
            # Scanner inactive (or coupler tracking): clear all tails and relax the morph level so
            # the next activation starts cleanly.
            self._scanner_overlap_L = None
            self._scanner_overlap_R = None
            self._scan_tone_overlap_L = None
            self._scan_tone_overlap_R = None
            self._scan_tone_level = 0.0

        # Node grabber hover beep (pitch varies by node height, forward on enter, reverse on leave)
        if self._node_beep_playback_pos >= 0 and self.NODE_BEEP_WAVEFORM is not None:
            beep_wav = self.NODE_BEEP_REV_WAVEFORM if self._node_beep_reverse else self.NODE_BEEP_WAVEFORM
            beep_len = len(beep_wav)
            pitch_mult = self._node_beep_freq / NODE_BEEP_BASE_FREQ_HZ
            indices = self._node_beep_playback_pos + np.arange(frames) * pitch_mult
            valid_mask = indices < (beep_len - 1)
            if np.any(valid_mask):
                valid_indices = indices[valid_mask]
                idx_floor = np.floor(valid_indices).astype(int)
                fract = valid_indices - idx_floor
                segment = beep_wav[idx_floor] + (beep_wav[idx_floor + 1] - beep_wav[idx_floor]) * fract
                bufL[valid_mask] += segment
                bufR[valid_mask] += segment
            next_pos = self._node_beep_playback_pos + frames * pitch_mult
            self._node_beep_playback_pos = next_pos if next_pos < (beep_len - 1) else -1.0

        # Clickspot hover beep (FM click — forward on enter, reverse waveform on leave)
        if self._clickspot_beep_playback_pos >= 0:
            beep_wav = self.CLICKSPOT_BEEP_REV_WAVEFORM if self._clickspot_beep_reverse else self.CLICKSPOT_BEEP_WAVEFORM
            if beep_wav is not None:
                beep_len = len(beep_wav)
                start = int(self._clickspot_beep_playback_pos)
                num_to_mix = min(frames, beep_len - start)
                if num_to_mix > 0:
                    segment = beep_wav[start : start + num_to_mix]
                    bufL[:num_to_mix] += segment
                    bufR[:num_to_mix] += segment
                next_pos = self._clickspot_beep_playback_pos + frames
                self._clickspot_beep_playback_pos = next_pos if next_pos < beep_len else -1.0

        # Coupler tracking audio (continuous sine with HRTF, beeping when in range)
        coupler_produced_audio = False
        if coupler_active and coupler_dist != float('inf'):
            amp = 10.0 ** (COUPLER_TONE_AMP_DB / 20.0)

            if coupler_in_range:
                # Rapid beeping mode - pitch higher, rate increases as distance decreases
                pitch_mult = COUPLER_BEEP_PITCH
                range_frac = max(0.0, 1.0 - (coupler_dist / COUPLER_RANGE_M))
                beep_rate = COUPLER_BEEP_MIN_RATE + (COUPLER_BEEP_MAX_RATE - COUPLER_BEEP_MIN_RATE) * range_frac
                beep_period = 1.0 / beep_rate
                beep_on = COUPLER_BEEP_DUR_MS / 1000.0

                freq = COUPLER_TONE_FREQ_HZ * pitch_mult
                inc = freq / self.samplerate
                t_phase = self._coupler_phase + np.arange(frames) * inc
                tone = np.sin(2.0 * np.pi * t_phase).astype(np.float32) * amp

                # Beep gate envelope
                t_env = self._coupler_beep_timer + np.arange(frames) / self.samplerate
                gate = np.where((t_env % beep_period) < beep_on, 1.0, 0.0).astype(np.float32)
                tone *= gate

                self._coupler_phase = (t_phase[-1] + inc) % 1.0
                self._coupler_beep_timer = t_env[-1] + (1.0 / self.samplerate)
            else:
                # Continuous tone, pitch scales with distance
                dist_norm = math.exp(-coupler_dist / COUPLER_HALF_DIST_M)
                pitch_mult = COUPLER_MIN_PITCH + (COUPLER_MAX_PITCH - COUPLER_MIN_PITCH) * dist_norm
                freq = COUPLER_TONE_FREQ_HZ * pitch_mult
                inc = freq / self.samplerate
                t_phase = self._coupler_phase + np.arange(frames) * inc
                tone = np.sin(2.0 * np.pi * t_phase).astype(np.float32) * amp

                self._coupler_phase = (t_phase[-1] + inc) % 1.0
                self._coupler_beep_timer = 0.0

            # HRTF spatial panning based on coupler bearing
            hrtf_az = coupler_bearing % 360.0

            if self._hrtf is not None and self._hrtf_user_enabled:
                ir_l, ir_r = self._hrtf.get_hrir(hrtf_az)
                if ir_l is not None:
                    conv_l = np.convolve(tone, ir_l, mode='full')
                    conv_r = np.convolve(tone, ir_r, mode='full')
                    if self._coupler_overlap_L is not None:
                        ol = min(len(self._coupler_overlap_L), len(conv_l))
                        conv_l[:ol] += self._coupler_overlap_L[:ol]
                        conv_r[:ol] += self._coupler_overlap_R[:ol]
                    bufL += conv_l[:frames].astype(np.float32)
                    bufR += conv_r[:frames].astype(np.float32)
                    self._coupler_overlap_L = conv_l[frames:].copy()
                    self._coupler_overlap_R = conv_r[frames:].copy()
                    coupler_produced_audio = True
                else:
                    pan_pos = -coupler_bearing / 90.0
                    Lg, Rg = self._pan_gains(pan_pos)
                    bufL += tone * Lg
                    bufR += tone * Rg
            else:
                pan_pos = -coupler_bearing / 90.0
                Lg, Rg = self._pan_gains(pan_pos)
                bufL += tone * Lg
                bufR += tone * Rg

        # Drain coupler HRTF tail when no audio was produced this frame
        if not coupler_produced_audio:
            if self._coupler_overlap_L is not None and len(self._coupler_overlap_L) > 0:
                ol = min(len(self._coupler_overlap_L), frames)
                bufL[:ol] += self._coupler_overlap_L[:ol].astype(np.float32)
                bufR[:ol] += self._coupler_overlap_R[:ol].astype(np.float32)
            self._coupler_overlap_L = None
            self._coupler_overlap_R = None

        # NEW: Obstacle Detection audio logic
        if obs_active and self.OBSTACLE_BUZZ_WAVEFORM is not None:
            base_buzz = self.OBSTACLE_BUZZ_WAVEFORM
            dt_obs = frames / self.samplerate
            use_hrtf = self._hrtf is not None and self._hrtf_user_enabled

            for q in range(NUM_OBSTACLE_QUADRANTS):
                if obs_types[q] == 0 or obs_distances[q] == float('inf'):
                    self._obstacle_playback_pos[q] = -1.0
                    self._obstacle_pulse_L[q] = None
                    self._obstacle_pulse_R[q] = None
                    continue

                dist = obs_distances[q]
                bearing = obs_bearings[q]

                trigger = False
                if dist <= OBSTACLE_CONTINUOUS_DIST:
                    # Imminent: retrigger as soon as the previous pulse finishes
                    if self._obstacle_playback_pos[q] < 0:
                        trigger = True
                else:
                    # Rate-based buzzing
                    rate_norm = math.exp(-dist / OBSTACLE_HALF_DIST_M)
                    rate_hz = OBSTACLE_MIN_RATE_HZ + (OBSTACLE_MAX_RATE_HZ - OBSTACLE_MIN_RATE_HZ) * rate_norm
                    interval_sec = 1.0 / rate_hz

                    self._obstacle_buzz_timers[q] += dt_obs
                    if self._obstacle_buzz_timers[q] >= interval_sec:
                        self._obstacle_buzz_timers[q] = 0.0
                        trigger = True

                if trigger:
                    # Distance-based volume rolloff: full near, exponential decay with range
                    dist_gain = math.exp(-max(0.0, dist - OBSTACLE_CONTINUOUS_DIST) / OBSTACLE_HALF_DIST_M)
                    dist_gain = max(0.05, dist_gain)
                    scaled = (base_buzz * dist_gain).astype(np.float32)

                    rendered = False
                    if use_hrtf:
                        hrtf_az = bearing % 360.0
                        ir_l, ir_r = self._hrtf.get_hrir(hrtf_az)
                        if ir_l is not None:
                            self._obstacle_pulse_L[q] = np.convolve(scaled, ir_l, mode='full').astype(np.float32)
                            self._obstacle_pulse_R[q] = np.convolve(scaled, ir_r, mode='full').astype(np.float32)
                            rendered = True
                    if not rendered:
                        pan_pos = -bearing / 90.0
                        Lg, Rg = self._pan_gains(pan_pos)
                        self._obstacle_pulse_L[q] = (scaled * Lg).astype(np.float32)
                        self._obstacle_pulse_R[q] = (scaled * Rg).astype(np.float32)
                    self._obstacle_playback_pos[q] = 0.0

                # Mix any currently-playing pulse
                if self._obstacle_playback_pos[q] >= 0 and self._obstacle_pulse_L[q] is not None:
                    pulse_L = self._obstacle_pulse_L[q]
                    pulse_R = self._obstacle_pulse_R[q]
                    pulse_len = len(pulse_L)
                    start_i = int(self._obstacle_playback_pos[q])
                    num_to_mix = min(frames, pulse_len - start_i)
                    if num_to_mix > 0:
                        bufL[:num_to_mix] += pulse_L[start_i:start_i + num_to_mix]
                        bufR[:num_to_mix] += pulse_R[start_i:start_i + num_to_mix]
                    next_pos = start_i + frames
                    if next_pos >= pulse_len:
                        self._obstacle_playback_pos[q] = -1.0
                    else:
                        self._obstacle_playback_pos[q] = float(next_pos)

            # Terrain warning sweep playback
            if self._terrain_playback_pos >= 0:
                sweep_wf = self.DROPOFF_SWEEP_WAVEFORM if terrain_type == 2 else self.HILL_SWEEP_WAVEFORM
                if sweep_wf is not None:
                    sweep_len = len(sweep_wf)
                    pos = int(self._terrain_playback_pos)
                    num_to_mix = min(frames, sweep_len - pos)
                    if num_to_mix > 0:
                        segment = sweep_wf[pos:pos + num_to_mix]
                        # Center-panned (ahead of vehicle)
                        bufL[:num_to_mix] += segment
                        bufR[:num_to_mix] += segment
                    next_pos = pos + frames
                    self._terrain_playback_pos = float(next_pos) if next_pos < sweep_len else -1.0

        # Road Detection: off-road guidance beep
        if road_active and not road_on_road and self.ROAD_BEEP_WAVEFORM is not None:
            base_pulse = self.ROAD_BEEP_WAVEFORM
            dt_rd = frames / self.samplerate
            use_hrtf = self._hrtf is not None and self._hrtf_user_enabled

            # Rate scales exponentially with proximity to road; floor at MIN, ceil at MAX
            rate_norm = math.exp(-max(0.0, road_distance) / ROAD_BEEP_HALF_DIST_M)
            rate_hz = ROAD_BEEP_MIN_RATE_HZ + (ROAD_BEEP_MAX_RATE_HZ - ROAD_BEEP_MIN_RATE_HZ) * rate_norm
            interval_sec = 1.0 / max(0.01, rate_hz)

            self._road_beep_timer += dt_rd
            trigger = False
            if self._road_beep_timer >= interval_sec:
                self._road_beep_timer = 0.0
                trigger = True

            if trigger:
                if use_hrtf:
                    hrtf_az = road_bearing % 360.0
                    ir_l, ir_r = self._hrtf.get_hrir(hrtf_az)
                    if ir_l is not None:
                        self._road_pulse_L = np.convolve(base_pulse, ir_l, mode='full').astype(np.float32)
                        self._road_pulse_R = np.convolve(base_pulse, ir_r, mode='full').astype(np.float32)
                    else:
                        pan_pos = -road_bearing / 90.0
                        Lg, Rg = self._pan_gains(pan_pos)
                        self._road_pulse_L = (base_pulse * Lg).astype(np.float32)
                        self._road_pulse_R = (base_pulse * Rg).astype(np.float32)
                else:
                    pan_pos = -road_bearing / 90.0
                    Lg, Rg = self._pan_gains(pan_pos)
                    self._road_pulse_L = (base_pulse * Lg).astype(np.float32)
                    self._road_pulse_R = (base_pulse * Rg).astype(np.float32)
                self._road_playback_pos = 0.0

            if self._road_playback_pos >= 0 and self._road_pulse_L is not None:
                pulse_L = self._road_pulse_L
                pulse_R = self._road_pulse_R
                pulse_len = len(pulse_L)
                start_i = int(self._road_playback_pos)
                num_to_mix = min(frames, pulse_len - start_i)
                if num_to_mix > 0:
                    bufL[:num_to_mix] += pulse_L[start_i:start_i + num_to_mix]
                    bufR[:num_to_mix] += pulse_R[start_i:start_i + num_to_mix]
                next_pos = start_i + frames
                if next_pos >= pulse_len:
                    self._road_playback_pos = -1.0
                else:
                    self._road_playback_pos = float(next_pos)

        # Road Orientation Chime: drain the pending two-tone queue
        if road_active and self._road_chime_queue:
            with self.lock:
                queue = self._road_chime_queue
                new_queue = []
                for entry in queue:
                    delay = entry["delay"]
                    L = entry["L"]
                    R = entry["R"]
                    pos = entry["pos"]

                    offset = 0
                    if delay >= frames:
                        entry["delay"] = delay - frames
                        new_queue.append(entry)
                        continue
                    if delay > 0:
                        offset = delay
                        entry["delay"] = 0

                    pulse_len = len(L)
                    slot = frames - offset
                    n = min(slot, pulse_len - pos)
                    if n > 0:
                        bufL[offset:offset + n] += L[pos:pos + n]
                        bufR[offset:offset + n] += R[pos:pos + n]
                    new_pos = pos + n
                    if new_pos < pulse_len:
                        entry["pos"] = new_pos
                        new_queue.append(entry)
                self._road_chime_queue = new_queue

        # NEW: Low Speed Detection Logic
        dt_ls = frames / self.samplerate
        if ls_active:
            # Fade envelope: attack toward 1.0
            alpha_att = 1.0 - math.exp(-dt_ls / max(1e-6, LS_FADE_ATTACK_S))
            self._ls_fade_gain += alpha_att * (1.0 - self._ls_fade_gain)
        else:
            # Fade envelope: decay toward 0.0 (rapid at 0 mph — within one click duration)
            decay_s = (LS_CLICK_DUR_MS / 1000.0 / 3.0) if ls_speed_mph < 0.5 else LS_FADE_DECAY_S
            alpha_dec = 1.0 - math.exp(-dt_ls / max(1e-6, decay_s))
            self._ls_fade_gain += alpha_dec * (0.0 - self._ls_fade_gain)

        if self._ls_fade_gain > 0.001:
            # Click rate from deceleration
            decel_norm = self._clamp(ls_decel / LS_DECEL_FOR_MAX_RATE, 0.0, 1.0)
            rate_hz = LS_MIN_RATE_HZ + (LS_MAX_RATE_HZ - LS_MIN_RATE_HZ) * decel_norm
            interval_sec = 1.0 / rate_hz

            # Pitch from speed
            speed_norm = self._clamp(ls_speed_mph / 25.0, 0.0, 1.0)
            pitch = LS_PITCH_AT_0MPH + (LS_PITCH_AT_25MPH - LS_PITCH_AT_0MPH) * speed_norm

            # Timer-based triggering
            self._ls_click_timer += dt_ls
            if self._ls_click_timer >= interval_sec:
                self._ls_click_timer = 0.0
                self._ls_playback_pos = 0.0
                self._ls_pitch_mult = pitch

                # HRTF convolution at azimuth 0° (front)
                if self._hrtf is not None and self._hrtf_user_enabled and self.LS_CLICK_WAVEFORM is not None:
                    ir_l, ir_r = self._hrtf.get_hrir(LS_HRTF_AZIMUTH)
                    if ir_l is not None:
                        gain = self._hrtf_emphasis_gain(LS_HRTF_AZIMUTH) * self._hrtf_distance_gain
                        self._ls_conv_L = (np.convolve(self.LS_CLICK_WAVEFORM, ir_l, mode='full') * gain).astype(np.float32)
                        self._ls_conv_R = (np.convolve(self.LS_CLICK_WAVEFORM, ir_r, mode='full') * gain).astype(np.float32)
                        self._ls_use_hrtf = True
                    else:
                        self._ls_use_hrtf = False
                else:
                    self._ls_use_hrtf = False

            # Playback with pitch shifting
            if self._ls_playback_pos >= 0 and self.LS_CLICK_WAVEFORM is not None:
                if self._ls_use_hrtf and self._ls_conv_L is not None:
                    click_len = len(self._ls_conv_L)
                    indices = self._ls_playback_pos + np.arange(frames) * self._ls_pitch_mult
                    valid_mask = indices < (click_len - 1)
                    if np.any(valid_mask):
                        valid_indices = indices[valid_mask]
                        idx_floor = np.floor(valid_indices).astype(int)
                        fract = valid_indices - idx_floor
                        bufL[valid_mask] += (self._ls_conv_L[idx_floor] + (self._ls_conv_L[idx_floor + 1] - self._ls_conv_L[idx_floor]) * fract) * self._ls_fade_gain
                        bufR[valid_mask] += (self._ls_conv_R[idx_floor] + (self._ls_conv_R[idx_floor + 1] - self._ls_conv_R[idx_floor]) * fract) * self._ls_fade_gain
                    next_pos = self._ls_playback_pos + frames * self._ls_pitch_mult
                    self._ls_playback_pos = next_pos if next_pos < (click_len - 1) else -1.0
                else:
                    click_len = len(self.LS_CLICK_WAVEFORM)
                    indices = self._ls_playback_pos + np.arange(frames) * self._ls_pitch_mult
                    valid_mask = indices < (click_len - 1)
                    if np.any(valid_mask):
                        valid_indices = indices[valid_mask]
                        idx_floor = np.floor(valid_indices).astype(int)
                        fract = valid_indices - idx_floor
                        sample1 = self.LS_CLICK_WAVEFORM[idx_floor]
                        sample2 = self.LS_CLICK_WAVEFORM[idx_floor + 1]
                        click_segment = (sample1 + (sample2 - sample1) * fract) * self._ls_fade_gain
                        # Center-pan (front)
                        bufL[valid_mask] += click_segment
                        bufR[valid_mask] += click_segment
                    next_pos = self._ls_playback_pos + frames * self._ls_pitch_mult
                    self._ls_playback_pos = next_pos if next_pos < (click_len - 1) else -1.0
        else:
            # Reset when fully faded
            self._ls_click_timer = 0.0
            self._ls_playback_pos = -1.0

        # NEW: Heading Guidance Logic
        if guide_active:
            # Smooth the error to avoid zippers
            tau, dt = 0.1, frames / self.samplerate # 100ms smoothing
            beta = 1.0 - math.exp(-dt / max(1e-6, tau))
            self._sm_guidance_error += beta * (guide_error - self._sm_guidance_error)
            
            abs_err = abs(self._sm_guidance_error)
            
            if abs_err > GUIDANCE_DEADZONE_DEG:
                # Calculate amplitude
                # Map error from [DEADZONE, FULL_SCALE] to [MIN_DB, MAX_DB]
                # If error > FULL_SCALE, clamp to MAX_DB
                excess_err = min(abs_err, GUIDANCE_FULL_SCALE_DEG) - GUIDANCE_DEADZONE_DEG
                range_deg = GUIDANCE_FULL_SCALE_DEG - GUIDANCE_DEADZONE_DEG
                # Use cubic curve for steeper fade-in
                norm_linear = max(0.0, excess_err / range_deg) # 0.0 to 1.0
                norm_amp = norm_linear ** 3
                
                # dB conversion
                db = GUIDANCE_MIN_DBFS + (GUIDANCE_MAX_DBFS - GUIDANCE_MIN_DBFS) * norm_amp
                amp = float(10.0 ** (db / 20.0))
                
                # Synthesize Tone with odd harmonics
                inc = GUIDANCE_FREQ_HZ / self.samplerate
                t = (np.arange(frames) * inc + self._phase_guidance) % 1.0
                
                # Fundamental + 3rd (1/3 amp) + 5th (1/5 amp) harmonics
                tone = (1.0 * np.sin(2.0 * np.pi * t) + 
                        0.33 * np.sin(2.0 * np.pi * 3.0 * t) + 
                        0.2 * np.sin(2.0 * np.pi * 5.0 * t))
                
                # Normalize peak roughly to 1.0 to preserve amp
                tone *= 0.7 
                
                tone *= amp
                
                # Pan
                # Error positive -> Right (pan 1.0)
                # Error negative -> Left (pan -1.0)
                # Map error to pan: 0 at 0 error, +/- 1.0 at +/- FULL_SCALE
                pan_val = self._clamp(self._sm_guidance_error / GUIDANCE_FULL_SCALE_DEG, -1.0, 1.0)
                Lg, Rg = self._pan_gains(pan_val)
                
                bufL += (tone * Lg).astype(np.float32)
                bufR += (tone * Rg).astype(np.float32)
                
                if frames > 0: self._phase_guidance = (t[-1] + inc) % 1.0
            else:
                # Keep phase running or reset? Continuous phase is better usually, 
                # but if silence is long, it doesn't matter.
                # Just reset smoother if we want instant response? No, keep smoother.
                pass
        else:
             self._sm_guidance_error = 0.0 # Reset when off

        # Hydraulic steering misalignment tone
        # Active when: driver not steering (|input| < DEAD) AND wheels off-centre (|actual| > DEAD).
        # actual_steering is 0.0 for all non-hydraulic vehicles, so this never fires on normal cars.
        # Plays continuously while steering is off-centre; HRTF azimuth is proportional to offset
        # so the tone smoothly moves from the ear towards centre as the vehicle straightens.
        abs_hydro = abs(hydro_actual)
        if abs_hydro > HYDRO_STEER_DEADZONE:
            norm = self._clamp(
                (abs_hydro - HYDRO_STEER_DEADZONE) / (HYDRO_STEER_FULL - HYDRO_STEER_DEADZONE)
            )
            amp = float(10.0 ** (HYDRO_STEER_AMP_DB / 20.0)) * norm
            n = np.arange(frames, dtype=np.float64)
            phase_arr = self._hydro_steer_phase + n * (HYDRO_STEER_TONE_HZ / self.samplerate)
            mono_hs = (amp * np.sin(2.0 * np.pi * phase_arr)).astype(np.float32)
            if frames > 0:
                self._hydro_steer_phase = (phase_arr[-1] + HYDRO_STEER_TONE_HZ / self.samplerate) % 1.0
            # HRTF azimuth proportional to steering: 90° = full left, 270° = full right, 0° = centre
            hrtf_az_hs = (90.0 * hydro_actual) % 360.0
            if self._hrtf is not None and self._hrtf_user_enabled:
                ir_l, ir_r = self._hrtf.get_hrir(hrtf_az_hs)
                if ir_l is not None:
                    conv_l = np.convolve(mono_hs, ir_l, mode='full')
                    conv_r = np.convolve(mono_hs, ir_r, mode='full')
                    if self._hydro_steer_overlap_L is not None:
                        ol = min(len(self._hydro_steer_overlap_L), len(conv_l))
                        conv_l[:ol] += self._hydro_steer_overlap_L[:ol]
                        conv_r[:ol] += self._hydro_steer_overlap_R[:ol]
                    bufL += conv_l[:frames].astype(np.float32)
                    bufR += conv_r[:frames].astype(np.float32)
                    self._hydro_steer_overlap_L = conv_l[frames:].copy()
                    self._hydro_steer_overlap_R = conv_r[frames:].copy()
                else:
                    # HRTF unavailable: proportional stereo pan
                    pan = self._clamp(hydro_actual)  # -1 = right, +1 = left
                    bufL += mono_hs * max(0.0, pan)
                    bufR += mono_hs * max(0.0, -pan)
                    self._hydro_steer_overlap_L = None
                    self._hydro_steer_overlap_R = None
            else:
                pan = self._clamp(hydro_actual)
                bufL += mono_hs * max(0.0, pan)
                bufR += mono_hs * max(0.0, -pan)
                self._hydro_steer_overlap_L = None
                self._hydro_steer_overlap_R = None
        else:
            # Not active: drain any remaining HRTF tail
            if self._hydro_steer_overlap_L is not None and len(self._hydro_steer_overlap_L) > 0:
                ol = min(len(self._hydro_steer_overlap_L), frames)
                bufL[:ol] += self._hydro_steer_overlap_L[:ol].astype(np.float32)
                bufR[:ol] += self._hydro_steer_overlap_R[:ol].astype(np.float32)
            self._hydro_steer_overlap_L = None
            self._hydro_steer_overlap_R = None

        # Coordinate Guidance FM tone
        if coord_guide_active:
            dt_cg = frames / self.samplerate
            beta_cg = 1.0 - math.exp(-dt_cg / 0.15)  # 150 ms smoothing
            self._sm_coord_error += beta_cg * (coord_guide_error - self._sm_coord_error)

            abs_err = abs(self._sm_coord_error)

            if abs_err > COORD_GUIDE_DEADZONE_DEG:
                # error_norm: 0 = just outside deadzone, 1 = 180° off course
                error_norm = self._clamp(
                    (abs_err - COORD_GUIDE_DEADZONE_DEG) / (180.0 - COORD_GUIDE_DEADZONE_DEG)
                )

                amp = float(10.0 ** (COORD_GUIDE_AMP_DB / 20.0)) * error_norm

                fc = COORD_GUIDE_FC_OFFCOURSE_HZ + (
                    COORD_GUIDE_FC_ONCOURSE_HZ - COORD_GUIDE_FC_OFFCOURSE_HZ
                ) * (1.0 - error_norm)

                fm_ratio = COORD_GUIDE_FM_RATIO_MIN + (
                    COORD_GUIDE_FM_RATIO_MAX - COORD_GUIDE_FM_RATIO_MIN
                ) * error_norm
                fm = fc * fm_ratio

                mod_index = COORD_GUIDE_MOD_INDEX_MAX * error_norm

                n = np.arange(frames, dtype=np.float64)
                mod_phase_arr = self._coord_phase_mod + n * (fm / self.samplerate)
                mod_sig = mod_index * np.sin(2.0 * np.pi * mod_phase_arr)
                car_phase_arr = self._coord_phase_carrier + n * (fc / self.samplerate)
                mono_tone = (amp * np.sin(2.0 * np.pi * car_phase_arr + mod_sig)).astype(np.float32)

                if frames > 0:
                    self._coord_phase_carrier = (car_phase_arr[-1] + fc / self.samplerate) % 1.0
                    self._coord_phase_mod = (mod_phase_arr[-1] + fm / self.samplerate) % 1.0

                # Spatialize: bearing error maps to HRTF azimuth.
                # 0° error = target ahead (az 0°), positive error = target left (az 90°),
                # negative error = target right (az 270°), ±180° = target behind (az 180°).
                hrtf_az = self._sm_coord_error % 360.0
                if self._hrtf is not None and self._hrtf_user_enabled:
                    ir_l, ir_r = self._hrtf.get_hrir(hrtf_az)
                    if ir_l is not None:
                        conv_l = np.convolve(mono_tone, ir_l, mode='full')
                        conv_r = np.convolve(mono_tone, ir_r, mode='full')
                        if self._coord_hrtf_overlap_L is not None:
                            ol = min(len(self._coord_hrtf_overlap_L), len(conv_l))
                            conv_l[:ol] += self._coord_hrtf_overlap_L[:ol]
                            conv_r[:ol] += self._coord_hrtf_overlap_R[:ol]
                        bufL += conv_l[:frames].astype(np.float32)
                        bufR += conv_r[:frames].astype(np.float32)
                        self._coord_hrtf_overlap_L = conv_l[frames:].copy()
                        self._coord_hrtf_overlap_R = conv_r[frames:].copy()
                    else:
                        pan_val = self._clamp(-self._sm_coord_error / 180.0, -1.0, 1.0)
                        Lg, Rg = self._pan_gains(pan_val)
                        bufL += mono_tone * Lg
                        bufR += mono_tone * Rg
                        self._coord_hrtf_overlap_L = None
                        self._coord_hrtf_overlap_R = None
                else:
                    pan_val = self._clamp(-self._sm_coord_error / 180.0, -1.0, 1.0)
                    Lg, Rg = self._pan_gains(pan_val)
                    bufL += mono_tone * Lg
                    bufR += mono_tone * Rg
                    self._coord_hrtf_overlap_L = None
                    self._coord_hrtf_overlap_R = None
            else:
                # In deadzone: drain HRTF tail
                if self._coord_hrtf_overlap_L is not None and len(self._coord_hrtf_overlap_L) > 0:
                    ol = min(len(self._coord_hrtf_overlap_L), frames)
                    bufL[:ol] += self._coord_hrtf_overlap_L[:ol].astype(np.float32)
                    bufR[:ol] += self._coord_hrtf_overlap_R[:ol].astype(np.float32)
                    self._coord_hrtf_overlap_L = None
                    self._coord_hrtf_overlap_R = None
        else:
            self._sm_coord_error = 0.0
            if self._coord_hrtf_overlap_L is not None and len(self._coord_hrtf_overlap_L) > 0:
                ol = min(len(self._coord_hrtf_overlap_L), frames)
                bufL[:ol] += self._coord_hrtf_overlap_L[:ol].astype(np.float32)
                bufR[:ol] += self._coord_hrtf_overlap_R[:ol].astype(np.float32)
            self._coord_hrtf_overlap_L = None
            self._coord_hrtf_overlap_R = None

        out = np.stack([bufL, bufR], axis=1).astype(np.float32)
        np.clip(out, -0.999, 0.999, out=out)
        outdata[:] = out

    # --- Stream Management ---
    
    def _find_hostapi_index(self, substr):
        try:
            apis = sd.query_hostapis()
            target = str(substr or "").lower()
            for idx, info in enumerate(apis):
                if target in str(info.get("name", "")).lower(): return idx
        except Exception as e: self.logger.warning(f"query_hostapis failed: {e}")
        return None

    def _find_target_device(self, verbose=False):
        try:
            wasapi_idx = self._find_hostapi_index("wasapi")

            if not self._follow_default_enabled and self._preferred_device_name:
                # Fixed-device mode: open the named WASAPI device directly.
                if wasapi_idx is None:
                    self.logger.warning("WASAPI not available; falling back to system default.")
                    return sd.default.device[1]
                all_devices = sd.query_devices()
                for idx, device in enumerate(all_devices):
                    if (device['hostapi'] == wasapi_idx and
                            device['max_output_channels'] > 0 and
                            self._preferred_device_name in device.get("name", "")):
                        if verbose:
                            self.logger.info(
                                f"Using fixed WASAPI device: '{device['name']}' (Index: {idx})")
                        return idx
                self.logger.warning(
                    f"Fixed device '{self._preferred_device_name}' not found; "
                    f"falling back to system default.")
                return sd.default.device[1]

            # Follow-default mode: find the WASAPI device matching the OS default.
            system_default_idx = sd.default.device[1]
            if system_default_idx == -1:
                self.logger.warning("No default output device found in the system.")
                return None

            default_info = sd.query_devices(system_default_idx)
            default_name = default_info.get("name", "")
            if verbose:
                self.logger.info(
                    f"System default output device is: '{default_name}' (Index: {system_default_idx})")

            if wasapi_idx is None:
                self.logger.warning("WASAPI not found; using system default device.")
                return system_default_idx

            all_devices = sd.query_devices()
            for idx, device in enumerate(all_devices):
                if (device['hostapi'] == wasapi_idx and
                        device['max_output_channels'] > 0 and
                        default_name in device.get("name", "")):
                    if verbose:
                        self.logger.info(
                            f"Found matching WASAPI device: '{device['name']}' (Index: {idx})")
                    return idx

            self.logger.warning(
                f"No WASAPI equivalent for '{default_name}'; falling back to system default.")
            return system_default_idx

        except Exception as e:
            self.logger.error(f"Error while searching for audio device: {e}. Falling back to system default.")
            try:
                return sd.default.device[1]
            except Exception:
                return None

    def _device_name(self, idx):
        try: return sd.query_devices(idx)["name"]
        except Exception: return "Unknown Device"

    def _restart_audio_stream(self, new_device_index):
        # Tearing a stream down must happen OUTSIDE self.lock. PortAudio's
        # Pa_StopStream (what Stream.stop() calls) blocks until the stream
        # callback has returned, and _audio_callback takes self.lock. Holding the
        # lock across stop()/close() therefore deadlocks the two against each
        # other: this thread waits for the callback to finish, the callback waits
        # for the lock it can never get. That hangs the device watcher and the
        # PortAudio callback thread for good, so audio never comes back after an
        # output device change. Detach the stream under the lock, tear it down
        # after releasing it.
        with self.lock:
            if self._current_device_index == new_device_index and self._audio_stream:
                return
            old_stream = self._audio_stream
            self._audio_stream = None

        if old_stream is not None:
            try:
                old_stream.stop()
                old_stream.close()
            except Exception as e:
                self.logger.warning(f"Error closing old audio stream: {e}")

        try:
            device_info = sd.query_devices(new_device_index)
            new_samplerate = device_info.get("default_samplerate", DEFAULT_SR)

            # No stream is running here, so nothing can be reading these.
            with self.lock:
                if self.samplerate != new_samplerate:
                    self.samplerate = new_samplerate
                    self._regenerate_waveforms()
                    self._phase_inc_shift = self.shift_freq / self.samplerate

            stream = sd.OutputStream(
                samplerate=self.samplerate,
                channels=2,
                dtype="float32",
                device=new_device_index,
                callback=self._audio_callback
            )
            # start() also outside the lock: it makes the callback live, and the
            # first invocation wants self.lock immediately.
            stream.start()
            with self.lock:
                self._audio_stream = stream
                self._current_device_index = new_device_index
                self._current_device_name = device_info.get("name", "Unknown")
            self.logger.info(f"Audio stream started on device: '{self._current_device_name}' at {int(self.samplerate)} Hz.")
        except Exception as e:
            self.logger.error(f"Failed to start audio stream on device index {new_device_index}: {e}")
            with self.lock:
                self._audio_stream, self._current_device_index = None, None

    def _device_watcher_loop(self):
        while not self._stop_event.is_set():
            self._stop_event.wait(self._audio_poll_interval)
            if self._stop_event.is_set(): break
            
            target_idx = self._find_target_device(verbose=False)
            
            if target_idx is not None and target_idx != self._current_device_index:
                new_name = self._device_name(target_idx)
                self.logger.info(f"Default audio device appears to have changed to: '{new_name}'. Restarting stream.")
                self._find_target_device(verbose=True)
                self._restart_audio_stream(target_idx)

    def start(self):
        if not self._is_enabled: return
        self._stop_event.clear()
        
        initial_device_index = self._find_target_device(verbose=True)

        if initial_device_index is None:
            self.logger.error("No suitable output audio device found. Audio will be disabled.")
            self._is_enabled = False
            return
        
        self._regenerate_waveforms()
        self._restart_audio_stream(initial_device_index)

        if self._follow_default_enabled:
            self._device_watcher_thread = threading.Thread(target=self._device_watcher_loop, daemon=True)
            self._device_watcher_thread.start()
            self.logger.info("Following OS default output device (WASAPI).")

    def stop(self):
        if not self._is_enabled: return
        self._stop_event.set()
        if self._device_watcher_thread:
            self._device_watcher_thread.join(timeout=self._audio_poll_interval)
        with self.lock:
            stream = self._audio_stream
            self._audio_stream = None
        # Outside the lock, for the same reason as _restart_audio_stream: stop()
        # waits on the callback and the callback waits on self.lock.
        if stream is not None:
            try:
                stream.stop()
                stream.close()
            except Exception: pass
        if self._scanner_diag is not None:
            self._scanner_diag.stop()
        self.logger.info("Audio system stopped.")
