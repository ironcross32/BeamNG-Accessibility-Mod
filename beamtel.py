import math
import socket
import struct
import time
import threading
import queue
import logging
import re
from nvda_ws_speaker import (
    start_server_in_thread,
    register_dom_dump_callback,
    register_loading_state_callback,
    register_settings_request_callback,
    broadcast,
    bnvda_debug_enabled,
)
import signal
import os
import json
import sys
import ctypes
import ctypes.wintypes
from collections import deque

import wx

from bnh_logger import get_logger, LOG_FILENAME
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

# ---------- DOM Dump Logger ----------
DOM_DUMP_PATH = os.path.join(CONFIG_DIR, "dom_dump.log")

# ---------- Speech Logger ----------
SPEECH_LOG_PATH = os.path.join(CONFIG_DIR, "speech.log")
speech_logging_active = False


def _on_dom_dump_received(lines):
    """Write DOM dump lines to a dedicated log file."""
    try:
        with open(DOM_DUMP_PATH, "w", encoding="utf-8") as f:
            for line in lines:
                f.write(line + "\n")
        logger.info(f"DOM dump written to {DOM_DUMP_PATH} ({len(lines)} lines)")
    except Exception as e:
        logger.error(f"Failed to write DOM dump: {e}")


def _handle_sigint(signum, frame):
    STOP.set()


signal.signal(signal.SIGINT, _handle_sigint)
if hasattr(signal, "SIGBREAK"):
    signal.signal(signal.SIGBREAK, _handle_sigint)

# =========================
#  Defaults & Config
# =========================
DEFAULT_CONFIG = {
    # "auto" lets Prism pick by backend priority; otherwise a Prism backend
    # name ("NVDA", "JAWS", "SAPI", "OneCore", ...). Rate and volume are
    # percentages and only apply to backends that report supporting them.
    "speech_backend": "auto",
    "speech_voice_name": "",
    "speech_rate": 50,
    "speech_volume": 100,
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
    "lowspeed_stop_tone_level_dbfs": -16.0,
    "slip_tone_level_dbfs": -18.0,
    "telemetry_protocol": "extended",
    "compass_click_interval": 15,
    "compass_highlight_enabled": True,
    "compass_highlight_nth_click": 6,  # MODIFIED
    "hrtf_enabled": True,
    "hrtf_front_emphasis_db": -6.0,
    "hrtf_distance_gain_db": 0.0,
    "follow_default_audio_device": True,
    "preferred_device_name": "",
    "audio_poll_interval_sec": 2.0,
    "obstacle_detection_enabled": False,
    "obstacle_buzz_volume_db": -12.0,
    "obstacle_max_range_m": 20,
    "obstacle_warning_range_m": 15,
    "road_beep_volume_db": -14.0,
    "placement_ping_volume_db": -12.0,
    "launch_beamng": False,
    "announce_turn_signals": True,
    "announce_speed": True,
    "speed_announce_interval": 25,
    "announce_gear": True,
    "scanner_distance_callout_enabled": False,
    "scanner_distance_callout_interval": 10,
    "scanner_steer_tone_enabled": True,
    "scanner_base_freq_hz": 1000.0,
    "scanner_pitch_offset_oct": 1.0,
    "ui_nav_hold_suppression": True,
    # Loader implement (WL-40 bucket / forks). Inert on every other vehicle.
    "implement_tones_enabled": True,
    "implement_ground_tone_dbfs": -16.0,
    "implement_tilt_tone_dbfs": -20.0,
    "implement_proximity_speech": True,
    "dock_tones_enabled": True,
    "dock_tone_dbfs": -18.0,
    "ai_describer_provider": "gemini",
    # Gemini's key/model keep their original names so existing configs migrate
    # for free; every other provider is namespaced.
    "ai_describer_api_key": "",
    "ai_describer_model": "models/gemini-3-flash-preview",
    "ai_describer_openai_api_key": "",
    "ai_describer_openai_model": "gpt-5.6-terra",
    "ai_describer_openai_base_url": "https://api.openai.com/v1",
    "ai_describer_openai_reasoning_effort": "low",
    "ai_describer_openai_detail": "auto",
    "ai_describer_disable_ui_toggle": False,
}

# =========================
#  Speech & Buffer
# =========================
import speech

SPEECH_BUFFER = deque(maxlen=100)

# Loading lifecycle state is driven by the UI's official screen-cover state.
_loading_active = False
_loading_settling = False
_loading_focus_text = ""
_loading_pending_vehicle = ""
_loading_generation = 0
_telemetry_baseline_pending = False
_loading_lock = threading.Lock()


def _normalize_speech_value(value, source="python"):
    """Return a scalar speech value, suppressing ambiguous containers."""
    original = value
    while isinstance(value, (list, dict)):
        try:
            logger.warning(
                "[LUA_TABLE_SPEECH] source=%s count=%d contents=%r",
                source,
                len(value),
                value,
            )
        except Exception:
            pass
        if len(value) != 1:
            return None
        value = next(iter(value.values())) if isinstance(value, dict) else value[0]
    if not isinstance(value, (str, int, float, bool)):
        if original is not None:
            logger.warning(
                "[LUA_TABLE_SPEECH] source=%s unsupported_type=%s",
                source,
                type(value).__name__,
            )
        return None
    text = str(value)
    if re.match(r"^table:\s*0x[0-9a-f]+$", text.strip(), re.IGNORECASE):
        logger.warning(
            "[LUA_TABLE_SPEECH] source=%s collapsed_pointer=%r contents_lost_upstream",
            source,
            text,
        )
        return None
    return text


def say(text: str, interrupt: bool = True, exclude_from_buffer: bool = False, source="python"):
    normalized = _normalize_speech_value(text, source)
    if normalized is None:
        return
    t = normalized.strip()
    if not t:
        return

    with _loading_lock:
        suppress_for_loading = _loading_active or _loading_settling
    if suppress_for_loading and not _command_context and source != "loading_lifecycle":
        logger.info("Suppressed speech during loading: source=%s text=%r", source, t)
        return

    if _command_context:
        logger.info(f"Speech output: '{t}'")

    # Background threads must not cut through a protected vehicle-name
    # announcement. Command context (user-triggered) always interrupts normally,
    # as does UI navigation speech -- see SPEECH_PROTECT_S.
    if (
        interrupt
        and not _command_context
        and source != "ui_bridge"
        and time.monotonic() < _speech_protected_until
    ):
        interrupt = False

    if speech_logging_active:
        try:
            import datetime

            with open(SPEECH_LOG_PATH, "a", encoding="utf-8") as f:
                f.write(
                    f"[{datetime.datetime.now().strftime('%H:%M:%S.%f')[:-3]}] {t}\n"
                )
        except Exception:
            pass

    if not exclude_from_buffer:
        SPEECH_BUFFER.append(t)

    speech.speak(t, bool(interrupt))


def stop_speech():
    speech.stop()


def _finish_loading_settle(generation):
    global _loading_settling, _loading_pending_vehicle, _speech_protected_until
    with _loading_lock:
        if generation != _loading_generation or _loading_active:
            return
        vehicle = _loading_pending_vehicle.strip()
        focus = _loading_focus_text.strip()
        _loading_pending_vehicle = ""
        _loading_settling = False
    ready_text = vehicle or focus or "Ready"
    _speech_protected_until = time.monotonic() + SPEECH_PROTECT_S
    logger.info("Loading lifecycle ready: generation=%d cue=%r", generation, ready_text)
    say(ready_text, exclude_from_buffer=True, source="loading_lifecycle")


def _on_loading_state_changed(active, focus_text=""):
    global _loading_active, _loading_settling, _loading_focus_text
    global _loading_pending_vehicle, _loading_generation, _telemetry_baseline_pending
    active = bool(active)
    with _loading_lock:
        if active == _loading_active and (active or _loading_settling):
            return
        _loading_generation += 1
        generation = _loading_generation
        _loading_focus_text = (focus_text or "").strip()
        if active:
            _loading_active = True
            _loading_settling = False
            _loading_pending_vehicle = ""
            _telemetry_baseline_pending = True
        else:
            _loading_active = False
            _loading_settling = True
    logger.info(
        "Loading lifecycle changed: active=%s generation=%d focus=%r",
        active,
        generation,
        focus_text,
    )
    if active:
        stop_speech()
        say("Loading", exclude_from_buffer=True, source="loading_lifecycle")
    else:
        timer = threading.Timer(1.0, _finish_loading_settle, args=(generation,))
        timer.daemon = True
        timer.start()


def _write_config(path, cfg):
    """Write the config atomically.

    beamtel.py and configurator.py share this file, and beamtel polls it once a
    second to hot-reload. `open(path, "w")` truncates in place, so a reader could
    land on a half-written file, fail to parse it, and take load_config's
    corruption branch -- which renames the real config to .bak and replaces it
    with defaults. A truncating write is also unrecoverable if the process dies
    partway through. Writing to a temp file and renaming means a reader always
    sees either the whole old file or the whole new one.
    """
    directory = os.path.dirname(path) or "."
    try:
        os.makedirs(directory, exist_ok=True)
    except OSError:
        pass
    tmp = "%s.%d.tmp" % (path, os.getpid())
    try:
        with open(tmp, "w", encoding="utf-8") as f:
            json.dump(cfg, f, indent=2, ensure_ascii=False)
            f.flush()
            os.fsync(f.fileno())
        # os.replace is atomic on Windows as well, but Windows refuses a rename
        # onto a file another process currently has open, so this can lose a
        # race with a reader. That is transient -- back off and retry rather
        # than dropping the user's settings on the floor.
        delay = 0.02
        for attempt in range(10):
            try:
                os.replace(tmp, path)
                return
            except PermissionError:
                if attempt == 9:
                    raise
                time.sleep(delay)
                delay = min(delay * 2, 0.25)
    finally:
        try:
            if os.path.exists(tmp):
                os.remove(tmp)
        except OSError:
            pass


def _read_config_raw(path):
    """Return (parsed, os_error). Retries briefly on transient OS errors.

    A sharing violation, or a read arriving mid-rename, is not corruption. The
    caller must keep the two apart: the corruption path destroys user settings,
    so it must never fire just because the file was momentarily unavailable.
    """
    last_os_error = None
    for attempt in range(5):
        try:
            with open(path, "r", encoding="utf-8") as f:
                return json.load(f), None
        except OSError as e:
            last_os_error = e
            time.sleep(0.05)
    return None, last_os_error


def load_config():
    if not os.path.isfile(CONFIG_PATH):
        _write_config(CONFIG_PATH, DEFAULT_CONFIG)
        return DEFAULT_CONFIG.copy()
    try:
        user, os_error = _read_config_raw(CONFIG_PATH)
        if os_error is not None:
            # Unreadable right now, most likely because configurator.py is
            # mid-save. Fall back to defaults for this call only and leave the
            # file alone -- it is almost certainly intact.
            logger.warning(
                f"Config temporarily unreadable ({os_error}); "
                "using defaults for this load without rewriting it."
            )
            return DEFAULT_CONFIG.copy()
        if not isinstance(user, dict):
            raise ValueError("Config root is not an object")
        if speech.migrate_config(user):
            logger.info("Migrated legacy SAPI speech keys to speech_* keys.")
            _write_config(CONFIG_PATH, user)
        merged = DEFAULT_CONFIG.copy()
        merged.update(user)
        merged["units"] = (
            "metric"
            if str(merged.get("units", "imperial")).lower().startswith("m")
            else "imperial"
        )
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

# Extended telemetry. bng_mod/ is a directory junction into the game install, so the Lua
# half can move (git checkout, mod update) without beamtel restarting. A hard equality test
# on the packet length would then reject every packet, leave `unpacked` at None, and take
# ALL extended telemetry down with it — no speed, no gear, no shift tone, no warning lights,
# and no error anywhere. So every historical layout stays decodable and short packets are
# padded with the sentinels the newer fields use for "absent".
EXT_FORMAT_V1 = "<H4sBx9fII28f"  # pre-implement (no bucket/fork block)
EXT_SIZE_V1 = struct.calcsize(EXT_FORMAT_V1)

EXT_FORMAT = "<H4sBx9fII36f"
EXT_SIZE = struct.calcsize(EXT_FORMAT)

# Values a v1 packet implies for the eight implement floats. Mirrors the sentinels in
# 796F6C6F313035.lua: flags 0 = nothing valid, -1 = no ground reading.
EXT_V1_IMPLEMENT_DEFAULTS = (0.0, -1.0, -1.0, 0.0, 0.0, 0.0, 0.0, 0.0)
_ext_version_warned = False

MS_MAGIC = b"BNG1"
MS_FORMAT = "<4s21f"
MS_SIZE = struct.calcsize(MS_FORMAT)

# Dashboard Light bitmasks
DL_SHIFT = 1 << 0
DL_FULLBEAM = 1 << 1
DL_HANDBRAKE = 1 << 2
DL_TC = 1 << 4
DL_SIGNAL_L = 1 << 5
DL_SIGNAL_R = 1 << 6
DL_CHECK = 1 << 7
DL_OILWARN = 1 << 8
DL_BATTERY = 1 << 9
DL_ABS = 1 << 10
DL_LOWBEAM = 1 << 11


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
            if not isinstance(payload, dict):
                _normalize_speech_value(payload, "ui_udp.payload")
                continue
            kind = payload.get("kind")
            d = payload.get("data") or {}
            if not isinstance(d, dict):
                _normalize_speech_value(d, f"ui_udp.{kind}.data")
                continue
            if kind == "toastr":
                title = _normalize_speech_value(d.get("title") or "", "ui_udp.toastr.title")
                msg = _normalize_speech_value(d.get("msg") or "", "ui_udp.toastr.msg")
                title = title.strip() if title is not None else ""
                msg = msg.strip() if msg is not None else ""
                text = f"{title} {msg}".strip() or msg or title
            elif kind == "message":
                text = _normalize_speech_value(d.get("msg") or "", "ui_udp.message.msg")
                text = text.strip() if text is not None else ""
            else:
                text = ""
            if text:
                say(text, source=f"ui_udp.{kind}")
    finally:
        try:
            sock.close()
        except Exception:
            pass


def scanner_listener(audio_controller, stop_event):
    """Listens for UDP packets from vehicleScanner.lua and passes data to the audio controller."""
    global \
        scan_mode_active, \
        coupler_dist_mode, \
        last_scanner_target_name, \
        last_scanner_distance, \
        last_scanner_approach_deg, \
        last_scanner_bearing, \
        _last_vehicle_switch_ts, \
        _speech_protected_until, \
        _loading_pending_vehicle
    SCANNER_PACKET_FORMAT = "<ff"
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
                    logger.info(
                        f"First UDP packet received from vehicle scanner (source: {addr})"
                    )
                    first_packet = False
                try:
                    text = data.decode("ascii").strip()
                    # Coupler tracking responses
                    if text.startswith("COUPLER_START:"):
                        tags = text[len("COUPLER_START:") :]
                        parts_tags = tags.split(",")
                        ptag = parts_tags[0] if len(parts_tags) > 0 else "unknown"
                        ttag = parts_tags[1] if len(parts_tags) > 1 else "unknown"
                        say(
                            f"Aligned. {ptag} and {ttag}. Tracking couplers. Reverse to couple.",
                            exclude_from_buffer=True,
                        )
                        audio_controller.set_coupler_tracking(True)
                    elif text.startswith("COUPLER_ATTACHED:"):
                        tags = text[len("COUPLER_ATTACHED:") :]
                        parts_tags = tags.split(",")
                        ptag = parts_tags[0] if len(parts_tags) > 0 else "unknown"
                        ttag = parts_tags[1] if len(parts_tags) > 1 else "unknown"
                        say(
                            f"Fifth wheel coupled. {ptag} and {ttag}.",
                            exclude_from_buffer=True,
                        )
                    elif text.startswith("COUPLER_FAIL:"):
                        reason = text[len("COUPLER_FAIL:") :]
                        say(reason, exclude_from_buffer=True)
                    elif text == "COUPLER_LOST":
                        say("Coupler tracking lost", exclude_from_buffer=True)
                        audio_controller.set_coupler_tracking(False)
                    elif text.startswith("COUPLER:"):
                        cparts = text[len("COUPLER:") :].split(",")
                        if len(cparts) >= 3:
                            c_bearing = float(cparts[0])
                            c_distance = float(cparts[1])
                            c_in_range = int(cparts[2]) != 0
                            audio_controller.update_coupler_target(
                                c_bearing, c_distance, c_in_range
                            )
                    # Coupler distance mode responses
                    elif text.startswith("COUPLER_DIST:"):
                        dparts = text[len("COUPLER_DIST:") :].split(",")
                        if len(dparts) >= 2:
                            horiz = float(dparts[0])
                            height = float(dparts[1])
                            if UNITS_MODE == "imperial":
                                horiz_val = horiz * 3.28084
                                height_val = abs(height) * 3.28084
                                unit = "feet"
                            else:
                                horiz_val = horiz
                                height_val = abs(height)
                                unit = "meters"
                            dist_str = f"{horiz_val:.1f} {unit}"
                            if height_val < 0.05:
                                height_str = "level"
                            elif height > 0:
                                height_str = f"{height_val:.1f} {unit} high"
                            else:
                                height_str = f"{height_val:.1f} {unit} low"
                            say(
                                f"{dist_str}, {height_str}",
                                interrupt=False,
                                exclude_from_buffer=True,
                            )
                    elif text.startswith("COUPLER_DIST_MODE:"):
                        mode = text[len("COUPLER_DIST_MODE:") :]
                        say(
                            f"Coupler distance {'on' if mode == 'ON' else 'off'}",
                            exclude_from_buffer=True,
                        )
                    elif text.startswith("COUPLER_DIST_FAIL:"):
                        reason = text[len("COUPLER_DIST_FAIL:") :]
                        say(reason, exclude_from_buffer=True)
                    elif text == "COUPLED_DETECTED:":
                        # Attach monitor detected the trailer is now physically coupled.
                        # Disable coupler distance mode and vehicle scanner.
                        parts_disabled = []
                        if coupler_dist_mode:
                            coupler_dist_mode = False
                            try:
                                s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
                                s.sendto(
                                    b"COUPLER_DIST", ("127.0.0.1", SCANNER_CMD_PORT)
                                )
                                s.close()
                            except Exception:
                                pass
                            parts_disabled.append("distance tracking")
                        if scan_mode_active:
                            scan_mode_active = False
                            last_scanner_target_name = ""
                            last_scanner_distance = float("inf")
                            last_scanner_approach_deg = 0.0
                            last_scanner_bearing = 0.0
                            try:
                                s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
                                s.sendto(b"OFF", ("127.0.0.1", SCANNER_CMD_PORT))
                                s.close()
                            except Exception:
                                pass
                            audio_controller.set_scan_mode(False)
                            audio_controller.set_coupler_tracking(False)
                            parts_disabled.append("scanner")
                        msg = "Coupled"
                        if parts_disabled:
                            msg += (
                                ". "
                                + " and ".join(parts_disabled).capitalize()
                                + " disabled"
                            )
                        say(msg, exclude_from_buffer=True)
                    elif text.startswith("ATTACH_MONITOR:"):
                        mode = text[len("ATTACH_MONITOR:") :]
                        say(
                            f"Attach monitor {'on' if mode == 'ON' else 'off'}",
                            exclude_from_buffer=True,
                        )
                    elif text.startswith("COUPLER_MODE:"):
                        state = text[len("COUPLER_MODE:") :]
                        say(
                            f"Couplers {'on' if state == 'ON' else 'off'}",
                            exclude_from_buffer=True,
                        )
                    # Alignment responses (legacy)
                    elif text == "ALIGN_OK":
                        say(
                            "Aligned to trailer. Reverse straight back and press L to couple.",
                            exclude_from_buffer=True,
                        )
                    elif text.startswith("ALIGN_FAIL:"):
                        reason = text[len("ALIGN_FAIL:") :]
                        say(f"Alignment failed. {reason}", exclude_from_buffer=True)
                    # AI control responses
                    elif text.startswith("AI_OK:"):
                        msg = text[len("AI_OK:") :]
                        logger.info(f"AI response: {msg}")
                    elif text.startswith("AI_ERR:"):
                        msg = text[len("AI_ERR:") :]
                        say(f"AI error. {msg}", exclude_from_buffer=True)
                    elif text.startswith("TARGET_NAME:"):
                        name = text[len("TARGET_NAME:") :]
                        with state_lock:
                            last_scanner_target_name = name
                        _speech_protected_until = time.monotonic() + SPEECH_PROTECT_S
                        say(name, exclude_from_buffer=True)
                    elif text.startswith("SWITCHED:"):
                        name = text[len("SWITCHED:") :]
                        with state_lock:
                            last_scanner_target_name = ""
                            last_scanner_distance = float("inf")
                        _last_vehicle_switch_ts = time.monotonic()
                        with _loading_lock:
                            loading_or_settling = _loading_active or _loading_settling
                            if loading_or_settling:
                                _loading_pending_vehicle = name
                        if loading_or_settling:
                            logger.info("Queued loading ready vehicle: %r", name)
                        else:
                            _speech_protected_until = time.monotonic() + SPEECH_PROTECT_S
                            say(name, exclude_from_buffer=True, source="vehicle_switch")
                    elif text.startswith("AI_STATUS_ALL:"):
                        _speak_status_all_response(text[len("AI_STATUS_ALL:") :])
                    elif text.startswith("AI_STATUS:"):
                        parts = text[len("AI_STATUS:") :].split(",")
                        if len(parts) >= 5:
                            mode, aggr, speed, avoid, lane = (
                                parts[0],
                                parts[1],
                                parts[2],
                                parts[3],
                                parts[4],
                            )
                            speed_str = (
                                "no limit" if speed == "off" else f"{speed} km/h"
                            )
                            say(
                                f"AI mode {mode}, aggression {aggr}, speed {speed_str}, avoid {avoid}, lane {lane}",
                                exclude_from_buffer=True,
                            )
                    else:
                        # Text protocol: "bearing,distance,approachDeg". distance is the
                        # surface GAP between the two vehicles, not centre-to-centre, so it
                        # legitimately reaches zero on contact — floor it so the "%.0f"
                        # readouts below can never say a negative distance.
                        parts = text.split(",")
                        if len(parts) >= 2:
                            bearing = float(parts[0])
                            distance = max(0.0, float(parts[1]))
                            approach = float(parts[2]) if len(parts) >= 3 else 0.0
                            with state_lock:
                                last_scanner_distance = distance
                                last_scanner_approach_deg = approach
                                last_scanner_bearing = bearing
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


def scanner_callout_thread_fn(stop_event):
    """Periodically announces scanner target distance and direction when enabled."""
    last_callout_ts = 0.0
    while not stop_event.is_set():
        stop_event.wait(timeout=1.0)
        if stop_event.is_set():
            break
        if not scanner_distance_callout_enabled:
            continue
        if not scan_mode_active:
            continue
        now = time.monotonic()
        interval = scanner_distance_callout_interval
        if now - last_callout_ts < interval:
            continue
        with state_lock:
            dist = last_scanner_distance
            brg = last_scanner_bearing
            tname = last_scanner_target_name
        if dist == float("inf"):
            continue
        last_callout_ts = now
        a = abs(brg)
        if a < 45:
            direction = "in front of you"
        elif a > 135:
            direction = "behind you"
        elif brg > 0:
            direction = "to the left of you"
        else:
            direction = "to the right of you"
        if UNITS_MODE == "imperial":
            dist_str = f"{dist * 3.28084:.0f} feet"
        else:
            dist_str = f"{dist:.0f} meters"
        prefix = f"{tname}, " if tname else ""
        say(f"{prefix}{dist_str}, {direction}", interrupt=False, exclude_from_buffer=True)


def obstacle_listener(audio_controller, stop_event):
    """Listens for UDP packets from obstacleDetector.lua and passes data to the audio controller."""
    # Text CSV format: "type,bearing,urgency,distance" or "0" for all clear

    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        sock.bind(("127.0.0.1", OBSTACLE_LISTEN_PORT))
        sock.settimeout(0.2)
        logger.info(
            f"Obstacle detector listener started on port {OBSTACLE_LISTEN_PORT}"
        )

        first_packet = True
        terrain_announced = {2: False, 3: False}

        while not stop_event.is_set():
            try:
                data, addr = sock.recvfrom(1024)
                if first_packet:
                    logger.info(
                        f"First UDP packet received from obstacle detector (source: {addr})"
                    )
                    first_packet = False

                text = data.decode("utf-8", errors="ignore").strip()
                if not text:
                    continue

                if text == "0":
                    audio_controller.update_static_obstacles([])
                    terrain_announced[2] = False
                    terrain_announced[3] = False
                    continue

                parts = text.split(",")
                if not parts:
                    continue

                pkt_type = int(parts[0])

                if pkt_type == 1:
                    # Static-obstacle packet: "1,n,b1,u1,d1,b2,u2,d2,...,bn,un,dn"
                    if len(parts) < 2:
                        continue
                    n = int(parts[1])
                    obstacles = []
                    for i in range(n):
                        base = 2 + i * 3
                        if base + 2 >= len(parts):
                            break
                        bearing = float(parts[base])
                        urgency = int(parts[base + 1])
                        distance = float(parts[base + 2])
                        obstacles.append((bearing, urgency, distance))
                    audio_controller.update_static_obstacles(obstacles)

                elif pkt_type in (2, 3) and len(parts) == 4:
                    bearing = float(parts[1])
                    urgency = int(parts[2])
                    distance = float(parts[3])
                    audio_controller.update_obstacle(
                        pkt_type, bearing, urgency, distance
                    )
                    if pkt_type == 2 and not terrain_announced[2]:
                        say("Drop-off ahead", exclude_from_buffer=True)
                        terrain_announced[2] = True
                        terrain_announced[3] = False
                    elif pkt_type == 3 and not terrain_announced[3]:
                        say("Steep hill ahead", exclude_from_buffer=True)
                        terrain_announced[3] = True
                        terrain_announced[2] = False

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


def road_listener(audio_controller, stop_event):
    """Listens for UDP packets from roadDetector.lua. Drives the off-road guidance beep."""
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        sock.bind(("127.0.0.1", ROAD_LISTEN_PORT))
        sock.settimeout(0.2)
        logger.info(f"Road detector listener started on port {ROAD_LISTEN_PORT}")

        first_packet = True
        last_state = None  # "DORMANT" / "ON_ROAD" / "OFF_ROAD"

        while not stop_event.is_set():
            try:
                data, addr = sock.recvfrom(1024)
                if first_packet:
                    logger.info(
                        f"First UDP packet received from road detector (source: {addr})"
                    )
                    first_packet = False

                text = data.decode("utf-8", errors="ignore").strip()
                if not text:
                    continue

                if text == "DORMANT":
                    audio_controller.update_road_state(on_road=True, bearing=0.0, distance=0.0)
                    if last_state != "DORMANT":
                        say("No roads detected on this map", exclude_from_buffer=True)
                        last_state = "DORMANT"
                elif text.startswith("ON_ROAD"):
                    parts = text.split(",")
                    audio_controller.update_road_state(on_road=True, bearing=0.0, distance=0.0)
                    if len(parts) >= 3:
                        try:
                            b_first = float(parts[1])
                            b_second = float(parts[2])
                            audio_controller.trigger_road_orientation_chime(b_first, b_second)
                        except ValueError:
                            pass
                    last_state = "ON_ROAD"
                elif text.startswith("OFF_ROAD"):
                    parts = text.split(",")
                    if len(parts) >= 3:
                        try:
                            bearing = float(parts[1])
                            distance = float(parts[2])
                            audio_controller.update_road_state(
                                on_road=False, bearing=bearing, distance=distance
                            )
                            last_state = "OFF_ROAD"
                        except ValueError:
                            pass
            except socket.timeout:
                continue
            except OSError:
                if stop_event.is_set():
                    break
                logger.error("Road listener socket error.")
    finally:
        try:
            sock.close()
        except Exception:
            pass
        logger.info("Road detector listener stopped.")


