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
    register_accessibility_action_callback,
    register_challenge_event_callback,
    register_screen_context_callback,
    register_page_text_callback,
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
from types import SimpleNamespace

import wx

from bnh_logger import get_logger, LOG_FILENAME
from audio import AudioController, DOCK_RAMP_MAX_RANGE_M, SCAN_FAMILY_CODES
from road_guidance import RoadGuidanceFeed, junction_phrase, parse_r2_packet
from road_diagnostics import RoadDiagnosticRecorder
from challenge_results import (
    HillClimbChallengeRecorder,
    completion_speech as hill_climb_completion_speech,
)
from route_beacon import (
    parse_route_packet,
    relative_bearing,
    route_beacon_phrase,
    target_bearing,
    normalize_bearing,
    AT_DESTINATION_M,
)
from poi_guidance import parse_poi_packet, poi_phrase

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
ROAD_DIAGNOSTICS = RoadDiagnosticRecorder(
    os.path.join(CONFIG_DIR, "road_diagnostics")
)
HILL_CLIMB_CHALLENGE = HillClimbChallengeRecorder(
    os.path.join(CONFIG_DIR, "challenges", "hill_climb")
)


STOP = threading.Event()

# The game launch below is deferred until the updater has run and the user has
# answered it -- starting BeamNG.drive underneath a download that is about to
# restart us is precisely what the update flow must not do. A gate rather than a
# reordering, so everything else in _run_engine (config, speech, audio, HRTF, the
# UI bridge) still initialises while the dialog is up. LAUNCH_ALLOWED is read
# once, immediately after the wait returns, so the decision must be final by the
# time the gate opens.
LAUNCH_GATE = threading.Event()
LAUNCH_ALLOWED = True

# Holding either Shift key as BEAM starts skips the automatic game launch for
# that run only -- "not this time", which the persistent launch_beamng setting
# cannot express. SAMPLED IN main(), TESTED AT THE GATE: the launch block runs
# after LAUNCH_GATE.wait(), which can block for minutes behind the updater's
# modal dialog, so asking there would be asking whether Shift is held now,
# long after the user let go -- and would quietly turn Shift-during-an-update
# into a launch veto. Nothing is written to the config.
LAUNCH_SUPPRESSED_BY_SHIFT = False

VK_LSHIFT = 0xA0
VK_RSHIFT = 0xA1


def _shift_held_at_startup():
    """True if either physical Shift key is down right now.

    GetAsyncKeyState reports physical key state regardless of focus or of
    whether a message queue exists yet, which is why nodegrab_listener already
    prefers it over the keyboard hooks. A failed query degrades to False, i.e.
    to today's behaviour.
    """
    try:
        get_state = ctypes.windll.user32.GetAsyncKeyState
        return any((get_state(vk) & 0x8000) != 0 for vk in (VK_LSHIFT, VK_RSHIFT))
    except Exception:
        return False


class LaunchGate:
    """The updater's view of the deferred launch: allow it, or veto it."""

    def allow(self):
        global LAUNCH_ALLOWED
        LAUNCH_ALLOWED = True
        LAUNCH_GATE.set()

    def deny(self):
        global LAUNCH_ALLOWED
        LAUNCH_ALLOWED = False
        LAUNCH_GATE.set()

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
    # Obstacle mode remains an explicit Ctrl+O action; these are presentation/timing
    # preferences only. Legacy enabled/range keys are tolerated when read from disk but are
    # intentionally no longer advertised as active configuration.
    "obstacle_buzz_volume_db": -18.0,
    "obstacle_warning_sensitivity": "normal",
    "road_beep_volume_db": -14.0,
    "route_beacon_volume_db": -16.0,
    "road_correction_volume_db": -24.0,
    "road_follow_guidance_enabled": True,
    "road_junction_speech_enabled": True,
    "road_junction_earcon_enabled": True,
    "road_include_private": False,
    "road_junction_volume_db": -14.0,
    "placement_ping_volume_db": -12.0,
    "launch_beamng": False,
    # Startup update check. On by default: the exe and the Lua/JS mod go out of
    # step silently, so the useful default is the one that keeps them together.
    "update_check_enabled": True,
    # Tag of an update that has been copied over the program directory but whose
    # mod zip has not been offered to BeamNG yet. Written by updater.py only
    # after the download has validated and staged; it survives the restart the
    # self-replacement forces, and has no UI control of its own.
    "pending_update_version": "",
    "announce_turn_signals": True,
    "announce_speed": True,
    "speed_announce_interval": 25,
    "announce_gear": True,
    "announce_clickspot_actions": False,
    "scanner_distance_callout_enabled": False,
    "scanner_distance_callout_interval": 10,
    "scanner_steer_tone_enabled": True,
    "scanner_base_freq_hz": 1000.0,
    "scanner_pitch_offset_oct": 1.0,
    "ui_nav_hold_suppression": True,
    # Loader implement (WL-40 bucket / forks). Inert on every other vehicle.
    "implement_tones_enabled": True,
    "implement_ground_tone_dbfs": -19.5,
    "implement_tilt_tone_dbfs": -20.0,
    "implement_proximity_speech": True,
    "dock_tones_enabled": True,
    "dock_tone_dbfs": -18.0,
    # Terrain sonification scanner (F9 then Space, while the world is live).
    "scan_tones_enabled": True,
    "scan_tone_dbfs": -20.0,
    # Spoken outcome of a shot fired out of the large cannon. No keybind: it announces itself
    # when the car comes to rest, which is the only moment it has anything to say.
    "cannon_shot_readout": True,
    # Learn Bindings Mode speaks "A button. Shift up." by default. The game also ships a
    # full sentence of description per action, which is useful the first time through and
    # tiring once the pad is known -- hence a setting rather than a fixed choice.
    "binding_learn_speak_description": False,
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
    # MCP automation server. Off by default: it executes arbitrary Lua in the game on
    # behalf of a local agent, so a shipped build listens on nothing unless asked.
    "mcp_server_enabled": False,
    "mcp_server_port": 4481,
}

# =========================
#  Speech & Buffer
# =========================
import speech
import secretstore

SPEECH_BUFFER = deque(maxlen=100)

# --- Speech tap (MCP) --------------------------------------------------------------
# SPEECH_BUFFER only holds text the user can replay, and most mod output passes
# exclude_from_buffer=True -- so it is not a record of what the mod said. This is.
# Every say() call lands here, including the ones suppressed during loading (flagged
# spoken=False), because "the mod correctly stayed quiet" is as worth asserting on as
# what it announced. Sequence numbers rather than timestamps: a reader needs a cursor
# with exactly-once semantics, and time.time() ties and skews.
SPEECH_TAP = deque(maxlen=1000)
_speech_tap_lock = threading.Lock()  # dedicated; never nested with state_lock
_speech_tap_seq = 0


def _speech_tap_record(text, source, interrupt, excluded, spoken, reason=None):
    global _speech_tap_seq
    try:
        with _speech_tap_lock:
            _speech_tap_seq += 1
            SPEECH_TAP.append(
                {
                    "seq": _speech_tap_seq,
                    "t": time.time(),
                    "text": text,
                    "source": source,
                    "interrupt": bool(interrupt),
                    "excluded": bool(excluded),
                    "spoken": bool(spoken),
                    "reason": reason,
                }
            )
    except Exception:
        pass


