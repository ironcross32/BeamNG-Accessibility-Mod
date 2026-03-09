# --- START OF beamtel.py ---

# Nuitka stuff
# nuitka-project: --output-dir=build
# nuitka-project: --remove-output
# nuitka-project: --include-package=sral
# Copy SRAL.dll (x64) next to your EXE or add:
# nuitka-project: --include-data-file=SRAL.dll=SRAL.dll
# nuitka-project: --include-data-file=nvdaControllerClient.dll=nvdaControllerClient.dll
# --include-package=websockets
#  --include-package=websockets.asyncio
#  --include-package=websockets.legacy
#  --include-module=websockets.server

# nuitka-project: --include-module=nvda_ws_speaker
# nuitka-project: --include-module=bnh_logger
# nuitka-project: --include-module=audio
# nuitka-project: --include-module=hrtf
# nuitka-project: --include-package=h5py
# nuitka-project: --include-package-data=h5py
# nuitka-project: --include-data-file=mit_kemar_normal_pinna.sofa=mit_kemar_normal_pinna.sofa
# nuitka-project: --onefile
# nuitka-project: --assume-yes-for-downloads
# nuitka-project: --windows-console-mode=force
# nuitka-project: --windows-uac-admin

import math
import socket
import struct
import time
import threading
from nvda_ws_speaker import start_server_in_thread, register_details_callbacks, broadcast
import signal
import os
import json
import sys
import ctypes
import ctypes.wintypes
from collections import deque

from bnh_logger import get_logger
from audio import AudioController

logger = get_logger()

FROZEN = getattr(sys, "frozen", False)
BASE_DIR = (
    os.path.dirname(os.path.abspath(sys.executable))
    if FROZEN
    else os.path.dirname(os.path.abspath(__file__))
)
HERE = BASE_DIR
# --- Use %localappdata% for configuration, with fallback to home directory ---
def _get_config_dir():
    """Gets the configuration directory, falling back if LOCALAPPDATA is not set."""
    base_path = os.getenv("LOCALAPPDATA")
    if base_path is None:
        # Fallback to user's home directory
        base_path = os.path.expanduser("~")
    return os.path.join(base_path, "beamtel")

CONFIG_DIR = _get_config_dir()
os.makedirs(CONFIG_DIR, exist_ok=True)
CONFIG_PATH = os.path.join(CONFIG_DIR, "beamtel_config.json")


STOP = threading.Event()


def _handle_sigint(signum, frame):
    STOP.set()


signal.signal(signal.SIGINT, _handle_sigint)
if hasattr(signal, "SIGBREAK"):
    signal.signal(signal.SIGBREAK, _handle_sigint)

# =========================
#  Defaults & Config
# =========================
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
    "compass_highlight_nth_click": 6, # MODIFIED
    "hrtf_enabled": True,
    "hrtf_front_emphasis_db": -6.0,
    "hrtf_distance_gain_db": 0.0,
    "follow_default_audio_device": True,
    "preferred_hostapi": "wasapi",
    "audio_poll_interval_sec": 2.0,
    "obstacle_detection_enabled": False,
    "obstacle_buzz_volume_db": -12.0,
    "obstacle_max_range_m": 20,
    "obstacle_warning_range_m": 15,
    "launch_beamng": False,
}

# =========================
#  Speech & Buffer
# =========================
try:
    import sral
    SRAL_OK = True
except Exception as e:
    SRAL_OK = False
    logger.warning("SRAL Python binding not found. Speech will fall back to other engines.")

SR_INSTANCE = None
SPEECH_BUFFER = deque(maxlen=100)

def sral_init():
    global SR_INSTANCE
    if SR_INSTANCE is not None:
        return SR_INSTANCE
    if not SRAL_OK:
        SR_INSTANCE = None
        return None
    try:
        try:
            os.chdir(HERE)
        except Exception:
            pass
        SR_INSTANCE = sral.Sral(32)
    except Exception as e:
        logger.warning(f"Failed to initialize SRAL: {e}")
        SR_INSTANCE = None
    return SR_INSTANCE


def say(text: str, interrupt: bool = True, exclude_from_buffer: bool = False):
    t = (text or "").strip()
    if not t:
        return
    
    if _command_context:
        logger.info(f"Speech output: '{t}'")
    
    if not exclude_from_buffer:
        SPEECH_BUFFER.append(t)

    inst = sral_init()
    if inst:
        try:
            inst.speak(t, bool(interrupt))
            return
        except Exception as e:
            logger.warning(f"SRAL speak failed: {e}")


def stop_speech():
    inst = sral_init()
    if inst:
        try:
            inst.stop()
        except Exception:
            pass


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
        merged["units"] = "metric" if str(merged.get("units", "imperial")).lower().startswith("m") else "imperial"
        return merged
    except Exception:
        try:
            os.replace(CONFIG_PATH, CONFIG_PATH + ".bak")
        except Exception:
            pass
        _write_config(CONFIG_PATH, DEFAULT_CONFIG)
        return DEFAULT_CONFIG.copy()


# =========================
#  OutGauge / MotionSim / Telemetry
# =========================
OG_FORMAT = "<I4sHBBfffffffIIfff16s16si"
OG_SIZE = struct.calcsize(OG_FORMAT)

EXT_FORMAT = "<H4sBx9fII23f"
EXT_SIZE = struct.calcsize(EXT_FORMAT)

MS_MAGIC = b"BNG1"
MS_FORMAT = "<4s21f"
MS_SIZE = struct.calcsize(MS_FORMAT)

# Dashboard Light bitmasks
DL_SHIFT     = 1 << 0
DL_FULLBEAM  = 1 << 1
DL_HANDBRAKE = 1 << 2
DL_TC        = 1 << 4
DL_SIGNAL_L  = 1 << 5
DL_SIGNAL_R  = 1 << 6
DL_CHECK     = 1 << 7
DL_OILWARN   = 1 << 8
DL_BATTERY   = 1 << 9
DL_ABS       = 1 << 10
DL_LOWBEAM   = 1 << 11


UI_PORT = 4579
SCANNER_LISTEN_PORT = 4445


def ui_listener(stop_event):
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        sock.bind(("127.0.0.1", UI_PORT))
        sock.settimeout(0.2)
        while not stop_event.is_set():
            try:
                data, _ = sock.recvfrom(8192)
            except socket.timeout:
                continue
            except OSError:
                break

            line = data.decode("utf-8", errors="ignore")
            if not line.startswith("BEAMTEL_UI "):
                continue
            try:
                payload = json.loads(line[len("BEAMTEL_UI ") :])
            except Exception:
                continue
            kind = payload.get("kind")
            d = payload.get("data") or {}
            if kind == "toastr":
                title = (d.get("title") or "").strip()
                msg = (d.get("msg") or "").strip()
                text = f"{title} {msg}".strip() or msg or title
            elif kind == "message":
                text = (d.get("msg") or "").strip()
            if text:
                say(text)
    finally:
        try:
            sock.close()
        except Exception:
            pass

def scanner_listener(audio_controller, stop_event):
    """Listens for UDP packets from vehicleScanner.lua and passes data to the audio controller."""
    global last_scanner_target_name, last_scanner_distance, last_scanner_approach_deg
    SCANNER_PACKET_FORMAT = '<ff'
    SCANNER_PACKET_SIZE = struct.calcsize(SCANNER_PACKET_FORMAT)
    
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        sock.bind(("127.0.0.1", SCANNER_LISTEN_PORT))
        sock.settimeout(0.2)
        logger.info(f"Vehicle scanner listener started on port {SCANNER_LISTEN_PORT}")
        
        first_packet = True
        while not stop_event.is_set():
            try:
                data, addr = sock.recvfrom(1024)
                if first_packet:
                    logger.info(f"First UDP packet received from vehicle scanner (source: {addr})")
                    first_packet = False
                try:
                    text = data.decode('ascii').strip()
                    # Alignment responses
                    if text == "ALIGN_OK":
                        say("Aligned to trailer. Reverse straight back and press L to couple.", exclude_from_buffer=True)
                    elif text.startswith("ALIGN_FAIL:"):
                        reason = text[len("ALIGN_FAIL:"):]
                        say(f"Alignment failed. {reason}", exclude_from_buffer=True)
                    # AI control responses
                    elif text.startswith("AI_OK:"):
                        msg = text[len("AI_OK:"):]
                        logger.info(f"AI response: {msg}")
                    elif text.startswith("AI_ERR:"):
                        msg = text[len("AI_ERR:"):]
                        say(f"AI error. {msg}", exclude_from_buffer=True)
                    elif text.startswith("TARGET_NAME:"):
                        name = text[len("TARGET_NAME:"):]
                        with state_lock:
                            last_scanner_target_name = name
                        say(name, exclude_from_buffer=True)
                    elif text.startswith("SWITCHED:"):
                        name = text[len("SWITCHED:"):]
                        with state_lock:
                            last_scanner_target_name = ""
                            last_scanner_distance = float('inf')
                        say(name, exclude_from_buffer=True)
                    elif text.startswith("AI_STATUS:"):
                        parts = text[len("AI_STATUS:"):].split(',')
                        if len(parts) >= 5:
                            mode, aggr, speed, avoid, lane = parts[0], parts[1], parts[2], parts[3], parts[4]
                            speed_str = "no limit" if speed == "off" else f"{speed} km/h"
                            say(f"AI mode {mode}, aggression {aggr}, speed {speed_str}, avoid {avoid}, lane {lane}", exclude_from_buffer=True)
                    else:
                        # Text protocol: "bearing,distance,approachDeg"
                        parts = text.split(',')
                        if len(parts) >= 2:
                            bearing  = float(parts[0])
                            distance = float(parts[1])
                            approach = float(parts[2]) if len(parts) >= 3 else 0.0
                            with state_lock:
                                last_scanner_distance     = distance
                                last_scanner_approach_deg = approach
                            audio_controller.update_scanner_target(bearing, distance)
                except (ValueError, UnicodeDecodeError):
                    # Fallback: old binary '<ff' format
                    if len(data) == SCANNER_PACKET_SIZE:
                        bearing, distance = struct.unpack(SCANNER_PACKET_FORMAT, data)
                        audio_controller.update_scanner_target(bearing, distance)
            except socket.timeout:
                continue
            except OSError:
                if stop_event.is_set():
                    break
                logger.error("Scanner listener socket error.")
    finally:
        try:
            sock.close()
        except Exception:
            pass
        logger.info("Vehicle scanner listener stopped.")