# Loader implement proximity speech. Distance hysteresis on its own is not enough: at the
# boundary a genuine relation flap would still re-announce, hence the per-transition dwell
# timers. Conversely a real change of circumstance — lowering the forks past a frame lip —
# crosses the relation margin decisively and holds, so it announces at once. That asymmetry
# is the whole requirement.
IMPL_PROX_ENTER_M = 3.0
IMPL_PROX_LEAVE_M = 4.5
IMPL_PROX_RELATION_HOLD = 0.40  # seconds a new relation must persist before it is spoken
IMPL_PROX_INSIDE_HOLD = 0.25
IMPL_PROX_LEAVE_HOLD = 0.30

# Docking readout thresholds. Both exist to stop the cane tap reading out noise: soft-body
# jitter moves the cutting edge by a couple of centimetres even on a parked machine, and a
# spoken "left 0.01 meters" is worse than "centred" because it invites a correction that
# cannot be made.
IMPL_DOCK_LEVEL_M = 0.05    # below this an axis is called level / centred rather than numbered
IMPL_DOCK_YAW_DEG = 8.0     # squareness is only worth saying once it would jam the tines

# Slam gate states -> the audio cue each fires. NONE is deliberately absent: leaving the
# gate is not itself an event worth marking, and a cue on every exit would fire constantly
# while manoeuvring around a yard.
_SLAM_CUES = {
    "CLEAR": "clear",
    "OVER": "over",
    "COMMITTED": "committed",
}

_IMPL_RELATION_PHRASE = {
    "ABOVE": "above it",
    "BELOW": "below it",
    "LEVEL": "level with it",
}


def _implement_word(part_name: str) -> str:
    """What to call the business end. 'Forks' and 'grapple' are worth saying by name."""
    low = (part_name or "").lower()
    if "fork" in low:
        return "Forks"
    if "grapple" in low:
        return "Grapple"
    return "Bucket"


def _band_name(kind: str, idx: int, count: int) -> str:
    """Name the docking reference band from its position in the stack.

    Named by ordinal rather than by absolute height because the mod reports world Z, which
    means nothing spoken aloud, and because what makes a band identifiable is where it sits
    relative to the others — the lowest void in a pallet is the pocket whether the pallet is
    on the ground or on a truck bed.
    """
    if kind == "GAP":
        if idx <= 1:
            return "underside"
        return "opening"
    if idx >= count:
        return "roof"
    if idx <= 1:
        return "base"
    return "body"


def _dock_phrase(dock: dict) -> str:
    """The cane tap: the whole docking picture in one utterance.

    Deliberately a single sentence fired on a keypress rather than anything continuous. A
    blind cane user taps when they want to know something; they do not receive an ambient
    three-dimensional field, which is precisely what made the obstacle detector unusable.
    """
    band = _band_name(dock["kind"], dock["idx"], dock["count"])
    thick = dock["hi_z"] - dock["lo_z"]
    bits = [f"{dock['impl_word']}, {dock['name']}"]

    ref = f"{band} {dock['idx']} of {dock['count']}"
    if dock["manual"]:
        ref += ", held"
    tv, tu = fmt_distance(max(0.0, thick))
    bits.append(f"{ref}, {tv} {tu} tall")

    vert = dock["vertical"]
    if abs(vert) < IMPL_DOCK_LEVEL_M:
        bits.append("level")
    else:
        vv, vu = fmt_distance(abs(vert))
        bits.append(f"{'raise' if vert > 0 else 'lower'} {vv} {vu}")

    lat = dock["lateral"]
    if abs(lat) < IMPL_DOCK_LEVEL_M:
        bits.append("centred")
    else:
        lv, lu = fmt_distance(abs(lat))
        bits.append(f"{'left' if lat > 0 else 'right'} {lv} {lu}")

    rv, ru = fmt_distance(max(0.0, dock["range"]))
    bits.append(f"range {rv} {ru}")

    # Squareness only matters at the moment of entry, so it is spoken when it is bad enough
    # to jam the tines and stays silent otherwise rather than becoming a fourth number.
    yaw = dock["yaw"]
    if abs(yaw) >= IMPL_DOCK_YAW_DEG:
        bits.append(f"face {abs(yaw):.0f} degrees {'left' if yaw > 0 else 'right'}")

    return ". ".join(bits)


def implement_listener(audio_controller, stop_event):
    """Listens for UDP packets from implementProximity.lua and speaks approach/leave events.

    Only ever announces the NEAREST object. The extension may report a different one from
    tick to tick in a crowded yard, and narrating all of them would be unusable.
    """
    global _implement_word_current, last_dock, last_dock_fail
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        sock.bind(("127.0.0.1", IMPLEMENT_LISTEN_PORT))
        sock.settimeout(0.2)
        logger.info(f"Implement proximity listener started on port {IMPLEMENT_LISTEN_PORT}")

        # Ask the mod to re-announce. Which implement is fitted is Python-side state that a
        # restart wipes, but the mod only sends the IMPLEMENT: line when the name CHANGES —
        # and that latch lives in Lua, where restarting beamtel cannot clear it. Without
        # this, restarting beamtel while the game keeps running leaves every implement
        # feature convinced no implement is fitted until the vehicle is reset. ON is already
        # exactly the right reset on the mod side, so it needs no new command.
        _send_implement_cmd("ON")

        impl_word = "Bucket"
        tracked = None  # name of the object currently being tracked
        tracked_relation = None
        tracked_inside = False
        # Pending transitions, each (value, first_seen_ts), so a flap has to persist.
        pending_relation = None
        pending_inside = None
        pending_leave = None

        while not stop_event.is_set():
            try:
                data, _addr = sock.recvfrom(2048)
                text = data.decode("utf-8", errors="ignore").strip()
                if not text:
                    continue
                now = time.time()

                if text.startswith("IMPLEMENT:"):
                    part = text.split(":", 1)[1]
                    impl_word = _implement_word(part)
                    _implement_word_current = (
                        "" if part.strip().upper() == "NONE" else impl_word
                    )
                    # A part swap invalidates whatever we were tracking — including the
                    # docking readout, whose band pick belonged to the old implement.
                    tracked = None
                    tracked_relation, tracked_inside = None, False
                    pending_relation = pending_inside = pending_leave = None
                    with state_lock:
                        last_dock = None
                        last_dock_fail = None
                    audio_controller.clear_dock_target()
                    continue

                # Docking lines are handled ahead of the proximity chain and never feed it:
                # they are a separate instrument with its own toggle, and the two would
                # otherwise fight over the same tracked/pending state machine.
                if text == "DOCKCLEAR":
                    with state_lock:
                        last_dock = None
                        last_dock_fail = None
                    audio_controller.clear_dock_target()
                    continue

                if text.startswith("DOCKFAIL:"):
                    # Not spoken unprompted — it would nag while manoeuvring. Held for the
                    # F9+I readout, which is where someone asks "why is this silent".
                    with state_lock:
                        last_dock = None
                        last_dock_fail = text[9:].strip()
                    audio_controller.clear_dock_target()
                    continue

                if text.startswith("SLAM:"):
                    # Sent only on a state change; the hysteresis that stops these
                    # chattering lives in the mod, next to the geometry it hysteresises.
                    state = text[5:].strip().upper()
                    cue = _SLAM_CUES.get(state)
                    if cue:
                        audio_controller.trigger_slam_cue(cue)
                        if announce_implement_proximity and state == "COMMITTED":
                            # Only the committed state is worth a word. The other two are
                            # waypoints you pass through on the way; speaking each one
                            # would talk over the manoeuvre it is describing.
                            say("Over it, clear", exclude_from_buffer=True)
                    continue

                if text.startswith("DOCK:"):
                    # rsplit so a target name containing a stray separator cannot shift the
                    # numeric fields, the same way the NEAR parse below does it.
                    fields = text[5:].rsplit(",", 10)
                    if len(fields) != 11:
                        continue
                    try:
                        parsed = {
                            "name": fields[0].strip(),
                            "range": float(fields[1]),
                            "lateral": float(fields[2]),
                            "vertical": float(fields[3]),
                            "idx": int(fields[4]),
                            "count": int(fields[5]),
                            "kind": fields[6].strip().upper(),
                            "lo_z": float(fields[7]),
                            "hi_z": float(fields[8]),
                            "yaw": float(fields[9]),
                            "manual": fields[10].strip() == "1",
                            "impl_word": impl_word,
                        }
                    except ValueError:
                        continue
                    with state_lock:
                        last_dock = parsed
                        last_dock_fail = None
                    audio_controller.update_dock_target(
                        parsed["range"], parsed["lateral"], parsed["vertical"]
                    )
                    continue

                if text == "CLEAR":
                    if tracked is not None:
                        if pending_leave is None:
                            pending_leave = now
                        elif now - pending_leave >= IMPL_PROX_LEAVE_HOLD:
                            say(f"Clear of {tracked}", exclude_from_buffer=True)
                            tracked = None
                            tracked_relation, tracked_inside = None, False
                            pending_relation = pending_inside = pending_leave = None
                    continue

                if not text.startswith("NEAR:"):
                    continue

                parts = text[5:].rsplit(",", 4)
                if len(parts) != 5:
                    continue
                name = parts[0].strip()
                try:
                    dist = float(parts[1])
                except ValueError:
                    continue
                relation = parts[2].strip().upper()
                inside = parts[3].strip() == "1"

                if not announce_implement_proximity:
                    continue

                if tracked is None:
                    if dist < IMPL_PROX_ENTER_M:
                        val, unit = fmt_distance(dist)
                        phrase = _IMPL_RELATION_PHRASE.get(relation, "")
                        say(
                            f"{impl_word} approaching {name}, {phrase}, {val} {unit}",
                            exclude_from_buffer=True,
                        )
                        tracked = name
                        tracked_relation, tracked_inside = relation, inside
                        pending_relation = pending_inside = pending_leave = None
                    continue

                if name != tracked:
                    # A nearer object took over. Announce the new one; no leave line for the
                    # old one, or every pass through a yard becomes a monologue.
                    if dist < IMPL_PROX_ENTER_M:
                        val, unit = fmt_distance(dist)
                        phrase = _IMPL_RELATION_PHRASE.get(relation, "")
                        say(
                            f"{impl_word} approaching {name}, {phrase}, {val} {unit}",
                            exclude_from_buffer=True,
                        )
                        tracked = name
                        tracked_relation, tracked_inside = relation, inside
                        pending_relation = pending_inside = pending_leave = None
                    continue

                # Still on the same object.
                if dist > IMPL_PROX_LEAVE_M:
                    if pending_leave is None:
                        pending_leave = now
                    elif now - pending_leave >= IMPL_PROX_LEAVE_HOLD:
                        say(f"Clear of {tracked}", exclude_from_buffer=True)
                        tracked = None
                        tracked_relation, tracked_inside = None, False
                        pending_relation = pending_inside = pending_leave = None
                    continue
                pending_leave = None

                if relation != tracked_relation:
                    if pending_relation is None or pending_relation[0] != relation:
                        pending_relation = (relation, now)
                    elif now - pending_relation[1] >= IMPL_PROX_RELATION_HOLD:
                        tracked_relation = relation
                        pending_relation = None
                        say(
                            f"{tracked}, now {_IMPL_RELATION_PHRASE.get(relation, relation)}",
                            exclude_from_buffer=True,
                        )
                else:
                    pending_relation = None

                if inside != tracked_inside:
                    if pending_inside is None or pending_inside[0] != inside:
                        pending_inside = (inside, now)
                    elif now - pending_inside[1] >= IMPL_PROX_INSIDE_HOLD:
                        tracked_inside = inside
                        pending_inside = None
                        if inside:
                            say(
                                f"{impl_word} under {tracked}", exclude_from_buffer=True
                            )
                        else:
                            say(
                                f"{impl_word} clear of {tracked}",
                                exclude_from_buffer=True,
                            )
                else:
                    pending_inside = None

            except socket.timeout:
                continue
            except OSError:
                if stop_event.is_set():
                    break
                logger.error("Implement proximity listener socket error.")
    finally:
        try:
            sock.close()
        except Exception:
            pass
        logger.info("Implement proximity listener stopped.")


def _send_implement_cmd(command):
    try:
        cmd_sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        cmd_sock.sendto(command.encode("utf-8"), ("127.0.0.1", IMPLEMENT_CMD_PORT))
        cmd_sock.close()
    except Exception as e:
        logger.error(f"Failed to send implement proximity command via UDP: {e}")


def _send_scanner_cmd(command):
    try:
        cmd_sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        cmd_sock.sendto(command.encode("utf-8"), ("127.0.0.1", SCANNER_CMD_PORT))
        cmd_sock.close()
    except Exception as e:
        logger.error(f"Failed to send scanner command via UDP: {e}")


def _push_gear_direction(gear):
    """Tell the scanner which end of the vehicle is the business end.

    Reversing is not just "the target is behind me" — the rear bumper becomes the thing
    that will hit something, so the scanner's contact set has to move to the rear node
    band and its bearing has to be measured against the direction of travel. On a loader
    it also has to stop measuring from the bucket, which in reverse is the far end.

    Pushed from here rather than polled cross-VM in Lua because this side already decodes
    gear out of the telemetry struct, and it is sent only on change, so the 60 Hz loop
    does not turn into a 60 Hz send. The Lua side ages it and falls back to velocity if
    these stop arriving, so an older mod or a stopped beamtel degrades rather than sticks.
    """
    reverse = str(gear or "").strip().upper().startswith("R")
    _send_scanner_cmd("GEAR:R" if reverse else "GEAR:F")


def camera_listener(audio_controller, stop_event):
    """Listens for UDP packets from cameraInfo.lua and drives camera compass clicks."""
    global \
        cam_yaw_deg, \
        cam_pitch_deg, \
        cam_agl, \
        cam_agl_valid, \
        cam_last_packet_ts, \
        cam_veh_bearing, \
        cam_veh_distance, \
        cam_is_free
    global \
        cam_last_click_heading_deg, \
        cam_compass_click_counter, \
        cam_last_announced_compass_idx, \
        cam_last_compass_ts
    global \
        compass_highlight_enabled, \
        compass_highlight_nth_click, \
        compass_click_interval_deg

    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        sock.bind(("127.0.0.1", CAMERA_LISTEN_PORT))
        sock.settimeout(0.2)
        logger.info(f"Camera info listener started on port {CAMERA_LISTEN_PORT}")

        first_packet = True
        while not stop_event.is_set():
            try:
                data, addr = sock.recvfrom(1024)
                if first_packet:
                    logger.info(
                        f"First UDP packet received from camera info (source: {addr})"
                    )
                    first_packet = False

                # Diagnostic replies answer a one-shot query, so they arrive whether or
                # not the live feed is switched on and are handled before that gate.
                try:
                    if data.startswith(b"DIAG:"):
                        body = data.decode("ascii", errors="replace").strip()[len("DIAG:"):]
                        heights, _, detail = body.partition("|")
                        logger.info(f"CAMERA DIAG: {detail or heights}")
                        say(_format_camera_diag(heights), exclude_from_buffer=True)
                        continue
                except Exception as e:
                    logger.error(f"Camera diagnostic reply failed: {e}")
                    continue

                if not free_cam_active:
                    continue
                try:
                    text = data.decode("ascii").strip()
                    parts = text.split(",")
                    if len(parts) >= 6:
                        yaw = float(parts[0])
                        pitch = float(parts[1])
                        agl = float(parts[2])
                        bearing = float(parts[3])
                        distance = float(parts[4])
                        is_free = int(parts[5]) == 1
                        # 7th field is optional: an older Lua mod sends six, and its AGL
                        # was always presented as valid.
                        agl_valid = len(parts) < 7 or int(parts[6]) == 1

                        now = time.time()
                        with state_lock:
                            cam_yaw_deg = yaw
                            cam_pitch_deg = pitch
                            cam_agl = agl
                            cam_agl_valid = agl_valid
                            cam_last_packet_ts = now
                            cam_veh_bearing = bearing
                            cam_veh_distance = distance
                            cam_is_free = is_free

                            # Camera compass click logic (same as vehicle compass)
                            heading = yaw
                            delta_heading = heading - cam_last_click_heading_deg
                            if delta_heading > 180.0:
                                delta_heading -= 360.0
                            if delta_heading < -180.0:
                                delta_heading += 360.0

                            if abs(delta_heading) >= compass_click_interval_deg:
                                cam_compass_click_counter += 1
                                pitch_mult = 1.0 + 0.25 * math.cos(
                                    math.radians(heading)
                                )

                                if (
                                    compass_highlight_enabled
                                    and cam_compass_click_counter
                                    >= compass_highlight_nth_click
                                ):
                                    audio_controller.trigger_cam_compass_highlight(
                                        heading, pitch_mult * 1.5
                                    )
                                    cam_compass_click_counter = 0
                                else:
                                    audio_controller.trigger_cam_compass_click(
                                        heading, pitch_mult
                                    )

                                num_intervals = round(
                                    heading / compass_click_interval_deg
                                )
                                cam_last_click_heading_deg = (
                                    num_intervals * compass_click_interval_deg
                                ) % 360.0

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
                                    in_switch_quiet = (
                                        time.monotonic() - _last_vehicle_switch_ts
                                    ) < VEHICLE_SWITCH_QUIET_S
                                    if (
                                        now - cam_last_compass_ts
                                    ) >= compass_min_interval and not in_switch_quiet:
                                        say(
                                            f"Camera {COMPASS_NAMES[current_compass_idx]}",
                                            exclude_from_buffer=True,
                                        )
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


def nodegrab_listener(audio_controller, stop_event):
    """Listens for UDP packets from nodeGrabberAccessible.lua — node hover data and snap coords."""
    global nodegrab_last_cid, nodegrab_scanning
    import ctypes

    VK_CONTROL = 0x11
    GetAsyncKeyState = ctypes.windll.user32.GetAsyncKeyState
    last_ctrl_state = False
    snap_sent_this_press = False

    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        sock.bind(("127.0.0.1", NODEGRAB_LISTEN_PORT))
        sock.settimeout(0.05)  # faster polling for CTRL state
        logger.info(f"Node grabber listener started on port {NODEGRAB_LISTEN_PORT}")

        first_packet = True
        last_announce_ts = 0.0
        DEBOUNCE_SEC = 0.15

        while not stop_event.is_set():
            # Poll CTRL key state via Win32 API (more reliable than keyboard hooks)
            if nodegrab_mode_active:
                ctrl_down = (GetAsyncKeyState(VK_CONTROL) & 0x8000) != 0
                if ctrl_down and not last_ctrl_state:
                    # CTRL just pressed
                    nodegrab_scanning = True
                    _send_nodegrab_cmd("SCAN_ON")
                    _send_nodegrab_cmd("SNAP")
                    snap_sent_this_press = True
                elif not ctrl_down and last_ctrl_state:
                    # CTRL just released
                    nodegrab_scanning = False
                    _send_nodegrab_cmd("SCAN_OFF")
                    snap_sent_this_press = False
                last_ctrl_state = ctrl_down
            else:
                last_ctrl_state = False

            try:
                data, addr = sock.recvfrom(2048)
                if first_packet:
                    logger.info(f"First UDP packet from node grabber (source: {addr})")
                    first_packet = False

                text = data.decode("utf-8", errors="ignore").strip()
                if not text:
                    continue

                now = time.time()

                if text == "NODE_CLEAR":
                    if nodegrab_last_cid != -1:
                        audio_controller.trigger_node_hover_beep(0.5, reverse=True)
                        nodegrab_last_cid = -1
                    continue

                # Ignore node hover data if Python knows CTRL isn't held (stale Lua scan)
                if not nodegrab_scanning and text.startswith("NODE:"):
                    continue

                if text.startswith("SNAP:"):
                    # SNAP:<x>,<y>,<viewW>,<viewH>,<cid>,<name>,<location>,<groups>,<heightNorm>
                    parts = text[5:].split(",", 8)
                    if len(parts) >= 4:
                        sx, sy = int(parts[0]), int(parts[1])
                        vw, vh = int(parts[2]), int(parts[3])
                        warp_cursor_to_viewport(sx, sy, vw, vh)
                        # Also announce the snapped node if we have info
                        if len(parts) >= 9:
                            cid = int(parts[4])
                            name = parts[5]
                            location = parts[6]
                            groups = parts[7]
                            height_norm = float(parts[8])
                            audio_controller.trigger_node_hover_beep(height_norm)
                            desc = f"{name}, {location}"
                            if groups:
                                desc += f", {groups}"
                            say(desc, exclude_from_buffer=True)
                    continue

                if text.startswith("NODE:"):
                    # NODE:<cid>,<name>,<location>,<groups>,<heightNorm>
                    parts = text[5:].split(",", 4)
                    if len(parts) == 5:
                        cid = int(parts[0])
                        name = parts[1]
                        location = parts[2]
                        groups = parts[3]
                        height_norm = float(parts[4])

                        nodegrab_last_cid = cid

                        # Trigger audio beep immediately
                        audio_controller.trigger_node_hover_beep(height_norm)

                        # Debounce speech
                        if now - last_announce_ts >= DEBOUNCE_SEC:
                            last_announce_ts = now
                            desc = f"{name}, {location}"
                            if groups:
                                desc += f", {groups}"
                            say(desc, exclude_from_buffer=True)

            except socket.timeout:
                continue
            except OSError:
                if stop_event.is_set():
                    break
                logger.error("Node grabber listener socket error.")
    finally:
        try:
            sock.close()
        except Exception:
            pass
        logger.info("Node grabber listener stopped.")


def clickspot_listener(audio_controller, stop_event):
    """Listens for UDP packets from clickspotAccessible.lua — trigger hover data and list."""
    import ctypes

    global clickspot_trigger_list, clickspot_last_hover_id

    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        sock.bind(("127.0.0.1", CLICKSPOT_LISTEN_PORT))
        sock.settimeout(0.2)
        logger.info(f"Clickspot listener started on port {CLICKSPOT_LISTEN_PORT}")

        first_packet = True
        last_announce_ts = 0.0
        DEBOUNCE_SEC = 0.15

        while not stop_event.is_set():
            try:
                data, addr = sock.recvfrom(2048)
                if first_packet:
                    logger.info(
                        f"First UDP packet from clickspot detector (source: {addr})"
                    )
                    first_packet = False

                text = data.decode("utf-8", errors="ignore").strip()
                if not text:
                    continue

                now = time.time()

                if text == "HOVER_CLEAR":
                    if clickspot_last_hover_id != -1:
                        audio_controller.trigger_clickspot_beep(reverse=True)
                    clickspot_last_hover_id = -1
                    continue

                if text.startswith("TRIGGER_LIST:"):
                    count = int(text[13:])
                    clickspot_trigger_list = []
                    if count == 0:
                        say(
                            "No clickspots found on this vehicle",
                            exclude_from_buffer=True,
                        )
                    else:
                        say(
                            f"{count} clickspot{'s' if count != 1 else ''} found",
                            exclude_from_buffer=True,
                        )
                    continue

                if text.startswith("TRIGGER:"):
                    # TRIGGER:<cacheIndex>,<triggerId>,<displayName>
                    parts = text[8:].split(",", 2)
                    if len(parts) == 3:
                        cache_idx = int(parts[0])
                        trigger_id = int(parts[1])
                        display_name = parts[2]
                        clickspot_trigger_list.append(
                            (cache_idx, trigger_id, display_name)
                        )
                    continue

                if text.startswith("HOVER:"):
                    # HOVER:<triggerId>,<displayName>
                    parts = text[6:].split(",", 1)
                    if len(parts) == 2:
                        trigger_id = int(parts[0])
                        display_name = parts[1]
                        clickspot_last_hover_id = trigger_id

                        # Trigger forward beep (mouse moved onto clickspot)
                        audio_controller.trigger_clickspot_beep(reverse=False)

                        # Debounce speech
                        if now - last_announce_ts >= DEBOUNCE_SEC:
                            last_announce_ts = now
                            say(display_name, exclude_from_buffer=True)
                    continue

                if text.startswith("SNAP_OK:"):
                    # SNAP_OK:<x>,<y>,<viewW>,<viewH>,<triggerId>,<displayName>
                    parts = text[8:].split(",", 5)
                    if len(parts) >= 6:
                        sx, sy = int(parts[0]), int(parts[1])
                        vw, vh = int(parts[2]), int(parts[3])
                        display_name = parts[5]
                        warp_cursor_to_viewport(sx, sy, vw, vh)
                        # No beep on menu-initiated jump — speech only
                        say(f"Jumped to {display_name}", exclude_from_buffer=True)
                    continue

                if text.startswith("SNAP_FAIL:"):
                    reason = text[10:]
                    say(f"Cannot jump, {reason}", exclude_from_buffer=True)
                    continue

            except socket.timeout:
                continue
            except OSError:
                if stop_event.is_set():
                    break
                logger.error("Clickspot listener socket error.")
    finally:
        try:
            sock.close()
        except Exception:
            pass
        logger.info("Clickspot listener stopped.")


def vehicle_bindings_listener(stop_event):
    """Listens for the current vehicle's special-control bindings from vehicleBindings.lua.

    Deliberately silent: the mod pushes a fresh list on every vehicle load, and
    announcing that would talk over the spawn. The list is only ever spoken when
    the user opens the browser (F9 then B).
    """
    global _vehicle_bindings_list, _vehicle_bindings_vehicle
    global _vehicle_bindings_building, _vehicle_bindings_building_name

    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        sock.bind(("127.0.0.1", VEHICLE_BINDINGS_LISTEN_PORT))
        sock.settimeout(0.2)
        logger.info(
            f"Vehicle bindings listener started on port {VEHICLE_BINDINGS_LISTEN_PORT}"
        )

        while not stop_event.is_set():
            try:
                data, _addr = sock.recvfrom(2048)
                text = data.decode("utf-8", errors="ignore").strip()
                if not text:
                    continue

                if text.startswith("BINDINGS_BEGIN:"):
                    # BINDINGS_BEGIN:<count>,<vehicleName>
                    parts = text[15:].split(",", 1)
                    _vehicle_bindings_building = []
                    _vehicle_bindings_building_name = (
                        parts[1].strip() if len(parts) == 2 else ""
                    )
                    continue

                if text.startswith("BINDING:"):
                    # BINDING:<cacheIndex>,<line>  — line may contain commas
                    parts = text[8:].split(",", 1)
                    if len(parts) == 2 and _vehicle_bindings_building is not None:
                        try:
                            _vehicle_bindings_building.append(
                                (int(parts[0]), parts[1])
                            )
                        except ValueError:
                            pass
                    continue

                if text == "BINDINGS_END":
                    # Swap in whole, so a browser opened mid-rebuild never sees a
                    # torn list.
                    if _vehicle_bindings_building is not None:
                        _vehicle_bindings_list = _vehicle_bindings_building
                        _vehicle_bindings_vehicle = _vehicle_bindings_building_name
                        _vehicle_bindings_building = None
                        logger.info(
                            f"Vehicle bindings updated: {len(_vehicle_bindings_list)} "
                            f"for {_vehicle_bindings_vehicle!r}"
                        )
                    continue

            except socket.timeout:
                continue
            except OSError:
                if stop_event.is_set():
                    break
                logger.error("Vehicle bindings listener socket error.")
    finally:
        try:
            sock.close()
        except Exception:
            pass
        logger.info("Vehicle bindings listener stopped.")


def slot_listener(stop_event):
    """Listens for SLOTS: messages from vehicleSlots.lua and updates _vehicle_slots."""
    global _vehicle_slots, _selected_slots, _target_slot
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        sock.bind(("127.0.0.1", SLOT_LISTEN_PORT))
        sock.settimeout(0.2)
        logger.info(f"Vehicle slot listener started on port {SLOT_LISTEN_PORT}")
        while not stop_event.is_set():
            try:
                data, _ = sock.recvfrom(4096)
                text = data.decode("utf-8").strip()
                if not text.startswith("SLOTS:"):
                    continue
                payload = text[6:]
                new_slots = {}
                if payload:
                    for part in payload.split("|"):
                        pieces = part.split(",", 2)
                        if len(pieces) == 3:
                            try:
                                slot = int(pieces[0])
                                vid = int(pieces[1])
                                name = pieces[2]
                                new_slots[slot] = {"id": vid, "name": name}
                            except ValueError:
                                pass
                with _slots_lock:
                    _vehicle_slots = new_slots
                    # Purge selections/target for slots that no longer exist.
                    _selected_slots = {s for s in _selected_slots if s in new_slots}
                    if _target_slot is not None and _target_slot not in new_slots:
                        _target_slot = None
            except socket.timeout:
                continue
            except OSError:
                if stop_event.is_set():
                    break
                logger.error("Slot listener socket error.")
    finally:
        try:
            sock.close()
        except Exception:
            pass
        logger.info("Slot listener stopped.")