def get_speech_log(since_seq=None, last_n=50, source=None, contains=None, spoken_only=False):
    """Read the speech tap. `since_seq` is a cursor; `dropped` says history was lost."""
    with _speech_tap_lock:
        entries = list(SPEECH_TAP)
        newest = _speech_tap_seq
    oldest = entries[0]["seq"] if entries else 0
    dropped = bool(since_seq is not None and entries and since_seq < oldest - 1)
    if since_seq is not None:
        entries = [e for e in entries if e["seq"] > since_seq]
    if source:
        entries = [e for e in entries if e.get("source") == source]
    if contains:
        needle = contains.lower()
        entries = [e for e in entries if needle in (e.get("text") or "").lower()]
    if spoken_only:
        entries = [e for e in entries if e.get("spoken")]
    if last_n:
        entries = entries[-int(last_n):]
    return {"entries": entries, "next_seq": newest, "dropped": dropped}

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
        _speech_tap_record(t, source, interrupt, exclude_from_buffer, False, "loading_suppressed")
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

    _speech_tap_record(t, source, interrupt, exclude_from_buffer, True)
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
        migrated = False
        if speech.migrate_config(user):
            logger.info("Migrated legacy SAPI speech keys to speech_* keys.")
            migrated = True
        # An API key written before at-rest protection existed is still sitting
        # in plaintext; seal it on the first run that sees it rather than waiting
        # for the user to re-enter a key they have no reason to touch again.
        if secretstore.migrate_config(user):
            logger.info("Encrypted plaintext config secrets with DPAPI.")
            migrated = True
        if migrated:
            _write_config(CONFIG_PATH, user)
        merged = DEFAULT_CONFIG.copy()
        merged.update(user)
        merged["units"] = (
            "metric"
            if str(merged.get("units", "imperial")).lower().startswith("m")
            else "imperial"
        )
        for key, fallback in (
            ("road_follow_guidance_enabled", True),
            ("road_junction_speech_enabled", True),
            ("road_junction_earcon_enabled", True),
            ("road_include_private", False),
        ):
            value = merged.get(key, fallback)
            merged[key] = value if isinstance(value, bool) else fallback
        for key, fallback in (
            ("route_beacon_volume_db", -16.0),
            ("road_beep_volume_db", -14.0),
            ("road_correction_volume_db", -24.0),
            ("road_junction_volume_db", -14.0),
            ("obstacle_buzz_volume_db", -18.0),
        ):
            try:
                merged[key] = max(-120.0, min(0.0, float(merged.get(key, fallback))))
            except (TypeError, ValueError):
                merged[key] = fallback
        sensitivity = str(merged.get("obstacle_warning_sensitivity", "normal")).lower()
        merged["obstacle_warning_sensitivity"] = (
            sensitivity if sensitivity in ("early", "normal", "late") else "normal"
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
OG_TURBO = 1 << 13  # BeamNG/LFS OutGauge flag: this vehicle exposes turbo boost

# Extended telemetry. bng_mod/ is a directory junction into the game install, so the Lua
# half can move (git checkout, mod update) without beamtel restarting. A hard equality test
# on the packet length would then reject every packet, leave `unpacked` at None, and take
# ALL extended telemetry down with it — no speed, no gear, no shift tone, no warning lights,
# and no error anywhere. So every historical layout stays decodable and short packets are
# padded with the sentinels the newer fields use for "absent".
EXT_FORMAT_V1 = "<H4sBx9fII28f"  # pre-implement (no bucket/fork block)
EXT_SIZE_V1 = struct.calcsize(EXT_FORMAT_V1)

EXT_FORMAT_V2 = "<H4sBx9fII36f"  # implement block, four corner wheels only
EXT_SIZE_V2 = struct.calcsize(EXT_FORMAT_V2)

# v3 appends centred front/rear pressure, tire temperature and brake temperature,
# followed by an explicit telemetry-presence bitmask. Appending keeps every v2 offset stable.
EXT_FORMAT = "<H4sBx9fII42fI"
EXT_SIZE = struct.calcsize(EXT_FORMAT)

# Values a v1 packet implies for the eight implement floats. Mirrors the sentinels in
# 796F6C6F313035.lua: flags 0 = nothing valid, -1 = no ground reading.
EXT_V1_IMPLEMENT_DEFAULTS = (0.0, -1.0, -1.0, 0.0, 0.0, 0.0, 0.0, 0.0)

WHEEL_POS_FL = 1 << 0
WHEEL_POS_FR = 1 << 1
WHEEL_POS_RL = 1 << 2
WHEEL_POS_RR = 1 << 3
WHEEL_POS_F = 1 << 4
WHEEL_POS_R = 1 << 5
TELEMETRY_HAS_CLUTCH = 1 << 6
WHEEL_POS_CORNERS = WHEEL_POS_FL | WHEEL_POS_FR | WHEEL_POS_RL | WHEEL_POS_RR

# A v2 mod cannot tell Python which wheel names were present. Preserve its historical
# four-corner interpretation and leave the new centred-wheel values unavailable.
EXT_V2_WHEEL_DEFAULTS = (0.0, 0.0, 0.0, 0.0, 0.0, 0.0, WHEEL_POS_CORNERS)
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
        coupler_run_active, \
        last_scanner_target_name, \
        last_scanner_distance, \
        last_scanner_approach_deg, \
        last_scanner_bearing, \
        scanner_ref_reversed, \
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
                        # Positional tail, guarded by length: fields 3 and 4 (the gap to
                        # reverse and how far the teleport was displaced) are optional, so a
                        # mod half older than this build still reads the two tags.
                        parts_tags = text[len("COUPLER_START:") :].split(",")
                        ptag = parts_tags[0] if len(parts_tags) > 0 else "unknown"
                        ttag = parts_tags[1] if len(parts_tags) > 1 else "unknown"
                        def _tail(idx):
                            if len(parts_tags) <= idx:
                                return None
                            try:
                                return float(parts_tags[idx])
                            except ValueError:
                                return None

                        gap_m = _tail(2)
                        shifted_m = _tail(3)
                        msg = f"Aligned. {ptag} and {ttag}."
                        if gap_m is not None:
                            gap_v, gap_u = fmt_distance(gap_m)
                            msg += f" Reverse {gap_v} {gap_u} to couple."
                        else:
                            msg += " Tracking couplers. Reverse to couple."
                        # Bad news only. The align is solicited and already speaks, so a
                        # clause confirming it worked would be one nobody asked for -- but
                        # a displacement makes the gap figure above untrue, and silence
                        # there is how the old build announced a perfect alignment while
                        # the truck sat 4.6 metres away and a metre off line.
                        if shifted_m is not None and shifted_m > 0.5:
                            sh_v, sh_u = fmt_distance(shifted_m)
                            msg += (
                                f" Could not park you square, moved {sh_v} {sh_u}."
                                " Something is in the way."
                            )
                        say(msg, exclude_from_buffer=True)
                        coupler_run_active = True
                        audio_controller.set_coupler_tracking(True)
                    elif text.startswith("COUPLER_FAIL:"):
                        reason = text[len("COUPLER_FAIL:") :]
                        say(reason, exclude_from_buffer=True)
                    elif text == "COUPLER_LOST":
                        say("Coupler tracking lost", exclude_from_buffer=True)
                        coupler_run_active = False
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
                        # Unconditional: the homing tone is armed by COUPLER_START, which no
                        # longer implies the scanner is still on by the time we couple. Left
                        # inside the scan-mode branch below it could be skipped, and the tone
                        # would then run for the rest of the session. It now outlives the
                        # scanner toggle, so this is the only thing that reliably stops it.
                        coupler_run_active = False
                        audio_controller.set_coupler_tracking(False)
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
                        # Text protocol: "bearing,distance,approachDeg,direction". distance is
                        # the surface GAP between the two vehicles, not centre-to-centre, so it
                        # legitimately reaches zero on contact — floor it so the "%.0f"
                        # readouts below can never say a negative distance.
                        #
                        # The direction field is the scanner's own resolved reference end, and
                        # it wins over the gear we pushed: the Lua side ages that push out and
                        # falls back to velocity, so the two used to disagree whenever the
                        # vehicle moved against its gear (rolling back in D) or right after a
                        # switch. Missing on an older mod, where the pushed gear is still the
                        # best guess available.
                        parts = text.split(",")
                        if len(parts) >= 2:
                            bearing = float(parts[0])
                            distance = max(0.0, float(parts[1]))
                            approach = float(parts[2]) if len(parts) >= 3 else 0.0
                            with state_lock:
                                last_scanner_distance = distance
                                last_scanner_approach_deg = approach
                                last_scanner_bearing = bearing
                                if len(parts) >= 4:
                                    scanner_ref_reversed = int(float(parts[3])) < 0
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
            reversed_ref = scanner_ref_reversed
        if dist == float("inf"):
            continue
        last_callout_ts = now
        a = abs(brg)
        # Fore/aft is stated in the DRIVER's frame, not the bearing's. The bearing is
        # referenced to the direction of travel, so in reverse its 0 is the back of the
        # vehicle; only these two words flip. Left and right do not — the mod deliberately
        # keeps them the driver's physical left and right in every gear.
        if a < 45:
            direction = "behind you" if reversed_ref else "in front of you"
        elif a > 135:
            direction = "in front of you" if reversed_ref else "behind you"
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


def parse_obstacle_packet(text):
    """Parse static-obstacle CSV while preserving both protocol generations.

    Returns ``("clear", None)``, ``("static", hazard)``, or
    ``("terrain", (type, bearing, urgency, distance))``. Invalid data raises ValueError.
    """
    text = str(text or "").strip()
    if text == "0":
        return "clear", None
    parts = text.split(",")
    pkt_type = int(parts[0])
    if pkt_type in (2, 3):
        if len(parts) != 4:
            raise ValueError("terrain packet must have four fields")
        return "terrain", (pkt_type, float(parts[1]), int(parts[2]), float(parts[3]))
    if pkt_type != 1 or len(parts) < 5:
        raise ValueError("unknown or incomplete obstacle packet")

    count = int(parts[1])
    # New wire shape: one legacy triple followed by state, closing, TTC and stopping margin.
    if count == 1 and len(parts) >= 9:
        state = int(parts[5])
        if state not in (1, 2, 3):
            raise ValueError("invalid obstacle state")
        return "static", {
            "bearing": float(parts[2]),
            "urgency": int(parts[3]),
            "gap": float(parts[4]),
            "state": state,
            "closing": float(parts[6]),
            "ttc": float(parts[7]),
            "stopping_margin": float(parts[8]),
            "legacy": False,
        }

    candidates = []
    for i in range(max(0, count)):
        base = 2 + i * 3
        if base + 2 >= len(parts):
            break
        candidates.append((float(parts[base]), int(parts[base + 1]), float(parts[base + 2])))
    if not candidates:
        raise ValueError("legacy static packet contains no complete triples")
    bearing, urgency, distance = max(candidates, key=lambda item: item[1])
    return "static", {
        "bearing": bearing,
        "urgency": urgency,
        "gap": distance,
        "state": 2 if urgency >= 170 else 1,
        "closing": 0.0,
        "ttc": -1.0,
        "stopping_margin": float("inf"),
        "legacy": True,
    }


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
        static_protocol_logged = False
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

                kind, payload = parse_obstacle_packet(text)
                if kind == "clear":
                    audio_controller.update_selected_hazard(None)
                    terrain_announced[2] = False
                    terrain_announced[3] = False
                    continue
                if kind == "static":
                    if not static_protocol_logged:
                        static_protocol_logged = True
                        if payload.get("legacy"):
                            logger.warning(
                                "Obstacle detector is sending the legacy multi-obstacle "
                                "protocol; new audio is active, but predictive TTC/surface-gap "
                                "detection is not. Restart BeamNG or reload obstacleDetector."
                            )
                        else:
                            logger.info(
                                "Predictive obstacle protocol active "
                                "(surface gap, explicit state, closing speed and TTC)."
                            )
                    audio_controller.update_selected_hazard(payload)
                elif kind == "terrain":
                    pkt_type, bearing, urgency, distance = payload
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
            except (ValueError, TypeError) as exc:
                logger.warning(f"Malformed obstacle packet: {exc}")
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
    """Consume R2 road awareness packets with transparent legacy fallback."""
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        sock.bind(("127.0.0.1", ROAD_LISTEN_PORT))
        sock.settimeout(0.2)
        logger.info(f"Road detector listener started on port {ROAD_LISTEN_PORT}")

        first_packet = True
        last_state = None  # "DORMANT" / "ON_ROAD" / "OFF_ROAD"
        last_r2_state = None

        while not stop_event.is_set():
            try:
                data, addr = sock.recvfrom(4096)
                if first_packet:
                    logger.info(
                        f"First UDP packet received from road detector (source: {addr})"
                    )
                    first_packet = False

                text = data.decode("utf-8", errors="ignore").strip()
                if not text:
                    continue

                if text.startswith("R2|"):
                    try:
                        packet = parse_r2_packet(text)
                    except ValueError as exc:
                        logger.warning(f"Ignored malformed road R2 packet: {exc}")
                        continue

                    telemetry_snapshot = _road_diagnostic_telemetry_snapshot()
                    HILL_CLIMB_CHALLENGE.record(packet, telemetry_snapshot)
                    events = ROAD_GUIDANCE_FEED.accept_r2(packet)
                    audio_event = audio_controller.update_road_guidance(
                        packet["state"],
                        packet.get("offRoad"),
                        packet.get("correction"),
                        road_mode_active and road_follow_guidance_enabled,
                    )
                    ROAD_DIAGNOSTICS.record(
                        packet, telemetry_snapshot, audio_event
                    )

                    if (
                        road_mode_active
                        and packet["state"] == "dormant"
                        and last_r2_state != "dormant"
                    ):
                        say(
                            "No roads detected on this map",
                            exclude_from_buffer=True,
                            source="road_guidance",
                        )
                    last_r2_state = packet["state"]

                    if (
                        road_mode_active
                        and events["orientation"]
                        and packet["roadDirections"]
                    ):
                        directions = sorted(
                            packet["roadDirections"], key=lambda value: abs(value)
                        )
                        if packet["oneWay"]:
                            audio_controller.trigger_road_orientation_chime(directions[0])
                            say(
                                "One-way road",
                                interrupt=False,
                                exclude_from_buffer=True,
                                source="road_guidance",
                            )
                        else:
                            second = directions[1] if len(directions) > 1 else None
                            audio_controller.trigger_road_orientation_chime(
                                directions[0], second
                            )

                    junction = events["junction"]
                    if road_mode_active and junction:
                        if (
                            junction["phase"] == "approach"
                            and road_junction_speech_enabled
                        ):
                            say(
                                junction_phrase(junction, UNITS_MODE),
                                interrupt=False,
                                source="road_guidance",
                            )
                        elif (
                            junction["phase"] == "near"
                            and road_junction_earcon_enabled
                        ):
                            audio_controller.trigger_road_junction_earcon()
                        elif (
                            junction["phase"] == "entered"
                            and road_junction_earcon_enabled
                        ):
                            audio_controller.trigger_road_junction_entry_earcon()
                    continue

                if text == "DORMANT":
                    use_legacy = ROAD_GUIDANCE_FEED.accept_legacy("DORMANT")
                    if use_legacy:
                        audio_controller.update_road_state(
                            on_road=True, bearing=0.0, distance=0.0
                        )
                    if use_legacy and last_state != "DORMANT":
                        say("No roads detected on this map", exclude_from_buffer=True)
                        last_state = "DORMANT"
                elif text.startswith("ON_ROAD"):
                    parts = text.split(",")
                    directions = []
                    if len(parts) >= 3:
                        try:
                            b_first = float(parts[1])
                            b_second = float(parts[2])
                            directions = [b_first, b_second]
                        except ValueError:
                            pass
                    use_legacy = ROAD_GUIDANCE_FEED.accept_legacy(
                        "ON_ROAD", directions=directions
                    )
                    if use_legacy:
                        audio_controller.update_road_state(
                            on_road=True, bearing=0.0, distance=0.0
                        )
                        if directions:
                            audio_controller.trigger_road_orientation_chime(*directions)
                        last_state = "ON_ROAD"
                elif text.startswith("OFF_ROAD"):
                    parts = text.split(",")
                    if len(parts) >= 3:
                        try:
                            bearing = float(parts[1])
                            distance = float(parts[2])
                            use_legacy = ROAD_GUIDANCE_FEED.accept_legacy(
                                "OFF_ROAD", bearing=bearing, distance=distance
                            )
                            if use_legacy:
                                audio_controller.update_road_state(
                                    on_road=False, bearing=bearing, distance=distance
                                )
                                last_state = "OFF_ROAD"
                        except ValueError:
                            pass
            except socket.timeout:
                if ROAD_GUIDANCE_FEED.check_timeout():
                    audio_controller.clear_road_audio()
                    logger.warning("Road R2 feed timed out; road audio stopped.")
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
# Insertion depth at or above which the mod calls the band enterable. Mirrors
# IMPL_ENTRY_MIN_DEPTH_M in implementProximity.lua; the gate itself (including its hysteresis)
# is decided over there, next to the geometry. This copy exists only so the phrase can say
# "too steep" about the same number the mod judged, and so a mod that omits the fields (an
# older half of the install) simply produces no clause at all.
IMPL_DOCK_ENTRY_MIN_M = 0.40
IMPL_DOCK_ENTRY_EXIT_M = 0.34   # ...and how far back through it must fall to be lost again
IMPL_ANNOUNCE_PROBE_S = 2.0  # keep asking the mod which implement is fitted until it answers

# Ramp-mode readout thresholds. Separate from the IMPL_DOCK_* pair above because they describe
# a car in a four-metre trough, not tines in a pallet pocket. IMPL_DOCK_LEVEL_M is 5 cm, which
# is a hydraulic-precision figure: applied to a car it would report an offset the driver cannot
# hold and cannot act on, and would chatter over the vehicle's own suspension.
RAMP_DOCK_CENTRE_M = 0.15    # at or under this, "centred" rather than a number
RAMP_DOCK_SQUARE_DEG = 3.0   # under this a bearing counts as dead ahead
RAMP_DOCK_HEADING_ZERO_DEG = 0.5
RAMP_DOCK_TIGHT_M = 0.25     # clearance under this is called tight; <= 0 is "too narrow"
RAMP_DOCK_PITCH_DEG = 5.0    # ramp inclination worth mentioning at all
# The deck readout: what the ramp machine you are SITTING IN is doing with its own ramp, as
# opposed to every other figure in this section, which measures from one machine to another.
RAMP_SELF_LEVEL_DEG = 1.0    # under this the ramp is called level rather than given a number
# Above this stroke a hydraulic group is read out as a DISTANCE ("14 of 18 feet") rather than as
# a percentage. Both are always available; the question is which one a driver can act on. For a
# deck that runs out five and a half metres the answer is obviously the distance — that is the
# run-up they are about to drive — while for a tilt ram with half a metre of travel "1 of 2 feet"
# is a worse way of saying 34 percent, so the short strokes keep the percentage. It is a property
# of the ram, not of the machine, so nothing here needs to know what vehicle it is on.
RAMP_SELF_DISTANCE_STROKE_M = 1.0
# Width margin after a ramp align teleport. Lateral is zero by construction at that moment, so
# the margin IS the clearance the driver will have. Two bands rather than one because "you have
# four centimetres a side" and "you are twenty centimetres too wide" want different decisions,
# and because -1 is the not-measured sentinel and must never be read as a negative clearance.
RAMP_ALIGN_TIGHT_M = 0.0     # at or below: it will not fit
RAMP_ALIGN_SNUG_M = 0.10     # ...and above that, but under this, it fits but barely
# How high the ramp's lip may sit above the ground before it is a wall rather than a ramp. Must
# match the mod's RAMP_ALIGN_LIP_SAY_M. Measured on a us_semi rollback: 1.30 m home and level,
# 0.95 m on full tilt alone, 0.15 m with the bed fully out AND tilted — so the deployed and
# undeployed cases sit either side of this with a wide margin, and the threshold is not
# balancing anything finely.
RAMP_ALIGN_LIP_SAY_M = 0.30
CANNON_READOUT_STALE_S = 1.5

# The approach corridor, and the one thing that decides which QUESTION the ramp instrument is
# answering.
#
# Lateral offset and squareness are corrections to a line you are already on. They are the
# right answer from inside the corridor and useless outside it: told "four metres right, 120
# degrees left" while sitting beside the machine, there is nothing to do with either number,
# because the thing you need first is simply WHERE THE MOUTH IS. So outside the corridor the
# same three channels answer that instead — distance to the mouth and bearing to it — and the
# handover is the corridor boundary.
#
# Hysteretic, because it swaps what the pan means and a boundary crossed twice a second would
# make the pulse jump between "the mouth is over there" and "you are off to the left". The
# wider handoff gives a diagonally approaching driver time to hear a large lateral offset,
# back up and straighten before reaching the mouth; the exit remains 1.5x wider.
RAMP_CORRIDOR_ENTER_M = 6.0
RAMP_CORRIDOR_EXIT_M = 9.0
# ...and how far in front of the mouth plane you must be for the corridor to mean anything at
# all. Behind the plane there is no corridor: driving straight ahead does not lead to the
# mouth from there no matter how well centred you are on its axis.
RAMP_CORRIDOR_MIN_RANGE_M = 0.5

# Cannon shot outcome (cannonShot.lua). Two numbers and then only what is notable, the rule the
# ramp cane tap arrived at after the "too much verbiage" play-test. Nothing here confirms that a
# shot went well; there is no such thing, and the numbers already say what happened.
CANNON_SHOT_CENTRE_M = 3.0    # at or under this the shot went straight; no lateral number
# Apex is a fact about every shot, so it earns its place in the utterance only when it is the
# interesting thing about that shot. A low flat shot has nothing to say about height.
CANNON_SHOT_APEX_SAY_M = 15.0
# Below this, two shots went the same distance and saying so is noise. It is deliberately coarse:
# the point of the comparison is "that change did something", not a measurement.
CANNON_SHOT_COMPARE_M = 5.0

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


def _ramp_bearing_deg(range_m: float, lateral_m: float, yaw_deg: float) -> float:
    """Bearing to the ramp mouth from the driver's own heading, positive-LEFT.

    Derived rather than sent, because the mod already puts every term on the wire and a
    derived value cannot arrive one packet out of step with the numbers it was derived from.
    Writing the mouth offset in the ramp's frame as d = range*axis + lateral*left, and the
    driver's frame as axis = cos(yaw)*fwd + sin(yaw)*driverLeft, the angle to d from fwd comes
    out as atan2(lateral, range) + yaw exactly. Wrapped to (-180, 180] so a mouth just behind
    one shoulder never reads as most of a lap around the other.

    This is a BEARING, not the lateral steering error: it nulls when you are POINTING at the
    mouth, which is what you want while hunting for it, and does not null when you are sitting
    on its centreline, which is what the lateral channel is for.
    """
    beta = math.degrees(math.atan2(lateral_m, range_m)) + yaw_deg
    return ((beta + 180.0) % 360.0) - 180.0


def _ramp_acquire(range_m: float, lateral_m: float, prev: bool) -> bool:
    """Are we still hunting for the mouth, or lined up on the approach to it?

    One authority for both consumers: the phrase spoken on F9+I and the tones being generated
    have to agree about which question is being answered, or the pan will point at the mouth
    while the speech reads out a steering correction. Derived here in the listener, from the
    numbers of the packet being handled, so it is atomic with them by construction.
    """
    if range_m < RAMP_CORRIDOR_MIN_RANGE_M:
        return True
    limit = RAMP_CORRIDOR_EXIT_M if not prev else RAMP_CORRIDOR_ENTER_M
    return abs(lateral_m) > limit


def _ramp_align_phrase(payload: str) -> str:
    """Speak the outcome of a ramp align teleport.

    Pure, and separate from the listener, for the reason every other readout in this file is:
    the wording and the margin bands are the part worth checking without the game, and
    ramp_resolve_sim mirrors this rather than the socket loop around it.

    OK carries the target name, the promised nose-to-mouth gap, and the width margin. FAIL
    carries one clause from the mod, which already names the machine and is already
    speech-sized -- so it is spoken verbatim, exactly as COUPLER_FAIL is.
    """
    if payload.startswith("FAIL,"):
        return payload[5:].strip()
    if not payload.startswith("OK,"):
        return ""

    bits = payload[3:].split(",")
    target = bits[0].strip() or "ramp"

    def num(i):
        try:
            return float(bits[i])
        except (IndexError, ValueError):
            return None

    gap, margin, lip = num(1), num(2), num(3)
    if gap is None:
        phrase = f"Aligned to {target}"
    else:
        val, unit = fmt_distance(gap)
        phrase = f"Aligned to {target}, {val} {unit} back"

    # How high the ramp's lip is off the ground, and the reason this clause exists: the align
    # is geometrically perfect against a ramp that has not been deployed, so it will happily
    # park you twenty feet in front of the back of a truck and say only where you are. First
    # in the tail because it is the one that stops you driving — a ramp you cannot get onto
    # makes the width margin beside the point.
    #
    # A missing field is silence, not zero. Zero reads as "lip on the ground", which is the
    # single most reassuring thing this readout can say and would be being said by a mod half
    # that does not measure it at all.
    if lip is not None and lip >= RAMP_ALIGN_LIP_SAY_M:
        lv, lu = fmt_distance(lip)
        phrase += f", ramp not down, lip {lv} {lu} up"

    # The mod sends the literal NA when it could not measure, which parses to None here and is
    # spoken as silence -- not as a clearance of zero, which would read as exactly touching both
    # walls. Only bad news is added: the driver already knows they were aligned, and confirming
    # it twice is noise.
    if margin is not None:
        if margin <= RAMP_ALIGN_TIGHT_M:
            phrase += ", you do not fit"
        elif margin < RAMP_ALIGN_SNUG_M:
            phrase += ", tight"
    return phrase


def _cannon_shot_long_distance(metres: float) -> str:
    """A cannon shot is hundreds of metres, so this is not fmt_distance.

    fmt_distance is tuned for implement clearances — two decimal places metric, one imperial —
    and asked for a 300 m shot it says "984.3 feet", which is four syllables of precision nobody
    can use about a car that bounced. This is the whole-number form the scanner callout already
    uses for long ranges.
    """
    if UNITS_MODE == "imperial":
        return f"{metres * FEET_PER_M:.0f} feet"
    return f"{metres:.0f} meters"


def _cannon_shot_phrase(shot: dict) -> str:
    """What happened to the car you just fired. Two numbers, then only what is notable.

    Same doctrine as _dock_phrase_ramp, for the same reason: this fires unprompted, right after
    a crash, and every clause it adds is one the listener did not ask for. So distance and how
    far off line it went, always — those two ARE the outcome — and everything else only when it
    is the interesting thing about this particular shot.

    Nothing in here confirms that a shot went well. There is no such thing as a good shot out of
    a cannon with no horizontal aim, and a clause saying "on target" would be inventing a target.
    """
    # A shot that never came to rest has no distance, and must not be given one. The car is
    # still falling, still rolling, or wedged somewhere on its roof — reporting where it happened
    # to be when the timer ran out would state a landing place for a flight that did not land,
    # which is the one error here the listener has no way to catch.
    if not shot.get("settled", True):
        return "Shot did not settle"

    bits = []

    downrange = shot["downrange"]
    lateral = shot["lateral"]

    # Positive is LEFT: the compass clicks, the scanner bearing, the docking readout and the ramp
    # tap all agree on that, and cannon_shot_sim.py asserts it in both directions.
    dist = _cannon_shot_long_distance(abs(downrange))
    if downrange < 0:
        # Backwards out of the barrel. Rare, absurd, and exactly the kind of thing worth being
        # told plainly rather than as an unsigned number that reads like a normal shot.
        bits.append(f"{dist} backwards")
    elif abs(lateral) <= CANNON_SHOT_CENTRE_M:
        bits.append(f"{dist}, straight")
    else:
        side = "left" if lateral > 0 else "right"
        bits.append(f"{dist}, {_cannon_shot_long_distance(abs(lateral))} {side}")

    apex = shot.get("apex", 0.0)
    if apex >= CANNON_SHOT_APEX_SAY_M:
        bits.append(f"peaked at {_cannon_shot_long_distance(apex)}")

    # What it stopped next to, when there is something. Silence means open ground, which is the
    # ordinary case and needs no words.
    near = shot.get("near_name") or ""
    if near and shot.get("near_dist", -1.0) >= 0.0:
        bits.append(f"next to {near}")

    # The comparison against the previous shot is what makes a session-only log worth keeping
    # without a key to read it back: the answer to "did raising the barrel do anything" arrives
    # in the announcement of the shot that answered it.
    prev = shot.get("prev_downrange")
    if prev is not None:
        delta = downrange - prev
        if abs(delta) >= CANNON_SHOT_COMPARE_M:
            word = "further" if delta > 0 else "shorter"
            bits.append(f"{_cannon_shot_long_distance(abs(delta))} {word}")

    return ". ".join(bits)


def _dock_phrase(dock: dict) -> str:
    """The cane tap. Dispatches on the mode the mod reported.

    A different answer, not a branch: the two readouts share no wording, because a ramp has no
    reference band, no thickness and nothing to raise. Splitting them rather than threading
    conditionals through one function is also what makes the implement phrasing provably
    unchanged — dock_readout_sim.py asserts it byte for byte.
    """
    if dock.get("mode") == "RAMP":
        return _dock_phrase_ramp(dock)
    return _dock_phrase_impl(dock)


def _dock_phrase_ramp(dock: dict) -> str:
    """The ramp cane tap. Compact geometry, and then only what is wrong.

    This readout was cut down hard after play-testing, on the operator's report that it was
    "too much verbiage". The old form opened with "Cannon ramp, Large Cannon standard white"
    on every single tap — twelve syllables identifying a machine you are already looking at —
    and then read four facts whether or not any of them needed acting on, twice a minute,
    while driving. The rules it follows now:

      * The target is named ONCE, when it is acquired, by the mode announcement in the
        listener. A tap is a question about geometry, not about identity.
      * Lateral and range first, followed by the heading that drives the beat pair.
      * The unit is spoken once per utterance, on the distance. Both figures are in it.
      * Clearance and the ramp's own pitch are BAD NEWS ONLY,
        the rule the implement readout already follows for its entry depth. Silence on those
        means there is nothing to fix, which is exactly what "square. 0.9 meters clearance
        each side" was taking two seconds to say.
    """
    bits = []

    rng = dock["range"]
    lat = dock["lateral"]
    yaw = dock["yaw"]

    if dock.get("acquire"):
        # Hunting. The mouth is a PLACE to get to, so it is named the way any other place is:
        # which way to turn, and how far. The along-axis range and the lateral offset are
        # still true here but neither is actionable — being "one metre left of the axis"
        # while parked beside the machine is a fact about a line you are nowhere near, and
        # acting on it drives you past the mouth rather than into it.
        dist = math.hypot(rng, lat)
        dv, du = fmt_distance(dist)
        bearing = _ramp_bearing_deg(rng, lat, yaw)
        if abs(bearing) < RAMP_DOCK_SQUARE_DEG:
            bits.append(f"ahead, {dv} {du}")
        else:
            bits.append(f"{abs(bearing):.0f} {'left' if bearing > 0 else 'right'}, {dv} {du}")
        # Which side of the mouth plane. The whole reason this phase exists: from the far
        # side, steering toward the mouth arrives at its BACK, so "turn left and drive" is
        # wrong advice however well aimed. One word, because it is a state and not a quantity.
        #
        # The word is NOT "behind". This clause and the bearing clause above are in different
        # reference frames — the bearing is measured from the driver's nose, this is measured
        # against the mouth plane — and both can be true at once, which produced the reading
        # "ahead, 73.9 feet. behind" and a fair question about how that could possibly be
        # interpreted. It was not contradictory, it was two frames wearing the same kind of
        # word. "Wrong side" cannot be mistaken for a bearing, which is what makes it safe to
        # stand next to one.
        #
        # Fired on the SIGN of the range, not on the corridor threshold. The corridor's half
        # metre is the point at which "on the approach" stops meaning anything, and a car
        # sitting 0.3 m short of the mouth plane is in the doorway, not on the wrong side of
        # it — the threshold would have called that state wrong side too.
        if rng <= -RAMP_CORRIDOR_MIN_RANGE_M:
            bits.append("wrong side")
    else:
        # On the approach. Lateral first: it is the axis carried by pulse position, so the tap
        # confirms what the spatial cue is already saying.
        # Positive is LEFT, matching the compass clicks, the scanner bearing and the
        # implement readout. dock_readout_sim.py asserts this in both directions.
        rv, ru = fmt_distance(max(0.0, rng))
        if abs(lat) <= RAMP_DOCK_CENTRE_M:
            bits.append(f"centred, {rv} {ru}")
        else:
            lv, _lu = fmt_distance(abs(lat))
            bits.append(f"{lv} {'left' if lat > 0 else 'right'}, {rv} {ru}")

        # Heading is always explicit on the approach because it is the beat pair's null.
        if abs(yaw) < RAMP_DOCK_HEADING_ZERO_DEG:
            bits.append("heading zero degrees")
        else:
            heading = max(1, int(math.floor(abs(yaw) + 0.5)))
            bits.append(
                f"heading {heading} degrees {'left' if yaw > 0 else 'right'}"
            )

    # Whether the vehicle fits between the walls, and now only when it does not comfortably.
    # A negative margin is the whole reason the number exists, so it is stated as an overlap
    # rather than as a negative clearance. None or a negative sentinel means the mod could not
    # measure it, which is silence rather than a guess — reporting zero would read as exactly
    # touching both walls.
    #
    # Held back entirely while hunting, because the margin is the mouth's half-width minus your
    # CURRENT lateral offset: parked beside the machine that is several metres negative, and
    # "too narrow by four metres" about a mouth you have not begun to line up with is a fact
    # about where you happen to be standing, not about whether the car fits.
    margin = None if dock.get("acquire") else dock.get("margin")
    if margin is not None and margin > -0.999:
        if margin <= 0.0:
            mv, mu = fmt_distance(abs(margin))
            bits.append(f"too narrow by {mv} {mu}")
        elif margin < RAMP_DOCK_TIGHT_M:
            mv, mu = fmt_distance(margin)
            bits.append(f"tight, {mv} {mu} each side")

    # The ramp's own inclination. Context, not an instruction — the driver of the car cannot
    # change it — but a steeply raised ramp is one you are about to drive UP, which changes the
    # approach speed, so it is worth a word once it is steep enough to matter.
    # -999 is the mod's "could not measure" sentinel, sent when rampGeometry's five chosen
    # nodes are not collision surface and the plane through them is therefore structure rather
    # than floor. It is deliberately not 0.0, which would read as a level ramp; the magnitude
    # band matches the one _ramp_self_phrase already applies to the same sentinel.
    pitch = dock.get("entry_theta")
    if pitch is not None and abs(pitch) > 900:
        pitch = None
    if pitch is not None and abs(pitch) >= RAMP_DOCK_PITCH_DEG:
        bits.append(f"ramp {'up' if pitch > 0 else 'down'} {abs(pitch):.0f} degrees")

    # The mod now feeds this readout from far outside the range at which anything is sonified,
    # so the tap keeps answering after the tones have faded. That is the whole point — it is
    # the state someone reaches for the key IN — but unexplained silence from the speakers is
    # the ambiguity this project keeps having to pay for, so it is named rather than left to be
    # inferred. Distance is the in-plane one, the same figure the tone gate is applied to.
    if math.hypot(rng, lat) >= DOCK_RAMP_MAX_RANGE_M:
        bits.append("too far for tones")

    return ". ".join(bits)


def _parse_ramp_self(payload: str):
    """The RAMPSELF: line -> the deck state of the machine you are sitting in, or None.

    Wire form is `<pitchDeg>;<name>:<pct>:<strokeCm>;<name>:<pct>:<strokeCm>...`, or the literal
    NONE when the player's vehicle has no ramp at all. The hydraulic tail is optional and may be
    empty: a ramp with no rams on it (a fixed dry-van ramp) still has a pitch worth reading, and
    the mod pushes an empty tail rather than withholding the line.

    Anything unparseable is dropped rather than guessed at. A deck readout is a set of absolute
    numbers with no sanity check available to the listener, so a half-decoded one is worse than
    silence — the same reason the mod sends -999 for an unmeasurable pitch instead of 0.
    """
    payload = (payload or "").strip()
    if not payload or payload.upper() == "NONE":
        return None
    fields = payload.split(";")
    # The head is `<pitchDeg>` or `<pitchDeg>,<lipM>`. Parsed with a length guard rather than a
    # fixed shape, the same optional-tail contract the DOCK: line's entry-gate fields use:
    # bng_mod/ is a live junction into the game install, so the two halves genuinely do go out
    # of step, and a mod half that cannot measure the lip should lose that clause rather than
    # taking the whole readout down.
    head = fields[0].split(",")
    try:
        pitch = float(head[0])
    except (ValueError, IndexError):
        return None
    try:
        lip = float(head[1])
    except (ValueError, IndexError):
        lip = None
    groups = []
    for chunk in fields[1:]:
        if not chunk:
            continue
        bits = chunk.split(":")
        if len(bits) != 3:
            continue
        try:
            groups.append((bits[0], int(bits[1]), int(bits[2]) / 100.0))
        except ValueError:
            continue
    # -999 is the mod's "could not measure" sentinel for both figures. Carried as None so the
    # phrase drops the clause entirely; rendering it would announce a ramp pointing 999 degrees
    # into the ground, or a lip a thousand metres underneath one.
    return {
        "pitch": None if pitch < -180.0 else pitch,
        "lip": None if (lip is None or lip < -900.0) else lip,
        "groups": groups,
    }


def _ramp_hyd_words(name: str) -> str:
    """A hydraulic group's own electrics name, said out loud.

    Mechanical: underscores and camelCase humps become spaces. Deliberately not a lookup table —
    this is the string the vehicle's own Special Vehicle Keys bindings are driven by, so speaking
    it is what lets a machine nobody has thought about report its controls under the names its
    own key list already uses, with no entry anywhere.
    """
    out = []
    for i, ch in enumerate(name):
        if ch == "_":
            out.append(" ")
        elif ch.isupper() and i > 0 and not name[i - 1].isupper() and name[i - 1] != "_":
            out.append(" ")
            out.append(ch.lower())
        else:
            out.append(ch.lower())
    return " ".join("".join(out).split())


def _ramp_hyd_strip_namespace(names: list) -> list:
    """Drop a leading name segment that every group shares.

    A us_semi's rams are upfit_tilt, upfit_extendRetract and upfit_extendRetractFeet, and
    "upfit" is a namespace rather than a description — three groups into one readout it is six
    wasted syllables that distinguish nothing.

    Requires TWO or more groups, and that is the whole justification rather than a guard bolted
    on: a prefix is only identifiable as a namespace by the fact that several names share it. On
    a machine with a single hydraulic group its first segment may well be the only word that says
    what the thing does, and stripping it there would be inventing an allowlist by the back door.
    """
    if len(names) < 2:
        return names
    heads = {n.split("_")[0] for n in names}
    if len(heads) != 1:
        return names
    head = next(iter(heads))
    stripped = [n[len(head) + 1:] for n in names]
    # A group whose entire name IS the shared prefix has nothing left to be called.
    if any(not s for s in stripped):
        return names
    return stripped


def _ramp_self_phrase(state: dict) -> str:
    """The deck readout: what your own ramp machine's ramp is doing.

    Everything else the alignment key can say measures from one machine to another. This is the
    one answer about the machine under you, and it exists because F9+I had nothing at all to say
    from the seat of a hauler — the cannon branch used to claim that key by mistake, and removing
    that left the correct answer ("docking instrument is off") and no useful one.

    It reads out unconditionally rather than following the ramp tap's bad-news-only rule. That
    rule is about clauses nobody asked for, appended to an utterance that was already going to
    happen; these two numbers ARE the question being asked, and a readout that stayed silent
    because the deck was level would be indistinguishable from a broken one.
    """
    bits = []

    pitch = state.get("pitch")
    if pitch is None:
        # No angle available. Named rather than skipped: this readout is mostly the angle, and
        # an utterance that quietly drops it reads as a level ramp.
        bits.append("Ramp angle unavailable")
    elif abs(pitch) < RAMP_SELF_LEVEL_DEG:
        bits.append("Ramp level")
    else:
        bits.append(
            f"Ramp {'up' if pitch > 0 else 'down'} {abs(pitch):.0f} degrees"
        )

    # Whether a car can actually get onto it, which is the question the other two numbers only
    # answer between them: on a rollback the tilt alone brings the lip from 1.30 m to 0.95 m and
    # it is running the BED out that does the rest, so neither figure alone is the answer.
    # Stated in BOTH directions rather than as bad news only — this readout is solicited, so
    # silence here would be indistinguishable from a mod half that cannot measure it, and
    # "lip on the ground" is precisely the confirmation somebody presses the key for.
    lip = state.get("lip")
    if lip is not None:
        if lip < RAMP_ALIGN_LIP_SAY_M:
            bits.append("Lip on the ground")
        else:
            lv, lu = fmt_distance(lip)
            bits.append(f"Lip {lv} {lu} up")

    groups = state.get("groups") or []
    labels = _ramp_hyd_strip_namespace([g[0] for g in groups])
    for (_name, pct, stroke), label in zip(groups, labels):
        words = _ramp_hyd_words(label)
        if stroke >= RAMP_SELF_DISTANCE_STROKE_M:
            # "14 of 18 feet", not "14 feet, 78 percent". One figure per group: the percentage
            # and the distance are the same fact twice, and the unit is spoken once because
            # both halves of the fraction are in it.
            out_v, out_u = fmt_distance(stroke * pct / 100.0)
            full_v, _full_u = fmt_distance(stroke)
            bits.append(f"{words} {out_v} of {full_v} {out_u}")
        else:
            bits.append(f"{words} {pct} percent")

    return ". ".join(bits)


def _dock_phrase_impl(dock: dict) -> str:
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

    # The entry gate, and the reason the whole docking instrument was re-aimed. Every other
    # axis can be nulled perfectly and the tines still not go in, because a tilted implement
    # climbs through the band's thickness after a few centimetres of travel. Spoken only when
    # it is bad news: an enterable band is the expected case and saying so on every tap would
    # be four words of nothing, four times a minute. A negative depth means the mod could not
    # measure it (or is older than this field), which is silence rather than a guess.
    depth = dock.get("entry_depth")
    if depth is not None and depth >= 0.0 and depth < IMPL_DOCK_ENTRY_MIN_M:
        dv, du = fmt_distance(max(0.0, depth))
        bits.append(f"tines enter {dv} {du}, too steep")

    return ". ".join(bits)


def cannon_shot_listener(stop_event):
    """Listens for shot outcomes from cannonShot.lua and speaks them.

    One short line per shot, arriving when the car comes to rest — so there is no polling, no
    state machine and no keybind. The mod owns the tracking because only it can see the firing
    axis and the ramp's live pitch; this owns every word.

    Takes no audio_controller: the outcome is speech and nothing else. There is no tone here on
    purpose — the crash has just finished making a great deal of noise, and an earcon in front of
    the sentence would be one more thing to hear rather than one less.
    """
    global last_cannon_shot, cannon_shot_session
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        sock.bind(("127.0.0.1", CANNON_SHOT_LISTEN_PORT))
        sock.settimeout(0.2)
        logger.info(f"Cannon shot listener started on port {CANNON_SHOT_LISTEN_PORT}")
        while not stop_event.is_set():
            try:
                data, _addr = sock.recvfrom(2048)
                text = data.decode("utf-8", errors="ignore").strip()
                if not text.startswith("SHOT:"):
                    continue
                parts = text[5:].split(",")
                if len(parts) < 11:
                    continue
                shot = {
                    "downrange": float(parts[0]),
                    "lateral": float(parts[1]),
                    "apex": float(parts[2]),
                    "flight": float(parts[3]),
                    "ramp_pitch": float(parts[4]),
                    "strength": int(parts[5]),
                    "settled": parts[6].strip() == "1",
                    "index": int(parts[7]),
                    "near_dist": float(parts[8]),
                    "near_name": parts[9].strip(),
                    "vehicle": parts[10].strip(),
                    "stamp": time.monotonic(),
                }
                # The comparison clause is against the shot before this one, which the mod
                # tracks because it is the half that knows the session's shot count. Read it
                # here rather than trusting our own list: a beamtel restart mid-session leaves
                # Python with an empty history and the mod with the real one.
                if len(parts) >= 12:
                    prev = float(parts[11])
                    if prev > -1e8:
                        shot["prev_downrange"] = prev
                elif cannon_shot_session:
                    shot["prev_downrange"] = cannon_shot_session[-1]["downrange"]

                with state_lock:
                    cannon_shot_session.append(shot)
                    last_cannon_shot = shot

                logger.info(
                    "Cannon shot %d: %.1f m downrange, %.1f m lateral, apex %.1f m, "
                    "%.1f s, ramp pitch %.1f deg, strength %d%s",
                    shot["index"], shot["downrange"], shot["lateral"], shot["apex"],
                    shot["flight"], shot["ramp_pitch"], shot["strength"],
                    "" if shot["settled"] else " (never settled)",
                )
                # Unsolicited, so it goes in the review buffer — unlike every keypress readout,
                # this is exactly the kind of thing you might miss and want to hear again.
                if announce_cannon_shot:
                    say(_cannon_shot_phrase(shot))
            except socket.timeout:
                continue
            except (ValueError, IndexError) as e:
                logger.warning(f"Malformed cannon shot packet: {e}")
                continue
            except OSError:
                if stop_event.is_set():
                    break
                logger.error("Cannon shot listener socket error.")
    finally:
        try:
            sock.close()
        except Exception:
            pass
        logger.info("Cannon shot listener stopped.")


def trailer_angle_listener(stop_event):
    """Listens for trailer articulation angles from trailerAngle.lua.

    Writes state only. The tone is driven from the telemetry loop's existing handoff to
    audio (see the update_telemetry_state call), so this thread never touches the audio
    controller — which is what keeps the age-out in one place: a value that stops arriving
    has to expire wherever it is READ, not wherever it was written, because nothing runs
    here to notice the silence.
    """
    global last_trailer_deg, last_trailer_id, last_trailer_name, last_trailer_stamp
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        sock.bind(("127.0.0.1", TRAILER_ANGLE_LISTEN_PORT))
        sock.settimeout(0.2)
        logger.info(f"Trailer angle listener started on port {TRAILER_ANGLE_LISTEN_PORT}")
        while not stop_event.is_set():
            try:
                data, _addr = sock.recvfrom(1024)
                text = data.decode("utf-8", errors="ignore").strip()
                if not text.startswith("TRAILER:"):
                    continue
                body = text[8:]

                # CLEAR is uncoupling, a vehicle switch, or a frame the mod could not measure.
                # Distinct from the age-out on purpose: this one is the mod telling us, so it
                # takes effect immediately rather than a second later.
                if body == "CLEAR":
                    with state_lock:
                        if last_trailer_id is not None:
                            logger.info("Trailer uncoupled (%s).", last_trailer_name or "?")
                        last_trailer_deg = 0.0
                        last_trailer_id = None
                        last_trailer_name = ""
                        last_trailer_stamp = 0.0
                    continue

                parts = body.split(",")
                if len(parts) < 2:
                    continue
                deg = float(parts[0])
                tid = int(parts[1])
                # Positional tail, guarded — the same optional-field contract the scanner
                # packet's fourth field uses, because bng_mod/ is a live junction and the two
                # halves genuinely do go out of step.
                name = parts[2].strip() if len(parts) >= 3 else "trailer"

                with state_lock:
                    if tid != last_trailer_id:
                        logger.info("Trailer coupled: %s (id %d).", name, tid)
                    last_trailer_deg = deg
                    last_trailer_id = tid
                    last_trailer_name = name
                    last_trailer_stamp = time.monotonic()
            except socket.timeout:
                continue
            except (ValueError, IndexError) as e:
                logger.warning(f"Malformed trailer angle packet: {e}")
                continue
            except OSError:
                if stop_event.is_set():
                    break
                logger.error("Trailer angle listener socket error.")
    finally:
        try:
            sock.close()
        except Exception:
            pass
        logger.info("Trailer angle listener stopped.")


def route_beacon_listener(stop_event):
    """Listens for the map route's destination from routeBeacon.lua.

    Writes state only. The beacon's bearing is derived in the telemetry loop, where the
    vehicle's position and heading already arrive at 60 Hz, and the age-out is applied
    there too -- for the reason _trailer_artic_norm records: a feed that stops has to
    expire wherever it is READ, because nothing runs in here to notice the silence.
    """
    global route_dest_x, route_dest_y
    global route_remaining_m, route_beacon_stamp
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        sock.bind(("127.0.0.1", ROUTE_BEACON_LISTEN_PORT))
        sock.settimeout(0.2)
        logger.info(f"Route beacon listener started on port {ROUTE_BEACON_LISTEN_PORT}")
        while not stop_event.is_set():
            try:
                data, _addr = sock.recvfrom(1024)
                text = data.decode("utf-8", errors="ignore").strip()
                if not text.startswith("ROUTE:"):
                    continue

                packet = parse_route_packet(text)

                # CLEAR is the route being cleared, arrival, or a level change. Distinct
                # from the age-out on purpose: this one is the mod telling us, so it
                # takes effect immediately rather than a second and a half later.
                if packet is None:
                    with state_lock:
                        if route_dest_x is not None:
                            logger.info("Navigation route cleared.")
                        route_dest_x = None
                        route_dest_y = None
                        route_remaining_m = 0.0
                        route_beacon_stamp = 0.0
                    continue

                dx, dy, _dz = packet["dest"]
                with state_lock:
                    if route_dest_x is None:
                        logger.info(
                            "Navigation route set: destination %.1f, %.1f.", dx, dy
                        )
                    route_dest_x = dx
                    route_dest_y = dy
                    route_remaining_m = packet["route_m"]
                    route_beacon_stamp = time.monotonic()
            except socket.timeout:
                continue
            except ValueError as e:
                logger.warning(f"Malformed route beacon packet: {e}")
                continue
            except OSError:
                if stop_event.is_set():
                    break
                logger.error("Route beacon listener socket error.")
    finally:
        try:
            sock.close()
        except Exception:
            pass
        logger.info("Route beacon listener stopped.")


def _route_is_set():
    """True when a live route destination is on hand.

    Takes no lock, for the reason _trailer_artic_norm gives: these are floats and a
    None, each written in one bound assignment, read from the telemetry loop outside
    its own lock alongside every other value.
    """
    if route_dest_x is None:
        return False
    return (time.monotonic() - route_beacon_stamp) <= ROUTE_STALE_SEC


def _trailer_artic_norm():
    """The trailer angle on the WL-40's normalised -1..1 scale, or 0.0 for silence.

    Returns 0.0 — not a sentinel — when nothing is coupled or the feed has gone stale,
    because 0.0 is what the tone already reads as "in line": the deadzone gate then
    silences it through exactly the same path an ordinary car takes, envelope and all.
    A sentinel would need its own branch in the callback to mean the same thing.

    Deliberately takes NO lock. It is called from the telemetry loop's handoff to audio,
    which sits outside that loop's `with state_lock:` and reads every other value the same
    way; the globals here are a float, an int and a str, each written in one bound
    assignment, so a torn read is not possible and the worst case is a value one packet
    old — which the 20 Hz feed replaces before the tone's own smoothing could resolve it.
    """
    if last_trailer_id is None or last_trailer_stamp <= 0.0:
        return 0.0
    if (time.monotonic() - last_trailer_stamp) > TRAILER_STALE_SEC:
        return 0.0
    return max(-1.0, min(1.0, last_trailer_deg / TRAILER_FULL_DEG))


def terrain_scan_listener(audio_controller, stop_event):
    """Listens for terrain scan snapshots from terrainScanner.lua and renders them.

    The synthesis runs HERE, on this thread, not in the audio callback — see render_scan.
    A snapshot is one datagram of roughly six kilobytes, which is why the buffer is 65535
    and not the 1024/2048 the other listeners use: a short read here would fail as a
    truncated scan, i.e. as a landscape that stops halfway, rather than as an error.
    """
    global _last_scan_reply_ts, _last_poi_reply_ts
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        sock.bind(("127.0.0.1", TERRAIN_SCAN_LISTEN_PORT))
        sock.settimeout(0.2)
        logger.info(f"Terrain scan listener started on port {TERRAIN_SCAN_LISTEN_PORT}")
        while not stop_event.is_set():
            try:
                data, _addr = sock.recvfrom(65535)
                text = data.decode("utf-8", errors="ignore").strip()
                if not text:
                    continue
                if text.startswith("POI:"):
                    _last_poi_reply_ts = time.time()
                    try:
                        say(
                            poi_phrase(parse_poi_packet(text), UNITS_MODE),
                            exclude_from_buffer=True,
                        )
                    except ValueError as e:
                        logger.warning(f"Malformed nearest POI packet: {e}")
                        say(
                            "Point of interest response was invalid",
                            exclude_from_buffer=True,
                        )
                    continue
                if text.startswith("POI_FAIL:"):
                    _last_poi_reply_ts = time.time()
                    reason = text.split(":", 1)[1].strip() or "unknown error"
                    say(
                        f"Nearest point of interest unavailable. {reason}",
                        exclude_from_buffer=True,
                    )
                    continue
                lines = text.split("\n")
                head = lines[0].split(",")
                if head[0] == "FAIL":
                    _last_scan_reply_ts = time.time()
                    say(
                        f"Scan failed. {head[1] if len(head) > 1 else 'unknown'}",
                        exclude_from_buffer=True,
                    )
                    continue
                if head[0] != "SCAN" or len(head) < 6:
                    continue
                _last_scan_reply_ts = time.time()
                reach = float(head[4])
                samples = []
                objects = []
                pois = []
                for ln in lines[1:]:
                    if ln == "END":
                        break
                    parts = ln.split(",")
                    if parts[0] == "S" and len(parts) >= 3:
                        bearing = float(parts[1])
                        cells = parts[2:]
                        # Range is derived from the cell's INDEX against the reach the mod
                        # reported, not sent per cell. Sending it would triple the packet to
                        # restate something both ends can compute, and would let the two
                        # disagree about the time axis.
                        denom = max(1, len(cells) - 1)
                        for i, cell in enumerate(cells):
                            rng = (i / denom) * reach
                            # The surface family is an OPTIONAL ':' suffix and is stripped
                            # FIRST, before the "~" and "_" tests, so an older mod half that
                            # sends none parses byte for byte as it always did. A missing or
                            # unrecognised code means paved, which is also what "no
                            # TerrainBlock" means — so the fallback is today's tone.
                            family = None
                            if ":" in cell:
                                cell, code = cell.split(":", 1)
                                family = SCAN_FAMILY_CODES.get(code)
                            if cell == "~":
                                # No surface. Not zero — zero is level ground.
                                samples.append((bearing, rng, None, None, None))
                            elif "_" in cell:
                                dz, depth = cell.split("_", 1)
                                samples.append(
                                    (bearing, rng, float(dz), float(depth), None)
                                )
                            else:
                                samples.append(
                                    (bearing, rng, float(cell), None, family)
                                )
                    elif parts[0] == "O" and len(parts) >= 4:
                        # The fourth field is the mod's p/v kind tag. It was on the wire and
                        # discarded for a long time; it now picks the ping length, so a cone
                        # ticks where a car rings. Absent means "v", the safer default.
                        kind = parts[4] if len(parts) >= 5 else "v"
                        objects.append(
                            (float(parts[1]), float(parts[2]), float(parts[3]), kind)
                        )
                    elif parts[0] == "P" and len(parts) >= 4:
                        pois.append(
                            (float(parts[1]), float(parts[2]), float(parts[3]))
                        )
                audio_controller.render_and_play_scan(
                    samples, objects, reach, pois=pois
                )
            except socket.timeout:
                continue
            except (ValueError, IndexError) as e:
                logger.warning(f"Malformed terrain scan packet: {e}")
                continue
            except OSError:
                if stop_event.is_set():
                    break
                logger.error("Terrain scan listener socket error.")
    finally:
        try:
            sock.close()
        except Exception:
            pass
        logger.info("Terrain scan listener stopped.")


def implement_listener(audio_controller, stop_event):
    """Listens for UDP packets from implementProximity.lua and speaks approach/leave events.

    Only ever announces the NEAREST object. The extension may report a different one from
    tick to tick in a crowded yard, and narrating all of them would be unusable.
    """
    global _implement_word_current, last_dock, last_dock_fail, cannon_active, last_dock_name
    global last_ramp_self
    global last_dock_mode, cannon_kind, last_cannon_aim
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
        # ...and keep asking until it answers. A single request at startup is lost whenever
        # beamtel is running before BeamNG is, which is the normal way round to start them,
        # and the mod has no way to know we are waiting. Stops on the first IMPLEMENT: line,
        # so a machine with nothing fitted settles after one reply too.
        implement_seen = False
        next_probe = time.time() + IMPL_ANNOUNCE_PROBE_S

        impl_word = "Bucket"
        tracked = None  # name of the object currently being tracked
        tracked_relation = None
        tracked_inside = False
        # Pending transitions, each (value, first_seen_ts), so a flap has to persist.
        pending_relation = None
        pending_inside = None
        pending_leave = None
        # Which side of the entry gate the last DOCK: line was on. None means "not known
        # yet", which is what stops the earcon firing on first acquisition — arriving with
        # the tines already level is not an event.
        entry_ok = None
        # Which side of the ramp approach corridor the last DOCK: line was on. Starts hunting,
        # because that is what acquiring a ramp target from nothing IS, and because the
        # hysteresis then has to be satisfied before the instrument claims you are lined up.
        ramp_acquire = True

        while not stop_event.is_set():
            try:
                data, _addr = sock.recvfrom(2048)
                text = data.decode("utf-8", errors="ignore").strip()
                if not text:
                    continue
                now = time.time()

                if text.startswith("IMPLEMENT:"):
                    implement_seen = True
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
                    entry_ok = None
                    ramp_acquire = True
                    with state_lock:
                        last_dock = None
                        last_dock_fail = None
                        last_dock_mode = None
                        last_dock_name = None
                    audio_controller.clear_dock_target()
                    continue

                # Docking lines are handled ahead of the proximity chain and never feed it:
                # they are a separate instrument with its own toggle, and the two would
                # otherwise fight over the same tracked/pending state machine.
                if text == "DOCKCLEAR":
                    # Losing the target drops the gate back to "not known": re-acquiring is
                    # not the same act as tilting into range, and only the second deserves
                    # the earcon. The corridor latch goes back to hunting for the same
                    # reason — whatever you re-acquire, you have not lined up with it yet.
                    entry_ok = None
                    ramp_acquire = True
                    with state_lock:
                        last_dock = None
                        last_dock_fail = None
                    audio_controller.clear_dock_target()
                    continue

                if text.startswith("DOCKFAIL:"):
                    # Not spoken unprompted — it would nag while manoeuvring. Held for the
                    # F9+I readout, which is where someone asks "why is this silent".
                    entry_ok = None
                    ramp_acquire = True
                    with state_lock:
                        last_dock = None
                        last_dock_fail = text[9:].strip()
                    audio_controller.clear_dock_target()
                    continue

                if text.startswith("RAMPALIGN:"):
                    phrase = _ramp_align_phrase(text[len("RAMPALIGN:") :])
                    if phrase:
                        say(phrase, exclude_from_buffer=True)
                    continue

                if text.startswith("CANNON:"):
                    # Which cannon is being driven. "1" is the pre-Old-Cannon spelling for
                    # the Large Cannon and remains accepted across a live mod/app version skew.
                    payload = text[7:].strip().upper()
                    kind = (
                        "OLD"
                        if payload == "OLD"
                        else "LARGE"
                        if payload in ("1", "LARGE")
                        else "NONE"
                    )
                    with state_lock:
                        cannon_kind = kind
                        cannon_active = kind != "NONE"
                        if kind != "OLD":
                            last_cannon_aim = None
                    if kind != "OLD":
                        audio_controller.clear_cannon_aim()
                    continue

                if text.startswith("RAMPSELF:"):
                    # Your own machine's deck. Sent on change only, so this is a latch and not
                    # a stream: what lands here stays true until the mod says otherwise, which
                    # is exactly what a key-press readout needs. NONE clears it, and that
                    # matters — without it, climbing out of a hauler into a car would leave the
                    # alignment key reading out a deck that is no longer under you.
                    with state_lock:
                        last_ramp_self = _parse_ramp_self(text[len("RAMPSELF:") :])
                    continue

                if text.startswith("CANNONAIM:"):
                    fields = text[10:].split(",")
                    if len(fields) != 7:
                        continue
                    try:
                        aim = {
                            "elevation": float(fields[0]),
                            "bearing": float(fields[1]),
                            "target_angle": float(fields[2]),
                            "line_angle": float(fields[3]),
                            "range": float(fields[4]),
                            "speed": float(fields[5]),
                            "reachable": fields[6].strip() == "1",
                        }
                    except ValueError:
                        continue
                    aim["solution_available"] = aim["speed"] > 0.0
                    aim["stamp"] = time.monotonic()
                    with state_lock:
                        last_cannon_aim = aim
                    if aim["range"] < 0.0:
                        audio_controller.clear_cannon_aim()
                    else:
                        audio_controller.update_cannon_aim(
                            aim["elevation"],
                            aim["bearing"],
                            aim["target_angle"],
                            aim["solution_available"],
                            aim["reachable"],
                        )
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
                    #
                    # Two entry-gate fields were appended to the end of this line. Try the
                    # long form first and fall back to the short one, so a mod half older
                    # than this build keeps its docking readout instead of losing it — the
                    # same optional-tail contract the scanner packet's fourth field uses.
                    # bng_mod/ is a live junction into the game install, so the two halves
                    # genuinely do go out of step.
                    # The ladder is longest-first. Two more fields — the MODE and the ramp
                    # width margin — were appended after the entry-gate pair, so there are now
                    # three shapes on the wire and all three must keep working.
                    fields = text[5:].rsplit(",", 14)
                    if len(fields) != 15:
                        fields = text[5:].rsplit(",", 12)
                        # elif, not a second if: a successful 15-field split leaves len 15, which
                        # is also != 13, so an independent test would immediately re-split it down
                        # to 11 and throw the mode and margin away on every ramp packet.
                        if len(fields) != 13:
                            fields = text[5:].rsplit(",", 10)
                    if len(fields) not in (11, 13, 15):
                        continue
                    try:
                        long_tail = len(fields) >= 13
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
                            "entry_theta": float(fields[11]) if long_tail else None,
                            "entry_depth": float(fields[12]) if long_tail else None,
                            # A mod older than this build has no other mode to be in, so
                            # defaulting to IMPL is not a guess — it is the only thing it can
                            # mean. The margin stays None rather than 0, which would read as
                            # exactly touching both walls.
                            "mode": (
                                fields[13].strip().upper() if len(fields) == 15 else "IMPL"
                            ),
                            "margin": float(fields[14]) if len(fields) == 15 else None,
                        }
                    except ValueError:
                        continue
                    if parsed["mode"] not in ("IMPL", "RAMP"):
                        parsed["mode"] = "IMPL"
                    # Which question the ramp instrument is answering this tick: where IS the
                    # mouth, or how far off the line into it are you. Latched here, once, and
                    # handed to both consumers — the tones below and the phrase spoken on
                    # F9+I — so the pan cannot be pointing at the mouth while the speech reads
                    # out a steering correction. Never set in implement mode: there is no
                    # corridor to be outside of when the load is a metre from the tines.
                    if parsed["mode"] == "RAMP":
                        was_acquiring = ramp_acquire
                        ramp_acquire = _ramp_acquire(
                            parsed["range"], parsed["lateral"], ramp_acquire
                        )
                        if was_acquiring and not ramp_acquire:
                            audio_controller.trigger_dock_approach_cue()
                        parsed["acquire"] = ramp_acquire
                    else:
                        # Keep the next ramp acquisition armed without marking an implement
                        # packet as being in a ramp-only phase.
                        ramp_acquire = True
                        parsed["acquire"] = False
                    with state_lock:
                        last_dock = parsed
                        last_dock_fail = None
                    # The packet stays unchanged: its existing yaw field is the ramp null,
                    # while implement mode retains vertical metres.
                    audio_controller.update_dock_target(
                        parsed["range"],
                        parsed["lateral"],
                        (
                            parsed["yaw"]
                            if parsed["mode"] == "RAMP"
                            else parsed["vertical"]
                        ),
                        parsed["mode"],
                        acquire=parsed["acquire"],
                        bearing_deg=(
                            _ramp_bearing_deg(
                                parsed["range"], parsed["lateral"], parsed["yaw"]
                            )
                            if parsed["acquire"]
                            else 0.0
                        ),
                    )
                    # Announced once on change. Not cosmetic: the same two tones mean different
                    # things in the two modes, so an unannounced switch — driving away from a
                    # cannon, or picking up a set of forks — leaves the operator reading degrees
                    # as metres.
                    #
                    # In ramp mode the announcement also carries the machine's NAME, and
                    # re-fires when that changes. This is the only place the target is named
                    # now: the cane tap used to open with it on every press, which is twelve
                    # syllables of an answer nobody asked for twice a minute. Identity belongs
                    # to acquisition; geometry belongs to the tap. Implement mode is left
                    # alone, because there the proximity speech already names what is being
                    # approached and its target changes constantly in a yard.
                    with state_lock:
                        ramp_named = (
                            parsed["mode"] == "RAMP" and parsed["name"] != last_dock_name
                        )
                        changed = parsed["mode"] != last_dock_mode
                        announce = (changed or ramp_named) and dock_mode_active
                        if changed:
                            last_dock_mode = parsed["mode"]
                        last_dock_name = parsed["name"]
                    if announce:
                        say(
                            f"Ramp alignment, {parsed['name']}"
                            if parsed["mode"] == "RAMP"
                            else "Implement alignment",
                            exclude_from_buffer=True,
                        )
                    # Entry-gate earcon, on the transition INTO enterable only. Without it the
                    # gate is only ever discoverable by tapping F9+I, which means tilting
                    # blind and re-asking — and the whole point of the cue is that the answer
                    # arrives while the hand is still on the joystick. The mod already applies
                    # hysteresis to the depth, so the edge detected here cannot chatter; all
                    # that is tracked on this side is which side of it we were last on.
                    depth = parsed["entry_depth"] if parsed["mode"] == "IMPL" else None
                    if depth is None or depth < 0.0:
                        entry_ok = None
                    else:
                        # Same two thresholds as the mod, applied the same way round. Reading
                        # only the enter threshold here would re-introduce exactly the chatter
                        # the mod's hysteresis exists to prevent: a machine breathing on its
                        # suspension between 0.34 and 0.40 m would fire the earcon on every
                        # packet while the mod itself considered nothing to have changed.
                        thresh = (
                            IMPL_DOCK_ENTRY_EXIT_M if entry_ok else IMPL_DOCK_ENTRY_MIN_M
                        )
                        now_ok = depth >= thresh
                        if now_ok and entry_ok is False:
                            audio_controller.trigger_entry_cue()
                        entry_ok = now_ok
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
                if not implement_seen and time.time() >= next_probe:
                    _send_implement_cmd("ON")
                    next_probe = time.time() + IMPL_ANNOUNCE_PROBE_S
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