def obstacle_listener(audio_controller, stop_event):
    """Listens for UDP packets from obstacleDetector.lua and passes data to the audio controller."""
    # Text CSV format: "type,bearing,urgency,distance" or "0" for all clear

    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        sock.bind(("127.0.0.1", OBSTACLE_LISTEN_PORT))
        sock.settimeout(0.2)
        logger.info(f"Obstacle detector listener started on port {OBSTACLE_LISTEN_PORT}")

        first_packet = True
        terrain_announced = {2: False, 3: False}

        while not stop_event.is_set():
            try:
                data, addr = sock.recvfrom(1024)
                if first_packet:
                    logger.info(f"First UDP packet received from obstacle detector (source: {addr})")
                    first_packet = False

                text = data.decode("utf-8", errors="ignore").strip()
                if not text:
                    continue

                if text == "0":
                    audio_controller.clear_obstacles()
                    terrain_announced[2] = False
                    terrain_announced[3] = False
                    continue

                parts = text.split(",")
                if len(parts) == 4:
                    pkt_type = int(parts[0])
                    bearing = float(parts[1])
                    urgency = int(parts[2])
                    distance = float(parts[3])

                    audio_controller.update_obstacle(pkt_type, bearing, urgency, distance)

                    if pkt_type == 2 and not terrain_announced[2]:
                        say("Drop-off ahead", exclude_from_buffer=True)
                        terrain_announced[2] = True
                    elif pkt_type == 3 and not terrain_announced[3]:
                        say("Steep hill ahead", exclude_from_buffer=True)
                        terrain_announced[3] = True

            except socket.timeout:
                continue
            except OSError:
                if stop_event.is_set():
                    break
                logger.error("Obstacle listener socket error.")
    finally:
        try:
            sock.close()
        except Exception:
            pass
        logger.info("Obstacle detector listener stopped.")


def camera_listener(audio_controller, stop_event):
    """Listens for UDP packets from cameraInfo.lua and drives camera compass clicks."""
    global cam_yaw_deg, cam_pitch_deg, cam_agl, cam_veh_bearing, cam_veh_distance, cam_is_free
    global cam_last_click_heading_deg, cam_compass_click_counter, cam_last_announced_compass_idx, cam_last_compass_ts

    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        sock.bind(("127.0.0.1", CAMERA_LISTEN_PORT))
        sock.settimeout(0.2)
        logger.info(f"Camera info listener started on port {CAMERA_LISTEN_PORT}")

        cfg = load_config()
        compass_highlight_enabled = cfg.get("compass_highlight_enabled", False)
        compass_highlight_nth_click = int(cfg.get("compass_highlight_nth_click", 6))
        click_interval_deg = float(cfg.get("compass_click_interval", 15.0))

        first_packet = True
        while not stop_event.is_set():
            try:
                data, addr = sock.recvfrom(1024)
                if first_packet:
                    logger.info(f"First UDP packet received from camera info (source: {addr})")
                    first_packet = False
                if not free_cam_active:
                    continue
                try:
                    text = data.decode('ascii').strip()
                    parts = text.split(',')
                    if len(parts) == 6:
                        yaw = float(parts[0])
                        pitch = float(parts[1])
                        agl = float(parts[2])
                        bearing = float(parts[3])
                        distance = float(parts[4])
                        is_free = int(parts[5]) == 1

                        now = time.time()
                        with state_lock:
                            cam_yaw_deg = yaw
                            cam_pitch_deg = pitch
                            cam_agl = agl
                            cam_veh_bearing = bearing
                            cam_veh_distance = distance
                            cam_is_free = is_free

                            # Camera compass click logic (same as vehicle compass)
                            heading = yaw
                            delta_heading = heading - cam_last_click_heading_deg
                            if delta_heading > 180.0: delta_heading -= 360.0
                            if delta_heading < -180.0: delta_heading += 360.0

                            if abs(delta_heading) >= click_interval_deg:
                                cam_compass_click_counter += 1
                                pitch_mult = 1.0 + 0.25 * math.cos(math.radians(heading))

                                if compass_highlight_enabled and cam_compass_click_counter >= compass_highlight_nth_click:
                                    audio_controller.trigger_cam_compass_highlight(heading, pitch_mult * 1.5)
                                    cam_compass_click_counter = 0
                                else:
                                    audio_controller.trigger_cam_compass_click(heading, pitch_mult)

                                num_intervals = round(heading / click_interval_deg)
                                cam_last_click_heading_deg = (num_intervals * click_interval_deg) % 360.0

                            # Camera compass direction announcements
                            targets = [i * 45.0 for i in range(8)]
                            half_width = 3.0
                            current_compass_idx = -1

                            for i, target_heading in enumerate(targets):
                                lower_bound = (target_heading - half_width + 360) % 360
                                upper_bound = (target_heading + half_width) % 360

                                is_inside = False
                                if lower_bound < upper_bound:
                                    if lower_bound <= heading < upper_bound:
                                        is_inside = True
                                else:
                                    if heading >= lower_bound or heading < upper_bound:
                                        is_inside = True

                                if is_inside:
                                    current_compass_idx = i
                                    break

                            if current_compass_idx != cam_last_announced_compass_idx:
                                if current_compass_idx != -1:
                                    if (now - cam_last_compass_ts) >= compass_min_interval:
                                        say(f"Camera {COMPASS_NAMES[current_compass_idx]}", exclude_from_buffer=True)
                                        cam_last_compass_ts = now
                                cam_last_announced_compass_idx = current_compass_idx
                except (ValueError, UnicodeDecodeError):
                    pass
            except socket.timeout:
                continue
            except OSError:
                if stop_event.is_set():
                    break
                logger.error("Camera listener socket error.")
    finally:
        try:
            sock.close()
        except Exception:
            pass
        logger.info("Camera info listener stopped.")


# =========================
#  Units & formatting
# =========================
UNITS_MODE = "imperial"
oil_chime_enabled = True
MPH_PER_MS = 2.2369362920544
KMH_PER_MS = 3.6
PSI_PER_BAR = 14.503773773


def fmt_speed(speed_ms: float):
    if UNITS_MODE == "metric":
        return int(round(speed_ms * KMH_PER_MS)), "km/h"
    else:
        return int(round(speed_ms * MPH_PER_MS)), "mph"


def fmt_pressure(psi_val: float):
    if UNITS_MODE == "metric":
        return round(float(psi_val) / PSI_PER_BAR, 2), "bar"
    else:
        return round(float(psi_val), 1), "psi"


def fmt_turbo(bar_val: float):
    if UNITS_MODE == "metric":
        return round(float(bar_val), 2), "bar"
    else:
        return round(float(bar_val) * PSI_PER_BAR, 1), "psi"


def fmt_temp_c_or_f(celsius: float):
    if UNITS_MODE == "metric":
        return int(round(celsius)), "Celsius"
    else:
        f = (float(celsius) * 9.0 / 5.0) + 32.0
        return int(round(f)), "Fahrenheit"


def fmt_heading():
    return f"{last_heading:.1f} degrees"


def flip_units():
    global UNITS_MODE
    if UNITS_MODE == "imperial":
        UNITS_MODE = "metric"
    else:
        UNITS_MODE = "imperial"


# =========================
#  Gear helpers
# =========================
_ORDINAL_WORDS = {
    1: "first", 2: "second", 3: "third", 4: "fourth", 5: "fifth", 6: "sixth",
    7: "seventh", 8: "eighth", 9: "ninth", 10: "tenth", 11: "eleventh", 12: "twelfth",
}


def gear_to_phrase(gear_byte: int) -> str:
    if gear_byte == 0: return "reverse"
    if gear_byte == 1: return "neutral"
    if gear_byte >= 2:
        n = gear_byte - 1
        return _ORDINAL_WORDS.get(n, f"{n}th")
    return "unknown"


def extended_gear_to_phrase(gear_str: str) -> str:
    s = (gear_str or "").strip().upper()
    if not s:
        return "unknown"

    if s == 'P': return "park"
    if s == 'D': return "drive"
    if s == 'R': return "reverse"
    if s == 'N': return "neutral"

    if len(s) > 1 and s[1:].isdigit():
        num_part = s[1:]
        if s.startswith('S'):
            return f"sport {num_part}"
        if s.startswith('M'):
            return f"manual {num_part}"

    if s.isdigit() or s.startswith("-"):
        try:
            num = int(s)
            if num == -1:
                return "reverse"
            elif num == 0:
                return "neutral"
            else:
                return _ORDINAL_WORDS.get(num, f"{num}th")
        except ValueError:
            return "unknown"

    return "unknown"