# =========================
#  Units & formatting
# =========================
UNITS_MODE = "imperial"
oil_chime_enabled = True
announce_turn_signals = True
announce_speed = True
speed_announce_interval = 25
announce_gear = True
# Held-navigation speech coalescing. Enforced entirely in the UI JS runtime; this
# side only owns the value and pushes it across the bridge.
ui_nav_hold_suppression = True
ai_describer_provider = "gemini"
# Every provider's describer settings, keyed by config name. Holding them in one
# dict keeps _ai_describe_worker free of a per-provider if-chain: it asks the
# registry which keys the active provider uses and looks them up here.
ai_describer_settings = {}
ai_describer_disable_ui_toggle = False
MPH_PER_MS = 2.2369362920544
KMH_PER_MS = 3.6
PSI_PER_BAR = 14.503773773
FEET_PER_M = 3.280839895


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


def fmt_distance(metres: float):
    """Short distances (implement height, clearance) as a (value, unit) pair.

    TODO: seven older sites still inline the 3.28084 conversion and format straight into a
    sentence at their own precision — the coupler distance mode (~line 560), the scanner
    callout (~760), _format_camera_diag (~2680), camera altitude (~3050), vehicle distance
    from camera (~3120), the F9+D scanner readout (~3240) and the F9+W waypoint readout
    (~3270). Folding those in here would change spoken output in six unrelated features, so
    it is deliberately left for its own change.
    """
    if UNITS_MODE == "metric":
        return round(float(metres), 2), "meters"
    return round(float(metres) * FEET_PER_M, 1), "feet"


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
    1: "first",
    2: "second",
    3: "third",
    4: "fourth",
    5: "fifth",
    6: "sixth",
    7: "seventh",
    8: "eighth",
    9: "ninth",
    10: "tenth",
    11: "eleventh",
    12: "twelfth",
}


def gear_to_phrase(gear_byte: int) -> str:
    if gear_byte == 0:
        return "reverse"
    if gear_byte == 1:
        return "neutral"
    if gear_byte >= 2:
        n = gear_byte - 1
        return _ORDINAL_WORDS.get(n, f"{n}th")
    return "unknown"