def _watch_scan_reply(before, timeout_s=1.5):
    """Speak only if the scan never comes back. `before` must be sampled by the CALLER
    before the command goes out: sampling it in here races the reply, and a scan that
    answered inside that window would be announced as a failure."""
    deadline = time.time() + timeout_s
    while time.time() < deadline:
        if _last_scan_reply_ts != before:
            return
        time.sleep(0.1)
    say("No scan. Terrain scanner not responding", exclude_from_buffer=True)


def _send_scan_cmd(command):
    try:
        cmd_sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        cmd_sock.sendto(command.encode("utf-8"), ("127.0.0.1", TERRAIN_SCAN_CMD_PORT))
        cmd_sock.close()
    except Exception as e:
        logger.error(f"Failed to send terrain scan command via UDP: {e}")


def trigger_terrain_scan_driving_only():
    """Start a terrain scan only while live driving telemetry is present.

    Unlike F9+Space, this path never falls through to a UI context action. Controller
    activation can therefore be delayed until after a screen transition without risking
    an unrelated button press in BeamNG's menus.
    """
    if not _world_is_active():
        say("Terrain scan is only available while driving", exclude_from_buffer=True)
        return
    scan_seen_before = _last_scan_reply_ts
    _send_scan_cmd("SCAN")
    threading.Thread(
        target=_watch_scan_reply, args=(scan_seen_before,), daemon=True
    ).start()


def _watch_poi_reply(before, timeout_s=1.5):
    """Report a missing POI response without confusing it with a scan response."""
    deadline = time.time() + timeout_s
    while time.time() < deadline:
        if _last_poi_reply_ts != before:
            return
        time.sleep(0.1)
    say(
        "No point of interest response. Terrain scanner not responding",
        exclude_from_buffer=True,
    )