def get_speed_bucket(speed_ms: float) -> int:
    if speed_ms < 0: return 0
    if UNITS_MODE == "metric":
        kph = speed_ms * KMH_PER_MS
        return int(kph // 25)
    else:
        mph = speed_ms * MPH_PER_MS
        return int(mph // 25)


# =========================
#  Bearing helpers (MotionSim yaw -> 8-way compass)
# =========================
COMPASS_NAMES = [
    "north", "northeast", "east", "southeast",
    "south", "southwest", "west", "northwest",
]


def yaw_to_heading_deg(yaw_rad: float) -> float:
    deg = math.degrees(float(yaw_rad))
    heading = deg % 360.0
    return heading


# =========================
#  Shared state
# =========================
state_lock = threading.Lock()
# Basic state
last_speed_ms = 0.0
last_rpm = 0.0
last_fuel = 0.0
last_turbo = 0.0
last_engtemp = 0.0
last_oiltemp = 0.0
last_throttle = 0.0
last_brake = 0.0
last_clutch = 0.0
last_steering = 0.0
last_actual_steering = 0.0
last_steering_input = 0.0
last_rpm_max = 0.0
last_turbo_max = 0.0
last_heading = 0.0
last_oil_pressure = 0.0
protocol_mode = "outgauge"

# Pneumatics
last_air_pressure = 0.0
last_air_pressure_max = 0.0

# Expanded Telemetry
last_clutch_temp = 0.0
last_g_lat = 0.0
last_g_lon = 0.0
last_tire_pressure_fl, last_tire_pressure_fr, last_tire_pressure_rl, last_tire_pressure_rr = 0.0, 0.0, 0.0, 0.0
last_tire_temp_fl, last_tire_temp_fr, last_tire_temp_rl, last_tire_temp_rr = 0.0, 0.0, 0.0, 0.0
last_brake_temp_fl, last_brake_temp_fr, last_brake_temp_rl, last_brake_temp_rr = 0.0, 0.0, 0.0, 0.0

# Status Mode
status_mode_active = False
current_status_metric_index = 0
status_arrow_hooks = []

# Buffer Mode
buffer_mode_active = False
current_buffer_index = -1
buffer_key_hooks = []

# Compass click state
last_click_heading_deg = 0.0
compass_click_counter = 0

# MotionSim state
last_pos_x, last_pos_y, last_pos_z, last_yaw_rad = 0.0, 0.0, 0.0, 0.0
last_compass_ts = 0.0
last_announced_compass_idx = -1
compass_min_interval = 0.1

# --- MotionSim attitude state ---
last_roll_rad, last_pitch_rad, last_up_z = 0.0, 0.0, 1.0
inverted, inverted_announced = False, False

last_bucket, last_speed_announce_ts, cooldown_sec = None, 0.0, 1.0

NEUTRAL = 1
last_gear_byte, last_gear_str = None, None
neutral_pending, neutral_spoken, neutral_start_ts, neutral_dwell_sec = False, False, 0.0, 0.30

# State shared with audio module
pedal_tones_active = False
scan_mode_active = False
obstacle_mode_active = False
_command_context = False   # True only while processing an F9/F10 command keystroke

# NEW: Heading Guidance State
heading_guidance_active = False
heading_guidance_target = 0.0

# NEW: Drift Detection State
drift_mode_active = False
last_drift_check_ts = 0.0
drift_baseline_heading = 0.0
drift_alert_active = False
drift_pan_direction = 0.0 # -1.0 Left, 1.0 Right

# NEW: Low Speed Detection State
low_speed_mode_active = False

# Coordinate Guidance State
coord_guidance_active = False
marked_coord_x = None   # None means no waypoint has been set yet
marked_coord_y = None
coord_target_bearing = 0.0    # cached bearing to waypoint, recalculated every ~1 s
_last_coord_bearing_ts = 0.0  # timestamp of last bearing recalculation
_ls_prev_speed_ms = 0.0
_ls_prev_speed_ts = 0.0
_ls_decel_smooth = 0.0
_ls_steady_ref_mph = 0.0
_ls_steady_start_ts = 0.0

# Scanner live data (updated by scanner_listener)
last_scanner_target_name = ""
last_scanner_distance    = float('inf')
last_scanner_approach_deg = 0.0

# Camera Info State
free_cam_active = False
cam_yaw_deg = 0.0
cam_pitch_deg = 0.0
cam_agl = 0.0
cam_veh_bearing = 0.0
cam_veh_distance = -1.0
cam_is_free = False
cam_last_click_heading_deg = 0.0
cam_compass_click_counter = 0
cam_last_announced_compass_idx = -1
cam_last_compass_ts = 0.0

# Vehicle Details Browsing State
_vehicle_selector_open = False
_vehicle_detail_lines = []
_vehicle_detail_index = 0
_vehicle_detail_arrow_hooks = []
audio_controller_ref = None

# =========================
#  Keyboard (suppressed layered commands)
# =========================
try:
    import keyboard
    KEYBOARD_OK = True
except Exception:
    KEYBOARD_OK = False
    logger.warning("keyboard module unavailable – 'pip install keyboard' and run as Administrator for key suppression.")

next_key_hook_press, next_key_hook_release, next_key_timer = None, None, None
command_timeout_sec = 4.0
_capture_mods = {"ctrl": False, "shift": False, "alt": False}

HELP_LINES = [
    "Speaks attitude (roll/pitch): A", "Speaks coordinates: C", "Speaks engine temperature: E", "Dump all electrics values to log: SHIFT+E", "Dump all powertrain data to log: SHIFT+P", "Dump all hydros data to log: SHIFT+H",
    "Speaks fuel: F", "Speaks gear: G", "Speaks oil temperature: O", "Speaks RPM: R",
    "Speaks Max RPM (red line): Shift+R", "Speaks speed: S", "Speaks turbo pressure: T",
    "Speaks max turbo pressure: Shift+T", "Toggle Status Mode: CTRL+S",
    "Toggle Buffer Mode: CTRL+B", "Toggle Pedal Tones: CTRL+C", "Toggle Vehicle Scanner: CTRL+V",
    "Toggle Obstacle Detection: CTRL+O",
    "Align to trailer coupler: SHIFT+V", "Cycle scanner target: TAB / SHIFT+TAB",
    "Toggle Heading Guidance: CTRL+H", "Toggle Drift Detection: CTRL+D",
    "Toggle Low Speed Detection: CTRL+L",
    "Mark waypoint at current position: SHIFT+C",
    "Speak marked waypoint: ALT+C",
    "Distance and bearing to waypoint: W",
    "Scanner distance and approach direction: D (scanner must be on)",
    "Toggle Coordinate Guidance: CTRL+G",
    "Toggle Camera Info: ALT+F",
    "Camera heading: ALT+H", "Camera altitude: ALT+A", "Camera pitch: ALT+P",
    "Vehicle bearing from camera: ALT+V", "Vehicle distance from camera: ALT+D",
    "Damage Report: M",
    "Show this help: question mark or slash",
]
HELP_TEXT = ",\n".join(HELP_LINES)


def _clear_next_key_hook(speak_exit: bool):
    global next_key_hook_press, next_key_hook_release, next_key_timer, _command_context
    _command_context = False
    try:
        if next_key_hook_press is not None: keyboard.unhook(next_key_hook_press)
    except Exception: pass
    try:
        if next_key_hook_release is not None: keyboard.unhook(next_key_hook_release)
    except Exception: pass
    next_key_hook_press, next_key_hook_release = None, None
    if next_key_timer is not None:
        try: next_key_timer.cancel()
        except Exception: pass
    next_key_timer = None
    _capture_mods["ctrl"] = _capture_mods["shift"] = _capture_mods["alt"] = False
    if speak_exit: say("Exit", exclude_from_buffer=True)

# Data structure for Status Mode
STATUS_METRICS = [
    {'label': 'Heading', 'getValue': lambda: (f"{last_heading:.1f}", "degrees"), 'isAvailable': lambda: True},
    {'label': 'Speed', 'getValue': lambda: fmt_speed(last_speed_ms), 'isAvailable': lambda: True},
    {'label': 'RPM', 'getValue': lambda: (int(round(last_rpm)), 'RPM'), 'isAvailable': lambda: True},
    {'label': 'Gear', 'getValue': lambda: (gear_to_phrase(last_gear_byte) if protocol_mode == 'outgauge' else extended_gear_to_phrase(last_gear_str or ''), ''), 'isAvailable': lambda: True},
    {'label': 'Fuel', 'getValue': lambda: (f"{int(round(last_fuel * 100))}", "percent"), 'isAvailable': lambda: True},
    {'label': 'Engine Temperature', 'getValue': lambda: fmt_temp_c_or_f(last_engtemp), 'isAvailable': lambda: True},
    {'label': 'Oil Temperature', 'getValue': lambda: fmt_temp_c_or_f(last_oiltemp), 'isAvailable': lambda: True},
    # {'label': 'Clutch Temperature', 'getValue': lambda: fmt_temp_c_or_f(last_clutch_temp), 'isAvailable': lambda: protocol_mode == 'extended'},
    {'label': 'Turbo Pressure', 'getValue': lambda: fmt_turbo(last_turbo), 'isAvailable': lambda: True},
    # {'label': 'Oil Pressure', 'getValue': lambda: fmt_turbo(last_oil_pressure), 'isAvailable': lambda: protocol_mode == 'extended'},
    # {'label': 'Air System Pressure', 'getValue': lambda: fmt_pressure(last_air_pressure), 'isAvailable': lambda: protocol_mode == 'extended' and last_air_pressure_max > 0},
    # {'label': 'Lateral G-Force', 'getValue': lambda: (f"{last_g_lat:.2f}", "G"), 'isAvailable': lambda: protocol_mode == 'extended'},
    # {'label': 'Longitudinal G-Force', 'getValue': lambda: (f"{last_g_lon:.2f}", "G"), 'isAvailable': lambda: protocol_mode == 'extended'},
    {'label': 'Tire Pressures (FL, FR, RL, RR)', 'getValue': lambda: (f"{fmt_pressure(last_tire_pressure_fl)[0]}, {fmt_pressure(last_tire_pressure_fr)[0]}, {fmt_pressure(last_tire_pressure_rl)[0]}, {fmt_pressure(last_tire_pressure_rr)[0]}", "psi" if UNITS_MODE == 'imperial' else "bar"), 'isAvailable': lambda: protocol_mode == 'extended'},
    # {'label': 'Tire Temps (FL, FR, RL, RR)', 'getValue': lambda: (f"{fmt_temp_c_or_f(last_tire_temp_fl)[0]}, {fmt_temp_c_or_f(last_tire_temp_fr)[0]}, {fmt_temp_c_or_f(last_tire_temp_rl)[0]}, {fmt_temp_c_or_f(last_tire_temp_rr)[0]}", "F" if UNITS_MODE == 'imperial' else "C"), 'isAvailable': lambda: protocol_mode == 'extended'},
    # {'label': 'Brake Temps (FL, FR, RL, RR)', 'getValue': lambda: (f"{fmt_temp_c_or_f(last_brake_temp_fl)[0]}, {fmt_temp_c_or_f(last_brake_temp_fr)[0]}, {fmt_temp_c_or_f(last_brake_temp_rl)[0]}, {fmt_temp_c_or_f(last_brake_temp_rr)[0]}", "F" if UNITS_MODE == 'imperial' else "C"), 'isAvailable': lambda: protocol_mode == 'extended'},
]

def on_status_arrow_press(event):
    global current_status_metric_index
    if _vehicle_selector_open: return
    if not status_mode_active: return
    with state_lock:
        available_metrics = [m for m in STATUS_METRICS if m['isAvailable']()]
        if not available_metrics:
            say("No status metrics available", exclude_from_buffer=True)
            return
        current_status_metric_index %= len(available_metrics)
        if event.name == 'down':
            current_status_metric_index = (current_status_metric_index + 1) % len(available_metrics)
        elif event.name == 'up':
            current_status_metric_index = (current_status_metric_index - 1 + len(available_metrics)) % len(available_metrics)
        metric = available_metrics[current_status_metric_index]
        value, unit = metric['getValue']()
        say(f"{metric['label']}, {value} {unit}" if event.name in ('up', 'down') else f"{value} {unit}", exclude_from_buffer=True)

def toggle_status_mode():
    global status_mode_active, current_status_metric_index, status_arrow_hooks
    status_mode_active = not status_mode_active
    if status_mode_active:
        current_status_metric_index = 0
        say("Status mode on", exclude_from_buffer=True)
        try:
            for key in ['up', 'down', 'left', 'right']:
                status_arrow_hooks.append(keyboard.on_press_key(key, on_status_arrow_press, suppress=True))
        except Exception as e: logger.error(f"Failed to hook status mode keys: {e}")
    else:
        say("Status mode off", exclude_from_buffer=True)
        for hook in status_arrow_hooks:
            try: keyboard.unhook(hook)
            except Exception: pass
        status_arrow_hooks.clear()

def on_buffer_nav_press(event):
    global current_buffer_index
    if not buffer_mode_active: return
    with state_lock:
        if not SPEECH_BUFFER:
            say("Buffer empty", exclude_from_buffer=True)
            return
        
        if event.name == ']': # Right bracket, newer messages
            current_buffer_index += 1
            if current_buffer_index >= len(SPEECH_BUFFER):
                current_buffer_index = len(SPEECH_BUFFER) - 1
                say(f"Bottom: {SPEECH_BUFFER[current_buffer_index]}", exclude_from_buffer=True)
            else:
                say(SPEECH_BUFFER[current_buffer_index], exclude_from_buffer=True)
        
        elif event.name == '[': # Left bracket, older messages
            current_buffer_index -= 1
            if current_buffer_index < 0:
                current_buffer_index = 0
                say(f"Top: {SPEECH_BUFFER[current_buffer_index]}", exclude_from_buffer=True)
            else:
                say(SPEECH_BUFFER[current_buffer_index], exclude_from_buffer=True)

def toggle_buffer_mode():
    global buffer_mode_active, current_buffer_index, buffer_key_hooks
    buffer_mode_active = not buffer_mode_active
    if buffer_mode_active:
        current_buffer_index = len(SPEECH_BUFFER) -1 if SPEECH_BUFFER else -1
        say("Buffer mode on", exclude_from_buffer=True)
        try:
            for key in ['[', ']']:
                buffer_key_hooks.append(keyboard.on_press_key(key, on_buffer_nav_press, suppress=True))
        except Exception as e: logger.error(f"Failed to hook buffer nav keys: {e}")
    else:
        say("Buffer mode off", exclude_from_buffer=True)
        for hook in buffer_key_hooks:
            try: keyboard.unhook(hook)
            except Exception: pass
        buffer_key_hooks.clear()

def _on_vehicle_detail_arrow(event):
    global _vehicle_detail_index
    if not _vehicle_detail_lines:
        return
    if event.name == 'down':
        _vehicle_detail_index += 1
        if _vehicle_detail_index >= len(_vehicle_detail_lines):
            _vehicle_detail_index = len(_vehicle_detail_lines) - 1
            say(f"Bottom: {_vehicle_detail_lines[_vehicle_detail_index]}", exclude_from_buffer=True)
        else:
            say(_vehicle_detail_lines[_vehicle_detail_index], exclude_from_buffer=True)
    elif event.name == 'up':
        _vehicle_detail_index -= 1
        if _vehicle_detail_index < 0:
            _vehicle_detail_index = 0
            say(f"Top: {_vehicle_detail_lines[_vehicle_detail_index]}", exclude_from_buffer=True)
        else:
            say(_vehicle_detail_lines[_vehicle_detail_index], exclude_from_buffer=True)


def _on_vehicle_details_received(lines):
    global _vehicle_detail_lines, _vehicle_detail_index
    filtered = [l.strip() for l in lines if isinstance(l, str) and l.strip()]
    if not filtered:
        return
    _vehicle_detail_lines = filtered
    _vehicle_detail_index = 0
    if audio_controller_ref:
        audio_controller_ref.trigger_details_tone()


def _on_vehicle_selector_state(is_open):
    global _vehicle_selector_open, _vehicle_detail_lines, _vehicle_detail_index, _vehicle_detail_arrow_hooks
    if is_open == _vehicle_selector_open:
        return
    _vehicle_selector_open = is_open
    if is_open:
        if KEYBOARD_OK:
            try:
                for key in ['up', 'down']:
                    _vehicle_detail_arrow_hooks.append(
                        keyboard.on_press_key(key, _on_vehicle_detail_arrow, suppress=True)
                    )
            except Exception as e:
                logger.error(f"Failed to hook vehicle detail arrow keys: {e}")
    else:
        for hook in _vehicle_detail_arrow_hooks:
            try:
                keyboard.unhook(hook)
            except Exception:
                pass
        _vehicle_detail_arrow_hooks.clear()
        _vehicle_detail_lines = []
        _vehicle_detail_index = 0


SCANNER_CMD_PORT = 4448  # UDP port to send ON/OFF commands to vehicle scanner
AI_CMD_PORT = 4449       # UDP port to send AI commands to beamtelAI extension
CAMERA_LISTEN_PORT = 4450  # UDP port to receive camera data from cameraInfo.lua
CAMERA_CMD_PORT = 4451     # UDP port to send ON/OFF commands to cameraInfo.lua
OBSTACLE_LISTEN_PORT = 4452  # UDP port to receive obstacle data from obstacleDetector.lua
OBSTACLE_CMD_PORT = 4453     # UDP port to send ON/OFF commands to obstacleDetector.lua

# AI Control State
ai_speed_limit_ms = None     # current speed limit in m/s, None = off
ai_avoid_mode = "auto"       # "auto", "on", "off"
ai_lane_driving = False

def toggle_scan_mode(audio_controller):
    global scan_mode_active, last_scanner_target_name, last_scanner_distance, last_scanner_approach_deg
    scan_mode_active = not scan_mode_active
    last_scanner_target_name = ""
    last_scanner_distance = float('inf')
    last_scanner_approach_deg = 0.0

    command = "ON" if scan_mode_active else "OFF"

    try:
        cmd_sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        cmd_sock.sendto(command.encode("utf-8"), ("127.0.0.1", SCANNER_CMD_PORT))
        cmd_sock.close()
    except Exception as e:
        logger.error(f"Failed to send scanner command via UDP: {e}")

    audio_controller.set_scan_mode(scan_mode_active)

    say(f"Vehicle scanner {'on' if scan_mode_active else 'off'}", exclude_from_buffer=True)

def toggle_obstacle_mode(audio_controller):
    global obstacle_mode_active
    obstacle_mode_active = not obstacle_mode_active

    command = "ON" if obstacle_mode_active else "OFF"

    try:
        cmd_sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        cmd_sock.sendto(command.encode("utf-8"), ("127.0.0.1", OBSTACLE_CMD_PORT))
        cmd_sock.close()
    except Exception as e:
        logger.error(f"Failed to send obstacle detector command via UDP: {e}")

    audio_controller.set_obstacle_mode(obstacle_mode_active)

    say(f"Obstacle detection {'on' if obstacle_mode_active else 'off'}", exclude_from_buffer=True)

def toggle_free_cam_info(audio_controller):
    global free_cam_active, cam_last_click_heading_deg, cam_compass_click_counter
    global cam_last_announced_compass_idx, cam_last_compass_ts
    free_cam_active = not free_cam_active

    command = "ON" if free_cam_active else "OFF"
    try:
        cmd_sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        cmd_sock.sendto(command.encode("utf-8"), ("127.0.0.1", CAMERA_CMD_PORT))
        cmd_sock.close()
    except Exception as e:
        logger.error(f"Failed to send camera info command via UDP: {e}")

    if free_cam_active:
        with state_lock:
            cam_last_click_heading_deg = 0.0
            cam_compass_click_counter = 0
            cam_last_announced_compass_idx = -1
            cam_last_compass_ts = 0.0

    say(f"Camera info {'on' if free_cam_active else 'off'}", exclude_from_buffer=True)


def _on_next_key_press(event, audio_controller):
    global marked_coord_x, marked_coord_y, _last_coord_bearing_ts
    if event.event_type != "down": return
    name = (event.name or "").lower()

    if name in ("ctrl", "control", "left ctrl", "right ctrl"): _capture_mods["ctrl"] = True; return
    if name in ("shift", "left shift", "right shift"): _capture_mods["shift"] = True; return
    if name in ("alt", "left alt", "right alt"): _capture_mods["alt"] = True; return

    if name == "f9": return

    if name.isdigit() and not (_capture_mods["ctrl"] or _capture_mods["shift"] or _capture_mods["alt"]):
        target_recency = 10 if name == '0' else int(name)
        with state_lock:
            if SPEECH_BUFFER and 1 <= target_recency <= len(SPEECH_BUFFER):
                message = SPEECH_BUFFER[-target_recency]
                say(message, exclude_from_buffer=True)
            else:
                say("No message", exclude_from_buffer=True)
        _clear_next_key_hook(speak_exit=False)
        return

    with state_lock:
        spd_val, spd_unit = fmt_speed(last_speed_ms)
        rpm = int(round(last_rpm))
        fuel_pct = int(round(max(0.0, min(1.0, last_fuel)) * 100))
        turbo_val, turbo_unit = fmt_turbo(last_turbo)
        etemp_val, etemp_unit = fmt_temp_c_or_f(last_engtemp)
        otemp_val, otemp_unit = fmt_temp_c_or_f(last_oiltemp)
        gear_phrase = gear_to_phrase(last_gear_byte if last_gear_byte is not None else 1) if protocol_mode == "outgauge" else extended_gear_to_phrase(last_gear_str or '')
        x, y, z = last_pos_x, last_pos_y, last_pos_z
        roll_deg_snap = math.degrees(last_roll_rad)
        pitch_deg_snap = math.degrees(last_pitch_rad)
        hdg = fmt_heading()
        heading_snap = last_heading
        rpm_max_snap = last_rpm_max
        turbo_max_snap = last_turbo_max
        air_pressure_snap = last_air_pressure
        air_pressure_max_snap = last_air_pressure_max
        cam_yaw_snap = cam_yaw_deg
        cam_pitch_snap = cam_pitch_deg
        cam_agl_snap = cam_agl
        cam_bearing_snap = cam_veh_bearing
        cam_dist_snap = cam_veh_distance

    # ALT+ camera info commands (before bare key handlers)
    if name == "f" and _capture_mods["alt"] and not (_capture_mods["ctrl"] or _capture_mods["shift"]):
        toggle_free_cam_info(audio_controller)
        _clear_next_key_hook(speak_exit=False)
        return
    elif name == "h" and _capture_mods["alt"] and not (_capture_mods["ctrl"] or _capture_mods["shift"]):
        if free_cam_active:
            say(f"Camera heading {cam_yaw_snap:.0f} degrees", exclude_from_buffer=True)
        else:
            say("Camera info is off", exclude_from_buffer=True)
        _clear_next_key_hook(speak_exit=False)
        return
    elif name == "a" and _capture_mods["alt"] and not (_capture_mods["ctrl"] or _capture_mods["shift"]):
        if free_cam_active:
            if UNITS_MODE == "imperial":
                alt_val = cam_agl_snap * 3.28084
                say(f"Camera altitude {alt_val:.1f} feet", exclude_from_buffer=True)
            else:
                say(f"Camera altitude {cam_agl_snap:.1f} meters", exclude_from_buffer=True)
        else:
            say("Camera info is off", exclude_from_buffer=True)
        _clear_next_key_hook(speak_exit=False)
        return
    elif name == "p" and _capture_mods["alt"] and not (_capture_mods["ctrl"] or _capture_mods["shift"]):
        if free_cam_active:
            direction = "up" if cam_pitch_snap >= 0 else "down"
            say(f"Camera pitch {abs(cam_pitch_snap):.0f} degrees {direction}", exclude_from_buffer=True)
        else:
            say("Camera info is off", exclude_from_buffer=True)
        _clear_next_key_hook(speak_exit=False)
        return
    elif name == "v" and _capture_mods["alt"] and not (_capture_mods["ctrl"] or _capture_mods["shift"]):
        if free_cam_active:
            if cam_dist_snap < 0:
                say("No vehicle", exclude_from_buffer=True)
            else:
                direction = "left" if cam_bearing_snap > 0 else "right"
                say(f"Vehicle {abs(cam_bearing_snap):.0f} degrees {direction}", exclude_from_buffer=True)
        else:
            say("Camera info is off", exclude_from_buffer=True)
        _clear_next_key_hook(speak_exit=False)
        return
    elif name == "d" and _capture_mods["alt"] and not (_capture_mods["ctrl"] or _capture_mods["shift"]):
        if free_cam_active:
            if cam_dist_snap < 0:
                say("No vehicle", exclude_from_buffer=True)
            else:
                if UNITS_MODE == "imperial":
                    dist_val = cam_dist_snap * 3.28084
                    say(f"Vehicle {dist_val:.0f} feet away", exclude_from_buffer=True)
                else:
                    say(f"Vehicle {cam_dist_snap:.0f} meters away", exclude_from_buffer=True)
        else:
            say("Camera info is off", exclude_from_buffer=True)
        _clear_next_key_hook(speak_exit=False)
        return

    if name == "s" and not _capture_mods["ctrl"]: say(f"{spd_val} {spd_unit}")
    elif name == "r" and _capture_mods["shift"]: say(f"Redline {int(round(rpm_max_snap))} RPM" if protocol_mode == "extended" else "Unavailable")
    elif name == "r": say(f"{rpm} rpm")
    elif name == "h" and _capture_mods["ctrl"]:
        global heading_guidance_active, heading_guidance_target
        heading_guidance_active = not heading_guidance_active
        if heading_guidance_active:
            with state_lock:
                heading_guidance_target = last_heading
            say(f"Heading guidance on", exclude_from_buffer=True)
        else:
            say("Heading guidance off", exclude_from_buffer=True)
    elif name == "d" and _capture_mods["ctrl"]:
        global drift_mode_active, last_drift_check_ts, drift_baseline_heading, drift_alert_active
        drift_mode_active = not drift_mode_active
        if drift_mode_active:
            with state_lock:
                last_drift_check_ts = time.time()
                drift_baseline_heading = last_heading
                drift_alert_active = False
            say("Drift detection on", exclude_from_buffer=True)
        else:
            drift_alert_active = False
            say("Drift detection off", exclude_from_buffer=True)
    elif name == "d" and not (_capture_mods["ctrl"] or _capture_mods["shift"] or _capture_mods["alt"]):
        if not scan_mode_active:
            say("Scanner is not active", exclude_from_buffer=True)
        else:
            with state_lock:
                dist     = last_scanner_distance
                approach = last_scanner_approach_deg
                tname    = last_scanner_target_name
            if dist == float('inf'):
                say("No target", exclude_from_buffer=True)
            else:
                a = abs(approach)
                if a < 45:
                    direction = "front"
                elif a > 135:
                    direction = "rear"
                elif approach > 0:
                    direction = "right"
                else:
                    direction = "left"
                if UNITS_MODE == "imperial":
                    dist_str = f"{dist * 3.28084:.0f} feet"
                else:
                    dist_str = f"{dist:.0f} meters"
                prefix = f"{tname}, " if tname else ""
                say(f"{prefix}{dist_str} away, approaching {direction}", exclude_from_buffer=True)
    elif name == "w" and not (_capture_mods["ctrl"] or _capture_mods["shift"] or _capture_mods["alt"]):
        if marked_coord_x is None:
            say("No waypoint marked", exclude_from_buffer=True)
        else:
            dx = marked_coord_x - x
            dy = marked_coord_y - y
            dist = math.sqrt(dx * dx + dy * dy)
            if dist < 0.5:
                say("At waypoint", exclude_from_buffer=True)
            else:
                target_brg = math.degrees(math.atan2(-dx, -dy)) % 360.0
                err = heading_snap - target_brg
                if err > 180.0: err -= 360.0
                elif err < -180.0: err += 360.0
                turn_dir = "left" if err > 0 else "right"
                if UNITS_MODE == "imperial":
                    dist_str = f"{dist * 3.28084:.0f} feet"
                else:
                    dist_str = f"{dist:.0f} meters"
                say(f"{dist_str}, {abs(err):.0f} degrees {turn_dir}", exclude_from_buffer=True)
    elif name == "l" and _capture_mods["ctrl"] and not (_capture_mods["shift"] or _capture_mods["alt"]):
        global low_speed_mode_active
        low_speed_mode_active = not low_speed_mode_active
        say("Low speed detection on" if low_speed_mode_active else "Low speed detection off", exclude_from_buffer=True)
    elif name == "g" and _capture_mods["ctrl"]:
        global coord_guidance_active
        coord_guidance_active = not coord_guidance_active
        if coord_guidance_active:
            if marked_coord_x is None:
                coord_guidance_active = False
                say("No waypoint marked", exclude_from_buffer=True)
            else:
                say("Coordinate guidance on", exclude_from_buffer=True)
        else:
            say("Coordinate guidance off", exclude_from_buffer=True)
    elif name == "h" and _capture_mods["shift"]:
        say("Requesting hydros dump", exclude_from_buffer=True)
        def _do_hdump():
            DUMP_CMD_PORT  = 4446
            DUMP_RESP_PORT = 4447
            recv_sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
            try:
                recv_sock.bind(("127.0.0.1", DUMP_RESP_PORT))
                cmd_sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
                cmd_sock.sendto(b"HDUMP", ("127.0.0.1", DUMP_CMD_PORT))
                cmd_sock.close()
                recv_sock.settimeout(5.0)
                lines = []
                for _ in range(1000):
                    try:
                        data, _ = recv_sock.recvfrom(4096)
                        text = data.decode("utf-8", errors="replace").strip()
                        if text == "DONE":
                            break
                        lines.append(text)
                        recv_sock.settimeout(0.5)
                    except socket.timeout:
                        break
            finally:
                recv_sock.close()
            if lines:
                lines.sort()
                content = "\n".join(lines)
                logger.info(f"=== HYDROS DUMP ({len(lines)} values) ===\n{content}\n=== END DUMP ===")
                say(f"Hydros dumped, {len(lines)} values logged", exclude_from_buffer=True)
            else:
                say("Hydros dump timed out — is a vehicle spawned?", exclude_from_buffer=True)
        threading.Thread(target=_do_hdump, daemon=True).start()
    elif name == "h": say(hdg)
    elif name == "f": say(f"Fuel {fuel_pct} percent")
    elif name == "g": say(gear_phrase)
    elif name == "t" and _capture_mods["shift"]:
        val, unit = fmt_turbo(turbo_max_snap)
        say(f"Max turbo {val} {unit}" if protocol_mode == "extended" else "Unavailable")
    elif name == "t": say(f"Turbo {turbo_val} {turbo_unit}")
    elif name == "p" and _capture_mods["shift"]:
        say("Requesting powertrain dump", exclude_from_buffer=True)
        def _do_pdump():
            DUMP_CMD_PORT  = 4446
            DUMP_RESP_PORT = 4447
            recv_sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
            try:
                recv_sock.bind(("127.0.0.1", DUMP_RESP_PORT))
                cmd_sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
                cmd_sock.sendto(b"PDUMP", ("127.0.0.1", DUMP_CMD_PORT))
                cmd_sock.close()
                recv_sock.settimeout(5.0)
                lines = []
                for _ in range(1000):
                    try:
                        data, _ = recv_sock.recvfrom(4096)
                        text = data.decode("utf-8", errors="replace").strip()
                        if text == "DONE":
                            break
                        lines.append(text)
                        recv_sock.settimeout(0.5)
                    except socket.timeout:
                        break
            finally:
                recv_sock.close()
            if lines:
                lines.sort()
                content = "\n".join(lines)
                logger.info(f"=== POWERTRAIN DUMP ({len(lines)} values) ===\n{content}\n=== END DUMP ===")
                say(f"Powertrain dumped, {len(lines)} values logged", exclude_from_buffer=True)
            else:
                say("Powertrain dump timed out — is a vehicle spawned?", exclude_from_buffer=True)
        threading.Thread(target=_do_pdump, daemon=True).start()
    elif name == "p" and not _capture_mods["shift"]:
        if protocol_mode == "extended":
            if air_pressure_snap > 0:
                val, unit = fmt_pressure(air_pressure_snap)
                if air_pressure_max_snap > 1:
                    max_val, _ = fmt_pressure(air_pressure_max_snap)
                    say(f"Air pressure {val} of {max_val} {unit}")
                else:
                    say(f"Air pressure {val} {unit}")
            else: say("Pneumatic system not available")
        else: say("Pneumatic data unavailable")
    elif name == "u": flip_units(); say(UNITS_MODE, exclude_from_buffer=True)
    elif name == "e" and _capture_mods["shift"]:
        say("Requesting electrics dump", exclude_from_buffer=True)
        def _do_dump():
            DUMP_CMD_PORT  = 4446
            DUMP_RESP_PORT = 4447
            # Open receive socket first so we don't miss the response
            recv_sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
            try:
                recv_sock.bind(("127.0.0.1", DUMP_RESP_PORT))
                # Send the DUMP command to the vehicle protocol
                cmd_sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
                cmd_sock.sendto(b"DUMP", ("127.0.0.1", DUMP_CMD_PORT))
                cmd_sock.close()
                # Collect lines until "DONE" or timeout
                recv_sock.settimeout(5.0)   # wait up to 5 s for first packet
                lines = []
                for _ in range(1000):
                    try:
                        data, _ = recv_sock.recvfrom(4096)
                        text = data.decode("utf-8", errors="replace").strip()
                        if text == "DONE":
                            break
                        lines.append(text)
                        recv_sock.settimeout(0.5)  # shorter timeout after first
                    except socket.timeout:
                        break
            finally:
                recv_sock.close()
            if lines:
                lines.sort()
                content = "\n".join(lines)
                logger.info(f"=== ELECTRICS DUMP ({len(lines)} values) ===\n{content}\n=== END DUMP ===")
                say(f"Electrics dumped, {len(lines)} values logged", exclude_from_buffer=True)
            else:
                say("Electrics dump timed out — is a vehicle spawned?", exclude_from_buffer=True)
        threading.Thread(target=_do_dump, daemon=True).start()
    elif name in ("e", "engtemp") and not _capture_mods["shift"]: say(f"Engine temperature {etemp_val} {etemp_unit}")
    elif name == "m" and not (_capture_mods["ctrl"] or _capture_mods["shift"] or _capture_mods["alt"]):
        say("Checking damage", exclude_from_buffer=True)
        def _do_damage_report():
            DUMP_CMD_PORT  = 4446
            DUMP_RESP_PORT = 4447
            recv_sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
            try:
                recv_sock.bind(("127.0.0.1", DUMP_RESP_PORT))
                cmd_sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
                cmd_sock.sendto(b"DAMAGE", ("127.0.0.1", DUMP_CMD_PORT))
                cmd_sock.close()
                recv_sock.settimeout(5.0)
                items = []
                for _ in range(200):
                    try:
                        data, _ = recv_sock.recvfrom(4096)
                        text = data.decode("utf-8", errors="replace").strip()
                        if text == "DONE":
                            break
                        if text == "NONE":
                            continue
                        items.append(text)
                        recv_sock.settimeout(0.5)
                    except socket.timeout:
                        break
            finally:
                recv_sock.close()
            if not items:
                say("No damaged components", exclude_from_buffer=True)
            else:
                if len(items) == 1:
                    sentence = items[0]
                elif len(items) == 2:
                    sentence = f"{items[0]} and {items[1]}"
                else:
                    sentence = ", ".join(items[:-1]) + ", and " + items[-1]
                say(sentence, exclude_from_buffer=True)
        threading.Thread(target=_do_damage_report, daemon=True).start()
    elif name == "o" and not _capture_mods["ctrl"]: say(f"Oil temperature {otemp_val} {otemp_unit}")
    elif name == "c" and _capture_mods["alt"]:
        if marked_coord_x is None:
            say("No waypoint marked", exclude_from_buffer=True)
        else:
            say(f"Waypoint X {marked_coord_x:.1f}, Y {marked_coord_y:.1f}", exclude_from_buffer=True)
    elif name == "c" and _capture_mods["shift"]:
        marked_coord_x, marked_coord_y = x, y
        _last_coord_bearing_ts = 0.0  # force immediate recalculation
        say(f"Waypoint marked at X {x:.1f}, Y {y:.1f}", exclude_from_buffer=True)
    elif name == "c" and not _capture_mods["ctrl"] and not _capture_mods["shift"] and not _capture_mods["alt"]: say(f"Coordinates X {x:.2f}, Y {y:.2f}, Z {z:.2f}")
    elif name in ("/", "?"): say(HELP_TEXT, exclude_from_buffer=True)
    elif name == "s" and _capture_mods["ctrl"]: toggle_status_mode()
    elif name == "b" and _capture_mods["ctrl"]: toggle_buffer_mode()
    elif name == "c" and _capture_mods["ctrl"]:
        global pedal_tones_active
        pedal_tones_active = not pedal_tones_active
        say("Pedal tones on" if pedal_tones_active else "Pedal tones off", exclude_from_buffer=True)
    elif name == "v" and _capture_mods["shift"]:
        if not scan_mode_active:
            say("Vehicle scanner is not active", exclude_from_buffer=True)
        else:
            say("Aligning to trailer", exclude_from_buffer=True)
            try:
                cmd_sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
                cmd_sock.sendto(b"ALIGN", ("127.0.0.1", SCANNER_CMD_PORT))
                cmd_sock.close()
            except Exception as e:
                logger.error(f"Failed to send ALIGN command via UDP: {e}")
    elif name == "v" and _capture_mods["ctrl"]:
        toggle_scan_mode(audio_controller)
    elif name == "o" and _capture_mods["ctrl"]:
        toggle_obstacle_mode(audio_controller)
    elif name == "tab":
        if not scan_mode_active:
            say("Turn on vehicle scanner first", exclude_from_buffer=True)
        else:
            direction = b"PREV" if _capture_mods["shift"] else b"NEXT"
            try:
                sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
                sock.sendto(direction, ("127.0.0.1", SCANNER_CMD_PORT))
                sock.close()
            except Exception as e:
                logger.error(f"Failed to send {direction} command: {e}")
    elif name == "a": say(f"Roll {roll_deg_snap:.1f} degrees, pitch {pitch_deg_snap:.1f} degrees")
    elif name == "space" and not (_capture_mods["ctrl"] or _capture_mods["shift"] or _capture_mods["alt"]):
        broadcast({"type": "context_action", "action": "activate"})

    _clear_next_key_hook(speak_exit=False)


def _on_next_key_release(event):
    if event.event_type != "up": return
    name = (event.name or "").lower()
    if name in ("ctrl", "control", "left ctrl", "right ctrl"): _capture_mods["ctrl"] = False
    elif name in ("shift", "left shift", "right shift"): _capture_mods["shift"] = False
    elif name in ("alt", "left alt", "right alt"): _capture_mods["alt"] = False

def _start_next_key_capture(audio_controller):
    global next_key_hook_press, next_key_hook_release, next_key_timer
    _clear_next_key_hook(speak_exit=False)
    next_key_hook_press = keyboard.on_press(lambda e: _on_next_key_press(e, audio_controller), suppress=True)
    next_key_hook_release = keyboard.on_release(_on_next_key_release)
    next_key_timer = threading.Timer(command_timeout_sec, lambda: _clear_next_key_hook(speak_exit=True))
    next_key_timer.daemon = True
    next_key_timer.start()


# =========================
#  AI Control (F10 layer)
# =========================
_AGGRESSION_MAP = {
    '1': 0.2, '2': 0.4, '3': 0.7, '4': 0.9,
    '5': 1.0, '6': 1.2, '7': 1.5, '8': 1.8, '9': 2.0,
}
_AVOID_CYCLE = ["auto", "on", "off"]

ai_hook_press = None
ai_hook_release = None
ai_timer = None
_ai_mods = {"ctrl": False, "shift": False, "alt": False}

def _send_ai_command(cmd):
    try:
        sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        sock.sendto(cmd.encode("utf-8"), ("127.0.0.1", AI_CMD_PORT))
        sock.close()
    except Exception as e:
        logger.error(f"Failed to send AI command via UDP: {e}")

def _clear_ai_hook(speak_exit: bool):
    global ai_hook_press, ai_hook_release, ai_timer, _command_context
    _command_context = False
    try:
        if ai_hook_press is not None: keyboard.unhook(ai_hook_press)
    except Exception: pass
    try:
        if ai_hook_release is not None: keyboard.unhook(ai_hook_release)
    except Exception: pass
    ai_hook_press, ai_hook_release = None, None
    if ai_timer is not None:
        try: ai_timer.cancel()
        except Exception: pass
    ai_timer = None
    _ai_mods["ctrl"] = _ai_mods["shift"] = _ai_mods["alt"] = False
    if speak_exit: say("Exit", exclude_from_buffer=True)

def _on_ai_key_release(event):
    if event.event_type != "up": return
    name = (event.name or "").lower()
    if name in ("ctrl", "control", "left ctrl", "right ctrl"): _ai_mods["ctrl"] = False
    elif name in ("shift", "left shift", "right shift"): _ai_mods["shift"] = False
    elif name in ("alt", "left alt", "right alt"): _ai_mods["alt"] = False

def _on_ai_key_press(event):
    global ai_speed_limit_ms, ai_avoid_mode, ai_lane_driving
    if event.event_type != "down": return
    name = (event.name or "").lower()

    if name in ("ctrl", "control", "left ctrl", "right ctrl"): _ai_mods["ctrl"] = True; return
    if name in ("shift", "left shift", "right shift"): _ai_mods["shift"] = True; return
    if name in ("alt", "left alt", "right alt"): _ai_mods["alt"] = True; return
    if name == "f10": return

    if name == "d":
        say("AI disabled", exclude_from_buffer=True)
        _send_ai_command("MODE:disabled")
    elif name == "t":
        say("Traffic mode", exclude_from_buffer=True)
        _send_ai_command("MODE:traffic")
    elif name == "r":
        say("Random mode", exclude_from_buffer=True)
        _send_ai_command("MODE:random")
    elif name == "s":
        say("AI stop", exclude_from_buffer=True)
        _send_ai_command("MODE:stop")
    elif name == "c":
        say("Chase mode", exclude_from_buffer=True)
        _send_ai_command("MODE:chase")
    elif name == "f":
        say("Follow mode", exclude_from_buffer=True)
        _send_ai_command("MODE:follow")
    elif name == "e":
        say("Flee mode", exclude_from_buffer=True)
        _send_ai_command("MODE:flee")
    elif name in _AGGRESSION_MAP:
        val = _AGGRESSION_MAP[name]
        say(f"Aggression {val}", exclude_from_buffer=True)
        _send_ai_command(f"AGGR:{val}")
    elif name in ("+", "=", "add"):
        if ai_speed_limit_ms is None:
            ai_speed_limit_ms = 10.0 / 3.6  # start at 10 km/h
        else:
            ai_speed_limit_ms += 10.0 / 3.6
        kmh = int(round(ai_speed_limit_ms * 3.6))
        if UNITS_MODE == "imperial":
            mph = int(round(ai_speed_limit_ms * MPH_PER_MS))
            say(f"Speed limit {mph} mph", exclude_from_buffer=True)
        else:
            say(f"Speed limit {kmh} km/h", exclude_from_buffer=True)
        _send_ai_command(f"SPEED:{ai_speed_limit_ms:.2f}")
    elif name in ("-", "subtract"):
        if ai_speed_limit_ms is None or ai_speed_limit_ms <= 10.0 / 3.6:
            ai_speed_limit_ms = 10.0 / 3.6
        else:
            ai_speed_limit_ms -= 10.0 / 3.6
        kmh = int(round(ai_speed_limit_ms * 3.6))
        if UNITS_MODE == "imperial":
            mph = int(round(ai_speed_limit_ms * MPH_PER_MS))
            say(f"Speed limit {mph} mph", exclude_from_buffer=True)
        else:
            say(f"Speed limit {kmh} km/h", exclude_from_buffer=True)
        _send_ai_command(f"SPEED:{ai_speed_limit_ms:.2f}")
    elif name == "0":
        ai_speed_limit_ms = None
        say("Speed limit off", exclude_from_buffer=True)
        _send_ai_command("CLEARSPEED")
    elif name == "a":
        idx = _AVOID_CYCLE.index(ai_avoid_mode) if ai_avoid_mode in _AVOID_CYCLE else 0
        ai_avoid_mode = _AVOID_CYCLE[(idx + 1) % len(_AVOID_CYCLE)]
        say(f"Avoid cars {ai_avoid_mode}", exclude_from_buffer=True)
        _send_ai_command(f"AVOID:{ai_avoid_mode}")
    elif name == "l":
        ai_lane_driving = not ai_lane_driving
        say(f"Lane driving {'on' if ai_lane_driving else 'off'}", exclude_from_buffer=True)
        _send_ai_command(f"LANE:{'on' if ai_lane_driving else 'off'}")
    elif name in ("/", "?"):
        ai_help = (
            "D disable, T traffic, R random, S stop, "
            "C chase, F follow, E flee, "
            "1 through 9 aggression, plus and minus speed limit, 0 clear speed, "
            "A cycle avoid cars, L toggle lane driving"
        )
        say(ai_help, exclude_from_buffer=True)
    else:
        _clear_ai_hook(speak_exit=False)
        return

    _clear_ai_hook(speak_exit=False)

def _start_ai_capture():
    global ai_hook_press, ai_hook_release, ai_timer
    _clear_ai_hook(speak_exit=False)
    ai_hook_press = keyboard.on_press(_on_ai_key_press, suppress=True)
    ai_hook_release = keyboard.on_release(_on_ai_key_release)
    ai_timer = threading.Timer(command_timeout_sec, lambda: _clear_ai_hook(speak_exit=True))
    ai_timer.daemon = True
    ai_timer.start()

def _is_beamng_focused() -> bool:
    """Return True if a BeamNG.drive window currently has foreground focus."""
    try:
        hwnd = ctypes.windll.user32.GetForegroundWindow()
        buf = ctypes.create_unicode_buffer(256)
        ctypes.windll.user32.GetWindowTextW(hwnd, buf, 256)
        return "BeamNG" in buf.value
    except Exception:
        return False


def install_hotkeys(audio_controller):
    if not KEYBOARD_OK:
        logger.info("Command mode disabled (keyboard module not available / not elevated).")
        return

    def on_f9():
        if not _is_beamng_focused():
            return
        global _command_context
        _command_context = True
        say("Command?", interrupt=True, exclude_from_buffer=True)
        _start_next_key_capture(audio_controller)

    def on_f10():
        if not _is_beamng_focused():
            return
        global _command_context
        _command_context = True
        say("AI?", interrupt=True, exclude_from_buffer=True)
        _start_ai_capture()

    keyboard.add_hotkey("f9", on_f9, suppress=False)
    keyboard.add_hotkey("f10", on_f10, suppress=False)

# =========================
#  Telemetry loop
# =========================
def telemetry_loop(audio_controller, host="0.0.0.0", port=4444, stop_event=None):
    global protocol_mode, last_pos_x, last_pos_y, last_pos_z, last_yaw_rad, last_roll_rad, last_pitch_rad, last_up_z
    global last_heading, last_click_heading_deg, compass_click_counter, last_announced_compass_idx, last_compass_ts, inverted, inverted_announced
    global last_speed_ms, last_rpm, last_fuel, last_turbo, last_engtemp, last_oiltemp, last_oil_pressure, last_rpm_max, last_turbo_max
    global last_throttle, last_brake, last_clutch, last_air_pressure, last_air_pressure_max, last_clutch_temp, last_g_lat, last_g_lon
    global last_tire_pressure_fl, last_tire_pressure_fr, last_tire_pressure_rl, last_tire_pressure_rr
    global last_tire_temp_fl, last_tire_temp_fr, last_tire_temp_rl, last_tire_temp_rr
    global last_brake_temp_fl, last_brake_temp_fr, last_brake_temp_rl, last_brake_temp_rr
    global last_gear_byte, last_gear_str, neutral_pending, neutral_start_ts, neutral_spoken, last_bucket, last_speed_announce_ts
    global drift_alert_active, last_drift_check_ts, drift_baseline_heading, drift_pan_direction # NEW
    global drift_rate_val # NEW
    global _ls_prev_speed_ms, _ls_prev_speed_ts, _ls_decel_smooth, _ls_steady_ref_mph, _ls_steady_start_ts
    global coord_target_bearing, _last_coord_bearing_ts
    drift_rate_val = 0.0
    
    # Drift state (10Hz sampling)
    prev_drift_sample_ts = 0.0
    prev_drift_sample_heading = 0.0

    cfg = load_config()
    compass_highlight_enabled = cfg.get("compass_highlight_enabled", False)
    compass_highlight_nth_click = int(cfg.get("compass_highlight_nth_click", 6))
    click_interval_deg = float(cfg.get("compass_click_interval", 15.0))

    if stop_event is None: stop_event = STOP
    
    protocol_mode = cfg.get("telemetry_protocol", "outgauge")
    logger.info(f"Telemetry mode set to: {protocol_mode.upper()}")
    logger.info(f"Listening for telemetry (UDP) on {host}:{port} ...")

    last_lowbeam_on, last_highbeam_on, last_l_signal_on, last_r_signal_on, last_hazards_on = False, False, False, False, False
    last_buzzer_ts, last_chime_ts = 0.0, 0.0
    ALERT_INTERVAL_SEC = 3.0

    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        sock.bind((host, port))
        sock.settimeout(0.25)
        while not stop_event.is_set():
            now = time.time()
            try:
                data, _ = sock.recvfrom(2048)
            except (socket.timeout, TimeoutError):
                with state_lock:
                    if neutral_pending and not neutral_spoken:
                        if (now - neutral_start_ts) >= neutral_dwell_sec:
                            say("neutral", exclude_from_buffer=True)
                            neutral_spoken = True
                continue
            except OSError:
                if stop_event.is_set(): break
                raise

            if not data: continue

            if len(data) >= 4 and data[:4] == MS_MAGIC:
                if len(data) >= MS_SIZE:
                    try:
                        ms = struct.unpack(MS_FORMAT, data[:MS_SIZE])
                        posX, posY, posZ, upZ, rollRad, pitchRad, yawPos = ms[1], ms[2], ms[3], ms[12], ms[13], ms[14], ms[15]
                    except Exception: continue
                    with state_lock:
                        last_pos_x, last_pos_y, last_pos_z = posX, posY, posZ
                        last_yaw_rad, last_roll_rad, last_pitch_rad, last_up_z = yawPos, rollRad, pitchRad, upZ
                        heading = yaw_to_heading_deg(yawPos)
                        
                        # NEW: Drift calculation (10Hz interval)
                        if drift_mode_active:
                            if prev_drift_sample_ts == 0.0:
                                prev_drift_sample_ts = now
                                prev_drift_sample_heading = heading
                            
                            dt = now - prev_drift_sample_ts
                            if dt >= 0.1: # Sample every 0.1 seconds
                                d_head = heading - prev_drift_sample_heading
                                if d_head > 180.0: d_head -= 360.0
                                elif d_head < -180.0: d_head += 360.0
                                
                                # Calculate rate over the last interval
                                rate = d_head / dt
                                drift_rate_val = abs(rate)
                                
                                # Only update direction if significant drift to avoid noise
                                if drift_rate_val > 0.2:
                                    drift_pan_direction = 1.0 if rate > 0 else -1.0
                                
                                prev_drift_sample_heading = heading
                                prev_drift_sample_ts = now
                        else:
                            drift_rate_val = 0.0
                            prev_drift_sample_ts = 0.0
                            prev_drift_sample_heading = 0.0

                        last_heading = heading
                        
                        # MODIFIED: Simplified click-counting logic
                        delta_heading = heading - last_click_heading_deg
                        if delta_heading > 180.0: delta_heading -= 360.0
                        if delta_heading < -180.0: delta_heading += 360.0
                        
                        if abs(delta_heading) >= click_interval_deg:
                            compass_click_counter += 1
                            pitch_mult = 1.0 + 0.25 * math.cos(yawPos)

                            # Check if this click should be a highlight
                            if compass_highlight_enabled and compass_click_counter >= compass_highlight_nth_click:
                                audio_controller.trigger_compass_highlight(heading, pitch_mult * 1.5)
                                compass_click_counter = 0 # Reset counter
                            else:
                                audio_controller.trigger_compass_click(heading, pitch_mult)
                            
                            # Reset the reference heading
                            num_intervals = round(heading / click_interval_deg)
                            last_click_heading_deg = (num_intervals * click_interval_deg) % 360.0
                        
                        # Compass announcement logic with tight hysteresis
                        targets = [i * 45.0 for i in range(8)]
                        half_width = 3.0
                        current_compass_idx = -1

                        for i, target_heading in enumerate(targets):
                            lower_bound = (target_heading - half_width + 360) % 360
                            upper_bound = (target_heading + half_width) % 360
                            
                            is_inside = False
                            if lower_bound < upper_bound:
                                if lower_bound <= heading < upper_bound:
                                    is_inside = True
                            else:
                                if heading >= lower_bound or heading < upper_bound:
                                    is_inside = True
                            
                            if is_inside:
                                current_compass_idx = i
                                break
                        
                        if current_compass_idx != last_announced_compass_idx:
                            if current_compass_idx != -1:
                                if (now - last_compass_ts) >= compass_min_interval:
                                    say(COMPASS_NAMES[current_compass_idx], exclude_from_buffer=True)
                                    last_compass_ts = now
                            last_announced_compass_idx = current_compass_idx
                        
                        current_inverted_state = upZ < -0.6
                        if not inverted and current_inverted_state:
                            inverted, inverted_announced = True, True
                            say("Up side down")
                        elif inverted and not current_inverted_state:
                            inverted, inverted_announced = False, False
                continue

            unpacked = None
            if protocol_mode == "extended" and len(data) == EXT_SIZE:
                try: unpacked = struct.unpack(EXT_FORMAT, data)
                except Exception: continue
            elif protocol_mode == "outgauge" and len(data) >= OG_SIZE:
                try: unpacked = struct.unpack(OG_FORMAT, data[:OG_SIZE])
                except Exception: continue
            
            if unpacked is None: continue

            shift_active_frame, tc_active_frame, showLights = False, False, 0

            with state_lock:
                if protocol_mode == "extended":
                    showLights = unpacked[13]
                    speed_ms, rpm, last_rpm_max, turbo, last_turbo_max, engtemp, fuel, oil_pressure, oiltemp = unpacked[3:12]
                    (throttle, brake, clutch, steering, actual_steering, steering_input,
                     air_pressure, air_pressure_max, clutch_temp, g_lat, g_lon,
                     tire_p_fl, tire_p_fr, tire_p_rl, tire_p_rr, tire_t_fl, tire_t_fr, tire_t_rl, tire_t_rr,
                     brake_t_fl, brake_t_fr, brake_t_rl, brake_t_rr) = unpacked[14:]
                    last_oil_pressure, last_air_pressure, last_air_pressure_max = oil_pressure, air_pressure, air_pressure_max
                    last_clutch_temp, last_g_lat, last_g_lon = clutch_temp, g_lat, g_lon
                    last_tire_pressure_fl, last_tire_pressure_fr, last_tire_pressure_rl, last_tire_pressure_rr = tire_p_fl, tire_p_fr, tire_p_rl, tire_p_rr
                    last_tire_temp_fl, last_tire_temp_fr, last_tire_temp_rl, last_tire_temp_rr = tire_t_fl, tire_t_fr, tire_t_rl, tire_t_rr
                    last_brake_temp_fl, last_brake_temp_fr, last_brake_temp_rl, last_brake_temp_rr = brake_t_fl, brake_t_fr, brake_t_rl, brake_t_rr
                else: # outgauge
                    showLights = unpacked[13]
                    speed_ms, rpm, turbo, engtemp, fuel, oil_pressure, oiltemp = unpacked[5:12]
                    throttle, brake, clutch = unpacked[14], unpacked[15], unpacked[16]
                    steering, actual_steering, steering_input = 0.0, 0.0, 0.0
                
                last_speed_ms, last_rpm, last_fuel, last_turbo, last_engtemp, last_oiltemp = speed_ms, rpm, fuel, turbo, engtemp, oiltemp
                last_throttle, last_brake, last_clutch = max(0.0, min(1.0, throttle)), max(0.0, min(1.0, brake)), max(0.0, min(1.0, clutch))
                last_steering, last_actual_steering, last_steering_input = steering, actual_steering, steering_input
                shift_active_frame, tc_active_frame = bool(showLights & DL_SHIFT), bool(showLights & DL_TC)
                
                if protocol_mode == "extended":
                    gear_str = unpacked[1].decode("utf-8", errors="ignore").strip("\x00")
                    if gear_str != last_gear_str:
                        phrase = extended_gear_to_phrase(gear_str)
                        if (gear_str or "").strip().upper() == "N": neutral_pending, neutral_start_ts, neutral_spoken = True, now, False
                        else:
                            if neutral_pending and not neutral_spoken: neutral_pending, neutral_spoken = False, False
                            if phrase not in ("unknown", "neutral"): say(phrase, exclude_from_buffer=True)
                        last_gear_str = gear_str
                else: # outgauge
                    gear_byte = unpacked[3]
                    if gear_byte != last_gear_byte:
                        phrase = gear_to_phrase(gear_byte)
                        if gear_byte == NEUTRAL: neutral_pending, neutral_start_ts, neutral_spoken = True, now, False
                        else:
                            if neutral_pending and not neutral_spoken: neutral_pending, neutral_spoken = False, False
                            if phrase not in ("unknown", "neutral"): say(phrase, exclude_from_buffer=True)
                        last_gear_byte = gear_byte
                
                if neutral_pending and not neutral_spoken and (now - neutral_start_ts) >= neutral_dwell_sec:
                    say("neutral", exclude_from_buffer=True)
                    neutral_spoken = True
                
                current_bucket = get_speed_bucket(speed_ms)
                if current_bucket != last_bucket:
                    if now - last_speed_announce_ts >= cooldown_sec:
                        spd_val, spd_unit = fmt_speed(speed_ms)
                        say(f"{spd_val} {spd_unit}", exclude_from_buffer=True)
                        last_speed_announce_ts = now
                    last_bucket = current_bucket

            # Low Speed Detection Logic
            ls_clicks_active = False
            ls_speed_mph = 0.0
            ls_decel = 0.0
            if low_speed_mode_active:
                current_mph = last_speed_ms * MPH_PER_MS

                # Compute deceleration via EMA
                if _ls_prev_speed_ts > 0.0:
                    dt = now - _ls_prev_speed_ts
                    if 0.0 < dt <= 1.0:
                        raw_accel = (last_speed_ms - _ls_prev_speed_ms) / dt
                        raw_decel = max(0.0, -raw_accel)
                        alpha = 1.0 - math.exp(-dt / 0.15)
                        _ls_decel_smooth += alpha * (raw_decel - _ls_decel_smooth)
                _ls_prev_speed_ms = last_speed_ms
                _ls_prev_speed_ts = now

                # Steady-speed suppression (3 second timer)
                if abs(current_mph - _ls_steady_ref_mph) > 1.5:
                    _ls_steady_ref_mph = current_mph
                    _ls_steady_start_ts = now
                steady_suppressed = (now - _ls_steady_start_ts) > 3.0

                # Gear check: neutral/park suppression
                in_neutral_or_park = False
                if protocol_mode == "outgauge":
                    if last_gear_byte == 1:
                        in_neutral_or_park = True
                else:
                    gs = (last_gear_str or "").strip().upper()
                    if gs in ("N", "P"):
                        in_neutral_or_park = True

                if 0.0 < current_mph < 25.0 and not in_neutral_or_park and not steady_suppressed:
                    ls_clicks_active = True
                    ls_speed_mph = current_mph
                    ls_decel = _ls_decel_smooth

            guidance_diff = 0.0
            if heading_guidance_active:
                diff = last_heading - heading_guidance_target
                if diff > 180.0: diff -= 360.0
                elif diff < -180.0: diff += 360.0
                guidance_diff = diff

            if coord_guidance_active and marked_coord_x is not None:
                if now - _last_coord_bearing_ts >= 1.0:
                    dx = marked_coord_x - last_pos_x
                    dy = marked_coord_y - last_pos_y
                    if dx * dx + dy * dy > 0.25:
                        coord_target_bearing = math.degrees(math.atan2(-dx, -dy)) % 360.0
                    _last_coord_bearing_ts = now

            coord_bearing_error = 0.0
            if coord_guidance_active and marked_coord_x is not None:
                diff = last_heading - coord_target_bearing
                if diff > 180.0: diff -= 360.0
                elif diff < -180.0: diff += 360.0
                coord_bearing_error = diff
            
            # NEW: Drift Detection Logic (Continuous)
            if drift_mode_active:
                # Activation Logic:
                # Start if: Rate > 0.5 AND Steering < 5.0 (Near zero)
                # Stop if: Rate < 0.5 (Rotation stops)
                # Note: Steering input does NOT cancel an active alert, as requested.
                
                if drift_alert_active:
                    if drift_rate_val < 0.5:
                        drift_alert_active = False
                else:
                    if drift_rate_val > 0.5 and abs(last_steering) < 5.0:
                        drift_alert_active = True
            else:
                drift_alert_active = False

            audio_controller.update_telemetry_state({
                'shift_active': shift_active_frame, 'tc_active': tc_active_frame, 'pedal_tones_active': pedal_tones_active,
                'last_clutch': last_clutch, 'last_brake': last_brake, 'last_throttle': last_throttle,
                'last_steering': last_steering,
                'last_actual_steering': last_actual_steering,
                'last_steering_input': last_steering_input,
                'inverted': inverted, 'last_roll_rad': last_roll_rad, 'last_pitch_rad': last_pitch_rad,
                'guidance_active': heading_guidance_active,
                'guidance_error_deg': guidance_diff,
                'drift_alert_active': drift_alert_active, # NEW
                'drift_pan': drift_pan_direction,         # NEW
                'drift_rate': drift_rate_val,             # NEW
                'ls_clicks_active': ls_clicks_active,
                'ls_speed_mph': ls_speed_mph,
                'ls_decel': ls_decel,
                'coord_guidance_active': coord_guidance_active,
                'coord_guidance_error_deg': coord_bearing_error,
            })

            lowbeam_on = bool(showLights & DL_LOWBEAM)
            highbeam_on = bool(showLights & DL_FULLBEAM)
            oil_warn_on = bool(showLights & DL_OILWARN)
            l_signal_on = bool(showLights & DL_SIGNAL_L)
            r_signal_on = bool(showLights & DL_SIGNAL_R)

            if lowbeam_on != last_lowbeam_on or highbeam_on != last_highbeam_on:
                if lowbeam_on and not highbeam_on:
                    say(f"Low beams on")
                elif highbeam_on and not lowbeam_on:
                    say(f"High beams on")
                else:
                    say("Headlights off")
                last_lowbeam_on = lowbeam_on
                last_highbeam_on = highbeam_on
            
            if oil_chime_enabled and oil_warn_on and (now - last_chime_ts) >= ALERT_INTERVAL_SEC:
                audio_controller.trigger_oil_chime()
                last_chime_ts = now
    finally:
        try: sock.close()
        except Exception: pass


def main():
    cfg = load_config()
    global UNITS_MODE, neutral_dwell_sec, oil_chime_enabled
    UNITS_MODE = cfg.get("units", "imperial")
    neutral_dwell_sec = max(0.0, float(cfg.get("neutral_dwell_ms", 300)) / 1000.0)
    oil_chime_enabled = cfg.get("oil_chime_enabled", True)

    global audio_controller_ref
    audio_controller = AudioController(logger)
    audio_controller.apply_config(cfg)
    audio_controller.start()
    audio_controller_ref = audio_controller

    sofa_path = os.path.join(HERE, "mit_kemar_normal_pinna.sofa")
    audio_controller.load_hrtf(sofa_path)

    _ws_thread, _ws_stop = None, lambda: None
    try:
        _ws_thread, _ws_stop = start_server_in_thread(lambda text: say(text, True))
    except Exception as _e:
        logger.error(f"Failed to start NVDA WS/HTTP bridge: {_e}")

    register_details_callbacks(_on_vehicle_details_received, _on_vehicle_selector_state)

    if cfg.get("launch_beamng", False):
        try:
            import subprocess
            result = subprocess.run(
                ["tasklist", "/FI", "IMAGENAME eq BeamNG.drive.x64.exe", "/NH"],
                capture_output=True, text=True, timeout=5
            )
            if "BeamNG.drive.x64.exe" in result.stdout:
                logger.info("BeamNG.drive is already running, skipping launch.")
            else:
                os.startfile("steam://rungameid/284160")
                logger.info("Launched BeamNG.drive via Steam.")
        except Exception as e:
            logger.warning(f"Failed to launch BeamNG.drive: {e}")

    install_hotkeys(audio_controller)
    ui_thread = threading.Thread(target=ui_listener, args=(STOP,), daemon=True)
    ui_thread.start()

    scanner_thread = threading.Thread(target=scanner_listener, args=(audio_controller, STOP), daemon=True)
    scanner_thread.start()

    obstacle_thread = threading.Thread(target=obstacle_listener, args=(audio_controller, STOP), daemon=True)
    obstacle_thread.start()

    camera_thread = threading.Thread(target=camera_listener, args=(audio_controller, STOP), daemon=True)
    camera_thread.start()

    try:
        telemetry_loop(audio_controller=audio_controller, port=4444, stop_event=STOP)
    finally:
        STOP.set()
        audio_controller.stop()
        if _ws_stop: _ws_stop()


if __name__ == "__main__":
    main()

# --- END OF beamtel.py ---