def extended_gear_to_phrase(gear_str: str) -> str:
    s = (gear_str or "").strip().upper()
    if not s:
        return "unknown"

    if s == "P":
        return "park"
    if s == "D":
        return "drive"
    if s == "R":
        return "reverse"
    if s == "N":
        return "neutral"

    if len(s) > 1 and s[1:].isdigit():
        num_part = s[1:]
        if s.startswith("S"):
            return f"sport {num_part}"
        if s.startswith("M"):
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
    if speed_ms < 0:
        return 0
    interval = max(1, speed_announce_interval)
    if UNITS_MODE == "metric":
        kph = speed_ms * KMH_PER_MS
        return int(kph // interval)
    else:
        mph = speed_ms * MPH_PER_MS
        return int(mph // interval)


# =========================
#  Bearing helpers (MotionSim yaw -> 8-way compass)
# =========================
COMPASS_NAMES = [
    "north",
    "northeast",
    "east",
    "southeast",
    "south",
    "southwest",
    "west",
    "northwest",
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
compass_highlight_enabled = True
compass_highlight_nth_click = 6
compass_click_interval_deg = 15.0

# Pneumatics
last_air_pressure = 0.0
last_air_pressure_max = 0.0

# Implement (loader bucket / forks). All zero-or-sentinel on any vehicle without hydraulic
# implement cylinders, which is what keeps the status metrics hidden and the tones silent
# everywhere except a machine like the WL-40. See IMPL_FLAG_* for the bitmask.
IMPL_FLAG_PRESENT = 1  # an implement part was resolved
IMPL_FLAG_ARTIC_VALID = 2  # articulationDeg means something (0 = straight, not "no data")
IMPL_FLAG_GROUND_VALID = 4  # a surface was found below the implement
IMPL_FLAG_JACKING = 8  # implement is against the ground and lifting the machine
IMPL_FLAG_DETACHED = 16  # implement has broken off its coupler
last_implement_flags = 0.0
last_implement_edge_height = -1.0  # m, cutting edge / tine tips above the surface below
last_implement_min_clearance = -1.0  # m, nearest of the five sample points to the surface
last_implement_tilt_deg = 0.0  # degrees from level, positive = curled back / up
last_implement_tilt_percent = 0.0  # 0..1 of tilt ram travel
last_implement_lift = 0.0  # m the machine has been jacked up off its wheels
last_implement_activity = 0.0  # 0..1, drives the tone fade gate
last_articulation_deg = 0.0  # degrees of frame articulation, positive = LEFT

# Expanded Telemetry
last_clutch_temp = 0.0
last_g_lat = 0.0
last_g_lon = 0.0
(
    last_tire_pressure_fl,
    last_tire_pressure_fr,
    last_tire_pressure_rl,
    last_tire_pressure_rr,
) = 0.0, 0.0, 0.0, 0.0
last_tire_temp_fl, last_tire_temp_fr, last_tire_temp_rl, last_tire_temp_rr = (
    0.0,
    0.0,
    0.0,
    0.0,
)
last_brake_temp_fl, last_brake_temp_fr, last_brake_temp_rl, last_brake_temp_rr = (
    0.0,
    0.0,
    0.0,
    0.0,
)
last_signal_left_input, last_signal_right_input, last_hazard_enabled = (
    False,
    False,
    False,
)
last_lightbar = -1
last_fog = -1

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

REVERSE = 0  # OutGauge gear byte: 0=R, 1=N, 2+=1st
NEUTRAL = 1
last_gear_byte, last_gear_str = None, None

# State shared with audio module
pedal_tones_active = False
scan_mode_active = False
coupler_dist_mode = False
obstacle_mode_active = False
road_mode_active = False
_command_context = False  # True only while processing an F9/F10 command keystroke

# NEW: Heading Guidance State
heading_guidance_active = False
heading_guidance_target = 0.0

# NEW: Drift Detection State
drift_mode_active = False
last_drift_check_ts = 0.0
drift_baseline_heading = 0.0
drift_alert_active = False
drift_pan_direction = 0.0  # -1.0 Left, 1.0 Right

# Drift filter/gate tuning. The yaw rate is sampled at the full telemetry rate
# (~60 Hz) and low-pass filtered rather than decimated, so the signal is
# responsive without inheriting the per-packet quantisation noise.
DRIFT_SMOOTH_TAU = 0.25    # seconds; one-pole time constant on the yaw rate
DRIFT_ON_RATE = 0.8        # deg/s; rate that arms the alert
DRIFT_OFF_RATE = 0.35      # deg/s; rate that releases it (hysteresis)
DRIFT_RELEASE_HOLD = 0.3   # seconds below DRIFT_OFF_RATE before releasing
DRIFT_PAN_MIN_RATE = 0.2   # deg/s; below this the pan side is held, not flipped

# NEW: Low Speed Detection State
low_speed_mode_active = False

# Wheel slip (lockup / wheelspin) detection state
slip_mode_active = False

# Coordinate Guidance State
coord_guidance_active = False
marked_coord_x = None  # None means no waypoint has been set yet
marked_coord_y = None
coord_target_bearing = 0.0  # cached bearing to waypoint, recalculated every ~1 s
_last_coord_bearing_ts = 0.0  # timestamp of last bearing recalculation
_ls_prev_speed_ms = 0.0
_ls_prev_speed_ts = 0.0
_ls_steady_start_ts = 0.0

# --- Ground-truth motion state, derived from the MotionSim (BNG1) packet ---
# MotionSim carries obj:getVelocityXYZ() in world space, which is immune to wheel
# lockup and wheelspin; the OutGauge/Extended speed field is drivetrain-derived and
# lies whenever the tires are not rolling with the road.
last_ground_speed_ms = 0.0
_ms_last_rx_ts = 0.0
_ls_accel_smooth = 0.0  # signed longitudinal accel, m/s^2 (+ speeding up)
_ls_ms_prev_speed_ms = 0.0
_ls_ms_prev_ts = 0.0
_ls_stopped = False
_ls_was_moving = False
_ls_slip_smooth = 0.0
_ls_slip_since_ts = 0.0
_ls_slip_prev_ts = 0.0

# Tuning for the ground-truth derivation. Audio-side response curves live in audio.py.
LS_ACCEL_TAU_S = 0.15  # EMA time constant for longitudinal accel
LS_ACCEL_MIN_DT = 0.05  # don't differentiate over shorter gaps; denominator gets noisy
LS_ACCEL_MAX_DT = 1.0  # longer gap => stream broke (vehicle reload); re-baseline
LS_ACCEL_DEADBAND = 0.25  # |accel| below this counts as "holding steady"
LS_STEADY_HOLD_S = 1.5  # how long inside the deadband before clicks are suppressed
LS_STOP_ENTER_MS = 0.20  # ground speed at/below which we call it a standstill
LS_STOP_EXIT_MS = 0.50  # and above which we call it moving again (hysteresis)
LS_STOP_TONE_MIN_MS = 1.0  # only chime if we were actually rolling beforehand
LS_MS_STALE_S = 1.0  # no MotionSim packet for this long => fall back to wheel speed
SLIP_TAU_S = 0.10
SLIP_MIN_GROUND_MS = 2.0
SLIP_ABS_THRESHOLD_MS = 1.5
SLIP_REL_THRESHOLD = 0.25
SLIP_SUSTAIN_S = 0.15

# Low speed diagnostics ride on window.BNVDA_DEBUG, set from the accessible console's
# CEF/UI JS context (`window.BNVDA_DEBUG = true`). An environment variable is no use
# here: beamtel is already running by the time the problem shows up, and the whole
# reason BNVDA_DEBUG exists is that the flag has to be flippable mid-session.
# Logging additionally requires low speed detection to be on, so enabling debug for
# something else doesn't fill the log with telemetry.
LOWSPEED_DIAG_INTERVAL_S = 0.25
_ls_diag_last_ts = 0.0

# Scanner live data (updated by scanner_listener)
last_scanner_target_name = ""
last_scanner_distance = float("inf")
last_scanner_approach_deg = 0.0
last_scanner_bearing = 0.0

# Scanner periodic distance callout settings (applied via _apply_live_config)
scanner_distance_callout_enabled = False
scanner_distance_callout_interval = 10

# Loader implement proximity speech (applied via _apply_live_config). The tones themselves
# are configured inside audio.py's apply_config; only the speech gate lives here.
announce_implement_proximity = True
# "Bucket" / "Forks" / "Grapple" once the mod reports one, else "" for every other vehicle.
# Read by toggle_scan_mode, which needs to say which end of the machine it is aiming from.
_implement_word_current = ""

# Docking instrument. dock_mode_active is the user-facing toggle; last_dock is the most
# recent readout from the mod, or None when there is nothing in range. Written by
# implement_listener under state_lock and read by the F9+I cane tap and the audio callback.
dock_mode_active = False
last_dock = None
# Why the instrument has nothing to say, when it has nothing to say. Held rather than
# spoken, and read out by F9+I on request.
last_dock_fail = None

# Monotonic timestamp of the last vehicle-switch announcement. The camera
# compass listener checks this and skips its own callout briefly afterwards
# so a heading-crossing tick can't talk over the (typically longer)
# "Switched to ETK 800-Series 856ti, blue" line.
_last_vehicle_switch_ts = 0.0
VEHICLE_SWITCH_QUIET_S = 2.5

# When a vehicle name is announced (SWITCHED or TARGET_NAME), background speech
# (camera compass, gear, obstacles) is forced to interrupt=False so it queues
# behind the name rather than cutting it off.
#
# UI screen-reader speech (source="ui_bridge") is exempt: it is direct user
# navigation, not a background tick, and demoting it made the part selector feel
# sluggish -- every row you landed on queued behind the previous one, because
# applying a part reloads the vehicle and re-arms this window.
_speech_protected_until = 0.0
SPEECH_PROTECT_S = 3.0

# Camera Info State
free_cam_active = False
cam_yaw_deg = 0.0
cam_pitch_deg = 0.0
cam_agl = 0.0
# False when the Lua side found no ground under the camera and cam_agl is therefore an
# absolute height, not a height above ground.
cam_agl_valid = True
# When the last camera packet arrived. Everything above goes stale the moment the feed
# stops, and the readouts must not speak a frozen number as if it were current.
cam_last_packet_ts = 0.0
# The feed runs at 10 Hz, so anything older than this means it has actually stopped
# rather than just missed a tick.
CAMERA_STALE_SEC = 1.5
cam_veh_bearing = 0.0
cam_veh_distance = -1.0
cam_is_free = False
cam_last_click_heading_deg = 0.0
cam_compass_click_counter = 0
cam_last_announced_compass_idx = -1
cam_last_compass_ts = 0.0

# Node Grabber State
nodegrab_mode_active = False
nodegrab_scanning = False
nodegrab_last_cid = -1
nodegrab_strength = 50  # tracked locally (0-100)
_nodegrab_scroll_hook = None

# Clickspot Detection State
clickspot_mode_active = False
clickspot_trigger_list = []  # list of (cache_index, trigger_id, display_name)
clickspot_last_hover_id = -1

# Vehicle-Specific Bindings State
# Rebuilt silently on every vehicle load — nothing here is ever announced; the
# list only becomes audible when the user opens the browser with F9 then B.
_vehicle_bindings_list = []  # list of (cache_index, line)
_vehicle_bindings_vehicle = ""
_vehicle_bindings_building = None  # accumulator between BEGIN and END
_vehicle_bindings_building_name = ""

# Generic Virtual Browser State
_vbrowser_lines = []
_vbrowser_index = 0
_vbrowser_hooks = []
_vbrowser_active = False
_vbrowser_on_enter = None
_vbrowser_entry_data = []

audio_controller_ref = None

# Vehicle spawner module reference (set in main() after import)
_vehicle_spawner = None

# =========================
#  Keyboard (suppressed layered commands)
# =========================
try:
    import keyboard

    KEYBOARD_OK = True
except Exception:
    KEYBOARD_OK = False
    logger.warning(
        "keyboard module unavailable – 'pip install keyboard' and run as Administrator for key suppression."
    )

next_key_timer = None
command_timeout_sec = 4.0
_capture_mods = {"ctrl": False, "shift": False, "alt": False}

_input_help_mode = False

# ---------------------------------------------------------------------------
#  Hook dispatch worker
#
#  A `keyboard` hook installed with suppress=True runs inside the Win32
#  WH_KEYBOARD_LL callback, not on a worker thread: _winkeyboard.py's
#  low_level_keyboard_handler invokes the listener's direct_callback inline and
#  uses the value it returns to choose between `return -1` (swallow the key) and
#  CallNextHookEx (let it through). Windows allows that callback only
#  LowLevelHooksTimeout milliseconds (300 by default, HKCU\Control Panel\Desktop)
#  before it stops waiting, honours the key as if the hook had never run, and
#  after repeated offences unregisters the hook outright. The visible symptom is
#  keys a layer meant to swallow arriving in BeamNG instead.
#
#  Everything the command handlers do is over that budget or can be: speech goes
#  out to a screen reader, several handlers take state_lock (held by the telemetry loop
#  and the audio callback), and teardown calls keyboard.unhook.
#
#  So the hook callbacks below do two things only — classify the key and decide
#  suppression — then queue the actual work here. Nothing on the queue can stall
#  the hook, because the hook never waits for it.
# ---------------------------------------------------------------------------
_kb_queue = queue.Queue()
_kb_worker_thread = None


class _SynthKeyEvent:
    """Stand-in for keyboard.KeyboardEvent, carrying what the handlers read.

    The real event object belongs to the hook callback that is about to return;
    handlers now run later, on the worker, so they get a snapshot instead.
    """

    __slots__ = ("name", "event_type")

    def __init__(self, name, event_type="down"):
        self.name = name
        self.event_type = event_type


def _kb_dispatch(fn, *args):
    """Queue fn(*args) to run on the keyboard worker. Safe inside a hook."""
    try:
        _kb_queue.put_nowait((fn, args))
    except Exception:
        logger.exception("Failed to queue keyboard command")


def _kb_enqueue(handler):
    """Wrap a suppressed-hook handler so its body runs on the worker.

    The wrapper returns None, which keyboard reads as "suppress this key" — the
    same verdict every handler wrapped here already returned. Mirrors
    vehicle_spawner._enqueue, which solved this for the F11 modal.
    """

    def wrapper(event):
        _kb_dispatch(handler, event)

    return wrapper


def _kb_worker_loop():
    while True:
        fn, args = _kb_queue.get()
        try:
            fn(*args)
        except Exception:
            # A handler that raised must not take the worker down with it, or
            # every later keystroke would be silently dropped.
            logger.exception("Keyboard command handler raised")
        finally:
            _kb_queue.task_done()


def _start_kb_worker():
    global _kb_worker_thread
    if _kb_worker_thread is not None:
        return
    _kb_worker_thread = threading.Thread(
        target=_kb_worker_loop, name="kb-dispatch", daemon=True
    )
    _kb_worker_thread.start()


# ---------------------------------------------------------------------------
#  Shared layer hook (F9 and F10)
#
#  Both layers share one blocking hook rather than installing their own. Two
#  reasons: opening F10 while F9 was still open used to leave F9's hook behind,
#  and removing one entry from keyboard's blocking_hooks list while
#  direct_callback iterates it (`all(hook(event) for hook in ...)`) makes the
#  generator skip the next entry, so that hook's key escapes to the game. With a
#  single entry there is nothing left to skip.
#
#  The hook exists only while a layer is open. With no layer open there is no
#  hook installed at all, so every key reaches BeamNG untouched.
# ---------------------------------------------------------------------------
_kb_layer = None  # None | "f9" | "f10"
_kb_layer_hook = None
_kb_layer_release_hook = None

# Modifier state as the hook sees it, live. Snapshotted per key-down and handed
# to the worker, because the user may well have released the modifier by the
# time the command runs.
_live_mods = {"ctrl": False, "shift": False, "alt": False}

_MOD_ALIASES = {
    "ctrl": "ctrl",
    "control": "ctrl",
    "left ctrl": "ctrl",
    "right ctrl": "ctrl",
    "shift": "shift",
    "left shift": "shift",
    "right shift": "shift",
    "alt": "alt",
    "left alt": "alt",
    "right alt": "alt",
}


def _kb_layer_press(event):
    """WH_KEYBOARD_LL context. Returns True to pass the key on, False to eat it.

    Keep this cheap: no speech, no locks, no I/O, no unhooking.
    """
    if event.event_type != "down":
        return True  # key-ups reach the game, as they always have
    layer = _kb_layer
    if layer is None:
        # Closed between this key arriving and teardown finishing.
        return True
    name = (event.name or "").lower()
    mod = _MOD_ALIASES.get(name)
    if mod is not None:
        _live_mods[mod] = True
        return False
    if name == layer:  # the F9/F10 that opened the layer, repeating
        return False
    _kb_dispatch(_kb_run_layer_command, layer, name, dict(_live_mods))
    return False


def _kb_layer_release(event):
    """Installed without suppress=True, so this runs on keyboard's own
    processing thread and is not on the hook timeout clock."""
    if event.event_type != "up":
        return
    mod = _MOD_ALIASES.get((event.name or "").lower())
    if mod is not None:
        _live_mods[mod] = False


def _kb_run_layer_command(layer, name, mods):
    """Worker context: restore the modifier snapshot, then run the layer body."""
    if layer != _kb_layer:
        return  # layer closed or switched while this sat in the queue
    if layer == "f9":
        _capture_mods.update(mods)
        _on_next_key_press(_SynthKeyEvent(name), audio_controller_ref)
    else:
        _ai_mods.update(mods)
        _on_ai_key_press(_SynthKeyEvent(name))


def _kb_open_layer(which):
    """Hand the keyboard to `which` layer, installing the hook if needed."""
    global _kb_layer, _kb_layer_hook, _kb_layer_release_hook
    _kb_layer = which
    _live_mods["ctrl"] = _live_mods["shift"] = _live_mods["alt"] = False
    if _kb_layer_hook is not None:
        return  # already installed; retargeted above
    try:
        _kb_layer_hook = keyboard.hook(_kb_layer_press, suppress=True)
        _kb_layer_release_hook = keyboard.on_release(_kb_layer_release)
    except Exception:
        logger.exception("Failed to install layer keyboard hook")
        _kb_layer = None


def _kb_close_layer():
    """Release the keyboard back to the game. Never called from inside the hook."""
    global _kb_layer, _kb_layer_hook, _kb_layer_release_hook
    _kb_layer = None
    press, release = _kb_layer_hook, _kb_layer_release_hook
    _kb_layer_hook = _kb_layer_release_hook = None
    for h in (press, release):
        if h is None:
            continue
        try:
            keyboard.unhook(h)
        except Exception:
            pass
    _live_mods["ctrl"] = _live_mods["shift"] = _live_mods["alt"] = False


# F9 command descriptions keyed by (name, ctrl, shift, alt)
_F9_HELP = {
    ("s", False, False, False): "Speak speed",
    ("r", False, False, False): "Speak RPM",
    ("r", False, True, False): "Speak redline RPM",
    ("g", False, False, False): "Speak gear",
    ("f", False, False, False): "Speak fuel",
    ("e", False, False, False): "Speak engine temperature",
    ("e", False, True, False): "Dump electrics to log",
    ("p", False, True, False): "Dump powertrain to log",
    ("h", False, True, False): "Dump hydros to log",
    ("t", False, False, False): "Speak turbo pressure",
    ("t", False, True, False): "Speak max turbo pressure",
    ("o", False, False, False): "Speak oil temperature",
    ("a", False, False, False): "Speak attitude, roll and pitch",
    ("c", False, False, False): "Speak coordinates",
    ("c", False, True, False): "Mark waypoint at current position",
    ("c", False, False, True): "Speak marked waypoint",
    ("w", False, False, False): "Distance and bearing to waypoint",
    ("d", False, False, False): "Scanner distance and orientation",
    ("d", False, True, False): "Scanner relative bearing to target",
    ("d", True, False, False): "Toggle drift detection",
    ("d", True, True, False): "Toggle coupler distance callouts",
    ("m", False, False, False): "Damage report",
    ("s", True, False, False): "Toggle status mode",
    ("b", False, False, False): "Browse vehicle bindings",
    ("b", True, False, False): "Toggle buffer mode",
    ("c", True, False, False): "Toggle pedal tones",
    ("v", True, False, False): "Toggle vehicle scanner",
    (
        "v",
        False,
        True,
        False,
    ): "Align to trailer coupler, start coupler tracking, and start attach monitor",
    ("o", True, False, False): "Toggle obstacle detection",
    ("r", True, False, False): "Toggle road detection",
    ("tab", False, False, False): "Next scanner target",
    ("tab", False, True, False): "Previous scanner target",
    ("tab", True, False, False): "Closest scanner target",
    ("h", True, False, False): "Toggle heading guidance",
    ("g", True, False, False): "Toggle coordinate guidance",
    ("l", True, False, False): "DOM dump",
    ("l", True, True, False): "Toggle low speed detection",
    ("k", True, False, False): "Toggle wheel slip detection",
    ("s", True, True, False): "Toggle speech logger",
    ("f", False, False, True): "Toggle camera info",
    ("h", False, False, True): "Camera heading",
    ("a", False, False, True): "Camera altitude",
    ("p", False, False, True): "Camera pitch",
    ("v", False, False, True): "Vehicle bearing from camera",
    ("d", False, False, True): "Vehicle distance from camera",
    ("h", False, False, False): "Speak heading",
    ("p", False, False, False): "Speak air pressure",
    ("u", False, False, False): "Switch between imperial and metric",
    ("space", False, False, False): "Activate context action",
    ("n", True, False, False): "Toggle accessible node grabber",
    ("c", True, True, False): "Toggle clickspot detection",
    ("c", True, True, True): "Browse clickspots",
    ("i", False, False, False): "Implement alignment readout",
    ("i", False, True, False): "Cycle implement alignment reference band",
    ("i", True, False, False): "Toggle implement docking instrument",
}

# F10 (AI) command descriptions
_F10_HELP = {
    ("d", False, False, False): "Disable AI",
    ("t", False, False, False): "Traffic mode",
    ("r", False, False, False): "Random mode",
    ("s", False, False, False): "Stop AI",
    ("c", False, False, False): "Chase mode",
    ("f", False, False, False): "Follow mode",
    ("e", False, False, False): "Flee mode",
    ("a", False, False, False): "Cycle avoid cars",
    ("a", True, False, False): "Select all vehicles",
    ("n", True, False, False): "Clear selection",
    ("l", False, False, False): "Toggle lane driving",
    ("+", False, False, False): "Increase speed limit",
    ("=", False, False, False): "Increase speed limit",
    ("-", False, False, False): "Decrease speed limit",
    ("0", False, False, False): "Clear speed limit",
    ("space", False, False, False): "Describe scene (AI)",
}
# Aggression keys 1-9 (unmodified)
for _k, _v in [
    ("1", "0.2"),
    ("2", "0.4"),
    ("3", "0.7"),
    ("4", "0.9"),
    ("5", "1.0"),
    ("6", "1.2"),
    ("7", "1.5"),
    ("8", "1.8"),
    ("9", "2.0"),
]:
    _F10_HELP[(_k, False, False, False)] = f"Aggression {_v}"

# Vehicle slot selection (SHIFT+digit) and target (CTRL+digit)
for _k in "1234567890":
    _slot_label = "10" if _k == "0" else _k
    _F10_HELP[(_k, False, True, False)] = f"Toggle slot {_slot_label} selection"
    _F10_HELP[(_k, True, False, False)] = f"Set slot {_slot_label} as AI target"

# Preset configurations (ALT+digit)
_F10_HELP[("1", False, False, True)] = "Preset: Motorcade (each follows the one ahead)"
_F10_HELP[("2", False, False, True)] = "Preset: Gang-up (all chase vehicle 1)"
_F10_HELP[("3", False, False, True)] = "Preset: Tour of Destruction (chase chain)"
_F10_HELP[("4", False, False, True)] = (
    "Preset: Tour of Destruction Spectator (you watch)"
)
_F10_HELP[("5", False, False, True)] = "Preset: Police mode (all follow CTRL target)"


def _clear_next_key_hook(speak_exit: bool):
    global next_key_timer, _command_context, _input_help_mode
    _command_context = False
    _input_help_mode = False
    if _kb_layer == "f9":
        _kb_close_layer()
    if next_key_timer is not None:
        try:
            next_key_timer.cancel()
        except Exception:
            pass
    next_key_timer = None
    _capture_mods["ctrl"] = _capture_mods["shift"] = _capture_mods["alt"] = False
    if speak_exit:
        say("Exit", exclude_from_buffer=True)


# Data structure for Status Mode
STATUS_METRICS = [
    {
        "label": "Heading",
        "getValue": lambda: (f"{last_heading:.1f}", "degrees"),
        "isAvailable": lambda: True,
    },
    {
        "label": "Speed",
        "getValue": lambda: fmt_speed(last_speed_ms),
        "isAvailable": lambda: True,
    },
    {
        "label": "RPM",
        "getValue": lambda: (int(round(last_rpm)), "RPM"),
        "isAvailable": lambda: True,
    },
    {
        "label": "Gear",
        "getValue": lambda: (
            gear_to_phrase(last_gear_byte)
            if protocol_mode == "outgauge"
            else extended_gear_to_phrase(last_gear_str or ""),
            "",
        ),
        "isAvailable": lambda: True,
    },
    {
        "label": "Fuel",
        "getValue": lambda: (f"{int(round(last_fuel * 100))}", "percent"),
        "isAvailable": lambda: True,
    },
    {
        "label": "Engine Temperature",
        "getValue": lambda: fmt_temp_c_or_f(last_engtemp),
        "isAvailable": lambda: True,
    },
    {
        "label": "Oil Temperature",
        "getValue": lambda: fmt_temp_c_or_f(last_oiltemp),
        "isAvailable": lambda: True,
    },
    # {'label': 'Clutch Temperature', 'getValue': lambda: fmt_temp_c_or_f(last_clutch_temp), 'isAvailable': lambda: protocol_mode == 'extended'},
    {
        "label": "Turbo Pressure",
        "getValue": lambda: fmt_turbo(last_turbo),
        "isAvailable": lambda: True,
    },
    # {'label': 'Oil Pressure', 'getValue': lambda: fmt_turbo(last_oil_pressure), 'isAvailable': lambda: protocol_mode == 'extended'},
    # {'label': 'Air System Pressure', 'getValue': lambda: fmt_pressure(last_air_pressure), 'isAvailable': lambda: protocol_mode == 'extended' and last_air_pressure_max > 0},
    # {'label': 'Lateral G-Force', 'getValue': lambda: (f"{last_g_lat:.2f}", "G"), 'isAvailable': lambda: protocol_mode == 'extended'},
    # {'label': 'Longitudinal G-Force', 'getValue': lambda: (f"{last_g_lon:.2f}", "G"), 'isAvailable': lambda: protocol_mode == 'extended'},
    {
        "label": "Tire Pressures (FL, FR, RL, RR)",
        "getValue": lambda: (
            f"{fmt_pressure(last_tire_pressure_fl)[0]}, {fmt_pressure(last_tire_pressure_fr)[0]}, {fmt_pressure(last_tire_pressure_rl)[0]}, {fmt_pressure(last_tire_pressure_rr)[0]}",
            "psi" if UNITS_MODE == "imperial" else "bar",
        ),
        "isAvailable": lambda: protocol_mode == "extended",
    },
    # Loader implement (WL-40 bucket / forks). These three only appear when the mod actually
    # resolved an implement or hydraulic steering — implementFlags is 0 on every ordinary
    # vehicle, so a Pickup's status list is unchanged.
    {
        "label": "Implement Height",
        "getValue": lambda: fmt_distance(max(0.0, last_implement_edge_height)),
        "isAvailable": lambda: bool(int(last_implement_flags) & IMPL_FLAG_PRESENT)
        and bool(int(last_implement_flags) & IMPL_FLAG_GROUND_VALID),
    },
    {
        "label": "Implement Tilt",
        "getValue": lambda: (
            f"{abs(last_implement_tilt_deg):.0f}",
            "degrees "
            + (
                "level"
                if abs(last_implement_tilt_deg) < 1.0
                else ("back" if last_implement_tilt_deg > 0 else "forward")
            ),
        ),
        "isAvailable": lambda: bool(int(last_implement_flags) & IMPL_FLAG_PRESENT),
    },
    {
        "label": "Frame Articulation",
        "getValue": lambda: (
            f"{abs(last_articulation_deg):.0f}",
            "degrees "
            + (
                "straight"
                if abs(last_articulation_deg) < 1.0
                else ("left" if last_articulation_deg > 0 else "right")
            ),
        ),
        "isAvailable": lambda: bool(int(last_implement_flags) & IMPL_FLAG_ARTIC_VALID),
    },
    # {'label': 'Tire Temps (FL, FR, RL, RR)', 'getValue': lambda: (f"{fmt_temp_c_or_f(last_tire_temp_fl)[0]}, {fmt_temp_c_or_f(last_tire_temp_fr)[0]}, {fmt_temp_c_or_f(last_tire_temp_rl)[0]}, {fmt_temp_c_or_f(last_tire_temp_rr)[0]}", "F" if UNITS_MODE == 'imperial' else "C"), 'isAvailable': lambda: protocol_mode == 'extended'},
    # {'label': 'Brake Temps (FL, FR, RL, RR)', 'getValue': lambda: (f"{fmt_temp_c_or_f(last_brake_temp_fl)[0]}, {fmt_temp_c_or_f(last_brake_temp_fr)[0]}, {fmt_temp_c_or_f(last_brake_temp_rl)[0]}, {fmt_temp_c_or_f(last_brake_temp_rr)[0]}", "F" if UNITS_MODE == 'imperial' else "C"), 'isAvailable': lambda: protocol_mode == 'extended'},
]



def _hook_suppressed(key, handler):
    """Suppressing KEY_DOWN hook that can always be torn down again.

    `keyboard` indexes every hook by its key NAME as well as by its callback
    (`_hooks[callback] = _hooks[key] = _hooks[remove] = remove`). Two suppressing hooks on
    the same key therefore clobber each other's `_hooks[key]` entry, and removing the FIRST
    one deletes the SECOND one's entry. The second teardown then raises KeyError from inside
    the library's own remove function — *before* it takes the callback out of
    `blocking_keys` — so the key stays swallowed for the life of the process with no handle
    left that can release it. That is what happened when the vehicle spawner and status mode
    both owned the arrows: closing both left the arrows dead.

    So keep our own reference to the callback and purge it directly on teardown.
    """
    cb = lambda e: e.event_type == keyboard.KEY_UP or handler(e)  # noqa: E731
    remove = keyboard.hook_key(key, cb, suppress=True)
    return {"key": key, "cb": cb, "remove": remove}


def _unhook_suppressed(records):
    """Undo `_hook_suppressed` records, tolerating already-corrupt library bookkeeping."""
    for rec in records:
        try:
            keyboard.unhook(rec["remove"])
        except Exception:
            pass
        # Belt and braces for the aliasing described above: whatever the library managed to
        # do, make sure this callback is no longer suppressing the key.
        try:
            blocking = keyboard._listener.blocking_keys
            for scan_code in keyboard.key_to_scan_codes(rec["key"]):
                lst = blocking.get(scan_code)
                while lst and rec["cb"] in lst:
                    lst.remove(rec["cb"])
        except Exception:
            pass
    records.clear()


def release_arrow_owners(reason=""):
    """Hand the arrow keys back, whoever currently holds them.

    Status mode and the virtual browser both take the arrows exclusively, and the vehicle
    spawner takes them too. Without one place that releases all of them, opening one on top
    of another leaves two owners on the same key — see `_hook_suppressed`.
    """
    close_virtual_browser(speak_exit=False)
    if status_mode_active:
        toggle_status_mode()
    elif status_arrow_hooks:
        # Hooks outliving the mode flag: release them anyway rather than leaving the arrows
        # eaten with no mode to toggle off.
        _unhook_suppressed(status_arrow_hooks)
        if reason:
            logger.warning(f"Released orphaned status-mode arrow hooks ({reason})")


def on_status_arrow_press(event):
    global current_status_metric_index
    # Don't steal the arrows while a virtual browser (vehicle selector, clickspot list,
    # bindings list) owns them. This used to read a `_vehicle_selector_open` name that was
    # never defined anywhere, so every arrow press in status mode raised NameError before
    # reaching a single metric — status mode looked dead.
    if _vbrowser_active:
        return
    if not status_mode_active:
        return
    with state_lock:
        available_metrics = [m for m in STATUS_METRICS if m["isAvailable"]()]
        if not available_metrics:
            say("No status metrics available", exclude_from_buffer=True)
            return
        current_status_metric_index %= len(available_metrics)
        if event.name == "down":
            current_status_metric_index = (current_status_metric_index + 1) % len(
                available_metrics
            )
        elif event.name == "up":
            current_status_metric_index = (
                current_status_metric_index - 1 + len(available_metrics)
            ) % len(available_metrics)
        metric = available_metrics[current_status_metric_index]
        value, unit = metric["getValue"]()
        say(
            f"{metric['label']}, {value} {unit}"
            if event.name in ("up", "down")
            else f"{value} {unit}",
            exclude_from_buffer=True,
        )


def toggle_status_mode():
    global status_mode_active, current_status_metric_index, status_arrow_hooks
    status_mode_active = not status_mode_active
    if status_mode_active:
        current_status_metric_index = 0
        say("Status mode on", exclude_from_buffer=True)
        try:
            for key in ["up", "down", "left", "right"]:
                status_arrow_hooks.append(
                    _hook_suppressed(key, _kb_enqueue(on_status_arrow_press))
                )
        except Exception as e:
            logger.error(f"Failed to hook status mode keys: {e}")
    else:
        say("Status mode off", exclude_from_buffer=True)
        _unhook_suppressed(status_arrow_hooks)


def on_buffer_nav_press(event):
    global current_buffer_index
    if not buffer_mode_active:
        return
    with state_lock:
        if not SPEECH_BUFFER:
            say("Buffer empty", exclude_from_buffer=True)
            return

        if event.name == "]":  # Right bracket, newer messages
            current_buffer_index += 1
            if current_buffer_index >= len(SPEECH_BUFFER):
                current_buffer_index = len(SPEECH_BUFFER) - 1
                say(
                    f"Bottom: {SPEECH_BUFFER[current_buffer_index]}",
                    exclude_from_buffer=True,
                )
            else:
                say(SPEECH_BUFFER[current_buffer_index], exclude_from_buffer=True)

        elif event.name == "[":  # Left bracket, older messages
            current_buffer_index -= 1
            if current_buffer_index < 0:
                current_buffer_index = 0
                say(
                    f"Top: {SPEECH_BUFFER[current_buffer_index]}",
                    exclude_from_buffer=True,
                )
            else:
                say(SPEECH_BUFFER[current_buffer_index], exclude_from_buffer=True)


def toggle_buffer_mode():
    global buffer_mode_active, current_buffer_index, buffer_key_hooks
    buffer_mode_active = not buffer_mode_active
    if buffer_mode_active:
        current_buffer_index = len(SPEECH_BUFFER) - 1 if SPEECH_BUFFER else -1
        say("Buffer mode on", exclude_from_buffer=True)
        try:
            for key in ["[", "]"]:
                buffer_key_hooks.append(
                    _hook_suppressed(key, _kb_enqueue(on_buffer_nav_press))
                )
        except Exception as e:
            logger.error(f"Failed to hook buffer nav keys: {e}")
    else:
        say("Buffer mode off", exclude_from_buffer=True)
        _unhook_suppressed(buffer_key_hooks)


def _on_vbrowser_nav(event):
    global _vbrowser_index
    if not _vbrowser_lines:
        return
    if event.name == "down":
        _vbrowser_index += 1
        if _vbrowser_index >= len(_vbrowser_lines):
            _vbrowser_index = len(_vbrowser_lines) - 1
            say(f"Bottom: {_vbrowser_lines[_vbrowser_index]}", exclude_from_buffer=True)
        else:
            say(_vbrowser_lines[_vbrowser_index], exclude_from_buffer=True)
    elif event.name == "up":
        _vbrowser_index -= 1
        if _vbrowser_index < 0:
            _vbrowser_index = 0
            say(f"Top: {_vbrowser_lines[_vbrowser_index]}", exclude_from_buffer=True)
        else:
            say(_vbrowser_lines[_vbrowser_index], exclude_from_buffer=True)


def _on_vbrowser_escape(event):
    close_virtual_browser(speak_exit=True)


def _on_vbrowser_enter(event):
    if not _vbrowser_lines or _vbrowser_on_enter is None:
        return
    idx = _vbrowser_index
    line = _vbrowser_lines[idx]
    data = _vbrowser_entry_data[idx] if idx < len(_vbrowser_entry_data) else None
    try:
        _vbrowser_on_enter(idx, line, data)
    except Exception as e:
        logger.error(f"Virtual browser enter callback error: {e}")


def open_virtual_browser(
    lines, title=None, on_enter=None, entry_data=None, start_index=0
):
    global _vbrowser_lines, _vbrowser_index, _vbrowser_active
    global _vbrowser_on_enter, _vbrowser_entry_data
    if _vehicle_spawner is not None and _vehicle_spawner.is_modal_open():
        _vehicle_spawner.close_modal()
    close_virtual_browser(speak_exit=False)
    if not lines:
        say("No information available", exclude_from_buffer=True)
        return
    _vbrowser_lines = list(lines)
    _vbrowser_index = max(0, min(start_index, len(_vbrowser_lines) - 1))
    _vbrowser_active = True
    _vbrowser_on_enter = on_enter
    _vbrowser_entry_data = list(entry_data) if entry_data else []
    if KEYBOARD_OK:
        try:
            for key in ["up", "down"]:
                _vbrowser_hooks.append(
                    _hook_suppressed(key, _kb_enqueue(_on_vbrowser_nav))
                )
            # Escape especially must be queued: close_virtual_browser unhooks the
            # very list keyboard is iterating to reach this handler.
            _vbrowser_hooks.append(
                _hook_suppressed("escape", _kb_enqueue(_on_vbrowser_escape))
            )
            if on_enter is not None:
                _vbrowser_hooks.append(
                    _hook_suppressed("enter", _kb_enqueue(_on_vbrowser_enter))
                )
        except Exception as e:
            logger.error(f"Failed to hook virtual browser keys: {e}")
    if title:
        say(f"{title}. {_vbrowser_lines[_vbrowser_index]}", exclude_from_buffer=True)
    else:
        say(_vbrowser_lines[_vbrowser_index], exclude_from_buffer=True)


def close_virtual_browser(speak_exit=True):
    global _vbrowser_lines, _vbrowser_index, _vbrowser_active
    global _vbrowser_on_enter, _vbrowser_entry_data
    _vbrowser_active = False
    _vbrowser_on_enter = None
    _vbrowser_entry_data = []
    _unhook_suppressed(_vbrowser_hooks)
    _vbrowser_lines = []
    _vbrowser_index = 0
    if speak_exit:
        say("Exit", exclude_from_buffer=True)


SCANNER_CMD_PORT = 4448  # UDP port to send ON/OFF commands to vehicle scanner
AI_CMD_PORT = 4449  # UDP port to send AI commands to beamtelAI extension
SLOT_LISTEN_PORT = 4458  # UDP port to receive vehicle slot data from vehicleSlots.lua
SLOT_CMD_PORT_OUT = (
    4459  # UDP port to send slot management commands to vehicleSlots.lua
)
CAMERA_LISTEN_PORT = 4450  # UDP port to receive camera data from cameraInfo.lua
CAMERA_CMD_PORT = 4451  # UDP port to send ON/OFF commands to cameraInfo.lua
OBSTACLE_LISTEN_PORT = (
    4452  # UDP port to receive obstacle data from obstacleDetector.lua
)
OBSTACLE_CMD_PORT = 4453  # UDP port to send ON/OFF commands to obstacleDetector.lua
NODEGRAB_LISTEN_PORT = (
    4454  # UDP port to receive node data from nodeGrabberAccessible.lua
)
NODEGRAB_CMD_PORT = 4455  # UDP port to send commands to nodeGrabberAccessible.lua
CLICKSPOT_LISTEN_PORT = (
    4456  # UDP port to receive clickspot data from clickspotAccessible.lua
)
CLICKSPOT_CMD_PORT = 4457  # UDP port to send commands to clickspotAccessible.lua
ROAD_LISTEN_PORT = 4462  # UDP port to receive road status from roadDetector.lua
ROAD_CMD_PORT = 4463  # UDP port to send commands to roadDetector.lua
UI_TOGGLE_CMD_PORT = 4464  # UDP port to send HIDE/SHOW/TOGGLE commands to uiToggle.lua
CONSOLE_CMD_PORT = 4465  # UDP port to send EXEC/CTXLIST/LOGON/LOGOFF to consoleAccessible.lua
CONSOLE_RESP_PORT = (
    4466  # UDP port to receive console responses/log stream from consoleAccessible.lua
)
VEHICLE_BINDINGS_LISTEN_PORT = (
    4467  # UDP port to receive vehicle binding lists from vehicleBindings.lua
)
VEHICLE_BINDINGS_CMD_PORT = 4468  # UDP port to send commands to vehicleBindings.lua
IMPLEMENT_LISTEN_PORT = (
    4469  # UDP port to receive implement proximity events from implementProximity.lua
)
IMPLEMENT_CMD_PORT = 4470  # UDP port to send ON/OFF/REBUILD to implementProximity.lua
CONSOLE_HISTORY_PATH = os.path.join(CONFIG_DIR, "console_history.json")
CONSOLE_HISTORY_MAX = 50  # cap on persisted accessible-console command history

# Reference to the GUI frame, set by BeamTelFrame once it is constructed, so the
# console_listener thread can marshal incoming messages onto the wx controls.
console_frame = None


def _load_console_history():
    """Load persisted accessible-console command history (most recent last)."""
    try:
        with open(CONSOLE_HISTORY_PATH, encoding="utf-8") as f:
            data = json.load(f)
        if isinstance(data, list):
            return [str(x) for x in data][-CONSOLE_HISTORY_MAX:]
    except Exception:
        pass
    return []


def _save_console_history(history):
    """Persist accessible-console command history to disk."""
    try:
        with open(CONSOLE_HISTORY_PATH, "w", encoding="utf-8") as f:
            json.dump(history[-CONSOLE_HISTORY_MAX:], f)
    except Exception as e:
        logger.error(f"Failed to save console history: {e}")


def send_console_command(msg):
    """Send a single command datagram to consoleAccessible.lua on the GE side."""
    try:
        s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        s.sendto(msg.encode("utf-8"), ("127.0.0.1", CONSOLE_CMD_PORT))
        s.close()
    except Exception as e:
        logger.error(f"Failed to send console command via UDP: {e}")


def console_listener(stop_event):
    """Receive responses / context list / log stream from consoleAccessible.lua.

    Each UDP datagram is one record (RESP|, OUT|, LOG|, CTX|, CTXEND, EXECEND).
    Records are handed to the GUI via wx.CallAfter so all control updates run on
    the wx main thread.
    """
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        # A single large dump is chunked into many datagrams arriving in a burst; enlarge the
        # receive buffer so the kernel doesn't drop them before this thread drains them.
        try:
            sock.setsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF, 1 << 20)
        except OSError:
            pass
        sock.bind(("127.0.0.1", CONSOLE_RESP_PORT))
        sock.settimeout(0.2)
        logger.info(f"Console listener started on port {CONSOLE_RESP_PORT}")
        while not stop_event.is_set():
            try:
                data, _addr = sock.recvfrom(8192)
            except socket.timeout:
                continue
            except OSError:
                break
            text = data.decode("utf-8", errors="replace")
            frame = console_frame
            if frame is not None:
                wx.CallAfter(frame.on_console_message, text)
    finally:
        sock.close()

# AI Control State
ai_speed_limit_ms = None  # current speed limit in m/s, None = off
ai_avoid_mode = "auto"  # "auto", "on", "off"
ai_lane_driving = False

# Vehicle Slot State (updated by slot_listener thread)
_vehicle_slots: dict = {}  # slot_num (1-10) → {"id": int, "name": str}
_selected_slots: set = set()  # slot numbers currently selected for multi-AI dispatch
_target_slot = None  # slot number whose vehicle is the AI target
_pending_target_confirm = None  # slot number awaiting CTRL+digit confirmation
_slots_lock = threading.Lock()

# Tracks the last AI command issued to each vehicle by Python.
# vid → {"mode": str, "target_id": int|None}
# Updated whenever a command is dispatched; read by the space-bar status reporter.
_ai_vehicle_commands: dict = {}


def toggle_scan_mode(audio_controller):
    global \
        scan_mode_active, \
        coupler_dist_mode, \
        last_scanner_target_name, \
        last_scanner_distance, \
        last_scanner_approach_deg, \
        last_scanner_bearing
    scan_mode_active = not scan_mode_active
    coupler_dist_mode = False
    last_scanner_target_name = ""
    last_scanner_distance = float("inf")
    last_scanner_approach_deg = 0.0
    last_scanner_bearing = 0.0

    command = "ON" if scan_mode_active else "OFF"

    try:
        cmd_sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        cmd_sock.sendto(command.encode("utf-8"), ("127.0.0.1", SCANNER_CMD_PORT))
        cmd_sock.close()
    except Exception as e:
        logger.error(f"Failed to send scanner command via UDP: {e}")

    audio_controller.set_scan_mode(scan_mode_active)
    if not scan_mode_active:
        audio_controller.set_coupler_tracking(False)

    # On a loader the scanner aims from the implement, not from the cab (see
    # getImplementFrame in implementProximity.lua). Say so, or the operator has no way to
    # know which end of the machine the bearing refers to.
    # Gated on the live telemetry flag as well as the name. The name arrives from the GE
    # extension and describes whatever vehicle last reported one, so on its own it can go
    # stale across a vehicle switch; implementFlags comes from the struct and always
    # describes the vehicle you are sitting in right now.
    suffix = ""
    if (
        scan_mode_active
        and _implement_word_current
        and int(last_implement_flags) & IMPL_FLAG_PRESENT
    ):
        suffix = f", measuring from the {_implement_word_current.lower()}"
    say(
        f"Vehicle scanner {'on' if scan_mode_active else 'off'}{suffix}",
        exclude_from_buffer=True,
    )


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

    say(
        f"Obstacle detection {'on' if obstacle_mode_active else 'off'}",
        exclude_from_buffer=True,
    )


def toggle_road_mode(audio_controller):
    global road_mode_active
    road_mode_active = not road_mode_active

    command = "ON" if road_mode_active else "OFF"
    try:
        cmd_sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        cmd_sock.sendto(command.encode("utf-8"), ("127.0.0.1", ROAD_CMD_PORT))
        cmd_sock.close()
    except Exception as e:
        logger.error(f"Failed to send road detector command via UDP: {e}")

    audio_controller.set_road_mode(road_mode_active)

    say(
        f"Road detection {'on' if road_mode_active else 'off'}",
        exclude_from_buffer=True,
    )


def _format_camera_diag(heights: str) -> str:
    """Phrase the camera diagnostic in the same units as every other readout.

    `heights` is "<cameraZ>,<groundZ>" in metres, with groundZ "nan" when no ground was
    found, or the sentinel "ERR". Doing the conversion here rather than in Lua is what
    keeps this and the Alt+A altitude from reporting one height as two numbers.
    """
    try:
        cam_z_m, ground_m = (float(v) for v in heights.split(",")[:2])
    except ValueError:
        return "Camera diagnostic failed, see the log."
    conv, unit = (3.28084, "feet") if UNITS_MODE == "imperial" else (1.0, "meters")
    if math.isnan(ground_m):
        return f"Camera height {cam_z_m * conv:.1f} {unit}. No ground found below."
    return (
        f"Camera height {cam_z_m * conv:.1f} {unit}. "
        f"Ground {ground_m * conv:.1f}. "
        f"Above ground {(cam_z_m - ground_m) * conv:.1f} {unit}."
    )


def send_camera_command(command: str) -> bool:
    """Send one command to cameraInfo.lua. Returns False if it couldn't be sent."""
    try:
        cmd_sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        cmd_sock.sendto(command.encode("utf-8"), ("127.0.0.1", CAMERA_CMD_PORT))
        cmd_sock.close()
        return True
    except Exception as e:
        logger.error(f"Failed to send camera command {command} via UDP: {e}")
        return False


def _camera_feed_problem():
    """Why the camera readouts can't be trusted right now, or None when they're live.

    The Lua side stops streaming whenever the mod reloads, and it has no way to tell us —
    so without this check the readouts keep speaking the last packet they ever received,
    frozen but indistinguishable from a live value. Asking for the feed again here means
    the next press usually works.
    """
    if not free_cam_active:
        return "Camera info is off"
    with state_lock:
        age = time.time() - cam_last_packet_ts
    if age > CAMERA_STALE_SEC:
        send_camera_command("ON")
        return "Camera info is not updating, reconnecting"
    return None


def send_camera_diag():
    """Ask cameraInfo.lua for a one-shot camera/ground dump (F9 then Alt+Shift+A).

    The reply comes back on the camera data port and is both spoken and logged; it works
    whether or not the live camera feed is switched on.
    """
    if send_camera_command("DIAG"):
        say("Camera diagnostic requested", exclude_from_buffer=True)
    else:
        say("Camera diagnostic failed to send", exclude_from_buffer=True)


def toggle_free_cam_info(audio_controller):
    global free_cam_active, cam_last_click_heading_deg, cam_compass_click_counter
    global cam_last_announced_compass_idx, cam_last_compass_ts
    free_cam_active = not free_cam_active

    send_camera_command("ON" if free_cam_active else "OFF")

    if free_cam_active:
        with state_lock:
            cam_last_click_heading_deg = 0.0
            cam_compass_click_counter = 0
            cam_last_announced_compass_idx = -1
            cam_last_compass_ts = 0.0

    say(f"Camera info {'on' if free_cam_active else 'off'}", exclude_from_buffer=True)


def _send_nodegrab_cmd(command):
    try:
        cmd_sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        cmd_sock.sendto(command.encode("utf-8"), ("127.0.0.1", NODEGRAB_CMD_PORT))
        cmd_sock.close()
    except Exception as e:
        logger.error(f"Failed to send node grabber command via UDP: {e}")


def _install_nodegrab_hooks():
    global _nodegrab_scroll_hook
    try:
        import mouse

        _nodegrab_scroll_hook = mouse.hook(_nodegrab_on_mouse_event)
    except ImportError:
        logger.warning(
            "mouse module unavailable — scroll/middle-click hooks disabled for node grabber"
        )


def _nodegrab_on_mouse_event(event):
    """Hook for mouse wheel events while node grabber is active."""
    try:
        import mouse

        if isinstance(event, mouse.WheelEvent):
            if not nodegrab_mode_active:
                return
            global nodegrab_strength
            if event.delta > 0:
                nodegrab_strength = min(100, nodegrab_strength + 5)
            else:
                nodegrab_strength = max(0, nodegrab_strength - 5)
            say(f"Strength {nodegrab_strength} percent", exclude_from_buffer=True)
        elif (
            isinstance(event, mouse.ButtonEvent)
            and event.button == "middle"
            and event.event_type == "up"
        ):
            if nodegrab_mode_active and nodegrab_scanning:
                say("Node pinned", exclude_from_buffer=True)
    except Exception as e:
        logger.error(f"Node grabber mouse event error: {e}")


def _remove_nodegrab_hooks():
    global _nodegrab_scroll_hook, nodegrab_scanning
    nodegrab_scanning = False
    if _nodegrab_scroll_hook is not None:
        try:
            import mouse

            mouse.unhook(_nodegrab_scroll_hook)
        except Exception:
            pass
        _nodegrab_scroll_hook = None


def toggle_nodegrab_mode(audio_controller):
    global nodegrab_mode_active, nodegrab_last_cid, nodegrab_strength
    nodegrab_mode_active = not nodegrab_mode_active
    nodegrab_last_cid = -1
    nodegrab_strength = 50

    command = "ON" if nodegrab_mode_active else "OFF"
    _send_nodegrab_cmd(command)

    if nodegrab_mode_active:
        _install_nodegrab_hooks()
    else:
        _remove_nodegrab_hooks()

    say(
        f"Accessible node grabber {'on' if nodegrab_mode_active else 'off'}",
        exclude_from_buffer=True,
    )


def _send_clickspot_cmd(command):
    try:
        cmd_sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        cmd_sock.sendto(command.encode("utf-8"), ("127.0.0.1", CLICKSPOT_CMD_PORT))
        cmd_sock.close()
    except Exception as e:
        logger.error(f"Failed to send clickspot command via UDP: {e}")


def _clickspot_browser_on_enter(idx, line, data):
    """Virtual browser enter callback — snap cursor to trigger and execute press+release."""
    if data is None:
        return
    cache_idx = data
    _send_clickspot_cmd(f"SNAP:{cache_idx}")
    # Also execute a press+release to activate the trigger
    import threading as _th

    def _do_exec():
        time.sleep(0.1)  # small delay to let SNAP/cursor warp complete
        _send_clickspot_cmd(f"EXEC:{cache_idx},1")
        time.sleep(0.15)
        _send_clickspot_cmd(f"EXEC:{cache_idx},0")

    _th.Thread(target=_do_exec, daemon=True).start()


def toggle_clickspot_mode(audio_controller):
    global clickspot_mode_active, clickspot_trigger_list
    clickspot_mode_active = not clickspot_mode_active
    clickspot_trigger_list = []

    command = "ON" if clickspot_mode_active else "OFF"
    _send_clickspot_cmd(command)

    say(
        f"Clickspot detection {'on' if clickspot_mode_active else 'off'}",
        exclude_from_buffer=True,
    )


def _send_vehicle_bindings_cmd(command):
    try:
        cmd_sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        cmd_sock.sendto(
            command.encode("utf-8"), ("127.0.0.1", VEHICLE_BINDINGS_CMD_PORT)
        )
        cmd_sock.close()
    except Exception as e:
        logger.error(f"Failed to send vehicle bindings command via UDP: {e}")


def _vehicle_bindings_on_enter(idx, line, data):
    """Virtual browser enter callback — fire the highlighted vehicle action."""
    if data is None:
        return
    # No delayed press/release pair is needed here (unlike clickspots): the Lua
    # side drives ActionMap's down+up itself.
    _send_vehicle_bindings_cmd(f"EXEC:{data}")


def open_vehicle_bindings_browser():
    """Open a virtual browser listing the current vehicle's special controls."""
    if not _vehicle_bindings_list:
        # Nudge the mod in case the push was missed (e.g. beamtel started after
        # the vehicle spawned), then report rather than opening an empty list.
        _send_vehicle_bindings_cmd("REQUEST")
        say("No vehicle bindings available", exclude_from_buffer=True)
        return
    lines = [b[1] for b in _vehicle_bindings_list]
    entry_data = [b[0] for b in _vehicle_bindings_list]
    vehicle = _vehicle_bindings_vehicle or "Vehicle"
    open_virtual_browser(
        lines,
        title=f"{vehicle}, {len(lines)} binding{'s' if len(lines) != 1 else ''}",
        on_enter=_vehicle_bindings_on_enter,
        entry_data=entry_data,
    )


def open_clickspot_browser():
    """Open a virtual browser listing all detected clickspots."""
    if not clickspot_mode_active:
        say("Clickspot detection is not active", exclude_from_buffer=True)
        return
    if not clickspot_trigger_list:
        say("No clickspots available", exclude_from_buffer=True)
        return
    lines = [t[2] for t in clickspot_trigger_list]  # display names
    entry_data = [t[0] for t in clickspot_trigger_list]  # cache indices
    open_virtual_browser(
        lines,
        title=f"{len(lines)} clickspots",
        on_enter=_clickspot_browser_on_enter,
        entry_data=entry_data,
    )


def _on_next_key_press(event, audio_controller):
    global marked_coord_x, marked_coord_y, _last_coord_bearing_ts, _input_help_mode
    global dock_mode_active, last_dock, last_dock_fail
    if event.event_type != "down":
        return
    name = (event.name or "").lower()

    if name in ("ctrl", "control", "left ctrl", "right ctrl"):
        _capture_mods["ctrl"] = True
        return
    if name in ("shift", "left shift", "right shift"):
        _capture_mods["shift"] = True
        return
    if name in ("alt", "left alt", "right alt"):
        _capture_mods["alt"] = True
        return

    if name == "f9":
        return

    # Toggle input help mode with ? (shift+/)
    if name in ("/", "?"):
        _input_help_mode = not _input_help_mode
        if _input_help_mode:
            say("Input help on", exclude_from_buffer=True)
            # Cancel timeout so layer stays open
            if next_key_timer is not None:
                try:
                    next_key_timer.cancel()
                except Exception:
                    pass
        else:
            say("Input help off", exclude_from_buffer=True)
            _clear_next_key_hook(speak_exit=False)
        return

    # In help mode: speak what the key does, reset timeout, don't execute
    if _input_help_mode:
        key = (
            name,
            _capture_mods["ctrl"],
            _capture_mods["shift"],
            _capture_mods["alt"],
        )
        desc = _F9_HELP.get(key)
        if name.isdigit() and not (
            _capture_mods["ctrl"] or _capture_mods["shift"] or _capture_mods["alt"]
        ):
            desc = (
                f"Speak buffer message {name}"
                if name != "0"
                else "Speak buffer message 10"
            )
        if desc:
            say(desc, exclude_from_buffer=True)
        else:
            say("No command", exclude_from_buffer=True)
        return

    if name.isdigit() and not (
        _capture_mods["ctrl"] or _capture_mods["shift"] or _capture_mods["alt"]
    ):
        target_recency = 10 if name == "0" else int(name)
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
        gear_phrase = (
            gear_to_phrase(last_gear_byte if last_gear_byte is not None else 1)
            if protocol_mode == "outgauge"
            else extended_gear_to_phrase(last_gear_str or "")
        )
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
        cam_agl_valid_snap = cam_agl_valid
        cam_bearing_snap = cam_veh_bearing
        cam_dist_snap = cam_veh_distance

    # ALT+ camera info commands (before bare key handlers)
    if (
        name == "f"
        and _capture_mods["alt"]
        and not (_capture_mods["ctrl"] or _capture_mods["shift"])
    ):
        toggle_free_cam_info(audio_controller)
        _clear_next_key_hook(speak_exit=False)
        return
    elif (
        name == "h"
        and _capture_mods["alt"]
        and not (_capture_mods["ctrl"] or _capture_mods["shift"])
    ):
        cam_problem = _camera_feed_problem()
        if cam_problem is None:
            say(f"Camera heading {cam_yaw_snap:.0f} degrees", exclude_from_buffer=True)
        else:
            say(cam_problem, exclude_from_buffer=True)
        _clear_next_key_hook(speak_exit=False)
        return
    elif (
        name == "a"
        and _capture_mods["alt"]
        and not (_capture_mods["ctrl"] or _capture_mods["shift"])
    ):
        cam_problem = _camera_feed_problem()
        if cam_problem is None:
            if UNITS_MODE == "imperial":
                alt_val, unit = cam_agl_snap * 3.28084, "feet"
            else:
                alt_val, unit = cam_agl_snap, "meters"
            # When no ground was found under the camera the number is an absolute
            # height, so say so instead of calling sea level the ground.
            suffix = "" if cam_agl_valid_snap else " above sea level, ground unknown"
            say(
                f"Camera altitude {alt_val:.1f} {unit}{suffix}",
                exclude_from_buffer=True,
            )
        else:
            say(cam_problem, exclude_from_buffer=True)
        _clear_next_key_hook(speak_exit=False)
        return
    elif (
        name == "a"
        and _capture_mods["alt"]
        and _capture_mods["shift"]
        and not _capture_mods["ctrl"]
    ):
        # Camera/ground diagnostic. Speaks a summary and writes the full detail to
        # bnvdahook.log, so a wrong altitude can be traced to the query behind it.
        send_camera_diag()
        _clear_next_key_hook(speak_exit=False)
        return
    elif (
        name == "p"
        and _capture_mods["alt"]
        and not (_capture_mods["ctrl"] or _capture_mods["shift"])
    ):
        cam_problem = _camera_feed_problem()
        if cam_problem is None:
            direction = "up" if cam_pitch_snap >= 0 else "down"
            say(
                f"Camera pitch {abs(cam_pitch_snap):.0f} degrees {direction}",
                exclude_from_buffer=True,
            )
        else:
            say(cam_problem, exclude_from_buffer=True)
        _clear_next_key_hook(speak_exit=False)
        return
    elif (
        name == "v"
        and _capture_mods["alt"]
        and not (_capture_mods["ctrl"] or _capture_mods["shift"])
    ):
        cam_problem = _camera_feed_problem()
        if cam_problem is None:
            if cam_dist_snap < 0:
                say("No vehicle", exclude_from_buffer=True)
            else:
                direction = "left" if cam_bearing_snap > 0 else "right"
                say(
                    f"Vehicle {abs(cam_bearing_snap):.0f} degrees {direction}",
                    exclude_from_buffer=True,
                )
        else:
            say(cam_problem, exclude_from_buffer=True)
        _clear_next_key_hook(speak_exit=False)
        return
    elif (
        name == "d"
        and _capture_mods["alt"]
        and not (_capture_mods["ctrl"] or _capture_mods["shift"])
    ):
        cam_problem = _camera_feed_problem()
        if cam_problem is None:
            if cam_dist_snap < 0:
                say("No vehicle", exclude_from_buffer=True)
            else:
                if UNITS_MODE == "imperial":
                    dist_val = cam_dist_snap * 3.28084
                    say(f"Vehicle {dist_val:.0f} feet away", exclude_from_buffer=True)
                else:
                    say(
                        f"Vehicle {cam_dist_snap:.0f} meters away",
                        exclude_from_buffer=True,
                    )
        else:
            say(cam_problem, exclude_from_buffer=True)
        _clear_next_key_hook(speak_exit=False)
        return
    if name == "s" and not _capture_mods["ctrl"]:
        say(f"{spd_val} {spd_unit}")
    elif name == "r" and _capture_mods["shift"] and not _capture_mods["ctrl"]:
        say(
            f"Redline {int(round(rpm_max_snap))} RPM"
            if protocol_mode == "extended"
            else "Unavailable"
        )
    elif name == "r" and _capture_mods["ctrl"] and not (_capture_mods["shift"] or _capture_mods["alt"]):
        toggle_road_mode(audio_controller)
    elif name == "r" and not (_capture_mods["ctrl"] or _capture_mods["shift"] or _capture_mods["alt"]):
        say(f"{rpm} rpm")
    elif name == "h" and _capture_mods["ctrl"]:
        global heading_guidance_active, heading_guidance_target
        heading_guidance_active = not heading_guidance_active
        if heading_guidance_active:
            with state_lock:
                heading_guidance_target = last_heading
            say(f"Heading guidance on", exclude_from_buffer=True)
        else:
            say("Heading guidance off", exclude_from_buffer=True)
    elif (
        name == "d"
        and _capture_mods["ctrl"]
        and _capture_mods["shift"]
        and not _capture_mods["alt"]
    ):
        global coupler_dist_mode
        if not scan_mode_active:
            say("Vehicle scanner is not active", exclude_from_buffer=True)
        else:
            coupler_dist_mode = not coupler_dist_mode
            try:
                cmd_sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
                cmd_sock.sendto(b"COUPLER_DIST", ("127.0.0.1", SCANNER_CMD_PORT))
                cmd_sock.close()
            except Exception as e:
                logger.error(f"Failed to send COUPLER_DIST command via UDP: {e}")
    elif (
        name == "d"
        and _capture_mods["ctrl"]
        and not (_capture_mods["shift"] or _capture_mods["alt"])
    ):
        global \
            drift_mode_active, \
            last_drift_check_ts, \
            drift_baseline_heading, \
            drift_alert_active
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
    elif (
        name == "d"
        and _capture_mods["shift"]
        and not (_capture_mods["ctrl"] or _capture_mods["alt"])
    ):
        if not scan_mode_active:
            say("Scanner is not active", exclude_from_buffer=True)
        else:
            with state_lock:
                brg = last_scanner_bearing
                dist = last_scanner_distance
            if dist == float("inf"):
                say("No target", exclude_from_buffer=True)
            else:
                direction = "left" if brg >= 0 else "right"
                say(f"{abs(brg):.0f} degrees {direction}", exclude_from_buffer=True)
    elif name == "d" and not (
        _capture_mods["ctrl"] or _capture_mods["shift"] or _capture_mods["alt"]
    ):
        if not scan_mode_active:
            say("Scanner is not active", exclude_from_buffer=True)
        else:
            with state_lock:
                dist = last_scanner_distance
                brg = last_scanner_bearing
                tname = last_scanner_target_name
                approach = last_scanner_approach_deg
            if dist == float("inf"):
                say("No target", exclude_from_buffer=True)
            else:
                a = abs(brg)
                if a < 30:
                    orientation = "approaching"
                elif a < 60:
                    orientation = "angling toward"
                elif a < 120:
                    orientation = "broadside"
                elif a < 150:
                    orientation = "angling away"
                else:
                    orientation = "departing"
                ap = abs(approach)
                if ap < 45:
                    side = "front"
                elif ap > 135:
                    side = "rear"
                elif approach > 0:  # positive = player is off the target's left
                    side = "left side"
                else:
                    side = "right side"
                if UNITS_MODE == "imperial":
                    dist_str = f"{dist * 3.28084:.0f} feet"
                else:
                    dist_str = f"{dist:.0f} meters"
                prefix = f"{tname}, " if tname else ""
                say(
                    f"{prefix}{dist_str} {orientation} {side}", exclude_from_buffer=True
                )
    elif name == "i" and not (
        _capture_mods["ctrl"] or _capture_mods["shift"] or _capture_mods["alt"]
    ):
        # The cane tap. One deliberate press, one complete picture, no ambient noise.
        if not _implement_word_current:
            say("No implement fitted", exclude_from_buffer=True)
        elif not dock_mode_active:
            say("Docking instrument is off", exclude_from_buffer=True)
        else:
            with state_lock:
                snap = dict(last_dock) if last_dock else None
                why = last_dock_fail
            if snap is not None:
                say(_dock_phrase(snap), exclude_from_buffer=True)
            elif why:
                # The instrument shares its soundscape with the scanner, which also pans,
                # changes pitch and pulses — so a dead instrument sounds exactly like a
                # working one. This is the only way to tell them apart from the driver's
                # seat, which is why the reason is carried all the way from the mod.
                say(f"No reading. {why}", exclude_from_buffer=True)
            else:
                say("Nothing in range", exclude_from_buffer=True)
    elif (
        name == "i"
        and _capture_mods["shift"]
        and not (_capture_mods["ctrl"] or _capture_mods["alt"])
    ):
        # Retarget the reference band. The mod auto-selects on lock — the lowest void for
        # forks, the tallest face for a bucket — and this overrides it when you want a
        # different part of the same object: a window rather than the sill, the roofline
        # rather than the pocket.
        if not _implement_word_current:
            say("No implement fitted", exclude_from_buffer=True)
        else:
            _send_implement_cmd("BANDNEXT")
            # The mod recomputes on its next 10 Hz tick, so read back rather than predicting.
            time.sleep(0.15)
            with state_lock:
                snap = dict(last_dock) if last_dock else None
            if snap is None:
                say("Nothing in range", exclude_from_buffer=True)
            else:
                band = _band_name(snap["kind"], snap["idx"], snap["count"])
                vert = snap["vertical"]
                if abs(vert) < IMPL_DOCK_LEVEL_M:
                    where = "level"
                else:
                    vv, vu = fmt_distance(abs(vert))
                    where = f"{'raise' if vert > 0 else 'lower'} {vv} {vu}"
                say(
                    f"{band} {snap['idx']} of {snap['count']}, {where}",
                    exclude_from_buffer=True,
                )
    elif (
        name == "i"
        and _capture_mods["ctrl"]
        and not (_capture_mods["shift"] or _capture_mods["alt"])
    ):
        dock_mode_active = not dock_mode_active
        # Re-announce on the way in, so toggling the instrument is a genuine recovery path
        # if the mod and this side have drifted apart about what is fitted.
        if dock_mode_active:
            _send_implement_cmd("ON")
        _send_implement_cmd("DOCK_ON" if dock_mode_active else "DOCK_OFF")
        audio_controller.set_dock_mode(dock_mode_active)
        if not dock_mode_active:
            with state_lock:
                last_dock = None
                last_dock_fail = None
            say("Docking instrument off", exclude_from_buffer=True)
        else:
            # The re-announce above travels to the mod and back, and the mod scans at 10 Hz,
            # so read the answer rather than the stale value. Without this the toggle reports
            # "no implement fitted" from state it is in the middle of refreshing.
            time.sleep(0.25)
            if _implement_word_current:
                # Auto-select is per target, so a fresh mode start should not inherit a band
                # pick made against something you have since driven away from.
                _send_implement_cmd("BANDAUTO")
                say("Docking instrument on", exclude_from_buffer=True)
            else:
                # Left on deliberately rather than refused: you may be about to climb into a
                # machine that has one, and the readout is inert until then. It also
                # self-heals shortly after a vehicle switch, once the mod's next heartbeat
                # re-delivers the implement to the game-engine side.
                say("Docking instrument on. No implement fitted", exclude_from_buffer=True)
    elif name == "w" and not (
        _capture_mods["ctrl"] or _capture_mods["shift"] or _capture_mods["alt"]
    ):
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
                if err > 180.0:
                    err -= 360.0
                elif err < -180.0:
                    err += 360.0
                turn_dir = "left" if err > 0 else "right"
                if UNITS_MODE == "imperial":
                    dist_str = f"{dist * 3.28084:.0f} feet"
                else:
                    dist_str = f"{dist:.0f} meters"
                say(
                    f"{dist_str}, {abs(err):.0f} degrees {turn_dir}",
                    exclude_from_buffer=True,
                )
    elif (
        name == "s"
        and _capture_mods["ctrl"]
        and _capture_mods["shift"]
        and not _capture_mods["alt"]
    ):
        global speech_logging_active
        speech_logging_active = not speech_logging_active
        if speech_logging_active:
            # Clear the log on each new session
            try:
                open(SPEECH_LOG_PATH, "w").close()
            except Exception:
                pass
            say("Speech logging on", exclude_from_buffer=True)
        else:
            say(
                f"Speech logging off, saved to {SPEECH_LOG_PATH}",
                exclude_from_buffer=True,
            )
    elif (
        name == "l"
        and _capture_mods["ctrl"]
        and _capture_mods["shift"]
        and not _capture_mods["alt"]
    ):
        global low_speed_mode_active
        low_speed_mode_active = not low_speed_mode_active
        say(
            "Low speed detection on"
            if low_speed_mode_active
            else "Low speed detection off",
            exclude_from_buffer=True,
        )
    elif (
        name == "k"
        and _capture_mods["ctrl"]
        and not (_capture_mods["shift"] or _capture_mods["alt"])
    ):
        global slip_mode_active
        slip_mode_active = not slip_mode_active
        say(
            "Wheel slip detection on"
            if slip_mode_active
            else "Wheel slip detection off",
            exclude_from_buffer=True,
        )
    elif (
        name == "l"
        and _capture_mods["ctrl"]
        and not (_capture_mods["shift"] or _capture_mods["alt"])
    ):
        say("Dumping DOM", exclude_from_buffer=True)
        broadcast({"type": "dom_dump"})
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
            DUMP_RESP_PORT = 4447
            recv_sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
            try:
                recv_sock.bind(("127.0.0.1", DUMP_RESP_PORT))
                cmd_sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
                cmd_sock.sendto(b"HDUMP", ("127.0.0.1", SCANNER_CMD_PORT))
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
                logger.info(
                    f"=== HYDROS DUMP ({len(lines)} values) ===\n{content}\n=== END DUMP ==="
                )
                say(
                    f"Hydros dumped, {len(lines)} values logged",
                    exclude_from_buffer=True,
                )
            else:
                say(
                    "Hydros dump timed out — is a vehicle spawned?",
                    exclude_from_buffer=True,
                )

        threading.Thread(target=_do_hdump, daemon=True).start()
    elif name == "h":
        say(hdg)
    elif name == "f":
        say(f"Fuel {fuel_pct} percent")
    elif name == "g":
        say(gear_phrase)
    elif name == "t" and _capture_mods["shift"]:
        val, unit = fmt_turbo(turbo_max_snap)
        say(f"Max turbo {val} {unit}" if protocol_mode == "extended" else "Unavailable")
    elif name == "t":
        say(f"Turbo {turbo_val} {turbo_unit}")
    elif name == "p" and _capture_mods["shift"]:
        say("Requesting powertrain dump", exclude_from_buffer=True)

        def _do_pdump():
            DUMP_RESP_PORT = 4447
            recv_sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
            try:
                recv_sock.bind(("127.0.0.1", DUMP_RESP_PORT))
                cmd_sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
                cmd_sock.sendto(b"PDUMP", ("127.0.0.1", SCANNER_CMD_PORT))
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
                logger.info(
                    f"=== POWERTRAIN DUMP ({len(lines)} values) ===\n{content}\n=== END DUMP ==="
                )
                say(
                    f"Powertrain dumped, {len(lines)} values logged",
                    exclude_from_buffer=True,
                )
            else:
                say(
                    "Powertrain dump timed out — is a vehicle spawned?",
                    exclude_from_buffer=True,
                )

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
            else:
                say("Pneumatic system not available")
        else:
            say("Pneumatic data unavailable")
    elif name == "u":
        flip_units()
        say(UNITS_MODE, exclude_from_buffer=True)
    elif name == "e" and _capture_mods["shift"]:
        say("Requesting electrics dump", exclude_from_buffer=True)

        def _do_dump():
            DUMP_RESP_PORT = 4447
            # Open receive socket first so we don't miss the response
            recv_sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
            try:
                recv_sock.bind(("127.0.0.1", DUMP_RESP_PORT))
                # Send the DUMP command to the vehicle scanner (GE VM)
                cmd_sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
                cmd_sock.sendto(b"DUMP", ("127.0.0.1", SCANNER_CMD_PORT))
                cmd_sock.close()
                # Collect lines until "DONE" or timeout
                recv_sock.settimeout(5.0)  # wait up to 5 s for first packet
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
                logger.info(
                    f"=== ELECTRICS DUMP ({len(lines)} values) ===\n{content}\n=== END DUMP ==="
                )
                say(
                    f"Electrics dumped, {len(lines)} values logged",
                    exclude_from_buffer=True,
                )
            else:
                say(
                    "Electrics dump timed out — is a vehicle spawned?",
                    exclude_from_buffer=True,
                )

        threading.Thread(target=_do_dump, daemon=True).start()
    elif name in ("e", "engtemp") and not _capture_mods["shift"]:
        say(f"Engine temperature {etemp_val} {etemp_unit}")
    elif name == "m" and not (
        _capture_mods["ctrl"] or _capture_mods["shift"] or _capture_mods["alt"]
    ):
        say("Checking damage", exclude_from_buffer=True)

        def _do_damage_report():
            DUMP_RESP_PORT = 4447
            recv_sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
            try:
                recv_sock.bind(("127.0.0.1", DUMP_RESP_PORT))
                cmd_sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
                cmd_sock.sendto(b"DAMAGE", ("127.0.0.1", SCANNER_CMD_PORT))
                cmd_sock.close()
                recv_sock.settimeout(5.0)
                veh_name = ""
                items = []
                for _ in range(200):
                    try:
                        data, _ = recv_sock.recvfrom(4096)
                        text = data.decode("utf-8", errors="replace").strip()
                        if text == "DONE":
                            break
                        if text == "NONE":
                            continue
                        if text.startswith("NAME:"):
                            veh_name = text[5:]
                            continue
                        items.append(text)
                        recv_sock.settimeout(0.5)
                    except socket.timeout:
                        break
            finally:
                recv_sock.close()
            preamble = f"Damage report for {veh_name}" if veh_name else "Damage report"
            if not items:
                say(f"{preamble}: no damaged components", exclude_from_buffer=True)
            else:
                if len(items) == 1:
                    sentence = items[0]
                elif len(items) == 2:
                    sentence = f"{items[0]} and {items[1]}"
                else:
                    sentence = ", ".join(items[:-1]) + ", and " + items[-1]
                say(f"{preamble}: {sentence}", exclude_from_buffer=True)

        threading.Thread(target=_do_damage_report, daemon=True).start()
    elif name == "o" and not _capture_mods["ctrl"]:
        say(f"Oil temperature {otemp_val} {otemp_unit}")
    elif (
        name == "c"
        and _capture_mods["ctrl"]
        and _capture_mods["shift"]
        and _capture_mods["alt"]
    ):
        open_clickspot_browser()
    elif (
        name == "c"
        and _capture_mods["ctrl"]
        and _capture_mods["shift"]
        and not _capture_mods["alt"]
    ):
        toggle_clickspot_mode(audio_controller)
    elif (
        name == "c"
        and _capture_mods["alt"]
        and not (_capture_mods["ctrl"] or _capture_mods["shift"])
    ):
        if marked_coord_x is None:
            say("No waypoint marked", exclude_from_buffer=True)
        else:
            say(
                f"Waypoint X {marked_coord_x:.1f}, Y {marked_coord_y:.1f}",
                exclude_from_buffer=True,
            )
    elif (
        name == "c"
        and _capture_mods["shift"]
        and not (_capture_mods["ctrl"] or _capture_mods["alt"])
    ):
        marked_coord_x, marked_coord_y = x, y
        _last_coord_bearing_ts = 0.0  # force immediate recalculation
        say(f"Waypoint marked at X {x:.1f}, Y {y:.1f}", exclude_from_buffer=True)
    elif (
        name == "c"
        and not _capture_mods["ctrl"]
        and not _capture_mods["shift"]
        and not _capture_mods["alt"]
    ):
        say(f"Coordinates X {x:.2f}, Y {y:.2f}, Z {z:.2f}")
    elif name == "s" and _capture_mods["ctrl"]:
        toggle_status_mode()
    elif name == "b" and _capture_mods["ctrl"]:
        toggle_buffer_mode()
    elif (
        name == "b"
        and not _capture_mods["ctrl"]
        and not _capture_mods["shift"]
        and not _capture_mods["alt"]
    ):
        open_vehicle_bindings_browser()
    elif (
        name == "c"
        and _capture_mods["ctrl"]
        and not (_capture_mods["shift"] or _capture_mods["alt"])
    ):
        global pedal_tones_active
        pedal_tones_active = not pedal_tones_active
        say(
            "Pedal tones on" if pedal_tones_active else "Pedal tones off",
            exclude_from_buffer=True,
        )
    elif name == "v" and _capture_mods["shift"] and not _capture_mods["ctrl"]:
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
    elif (
        name == "n"
        and _capture_mods["ctrl"]
        and not (_capture_mods["shift"] or _capture_mods["alt"])
    ):
        toggle_nodegrab_mode(audio_controller)
    elif name == "tab":
        if not scan_mode_active:
            say("Turn on vehicle scanner first", exclude_from_buffer=True)
        else:
            if _capture_mods["ctrl"]:
                direction = b"CLOSEST"
            elif _capture_mods["shift"]:
                direction = b"PREV"
            else:
                direction = b"NEXT"
            try:
                sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
                sock.sendto(direction, ("127.0.0.1", SCANNER_CMD_PORT))
                sock.close()
            except Exception as e:
                logger.error(f"Failed to send {direction} command: {e}")
    elif name == "a":
        say(f"Roll {roll_deg_snap:.1f} degrees, pitch {pitch_deg_snap:.1f} degrees")
    elif name == "space" and not (
        _capture_mods["ctrl"] or _capture_mods["shift"] or _capture_mods["alt"]
    ):
        broadcast({"type": "context_action", "action": "activate"})

    _clear_next_key_hook(speak_exit=False)


def _start_next_key_capture(audio_controller):
    global next_key_timer
    _clear_next_key_hook(speak_exit=False)
    _kb_open_layer("f9")
    next_key_timer = threading.Timer(
        command_timeout_sec, lambda: _clear_next_key_hook(speak_exit=True)
    )
    next_key_timer.daemon = True
    next_key_timer.start()


# =========================
#  AI Control (F10 layer)
# =========================
_AGGRESSION_MAP = {
    "1": 0.2,
    "2": 0.4,
    "3": 0.7,
    "4": 0.9,
    "5": 1.0,
    "6": 1.2,
    "7": 1.5,
    "8": 1.8,
    "9": 2.0,
}
_AVOID_CYCLE = ["auto", "on", "off"]

ai_timer = None
_ai_mods = {"ctrl": False, "shift": False, "alt": False}


def _send_ai_command(cmd):
    try:
        sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        sock.sendto(cmd.encode("utf-8"), ("127.0.0.1", AI_CMD_PORT))
        sock.close()
    except Exception as e:
        logger.error(f"Failed to send AI command via UDP: {e}")


def _send_slot_command(cmd):
    try:
        sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        sock.sendto(cmd.encode("utf-8"), ("127.0.0.1", SLOT_CMD_PORT_OUT))
        sock.close()
    except Exception as e:
        logger.error(f"Failed to send slot command via UDP: {e}")


def _send_ui_command(cmd):
    """Send a HIDE/SHOW/TOGGLE command to the uiToggle.lua GE extension."""
    try:
        sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        sock.sendto(cmd.encode("utf-8"), ("127.0.0.1", UI_TOGGLE_CMD_PORT))
        sock.close()
    except Exception as e:
        logger.error(f"Failed to send UI toggle command via UDP: {e}")


# =========================
#  AI Describer (F10 + Space)
# =========================
_describer_lock = threading.Lock()
_describer_busy = False


def _ai_describe_worker(audio_controller):
    """Background worker: hide the UI, screenshot, describe it, speak the result.

    Runs on a daemon thread so the keyboard hook returns immediately.
    """
    import ai_describer

    global _describer_busy
    try:
        settings = ai_describer_settings
        provider = ai_describer_provider or ai_describer.DEFAULT_PROVIDER
        key_cfg, model_cfg = ai_describer.config_keys_for(provider)
        api_key = settings.get(key_cfg, "")
        model = settings.get(model_cfg) or ai_describer.default_model_for(provider)
        extra_args = ai_describer.extra_args_for(provider, settings)
        toggle_ui = not ai_describer_disable_ui_toggle

        if not (api_key or "").strip():
            display = ai_describer.provider_display_name(provider)
            msg = f"No {display} API key set. Configure it in the AI Describer tab."
            ai_describer.log_error(msg)
            say(msg)
            return

        # Capture the scene, hiding the game UI around the grab so HUD/menu
        # elements don't pollute what the model sees. The UI is always restored.
        png = None
        capture_err = None
        try:
            if toggle_ui:
                _send_ui_command("HIDE")
                time.sleep(0.2)  # let the game render a UI-free frame
            png = ai_describer.capture_primary_monitor()
        except Exception as e:
            capture_err = f"Screenshot failed: {e}"
        finally:
            if toggle_ui:
                _send_ui_command("SHOW")

        if png is None:
            ai_describer.log_error(capture_err or "Screenshot failed.")
            say(capture_err or "Screenshot failed.")
            return

        say("Description in progress.", exclude_from_buffer=True)
        text, err = ai_describer.describe_image(
            png, model, api_key, provider=provider, timeout=60, **extra_args
        )
        if text:
            ai_describer.log_description(text)
            say(text)
        else:
            err = err or "Description failed."
            ai_describer.log_error(err)
            say(f"Description failed. {err}")
    except Exception as e:
        try:
            ai_describer.log_error(f"Unexpected error: {e}")
        except Exception:
            pass
        say("Description failed.")
    finally:
        with _describer_lock:
            _describer_busy = False


def _trigger_ai_describe(audio_controller):
    """Kick off an AI scene description, guarding against overlapping requests.

    A double-press while a request is in flight plays a low FM error buzz and is
    otherwise ignored (no speech), per design.
    """
    global _describer_busy
    with _describer_lock:
        if _describer_busy:
            if audio_controller is not None:
                audio_controller.trigger_describe_error_buzz()
            return
        _describer_busy = True
    threading.Thread(
        target=_ai_describe_worker, args=(audio_controller,), daemon=True
    ).start()


def _record_ai_command(vid, mode, target_id=None):
    """Record what AI command was most recently issued to a vehicle (by vehicle ID)."""
    _ai_vehicle_commands[vid] = {"mode": mode, "target_id": target_id}


# Vehicle IDs whose lights+siren we turned on for police-mode preset.
# Cleared when any other AI mode change is dispatched, so the lights stop with the role.
_police_lights_active = set()


def _set_lightbars(state, vids):
    """Queue electrics.set_lightbar_signal(state) on each vid via the LIGHTBAR command."""
    if not vids:
        return
    id_list = ",".join(str(v) for v in vids)
    _send_ai_command(f"LIGHTBAR:{state}:{id_list}")


def _clear_police_lights():
    """Turn off lights+siren on any vehicles still flagged as police-mode followers."""
    global _police_lights_active
    if not _police_lights_active:
        return
    _set_lightbars(0, list(_police_lights_active))
    _police_lights_active = set()


def _speak_status_all_response(payload):
    """Parse and speak an AI_STATUS_ALL: response received from the game."""
    if not payload or payload == "no vehicles":
        say("No vehicles tracked in game", exclude_from_buffer=True)
        return
    parts = []
    for entry in payload.split("|"):
        fields = entry.split(",", 3)
        if len(fields) < 3:
            continue
        slot_num, vname, mode = fields[0], fields[1], fields[2]
        tname = fields[3] if len(fields) > 3 else ""
        key = "0" if slot_num == "10" else slot_num
        if mode == "disabled":
            status = "AI disabled"
        elif mode == "stop":
            status = "stopped"
        elif mode in ("traffic", "random"):
            status = f"{mode} mode"
        elif mode == "chase":
            status = f"chasing {tname}" if tname else "chasing"
        elif mode == "follow":
            status = f"following {tname}" if tname else "following"
        elif mode == "flee":
            status = f"fleeing from {tname}" if tname else "fleeing"
        elif mode in ("unknown", "no response"):
            status = mode
        else:
            status = mode
        parts.append(f"Vehicle {key}, {vname}: {status}")
    if parts:
        say(". ".join(parts), exclude_from_buffer=True)
    else:
        say("No status data received", exclude_from_buffer=True)


def _speak_ai_status():
    """Read out a summary of the last commanded AI mode for every tracked vehicle."""
    with _slots_lock:
        slots = dict(_vehicle_slots)
    commands = dict(_ai_vehicle_commands)
    vid_to_name = {info["id"]: info["name"] for info in slots.values()}

    if not slots:
        say("No vehicles tracked", exclude_from_buffer=True)
        return

    parts = []
    for slot_num in sorted(slots.keys()):
        vid = slots[slot_num]["id"]
        name = slots[slot_num]["name"]
        key = "0" if slot_num == 10 else str(slot_num)
        cmd = commands.get(vid)
        if cmd is None:
            status = "no command given"
        else:
            mode = cmd["mode"]
            target_id = cmd.get("target_id")
            tname = vid_to_name.get(target_id) if target_id is not None else None
            if mode == "disabled":
                status = "AI disabled"
            elif mode == "stop":
                status = "stopped"
            elif mode in ("traffic", "random"):
                status = f"{mode} mode"
            elif mode == "chase":
                status = f"chasing {tname}" if tname else "chasing"
            elif mode == "follow":
                status = f"following {tname}" if tname else "following"
            elif mode == "flee":
                status = f"fleeing from {tname}" if tname else "fleeing"
            else:
                status = mode
        parts.append(f"Vehicle {key}, {name}: {status}")

    say(". ".join(parts), exclude_from_buffer=True)


def _reset_ai_timer():
    """Restart the F10 layer inactivity timer without closing the layer."""
    global ai_timer
    if ai_timer is not None:
        try:
            ai_timer.cancel()
        except Exception:
            pass
    ai_timer = threading.Timer(
        command_timeout_sec, lambda: _clear_ai_hook(speak_exit=True)
    )
    ai_timer.daemon = True
    ai_timer.start()


def _slot_key_label(slot):
    """Return the keyboard label for a slot number (slot 10 → '0')."""
    return "0" if slot == 10 else str(slot)


def _toggle_slot_selection(slot):
    """Toggle the selected state of a vehicle slot. Does not close the F10 layer."""
    global _selected_slots
    with _slots_lock:
        if slot not in _vehicle_slots:
            label = _slot_key_label(slot)
            say(f"Slot {label} is empty", exclude_from_buffer=True)
            return
        name = _vehicle_slots[slot]["name"]
        if slot == _target_slot:
            say(
                f"Cannot select {name}, it is the target vehicle",
                exclude_from_buffer=True,
            )
            return
        if slot in _selected_slots:
            _selected_slots.discard(slot)
            selected = False
        else:
            _selected_slots.add(slot)
            selected = True
    say(f"{name}, {'selected' if selected else 'unselected'}", exclude_from_buffer=True)


def _select_all_slots():
    """Select all non-target vehicles. Does not close the F10 layer."""
    global _selected_slots
    with _slots_lock:
        selectables = [s for s in _vehicle_slots if s != _target_slot]
        _selected_slots = set(selectables)
        names = [_vehicle_slots[s]["name"] for s in sorted(selectables)]
    if names:
        say(f"Selected: {', '.join(names)}", exclude_from_buffer=True)
    else:
        say("No vehicles to select", exclude_from_buffer=True)


def _set_target_slot(slot):
    """Set the AI target slot, with confirmation if the slot is currently selected."""
    global _target_slot, _pending_target_confirm
    with _slots_lock:
        if slot not in _vehicle_slots:
            label = _slot_key_label(slot)
            say(f"Slot {label} is empty", exclude_from_buffer=True)
            _pending_target_confirm = None
            return
        name = _vehicle_slots[slot]["name"]
        is_selected = slot in _selected_slots

    if is_selected:
        if _pending_target_confirm == slot:
            # Confirmed — set target and silently remove from selection.
            with _slots_lock:
                _target_slot = slot
                _selected_slots.discard(slot)
            _pending_target_confirm = None
            say(
                f"Target set to {name}, removed from selection",
                exclude_from_buffer=True,
            )
        else:
            _pending_target_confirm = slot
            label = _slot_key_label(slot)
            say(
                f"{name} is selected. Press CTRL+{label} again to confirm target change, any other key to cancel",
                exclude_from_buffer=True,
            )
    else:
        _pending_target_confirm = None
        with _slots_lock:
            _target_slot = slot
        say(f"Target: {name}", exclude_from_buffer=True)


def _dispatch_ai_mode(mode):
    """Issue an AI mode command to all selected vehicles, or the player vehicle if none selected."""
    _clear_police_lights()
    with _slots_lock:
        selected = set(_selected_slots)
        target_slot = _target_slot
        slots = dict(_vehicle_slots)

    if not selected:
        # Legacy single-vehicle path — targets current player vehicle via scanner.
        _send_ai_command(f"MODE:{mode}")
        # Record against slot 1 as best approximation of the player vehicle.
        if 1 in slots:
            _record_ai_command(slots[1]["id"], mode)
        return

    if mode in ("chase", "follow", "flee"):
        if target_slot is None or target_slot not in slots:
            say(
                "No target set. Use CTRL+1-0 to set a target vehicle.",
                exclude_from_buffer=True,
            )
            return
        target_id = slots[target_slot]["id"]
        ids = [slots[s]["id"] for s in sorted(selected) if s in slots]
        if not ids:
            say("No valid vehicles selected", exclude_from_buffer=True)
            return
        _send_ai_command(
            f"MULTI_MODE:{mode}:{target_id}:{','.join(str(i) for i in ids)}"
        )
        for vid in ids:
            _record_ai_command(vid, mode, target_id)
    else:
        ids = [slots[s]["id"] for s in sorted(selected) if s in slots]
        if not ids:
            say("No valid vehicles selected", exclude_from_buffer=True)
            return
        _send_ai_command(f"MULTI_MODE:{mode}:none:{','.join(str(i) for i in ids)}")
        for vid in ids:
            _record_ai_command(vid, mode)


_PRESET_DELAY_SEC = 3.0


def _run_staggered(commands):
    """Run (ai_cmd_str,) tuples sequentially with _PRESET_DELAY_SEC gaps in a daemon thread."""

    def _worker():
        for i, cmd in enumerate(commands):
            if i > 0:
                time.sleep(_PRESET_DELAY_SEC)
            _send_ai_command(cmd)

    threading.Thread(target=_worker, daemon=True).start()


def _preset_motorcade(slots, occupied):
    """Each vehicle follows the one before it in slot order."""
    if len(occupied) < 2:
        say("Motorcade needs at least 2 vehicles", exclude_from_buffer=True)
        return
    cmds = []
    for i in range(1, len(occupied)):
        follower_id = slots[occupied[i]]["id"]
        leader_id = slots[occupied[i - 1]]["id"]
        cmds.append(f"MULTI_MODE:follow:{leader_id}:{follower_id}")
        _record_ai_command(follower_id, "follow", leader_id)
    say(f"Motorcade, {len(occupied)} vehicles", exclude_from_buffer=True)
    _run_staggered(cmds)


def _preset_gangup(slots, occupied):
    """Every vehicle except slot 1 chases slot 1."""
    if 1 not in slots:
        say("Gang-up requires a vehicle in slot 1", exclude_from_buffer=True)
        return
    target_id = slots[1]["id"]
    chasers = [s for s in occupied if s != 1]
    if not chasers:
        say("Gang-up needs at least 2 vehicles", exclude_from_buffer=True)
        return
    cmds = []
    for s in chasers:
        vid = slots[s]["id"]
        cmds.append(f"MULTI_MODE:chase:{target_id}:{vid}")
        _record_ai_command(vid, "chase", target_id)
    say(
        f"Gang-up, {len(chasers)} vehicles on {slots[1]['name']}",
        exclude_from_buffer=True,
    )
    _run_staggered(cmds)


def _preset_tod(slots, occupied):
    """Chase chain: each vehicle chases the one before it in slot order."""
    if len(occupied) < 2:
        say("Tour of Destruction needs at least 2 vehicles", exclude_from_buffer=True)
        return
    cmds = []
    for i in range(1, len(occupied)):
        chaser_id = slots[occupied[i]]["id"]
        target_id = slots[occupied[i - 1]]["id"]
        cmds.append(f"MULTI_MODE:chase:{target_id}:{chaser_id}")
        _record_ai_command(chaser_id, "chase", target_id)
    say(f"Tour of Destruction, {len(occupied)} vehicles", exclude_from_buffer=True)
    _run_staggered(cmds)


def _preset_tod_spectator(slots, occupied):
    """Chase chain among slots 2+; slot 1 watches. Lead AI (slot 2) gets random mode."""
    non_p1 = [s for s in occupied if s != 1]
    if len(non_p1) < 2:
        say(
            "Tour of Destruction Spectator needs at least 3 vehicles total",
            exclude_from_buffer=True,
        )
        return
    cmds = []
    lead_id = slots[non_p1[0]]["id"]
    cmds.append(f"MULTI_MODE:random:none:{lead_id}")
    _record_ai_command(lead_id, "random")
    for i in range(1, len(non_p1)):
        chaser_id = slots[non_p1[i]]["id"]
        target_id = slots[non_p1[i - 1]]["id"]
        cmds.append(f"MULTI_MODE:chase:{target_id}:{chaser_id}")
        _record_ai_command(chaser_id, "chase", target_id)
    say(
        f"Tour of Destruction Spectator, {len(non_p1)} AI vehicles, you watch",
        exclude_from_buffer=True,
    )
    _run_staggered(cmds)


def _preset_police(slots, occupied, target_slot):
    """All vehicles except the target follow the target (the 'bad guy')."""
    if target_slot is None or target_slot not in slots:
        say("Set the bad guy target first with CTRL+1-0", exclude_from_buffer=True)
        return
    bad_guy_id = slots[target_slot]["id"]
    bad_guy_name = slots[target_slot]["name"]
    followers = [s for s in occupied if s != target_slot]
    if not followers:
        say("Police mode needs at least 2 vehicles", exclude_from_buffer=True)
        return
    cmds = []
    follower_ids = []
    for s in followers:
        vid = slots[s]["id"]
        follower_ids.append(vid)
        cmds.append(f"MULTI_MODE:follow:{bad_guy_id}:{vid}")
        _record_ai_command(vid, "follow", bad_guy_id)
    say(
        f"Police mode, {len(followers)} vehicles following {bad_guy_name}",
        exclude_from_buffer=True,
    )
    _run_staggered(cmds)
    # Trigger lights + siren on each follower. Vehicles without a sirenSound will
    # ignore state 2 (BeamNG wraps modulo 2 → 0), but police-equipped vehicles —
    # the only ones this preset is useful for — engage both.
    _set_lightbars(2, follower_ids)
    global _police_lights_active
    _police_lights_active = set(follower_ids)


def _trigger_preset(preset_num):
    with _slots_lock:
        slots = dict(_vehicle_slots)
        target_slot = _target_slot
    occupied = sorted(slots.keys())
    if not occupied:
        say("No vehicles tracked yet", exclude_from_buffer=True)
        return
    if preset_num != 5:
        _clear_police_lights()
    if preset_num == 1:
        _preset_motorcade(slots, occupied)
    elif preset_num == 2:
        _preset_gangup(slots, occupied)
    elif preset_num == 3:
        _preset_tod(slots, occupied)
    elif preset_num == 4:
        _preset_tod_spectator(slots, occupied)
    elif preset_num == 5:
        _preset_police(slots, occupied, target_slot)


def _clear_ai_hook(speak_exit: bool):
    global ai_timer, _command_context, _input_help_mode, _pending_target_confirm
    _command_context = False
    _input_help_mode = False
    _pending_target_confirm = None
    if _kb_layer == "f10":
        _kb_close_layer()
    if ai_timer is not None:
        try:
            ai_timer.cancel()
        except Exception:
            pass
    ai_timer = None
    _ai_mods["ctrl"] = _ai_mods["shift"] = _ai_mods["alt"] = False
    if speak_exit:
        say("Exit", exclude_from_buffer=True)


def _on_ai_key_press(event):
    global \
        ai_speed_limit_ms, \
        ai_avoid_mode, \
        ai_lane_driving, \
        _input_help_mode, \
        _pending_target_confirm
    if event.event_type != "down":
        return
    name = (event.name or "").lower()

    if name in ("ctrl", "control", "left ctrl", "right ctrl"):
        _ai_mods["ctrl"] = True
        return
    if name in ("shift", "left shift", "right shift"):
        _ai_mods["shift"] = True
        return
    if name in ("alt", "left alt", "right alt"):
        _ai_mods["alt"] = True
        return
    if name == "f10":
        return

    # Normalise shifted digit characters before any checks (see note below).
    _SHIFTED_DIGITS = {
        "!": "1",
        "@": "2",
        "#": "3",
        "$": "4",
        "%": "5",
        "^": "6",
        "&": "7",
        "*": "8",
        "(": "9",
        ")": "0",
    }
    is_shifted_char = name in _SHIFTED_DIGITS
    base_name = _SHIFTED_DIGITS.get(name, name)

    # If a target-change confirmation is pending, any key other than the
    # confirming CTRL+digit cancels it before normal processing continues.
    if _pending_target_confirm is not None:
        pending_label = _slot_key_label(_pending_target_confirm)
        if not (_ai_mods["ctrl"] and base_name == pending_label):
            _pending_target_confirm = None
            say("Target change cancelled", exclude_from_buffer=True)
            # Fall through to process the current key normally.

    # SHIFT+1-0: toggle vehicle slot selection (layer stays open).
    if base_name in "1234567890" and (
        (_ai_mods["shift"] and not _ai_mods["ctrl"]) or is_shifted_char
    ):
        slot = 10 if base_name == "0" else int(base_name)
        _toggle_slot_selection(slot)
        return

    # CTRL+1-0: set AI target slot (layer stays open).
    if (
        base_name in "1234567890"
        and _ai_mods["ctrl"]
        and not _ai_mods["shift"]
        and not is_shifted_char
    ):
        slot = 10 if base_name == "0" else int(base_name)
        _set_target_slot(slot)
        return

    # ALT+1-5: fire a preset AI configuration (layer closes after triggering).
    if (
        name in "12345"
        and _ai_mods["alt"]
        and not _ai_mods["ctrl"]
        and not _ai_mods["shift"]
    ):
        _trigger_preset(int(name))
        _clear_ai_hook(speak_exit=False)
        return

    # Toggle input help mode with ? (shift+/)
    if name in ("/", "?"):
        _input_help_mode = not _input_help_mode
        if _input_help_mode:
            say("Input help on", exclude_from_buffer=True)
            if ai_timer is not None:
                try:
                    ai_timer.cancel()
                except Exception:
                    pass
        else:
            say("Input help off", exclude_from_buffer=True)
            _clear_ai_hook(speak_exit=False)
        return

    # In help mode: speak what the key does, don't execute
    if _input_help_mode:
        key = (name, _ai_mods["ctrl"], _ai_mods["shift"], _ai_mods["alt"])
        # Also match "add"/"subtract" variants
        if name == "add":
            key = ("+", False, False, False)
        elif name == "subtract":
            key = ("-", False, False, False)
        desc = _F10_HELP.get(key)
        if desc:
            say(desc, exclude_from_buffer=True)
        else:
            say("No command", exclude_from_buffer=True)
        return

    if name == "d":
        say("AI disabled", exclude_from_buffer=True)
        _dispatch_ai_mode("disabled")
    elif name == "t":
        say("Traffic mode", exclude_from_buffer=True)
        _dispatch_ai_mode("traffic")
    elif name == "r":
        say("Random mode", exclude_from_buffer=True)
        _dispatch_ai_mode("random")
    elif name == "s":
        say("AI stop", exclude_from_buffer=True)
        _dispatch_ai_mode("stop")
    elif name == "c":
        say("Chase mode", exclude_from_buffer=True)
        _dispatch_ai_mode("chase")
    elif name == "f":
        say("Follow mode", exclude_from_buffer=True)
        _dispatch_ai_mode("follow")
    elif name == "e":
        say("Flee mode", exclude_from_buffer=True)
        _dispatch_ai_mode("flee")
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
    elif name == "a" and _ai_mods["ctrl"]:
        _select_all_slots()
        return  # layer stays open; no AI command issued
    elif name == "n" and _ai_mods["ctrl"]:
        with _slots_lock:
            _selected_slots.clear()
        say("Selection cleared", exclude_from_buffer=True)
        return
    elif name == "a":
        idx = _AVOID_CYCLE.index(ai_avoid_mode) if ai_avoid_mode in _AVOID_CYCLE else 0
        ai_avoid_mode = _AVOID_CYCLE[(idx + 1) % len(_AVOID_CYCLE)]
        say(f"Avoid cars {ai_avoid_mode}", exclude_from_buffer=True)
        _send_ai_command(f"AVOID:{ai_avoid_mode}")
    elif name == "l":
        ai_lane_driving = not ai_lane_driving
        say(
            f"Lane driving {'on' if ai_lane_driving else 'off'}",
            exclude_from_buffer=True,
        )
        _send_ai_command(f"LANE:{'on' if ai_lane_driving else 'off'}")
    elif name == "space":
        # Close the AI layer and kick off an AI scene description. The pipeline
        # runs on its own daemon thread so this hook returns immediately.
        _clear_ai_hook(speak_exit=False)
        _trigger_ai_describe(audio_controller_ref)
        return
    else:
        _clear_ai_hook(speak_exit=False)
        return

    _clear_ai_hook(speak_exit=False)


def _start_ai_capture():
    global ai_timer
    _clear_ai_hook(speak_exit=False)
    _kb_open_layer("f10")
    ai_timer = None  # F10 layer has no timeout; stays open until a command is issued.


# --- Cursor warping into the game viewport -----------------------------------
# SNAP projects a world position onto BeamNG's render viewport. Those
# coordinates are relative to the top-left of the game's client area and are in
# physical render pixels. SetCursorPos wants desktop coordinates, and beamtel is
# a DPI-unaware process, so Windows virtualises whatever we pass it. Sending the
# viewport figure straight through therefore only lands correctly on a
# fullscreen game, on the primary monitor, at 100% scaling.

_user32 = ctypes.windll.user32
# DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2
_DPI_CTX_PER_MONITOR_V2 = ctypes.c_void_p(-4)
_WNDENUMPROC = ctypes.WINFUNCTYPE(
    ctypes.c_bool, ctypes.wintypes.HWND, ctypes.wintypes.LPARAM
)


def _set_thread_dpi_aware():
    """Mark the calling thread per-monitor DPI aware; return the old context.

    Scoped to the thread rather than the process on purpose: making the whole
    process DPI aware would change how the wx windows are scaled, and this only
    needs to be true for the moment we read window geometry and move the cursor.
    Returns None on Windows older than 1607, where there is nothing to do and
    nothing to restore.
    """
    fn = getattr(_user32, "SetThreadDpiAwarenessContext", None)
    if fn is None:
        return None
    try:
        fn.restype = ctypes.c_void_p
        fn.argtypes = [ctypes.c_void_p]
        return fn(_DPI_CTX_PER_MONITOR_V2)
    except Exception:
        return None


def _restore_thread_dpi(previous):
    if previous is None:
        return
    try:
        _user32.SetThreadDpiAwarenessContext(previous)
    except Exception:
        pass


_GAME_WINDOW_CLASS = "GameEngineMainWindow"
_PROCESS_QUERY_LIMITED_INFORMATION = 0x1000


def _window_process_name(hwnd):
    """Lowercase image name of the process owning hwnd, or ""."""
    try:
        pid = ctypes.wintypes.DWORD()
        _user32.GetWindowThreadProcessId(hwnd, ctypes.byref(pid))
        if not pid.value:
            return ""
        kernel32 = ctypes.windll.kernel32
        handle = kernel32.OpenProcess(
            _PROCESS_QUERY_LIMITED_INFORMATION, False, pid.value
        )
        if not handle:
            return ""
        try:
            size = ctypes.wintypes.DWORD(260)
            buf = ctypes.create_unicode_buffer(size.value)
            if not kernel32.QueryFullProcessImageNameW(
                handle, 0, buf, ctypes.byref(size)
            ):
                return ""
            return os.path.basename(buf.value).lower()
        finally:
            kernel32.CloseHandle(handle)
    except Exception:
        return ""


def _find_beamng_window():
    """Handle of the game's render window, or None.

    Not GetForegroundWindow: SNAP can be issued while the beamtel window has
    focus, and then the foreground window is the wrong one.

    Not the window title either. An Explorer window sitting on the game folder
    is called "BeamNG.drive - File Explorer" and matches just as well, and
    warping the cursor into that would be worse than not warping at all. Match
    the render window's class, and fall back to a title match only when the
    owning process is actually the game.
    """
    exact = []
    fallback = []

    def _cb(hwnd, _lparam):
        try:
            if not _user32.IsWindowVisible(hwnd):
                return True
            cls = ctypes.create_unicode_buffer(256)
            _user32.GetClassNameW(hwnd, cls, 256)
            if cls.value == _GAME_WINDOW_CLASS:
                exact.append(hwnd)
                return False
            length = _user32.GetWindowTextLengthW(hwnd)
            if length <= 0:
                return True
            buf = ctypes.create_unicode_buffer(length + 1)
            _user32.GetWindowTextW(hwnd, buf, length + 1)
            if "BeamNG" in buf.value and _window_process_name(hwnd).startswith(
                "beamng"
            ):
                fallback.append(hwnd)
        except Exception:
            pass
        return True

    try:
        _user32.EnumWindows(_WNDENUMPROC(_cb), 0)
    except Exception:
        logger.exception("EnumWindows failed while looking for the game window")
    if exact:
        return exact[0]
    return fallback[0] if fallback else None


def warp_cursor_to_viewport(view_x, view_y, view_w=0, view_h=0):
    """Move the cursor to a point expressed in BeamNG viewport pixels."""
    previous = _set_thread_dpi_aware()
    try:
        x, y = int(view_x), int(view_y)
        hwnd = _find_beamng_window()
        if hwnd:
            rect = ctypes.wintypes.RECT()
            if _user32.GetClientRect(hwnd, ctypes.byref(rect)):
                client_w = rect.right - rect.left
                client_h = rect.bottom - rect.top
                # The render resolution can differ from the client area, e.g.
                # windowed at a non-native size or with a resolution scale set.
                if view_w > 0 and view_h > 0 and client_w > 0 and client_h > 0:
                    x = int(round(x * client_w / float(view_w)))
                    y = int(round(y * client_h / float(view_h)))
            origin = ctypes.wintypes.POINT(0, 0)
            if _user32.ClientToScreen(hwnd, ctypes.byref(origin)):
                x += origin.x
                y += origin.y
        else:
            logger.warning(
                "No BeamNG window found; warping to raw viewport coordinates."
            )
        _user32.SetCursorPos(x, y)
    except Exception as e:
        logger.error(f"Cursor warp failed: {e}")
    finally:
        _restore_thread_dpi(previous)


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
        logger.info(
            "Command mode disabled (keyboard module not available / not elevated)."
        )
        return

    _start_kb_worker()

    # Force the speech library load now. on_f9/on_f10 speak the layer prompt
    # before installing the layer hook, and they run on keyboard's event-
    # processing thread; if the first utterance of the session had to load the
    # native library there, the hook would go in late and the user's next key
    # would reach the game instead of the layer.
    if speech.init() is None:
        logger.warning("Speech unavailable at hotkey install; speech may be degraded.")

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
        # Revalidate slots so deleted/renamed vehicles don't linger in the
        # list the user is about to address with Ctrl+/Shift+digit. Lua
        # responds with a fresh SLOTS: packet within a frame or two, well
        # before the user can press the next key.
        _send_slot_command("SLOT_VALIDATE")
        say("AI?", interrupt=True, exclude_from_buffer=True)
        _start_ai_capture()

    keyboard.add_hotkey("f9", on_f9, suppress=False)
    keyboard.add_hotkey("f10", on_f10, suppress=False)


# =========================
#  Telemetry loop
# =========================
def telemetry_loop(audio_controller, host="0.0.0.0", port=4444, stop_event=None):
    global \
        protocol_mode, \
        last_pos_x, \
        last_pos_y, \
        last_pos_z, \
        last_yaw_rad, \
        last_roll_rad, \
        last_pitch_rad, \
        last_up_z
    global \
        last_heading, \
        last_click_heading_deg, \
        compass_click_counter, \
        last_announced_compass_idx, \
        last_compass_ts, \
        inverted, \
        inverted_announced
    global \
        last_speed_ms, \
        last_rpm, \
        last_fuel, \
        last_turbo, \
        last_engtemp, \
        last_oiltemp, \
        last_oil_pressure, \
        last_rpm_max, \
        last_turbo_max
    global \
        last_throttle, \
        last_brake, \
        last_clutch, \
        last_air_pressure, \
        last_air_pressure_max, \
        last_clutch_temp, \
        last_g_lat, \
        last_g_lon
    global \
        last_tire_pressure_fl, \
        last_tire_pressure_fr, \
        last_tire_pressure_rl, \
        last_tire_pressure_rr
    global last_tire_temp_fl, last_tire_temp_fr, last_tire_temp_rl, last_tire_temp_rr
    global \
        last_brake_temp_fl, \
        last_brake_temp_fr, \
        last_brake_temp_rl, \
        last_brake_temp_rr
    global \
        last_signal_left_input, \
        last_signal_right_input, \
        last_hazard_enabled, \
        last_lightbar, \
        last_fog
    global last_gear_byte, last_gear_str, last_bucket, last_speed_announce_ts
    global _telemetry_baseline_pending
    global drift_alert_active, last_drift_check_ts, drift_baseline_heading, drift_pan_direction  # NEW
    global drift_rate_val  # NEW
    global \
        _ls_prev_speed_ms, \
        _ls_prev_speed_ts, \
        _ls_steady_start_ts
    global \
        last_ground_speed_ms, \
        _ms_last_rx_ts, \
        _ls_accel_smooth, \
        _ls_ms_prev_speed_ms, \
        _ls_ms_prev_ts, \
        _ls_stopped, \
        _ls_was_moving, \
        _ls_diag_last_ts, \
        _ls_slip_smooth, \
        _ls_slip_since_ts, \
        _ls_slip_prev_ts
    global coord_target_bearing, _last_coord_bearing_ts
    global _ext_version_warned
    global \
        last_implement_flags, \
        last_implement_edge_height, \
        last_implement_min_clearance, \
        last_implement_tilt_deg, \
        last_implement_tilt_percent, \
        last_implement_lift, \
        last_implement_activity, \
        last_articulation_deg
    drift_rate_val = 0.0

    # Drift state (sampled per telemetry packet, low-pass filtered)
    drift_rate_signed = 0.0
    drift_below_since = 0.0
    prev_drift_sample_ts = 0.0
    prev_drift_sample_heading = 0.0

    global \
        compass_highlight_enabled, \
        compass_highlight_nth_click, \
        compass_click_interval_deg

    if stop_event is None:
        stop_event = STOP

    logger.info(f"Telemetry mode set to: {protocol_mode.upper()}")
    logger.info(f"Listening for telemetry (UDP) on {host}:{port} ...")

    (
        last_lowbeam_on,
        last_highbeam_on,
        last_l_signal_on,
        last_r_signal_on,
        last_hazards_on,
    ) = False, False, False, False, False
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
                continue
            except OSError:
                if stop_event.is_set():
                    break
                raise

            if not data:
                continue

            if len(data) >= 4 and data[:4] == MS_MAGIC:
                if len(data) >= MS_SIZE:
                    try:
                        ms = struct.unpack(MS_FORMAT, data[:MS_SIZE])
                        posX, posY, posZ, velX, velY, upZ, rollRad, pitchRad, yawPos = (
                            ms[1],
                            ms[2],
                            ms[3],
                            ms[4],
                            ms[5],
                            ms[12],
                            ms[13],
                            ms[14],
                            ms[15],
                        )
                    except Exception:
                        continue
                    with state_lock:
                        # Ground truth speed: horizontal component only, so driving down
                        # a slope doesn't read as extra speed.
                        ground_ms = math.hypot(velX, velY)
                        last_ground_speed_ms = ground_ms
                        _ms_last_rx_ts = now

                        # Signed longitudinal acceleration by differentiating ground
                        # speed. Deriving it this way (rather than from the packet's
                        # accXYZ, which is a vehicle-local accelerometer reading) keeps
                        # the sign unambiguous: speed rising means speeding up.
                        ms_dt = now - _ls_ms_prev_ts
                        if _ls_ms_prev_ts <= 0.0 or ms_dt > LS_ACCEL_MAX_DT:
                            # First sample, or a gap in the stream — a vehicle reload
                            # (part change, respawn) halts the vehicle VM for well over
                            # a second. Re-baseline instead of differentiating across
                            # the gap, and never leave the timestamp stale or the EMA
                            # would stop updating for the rest of the session.
                            _ls_ms_prev_speed_ms = ground_ms
                            _ls_ms_prev_ts = now
                            _ls_accel_smooth = 0.0
                        elif ms_dt >= LS_ACCEL_MIN_DT:
                            raw_accel = (ground_ms - _ls_ms_prev_speed_ms) / ms_dt
                            alpha = 1.0 - math.exp(-ms_dt / LS_ACCEL_TAU_S)
                            _ls_accel_smooth += alpha * (raw_accel - _ls_accel_smooth)
                            _ls_ms_prev_speed_ms = ground_ms
                            _ls_ms_prev_ts = now
                        # else: gap shorter than LS_ACCEL_MIN_DT, keep accumulating

                        last_pos_x, last_pos_y, last_pos_z = posX, posY, posZ
                        last_yaw_rad, last_roll_rad, last_pitch_rad, last_up_z = (
                            yawPos,
                            rollRad,
                            pitchRad,
                            upZ,
                        )
                        heading = yaw_to_heading_deg(yawPos)

                        # Drift calculation. Sampled every packet (~60 Hz) and
                        # low-pass filtered, instead of decimated to 10 Hz: the
                        # raw finite difference over a 100 ms window turns yaw
                        # quantisation into large rate steps, which the audio
                        # then rendered as a staircase.
                        if drift_mode_active:
                            if prev_drift_sample_ts == 0.0:
                                prev_drift_sample_ts = now
                                prev_drift_sample_heading = heading

                            dt = now - prev_drift_sample_ts
                            if dt >= 0.004:  # guard against dt ~= 0 blowing up the rate
                                d_head = heading - prev_drift_sample_heading
                                if d_head > 180.0:
                                    d_head -= 360.0
                                elif d_head < -180.0:
                                    d_head += 360.0

                                rate = d_head / dt
                                beta = 1.0 - math.exp(-dt / DRIFT_SMOOTH_TAU)
                                drift_rate_signed += beta * (rate - drift_rate_signed)
                                drift_rate_val = abs(drift_rate_signed)

                                # Only update direction if significant drift to avoid noise
                                if drift_rate_val > DRIFT_PAN_MIN_RATE:
                                    drift_pan_direction = (
                                        1.0 if drift_rate_signed > 0 else -1.0
                                    )

                                prev_drift_sample_heading = heading
                                prev_drift_sample_ts = now
                        else:
                            drift_rate_val = 0.0
                            drift_rate_signed = 0.0
                            prev_drift_sample_ts = 0.0
                            prev_drift_sample_heading = 0.0

                        last_heading = heading

                        # MODIFIED: Simplified click-counting logic
                        delta_heading = heading - last_click_heading_deg
                        if delta_heading > 180.0:
                            delta_heading -= 360.0
                        if delta_heading < -180.0:
                            delta_heading += 360.0

                        if abs(delta_heading) >= compass_click_interval_deg:
                            compass_click_counter += 1
                            pitch_mult = 1.0 + 0.25 * math.cos(yawPos)

                            # Check if this click should be a highlight
                            if (
                                compass_highlight_enabled
                                and compass_click_counter >= compass_highlight_nth_click
                            ):
                                audio_controller.trigger_compass_highlight(
                                    heading, pitch_mult * 1.5
                                )
                                compass_click_counter = 0  # Reset counter
                            else:
                                audio_controller.trigger_compass_click(
                                    heading, pitch_mult
                                )

                            # Snap to the nearest interval boundary
                            num_intervals = round(heading / compass_click_interval_deg)
                            last_click_heading_deg = (
                                num_intervals * compass_click_interval_deg
                            ) % 360.0

                            # Announce compass name when the snapped heading lands on a
                            # named direction (a 45° multiple). Fires in sync with the
                            # click rather than from a separate zone-entry check.
                            snapped = last_click_heading_deg
                            compass_match = -1
                            for i, target in enumerate(COMPASS_NAMES):
                                target_deg = i * 45.0
                                diff = abs(
                                    (snapped - target_deg + 180.0) % 360.0 - 180.0
                                )
                                if diff < 0.5:
                                    compass_match = i
                                    break

                            if (
                                compass_match != -1
                                and compass_match != last_announced_compass_idx
                            ):
                                if (now - last_compass_ts) >= compass_min_interval:
                                    say(
                                        COMPASS_NAMES[compass_match],
                                        exclude_from_buffer=True,
                                    )
                                    last_compass_ts = now
                                last_announced_compass_idx = compass_match
                            elif compass_match == -1:
                                last_announced_compass_idx = -1

                        current_inverted_state = upZ < -0.6
                        if not inverted and current_inverted_state:
                            inverted, inverted_announced = True, True
                            say("Up side down")
                        elif inverted and not current_inverted_state:
                            inverted, inverted_announced = False, False
                continue

            unpacked = None
            if protocol_mode == "extended" and len(data) == EXT_SIZE:
                try:
                    unpacked = struct.unpack(EXT_FORMAT, data)
                except Exception:
                    continue
            elif protocol_mode == "extended" and len(data) == EXT_SIZE_V1:
                # Older mod half. Decode what it does send and pad the implement block, so a
                # version skew costs the loader features rather than all telemetry.
                try:
                    unpacked = (
                        struct.unpack(EXT_FORMAT_V1, data) + EXT_V1_IMPLEMENT_DEFAULTS
                    )
                except Exception:
                    continue
                if not _ext_version_warned:
                    _ext_version_warned = True
                    logger.warning(
                        f"Extended telemetry packet is {EXT_SIZE_V1} bytes, expected "
                        f"{EXT_SIZE}. The Lua mod in bng_mod/ is older than this build; "
                        f"implement (bucket/fork) telemetry will be unavailable."
                    )
            elif protocol_mode == "outgauge" and len(data) >= OG_SIZE:
                try:
                    unpacked = struct.unpack(OG_FORMAT, data[:OG_SIZE])
                except Exception:
                    continue

            if unpacked is None:
                continue

            shift_active_frame, tc_active_frame, showLights = False, False, 0

            with state_lock:
                baseline_frame = _telemetry_baseline_pending
                if baseline_frame:
                    _telemetry_baseline_pending = False
                if protocol_mode == "extended":
                    showLights = unpacked[13]
                    (
                        speed_ms,
                        rpm,
                        last_rpm_max,
                        turbo,
                        last_turbo_max,
                        engtemp,
                        fuel,
                        oil_pressure,
                        oiltemp,
                    ) = unpacked[3:12]
                    (
                        throttle,
                        brake,
                        clutch,
                        steering,
                        actual_steering,
                        steering_input,
                        air_pressure,
                        air_pressure_max,
                        clutch_temp,
                        g_lat,
                        g_lon,
                        tire_p_fl,
                        tire_p_fr,
                        tire_p_rl,
                        tire_p_rr,
                        tire_t_fl,
                        tire_t_fr,
                        tire_t_rl,
                        tire_t_rr,
                        brake_t_fl,
                        brake_t_fr,
                        brake_t_rl,
                        brake_t_rr,
                        sig_left_in,
                        sig_right_in,
                        hazard_en,
                        lightbar_raw,
                        fog_raw,
                        implement_flags,
                        implement_edge_height,
                        implement_min_clearance,
                        implement_tilt_deg,
                        implement_tilt_percent,
                        implement_lift,
                        implement_activity,
                        articulation_deg,
                    ) = unpacked[14:]
                    last_oil_pressure, last_air_pressure, last_air_pressure_max = (
                        oil_pressure,
                        air_pressure,
                        air_pressure_max,
                    )
                    last_clutch_temp, last_g_lat, last_g_lon = clutch_temp, g_lat, g_lon
                    (
                        last_tire_pressure_fl,
                        last_tire_pressure_fr,
                        last_tire_pressure_rl,
                        last_tire_pressure_rr,
                    ) = tire_p_fl, tire_p_fr, tire_p_rl, tire_p_rr
                    (
                        last_tire_temp_fl,
                        last_tire_temp_fr,
                        last_tire_temp_rl,
                        last_tire_temp_rr,
                    ) = tire_t_fl, tire_t_fr, tire_t_rl, tire_t_rr
                    (
                        last_brake_temp_fl,
                        last_brake_temp_fr,
                        last_brake_temp_rl,
                        last_brake_temp_rr,
                    ) = brake_t_fl, brake_t_fr, brake_t_rl, brake_t_rr

                    # Turn signal / hazard announcements
                    cur_left = sig_left_in > 0.5
                    cur_right = sig_right_in > 0.5
                    cur_hazard = hazard_en > 0.5
                    prev_left = last_signal_left_input
                    prev_right = last_signal_right_input
                    prev_hazard = last_hazard_enabled
                    last_signal_left_input = cur_left
                    last_signal_right_input = cur_right
                    last_hazard_enabled = cur_hazard
                    if not baseline_frame and announce_turn_signals and (
                        cur_hazard != prev_hazard
                        or cur_left != prev_left
                        or cur_right != prev_right
                    ):
                        if cur_hazard:
                            say("Hazards on")
                        elif cur_left:
                            say("Left turn signal on")
                        elif cur_right:
                            say("Right turn signal on")
                        else:
                            say("Hazards off" if prev_hazard else "Turn signals off")

                    # Lightbar announcements
                    cur_lightbar = int(lightbar_raw)
                    if cur_lightbar != last_lightbar:
                        last_lightbar = cur_lightbar
                        if baseline_frame:
                            pass
                        elif cur_lightbar == 0:
                            say("Lightbar off")
                        elif cur_lightbar == 1:
                            say("Lightbar on")
                        elif cur_lightbar == 2:
                            say("Lightbar and siren on")

                    # Fog light announcements
                    cur_fog = int(fog_raw)
                    if cur_fog != last_fog:
                        last_fog = cur_fog
                        if not baseline_frame:
                            say("Fog lights on" if cur_fog else "Fog lights off")
                else:  # outgauge
                    showLights = unpacked[13]
                    speed_ms, rpm, turbo, engtemp, fuel, oil_pressure, oiltemp = (
                        unpacked[5:12]
                    )
                    throttle, brake, clutch = unpacked[14], unpacked[15], unpacked[16]
                    steering, actual_steering, steering_input = 0.0, 0.0, 0.0
                    # OutGauge carries none of the implement block; the sentinels here are
                    # what keep the status metrics hidden and the tones silent in that mode.
                    implement_flags = 0.0
                    implement_edge_height = -1.0
                    implement_min_clearance = -1.0
                    implement_tilt_deg = 0.0
                    implement_tilt_percent = 0.0
                    implement_lift = 0.0
                    implement_activity = 0.0
                    articulation_deg = 0.0

                (
                    last_speed_ms,
                    last_rpm,
                    last_fuel,
                    last_turbo,
                    last_engtemp,
                    last_oiltemp,
                ) = speed_ms, rpm, fuel, turbo, engtemp, oiltemp
                last_throttle, last_brake, last_clutch = (
                    max(0.0, min(1.0, throttle)),
                    max(0.0, min(1.0, brake)),
                    max(0.0, min(1.0, clutch)),
                )
                last_steering, last_actual_steering, last_steering_input = (
                    steering,
                    actual_steering,
                    steering_input,
                )
                (
                    last_implement_flags,
                    last_implement_edge_height,
                    last_implement_min_clearance,
                    last_implement_tilt_deg,
                    last_implement_tilt_percent,
                    last_implement_lift,
                    last_implement_activity,
                    last_articulation_deg,
                ) = (
                    implement_flags,
                    implement_edge_height,
                    implement_min_clearance,
                    implement_tilt_deg,
                    implement_tilt_percent,
                    implement_lift,
                    implement_activity,
                    articulation_deg,
                )
                shift_active_frame, tc_active_frame = (
                    bool(showLights & DL_SHIFT),
                    bool(showLights & DL_TC),
                )

                if protocol_mode == "extended":
                    gear_str = (
                        unpacked[1].decode("utf-8", errors="ignore").strip("\x00")
                    )
                    if gear_str != last_gear_str:
                        if announce_gear and not baseline_frame:
                            phrase = extended_gear_to_phrase(gear_str)
                            if (gear_str or "").strip().upper() == "N":
                                say("neutral", exclude_from_buffer=True)
                            elif phrase not in ("unknown", "neutral"):
                                say(phrase, exclude_from_buffer=True)
                        last_gear_str = gear_str
                        _push_gear_direction(gear_str)
                else:  # outgauge
                    gear_byte = unpacked[3]
                    if gear_byte != last_gear_byte:
                        if announce_gear and not baseline_frame:
                            phrase = gear_to_phrase(gear_byte)
                            if gear_byte == NEUTRAL:
                                say("neutral", exclude_from_buffer=True)
                            elif phrase not in ("unknown", "neutral"):
                                say(phrase, exclude_from_buffer=True)
                        last_gear_byte = gear_byte
                        # OutGauge encodes reverse as byte 0, unlike the extended protocol's
                        # "R" string, so the two paths cannot share a formatter.
                        _push_gear_direction("R" if gear_byte == REVERSE else "D")

                current_bucket = get_speed_bucket(speed_ms)
                if current_bucket != last_bucket:
                    if not baseline_frame and announce_speed and now - last_speed_announce_ts >= cooldown_sec:
                        spd_val, spd_unit = fmt_speed(speed_ms)
                        say(f"{spd_val} {spd_unit}", exclude_from_buffer=True)
                        last_speed_announce_ts = now
                    last_bucket = current_bucket

            # --- Ground truth selection, shared by low speed and slip detection ---
            # MotionSim gives real chassis velocity; the telemetry speed field is
            # drivetrain-derived and collapses to zero under a locked-wheel brake. Fall
            # back to it only when the MotionSim protocol isn't running.
            ms_fresh = (now - _ms_last_rx_ts) <= LS_MS_STALE_S
            if ms_fresh:
                ground_ms = last_ground_speed_ms
                a_long = _ls_accel_smooth
            else:
                ground_ms = last_speed_ms
                dt = now - _ls_prev_speed_ts
                if _ls_prev_speed_ts <= 0.0 or dt > LS_ACCEL_MAX_DT:
                    _ls_prev_speed_ms = last_speed_ms
                    _ls_prev_speed_ts = now
                    _ls_accel_smooth = 0.0
                elif dt >= LS_ACCEL_MIN_DT:
                    raw_accel = (last_speed_ms - _ls_prev_speed_ms) / dt
                    alpha = 1.0 - math.exp(-dt / LS_ACCEL_TAU_S)
                    _ls_accel_smooth += alpha * (raw_accel - _ls_accel_smooth)
                    _ls_prev_speed_ms = last_speed_ms
                    _ls_prev_speed_ts = now
                a_long = _ls_accel_smooth

            # Standstill tracking, with hysteresis so idle jitter can't chatter the flag.
            # _ls_was_moving latches once the vehicle has genuinely driven, so a car
            # that is merely parked at startup doesn't chime.
            if ground_ms > LS_STOP_TONE_MIN_MS:
                _ls_was_moving = True
            if _ls_stopped:
                if ground_ms > LS_STOP_EXIT_MS:
                    _ls_stopped = False
            elif ground_ms < LS_STOP_ENTER_MS:
                _ls_stopped = True
                if low_speed_mode_active and _ls_was_moving:
                    audio_controller.trigger_lowspeed_stopped()
                _ls_was_moving = False

            # Low Speed Detection Logic
            ls_clicks_active = False
            ls_speed_mph = 0.0
            ls_accel = 0.0
            if low_speed_mode_active:
                current_mph = ground_ms * MPH_PER_MS

                # Steady-speed suppression: go quiet once longitudinal accel has stayed
                # inside the deadband long enough. Keying this off accel rather than a
                # speed window means a braking transient can't reset it spuriously.
                if abs(a_long) > LS_ACCEL_DEADBAND:
                    _ls_steady_start_ts = now
                steady_suppressed = (now - _ls_steady_start_ts) > LS_STEADY_HOLD_S

                # Gear check: neutral/park suppression
                in_neutral_or_park = False
                if protocol_mode == "outgauge":
                    if last_gear_byte == 1:
                        in_neutral_or_park = True
                else:
                    gs = (last_gear_str or "").strip().upper()
                    if gs in ("N", "P"):
                        in_neutral_or_park = True

                if (
                    not _ls_stopped
                    and current_mph < 25.0
                    and not in_neutral_or_park
                    and not steady_suppressed
                ):
                    ls_clicks_active = True
                    ls_speed_mph = current_mph
                    ls_accel = a_long

                if (
                    bnvda_debug_enabled()
                    and (now - _ls_diag_last_ts) >= LOWSPEED_DIAG_INTERVAL_S
                ):
                    _ls_diag_last_ts = now
                    logger.info(
                        "[LOWSPEED] src=%s ground=%.2fm/s wheel=%.2fm/s mph=%.1f "
                        "accel=%+.2f clicks=%s stopped=%s steady_supp=%s gearNP=%s "
                        "ms_age=%.2fs",
                        "motionsim" if ms_fresh else "wheel-fallback",
                        ground_ms,
                        last_speed_ms,
                        current_mph,
                        a_long,
                        ls_clicks_active,
                        _ls_stopped,
                        steady_suppressed,
                        in_neutral_or_park,
                        now - _ms_last_rx_ts,
                    )

            # --- Wheel slip detection (lockup / wheelspin) ---
            # Positive slip => wheels turning slower than the ground is moving (lockup).
            # Negative slip => wheels outrunning the ground (wheelspin).
            slip_active = False
            slip_kind = 0
            slip_mag = 0.0
            if slip_mode_active and ms_fresh:
                slip_dt = (now - _ls_slip_prev_ts) if _ls_slip_prev_ts > 0.0 else 0.0
                raw_slip = ground_ms - last_speed_ms
                if 0.0 < slip_dt <= 1.0:
                    alpha = 1.0 - math.exp(-slip_dt / SLIP_TAU_S)
                    _ls_slip_smooth += alpha * (raw_slip - _ls_slip_smooth)
                else:
                    _ls_slip_smooth = raw_slip
                _ls_slip_prev_ts = now

                threshold = max(
                    SLIP_ABS_THRESHOLD_MS, SLIP_REL_THRESHOLD * ground_ms
                )
                diverging = (
                    ground_ms > SLIP_MIN_GROUND_MS and abs(_ls_slip_smooth) > threshold
                )
                if diverging:
                    if _ls_slip_since_ts == 0.0:
                        _ls_slip_since_ts = now
                    # Require the divergence to persist so gearshifts and packet jitter
                    # don't trip it.
                    if (now - _ls_slip_since_ts) >= SLIP_SUSTAIN_S:
                        slip_active = True
                        slip_kind = -1 if _ls_slip_smooth > 0.0 else 1
                        slip_mag = abs(_ls_slip_smooth)
                else:
                    _ls_slip_since_ts = 0.0
            else:
                _ls_slip_smooth = 0.0
                _ls_slip_since_ts = 0.0
                _ls_slip_prev_ts = 0.0

            guidance_diff = 0.0
            if heading_guidance_active:
                diff = last_heading - heading_guidance_target
                if diff > 180.0:
                    diff -= 360.0
                elif diff < -180.0:
                    diff += 360.0
                guidance_diff = diff

            if coord_guidance_active and marked_coord_x is not None:
                if now - _last_coord_bearing_ts >= 1.0:
                    dx = marked_coord_x - last_pos_x
                    dy = marked_coord_y - last_pos_y
                    if dx * dx + dy * dy > 0.25:
                        coord_target_bearing = (
                            math.degrees(math.atan2(-dx, -dy)) % 360.0
                        )
                    _last_coord_bearing_ts = now

            coord_bearing_error = 0.0
            if coord_guidance_active and marked_coord_x is not None:
                diff = last_heading - coord_target_bearing
                if diff > 180.0:
                    diff -= 360.0
                elif diff < -180.0:
                    diff += 360.0
                coord_bearing_error = diff

            # NEW: Drift Detection Logic (Continuous)
            if drift_mode_active:
                # Activation Logic:
                # Start if: Rate > DRIFT_ON_RATE AND Steering < 5.0 (Near zero)
                # Stop if: Rate stays below DRIFT_OFF_RATE for DRIFT_RELEASE_HOLD
                # The separate on/off thresholds plus the release hold keep the
                # alert from chattering when the rate hovers near the threshold.
                # Note: Steering input does NOT cancel an active alert, as requested.

                if drift_alert_active:
                    if drift_rate_val < DRIFT_OFF_RATE:
                        if drift_below_since == 0.0:
                            drift_below_since = now
                        elif now - drift_below_since >= DRIFT_RELEASE_HOLD:
                            drift_alert_active = False
                            drift_below_since = 0.0
                    else:
                        drift_below_since = 0.0
                else:
                    if drift_rate_val > DRIFT_ON_RATE and abs(last_steering) < 5.0:
                        drift_alert_active = True
                        drift_below_since = 0.0
            else:
                drift_alert_active = False
                drift_below_since = 0.0

            audio_controller.update_telemetry_state(
                {
                    "shift_active": shift_active_frame,
                    "tc_active": tc_active_frame,
                    "pedal_tones_active": pedal_tones_active,
                    "last_clutch": last_clutch,
                    "last_brake": last_brake,
                    "last_throttle": last_throttle,
                    "last_steering": last_steering,
                    "last_actual_steering": last_actual_steering,
                    "last_steering_input": last_steering_input,
                    "implement_flags": last_implement_flags,
                    "implement_min_clearance": last_implement_min_clearance,
                    "implement_tilt_deg": last_implement_tilt_deg,
                    "implement_lift": last_implement_lift,
                    "implement_activity": last_implement_activity,
                    "inverted": inverted,
                    "last_roll_rad": last_roll_rad,
                    "last_pitch_rad": last_pitch_rad,
                    "guidance_active": heading_guidance_active,
                    "guidance_error_deg": guidance_diff,
                    "drift_alert_active": drift_alert_active,  # NEW
                    "drift_pan": drift_pan_direction,  # NEW
                    "drift_rate": drift_rate_val,  # NEW
                    "ls_clicks_active": ls_clicks_active,
                    "ls_speed_mph": ls_speed_mph,
                    "scan_speed_ms": speed_ms,
                    "ls_accel": ls_accel,
                    "ls_stopped": _ls_stopped,
                    "slip_active": slip_active,
                    "slip_kind": slip_kind,
                    "slip_mag": slip_mag,
                    "coord_guidance_active": coord_guidance_active,
                    "coord_guidance_error_deg": coord_bearing_error,
                }
            )

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

            if (
                oil_chime_enabled
                and oil_warn_on
                and (now - last_chime_ts) >= ALERT_INTERVAL_SEC
            ):
                audio_controller.trigger_oil_chime()
                last_chime_ts = now
    finally:
        try:
            sock.close()
        except Exception:
            pass


# =========================
#  GUI: wx log handler
# =========================


class WxLogHandler(logging.Handler):
    """Logging handler that appends messages to a wx.TextCtrl via CallAfter."""

    def __init__(self, text_ctrl):
        super().__init__()
        self._ctrl = text_ctrl

    def emit(self, record):
        try:
            msg = self.format(record)
            wx.CallAfter(self._append, msg)
        except Exception:
            pass

    def _append(self, msg):
        try:
            self._ctrl.AppendText(msg + "\n")
        except Exception:
            pass


# =========================
#  GUI: Main frame
# =========================


class BeamTelFrame(wx.Frame):
    """Unified BEAM application window with Main and Configuration tabs."""

    def __init__(self):
        super().__init__(None, title="BeamNG Accessibility", size=(700, 700))
        self.SetMinSize((600, 500))
        self._engine_thread = None

        # Accessible-console state
        self._console_history = _load_console_history()
        self._console_history_pos = len(self._console_history)  # one past the end
        self._console_search_prefix = None  # locked prefix during history navigation
        self._console_ctx_indices = []  # context index per wx.Choice item
        self._console_ctx_pending = []  # accumulates CTX lines until CTXEND
        self._console_lines = []  # full output backlog (filter shows a subset)
        self._console_cmd_lines = None  # per-command output capture (None = idle)

        global console_frame
        console_frame = self

        notebook = wx.Notebook(self)
        notebook.SetName("BEAM Tabs")

        # ---- Main tab ----
        main_panel = wx.Panel(notebook)
        main_sizer = wx.BoxSizer(wx.VERTICAL)

        self.log_text = wx.TextCtrl(
            main_panel,
            style=wx.TE_MULTILINE | wx.TE_READONLY | wx.TE_RICH2 | wx.TE_DONTWRAP,
        )
        self.log_text.SetName("Application Log")
        main_sizer.Add(self.log_text, 1, wx.EXPAND | wx.ALL, 5)

        # Log-file buttons
        log_btn_sizer = wx.BoxSizer(wx.HORIZONTAL)
        btn_app_log = wx.Button(main_panel, label="Open Application Log")
        btn_app_log.SetToolTip(f"Open {LOG_FILENAME}")
        btn_speech_log = wx.Button(main_panel, label="Open Speech Log")
        btn_speech_log.SetToolTip(f"Open {SPEECH_LOG_PATH}")
        btn_dom_log = wx.Button(main_panel, label="Open DOM Dump")
        btn_dom_log.SetToolTip(f"Open {DOM_DUMP_PATH}")
        for b in (btn_app_log, btn_speech_log, btn_dom_log):
            log_btn_sizer.Add(b, 0, wx.RIGHT, 5)
        main_sizer.Add(log_btn_sizer, 0, wx.LEFT | wx.RIGHT | wx.BOTTOM, 5)

        # Accessible developer console (created here so its controls fall between the
        # log buttons and the bottom buttons in keyboard tab order).
        console_sizer = self._build_console_section(main_panel)
        main_sizer.Add(console_sizer, 2, wx.EXPAND | wx.ALL, 5)

        # Bottom button row
        bottom_btn_sizer = wx.BoxSizer(wx.HORIZONTAL)
        btn_install_mod = wx.Button(main_panel, label="Install Mod")
        btn_install_mod.SetToolTip(
            "Install the BeamNG.drive mod and activate its screen-reader UI app."
        )
        bottom_btn_sizer.Add(btn_install_mod, 0, wx.RIGHT, 5)
        bottom_btn_sizer.AddStretchSpacer()
        btn_exit = wx.Button(main_panel, label="Exit")
        bottom_btn_sizer.Add(btn_exit)
        main_sizer.Add(
            bottom_btn_sizer, 0, wx.EXPAND | wx.LEFT | wx.RIGHT | wx.BOTTOM, 10
        )

        main_panel.SetSizer(main_sizer)

        # ---- Configuration tab ----
        from config_ui import (
            ConfigPanel,
            AIDescriberPanel,
            wrap_nav_key,
            _focusable_leaves,
            install_mod_interactive,
        )

        config_panel = ConfigPanel(notebook)
        describer_panel = AIDescriberPanel(notebook)
        # Kept so _on_close can flush their debounced saves before we tear down.
        self._config_panel = config_panel
        self._describer_panel = describer_panel

        main_panel.Bind(
            wx.EVT_NAVIGATION_KEY, lambda evt: wrap_nav_key(evt, main_panel)
        )

        notebook.AddPage(main_panel, "Main")
        notebook.AddPage(config_panel, "Configuration")
        notebook.AddPage(describer_panel, "AI Describer")

        self._notebook = notebook
        self._focusable_leaves = _focusable_leaves
        notebook.Bind(wx.EVT_NAVIGATION_KEY, self._on_notebook_nav)

        # Wire up the wx log handler so logger output appears in the text control
        wx_handler = WxLogHandler(self.log_text)
        wx_handler.setFormatter(
            logging.Formatter(
                "%(asctime)s [%(levelname)s] %(message)s", datefmt="%Y-%m-%d %H:%M:%S"
            )
        )
        logging.getLogger("bnvdahook").addHandler(wx_handler)

        # Events
        btn_app_log.Bind(wx.EVT_BUTTON, self._on_open_app_log)
        btn_speech_log.Bind(wx.EVT_BUTTON, self._on_open_speech_log)
        btn_dom_log.Bind(wx.EVT_BUTTON, self._on_open_dom_log)
        btn_install_mod.Bind(wx.EVT_BUTTON, lambda evt: install_mod_interactive(self))
        btn_exit.Bind(wx.EVT_BUTTON, lambda evt: self.Close())
        self.Bind(wx.EVT_CLOSE, self._on_close)

        self.Centre()

    # ---- Log-file openers ----

    @staticmethod
    def _open_file_if_exists(path):
        if os.path.isfile(path):
            os.startfile(path)
        else:
            wx.MessageBox(
                f"File not found:\n{path}", "Not Found", wx.OK | wx.ICON_WARNING
            )

    def _on_open_app_log(self, evt):
        self._open_file_if_exists(LOG_FILENAME)

    def _on_open_speech_log(self, evt):
        self._open_file_if_exists(SPEECH_LOG_PATH)

    def _on_open_dom_log(self, evt):
        self._open_file_if_exists(DOM_DUMP_PATH)

    # ---- Notebook keyboard navigation ----

    def _on_notebook_nav(self, evt):
        """Route Tab/Shift+Tab from the Notebook tab bar into the current page.

        wrap_nav_key (bound on each page) focuses the Notebook when Tab is
        pressed at the last control or Shift+Tab at the first.  This handler
        completes the cycle: when the Notebook itself has focus, Tab goes to
        the first leaf control in the current page and Shift+Tab goes to the
        last, so the full order is:

            ... → last control → Notebook tabs → first control → ...
        """
        if evt.IsWindowChange():
            evt.Skip()  # Ctrl+Tab: let the Notebook switch pages normally
            return
        focused = wx.Window.FindFocus()
        if focused is not self._notebook:
            evt.Skip()  # Event originated from inside a page; let it propagate
            return
        page = self._notebook.GetCurrentPage()
        leaves = self._focusable_leaves(page)
        if not leaves:
            evt.Skip()
            return
        if evt.GetDirection():  # Tab → first control in page
            leaves[0].SetFocus()
        else:  # Shift+Tab → last control in page
            leaves[-1].SetFocus()

    # ---- Accessible developer console ----

    def _build_console_section(self, parent):
        """Build the accessible developer-console controls; returns a sizer.

        Controls are parented to the StaticBox, not to ``parent``: on Windows
        that nesting is what makes a screen reader announce "Developer Console"
        when focus enters the group.  It costs nothing in tab order, because
        _focusable_leaves recurses through GetChildren() and descends into the
        box like any other container.
        """
        sb = wx.StaticBox(parent, label="Developer Console")
        box = wx.StaticBoxSizer(sb, wx.VERTICAL)

        # Context (sandbox) selector
        ctx_row = wx.BoxSizer(wx.HORIZONTAL)
        ctx_label = wx.StaticText(sb, label="Context:")
        self.console_ctx_choice = wx.Choice(
            sb, choices=["GE - Lua", "GE - TorqueScript", "CEF/UI - JS"]
        )
        self.console_ctx_choice.SetName("Console Context")
        self.console_ctx_choice.SetSelection(0)
        self._console_ctx_indices = [0, 1, 2]
        btn_refresh = wx.Button(sb, label="Refresh Contexts")
        ctx_row.Add(ctx_label, 0, wx.ALIGN_CENTER_VERTICAL | wx.RIGHT, 5)
        ctx_row.Add(self.console_ctx_choice, 1, wx.RIGHT, 5)
        ctx_row.Add(btn_refresh, 0)
        box.Add(ctx_row, 0, wx.EXPAND | wx.ALL, 3)

        # Command input
        cmd_row = wx.BoxSizer(wx.HORIZONTAL)
        cmd_label = wx.StaticText(sb, label="Command:")
        self.console_input = wx.TextCtrl(sb, style=wx.TE_PROCESS_ENTER)
        self.console_input.SetName("Console Command")
        btn_run = wx.Button(sb, label="Run")
        cmd_row.Add(cmd_label, 0, wx.ALIGN_CENTER_VERTICAL | wx.RIGHT, 5)
        cmd_row.Add(self.console_input, 1, wx.RIGHT, 5)
        cmd_row.Add(btn_run, 0)
        box.Add(cmd_row, 0, wx.EXPAND | wx.ALL, 3)

        # Output (review with the screen reader's own reading keys)
        self.console_output = wx.TextCtrl(
            sb,
            style=wx.TE_MULTILINE | wx.TE_READONLY | wx.TE_RICH2 | wx.TE_DONTWRAP,
        )
        self.console_output.SetName("Console Output")
        box.Add(self.console_output, 1, wx.EXPAND | wx.ALL, 3)

        # Output filter (applies to all output) + stream toggle + clear
        filt_row = wx.BoxSizer(wx.HORIZONTAL)
        filt_label = wx.StaticText(sb, label="Filter:")
        self.console_filter = wx.TextCtrl(sb)
        self.console_filter.SetName("Console Output Filter")
        self.console_log_check = wx.CheckBox(sb, label="Stream game log")
        btn_clear = wx.Button(sb, label="Clear Output")
        filt_row.Add(filt_label, 0, wx.ALIGN_CENTER_VERTICAL | wx.RIGHT, 5)
        filt_row.Add(self.console_filter, 1, wx.RIGHT, 5)
        filt_row.Add(self.console_log_check, 0, wx.ALIGN_CENTER_VERTICAL | wx.RIGHT, 5)
        filt_row.Add(btn_clear, 0)
        box.Add(filt_row, 0, wx.EXPAND | wx.ALL, 3)

        # Events
        self.console_input.Bind(wx.EVT_TEXT_ENTER, self._on_console_run)
        self.console_input.Bind(wx.EVT_KEY_DOWN, self._on_console_input_key)
        btn_run.Bind(wx.EVT_BUTTON, self._on_console_run)
        btn_refresh.Bind(
            wx.EVT_BUTTON, lambda evt: send_console_command("CTXLIST")
        )
        btn_clear.Bind(wx.EVT_BUTTON, self._on_console_clear)
        self.console_filter.Bind(wx.EVT_TEXT, self._on_console_filter_changed)
        self.console_log_check.Bind(wx.EVT_CHECKBOX, self._on_console_log_toggle)

        return box

    def _console_matches_filter(self, line):
        """True if `line` should be visible under the current Filter substring."""
        filt = self.console_filter.GetValue().strip().lower()
        return not filt or filt in line.lower()

    def _console_append(self, line):
        """Record a line in the backlog and show it if it matches the Filter."""
        line = line.rstrip("\n")
        self._console_lines.append(line)
        if self._console_matches_filter(line):
            self.console_output.AppendText(line + "\n")

    def _console_rerender(self):
        """Rebuild the output control from the backlog under the current Filter."""
        visible = [
            ln for ln in self._console_lines if self._console_matches_filter(ln)
        ]
        # SetValue in one shot (AppendText per line is slow for large backlogs).
        self.console_output.SetValue("\n".join(visible) + ("\n" if visible else ""))
        self.console_output.SetInsertionPointEnd()

    def _on_console_filter_changed(self, evt):
        self._console_rerender()

    def _on_console_clear(self, evt):
        self._console_lines = []
        self.console_output.SetValue("")

    def _on_console_run(self, evt):
        cmd = self.console_input.GetValue().strip()
        if not cmd:
            return
        sel = self.console_ctx_choice.GetSelection()
        if sel == wx.NOT_FOUND:
            sel = 0
        ctx_index = (
            self._console_ctx_indices[sel]
            if sel < len(self._console_ctx_indices)
            else 0
        )
        ctx_label = self.console_ctx_choice.GetString(sel)

        # Command history (skip consecutive duplicates), persisted to disk.
        if not self._console_history or self._console_history[-1] != cmd:
            self._console_history.append(cmd)
            self._console_history = self._console_history[-CONSOLE_HISTORY_MAX:]
            _save_console_history(self._console_history)
        self._console_history_pos = len(self._console_history)
        self._console_search_prefix = None  # end any prefix-search session

        self._console_append(f"[{ctx_label}] > {cmd}")
        send_console_command(f"EXEC|{ctx_index}|{cmd}")
        self.console_input.SetValue("")
        self.console_input.SetFocus()

    def _on_console_input_key(self, evt):
        """Up/Down history recall with optional prefix search (this control only).

        Whatever is in the field when navigation begins is locked as a search prefix:
        Up then walks back through only those history entries that start with it (and
        recalls nothing on a failed match). An empty field locks an empty prefix, which
        matches everything -- i.e. the plain history walk. Pressing any other key ends
        the session so the next Up re-captures the current text as a fresh prefix.
        """
        key = evt.GetKeyCode()
        if key not in (wx.WXK_UP, wx.WXK_DOWN):
            self._console_search_prefix = None
            evt.Skip()
            return

        if not self._console_history:
            return

        # Begin a navigation session: lock the current field text as the prefix.
        if self._console_search_prefix is None:
            self._console_search_prefix = self.console_input.GetValue()
            self._console_history_pos = len(self._console_history)
        prefix = self._console_search_prefix

        if key == wx.WXK_UP:
            idx = self._console_history_pos - 1
            while idx >= 0:
                if self._console_history[idx].startswith(prefix):
                    self._console_history_pos = idx
                    self.console_input.ChangeValue(self._console_history[idx])
                    self.console_input.SetInsertionPointEnd()
                    return
                idx -= 1
            # No older match: leave the field unchanged.
            return

        # WXK_DOWN -- search toward newer entries for the next prefix match.
        idx = self._console_history_pos + 1
        while idx < len(self._console_history):
            if self._console_history[idx].startswith(prefix):
                self._console_history_pos = idx
                self.console_input.ChangeValue(self._console_history[idx])
                self.console_input.SetInsertionPointEnd()
                return
            idx += 1
        # Past the newest match: restore the originally typed prefix and end the session.
        self._console_history_pos = len(self._console_history)
        self.console_input.ChangeValue(prefix)
        self.console_input.SetInsertionPointEnd()

    def _on_console_log_toggle(self, evt):
        send_console_command("LOGON" if self.console_log_check.GetValue() else "LOGOFF")

    def _rebuild_context_choice(self):
        """Replace the context dropdown contents from accumulated CTX lines."""
        if not self._console_ctx_pending:
            return
        prev_idx = None
        sel = self.console_ctx_choice.GetSelection()
        if sel != wx.NOT_FOUND and sel < len(self._console_ctx_indices):
            prev_idx = self._console_ctx_indices[sel]
        labels = [lbl for (_i, lbl) in self._console_ctx_pending]
        indices = [i for (i, _lbl) in self._console_ctx_pending]
        self.console_ctx_choice.Set(labels)
        self._console_ctx_indices = indices
        new_sel = 0
        if prev_idx is not None and prev_idx in indices:
            new_sel = indices.index(prev_idx)
        self.console_ctx_choice.SetSelection(new_sel)
        self._console_ctx_pending = []

    def on_console_message(self, text):
        """Handle one record from consoleAccessible.lua (runs on the wx thread)."""
        parts = text.split("|")
        tag = parts[0]
        if tag == "CTX":
            if len(parts) >= 3:
                try:
                    idx = int(parts[1])
                except ValueError:
                    return
                self._console_ctx_pending.append((idx, "|".join(parts[2:])))
            return
        if tag == "CTXEND":
            self._rebuild_context_choice()
            return
        if tag == "RESP":
            status = parts[1] if len(parts) > 1 else ""
            body = "|".join(parts[2:]) if len(parts) > 2 else ""
            # A RESP marks the start of one command's response; begin capturing its
            # output so a single-line result can be spoken on EXECEND. Log-stream lines
            # arrive outside this window and are never captured/spoken.
            self._console_cmd_lines = []
            if status == "error":
                self._console_append(f"  error: {body}")
                self._console_cmd_lines.append(f"error: {body}" if body else "error")
            elif status == "ok":
                if body:
                    self._console_append(f"  = {body}")
                    self._console_cmd_lines.append(body)
            elif status == "queued":
                self._console_append("  (queued)")
                self._console_cmd_lines.append("queued")
            return
        if tag == "OUT":
            content = "|".join(parts[1:])
            self._console_append("  " + content)
            if self._console_cmd_lines is not None:
                spoken = content[2:] if content.startswith("= ") else content
                self._console_cmd_lines.append(spoken)
            return
        if tag == "EXECEND":
            # Speak the result only if the whole command produced exactly one line.
            if self._console_cmd_lines is not None:
                lines = [ln for ln in self._console_cmd_lines if ln.strip()]
                if len(lines) == 1:
                    say(lines[0].strip())
                self._console_cmd_lines = None
            return
        if tag == "LOG":
            if len(parts) >= 5:
                lvl, origin, msg = parts[2], parts[3], "|".join(parts[4:])
            else:
                lvl, origin, msg = "", "", "|".join(parts[1:])
            self._console_append(f"  [{lvl} {origin}] {msg}")
            return

    # ---- Shutdown ----

    def _on_close(self, evt):
        # The Configuration tab debounces its writes by two seconds (as does the
        # AI Describer tab's base-URL field), so closing right after an edit
        # would drop it silently. Commit them first.
        for panel in (self._config_panel, self._describer_panel):
            try:
                panel.flush_pending_save()
            except Exception:
                pass
        STOP.set()
        if self._engine_thread and self._engine_thread.is_alive():
            self._engine_thread.join(timeout=2.0)
        self.Destroy()


# =========================
#  Engine (background)
# =========================


def _broadcast_ui_settings():
    """Push the UI-side settings to the mod's JS runtime.

    Called both on config change and in reply to the runtime's settings_request,
    since a broadcast with no client attached is silently dropped.
    """
    with state_lock:
        payload = {"type": "settings", "ui_nav_hold_suppression": ui_nav_hold_suppression}
    try:
        broadcast(payload)
    except Exception as e:
        logger.error(f"Failed to broadcast UI settings: {e}")


def _apply_live_config(audio_controller):
    """Load config from disk and apply all settings to the running engine."""
    global UNITS_MODE, oil_chime_enabled
    global \
        protocol_mode, \
        compass_highlight_enabled, \
        compass_highlight_nth_click, \
        compass_click_interval_deg
    global announce_turn_signals, announce_speed, speed_announce_interval, announce_gear
    global scanner_distance_callout_enabled, scanner_distance_callout_interval
    global announce_implement_proximity
    global ai_describer_provider, ai_describer_settings, ai_describer_disable_ui_toggle
    global ui_nav_hold_suppression
    try:
        cfg = load_config()
    except Exception as e:
        logger.warning(f"Config reload failed: {e}")
        return
    with state_lock:
        UNITS_MODE = cfg.get("units", "imperial")
        oil_chime_enabled = cfg.get("oil_chime_enabled", True)
        protocol_mode = cfg.get("telemetry_protocol", "outgauge")
        compass_highlight_enabled = cfg.get("compass_highlight_enabled", True)
        compass_highlight_nth_click = int(cfg.get("compass_highlight_nth_click", 6))
        compass_click_interval_deg = float(cfg.get("compass_click_interval", 15.0))
        announce_turn_signals = cfg.get("announce_turn_signals", True)
        announce_speed = cfg.get("announce_speed", True)
        speed_announce_interval = cfg.get("speed_announce_interval", 25)
        announce_gear = cfg.get("announce_gear", True)
        scanner_distance_callout_enabled = cfg.get("scanner_distance_callout_enabled", False)
        scanner_distance_callout_interval = cfg.get("scanner_distance_callout_interval", 10)
        announce_implement_proximity = bool(cfg.get("implement_proximity_speech", True))
        ui_nav_hold_suppression = bool(cfg.get("ui_nav_hold_suppression", True))
        ai_describer_provider = cfg.get("ai_describer_provider", "gemini")
        ai_describer_settings = {
            k: cfg.get(k, DEFAULT_CONFIG.get(k))
            for k in DEFAULT_CONFIG
            if k.startswith("ai_describer_")
        }
        ai_describer_disable_ui_toggle = cfg.get("ai_describer_disable_ui_toggle", False)
    if audio_controller is not None:
        audio_controller.apply_config(cfg)
    # Outside state_lock: rebuilding the backend can take long enough that the
    # telemetry loop and audio callback would notice the stall.
    speech.configure(cfg)
    _broadcast_ui_settings()
    logger.info("Configuration reloaded.")


def _config_watcher(stop_event):
    """Daemon thread: polls CONFIG_PATH for mtime changes and hot-reloads settings."""
    try:
        last_mtime = os.path.getmtime(CONFIG_PATH)
    except OSError:
        last_mtime = 0.0
    while not stop_event.wait(1.0):
        try:
            mtime = os.path.getmtime(CONFIG_PATH)
        except OSError:
            continue
        if mtime != last_mtime:
            last_mtime = mtime
            _apply_live_config(audio_controller_ref)


def _run_engine():
    """Telemetry engine — runs in a daemon thread while the GUI event loop owns the main thread."""
    cfg = load_config()
    global UNITS_MODE, oil_chime_enabled
    global announce_turn_signals, announce_speed, speed_announce_interval, announce_gear
    global scanner_distance_callout_enabled, scanner_distance_callout_interval
    global announce_implement_proximity
    global ui_nav_hold_suppression
    UNITS_MODE = cfg.get("units", "imperial")
    oil_chime_enabled = cfg.get("oil_chime_enabled", True)
    announce_turn_signals = cfg.get("announce_turn_signals", True)
    announce_speed = cfg.get("announce_speed", True)
    speed_announce_interval = cfg.get("speed_announce_interval", 25)
    announce_gear = cfg.get("announce_gear", True)
    scanner_distance_callout_enabled = cfg.get("scanner_distance_callout_enabled", False)
    scanner_distance_callout_interval = cfg.get("scanner_distance_callout_interval", 10)
    announce_implement_proximity = bool(cfg.get("implement_proximity_speech", True))
    ui_nav_hold_suppression = bool(cfg.get("ui_nav_hold_suppression", True))

    if speech.init(cfg) is None:
        logger.warning("No speech backend available; callouts will be silent.")
    else:
        logger.info(f"Speech backend: {speech.describe_capabilities()}")

    global audio_controller_ref
    audio_controller = AudioController(logger)
    audio_controller.apply_config(cfg)
    audio_controller.start()
    audio_controller_ref = audio_controller

    _apply_live_config(audio_controller)

    watcher_thread = threading.Thread(target=_config_watcher, args=(STOP,), daemon=True)
    watcher_thread.start()

    hrir_path = os.path.join(HERE, "hrtf_kemar_horizontal.npz")
    audio_controller.load_hrtf(hrir_path)

    _ws_thread, _ws_stop = None, lambda: None
    try:
        _ws_thread, _ws_stop = start_server_in_thread(
            lambda text, interrupt=True: say(text, interrupt, source="ui_bridge")
        )
    except Exception as _e:
        logger.error(f"Failed to start NVDA WS/HTTP bridge: {_e}")

    register_dom_dump_callback(_on_dom_dump_received)
    register_loading_state_callback(_on_loading_state_changed)
    register_settings_request_callback(_broadcast_ui_settings)

    if cfg.get("launch_beamng", False):
        try:
            import subprocess

            result = subprocess.run(
                ["tasklist", "/FI", "IMAGENAME eq BeamNG.drive.x64.exe", "/NH"],
                capture_output=True,
                text=True,
                timeout=5,
            )
            if "BeamNG.drive.x64.exe" in result.stdout:
                logger.info("BeamNG.drive is already running, skipping launch.")
            else:
                import winreg

                steam_path = None
                for reg_path in (
                    r"SOFTWARE\Valve\Steam",
                    r"SOFTWARE\WOW6432Node\Valve\Steam",
                ):
                    try:
                        with winreg.OpenKey(winreg.HKEY_LOCAL_MACHINE, reg_path) as k:
                            steam_path = winreg.QueryValueEx(k, "InstallPath")[0]
                        break
                    except OSError:
                        pass
                if not steam_path:
                    raise RuntimeError("Steam installation not found in registry.")
                steam_exe = os.path.join(steam_path, "steam.exe")
                renderer = cfg.get("beamng_renderer", "d3d")
                gfx_flag = "vk" if renderer == "vulkan" else "dx11"
                subprocess.Popen([steam_exe, "-applaunch", "284160", "-gfx", gfx_flag])
                logger.info(f"Launched BeamNG.drive via Steam (-applaunch, {gfx_flag}).")
        except Exception as e:
            logger.warning(f"Failed to launch BeamNG.drive: {e}")

    install_hotkeys(audio_controller)

    ui_thread = threading.Thread(target=ui_listener, args=(STOP,), daemon=True)
    ui_thread.start()

    scanner_thread = threading.Thread(
        target=scanner_listener, args=(audio_controller, STOP), daemon=True
    )
    scanner_thread.start()

    callout_thread = threading.Thread(
        target=scanner_callout_thread_fn, args=(STOP,), daemon=True
    )
    callout_thread.start()

    obstacle_thread = threading.Thread(
        target=obstacle_listener, args=(audio_controller, STOP), daemon=True
    )
    obstacle_thread.start()

    road_thread = threading.Thread(
        target=road_listener, args=(audio_controller, STOP), daemon=True
    )
    road_thread.start()

    implement_thread = threading.Thread(
        target=implement_listener, args=(audio_controller, STOP), daemon=True
    )
    implement_thread.start()

    camera_thread = threading.Thread(
        target=camera_listener, args=(audio_controller, STOP), daemon=True
    )
    camera_thread.start()

    nodegrab_thread = threading.Thread(
        target=nodegrab_listener, args=(audio_controller, STOP), daemon=True
    )
    nodegrab_thread.start()

    clickspot_thread = threading.Thread(
        target=clickspot_listener, args=(audio_controller, STOP), daemon=True
    )
    clickspot_thread.start()

    bindings_thread = threading.Thread(
        target=vehicle_bindings_listener, args=(STOP,), daemon=True
    )
    bindings_thread.start()
    # Ask for the current vehicle's bindings, in case beamtel started after the
    # game had already spawned one and the load-time push was missed.
    _send_vehicle_bindings_cmd("REQUEST")

    slot_thread = threading.Thread(target=slot_listener, args=(STOP,), daemon=True)
    slot_thread.start()
    # Request initial slot list from Lua once the thread is running.
    threading.Timer(2.0, lambda: _send_slot_command("SLOT_STATUS")).start()

    console_thread = threading.Thread(
        target=console_listener, args=(STOP,), daemon=True
    )
    console_thread.start()
    # Request the initial context list from Lua once the listener is bound.
    threading.Timer(2.0, lambda: send_console_command("CTXLIST")).start()

    try:
        import vehicle_spawner as _vs_module
        global _vehicle_spawner
        _vehicle_spawner = _vs_module

        def _get_spawner_slots():
            with _slots_lock:
                return {k: dict(v) for k, v in _vehicle_slots.items()}

        _vehicle_spawner.init(
            say,
            _is_beamng_focused,
            logger,
            get_slots_fn=_get_spawner_slots,
            # Must release EVERY arrow owner, not just the virtual browser. Leaving status
            # mode hooked while the spawner also hooks the arrows is what produced the
            # permanently-dead arrow keys described in _hook_suppressed.
            close_others_fn=lambda: release_arrow_owners("vehicle spawner opened"),
            ping_fn=audio_controller.trigger_placement_ping,
        )
        _vehicle_spawner.start(STOP)
    except Exception as e:
        logger.error(f"Failed to start vehicle spawner: {e}")

    try:
        telemetry_loop(audio_controller=audio_controller, port=4444, stop_event=STOP)
    finally:
        STOP.set()
        audio_controller.stop()
        if _ws_stop:
            _ws_stop()


# =========================
#  Entry point
# =========================


def main():
    app = wx.App(False)
    frame = BeamTelFrame()
    frame.Show()

    engine = threading.Thread(target=_run_engine, name="beamtel-engine", daemon=True)
    engine.start()
    frame._engine_thread = engine

    app.MainLoop()

    # Ensure clean shutdown after GUI closes
    STOP.set()


if __name__ == "__main__":
    main()

# --- END OF beamtel.py ---