def trigger_nearest_poi():
    """Ask the game for the closest big-map POI to the current vehicle."""
    if not _world_is_active():
        say(
            "Nearest point of interest is only available while driving",
            exclude_from_buffer=True,
        )
        return
    poi_seen_before = _last_poi_reply_ts
    _send_scan_cmd("NEAREST_POI")
    threading.Thread(
        target=_watch_poi_reply, args=(poi_seen_before,), daemon=True
    ).start()


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

    The local flag set here is only a seed for the speech reference: once scanner packets
    start arriving they carry the direction Lua actually resolved, and that wins. Setting
    it from the push alone is what let the two disagree in the cases Lua ignores the push.
    """
    global scanner_ref_reversed
    reverse = str(gear or "").strip().upper().startswith("R")
    scanner_ref_reversed = reverse
    _send_scanner_cmd("GEAR:R" if reverse else "GEAR:F")
    _push_obstacle_state(force=True, gear=gear)


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


def _clickspot_set_pending():
    """Clear published rows while Lua rebuilds them for the active vehicle."""
    global clickspot_mode_active, clickspot_trigger_list, clickspot_list_loading
    global _clickspot_trigger_building, _clickspot_expected_count
    global current_clickspot_index
    with state_lock:
        clickspot_mode_active = True
        clickspot_trigger_list = []
        clickspot_list_loading = True
        _clickspot_trigger_building = None
        _clickspot_expected_count = None
        current_clickspot_index = 0


def _clickspot_set_off():
    """Synchronize Python with a Lua-side detector shutdown or world reset."""
    global clickspot_mode_active, clickspot_trigger_list, clickspot_list_loading
    global _clickspot_trigger_building, _clickspot_expected_count
    global current_clickspot_index
    with state_lock:
        clickspot_mode_active = False
        clickspot_trigger_list = []
        clickspot_list_loading = False
        _clickspot_trigger_building = None
        _clickspot_expected_count = None
        current_clickspot_index = 0


def _clickspot_begin_list(count):
    """Begin an atomic clickspot list transfer; publish zero rows immediately."""
    global clickspot_mode_active, clickspot_trigger_list, clickspot_list_loading
    global _clickspot_trigger_building, _clickspot_expected_count
    global current_clickspot_index
    count = max(0, int(count))
    with state_lock:
        clickspot_mode_active = True
        clickspot_trigger_list = []
        _clickspot_trigger_building = {}
        _clickspot_expected_count = count
        clickspot_list_loading = count > 0
        current_clickspot_index = 0
        if count == 0:
            _clickspot_trigger_building = None
            _clickspot_expected_count = None


def _clickspot_append_row(cache_idx, trigger_id, display_name):
    """Stage one row and atomically publish the list when its announced count arrives."""
    global clickspot_trigger_list, clickspot_list_loading
    global _clickspot_trigger_building, _clickspot_expected_count
    global current_clickspot_index
    with state_lock:
        if _clickspot_trigger_building is None or _clickspot_expected_count is None:
            logger.warning("Clickspot row arrived outside a list transfer; ignoring it")
            return False
        if cache_idx < 0 or cache_idx >= _clickspot_expected_count:
            logger.warning(
                "Clickspot cache index %r is outside the announced list", cache_idx
            )
            return False
        _clickspot_trigger_building[cache_idx] = (
            cache_idx,
            trigger_id,
            display_name,
        )
        if len(_clickspot_trigger_building) != _clickspot_expected_count:
            return False
        clickspot_trigger_list = [
            _clickspot_trigger_building[idx]
            for idx in sorted(_clickspot_trigger_building)
        ]
        _clickspot_trigger_building = None
        _clickspot_expected_count = None
        clickspot_list_loading = False
        current_clickspot_index = 0
        return True


def _announce_clickspot_action(display_name, failure_reason=None):
    """Speak clickspot activation feedback only when its opt-in setting is enabled."""
    if announce_clickspot_actions:
        if failure_reason is None:
            message = f"Jumped to {display_name}"
        else:
            message = f"Cannot jump, {failure_reason}"
        say(message, exclude_from_buffer=True)


def clickspot_listener(audio_controller, stop_event):
    """Listens for UDP packets from clickspotAccessible.lua — trigger hover data and list."""
    import ctypes

    global clickspot_last_hover_id

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

                if text == "TRIGGER_LIST_PENDING":
                    _clickspot_set_pending()
                    continue

                if text == "TRIGGER_LIST_OFF":
                    _clickspot_set_off()
                    continue

                if text.startswith("TRIGGER_LIST:"):
                    try:
                        count = int(text[13:])
                    except ValueError:
                        logger.warning("Malformed clickspot list count: %r", text)
                        continue
                    _clickspot_begin_list(count)
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
                        try:
                            cache_idx = int(parts[0])
                            trigger_id = int(parts[1])
                        except ValueError:
                            logger.warning("Malformed clickspot row: %r", text)
                            continue
                        _clickspot_append_row(cache_idx, trigger_id, parts[2])
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
                        # Menu activation never beeps; its success speech is opt-in.
                        _announce_clickspot_action(display_name)
                    continue

                if text.startswith("SNAP_FAIL:"):
                    reason = text[10:]
                    _announce_clickspot_action(None, failure_reason=reason)
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


def environment_listener(stop_event):
    """Listens for environment rows from environmentAccessible.lua.

    Silent like the bindings listener: rows are only ever spoken when the user
    opens the browser (F9 then N) or edits a value from inside it.
    """
    global _env_rows, _env_level, _env_can_change, _env_building, _env_unavailable

    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        sock.bind(("127.0.0.1", ENV_LISTEN_PORT))
        sock.settimeout(0.2)
        logger.info(f"Environment listener started on port {ENV_LISTEN_PORT}")

        while not stop_event.is_set():
            try:
                data, _addr = sock.recvfrom(2048)
                text = data.decode("utf-8", errors="ignore").strip()
                if not text:
                    continue

                if text.startswith("ENV_BEGIN:"):
                    # ENV_BEGIN:<count>;level=<id>;canChange=<0|1>
                    _env_building = []
                    _env_unavailable = ""
                    head = _parse_env_fields(text[len("ENV_BEGIN:") :])
                    _env_level = head.get("level", "")
                    _env_can_change = head.get("canChange", "1") == "1"
                    continue

                if text.startswith("ENV:"):
                    if _env_building is None:
                        continue
                    row = _parse_env_fields(text[len("ENV:") :])
                    if row.get("key"):
                        _env_building.append(row)
                    continue

                if text == "ENV_END":
                    # Swapped in whole, so a browser opened mid-push never sees a
                    # torn list.
                    if _env_building is not None:
                        _env_rows = _env_building
                        _env_building = None
                        _env_notify_refresh()
                    continue

                if text.startswith("ENV_UNAVAILABLE:"):
                    _env_rows = []
                    _env_building = None
                    _env_unavailable = text[len("ENV_UNAVAILABLE:") :].strip()
                    continue

                if text.startswith("ENV_ERROR:"):
                    # A refusal is the one environment message that speaks on its
                    # own: it is always the direct answer to a key the user just
                    # pressed, and staying quiet would read as the key doing
                    # nothing.
                    reason = text[len("ENV_ERROR:") :].strip()
                    logger.warning(f"Environment change refused: {reason}")
                    say(reason or "Environment change refused",
                        exclude_from_buffer=True)
                    continue

            except socket.timeout:
                continue
            except OSError:
                if stop_event.is_set():
                    break
                logger.error("Environment listener socket error.")
    finally:
        try:
            sock.close()
        except Exception:
            pass
        logger.info("Environment listener stopped.")


def vehicle_info_listener(stop_event):
    """Listens for vehicle specification rows from vehicleInfo.lua.

    Silent by construction: rows are only ever spoken because the user asked for
    them (F9 then SPACE on the stock selector, `i` in the spawner). The old
    version of this feature read automatically, which is exactly why it was
    unusable.

    The reply is chunked, so a whole answer is accumulated and only published on
    INFO_END -- a browser opened against a half-arrived list would read a
    truncated spec sheet as though it were the whole thing. INFOFAIL publishes a
    reason instead, and both paths set the event so a waiting caller is never
    left sitting out its full timeout.
    """
    global _vinfo_rows, _vinfo_building, _vinfo_error, _vinfo_absent

    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        sock.bind(("127.0.0.1", VEHICLE_INFO_LISTEN_PORT))
        sock.settimeout(0.2)
        logger.info(f"Vehicle info listener started on port {VEHICLE_INFO_LISTEN_PORT}")

        while not stop_event.is_set():
            try:
                data, _addr = sock.recvfrom(8192)
                text = data.decode("utf-8", errors="ignore").strip()
                if not text:
                    continue

                # Any packet at all proves the mod half is alive and speaking this
                # protocol, which is the only evidence needed to drop the latch.
                _vinfo_absent = False

                if text.startswith("INFO_BEGIN:"):
                    _vinfo_building = []
                    _vinfo_error = ""
                    _vinfo_event.clear()
                    continue

                if text.startswith("INFO_ROW:"):
                    if _vinfo_building is None:
                        continue
                    try:
                        _vinfo_building.append(json.loads(text[len("INFO_ROW:"):]))
                    except Exception:
                        logger.warning("Malformed INFO_ROW dropped.")
                    continue

                if text == "INFO_END":
                    if _vinfo_building is not None:
                        _vinfo_rows = _vinfo_building
                        _vinfo_building = None
                        _vinfo_error = ""
                    _vinfo_event.set()
                    continue

                if text.startswith("INFOFAIL:"):
                    # "<code>;<sentence>". The code exists because one of these
                    # causes -- notselector -- is the answer on every screen in
                    # the game, including the one where F9 SPACE means "scan the
                    # terrain"; it has to fall through silently while the others
                    # are spoken. Prose cannot carry that distinction.
                    _vinfo_rows = []
                    _vinfo_building = None
                    body = text[len("INFOFAIL:"):].strip()
                    code, sep, sentence = body.partition(";")
                    if not sep:
                        # An older mod half sent the sentence alone. Treat it as a
                        # real refusal rather than guessing it was notselector,
                        # which would silently swallow every failure.
                        code, sentence = "unknown", body
                    _vinfo_error = (code.strip(), sentence.strip())
                    _vinfo_event.set()
                    continue

            except socket.timeout:
                continue
            except OSError:
                if stop_event.is_set():
                    break
                logger.error("Vehicle info listener socket error.")
    finally:
        try:
            sock.close()
        except Exception:
            pass
        logger.info("Vehicle info listener stopped.")


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
last_protocol_flags = 0
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

# Trailer articulation — the yaw between the driven vehicle and whatever is hooked to it,
# written by trailer_angle_listener under state_lock. Same quantity as last_articulation_deg
# with the hinge moved to the coupler, and it drives the same tone: see TRAILER_FULL_DEG.
#
# The angle is a live reading and NOT a latch, so it has to expire. The mod sends an explicit
# TRAILER:CLEAR on uncoupling, which is the normal way this goes quiet — but a mod/Python
# version skew, a game exit or a crashed extension all end the feed with the last angle still
# standing, and a jackknife tone that stays on because nothing arrived to turn it off is the
# worst failure this feature has. Hence the stamp and TRAILER_STALE_SEC.
last_trailer_deg = 0.0  # degrees between vehicle and trailer, positive = LEFT
last_trailer_id = None  # game object id of the trailer, or None when nothing is coupled
last_trailer_name = ""
last_trailer_stamp = 0.0  # time.monotonic() of the last packet; 0.0 = never
# Full scale for the tone. Feeding the WL-40's normalised -1..1 scale means this is the angle
# at which the pitch pins at its top note. 45 deg puts the existing 0.05 deadzone at about 2.3
# deg of slack — the "in line, stay silent" band — and reaches full pitch around 38 deg. Past
# that it clamps, which is the honest answer: you have already jackknifed. MUST match audio.py's
# TRAILER_FULL_DEG; trailer_angle_sim.lua greps both.
TRAILER_FULL_DEG = 45.0
TRAILER_STALE_SEC = 1.0  # no packet for this long and the tone goes silent

# --- Route beacon state, written by route_beacon_listener under state_lock ---
# The DESTINATION, not a bearing. routeBeacon.lua sends a world position because the
# destination is static while the bearing to it is not: deriving the bearing here means
# it is recomputed from the 60 Hz MotionSim position and heading rather than arriving at
# the mod's send rate, so the beacon pans smoothly and a dropped datagram cannot jog it.
route_beacon_active = False  # the user's F9 Ctrl+W toggle
route_dest_x = None  # None means no route is set
route_dest_y = None
# No route_dest_z: the beacon is flattened like every other bearing in this mod, so the
# destination's height is not a thing anything here reads. It still rides the wire, where
# it completes the destination and leaves the tail open for an altitude readout later --
# but it is deliberately not held as state nothing consumes.
route_remaining_m = 0.0  # distance along the ROUTE, which is not the crow-flies range
route_beacon_stamp = 0.0  # time.monotonic() of the last packet; 0.0 = never
# The mod sends ROUTE:CLEAR when a route goes away, so this age-out is the failure path
# only -- a crashed extension or a version skew. It matters anyway: without it the beacon
# would keep pulsing at the last bearing it was told, which is a confident wrong answer
# about where the destination is. Must stay comfortably above routeBeacon's HEARTBEAT_S
# (0.35) so a live, unchanging route can never expire.
ROUTE_STALE_SEC = 1.5

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
last_tire_pressure_f, last_tire_pressure_r = 0.0, 0.0
last_tire_temp_f, last_tire_temp_r = 0.0, 0.0
last_brake_temp_f, last_brake_temp_r = 0.0, 0.0
last_telemetry_presence = WHEEL_POS_CORNERS
last_signal_left_input, last_signal_right_input, last_hazard_enabled = (
    False,
    False,
    False,
)
last_lightbar = -1
last_fog = -1

# Status keyboard mode
status_keyboard_mode_active = False
current_status_metric_index = 0
current_functions_item_index = 0
current_clickspot_index = 0
current_accessibility_screen_index = 0
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
# True between COUPLER_START and the run ending (coupled, target lost, vehicle switch).
# The scanner toggle must not end a coupling you are halfway through -- see section 2d
# of vehicleScanner.lua. Lua keeps sending COUPLER: packets with the scanner off, so
# clearing the tone here would silence the instrument while its feed was still live.
coupler_run_active = False
obstacle_mode_active = False
obstacle_warning_sensitivity = "normal"
_obstacle_state_last = None
_obstacle_state_last_sent = 0.0
OBSTACLE_STATE_HEARTBEAT_S = 0.5
OBSTACLE_STEERING_DELTA = 0.03
OBSTACLE_PEDAL_DELTA = 0.05
road_mode_active = False
road_follow_guidance_enabled = True
road_junction_speech_enabled = True
road_junction_earcon_enabled = True
road_include_private = False
ROAD_GUIDANCE_FEED = RoadGuidanceFeed()
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
last_slip_active = False
last_slip_kind = 0
last_slip_mag = 0.0
last_tc_active = False

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
# Which end of the vehicle the scanner is currently measuring and referencing its bearing
# from. The bearing on the wire is relative to the DIRECTION OF TRAVEL: in reverse a target
# dead astern arrives as ~0 deg, and speech that reads that literally says "in front of you"
# about the thing you are backing into. The tones need no equivalent — a bearing of 0 is dead
# ahead on the stereo image either way, which is the point of re-referencing it.
#
# Taken from the scanner packet's fourth field, i.e. from the direction Lua actually resolved,
# not from the GEAR:R/GEAR:F we pushed. Mirroring the push was wrong in both of the cases where
# Lua does not use it: it ages the push out after two seconds and falls back to velocity, and
# it clears the push on a vehicle switch. This only falls back to the pushed gear against a mod
# too old to send the field.
scanner_ref_reversed = False

# Scanner periodic distance callout settings (applied via _apply_live_config)
scanner_distance_callout_enabled = False
scanner_distance_callout_interval = 10

# Loader implement proximity speech (applied via _apply_live_config). The tones themselves
# are configured inside audio.py's apply_config; only the speech gate lives here.
announce_implement_proximity = True
announce_cannon_shot = True
announce_binding_learn_description = False
# "Bucket" / "Forks" / "Grapple" once the mod reports one, else "" for every other vehicle.
# Read by toggle_scan_mode, which needs to say which end of the machine it is aiming from.
_implement_word_current = ""

# When the terrain scanner last answered. The scan speaks nothing on success — the reference
# ping IS the acknowledgement — so this is the only way to tell "no reply" from "playing".
_last_scan_reply_ts = 0.0
# Nearest-POI requests share terrainScanner.lua's socket but have their own reply latch, so
# a scan and a POI lookup issued close together cannot satisfy one another's timeout.
_last_poi_reply_ts = 0.0

# When telemetry last arrived on 4444. That feed comes from the vehicle VM's own protocol,
# so it flows only while a level is loaded with a vehicle in it — which is exactly the
# condition under which a terrain scan means anything, and exactly the condition under which
# the UI has no context action to offer, since both screens that DO offer one (the freeroam
# wizard and the configurator) are pre-level. It is driven by physics, so it also stops while
# the game is paused; a scan is declined there, which is the conservative way round.
_last_telemetry_ts = 0.0
WORLD_ACTIVE_TELEMETRY_S = 2.0


def _world_is_active():
    """Is there a world to scan, as opposed to a menu to press a button in?"""
    return (time.time() - _last_telemetry_ts) < WORLD_ACTIVE_TELEMETRY_S


def _mcp_snapshot_state(sections=None):
    """Flat snapshot of live state for the MCP server.

    Everything is copied out under the lock and returned; no I/O, no say(), no nested
    lock while held. state_lock is a plain non-reentrant Lock that the telemetry loop
    already takes, so anything slower than a few reads here would stall the feed.
    """
    want = set(sections) if sections else None

    def on(name):
        return want is None or name in want

    out = {}
    with state_lock:
        if on("telemetry"):
            out["telemetry"] = {
                "speed_ms": last_speed_ms,
                "rpm": last_rpm,
                "rpm_max": last_rpm_max,
                "gear": last_gear_str,
                "fuel": last_fuel,
                "turbo": last_turbo,
                "engine_temp": last_engtemp,
                "oil_temp": last_oiltemp,
                "oil_pressure": last_oil_pressure,
                "throttle": last_throttle,
                "brake": last_brake,
                "clutch": last_clutch,
                "steering": last_steering,
                "actual_steering": last_actual_steering,
                "heading": last_heading,
                "ground_speed_ms": last_ground_speed_ms,
            }
        if on("position"):
            out["position"] = {
                "x": last_pos_x,
                "y": last_pos_y,
                "z": last_pos_z,
                "yaw_rad": last_yaw_rad,
                "roll_rad": last_roll_rad,
                "pitch_rad": last_pitch_rad,
            }
        if on("implement"):
            out["implement"] = {
                "flags": last_implement_flags,
                "edge_height_m": last_implement_edge_height,
                "min_clearance_m": last_implement_min_clearance,
                "tilt_deg": last_implement_tilt_deg,
                "tilt_percent": last_implement_tilt_percent,
                "lift_m": last_implement_lift,
                "activity": last_implement_activity,
                "articulation_deg": last_articulation_deg,
            }
        if on("trailer"):
            # Both the raw angle and the normalised value that actually reaches the tone,
            # because "the number is right but it sounds wrong" and "the number is wrong"
            # are the two failure modes here and only the pair separates them.
            out["trailer"] = {
                "coupled": last_trailer_id is not None,
                "id": last_trailer_id,
                "name": last_trailer_name,
                "angle_deg": last_trailer_deg,
                "artic_norm": _trailer_artic_norm(),
                "age_s": (
                    (time.monotonic() - last_trailer_stamp)
                    if last_trailer_stamp > 0.0
                    else None
                ),
            }
        if on("dock"):
            out["dock"] = {
                "mode_active": dock_mode_active,
                "last_dock": last_dock,
                "last_fail": last_dock_fail,
                "mode": last_dock_mode,
                "name": last_dock_name,
            }
        if on("cannon"):
            out["cannon"] = {
                "active": cannon_active,
                "kind": cannon_kind,
                "aim": last_cannon_aim,
                "last_shot": last_cannon_shot,
                "session_shots": len(cannon_shot_session or []),
            }
        if on("scanner"):
            dist = last_scanner_distance
            out["scanner"] = {
                "target_name": last_scanner_target_name,
                "distance_m": None if dist == float("inf") else dist,
                "bearing_deg": last_scanner_bearing,
                "approach_deg": last_scanner_approach_deg,
                "reference_reversed": scanner_ref_reversed,
            }
        if on("modes"):
            out["modes"] = {
                "scan": scan_mode_active,
                "coupler_distance": coupler_dist_mode,
                "obstacle": obstacle_mode_active,
                "road": road_mode_active,
                "dock": dock_mode_active,
            }
        if on("liveness"):
            # _last_telemetry_ts starts at 0, so an age measured against it before the
            # first packet is the age of the epoch -- a plausible-looking number that
            # means nothing. Report "never" as null instead.
            ever = _last_telemetry_ts > 0
            out["liveness"] = {
                "world_active": ever
                and (time.time() - _last_telemetry_ts) < WORLD_ACTIVE_TELEMETRY_S,
                "seconds_since_telemetry": (
                    round(time.time() - _last_telemetry_ts, 2) if ever else None
                ),
                "telemetry_ever_seen": ever,
                "world_active_threshold_s": WORLD_ACTIVE_TELEMETRY_S,
            }
    if on("slots"):
        with _slots_lock:
            out["slots"] = {
                "vehicles": {k: dict(v) for k, v in _vehicle_slots.items()},
                "selected": sorted(_selected_slots),
                "target": _target_slot,
            }
    return out


def _road_diagnostic_telemetry_snapshot():
    """Fields needed to correlate lane instructions with driver input and traction."""
    with state_lock:
        return {
            "position": {"x": last_pos_x, "y": last_pos_y, "z": last_pos_z},
            "speed_ms": last_speed_ms,
            "ground_speed_ms": last_ground_speed_ms,
            "wheel_speed_ms": last_speed_ms,
            "throttle": last_throttle,
            "brake": last_brake,
            "steering": last_steering,
            "actual_steering": last_actual_steering,
            "steering_input": last_steering_input,
            "heading": last_heading,
            "roll_rad": last_roll_rad,
            "pitch_rad": last_pitch_rad,
            "gear": last_gear_str,
            "traction_control_active": last_tc_active,
            "slip_detector_enabled": slip_mode_active,
            "slip_active": last_slip_active,
            "slip_kind": last_slip_kind,
            "slip_magnitude_mps": last_slip_mag,
            "slip_filtered_mps": _ls_slip_smooth,
        }

# Docking instrument. dock_mode_active is the user-facing toggle; last_dock is the most
# recent readout from the mod, or None when there is nothing in range. Written by
# implement_listener under state_lock and read by the F9+I cane tap and the audio callback.
dock_mode_active = False
last_dock = None
# Why the instrument has nothing to say, when it has nothing to say. Held rather than
# spoken, and read out by F9+I on request.
last_dock_fail = None
# True while the vehicle being driven has a ramp of its own, i.e. you are sitting in the
# cannon rather than lining up with it. Pushed by the mod on change; see the F9+I handler,
# which uses it to answer the question you actually have at that point.
cannon_active = False
cannon_kind = "NONE"
# The deck state of the ramp machine being driven, or None when it has no ramp. A fact about
# your own vehicle rather than about anything you are lining up with, which is why it is latched
# beside cannon_active and not inside the docking state above.
last_ramp_self = None
# Old Cannon live barrel/target solution. The range sentinel is negative when no scanner
# target is selected, while elevation remains valid for the on-demand angle readout.
last_cannon_aim = None
# Which answer the alignment instrument last gave ("IMPL"/"RAMP"), or None for "not known
# yet". Shared between the listener and the F9 handlers so the two cannot both announce the
# same change. Deliberately NOT cleared on DOCKCLEAR or DOCKFAIL: losing the target is not a
# change of mode, and clearing it there would re-announce every time you drifted across the
# feed's edge. Only a part swap, a vehicle change or the toggle resets it.
last_dock_mode = None
# ...and which ramp machine it last named, so acquiring a different one re-announces while
# tapping the same one does not. Latched under exactly the same rules as the mode above and
# for the same reason: cleared on DOCKCLEAR it would re-announce every time the feed's edge
# was crossed, which on a ramp approach is several times a minute.
last_dock_name = None

# Cannon shot outcomes. Written by cannon_shot_listener under state_lock; nothing else writes
# them. Kept in memory only and never persisted: the useful comparison is against the shot you
# fired a minute ago, at settings you still remember, and a file would outlive that context
# without carrying it. The mod holds the authoritative session count, so a beamtel restart
# mid-session leaves this list short but the readout still correct.
last_cannon_shot = None
cannon_shot_session = []

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
announce_clickspot_actions = False
clickspot_trigger_list = []  # list of (cache_index, trigger_id, display_name)
clickspot_list_loading = False
_clickspot_trigger_building = None
_clickspot_expected_count = None
clickspot_last_hover_id = -1

# Vehicle-Specific Bindings State
# Rebuilt silently on every vehicle load — nothing here is ever announced; the
# list only becomes audible when the user opens the browser with F9 then B.
# Environment browser state (environmentAccessible.lua, F9 then N). Rows are
# dicts of the named fields the mod sends, kept raw so a field added on the Lua
# side needs no parser change here.
_env_rows = []
_env_level = ""
_env_can_change = True
_env_building = None  # accumulator between ENV_BEGIN and ENV_END
_env_unavailable = ""

# Vehicle Information State (vehicleInfo.lua, ports 4477/4478)
# _vinfo_building is the accumulator between INFO_BEGIN and INFO_END; the finished
# list is swapped into _vinfo_rows whole so a reader never sees a torn spec sheet.
_vinfo_rows = []
_vinfo_building = None
_vinfo_error = None  # None, or a (code, sentence) pair from INFOFAIL
_vinfo_event = threading.Event()
# Set once the mod half has failed to answer, and cleared by the first packet that ever
# arrives. F9 SPACE is the terrain scan and is pressed while DRIVING, so paying the full
# timeout on every press to re-discover a mod that is not there is a regression on a key that
# has always worked -- and bng_mod/ is a live junction, so a half that does not know this
# command is the ordinary consequence of updating one side. See request_vehicle_info.
_vinfo_absent = False

# ---------- UI page text (bnvdaRuntime.js) ----------
# The UI runtime latches which readable screen is on -- today only the mod repository /
# automation details pages, whose spec table and description body carry nothing focusable.
# The latch exists so the controller Functions menu can hide the entry everywhere else;
# the readout itself always asks the runtime, which answers `notdetails` if the screen has
# since gone.
_ui_screen_context = ""
_ui_screen_title = ""
_page_text_lines = []
_page_text_title = ""
_page_text_error = None  # None, or a (code, sentence) pair
_page_text_event = threading.Event()
# Same latch, and the same reason, as _vinfo_absent: F9 SPACE is the terrain scan and is
# pressed while driving, so a UI half that predates this feature must not charge the full
# timeout to every press. See request_page_text.
_page_text_absent = False

_vehicle_bindings_list = []  # list of (cache_index, line)
_vehicle_bindings_vehicle = ""
_vehicle_bindings_building = None  # accumulator between BEGIN and END
_vehicle_bindings_building_name = ""

# Learn Bindings Mode. This flag drives the keepalive only -- the mod owns the real state and
# reports it back on the LEARNMODE: line, which is also what gets spoken.
_binding_learn_active = False

# Generic Virtual Browser State
_vbrowser_lines = []
_vbrowser_index = 0
_vbrowser_hooks = []
_vbrowser_active = False
_vbrowser_on_enter = None
_vbrowser_on_adjust = None
_vbrowser_on_escape = None
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
#  Command dispatch worker
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
_command_queue = queue.Queue()
_command_worker_thread = None


class _SynthKeyEvent:
    """Stand-in for keyboard.KeyboardEvent, carrying what the handlers read.

    The real event object belongs to the hook callback that is about to return;
    handlers now run later, on the worker, so they get a snapshot instead.
    """

    __slots__ = ("name", "event_type")

    def __init__(self, name, event_type="down"):
        self.name = name
        self.event_type = event_type


def _command_dispatch(fn, *args):
    """Queue fn(*args) on the serial command worker. Safe inside a hook or callback."""
    try:
        _command_queue.put_nowait((fn, args))
    except Exception:
        logger.exception("Failed to queue command")


def _kb_enqueue(handler):
    """Wrap a suppressed-hook handler so its body runs on the worker.

    The wrapper returns None, which keyboard reads as "suppress this key" — the
    same verdict every handler wrapped here already returned. Mirrors
    vehicle_spawner._enqueue, which solved this for the F11 modal.
    """

    def wrapper(event):
        _command_dispatch(handler, event)

    return wrapper


def _command_worker_loop():
    while True:
        fn, args = _command_queue.get()
        try:
            fn(*args)
        except Exception:
            # A handler that raised must not take the worker down with it, or
            # every later command would be silently dropped.
            logger.exception("Command handler raised")
        finally:
            _command_queue.task_done()


def _start_command_worker():
    global _command_worker_thread
    if _command_worker_thread is not None:
        return
    _command_worker_thread = threading.Thread(
        target=_command_worker_loop, name="command-dispatch", daemon=True
    )
    _command_worker_thread.start()


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
    _command_dispatch(_kb_run_layer_command, layer, name, dict(_live_mods))
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
    ("w", True, False, False): "Toggle route beacon",
    ("w", False, True, False): "Nearest point of interest",
    ("d", False, False, False): "Scanner distance and orientation",
    ("d", False, True, False): "Scanner relative bearing to target",
    ("d", True, False, False): "Toggle drift detection",
    ("d", True, True, False): "Toggle coupler distance callouts",
    ("m", False, False, False): "Damage report",
    ("s", True, False, False): "Toggle status mode",
    ("b", False, False, False): "Browse vehicle bindings",
    ("n", False, False, False): "Browse environment settings",
    ("b", True, False, False): "Toggle buffer mode",
    ("c", True, False, False): "Toggle pedal tones",
    ("v", True, False, False): "Toggle vehicle scanner",
    (
        "v",
        False,
        True,
        False,
    ): (
        "Align to a ramp when the docking instrument is on, otherwise to a trailer "
        "coupler, starting coupler tracking and the attach monitor"
    ),
    ("o", True, False, False): "Toggle obstacle detection",
    ("r", True, False, False): "Toggle road detection",
    ("r", True, True, False): "Read road guidance status",
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
    ("space", False, False, False): "Read vehicle information on the vehicle selector, scan terrain when driving, or activate the on-screen control in a menu",
    ("n", True, False, False): "Toggle accessible node grabber",
    ("c", True, True, False): "Toggle clickspot detection",
    ("c", True, True, True): "Browse clickspots",
    ("i", False, False, False): "Alignment readout, or cannon aim when in a cannon",
    ("i", False, True, False): "Cycle alignment reference band",
    ("i", True, False, False): "Toggle alignment instrument (implement or ramp)",
    ("b", False, True, False): "Toggle learn bindings mode",
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


# Shared status catalog for keyboard and controller navigation
def _wheel_status_metric(label, category, value_getter, position_flag):
    return {
        "label": label,
        "category": category,
        "getValue": value_getter,
        "isAvailable": lambda: protocol_mode == "extended"
        and bool(int(last_telemetry_presence) & position_flag),
    }


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
    {
        "label": "Clutch Temperature",
        "getValue": lambda: fmt_temp_c_or_f(last_clutch_temp),
        "isAvailable": lambda: protocol_mode == "extended"
        and bool(int(last_telemetry_presence) & TELEMETRY_HAS_CLUTCH),
    },
    {
        "label": "Turbo Pressure",
        "getValue": lambda: fmt_turbo(last_turbo),
        "isAvailable": lambda: bool(int(last_protocol_flags) & OG_TURBO),
    },
    # {'label': 'Oil Pressure', 'getValue': lambda: fmt_turbo(last_oil_pressure), 'isAvailable': lambda: protocol_mode == 'extended'},
    # {'label': 'Air System Pressure', 'getValue': lambda: fmt_pressure(last_air_pressure), 'isAvailable': lambda: protocol_mode == 'extended' and last_air_pressure_max > 0},
    # {'label': 'Lateral G-Force', 'getValue': lambda: (f"{last_g_lat:.2f}", "G"), 'isAvailable': lambda: protocol_mode == 'extended'},
    # {'label': 'Longitudinal G-Force', 'getValue': lambda: (f"{last_g_lon:.2f}", "G"), 'isAvailable': lambda: protocol_mode == 'extended'},
    _wheel_status_metric(
        "front",
        "Tire pressures",
        lambda: fmt_pressure(last_tire_pressure_f),
        WHEEL_POS_F,
    ),
    _wheel_status_metric(
        "front left",
        "Tire pressures",
        lambda: fmt_pressure(last_tire_pressure_fl),
        WHEEL_POS_FL,
    ),
    _wheel_status_metric(
        "front right",
        "Tire pressures",
        lambda: fmt_pressure(last_tire_pressure_fr),
        WHEEL_POS_FR,
    ),
    _wheel_status_metric(
        "rear",
        "Tire pressures",
        lambda: fmt_pressure(last_tire_pressure_r),
        WHEEL_POS_R,
    ),
    _wheel_status_metric(
        "rear left",
        "Tire pressures",
        lambda: fmt_pressure(last_tire_pressure_rl),
        WHEEL_POS_RL,
    ),
    _wheel_status_metric(
        "rear right",
        "Tire pressures",
        lambda: fmt_pressure(last_tire_pressure_rr),
        WHEEL_POS_RR,
    ),
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
    {
        # The same quantity as Frame Articulation with the hinge at the coupler, and worth
        # having as a number and not only as a tone: the tone says how far and which way, but
        # confirming WHICH trailer the mod picked on a rig hooked at both ends is a question
        # only a name answers. Availability is the live trailer id rather than the angle,
        # since 0 degrees is a perfectly ordinary reading here — it means correctly in line.
        "label": "Trailer Angle",
        "getValue": lambda: (
            f"{abs(last_trailer_deg):.0f}",
            "degrees "
            + (
                "in line"
                if abs(last_trailer_deg) < 1.0
                else ("left" if last_trailer_deg > 0 else "right")
            )
            + (f", {last_trailer_name}" if last_trailer_name else ""),
        ),
        "isAvailable": lambda: last_trailer_id is not None
        and last_trailer_stamp > 0.0
        and (time.monotonic() - last_trailer_stamp) <= TRAILER_STALE_SEC,
    },
    # {'label': 'Tire Temps (FL, FR, RL, RR)', 'getValue': lambda: (f"{fmt_temp_c_or_f(last_tire_temp_fl)[0]}, {fmt_temp_c_or_f(last_tire_temp_fr)[0]}, {fmt_temp_c_or_f(last_tire_temp_rl)[0]}, {fmt_temp_c_or_f(last_tire_temp_rr)[0]}", "F" if UNITS_MODE == 'imperial' else "C"), 'isAvailable': lambda: protocol_mode == 'extended'},
    _wheel_status_metric(
        "front",
        "Brake temperatures",
        lambda: fmt_temp_c_or_f(last_brake_temp_f),
        WHEEL_POS_F,
    ),
    _wheel_status_metric(
        "front left",
        "Brake temperatures",
        lambda: fmt_temp_c_or_f(last_brake_temp_fl),
        WHEEL_POS_FL,
    ),
    _wheel_status_metric(
        "front right",
        "Brake temperatures",
        lambda: fmt_temp_c_or_f(last_brake_temp_fr),
        WHEEL_POS_FR,
    ),
    _wheel_status_metric(
        "rear",
        "Brake temperatures",
        lambda: fmt_temp_c_or_f(last_brake_temp_r),
        WHEEL_POS_R,
    ),
    _wheel_status_metric(
        "rear left",
        "Brake temperatures",
        lambda: fmt_temp_c_or_f(last_brake_temp_rl),
        WHEEL_POS_RL,
    ),
    _wheel_status_metric(
        "rear right",
        "Brake temperatures",
        lambda: fmt_temp_c_or_f(last_brake_temp_rr),
        WHEEL_POS_RR,
    ),
]


def _function_item(
    label,
    category,
    name=None,
    ctrl=False,
    shift=False,
    alt=False,
    is_available=lambda: True,
    handler=None,
):
    item = {
        "label": label,
        "category": category,
        "isAvailable": is_available,
    }
    if handler is not None:
        item["handler"] = handler
    else:
        item["command"] = (name, ctrl, shift, alt)
    return item


# A flat controller catalog over the existing F9 command layer. Categories are spoken
# boundaries, not nested menus. Availability is deliberately live: modes toggled by one
# item can expose or remove dependent items before the next controller press.
FUNCTION_ITEMS = [
    _function_item("Toggle vehicle scanner", "Vehicle scanner", "v", ctrl=True),
    _function_item(
        "Next vehicle scanner target", "Vehicle scanner", "tab"
    ),
    _function_item(
        "Previous vehicle scanner target",
        "Vehicle scanner",
        "tab",
        shift=True,
    ),
    _function_item(
        "Lock onto nearest vehicle",
        "Vehicle scanner",
        "tab",
        ctrl=True,
    ),
    _function_item(
        "Distance and orientation",
        "Vehicle scanner",
        "d",
        is_available=lambda: scan_mode_active,
    ),
    _function_item(
        "Relative bearing",
        "Vehicle scanner",
        "d",
        shift=True,
        is_available=lambda: scan_mode_active,
    ),
    _function_item(
        "Trailer or ramp alignment",
        "Vehicle scanner",
        "v",
        shift=True,
        is_available=lambda: scan_mode_active or dock_mode_active,
    ),
    _function_item(
        "Coupler distance callouts",
        "Vehicle scanner",
        "d",
        ctrl=True,
        shift=True,
        is_available=lambda: scan_mode_active,
    ),
    _function_item("Toggle alignment instrument", "Alignment", "i", ctrl=True),
    _function_item("Alignment or cannon readout", "Alignment", "i"),
    _function_item("Cycle reference band", "Alignment", "i", shift=True),
    _function_item("Pedal tones", "Driving assistance", "c", ctrl=True),
    _function_item("Heading guidance", "Driving assistance", "h", ctrl=True),
    _function_item(
        "Coordinate guidance",
        "Driving assistance",
        "g",
        ctrl=True,
        is_available=lambda: marked_coord_x is not None,
    ),
    _function_item("Drift detection", "Driving assistance", "d", ctrl=True),
    _function_item(
        "Low-speed detection", "Driving assistance", "l", ctrl=True, shift=True
    ),
    _function_item("Wheel-slip detection", "Driving assistance", "k", ctrl=True),
    _function_item("Obstacle detection", "Driving assistance", "o", ctrl=True),
    _function_item("Road detection", "Driving assistance", "r", ctrl=True),
    _function_item(
        "Road-status readout", "Driving assistance", "r", ctrl=True, shift=True
    ),
    _function_item("Mark waypoint", "Waypoints", "c", shift=True),
    _function_item(
        "Speak marked coordinates",
        "Waypoints",
        "c",
        alt=True,
        is_available=lambda: marked_coord_x is not None,
    ),
    _function_item(
        "Distance and bearing",
        "Waypoints",
        "w",
        is_available=lambda: marked_coord_x is not None,
    ),
    _function_item(
        "Toggle route beacon",
        "Waypoints",
        "w",
        ctrl=True,
        is_available=_route_is_set,
    ),
    _function_item(
        "Nearest point of interest",
        "Waypoints",
        "w",
        shift=True,
        is_available=_world_is_active,
    ),
    _function_item(
        "Redline RPM",
        "Vehicle information",
        "r",
        shift=True,
        is_available=lambda: protocol_mode == "extended",
    ),
    _function_item(
        "Maximum turbo pressure",
        "Vehicle information",
        "t",
        shift=True,
        is_available=lambda: protocol_mode == "extended"
        and bool(int(last_protocol_flags) & OG_TURBO),
    ),
    _function_item(
        "Air pressure",
        "Vehicle information",
        "p",
        is_available=lambda: protocol_mode == "extended"
        and last_air_pressure_max > 0,
    ),
    _function_item("Attitude", "Vehicle information", "a"),
    _function_item("Coordinates", "Vehicle information", "c"),
    _function_item("Damage report", "Vehicle information", "m"),
    _function_item("Toggle camera information", "Camera", "f", alt=True),
    _function_item(
        "Camera heading", "Camera", "h", alt=True, is_available=lambda: free_cam_active
    ),
    _function_item(
        "Camera altitude", "Camera", "a", alt=True, is_available=lambda: free_cam_active
    ),
    _function_item(
        "Camera pitch", "Camera", "p", alt=True, is_available=lambda: free_cam_active
    ),
    _function_item(
        "Vehicle bearing", "Camera", "v", alt=True, is_available=lambda: free_cam_active
    ),
    _function_item(
        "Vehicle distance", "Camera", "d", alt=True, is_available=lambda: free_cam_active
    ),
    _function_item("Accessible node grabber", "Interaction", "n", ctrl=True),
    # Reachable from the controller as well as from F9, which is the point: this mode exists to
    # be used by somebody holding a pad they do not yet understand. It works from in here
    # because the mod's own six accessibility actions are exempt and keep firing while the mode
    # is on -- so the menu that armed it can still disarm it.
    _function_item("Learn bindings mode", "Interaction", "b", shift=True),
    _function_item(
        "Clickspot detection", "Interaction", "c", ctrl=True, shift=True
    ),
    # Available only while the UI runtime has a readable screen latched, so the entry is
    # absent everywhere else rather than present and answering "nothing to read". The
    # handler is a lambda because open_page_text_browser is defined further down the file
    # than this catalog.
    _function_item(
        "Read page text",
        "Interaction",
        handler=lambda: open_page_text_browser(),
        is_available=lambda: _ui_screen_context == "mod_details",
    ),
    _function_item(
        "Hill-climb reports",
        "Challenge reports",
        handler=lambda: open_hill_climb_history(),
        is_available=lambda: bool(HILL_CLIMB_CHALLENGE.list_reports(limit=1)),
    ),
    _function_item("Switch units", "Interaction", "u"),
    _function_item(
        "Terrain scan",
        "Environment",
        is_available=_world_is_active,
        handler=trigger_terrain_scan_driving_only,
    ),
]

ACCESSIBILITY_SCREENS = ("Status", "Functions", "Click spots")
ACCESSIBILITY_ACTIONS = frozenset(
    {
        "status_up",
        "status_down",
        "status_repeat",
        "next_menu",
        "previous_menu",
        "activate",
    }
)



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
    if status_keyboard_mode_active:
        toggle_status_mode()
    elif status_arrow_hooks:
        # Hooks outliving the mode flag: release them anyway rather than leaving the arrows
        # eaten with no mode to toggle off.
        _unhook_suppressed(status_arrow_hooks)
        if reason:
            logger.warning(f"Released orphaned status-mode arrow hooks ({reason})")


def _execute_function_item(item):
    handler = item.get("handler")
    if handler is not None:
        handler()
        return
    name, ctrl, shift, alt = item["command"]
    _invoke_f9_command(name, ctrl=ctrl, shift=shift, alt=alt)


def _drive_virtual_browser(action):
    """Move the open virtual browser from a controller's accessibility actions."""
    if action in ("next_menu", "previous_menu"):
        _on_vbrowser_escape(None)
        return
    if action == "status_up":
        _on_vbrowser_nav(SimpleNamespace(name="up"))
        return
    if action == "status_down":
        _on_vbrowser_nav(SimpleNamespace(name="down"))
        return
    if action == "activate" and _vbrowser_on_enter is not None:
        _on_vbrowser_enter(None)
        return
    # status_repeat, and activate on a read-only list: re-read where we are. A browser
    # with no enter action has nothing to activate, and silence there would read as the
    # pad having stopped working.
    if _vbrowser_lines:
        say(_vbrowser_lines[_vbrowser_index], exclude_from_buffer=True)


def navigate_accessibility_menu(action):
    """Navigate or activate the controller-facing accessibility screens."""
    global current_accessibility_screen_index
    global current_status_metric_index, current_functions_item_index
    global current_clickspot_index

    if action not in ACCESSIBILITY_ACTIONS:
        logger.warning("Ignoring unknown accessibility action: %r", action)
        return

    # A virtual browser wins over the three accessibility screens, which is the rule
    # on_status_arrow_press already applies on the keyboard -- it returns early while a
    # browser is up. Without the same rule here the pad could OPEN a browser from the
    # Functions menu (the mod page readout does exactly that) and then have no way to move
    # through it, because open_virtual_browser hooks the keyboard arrows and nothing else.
    # next_menu/previous_menu is the way out: it is what a listener reaches for to leave,
    # and it costs no seventh action to bind.
    if _vbrowser_active:
        _drive_virtual_browser(action)
        return

    execute_item = None
    clickspot_action = None
    speech = None
    with state_lock:
        if action == "next_menu":
            current_accessibility_screen_index = (
                current_accessibility_screen_index + 1
            ) % len(ACCESSIBILITY_SCREENS)
        elif action == "previous_menu":
            current_accessibility_screen_index = (
                current_accessibility_screen_index - 1
            ) % len(ACCESSIBILITY_SCREENS)

        switched_screen = action in {"next_menu", "previous_menu"}
        if current_accessibility_screen_index == 0:
            available_metrics = [m for m in STATUS_METRICS if m["isAvailable"]()]
            if not available_metrics:
                speech = "No status metrics available"
            else:
                current_status_metric_index %= len(available_metrics)
                previous_category = available_metrics[current_status_metric_index].get(
                    "category"
                )
                if not switched_screen and action == "status_down":
                    current_status_metric_index = (
                        current_status_metric_index + 1
                    ) % len(available_metrics)
                elif not switched_screen and action == "status_up":
                    current_status_metric_index = (
                        current_status_metric_index - 1
                    ) % len(available_metrics)

                metric = available_metrics[current_status_metric_index]
                value, unit = metric["getValue"]()
                value_text = f"{value} {unit}".strip()
                category = metric.get("category")
                label = metric["label"]
                if switched_screen:
                    if category:
                        label = f"{category}: {label}"
                    speech = f"Status: {label}, {value_text}"
                elif action in {"status_repeat", "activate"}:
                    speech = value_text
                else:
                    if category and category != previous_category:
                        label = f"{category}: {label}"
                    speech = f"{label}, {value_text}"
        elif current_accessibility_screen_index == 1:
            available_items = [i for i in FUNCTION_ITEMS if i["isAvailable"]()]
            if not available_items:
                speech = "No functions available"
            else:
                current_functions_item_index %= len(available_items)
                previous_category = available_items[current_functions_item_index][
                    "category"
                ]
                if not switched_screen and action == "status_down":
                    current_functions_item_index = (
                        current_functions_item_index + 1
                    ) % len(available_items)
                elif not switched_screen and action == "status_up":
                    current_functions_item_index = (
                        current_functions_item_index - 1
                    ) % len(available_items)

                item = available_items[current_functions_item_index]
                label = item["label"]
                category = item["category"]
                if switched_screen:
                    speech = f"Functions: {category}: {label}"
                elif action == "activate":
                    execute_item = item
                elif action == "status_repeat":
                    speech = label
                else:
                    if category != previous_category:
                        label = f"{category}: {label}"
                    speech = label
        else:
            if not clickspot_mode_active:
                clickspot_items = [("enable", None, "Turn on clickspot detection")]
            elif clickspot_list_loading:
                clickspot_items = [("status", None, "Detecting click spots")]
            elif not clickspot_trigger_list:
                clickspot_items = [("status", None, "No click spots found")]
            else:
                clickspot_items = [
                    ("clickspot", cache_idx, display_name)
                    for cache_idx, _trigger_id, display_name in clickspot_trigger_list
                ]

            current_clickspot_index %= len(clickspot_items)
            if not switched_screen and action == "status_down":
                current_clickspot_index = (current_clickspot_index + 1) % len(
                    clickspot_items
                )
            elif not switched_screen and action == "status_up":
                current_clickspot_index = (current_clickspot_index - 1) % len(
                    clickspot_items
                )

            kind, cache_idx, label = clickspot_items[current_clickspot_index]
            if switched_screen:
                speech = f"Click spots: {label}"
            elif action == "activate":
                if kind == "enable":
                    clickspot_action = ("enable", None)
                elif kind == "clickspot":
                    clickspot_action = ("activate", cache_idx)
                else:
                    speech = label
            else:
                speech = label

    if speech is not None:
        say(speech, exclude_from_buffer=True)
    if execute_item is not None:
        _execute_function_item(execute_item)
    if clickspot_action is not None:
        kind, cache_idx = clickspot_action
        if kind == "enable":
            _invoke_f9_command("c", ctrl=True, shift=True)
        else:
            _activate_clickspot(cache_idx)


def navigate_status(action):
    """Compatibility entry point for Ctrl+S arrow navigation."""
    navigate_accessibility_menu(action)


def _on_accessibility_action(action):
    # The TCP callback runs on aiohttp's event-loop thread. Menu reads and
    # speech stay serialized with keyboard commands on the command worker.
    _command_dispatch(navigate_accessibility_menu, action)


def on_status_arrow_press(event):
    # Don't steal the arrows while a virtual browser (vehicle selector, clickspot list,
    # bindings list) owns them. This used to read a `_vehicle_selector_open` name that was
    # never defined anywhere, so every arrow press in status mode raised NameError before
    # reaching a single metric — status mode looked dead.
    if _vbrowser_active:
        return
    if not status_keyboard_mode_active:
        return
    action = {
        "up": "status_up",
        "down": "status_down",
        "left": "status_repeat",
        "right": "status_repeat",
    }.get(event.name)
    if action:
        navigate_status(action)


def toggle_status_mode():
    global status_keyboard_mode_active, current_status_metric_index, status_arrow_hooks
    status_keyboard_mode_active = not status_keyboard_mode_active
    if status_keyboard_mode_active:
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


def _on_vbrowser_adjust(event):
    """Left/right on a browser row that supports editing.

    Only hooked when a browser passes on_adjust, so every other browser keeps
    left/right for the game. Shift is the coarse step: a value with a range of a
    hundred is otherwise a hundred presses away from its far end.
    """
    if not _vbrowser_lines or _vbrowser_on_adjust is None:
        return
    delta = 1 if event.name == "right" else -1
    try:
        if keyboard.is_pressed("shift"):
            delta *= 10
    except Exception:
        pass
    idx = _vbrowser_index
    data = _vbrowser_entry_data[idx] if idx < len(_vbrowser_entry_data) else None
    try:
        _vbrowser_on_adjust(idx, _vbrowser_lines[idx], data, delta)
    except Exception as e:
        logger.error(f"Virtual browser adjust callback error: {e}")


def _on_vbrowser_escape(event):
    if _vbrowser_on_escape is not None:
        try:
            _vbrowser_on_escape()
        except Exception as e:
            logger.error(f"Virtual browser escape callback error: {e}")
        return
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
    lines,
    title=None,
    on_enter=None,
    entry_data=None,
    start_index=0,
    on_adjust=None,
    on_escape=None,
    announce_interrupt=True,
):
    global _vbrowser_lines, _vbrowser_index, _vbrowser_active
    global _vbrowser_on_enter, _vbrowser_entry_data, _vbrowser_on_adjust
    global _vbrowser_on_escape
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
    _vbrowser_on_adjust = on_adjust
    _vbrowser_on_escape = on_escape
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
            # Left/right are only taken when a browser actually edits something.
            # Suppressing them unconditionally would swallow steering for every
            # read-only list in the mod.
            if on_adjust is not None:
                for key in ["left", "right"]:
                    _vbrowser_hooks.append(
                        _hook_suppressed(key, _kb_enqueue(_on_vbrowser_adjust))
                    )
        except Exception as e:
            logger.error(f"Failed to hook virtual browser keys: {e}")
    if title:
        say(
            f"{title}. {_vbrowser_lines[_vbrowser_index]}",
            interrupt=announce_interrupt,
            exclude_from_buffer=True,
        )
    else:
        say(
            _vbrowser_lines[_vbrowser_index],
            interrupt=announce_interrupt,
            exclude_from_buffer=True,
        )


def close_virtual_browser(speak_exit=True):
    global _vbrowser_lines, _vbrowser_index, _vbrowser_active
    global _vbrowser_on_enter, _vbrowser_entry_data, _vbrowser_on_adjust
    global _vbrowser_on_escape
    global _env_browser_open
    _env_browser_open = False
    _vbrowser_active = False
    _vbrowser_on_enter = None
    _vbrowser_on_adjust = None
    _vbrowser_on_escape = None
    _vbrowser_entry_data = []
    _unhook_suppressed(_vbrowser_hooks)
    _vbrowser_lines = []
    _vbrowser_index = 0
    if speak_exit:
        say("Exit", exclude_from_buffer=True)


def open_hill_climb_report(summary, return_to_history=None, announce_interrupt=True):
    """Open one persisted challenge summary in the existing virtual browser."""
    lines = HILL_CLIMB_CHALLENGE.report_lines(summary, UNITS_MODE)
    on_escape = None
    if return_to_history is not None:
        on_escape = lambda: open_hill_climb_history(start_index=return_to_history)
    open_virtual_browser(
        lines,
        title="Hill-climb report",
        on_escape=on_escape,
        announce_interrupt=announce_interrupt,
    )


def open_hill_climb_history(start_index=0):
    """Browse retained attempts newest-first and activate one for its full report."""
    reports = HILL_CLIMB_CHALLENGE.list_reports()
    if not reports:
        say("No hill-climb reports available", exclude_from_buffer=True)
        return

    lines = [HILL_CLIMB_CHALLENGE.history_line(item, UNITS_MODE) for item in reports]

    def open_selected(index, _line, summary):
        open_hill_climb_report(summary, return_to_history=index)

    open_virtual_browser(
        lines,
        title=f"Hill-climb report history, {len(lines)} attempts. Press Enter for details",
        on_enter=open_selected,
        entry_data=reports,
        start_index=start_index,
    )


def _present_hill_climb_result(summary, auto_open):
    if not auto_open or summary.get("status") != "completed":
        return
    say(
        hill_climb_completion_speech(summary),
        exclude_from_buffer=True,
        source="hill_climb",
    )
    open_hill_climb_report(summary, announce_interrupt=False)


def _hill_climb_finalized(summary, auto_open):
    stats = summary.get("statistics") or {}
    logger.info(
        "Hill-climb data quality: attempt=%s, status=%s, samples=%s, "
        "sample_rate_hz=%s, packet_gaps=%s",
        summary.get("attempt_id", "unknown"),
        summary.get("status", "unknown"),
        stats.get("samples", 0),
        stats.get("sample_rate_hz", 0),
        (stats.get("packet_gaps") or {}).get("count", 0),
    )
    _command_dispatch(_present_hill_climb_result, summary, auto_open)


def _on_hill_climb_event(data):
    """Handle native mission events arriving on the Lua TCP relay thread."""
    action = HILL_CLIMB_CHALLENGE.handle_event(
        data, telemetry=_road_diagnostic_telemetry_snapshot()
    )
    if not action or action.get("capture") is None:
        return
    _send_road_command("CAPTURE_ON" if action["capture"] else "CAPTURE_OFF")


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
BINDING_LEARN_LISTEN_PORT = (
    4479  # UDP port to receive learn-mode events from bindingLearn.lua
)
BINDING_LEARN_CMD_PORT = 4480  # UDP port to send LEARN_ON/LEARN_OFF/KEEPALIVE/DIAG
# The keepalive interval, against bindingLearn.lua's HEARTBEAT_TIMEOUT_S of 6.0. Well under
# half of it on purpose: with the mode on, every binding in the game runs through the mod, so
# a couple of dropped datagrams must not be able to end it -- and beamtel dying must end it
# within seconds. Same reasoning trailerAngle.lua's heartbeat rests on.
BINDING_LEARN_KEEPALIVE_S = 1.0
ENV_LISTEN_PORT = 4474  # UDP port to receive environment rows from environmentAccessible.lua
ENV_CMD_PORT = 4475  # UDP port to send commands to environmentAccessible.lua
VEHICLE_INFO_LISTEN_PORT = 4477  # UDP port to receive vehicle info rows from vehicleInfo.lua
VEHICLE_INFO_CMD_PORT = 4478  # UDP port to send INFO_SELECTOR/INFO: to vehicleInfo.lua
IMPLEMENT_LISTEN_PORT = (
    4469  # UDP port to receive implement proximity events from implementProximity.lua
)
IMPLEMENT_CMD_PORT = 4470  # UDP port to send ON/OFF/REBUILD to implementProximity.lua
TERRAIN_SCAN_LISTEN_PORT = (
    4471  # UDP port to receive terrain scan snapshots from terrainScanner.lua
)
TERRAIN_SCAN_CMD_PORT = 4472  # UDP port to send SCAN to terrainScanner.lua
CANNON_SHOT_LISTEN_PORT = (
    4473  # UDP port to receive cannon shot outcomes from cannonShot.lua
)
# cannonShot has no command port at all — the only user-facing setting is whether the outcome
# is spoken, which is enforced here. (4474/4475 went to environmentAccessible above.)
TRAILER_ANGLE_LISTEN_PORT = (
    4476  # UDP port to receive trailer articulation angles from trailerAngle.lua
)
ROUTE_BEACON_LISTEN_PORT = (
    4482  # UDP port to receive the map route's destination from routeBeacon.lua
)
# No command port for routeBeacon either, and for the same reason: whether a route is
# set is a fact the game maintains, so the mod has nothing to be told. Whether the
# beacon SOUNDS is this side's toggle and never leaves the process.
# No command port for the same reason: the mod reads "is a trailer attached" out of the game's
# own registry, so there is nothing to tell it and nothing for the driver to switch on.
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


# Set by mcp_server when it starts; None otherwise, so the tap costs one identity
# comparison per datagram when the MCP server is off.
_mcp_console_tap = None


def register_console_tap(fn):
    """Install a callable that sees every console record and may consume it."""
    global _mcp_console_tap
    _mcp_console_tap = fn


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
            # The MCP server correlates its EXECs by ordering plus the EXECEND
            # sentinel, so it must also SWALLOW the records it owns: on_console_message
            # speaks any single-line result aloud, and an agent exec talking over the
            # user mid-test is exactly what this prevents. LOG records are copied there
            # but never consumed -- they belong to the GUI pane.
            tap = _mcp_console_tap
            if tap is not None:
                try:
                    if tap(text):
                        continue
                except Exception:
                    pass  # a tap fault must never kill this listener
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
    if not scan_mode_active and not coupler_run_active:
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


def _send_obstacle_command(command):
    try:
        cmd_sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        cmd_sock.sendto(str(command).encode("utf-8"), ("127.0.0.1", OBSTACLE_CMD_PORT))
        cmd_sock.close()
        return True
    except Exception as exc:
        logger.error(f"Failed to send obstacle detector command via UDP: {exc}")
        return False


def _push_obstacle_state(force=False, gear=None):
    """Push driver intent on material changes and as a half-second heartbeat."""
    global _obstacle_state_last, _obstacle_state_last_sent
    if not obstacle_mode_active:
        return False
    now = time.monotonic()
    gear_value = gear
    if gear_value is None:
        gear_value = last_gear_str if protocol_mode == "extended" else (
            "R" if last_gear_byte == REVERSE else "F"
        )
    direction = "R" if str(gear_value or "").strip().upper().startswith("R") else "F"
    state = (
        direction,
        max(-1.0, min(1.0, float(last_steering_input))),
        max(0.0, min(1.0, float(last_throttle))),
        max(0.0, min(1.0, float(last_brake))),
    )
    changed = _obstacle_state_last is None or state[0] != _obstacle_state_last[0]
    if _obstacle_state_last is not None:
        changed = changed or abs(state[1] - _obstacle_state_last[1]) >= OBSTACLE_STEERING_DELTA
        changed = changed or abs(state[2] - _obstacle_state_last[2]) >= OBSTACLE_PEDAL_DELTA
        changed = changed or abs(state[3] - _obstacle_state_last[3]) >= OBSTACLE_PEDAL_DELTA
    if not (force or changed or now - _obstacle_state_last_sent >= OBSTACLE_STATE_HEARTBEAT_S):
        return False
    command = "STATE,{},{:.3f},{:.3f},{:.3f}".format(*state)
    if _send_obstacle_command(command):
        _obstacle_state_last = state
        _obstacle_state_last_sent = now
        return True
    return False


def _send_obstacle_configuration():
    sensitivity = str(obstacle_warning_sensitivity or "normal").lower()
    if sensitivity not in ("early", "normal", "late"):
        sensitivity = "normal"
    return _send_obstacle_command(f"SENSITIVITY,{sensitivity}")


def toggle_obstacle_mode(audio_controller):
    global obstacle_mode_active, _obstacle_state_last, _obstacle_state_last_sent
    obstacle_mode_active = not obstacle_mode_active

    command = "ON" if obstacle_mode_active else "OFF"
    _send_obstacle_command(command)
    if obstacle_mode_active:
        _send_obstacle_configuration()
        _push_obstacle_state(force=True)
    else:
        _obstacle_state_last = None
        _obstacle_state_last_sent = 0.0

    audio_controller.set_obstacle_mode(obstacle_mode_active)

    say(
        f"Obstacle detection {'on' if obstacle_mode_active else 'off'}",
        exclude_from_buffer=True,
    )


def _send_road_command(command):
    try:
        cmd_sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        cmd_sock.sendto(command.encode("utf-8"), ("127.0.0.1", ROAD_CMD_PORT))
        cmd_sock.close()
        return True
    except Exception as exc:
        logger.error(f"Failed to send road detector command via UDP: {exc}")
        return False


def _send_road_configuration():
    _send_road_command(f"PRIVATE,{1 if road_include_private else 0}")


def road_diagnostic_control(action="status", label=None, session=None, note=None, limit=20):
    """MCP-facing lifecycle for a loss-resistant road-guidance recording."""
    action = str(action or "status").strip().lower()
    if action == "start":
        if not _world_is_active():
            raise RuntimeError("no live world; start the recording once driving telemetry is active")
        if not road_mode_active:
            raise RuntimeError("road detection is off; enable it before starting the recording")
        if not road_follow_guidance_enabled:
            raise RuntimeError("road-follow guidance is disabled in configuration")
        result = ROAD_DIAGNOSTICS.start(label or "hill-climb")
        if not _send_road_command("DIAG_ON"):
            result["warning"] = "recording started, but Lua diagnostic mode could not be requested"
        result["road_detection_active"] = road_mode_active
        result["road_guidance_enabled"] = road_follow_guidance_enabled
        return result
    if action == "stop":
        _send_road_command("DIAG_OFF")
        return ROAD_DIAGNOSTICS.stop()
    if action == "mark":
        return ROAD_DIAGNOSTICS.mark(note)
    if action == "review":
        return ROAD_DIAGNOSTICS.review(session=session)
    if action == "list":
        return {"sessions": ROAD_DIAGNOSTICS.list_sessions(limit=limit)}
    if action == "status":
        result = ROAD_DIAGNOSTICS.status()
        result["road_detection_active"] = road_mode_active
        result["road_guidance_enabled"] = road_follow_guidance_enabled
        return result
    raise ValueError("action must be start, stop, status, mark, review, or list")


def speak_road_status():
    say(
        ROAD_GUIDANCE_FEED.status_phrase(road_mode_active, UNITS_MODE),
        exclude_from_buffer=True,
        source="road_guidance",
    )


def toggle_road_mode(audio_controller):
    global road_mode_active
    road_mode_active = not road_mode_active

    command = "ON" if road_mode_active else "OFF"
    _send_road_command(command)
    _send_road_configuration()

    ROAD_GUIDANCE_FEED.reset()
    audio_controller.set_road_mode(road_mode_active)

    say(
        f"Road detection {'on' if road_mode_active else 'off'}",
        exclude_from_buffer=True,
    )


def toggle_route_beacon(audio_controller):
    """Toggle the crow-flies beacon on the map route's destination.

    Refuses to switch on with no route, the way the coordinate-guidance toggle refuses
    with no waypoint marked: a mode that reports itself on and then makes no sound is
    indistinguishable from a broken one, and this instrument's whole promise is that it
    is audible whenever it is armed.
    """
    global route_beacon_active

    if not route_beacon_active:
        if not _route_is_set():
            audio_controller.set_route_beacon_mode(False)
            say(
                "No route set. Choose a destination on the map first.",
                exclude_from_buffer=True,
            )
            return
        route_beacon_active = True
        audio_controller.set_route_beacon_mode(True)
        dist, brg = relative_bearing(
            last_pos_x, last_pos_y, last_heading, route_dest_x, route_dest_y
        )
        say(
            route_beacon_phrase(dist, brg, route_remaining_m, UNITS_MODE),
            exclude_from_buffer=True,
        )
        return

    route_beacon_active = False
    audio_controller.set_route_beacon_mode(False)
    say("Route beacon off", exclude_from_buffer=True)


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


def _activate_clickspot(cache_idx):
    """Snap to and activate one cached clickspot through Lua's command path."""
    _send_clickspot_cmd(f"SNAP:{cache_idx}")

    def _do_exec():
        time.sleep(0.1)  # small delay to let SNAP/cursor warp complete
        _send_clickspot_cmd(f"EXEC:{cache_idx},1")
        time.sleep(0.15)
        _send_clickspot_cmd(f"EXEC:{cache_idx},0")

    threading.Thread(target=_do_exec, daemon=True).start()


def _clickspot_browser_on_enter(idx, line, data):
    """Virtual browser enter callback — snap cursor to trigger and execute press+release."""
    if data is not None:
        _activate_clickspot(data)


def toggle_clickspot_mode(audio_controller):
    with state_lock:
        turning_on = not clickspot_mode_active
    if turning_on:
        _clickspot_set_pending()
    else:
        _clickspot_set_off()

    command = "ON" if turning_on else "OFF"
    _send_clickspot_cmd(command)

    say(
        f"Clickspot detection {'on' if turning_on else 'off'}",
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


def _send_binding_learn_cmd(command):
    try:
        cmd_sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        cmd_sock.sendto(command.encode("utf-8"), ("127.0.0.1", BINDING_LEARN_CMD_PORT))
        cmd_sock.close()
    except Exception as e:
        logger.error(f"Failed to send binding learn command via UDP: {e}")


def toggle_binding_learn_mode():
    """Ask the mod to enter or leave Learn Bindings Mode.

    Lua owns the authoritative state, and the confirmation is spoken from the LEARNMODE: reply
    rather than from this request. That is not fussiness: entering the mode can fail on the Lua
    side (the wrapper or the binding re-push), and announcing "learn bindings on" here would
    claim a mode that is not running -- in a feature whose whole promise is that your buttons
    have stopped doing things. If the mod never answers, nothing is said and the log carries it.
    """
    global _binding_learn_active
    _binding_learn_active = not _binding_learn_active
    _send_binding_learn_cmd("LEARN_ON" if _binding_learn_active else "LEARN_OFF")


def binding_learn_keepalive_thread_fn(stop_event):
    """Hold the mod's watchdog open while learn mode is on.

    This thread IS the safety property. With the mode on, every binding in the game points at
    the mod; if beamtel dies the game is unplayable until it is restarted. The mod tears the
    mode down after HEARTBEAT_TIMEOUT_S of silence, so simply not sending is the recovery.
    """
    while not stop_event.is_set():
        stop_event.wait(BINDING_LEARN_KEEPALIVE_S)
        if stop_event.is_set():
            break
        if _binding_learn_active:
            _send_binding_learn_cmd("KEEPALIVE")


def _binding_learn_phrase(row):
    """Render one learn-mode press into a spoken line.

    Control first, then what it does: the control is what the listener just touched and is the
    thing they are trying to attach a meaning to, so leading with it means the answer arrives
    in the order the question was asked. The description is long, so it is off by default.

    A press is reported as a GROUP, because one control routinely carries several bindings at
    once -- a stock pad has btn_a on accept, menu_item_select, shiftUp, triggerAction0 and
    bigMapControllerSelect. Sent one at a time each announcement interrupted the last and only
    the final one was ever heard, which reads as the rest being ignored rather than as a mod
    that only says one thing. The count is spoken because "this button does three things" is
    itself the answer to the question being asked. `items` is optional on the wire and a packet
    without it is read as a single unnamed-count action, for the reason every other tail in this
    project carries: bng_mod/ is a live junction and the two halves genuinely go out of step.
    """
    control = (row.get("control") or "").strip()

    # Holding a modifier over a control that carries no binding for it is not an unbound control
    # as far as the engine is concerned -- it falls through and fires the bare binding. The mod
    # drops that fall-through and marks the report, because "modifier 1 plus d-pad right" is a
    # different button from "d-pad right" and naming the right indicator for it is a confident
    # wrong answer about the combination actually pressed. Reported rather than dropped: silence
    # is indistinguishable from the mode being broken.
    if row.get("unbound"):
        return f"{control}. Nothing bound" if control else "Nothing bound"

    items = row.get("items")
    if not isinstance(items, list) or not items:
        items = [
            {
                "title": row.get("title") or row.get("action") or "",
                "desc": row.get("desc") or "",
                "suppressed": row.get("suppressed", 1),
            }
        ]

    parts = []
    if control:
        parts.append(control + (", axis" if row.get("kind") == "axis" else ""))
    elif row.get("kind") == "axis":
        parts.append("axis")
    if len(items) > 1:
        parts.append(f"{len(items)} actions")

    for item in items:
        if not isinstance(item, dict):
            continue
        title = (item.get("title") or "").strip() or "unnamed action"
        # An exempt binding fired as well as being named -- menu navigation, the pad's modifier
        # buttons, and this mod's own controller keys. Saying so is what stops "it did something
        # anyway" reading as a bug.
        if not item.get("suppressed"):
            title += ", still active"
        if announce_binding_learn_description:
            desc = (item.get("desc") or "").strip()
            if desc and desc != (item.get("title") or "").strip():
                title += ". " + desc
        parts.append(title)

    return ". ".join(p for p in parts if p)


def binding_learn_listener(stop_event):
    """Listens for Learn Bindings Mode events from bindingLearn.lua."""
    global _binding_learn_active

    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        sock.bind(("127.0.0.1", BINDING_LEARN_LISTEN_PORT))
        sock.settimeout(0.2)
        logger.info(
            f"Binding learn listener started on port {BINDING_LEARN_LISTEN_PORT}"
        )

        while not stop_event.is_set():
            try:
                data, _addr = sock.recvfrom(4096)
                text = data.decode("utf-8", errors="ignore").strip()
                if not text:
                    continue

                if text.startswith("LEARN:"):
                    # JSON rather than a positional or k=v tail: the action description is free
                    # text with commas and quotes in it. Same call vehicleInfo.lua's INFO_ROW
                    # already makes.
                    try:
                        row = json.loads(text[6:])
                    except (ValueError, TypeError) as e:
                        logger.warning(f"Malformed learn packet: {e}")
                        continue
                    if not isinstance(row, dict):
                        continue
                    # interrupt=True: presses come as fast as the user can make them, and the
                    # answer they want is about the button under their thumb now.
                    say(_binding_learn_phrase(row))
                    continue

                if text.startswith("LEARNMODE:"):
                    body = text[10:]
                    state, _, reason = body.partition(";")
                    on = state.strip().upper() == "ON"
                    _binding_learn_active = on
                    logger.info(f"Learn bindings mode {'on' if on else 'off'}: {reason}")
                    say(
                        f"Learn bindings {'on' if on else 'off'}",
                        exclude_from_buffer=True,
                    )
                    continue

                if text.startswith("LEARNFAIL:"):
                    code, _, sentence = text[10:].partition(";")
                    logger.error(f"Learn bindings failed ({code}): {sentence}")
                    # Every current code means the mode is not running -- it would not start
                    # (nowrap, norefresh), or it has ended and is still repairing (norestore).
                    # Leaving the flag set would spend the next Shift+B turning off a mode that
                    # was never on, so the user would have to press twice to get in.
                    _binding_learn_active = False
                    # A failure here is never silent. Every code names a state in which the
                    # user's controls are not behaving the way the mode just promised.
                    say(sentence or "Learn bindings failed", exclude_from_buffer=True)
                    continue

            except socket.timeout:
                continue
            except OSError:
                if stop_event.is_set():
                    break
                logger.error("Binding learn listener socket error.")
    finally:
        try:
            sock.close()
        except Exception:
            pass
        logger.info("Binding learn listener stopped.")


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


def _parse_env_fields(payload):
    """Parse the mod's ``k=v;k=v`` row body into a dict.

    Split on the FIRST '=' per field, and keep unknown keys: the Lua side
    sanitizes ';' and '=' out of every value precisely so this stays a two-line
    parser, and a field this build does not know about must be ignored rather
    than shifting anything after it.
    """
    out = {}
    for field in payload.split(";"):
        if not field:
            continue
        key, sep, value = field.partition("=")
        if sep:
            out[key.strip()] = value.strip()
        elif "count" not in out:
            # The bare leading number on ENV_BEGIN.
            out["count"] = field.strip()
    return out


def _send_vehicle_info_cmd(command):
    try:
        cmd_sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        cmd_sock.sendto(command.encode("utf-8"), ("127.0.0.1", VEHICLE_INFO_CMD_PORT))
        cmd_sock.close()
    except Exception as e:
        logger.error(f"Failed to send vehicle info command via UDP: {e}")


def _vinfo_render(rows):
    """Turn the mod's {kind,label,value} rows into lines for the virtual browser.

    A group row is a heading and is spoken as a bare name with no value, so a
    listener arrowing down hears the structure of the page rather than a flat run
    of thirty numbers. A spec with no label (the icon strip, the source tag)
    carries its whole sentence in the value already, so prefixing an empty label
    would leave a stray comma.
    """
    lines = []
    for r in rows:
        kind = r.get("kind", "spec")
        label = (r.get("label") or "").strip()
        value = (r.get("value") or "").strip()
        if kind == "group":
            lines.append(label)
        elif label and value:
            lines.append(f"{label}: {value}")
        elif value:
            lines.append(value)
    return lines


def request_vehicle_info(model=None, config=None, timeout=0.6):
    """Ask vehicleInfo.lua for a spec sheet and wait briefly for the reply.

    Returns (lines, failure), where failure is None on success and otherwise a
    (code, sentence) pair. `model` None asks about whatever the stock selector's
    details page is showing; otherwise it asks for that explicit pair, with an
    empty config meaning the model's default.

    The bounded wait is safe wherever this is called from today -- the F9
    dispatcher runs on the `command-dispatch` worker thread and the spawner on its own
    worker, never on a keyboard hook, so it cannot trip the ~300 ms Windows
    low-level-hook timeout that would leak suppressed keys to the game.
    """
    global _vinfo_rows, _vinfo_error, _vinfo_absent
    _vinfo_event.clear()
    _vinfo_rows = []
    _vinfo_error = None
    if model:
        _send_vehicle_info_cmd(f"INFO:{model},{config or ''}")
    else:
        _send_vehicle_info_cmd("INFO_SELECTOR")

    # A mod half that has already failed to answer is asked again -- a reply is how the latch
    # clears -- but not WAITED for. Blocking is what would be felt: this runs inline on the
    # F9 SPACE path, where the other two answers are a terrain scan and a menu press, so a
    # user whose bng_mod predates this feature would otherwise pay the full timeout in dead
    # air before every single scan.
    if not _vinfo_event.wait(0.0 if _vinfo_absent else timeout):
        # Silence is its own diagnosis and must not be reported as "no vehicle":
        # the mod half is missing, out of date, or its command socket never bound.
        _vinfo_absent = True
        return [], ("timeout", "")

    if _vinfo_error:
        return [], _vinfo_error
    return _vinfo_render(_vinfo_rows), None


def _on_screen_context(context, title):
    """The UI runtime says which readable screen is up (empty when none)."""
    global _ui_screen_context, _ui_screen_title
    _ui_screen_context = context
    _ui_screen_title = title


def _on_page_text(lines, title, code, sentence):
    global _page_text_lines, _page_text_title, _page_text_error, _page_text_absent
    _page_text_absent = False
    if code:
        _page_text_lines = []
        _page_text_title = ""
        _page_text_error = (code, sentence)
    else:
        _page_text_lines = lines
        _page_text_title = title
        _page_text_error = None
    _page_text_event.set()


def request_page_text(timeout=0.6):
    """Ask the UI runtime for the current readable screen and wait briefly.

    Returns (lines, failure) with the same contract request_vehicle_info uses: failure is
    None on success and otherwise a (code, sentence) pair. `notdetails` is the answer on
    every screen in the game that is not a mod details page, so the caller must fall
    through silently on it -- speaking it would replace the terrain scan with a complaint
    on a key that has always scanned.
    """
    global _page_text_lines, _page_text_title, _page_text_error, _page_text_absent
    _page_text_event.clear()
    _page_text_lines = []
    _page_text_title = ""
    _page_text_error = None
    broadcast({"type": "page_text"})

    # Asked again -- a reply is how the latch clears -- but not WAITED for.
    if not _page_text_event.wait(0.0 if _page_text_absent else timeout):
        _page_text_absent = True
        return [], ("timeout", "")

    if _page_text_error:
        return [], _page_text_error
    return list(_page_text_lines), None


def _read_mod_details_page():
    """F9 SPACE on a mod repository details page. True when it handled the press.

    Gated on the latch BEFORE the round trip: this key is the terrain scan and is pressed
    while driving, so the ordinary case has to cost nothing at all. The latch is pushed on
    entering and leaving the route and re-pushed on every transport pong, so a UI reload or
    a beamtel restart cannot leave it stuck on.

    A failure is spoken only when the screen really was up and still could not be read --
    the user asked a question about a mod and deserves the reason. A timeout says nothing
    about the mod, only that the UI half is missing or deaf, so it stays silent and the
    chain falls through to the scan.
    """
    if _ui_screen_context != "mod_details":
        return False
    lines, failure = request_page_text()
    if lines:
        open_virtual_browser(
            lines,
            title=f"{_page_text_title or 'Mod details'}, {len(lines)} items",
        )
        return True
    if failure and failure[0] not in ("notdetails", "timeout"):
        say(failure[1] or "No mod information", exclude_from_buffer=True)
        return True
    return False


def open_page_text_browser():
    """Read the current readable screen, or say why it cannot be read.

    Used by the controller Functions menu, where the entry only appears while a readable
    screen is latched -- so unlike the F9 SPACE path, every failure here IS news and is
    spoken, including the one that means the screen went away underneath the menu.
    """
    lines, failure = request_page_text()
    if lines:
        open_virtual_browser(
            lines,
            title=f"{_page_text_title or 'Page text'}, {len(lines)} items",
        )
        return True
    if failure and failure[0] == "timeout":
        say("The mod interface did not answer", exclude_from_buffer=True)
    else:
        say(
            (failure[1] if failure else "") or "Nothing to read here",
            exclude_from_buffer=True,
        )
    return False


def _send_env_cmd(command):
    try:
        cmd_sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        cmd_sock.sendto(command.encode("utf-8"), ("127.0.0.1", ENV_CMD_PORT))
        cmd_sock.close()
    except Exception as e:
        logger.error(f"Failed to send environment command via UDP: {e}")


def _env_float(row, key):
    try:
        return float(row.get(key, ""))
    except (TypeError, ValueError):
        return None


def _env_temp_from_display(display_value):
    """Inverse of fmt_temp_c_or_f — the wire is always Celsius."""
    if UNITS_MODE == "metric":
        return float(display_value)
    return (float(display_value) - 32.0) * 5.0 / 9.0


def _env_row_line(row):
    """One spoken line for an environment row."""
    label = row.get("label", row.get("key", "Setting"))
    kind = row.get("kind", "")
    lo = _env_float(row, "curveLo")
    hi = _env_float(row, "curveHi")

    if kind == "numberC":
        value = _env_float(row, "value")
        if value is None:
            return f"{label}, not available"
        shown, unit = fmt_temp_c_or_f(value)
        line = f"{label}, {shown} {unit}"
        # A level whose curve is not flat has no single temperature: the figure
        # above is only true for the current time of day, and editing it
        # replaces the whole cycle with one number. Saying so is the difference
        # between an edit and a silent loss of the level's own weather.
        if lo is not None and hi is not None and round(hi - lo) >= 1:
            lo_shown, _ = fmt_temp_c_or_f(lo)
            hi_shown, _ = fmt_temp_c_or_f(hi)
            line += f", varies {lo_shown} to {hi_shown} through the day"
        if row.get("editable", "1") != "1":
            line += ", locked"
        return line

    if kind == "action":
        if lo is None:
            return f"{label}, not available"
        lo_shown, unit = fmt_temp_c_or_f(lo)
        if hi is not None and round(hi - lo) >= 1:
            hi_shown, _ = fmt_temp_c_or_f(hi)
            return f"{label}, {lo_shown} to {hi_shown} {unit}"
        return f"{label}, {lo_shown} {unit}"

    return str(label)


# Set while an edit is in flight, so the mod's own pushes (a level load, a
# second beamtel request) cannot speak over whatever the user is doing.
_env_awaiting_change = False
_env_change_timer = None
_env_browser_open = False


def _env_change_timed_out():
    global _env_awaiting_change
    if not _env_awaiting_change:
        return
    _env_awaiting_change = False
    # The alternative is a key that appears to do nothing, which is the failure
    # this project has already lost an afternoon to on a dead command port.
    say("No response from the game", exclude_from_buffer=True)


def _env_notify_refresh():
    """Called from the listener thread when a fresh row set lands."""
    global _env_awaiting_change, _env_change_timer
    if _env_browser_open and _vbrowser_active:
        lines = [_env_row_line(r) for r in _env_rows]
        # Replaced in place and only when the shape matches, so a push that
        # arrives while the list is being navigated cannot move the cursor onto
        # a different setting than the one the user last heard.
        if len(lines) == len(_vbrowser_lines):
            _vbrowser_lines[:] = lines
    if _env_awaiting_change:
        _env_awaiting_change = False
        if _env_change_timer is not None:
            try:
                _env_change_timer.cancel()
            except Exception:
                pass
            _env_change_timer = None
        idx = _vbrowser_index
        if 0 <= idx < len(_vbrowser_lines):
            say(_vbrowser_lines[idx], exclude_from_buffer=True)


def _env_arm_change():
    global _env_awaiting_change, _env_change_timer
    _env_awaiting_change = True
    if _env_change_timer is not None:
        try:
            _env_change_timer.cancel()
        except Exception:
            pass
    _env_change_timer = threading.Timer(1.0, _env_change_timed_out)
    _env_change_timer.daemon = True
    _env_change_timer.start()


def _env_on_adjust(idx, line, data, delta):
    """Left/right on an environment row."""
    if data is None or data >= len(_env_rows):
        return
    row = _env_rows[data]
    if row.get("kind") != "numberC":
        return
    if row.get("editable", "1") != "1":
        say("Locked", exclude_from_buffer=True)
        return
    current = _env_float(row, "value")
    if current is None:
        return
    # Stepped in the unit the user HEARS, not in Celsius: a one-degree press on
    # an imperial readout that moved the value by 1.8 F would skip numbers.
    shown, _unit = fmt_temp_c_or_f(current)
    target_c = _env_temp_from_display(shown + delta)
    lo = _env_float(row, "min")
    hi = _env_float(row, "max")
    if lo is not None:
        target_c = max(lo, target_c)
    if hi is not None:
        target_c = min(hi, target_c)
    _env_arm_change()
    _send_env_cmd(f"SET:{row.get('key')}={target_c:.2f}")


def _env_on_enter(idx, line, data):
    """Enter on an environment row."""
    if data is None or data >= len(_env_rows):
        return
    row = _env_rows[data]
    kind = row.get("kind")
    if kind == "action" and row.get("key") == "restore":
        if row.get("editable", "1") != "1":
            say("No level default to restore", exclude_from_buffer=True)
            return
        _env_arm_change()
        _send_env_cmd("RESTORE")
        return
    if kind == "numberC":
        say(
            "Use left and right to adjust, hold shift for ten at a time",
            exclude_from_buffer=True,
        )


def open_environment_browser():
    """Open a virtual browser over the environment values the pause UI omits."""
    global _env_browser_open
    if not _env_rows:
        # Nudge the mod in case the push was missed, then report rather than
        # opening an empty list.
        _send_env_cmd("REQUEST")
        if _env_unavailable:
            say(
                f"Environment unavailable, {_env_unavailable}",
                exclude_from_buffer=True,
            )
        else:
            say("No environment settings available", exclude_from_buffer=True)
        return
    lines = [_env_row_line(r) for r in _env_rows]
    entry_data = list(range(len(_env_rows)))
    open_virtual_browser(
        lines,
        title=f"Environment, {len(lines)} setting{'s' if len(lines) != 1 else ''}",
        on_enter=_env_on_enter,
        on_adjust=_env_on_adjust,
        entry_data=entry_data,
    )
    # Set AFTER the open, never before: open_virtual_browser closes whatever
    # browser was already up, and close_virtual_browser clears this flag. Setting
    # it first left it false, so a value edited from inside the browser updated
    # the rows and never refreshed the line the user was sitting on.
    _env_browser_open = _vbrowser_active


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


def _invoke_f9_command(name, ctrl=False, shift=False, alt=False):
    """Invoke an F9 command in-process without injecting an operating-system key.

    _on_next_key_press duck-types its event -- it reads only .event_type and .name, with
    the modifiers coming from _capture_mods -- so controller and MCP commands can reuse the
    keyboard handlers without BeamNG having focus. The modifier dict is saved and restored
    because a real capture may be in progress, and _command_context is set so speech behaves
    as it would for a genuine keypress.
    """
    global _command_context
    if audio_controller_ref is None:
        raise RuntimeError("audio controller is not up yet")
    saved = dict(_capture_mods)
    prev_ctx = _command_context
    try:
        _capture_mods["ctrl"] = bool(ctrl)
        _capture_mods["shift"] = bool(shift)
        _capture_mods["alt"] = bool(alt)
        _command_context = True
        evt = SimpleNamespace(event_type="down", name=str(name).lower())
        _on_next_key_press(evt, audio_controller_ref)
    finally:
        _command_context = prev_ctx
        _capture_mods.clear()
        _capture_mods.update(saved)
    return True


def _mcp_press_command(name, ctrl=False, shift=False, alt=False):
    """MCP-facing wrapper retained for the server registration below."""
    return _invoke_f9_command(name, ctrl=ctrl, shift=shift, alt=alt)


def _on_next_key_press(event, audio_controller):
    global marked_coord_x, marked_coord_y, _last_coord_bearing_ts, _input_help_mode
    global dock_mode_active, last_dock, last_dock_fail, last_dock_mode, last_dock_name
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
    elif (
        name == "r"
        and _capture_mods["ctrl"]
        and _capture_mods["shift"]
        and not _capture_mods["alt"]
    ):
        speak_road_status()
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
        #
        # Deliberately does NOT short-circuit on _implement_word_current. That is this
        # side's guess at what the mod knows, and gating on it masked the mod's own, far
        # more specific answer — "no implement fitted" was reported for three separate
        # underlying causes, none of which it could distinguish. Ask, and report what comes
        # back.
        with state_lock:
            in_cannon = cannon_active
            active_cannon_kind = cannon_kind
            cannon_aim = dict(last_cannon_aim) if last_cannon_aim else None
            rpm_now = last_rpm
            gear_now = last_gear_str
            deck = dict(last_ramp_self) if last_ramp_self else None
            dock_snap = dict(last_dock) if last_dock else None
            dock_why = last_dock_fail
        if active_cannon_kind == "OLD":
            if (
                cannon_aim is None
                or time.monotonic() - cannon_aim.get("stamp", 0.0)
                > CANNON_READOUT_STALE_S
            ):
                say("Old Cannon elevation unavailable", exclude_from_buffer=True)
            else:
                elevation = cannon_aim["elevation"]
                bits = [f"Elevation {elevation:.1f} degrees"]
                if cannon_aim["range"] < 0.0:
                    bits.append("select a scanner target")
                else:
                    bearing = cannon_aim["bearing"]
                    if abs(bearing) <= 0.5:
                        bits.append("bearing aligned")
                    else:
                        bits.append(
                            f"{abs(bearing):.1f} degrees "
                            f"{'left' if bearing > 0 else 'right'}"
                        )
                    rv, ru = fmt_distance(cannon_aim["range"])
                    if not cannon_aim["solution_available"]:
                        bits.append(
                            f"line of sight {cannon_aim['line_angle']:.1f} degrees"
                        )
                        bits.append("ballistic solution unavailable")
                    elif not cannon_aim["reachable"]:
                        bits.append("target out of ballistic range")
                    else:
                        target_angle = cannon_aim["target_angle"]
                        error = target_angle - elevation
                        if abs(error) <= 0.5:
                            bits.append(f"elevation aligned at {target_angle:.1f} degrees")
                        else:
                            bits.append(
                                f"{'raise' if error > 0 else 'lower'} "
                                f"{abs(error):.1f} degrees to {target_angle:.1f}"
                            )
                    bits.append(f"{rv} {ru}")
                say(". ".join(bits), exclude_from_buffer=True)
        elif in_cannon:
            # Sitting in the cannon, the alignment task is over and the aiming task has begun,
            # so the same key answers the question you actually have. Folded in rather than
            # given a binding of its own: there is no alignment readout to displace, and a
            # second key is one more thing to remember mid-manoeuvre.
            #
            # Both numbers already arrive on ordinary telemetry, because large_cannon's
            # controller publishes them there itself — rpm = inclination * 1000 and the gear
            # string is the shoot strength as a percentage. Nothing extra is polled.
            incl = max(0.0, min(1.0, (rpm_now or 0.0) / 1000.0))
            strength = (gear_now or "").strip()
            if not strength.endswith("%"):
                strength = "unknown"
            say(
                f"Inclination {incl * 100:.0f} percent, strength {strength}",
                exclude_from_buffer=True,
            )
        elif dock_mode_active and dock_snap is not None:
            say(_dock_phrase(dock_snap), exclude_from_buffer=True)
        elif deck is not None:
            # You are sitting in a ramp machine and the instrument has nothing to line you up
            # with, so the key answers the question that is left: what is your own deck doing.
            #
            # Ordered strictly BELOW a live docking reading, which is the conservative half of
            # this. The cannon branch above wins outright on the argument that once you are in
            # the cannon the alignment task is over; that argument does not carry here, because
            # a hauler is a perfectly ordinary thing to drive up somebody else's ramp, and an
            # alignment readout the driver deliberately switched on must not be displaced by a
            # fact about their own bed. So this fills in the three cases that previously had
            # nothing useful to say and changes no case that did.
            say(_ramp_self_phrase(deck), exclude_from_buffer=True)
        elif not dock_mode_active:
            say("Docking instrument is off", exclude_from_buffer=True)
        elif dock_why:
            # The instrument shares its soundscape with the scanner, which also pans,
            # changes pitch and pulses — so a dead instrument sounds exactly like a
            # working one. This is the only way to tell them apart from the driver's
            # seat, which is why the reason is carried all the way from the mod.
            say(f"No reading. {dock_why}", exclude_from_buffer=True)
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
        with state_lock:
            snap_mode = (last_dock or {}).get("mode")
        if snap_mode == "RAMP":
            # A ramp has one reference — its mouth — so there is nothing to cycle. Saying so
            # beats silently sending a command that mutates an index governing nothing.
            say("No reference bands in ramp alignment", exclude_from_buffer=True)
        elif not _implement_word_current:
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
                last_dock_mode = None
                last_dock_name = None
            say("Docking instrument off", exclude_from_buffer=True)
        else:
            # The re-announce above travels to the mod and back, and the mod scans at 10 Hz,
            # so read the answer rather than the stale value. Without this the toggle reports
            # "no implement fitted" from state it is in the middle of refreshing.
            time.sleep(0.3)
            # Auto-select is per target, so a fresh mode start should not inherit a band
            # pick made against something you have since driven away from.
            _send_implement_cmd("BANDAUTO")
            with state_lock:
                why = last_dock_fail
                mode = (last_dock or {}).get("mode")
                tgt = (last_dock or {}).get("name")
                # Claim both latches, so the listener does not also announce the mode and the
                # machine we are about to name here.
                last_dock_mode = mode
                last_dock_name = tgt
            # Name the mode on the way in, and in ramp mode the machine with it — this and the
            # listener's announcement are now the only two places the target is named, since
            # the cane tap dropped it. The instrument auto-selects the mode, so without this
            # the only way to know which of two meanings the tones carry is to infer it from
            # what you happen to be sitting in.
            which = (
                ("ramp alignment" + (f", {tgt}" if tgt else ""))
                if mode == "RAMP"
                else "implement alignment" if mode == "IMPL" else None
            )
            head = "Docking instrument on" + (f", {which}" if which else "")
            # Report the MOD's reason, not a guess from this side's own state. "No implement
            # fitted" used to be said here on the strength of a Python variable, which was
            # true for three different underlying causes and pointed at none of them.
            if why:
                say(f"{head}. {why}", exclude_from_buffer=True)
            else:
                say(head, exclude_from_buffer=True)
    elif (
        name == "w"
        and _capture_mods["ctrl"]
        and not (_capture_mods["shift"] or _capture_mods["alt"])
    ):
        toggle_route_beacon(audio_controller)
    elif (
        name == "w"
        and _capture_mods["shift"]
        and not (_capture_mods["ctrl"] or _capture_mods["alt"])
    ):
        trigger_nearest_poi()
    elif name == "w" and not (
        _capture_mods["ctrl"] or _capture_mods["shift"] or _capture_mods["alt"]
    ):
        if marked_coord_x is None:
            say("No waypoint marked", exclude_from_buffer=True)
        else:
            dist, err = relative_bearing(
                x, y, heading_snap, marked_coord_x, marked_coord_y
            )
            if dist < 0.5:
                say("At waypoint", exclude_from_buffer=True)
            else:
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
        name == "b"
        and _capture_mods["shift"]
        and not _capture_mods["ctrl"]
        and not _capture_mods["alt"]
    ):
        # Shift+B sits next to plain B on purpose: the browser LISTS this vehicle's special
        # controls, and this names whatever control you press on any device. Same subject,
        # two directions.
        toggle_binding_learn_mode()
    elif (
        name == "n"
        and not _capture_mods["ctrl"]
        and not _capture_mods["shift"]
        and not _capture_mods["alt"]
    ):
        # The environment values the pause screen has no control for at all.
        # Temperature is the only one today; a browser rather than a stepper key
        # so the next one costs a row rather than a keybind.
        #
        # N rather than the obvious E: plain E is "Speak engine temperature" and
        # its branch sits earlier in this chain, so an E binding here would have
        # been unreachable -- dead code that reads as a working feature.
        open_environment_browser()
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
        # One key, two alignments, disambiguated by the docking instrument. Coupling to a
        # trailer and driving up its ramp are the same act from the seat -- get me squared up
        # to that machine -- and they are indistinguishable from a keypress, so the
        # disambiguator has to be something already switched on for one of them. The docking
        # instrument is exactly that: it is what you run to drive onto something, and it is the
        # half of the mod that knows where a ramp mouth is at all.
        #
        # Deliberately NOT falling through to the coupler align when no ramp is found. With the
        # instrument on you asked for a ramp; quietly lining you up to reverse onto a tow hitch
        # instead is a surprise, and a named reason is what tells you whether to keep looking.
        if dock_mode_active:
            say("Aligning to ramp", exclude_from_buffer=True)
            _send_implement_cmd("RAMPALIGN")
        elif not scan_mode_active:
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
        # One key, four answers. On the stock vehicle selector it reads the details page's
        # specifications; on a mod repository details page it reads that mod's sheet and
        # description; driving, it is the terrain sonification scan; in a menu it activates
        # the on-screen control, which is what this key has always done.
        #
        # The mod page sits after the selector and before the scan for the selector's own
        # reason: being on that route is a POSITIVE fact about an open screen, where
        # _world_is_active() is a recency heuristic that also goes false while paused. Its
        # "notdetails" answer -- which is what every other screen in the game returns -- falls
        # through in silence, so nothing that works today can regress.
        #
        # The selector is asked FIRST, and that ordering is the one real priority call here.
        # The other two disambiguate cleanly on _world_is_active(), because the only screens
        # offering a context action run with no level loaded. The selector does not: it is
        # reachable from the pause menu with a level loaded, so it can look applicable at the
        # same time as the scan. It wins because it is a positive fact about a screen that is
        # open, where _world_is_active() is a recency heuristic that also goes false while
        # paused. A refusal or a silent mod falls straight through to the old behaviour, so
        # nothing that works today can regress.
        #
        # The scan says nothing on success on purpose — the reference ping at the head of the
        # cloud IS the acknowledgement, and speech would talk over the first half second of
        # the very scan it was announcing. Silence and a dead extension are otherwise
        # identical, so a watcher speaks if the mod never answers.
        info_lines, info_fail = request_vehicle_info()
        info_code = info_fail[0] if info_fail else None
        if info_lines:
            open_virtual_browser(
                info_lines,
                title=f"Vehicle information, {len(info_lines)} items",
            )
        elif info_code not in (None, "notselector", "timeout"):
            # Spoken only when the mod was ON the selector and still could not answer — the
            # user asked a question about a vehicle and deserves the reason. "notselector" is
            # the answer on every other screen in the game and must fall through silently, and
            # a timeout says nothing about the vehicle at all, only that the mod half is
            # missing or deaf.
            say(info_fail[1] or "No vehicle information", exclude_from_buffer=True)
        elif _read_mod_details_page():
            pass
        elif _world_is_active():
            trigger_terrain_scan_driving_only()
        else:
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
#  Screen capture (shared by the AI Describer and the MCP screenshot tool)
# =========================

# How long to let the game render a UI-free frame after HIDE before grabbing.
UI_HIDE_SETTLE_S = 0.2

# Held across the whole HIDE -> grab -> SHOW sequence. Two overlapping captures race:
# the first one's SHOW lands mid-grab of the second, so the shot that asked for a clean
# world gets the HUD back in it. Deliberately NOT `_describer_lock` below -- that one
# exists to make a double-press of F10+Space buzz rather than queue a second AI request,
# and an agent screenshot must neither be refused because a description is in flight nor
# consume the user's busy flag.
_capture_lock = threading.Lock()


# How long to wait after raising the game window before grabbing. Longer than the UI
# settle: a minimized BeamNG reports itself as "- background" in its own title bar and
# throttles rendering, so it needs time to come back up and draw a real frame, not just
# to repaint one.
GAME_FOCUS_SETTLE_S = 0.6

_GAME_WINDOW_HINT = "BeamNG.drive"


def _find_game_window():
    """The BeamNG.drive main window, or None.

    Matched on the title rather than the process, because the mod's own wx frame is
    called "BeamNG Accessibility" and would otherwise match a process-name search for
    anything BeamNG-shaped.
    """
    try:
        import ctypes
        from ctypes import wintypes
    except Exception:
        return None
    u = ctypes.windll.user32
    found = []

    def cb(hwnd, _lparam):
        if not u.IsWindowVisible(hwnd):
            return True
        n = u.GetWindowTextLengthW(hwnd)
        if not n:
            return True
        buf = ctypes.create_unicode_buffer(n + 1)
        u.GetWindowTextW(hwnd, buf, n + 1)
        if buf.value.startswith(_GAME_WINDOW_HINT):
            found.append(hwnd)
        return True

    proc = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)(cb)
    try:
        u.EnumWindows(proc, 0)
    except Exception:
        return None
    return found[0] if found else None


def _raise_game_window(hwnd):
    """Restore and foreground the game window. Returns the previous foreground hwnd.

    SetForegroundWindow is refused for a process that does not already own the
    foreground -- and beamtel runs elevated while the game does not -- so the input
    queues are attached first, which is the documented way to be allowed. Every step is
    best-effort: a refusal must degrade to a worse screenshot, never to an exception.
    """
    import ctypes

    u = ctypes.windll.user32
    prev = u.GetForegroundWindow()
    SW_RESTORE = 9
    try:
        if u.IsIconic(hwnd):
            u.ShowWindow(hwnd, SW_RESTORE)
        cur_thread = ctypes.windll.kernel32.GetCurrentThreadId()
        tgt_thread = u.GetWindowThreadProcessId(hwnd, None)
        attached = False
        if tgt_thread and tgt_thread != cur_thread:
            attached = bool(u.AttachThreadInput(cur_thread, tgt_thread, True))
        try:
            u.BringWindowToTop(hwnd)
            u.SetForegroundWindow(hwnd)
        finally:
            if attached:
                u.AttachThreadInput(cur_thread, tgt_thread, False)
    except Exception as e:
        logger.error(f"Could not raise the game window: {e}")
    return prev


def _window_region(hwnd):
    """The window's screen rectangle as an mss region dict, or None if degenerate."""
    import ctypes
    from ctypes import wintypes

    r = wintypes.RECT()
    if not ctypes.windll.user32.GetWindowRect(hwnd, ctypes.byref(r)):
        return None
    w, h = r.right - r.left, r.bottom - r.top
    # A minimized window parks at -32000; anything tiny or off-screen is not a frame.
    if w < 64 or h < 64 or r.left < -30000 or r.top < -30000:
        return None
    return {"left": r.left, "top": r.top, "width": w, "height": h}


def capture_scene_png(hide_ui=True, settle_s=UI_HIDE_SETTLE_S, focus_game=False):
    """Grab the game as PNG bytes. Returns (png_bytes, meta).

    `focus_game` exists because the two callers are in genuinely different situations.
    The AI Describer fires from a keypress while the user is playing, so the game is
    already foreground and raising it would be a no-op with a real cost -- a window
    z-order change under someone mid-corner. An agent's screenshot fires while the user
    is looking at a terminal, and BeamNG is then typically MINIMIZED: it parks its window
    off-screen at -32000 and renames itself "- background", so a primary-monitor grab
    returns the desktop. That failure is silent and entirely plausible-looking, which is
    the one kind this project cares most about -- hence `meta`, which reports what was
    actually captured so a wrong picture can be diagnosed instead of believed.

    The UI is always restored, and the previously focused window is always put back.
    """
    import ai_describer

    meta = {"focused_game": False, "game_window_found": False, "region": None}

    with _capture_lock:
        hwnd = _find_game_window() if focus_game else None
        meta["game_window_found"] = hwnd is not None
        prev = None
        try:
            if hwnd is not None:
                prev = _raise_game_window(hwnd)
                time.sleep(GAME_FOCUS_SETTLE_S)
                region = _window_region(hwnd)
                meta["region"] = region
                meta["focused_game"] = region is not None
                if region is None:
                    meta["warning"] = (
                        "the game window would not come up (still minimized or "
                        "off-screen); this is a grab of the primary monitor instead"
                    )
            if hide_ui:
                _send_ui_command("HIDE")
                time.sleep(max(0.0, settle_s))
            png = ai_describer.capture_region(meta["region"])
            return png, meta
        finally:
            if hide_ui:
                _send_ui_command("SHOW")
            if prev:
                try:
                    ctypes_user32 = __import__("ctypes").windll.user32
                    ctypes_user32.SetForegroundWindow(prev)
                except Exception:
                    pass


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
        # Stored DPAPI-sealed; unseal at the point of use so the plaintext lives
        # only in this worker's frame and never in `ai_describer_settings`.
        stored_key = settings.get(key_cfg, "")
        api_key = secretstore.unprotect(stored_key)
        if api_key is None:
            # Sealed, but not by this Windows account -- a copied config or a
            # restored profile. Distinct from "no key set": the fix is to enter
            # it again, and saying "not set" would send the user looking for a
            # key that is plainly there in the file.
            msg = (
                "The stored API key could not be decrypted on this Windows account. "
                "Set it again in the AI Describer tab."
            )
            ai_describer.log_error(msg)
            say(msg)
            return
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
            png, _cap_meta = capture_scene_png(hide_ui=toggle_ui)
        except Exception as e:
            capture_err = f"Screenshot failed: {e}"

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

    _start_command_worker()

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
        last_turbo_max, \
        last_protocol_flags
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
    global last_tire_pressure_f, last_tire_pressure_r
    global last_tire_temp_f, last_tire_temp_r
    global last_brake_temp_f, last_brake_temp_r, last_telemetry_presence
    global \
        last_signal_left_input, \
        last_signal_right_input, \
        last_hazard_enabled, \
        last_lightbar, \
        last_fog
    global last_gear_byte, last_gear_str, last_bucket, last_speed_announce_ts
    global _telemetry_baseline_pending
    global _last_telemetry_ts
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
    global last_slip_active, last_slip_kind, last_slip_mag, last_tc_active
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

            _last_telemetry_ts = now

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
            elif protocol_mode == "extended" and len(data) == EXT_SIZE_V2:
                try:
                    unpacked = struct.unpack(EXT_FORMAT_V2, data) + EXT_V2_WHEEL_DEFAULTS
                except Exception:
                    continue
                if not _ext_version_warned:
                    _ext_version_warned = True
                    logger.warning(
                        f"Extended telemetry packet is {EXT_SIZE_V2} bytes, expected "
                        f"{EXT_SIZE}. The Lua mod in bng_mod/ is older than this build; "
                        "centred-wheel telemetry will be unavailable."
                    )
            elif protocol_mode == "extended" and len(data) == EXT_SIZE_V1:
                # Older mod half. Decode what it does send and pad the implement block, so a
                # version skew costs the loader features rather than all telemetry.
                try:
                    unpacked = struct.unpack(EXT_FORMAT_V1, data) + (
                        EXT_V1_IMPLEMENT_DEFAULTS + EXT_V2_WHEEL_DEFAULTS
                    )
                except Exception:
                    continue
                if not _ext_version_warned:
                    _ext_version_warned = True
                    logger.warning(
                        f"Extended telemetry packet is {EXT_SIZE_V1} bytes, expected "
                        f"{EXT_SIZE}. The Lua mod in bng_mod/ is older than this build; "
                        "implement and centred-wheel telemetry will be unavailable."
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
                    protocol_flags = int(unpacked[0])
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
                    ) = unpacked[14:50]
                    (
                        tire_p_f,
                        tire_p_r,
                        tire_t_f,
                        tire_t_r,
                        brake_t_f,
                        brake_t_r,
                        telemetry_presence,
                    ) = unpacked[50:]
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
                    last_tire_pressure_f, last_tire_pressure_r = tire_p_f, tire_p_r
                    last_tire_temp_f, last_tire_temp_r = tire_t_f, tire_t_r
                    last_brake_temp_f, last_brake_temp_r = brake_t_f, brake_t_r
                    last_telemetry_presence = int(telemetry_presence)

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
                    protocol_flags = int(unpacked[2])
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
                    last_protocol_flags,
                ) = speed_ms, rpm, fuel, turbo, engtemp, oiltemp, protocol_flags
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
                last_tc_active = tc_active_frame

                if protocol_mode == "extended":
                    gear_str = (
                        unpacked[1].decode("utf-8", errors="ignore").strip("\x00")
                    )
                    if gear_str != last_gear_str:
                        # Suppressed in a cannon. large_cannon's controller writes the shoot
                        # strength into electrics.values.gear as "80%", and rewrites it on
                        # every press of the strength keys — so aiming would produce a stream
                        # of gear announcements over the whole manoeuvre. The strength is not
                        # lost: F9+I reads it out on demand, which is the right shape for a
                        # value you sweep and then check.
                        #
                        # Read directly, NOT under a `with state_lock:` of its own. This
                        # whole block already runs inside the telemetry loop's state_lock,
                        # and state_lock is a plain (non-reentrant) Lock, so re-acquiring it
                        # here deadlocks the telemetry thread *while holding the lock* — on
                        # the very first packet, since last_gear_str starts as None. Every
                        # other feature then hangs behind it: no telemetry, and no F9 layered
                        # key either, because _on_next_key_press takes state_lock before
                        # dispatching. F9 and F11 keep working, which is what makes it look
                        # like a Lua fault rather than a Python one.
                        in_cannon = cannon_active
                        if announce_gear and not baseline_frame and not in_cannon:
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

            _push_obstacle_state()

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
            last_slip_active = slip_active
            last_slip_kind = slip_kind
            last_slip_mag = slip_mag

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
                        coord_target_bearing = target_bearing(
                            last_pos_x, last_pos_y, marked_coord_x, marked_coord_y
                        )
                    _last_coord_bearing_ts = now

            coord_bearing_error = 0.0
            if coord_guidance_active and marked_coord_x is not None:
                coord_bearing_error = normalize_bearing(
                    last_heading - coord_target_bearing
                )

            # The route beacon's bearing is derived HERE, from the position and heading
            # this loop already has at 60 Hz, rather than being sent by the mod -- see
            # the route state block for why. The age-out lives here too, because this is
            # where the value is read.
            if route_beacon_active and _route_is_set():
                rb_dist, rb_bearing = relative_bearing(
                    last_pos_x, last_pos_y, last_heading, route_dest_x, route_dest_y
                )
                # Standing on the destination: the bearing to a point you are on is
                # residue and would spin the beacon around the head. Silence is the
                # honest answer, and the route is about to clear itself anyway.
                audio_controller.update_route_beacon(
                    rb_dist > AT_DESTINATION_M, rb_bearing, rb_dist
                )
            elif route_beacon_active:
                audio_controller.update_route_beacon(False)

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
                    # Same scale as last_actual_steering and driving the same tone — see
                    # audio.py's source selection. Reverse comes from the gear rather than
                    # from velocity for the reason _push_gear_direction gives: at a standstill
                    # about to reverse, the instrument should already be at full volume, and
                    # velocity reads zero there.
                    "trailer_artic": _trailer_artic_norm(),
                    "trailer_reverse": (last_gear_str or "").strip().upper().startswith("R"),
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
        # The version goes in the title bar because it is the one place a
        # screen reader reads without being asked, which is what makes it
        # useful for telling at a glance whether an update actually landed --
        # the question the updater's restart makes hard to answer otherwise.
        # Imported here rather than at module scope to match every other use of
        # updater in this file, and falling back to an unversioned title: a
        # window that will not open is a far worse failure than one that cannot
        # name itself.
        try:
            from updater import APP_VERSION

            title = "BeamNG Accessibility %s" % APP_VERSION
        except Exception as e:
            logger.error(f"Could not read APP_VERSION for the title bar: {e}")
            title = "BeamNG Accessibility"
        super().__init__(None, title=title, size=(700, 700))
        self.SetMinSize((600, 500))
        self._engine_thread = None
        self._shutting_down = False
        self._wx_log_handler = None

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

        # Log-file button.  One button and a pop-up menu rather than a button
        # per log: the list has outgrown a row -- six files, three of which
        # exist only in some sessions -- and every extra button here is another
        # stop a keyboard user tabs through on the way to the console below.
        log_btn_sizer = wx.BoxSizer(wx.HORIZONTAL)
        self.btn_view_logs = wx.Button(main_panel, label="View Logs...")
        self.btn_view_logs.SetToolTip(
            "Open one of the log files BEAM writes, or the folder holding them."
        )
        log_btn_sizer.Add(self.btn_view_logs, 0, wx.RIGHT, 5)
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
        btn_check_updates = wx.Button(main_panel, label="Check for Updates")
        btn_check_updates.SetToolTip(
            "Ask GitHub whether a newer version of BEAM has been released."
        )
        bottom_btn_sizer.Add(btn_check_updates, 0, wx.RIGHT, 5)
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
        self._wx_log_handler = wx_handler

        # Events
        self.btn_view_logs.Bind(wx.EVT_BUTTON, self._on_view_logs)
        btn_install_mod.Bind(wx.EVT_BUTTON, lambda evt: install_mod_interactive(self))
        btn_check_updates.Bind(wx.EVT_BUTTON, self._on_check_updates)
        btn_exit.Bind(wx.EVT_BUTTON, lambda evt: self.Close())
        self.Bind(wx.EVT_CLOSE, self._on_close)

        self.Centre()

    def _on_check_updates(self, evt):
        """Manual update check.

        Runs the same flow as startup, with a gate that decides nothing: by the
        time this button can be pressed the deferred launch has long since been
        released, and a manual check must never fire a second one. `manual`
        additionally makes it ignore the config gate and report the
        already-up-to-date case, which the silent startup check does not.
        """
        try:
            import updater

            updater.run_startup_flow(self, updater.NullGate(), manual=True)
        except Exception as e:
            logger.error(f"Manual update check failed: {e}")
            wx.MessageBox(
                f"The update check could not run.\n\n{e}",
                "Update Check Failed",
                wx.OK | wx.ICON_ERROR,
                self,
            )

    # ---- Log-file openers ----

    @staticmethod
    def _open_file_if_exists(path):
        if os.path.isfile(path):
            os.startfile(path)
        else:
            wx.MessageBox(
                f"File not found:\n{path}", "Not Found", wx.OK | wx.ICON_WARNING
            )

    @staticmethod
    def _log_menu_entries():
        """(label, target) for every log BEAM writes, in menu order.

        ``target`` is a path, or a list of (label, path) pairs to be rendered
        as a submenu.

        Built at call time rather than at import, and each reason would
        otherwise cost an entry: two of the paths belong to modules this file
        imports lazily everywhere else (``ai_describer``, ``updater``), the
        rotated backups of the application log exist only once it has turned
        over, and the scanner diagnostic is written only when
        BEAM_SCANNER_DIAG is set.

        A file that does not exist is still listed.  "There is no AI
        description log yet" and "I cannot find the AI description log" are
        different answers and only the first one is true; hiding the row leaves
        the reader to work out which, from an absence.  The rotated backups are
        the one exception -- an absent ``.3`` is not a fact about anything.
        """
        # bnh_logger's RotatingFileHandler keeps three backups, which had no
        # route out of the UI at all -- and they are exactly what is wanted
        # after a crash, since the live file has usually rolled well past it.
        # They go in a submenu rather than in the top level because they are
        # one log in four pieces, not four logs: flat, they were most of the
        # menu, and every one of them read as a peer of the Speech Log.  The
        # submenu costs a Right Arrow to enter and gives the top level back to
        # the six distinct logs.
        rolled = [
            ("Previous %d" % n, "%s.%d" % (LOG_FILENAME, n))
            for n in (1, 2, 3)
            if os.path.isfile("%s.%d" % (LOG_FILENAME, n))
        ]
        if rolled:
            # "Current" only earns a name once there is something to
            # distinguish it from; with no backups the submenu would be one
            # item deep, i.e. a Right Arrow charged for nothing.
            entries = [("Application Log", [("Current", LOG_FILENAME)] + rolled)]
        else:
            entries = [("Application Log", LOG_FILENAME)]
        entries.append(("Speech Log", SPEECH_LOG_PATH))
        try:
            import ai_describer

            entries.append(("AI Description Log", ai_describer.LOG_PATH))
        except Exception as e:
            logger.error(f"Could not resolve the AI description log path: {e}")
        entries.append(("DOM Dump", DOM_DUMP_PATH))
        try:
            from scanner_hrtf_diag import DEFAULT_PATH as scanner_diag_path

            entries.append(("Scanner and HRTF Diagnostic", scanner_diag_path))
        except Exception as e:
            logger.error(f"Could not resolve the scanner diagnostic path: {e}")
        try:
            import updater

            entries.append(
                (
                    "Update Helper Log",
                    os.path.join(updater.UPDATE_DIR, updater.HELPER_LOG),
                )
            )
        except Exception as e:
            logger.error(f"Could not resolve the update helper log path: {e}")
        return entries

    def _on_view_logs(self, evt):
        """Pop up the log menu under the View Logs button.

        A menu rather than a dialog because these are one-shot actions with no
        state to confirm, and Windows announces a menu without any of the
        framing a modal costs.  A file that has not been written yet stays in
        the menu, disabled, and says so in its own label: a disabled item is
        still arrowed onto and read out, so the answer arrives without the user
        having to activate anything and then dismiss a warning box.

        Submenus are plain ``AppendSubMenu`` children, so Right Arrow opens one
        and Left Arrow closes it -- the native Windows menu behaviour a screen
        reader already announces.  Nothing here handles a key.
        """
        menu = wx.Menu()

        def add_file_item(into, label, path):
            """Append one log to `into`, disabled and marked if it is absent."""
            exists = os.path.isfile(path)
            item = into.Append(
                wx.ID_ANY, label if exists else "%s (not created yet)" % label
            )
            item.SetHelp(path)
            if not exists:
                item.Enable(False)
                return
            # Bound on the TOP-LEVEL menu even for a submenu item: wxMSW routes
            # a popup selection through the menu that was popped up, never
            # through the submenu the item happens to live in, so a handler
            # bound on the child would simply never run.
            menu.Bind(
                wx.EVT_MENU, lambda e, p=path: self._open_file_if_exists(p), item
            )

        for label, target in self._log_menu_entries():
            if isinstance(target, list):
                sub = wx.Menu()
                for sub_label, sub_path in target:
                    add_file_item(sub, sub_label, sub_path)
                menu.AppendSubMenu(sub, label)
            else:
                add_file_item(menu, label, target)

        menu.AppendSeparator()
        folder_item = menu.Append(wx.ID_ANY, "Open Log Folder")
        folder_item.SetHelp(CONFIG_DIR)
        menu.Bind(wx.EVT_MENU, self._on_open_log_folder, folder_item)
        # Anchored to the button rather than to the mouse, so it opens where
        # the keyboard focus already is when the button is pressed with Space.
        self.btn_view_logs.PopupMenu(menu, (0, self.btn_view_logs.GetSize().height))
        menu.Destroy()

    def _on_open_log_folder(self, evt):
        try:
            os.startfile(CONFIG_DIR)
        except Exception as e:
            logger.error(f"Could not open the log folder: {e}")
            wx.MessageBox(
                f"The log folder could not be opened.\n\n{e}",
                "Open Failed",
                wx.OK | wx.ICON_ERROR,
                self,
            )

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
        # CallAfter events queued before the frame was destroyed still fire after it,
        # and touching a dead control raises a C++ assertion out of wx's CallAfter
        # lambda rather than a catchable Python error at the call site.
        if self._shutting_down:
            return
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
        # Stop feeding the wx event queue before anything else: the join below does
        # not pump events, so everything the listener threads emit while they wind
        # down would run against destroyed controls.
        global console_frame
        self._shutting_down = True
        console_frame = None
        if self._wx_log_handler is not None:
            logging.getLogger("bnvdahook").removeHandler(self._wx_log_handler)
            self._wx_log_handler = None
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
    global announce_clickspot_actions
    global scanner_distance_callout_enabled, scanner_distance_callout_interval
    global announce_implement_proximity, announce_cannon_shot
    global announce_binding_learn_description
    global road_follow_guidance_enabled, road_junction_speech_enabled
    global road_junction_earcon_enabled, road_include_private
    global obstacle_warning_sensitivity
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
        announce_clickspot_actions = bool(
            cfg.get("announce_clickspot_actions", False)
        )
        scanner_distance_callout_enabled = cfg.get("scanner_distance_callout_enabled", False)
        scanner_distance_callout_interval = cfg.get("scanner_distance_callout_interval", 10)
        announce_implement_proximity = bool(cfg.get("implement_proximity_speech", True))
        announce_cannon_shot = bool(cfg.get("cannon_shot_readout", True))
        announce_binding_learn_description = bool(
            cfg.get("binding_learn_speak_description", False)
        )
        road_follow_guidance_enabled = bool(
            cfg.get("road_follow_guidance_enabled", True)
        )
        road_junction_speech_enabled = bool(
            cfg.get("road_junction_speech_enabled", True)
        )
        road_junction_earcon_enabled = bool(
            cfg.get("road_junction_earcon_enabled", True)
        )
        road_include_private = bool(cfg.get("road_include_private", False))
        obstacle_warning_sensitivity = cfg.get("obstacle_warning_sensitivity", "normal")
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
    _send_road_configuration()
    if obstacle_mode_active:
        _send_obstacle_configuration()
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
    global announce_clickspot_actions
    global scanner_distance_callout_enabled, scanner_distance_callout_interval
    global announce_implement_proximity, announce_cannon_shot
    global announce_binding_learn_description
    global road_follow_guidance_enabled, road_junction_speech_enabled
    global road_junction_earcon_enabled, road_include_private
    global obstacle_warning_sensitivity
    global ui_nav_hold_suppression
    UNITS_MODE = cfg.get("units", "imperial")
    oil_chime_enabled = cfg.get("oil_chime_enabled", True)
    announce_turn_signals = cfg.get("announce_turn_signals", True)
    announce_speed = cfg.get("announce_speed", True)
    speed_announce_interval = cfg.get("speed_announce_interval", 25)
    announce_gear = cfg.get("announce_gear", True)
    announce_clickspot_actions = bool(cfg.get("announce_clickspot_actions", False))
    scanner_distance_callout_enabled = cfg.get("scanner_distance_callout_enabled", False)
    scanner_distance_callout_interval = cfg.get("scanner_distance_callout_interval", 10)
    announce_implement_proximity = bool(cfg.get("implement_proximity_speech", True))
    announce_cannon_shot = bool(cfg.get("cannon_shot_readout", True))
    announce_binding_learn_description = bool(
        cfg.get("binding_learn_speak_description", False)
    )
    road_follow_guidance_enabled = bool(cfg.get("road_follow_guidance_enabled", True))
    road_junction_speech_enabled = bool(cfg.get("road_junction_speech_enabled", True))
    road_junction_earcon_enabled = bool(cfg.get("road_junction_earcon_enabled", True))
    road_include_private = bool(cfg.get("road_include_private", False))
    obstacle_warning_sensitivity = cfg.get("obstacle_warning_sensitivity", "normal")
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

    # Controller actions arrive on the TCP event-loop thread even when the
    # keyboard module is unavailable or cannot install elevated hooks.
    _start_command_worker()
    register_accessibility_action_callback(_on_accessibility_action)

    watcher_thread = threading.Thread(target=_config_watcher, args=(STOP,), daemon=True)
    watcher_thread.start()

    hrir_path = os.path.join(HERE, "hrtf_kemar_horizontal.npz")
    audio_controller.load_hrtf(hrir_path)

    _ws_thread, _ws_stop = None, lambda: None
    HILL_CLIMB_CHALLENGE.set_finalize_callback(_hill_climb_finalized)
    try:
        _ws_thread, _ws_stop = start_server_in_thread(
            lambda text, interrupt=True: say(text, interrupt, source="ui_bridge")
        )
    except Exception as _e:
        logger.error(f"Failed to start NVDA WS/HTTP bridge: {_e}")

    register_dom_dump_callback(_on_dom_dump_received)
    register_loading_state_callback(_on_loading_state_changed)
    register_settings_request_callback(_broadcast_ui_settings)
    register_screen_context_callback(_on_screen_context)
    register_page_text_callback(_on_page_text)
    register_challenge_event_callback(_on_hill_climb_event)

    # Wait for the updater's answer before touching the game. The timeout is the
    # failure path, not the normal one: a flow that never answers -- a dialog the
    # user walked away from, a crash before the gate is set -- degrades to today's
    # behaviour rather than to a game that never starts.
    if not LAUNCH_GATE.wait(timeout=180):
        logger.warning("Updater did not answer within 180s; launching as configured.")
    if not LAUNCH_ALLOWED:
        logger.info("Game launch skipped: an update is being applied.")
    elif LAUNCH_SUPPRESSED_BY_SHIFT:
        logger.info("Game launch skipped: Shift held at startup.")
    elif cfg.get("launch_beamng", False):
        try:
            import subprocess

            system_root = os.environ.get("SystemRoot") or r"C:\Windows"
            tasklist_exe = os.path.join(system_root, "System32", "tasklist.exe")
            result = subprocess.run(
                [
                    tasklist_exe,
                    "/FI",
                    "IMAGENAME eq BeamNG.drive.x64.exe",
                    "/NH",
                ],
                capture_output=True,
                text=True,
                timeout=5,
                # beamtel is a GUI executable with no console to inherit. Without
                # this flag, Windows is free to allocate a terminal just for this
                # short process probe -- especially visible after an updater
                # restart, when the main window has just disappeared.
                creationflags=subprocess.CREATE_NO_WINDOW,
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

    terrain_scan_thread = threading.Thread(
        target=terrain_scan_listener, args=(audio_controller, STOP), daemon=True
    )
    terrain_scan_thread.start()

    cannon_shot_thread = threading.Thread(
        target=cannon_shot_listener, args=(STOP,), daemon=True
    )
    cannon_shot_thread.start()

    trailer_angle_thread = threading.Thread(
        target=trailer_angle_listener, args=(STOP,), daemon=True
    )
    trailer_angle_thread.start()

    route_beacon_thread = threading.Thread(
        target=route_beacon_listener, args=(STOP,), daemon=True
    )
    route_beacon_thread.start()

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

    binding_learn_thread = threading.Thread(
        target=binding_learn_listener, args=(STOP,), daemon=True
    )
    binding_learn_thread.start()
    binding_learn_keepalive = threading.Thread(
        target=binding_learn_keepalive_thread_fn, args=(STOP,), daemon=True
    )
    binding_learn_keepalive.start()
    # No startup request: the mode starts off, and a LEARN_OFF here would be answered by a mod
    # half that is not in the mode with a message the user did not ask for. If the mode WAS
    # somehow left on by a previous beamtel, the mod's own keepalive watchdog has already ended
    # it -- that is the point of the watchdog owning the timeout rather than this side.

    environment_thread = threading.Thread(
        target=environment_listener, args=(STOP,), daemon=True
    )
    environment_thread.start()

    # No startup request to match: this one only ever answers a key press, so
    # there is nothing to pre-fetch and nothing to go stale.
    vehicle_info_thread = threading.Thread(
        target=vehicle_info_listener, args=(STOP,), daemon=True
    )
    vehicle_info_thread.start()
    # Ask for the current level's environment rows, for the same reason the
    # bindings request exists: beamtel may have started after the level loaded.
    _send_env_cmd("REQUEST")

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

    # MCP automation server. Off by default; started here because by this point config,
    # audio, every listener and the console client are all live. A failure to bind must
    # only log -- beamtel still has a user to serve.
    _mcp_stop = None
    if cfg.get("mcp_server_enabled", False):
        try:
            import mcp_server

            mcp_server.init(
                mcp_server.Deps(
                    say_fn=lambda text, interrupt=True: say(
                        text, interrupt, exclude_from_buffer=True, source="mcp"
                    ),
                    get_speech_log_fn=get_speech_log,
                    snapshot_state_fn=_mcp_snapshot_state,
                    load_config_fn=load_config,
                    send_console_command_fn=send_console_command,
                    send_udp_fn=mcp_server.make_udp_sender(logger),
                    press_command_fn=_mcp_press_command,
                    f9_help=_F9_HELP,
                    world_active_fn=_world_is_active,
                    capture_png_fn=capture_scene_png,
                    road_diagnostic_fn=road_diagnostic_control,
                    stop_event=STOP,
                    logger=logger,
                    version="1.0",
                )
            )
            register_console_tap(mcp_server.console_tap)
            _mcp_thread, _mcp_stop = mcp_server.start(
                STOP, port=int(cfg.get("mcp_server_port", 4481))
            )
        except Exception as e:
            logger.error(f"Failed to start MCP server: {e}")

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
            # beamtel owns the 4477/4478 socket pair; the spawner asks through it rather
            # than binding a second listener on 4477.
            request_info_fn=request_vehicle_info,
        )
        _vehicle_spawner.start(STOP)
    except Exception as e:
        logger.error(f"Failed to start vehicle spawner: {e}")

    try:
        telemetry_loop(audio_controller=audio_controller, port=4444, stop_event=STOP)
    finally:
        if HILL_CLIMB_CHALLENGE.is_capturing():
            _send_road_command("CAPTURE_OFF")
        HILL_CLIMB_CHALLENGE.shutdown()
        if ROAD_DIAGNOSTICS.status()["active"]:
            try:
                _send_road_command("DIAG_OFF")
                ROAD_DIAGNOSTICS.stop()
            except Exception as exc:
                logger.error(f"Failed to close road diagnostic recording: {exc}")
        STOP.set()
        audio_controller.stop()
        if _ws_stop:
            _ws_stop()
        # Drop the tap before tearing the server down, so a record arriving mid-shutdown
        # cannot reach a half-stopped collector.
        register_console_tap(None)
        if _mcp_stop:
            _mcp_stop()


# =========================
#  Entry point
# =========================


def _run_update_flow(frame):
    """Startup update check, then release the deferred game launch.

    Imported here rather than at module scope so that a broken or missing
    updater module costs the update check and nothing else -- the gate is opened
    in the failure path too, or beamtel would sit out the full 180 s timeout
    before starting the game.
    """
    try:
        import updater

        updater.run_startup_flow(frame, LaunchGate())
    except Exception as e:
        logger.error(f"Update check failed to run: {e}")
        LaunchGate().allow()


def main():
    # First statement on purpose: the earliest moment this process can observe
    # the key the user is holding while it starts.
    global LAUNCH_SUPPRESSED_BY_SHIFT
    LAUNCH_SUPPRESSED_BY_SHIFT = _shift_held_at_startup()
    if LAUNCH_SUPPRESSED_BY_SHIFT:
        logger.info("Shift held at startup: automatic game launch disabled for this session.")

    app = wx.App(False)
    frame = BeamTelFrame()
    frame.Show()

    engine = threading.Thread(target=_run_engine, name="beamtel-engine", daemon=True)
    engine.start()
    frame._engine_thread = engine

    # Scheduled rather than called: the flow is modal and the event loop is not
    # running yet, so calling it here would put a dialog up in front of a window
    # that cannot pump. The engine thread is already going and is blocked on
    # LAUNCH_GATE, which this releases exactly once.
    wx.CallAfter(_run_update_flow, frame)

    app.MainLoop()

    # Ensure clean shutdown after GUI closes
    # Learn mode goes off explicitly first: the watchdog would end it within HEARTBEAT_TIMEOUT_S
    # anyway, but there is no reason to leave somebody's controls dead for six seconds when we
    # know we are quitting. The watchdog remains the backstop for the ways we do NOT get here.
    if _binding_learn_active:
        _send_binding_learn_cmd("LEARN_OFF")
    STOP.set()


if __name__ == "__main__":
    main()

# --- END OF beamtel.py ---
