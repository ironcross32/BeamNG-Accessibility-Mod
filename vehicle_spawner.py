"""
Accessible Vehicle Spawner — Python side.

Implements a multi-screen virtual-browser modal driven by `keyboard` low-level
hooks. Communicates with the Lua extension `vehicleSpawnerAccessible.lua` over
UDP (data on 4460, commands on 4461) to fetch the vehicle catalog, the active
vehicle list (for "mark a vehicle"), and to issue spawn batches.

Public API:
    init(say_fn, is_focused_fn, logger)
    start(stop_event)         # starts listener thread + installs F11 hotkey
    is_modal_open()           # for callers that want to gate other behavior

The modal is opened with F11 and uses these keys while open (all suppressed):
    Tab        — cycle pages forward (main -> to-be-spawned -> manage -> main)
    Shift+Tab  — cycle pages backward
    Up/Down    — navigate
    Home/End   — jump to first/last entry
    PageUp/Dn  — jump 20 entries
    Left/Right — drill into / back out of configs (on main screen)
    Enter      — confirm / drill in / toggle mark on manage page
    Escape     — back up / close
    Space      — on main/configs/to_spawn: spawn all queued; in filter dialog: toggle box
    Delete     — on to-be-spawned list: remove queued; on manage: delete selected world vehicles
    F          — open filter dialog (main only)
    C          — clear filters (main only)
    G          — open arrangement presets screen (main/to-be-spawned/manage)
    W          — placement wizard (to-be-spawned only)
    R          — reload selected world vehicles (manage); add random vehicle to queue (main/configs)
    V          — set ignition to 0 on every world vehicle (manage only)
    Ctrl+A     — select every world vehicle (manage only)
    Shift+Enter — on configs: queue selected config as a replacement (opens slot picker)
    X          — on to-be-spawned: toggle item between replace-in-place and add new
    F11        — toggle modal

Arrangement screen (opened via G):
    Up/Down    — move between rows: Type / Variant / Spacing / Apply Queue / Arrange Active
    Left/Right — change value of the Type / Variant rows
    Enter      — on Spacing: open the distance editor; on a button row: activate it
    Escape     — return to previous screen

Distance editor (wizard distance screen, and the arrangement Spacing row):
    A 5-digit odometer (0..99999 feet, default 15). Left/Right move the cursor
    between digit positions (wrapping); Up/Down cycle the current digit 0-9
    (wrapping). Each digit change speaks the new value (leading zeros stripped);
    moving the cursor speaks the digit at the new position. Enter confirms.
"""

from __future__ import annotations

import json
import queue
import random
import socket
import threading
from typing import Any, Callable

try:
    import keyboard  # type: ignore
    _KEYBOARD_OK = True
except Exception:
    keyboard = None  # type: ignore
    _KEYBOARD_OK = False


# =============================================================================
#  Configuration
# =============================================================================

DATA_PORT = 4460   # receive from Lua
CMD_PORT  = 4461   # send to Lua

SIDES = ["front", "back", "left", "right"]
SIDE_PHRASES = {
    "front": "in front of",
    "back":  "behind",
    "left":  "to the left of",
    "right": "to the right of",
}

# Filter categories — pulled from catalog metadata fields.
# "police" is a synthetic boolean derived in Lua from each config's "Config Type"
# field (info_<config>.json). A vehicle is flagged police if any of its configs
# has Config Type == "Police". The field is only populated ("Yes") on police
# vehicles, so checking it filters down to police-equipped vehicles only.
FILTER_CATEGORIES = [
    ("police",     "Police"),
    ("type",       "Type"),
    ("brand",      "Brand"),
    ("bodyStyle",  "Body style"),
    ("drivetrain", "Drivetrain"),
    ("propulsion", "Propulsion"),
    ("country",    "Country"),
]

# (type_key, display_label, [(variant_key, variant_label), ...])
ARRANGE_TYPES: list[tuple[str, str, list[tuple[str, str]]]] = [
    ("line",          "Line",          [("start", "Anchor at front"), ("end", "Anchor at back"), ("middle", "Anchor in middle")]),
    ("side_by_side",  "Side by side",  [("left",  "Anchor at left"),  ("right", "Anchor at right"), ("middle", "Anchor in middle")]),
    ("two_columns",   "Two columns",   [("front", "Anchor at front"), ("middle", "Anchor in middle"), ("back", "Anchor at back")]),
    ("three_columns", "Three columns", [("front", "Anchor at front"), ("middle", "Anchor in middle"), ("back", "Anchor at back")]),
    ("boxed_in",      "Boxed in",      [("middle", "Anchor in center")]),
]


# =============================================================================
#  Module-level state
# =============================================================================

_say: Callable[..., None] | None = None
_is_focused: Callable[[], bool] | None = None
_close_others: Callable[[], None] | None = None
_log = None

_listener_sock: socket.socket | None = None
_listener_thread: threading.Thread | None = None
_stop_event: threading.Event | None = None

# Persistent UDP send socket — created once, reused, to avoid per-send socket
# construction on the keyboard hook callback path (creating a socket per call
# can take milliseconds on Windows and contributes to hook timeout leaks).
_cmd_sock: socket.socket | None = None
_cmd_sock_lock = threading.Lock()

# Worker thread + queue. Every keyboard hook handler enqueues here and returns
# immediately; the worker runs the actual logic. This keeps the keyboard
# library's hook/listener path free of any blocking work (UDP, speech, waits)
# so suppressed keys (F11, Tab, etc.) don't leak through to the game.
_event_queue: "queue.Queue[tuple[Callable[..., None], Any] | None]" = queue.Queue()
_worker_thread: threading.Thread | None = None

_state_lock = threading.RLock()

# Slot getter callback — injected by beamtel.py via init()
_get_slots_fn: "Callable[[], dict] | None" = None

# Catalog (received from Lua)
_catalog: list[dict[str, Any]] = []
_catalog_building: list[dict[str, Any]] = []
_catalog_expected = 0
_catalog_ready = threading.Event()

# Active vehicle list for "mark a vehicle"
_active_vehicles: list[tuple[int, str]] = []   # (vehId, modelName)
_active_vehicles_event = threading.Event()

# Modal state
_modal_open = False
_hook_handles: list = []

# Filter state: { category_key: set(values_selected) } — empty set means "no filter"
_filters: dict[str, set[str]] = {}
_filter_draft: dict[str, set[str]] | None = None   # snapshot used while in filter dialog

# Per-screen cursors
_screen = "main"        # main | configs | to_spawn | manage | filter | side | distance | ref | mark_picker | replace_slot | arrange | spacing_edit
_idx_main = 0
_idx_configs = 0
_idx_to_spawn = 0
_idx_filter = 0
_idx_side = 0
_idx_distance = 0
_idx_ref = 0
_idx_mark = 0
_idx_manage = 0

# Manage screen state
_manage_vehicles: list[tuple[int, str]] = []   # snapshot at last refresh
_manage_selected: set[int] = set()             # vehicle ids selected

# Currently drilled-into vehicle on main screen
_drill_vehicle: dict[str, Any] | None = None

# To-be-spawned queue
# Each item: { "model": str, "config": str, "displayName": str,
#              "side": str|None, "distFt": int|None,
#              "refMode": "vehicle"|None, "refVehId": int|None }
_to_spawn: list[dict[str, Any]] = []

# Placement wizard scratch state
_wizard_target_idx: int | None = None     # which item in _to_spawn is being configured
_wizard_side: str | None = None
_wizard_distance: int | None = None

# When in mark_picker, what we'll write the marked id back into
_mark_target_idx: int | None = None

# Replace-in-place state
_idx_replace_slot = 0
_pending_replace_item: "dict[str, Any] | None" = None   # item assembled on configs screen, awaiting slot pick
_replace_editing_idx: "int | None" = None               # to_spawn index being reassigned via X key

# When set, the next ACTIVE_VEHICLES packet should re-speak the manage cursor
# (used when entering the manage page — we can't block the keyboard hook
# thread waiting for the response, so we announce after data arrives).
_pending_manage_announce: bool = False

# Arrangement screen state
_arrange_type_idx: int = 0
_arrange_variant_idx: int = 0
_idx_arrange: int = 0               # cursor over 5 rows: type/variant/spacing/queue_btn/active_btn
_arrange_return_screen: str = "main"
_player_veh_id: int | None = None   # updated from PLAYER_VEH_ID: Lua response


# =============================================================================
#  Utilities
# =============================================================================

class _DigitField:
    """Five-digit odometer-style numeric editor for distances in feet.

    Left/Right move the cursor between digit positions (wrapping at the ends);
    Up/Down cycle the digit under the cursor through 0-9 (wrapping). Represents
    values from 0 to 99999 feet. Position 0 is the most significant (leftmost)
    digit.
    """
    NDIGITS = 5

    def __init__(self, value: int = 15):
        self.cursor = 0
        self.set_value(value)

    def set_value(self, value: int):
        try:
            value = int(value)
        except (TypeError, ValueError):
            value = 0
        value = max(0, min(value, 10 ** self.NDIGITS - 1))
        s = str(value).rjust(self.NDIGITS, "0")
        self.digits = [int(c) for c in s]

    def value(self) -> int:
        n = 0
        for d in self.digits:
            n = n * 10 + d
        return n

    def move_left(self):
        self.cursor = (self.cursor - 1) % self.NDIGITS

    def move_right(self):
        self.cursor = (self.cursor + 1) % self.NDIGITS

    def home(self):
        self.cursor = 0

    def end(self):
        self.cursor = self.NDIGITS - 1

    def inc(self):
        self.digits[self.cursor] = (self.digits[self.cursor] + 1) % 10

    def dec(self):
        self.digits[self.cursor] = (self.digits[self.cursor] - 1) % 10

    def current_digit(self) -> int:
        return self.digits[self.cursor]

    def spoken_value(self) -> str:
        # str(int) drops leading zeros so the screen reader pronounces the
        # number naturally ("150 feet", not "zero zero one five zero feet").
        return str(self.value())


# Reused digit editors. The wizard distance is seeded from the queued item each
# time the screen is entered; the arrangement spacing field persists its value
# across openings so the user's last spacing choice is remembered.
_distance_field = _DigitField(15)
_spacing_field = _DigitField(15)


def _active_digit_field() -> _DigitField:
    return _spacing_field if _screen == "spacing_edit" else _distance_field


def _say_safe(text: str):
    if _say is not None:
        try:
            _say(text, interrupt=True, exclude_from_buffer=True)
        except Exception:
            pass


def _get_slots() -> dict:
    if _get_slots_fn is not None:
        try:
            return _get_slots_fn()
        except Exception:
            pass
    return {}


def _logi(msg: str):
    if _log is not None:
        try:
            _log.info(msg)
        except Exception:
            pass


def _logw(msg: str):
    if _log is not None:
        try:
            _log.warning(msg)
        except Exception:
            pass


def _send_cmd(cmd: str):
    global _cmd_sock
    try:
        with _cmd_sock_lock:
            if _cmd_sock is None:
                _cmd_sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
            _cmd_sock.sendto(cmd.encode("utf-8"), ("127.0.0.1", CMD_PORT))
    except Exception as e:
        _logw(f"vehicle_spawner: failed to send command: {e}")
        try:
            if _cmd_sock is not None:
                _cmd_sock.close()
        except Exception:
            pass
        _cmd_sock = None


# =============================================================================
#  UDP Listener
# =============================================================================

def _listener_loop(stop_event: threading.Event):
    global _listener_sock, _catalog, _catalog_building, _catalog_expected
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        try:
            sock.setsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF, 1024 * 1024)
        except Exception:
            pass
        sock.bind(("127.0.0.1", DATA_PORT))
        sock.settimeout(0.2)
        _listener_sock = sock
        _logi(f"Vehicle spawner listener started on port {DATA_PORT}")
        while not stop_event.is_set():
            try:
                data, _ = sock.recvfrom(65536)
            except socket.timeout:
                continue
            except OSError:
                if stop_event.is_set():
                    break
                continue
            try:
                text = data.decode("utf-8", errors="replace").strip()
            except Exception:
                continue
            _handle_packet(text)
    finally:
        try:
            sock.close()
        except Exception:
            pass
        _logi("Vehicle spawner listener stopped.")


def _handle_packet(text: str):
    global _catalog, _catalog_building, _catalog_expected
    global _active_vehicles
    if not text:
        return

    if text.startswith("CATALOG_BEGIN:"):
        try:
            n = int(text.split(":", 1)[1])
        except ValueError:
            n = 0
        with _state_lock:
            _catalog_building = []
            _catalog_expected = n
            _catalog_ready.clear()
        return

    if text.startswith("CATALOG_ITEM:"):
        try:
            obj = json.loads(text[len("CATALOG_ITEM:"):])
        except Exception as e:
            _logw(f"vehicle_spawner: bad catalog item: {e}")
            return
        # Derive the synthetic "police" filter field. Lua sets hasPolice when
        # any config's "Config Type" is "Police". Populating the field only on
        # police vehicles makes the standard filter pipeline (which excludes
        # vehicles whose value isn't in the allowed set) work as a Police-only
        # filter when the user checks "Police: Yes".
        if obj.get("hasPolice"):
            obj["police"] = "Yes"
        with _state_lock:
            _catalog_building.append(obj)
        return

    if text == "CATALOG_END":
        with _state_lock:
            # Sort: vehicles (Type != "Prop") alphabetically first, then props alphabetically.
            # Use the same display-name logic as _vehicle_summary so that vehicles like
            # ETK "800-Series" sort under E (not 8).
            def _sort_key(v):
                is_prop = 1 if (v.get("type") or "").lower() == "prop" else 0
                name = v.get("name") or v.get("model") or ""
                brand = v.get("brand") or ""
                if brand and brand.lower() not in name.lower():
                    display = f"{brand} {name}"
                else:
                    display = name
                return (is_prop, display.lower())
            _catalog = sorted(_catalog_building, key=_sort_key)
            _catalog_building = []
            _catalog_ready.set()
        _logi(f"Vehicle spawner catalog received: {len(_catalog)} entries")
        return

    if text.startswith("CATALOG_ERR:"):
        _logw(f"vehicle_spawner: catalog error: {text[len('CATALOG_ERR:'):]}")
        return

    if text.startswith("ACTIVE_VEHICLES:"):
        payload = text[len("ACTIVE_VEHICLES:"):]
        entries = []
        if payload:
            for part in payload.split(";"):
                if not part:
                    continue
                pieces = part.split(":", 1)
                if len(pieces) == 2:
                    try:
                        vid = int(pieces[0])
                        entries.append((vid, pieces[1]))
                    except ValueError:
                        pass
        global _manage_vehicles, _manage_selected, _idx_manage
        with _state_lock:
            _active_vehicles = entries
            # Refresh manage page snapshot too. Drop selections for vehicles no longer present.
            _manage_vehicles = list(entries)
            present_ids = {vid for vid, _ in entries}
            _manage_selected = {vid for vid in _manage_selected if vid in present_ids}
            if _manage_vehicles:
                if _idx_manage >= len(_manage_vehicles):
                    _idx_manage = len(_manage_vehicles) - 1
            else:
                _idx_manage = 0
        _active_vehicles_event.set()
        global _pending_manage_announce
        if _pending_manage_announce and _screen == "manage":
            _pending_manage_announce = False
            _speak_current()
        return

    if text.startswith("RELOAD_DONE:"):
        body = text[len("RELOAD_DONE:"):]
        parts = body.split(",")
        if len(parts) == 2:
            _say_safe(f"Reloaded {parts[0]}, failed {parts[1]}.")
        _send_cmd("REQUEST_ACTIVE_VEHICLES")
        return

    if text.startswith("REMOVE_DONE:"):
        body = text[len("REMOVE_DONE:"):]
        parts = body.split(",")
        if len(parts) == 2:
            _say_safe(f"Removed {parts[0]}, failed {parts[1]}.")
        _send_cmd("REQUEST_ACTIVE_VEHICLES")
        return

    if text.startswith("IGNITION_OFF_DONE:"):
        try:
            n = int(text[len("IGNITION_OFF_DONE:"):])
        except ValueError:
            n = 0
        _say_safe(f"Ignition off on {n} {'vehicle' if n == 1 else 'vehicles'}.")
        return

    if text.startswith("SPAWN_OK:"):
        try:
            n = int(text[len("SPAWN_OK:"):])
        except ValueError:
            n = 0
        _say_safe(f"Spawned {n} {'vehicle' if n == 1 else 'vehicles'}.")
        return

    if text.startswith("SPAWN_PARTIAL:"):
        body = text[len("SPAWN_PARTIAL:"):]
        # body: "<successes>,<fails>,<msg>"
        parts = body.split(",", 2)
        if len(parts) >= 2:
            _say_safe(f"Spawned {parts[0]}, failed {parts[1]}.")
        else:
            _say_safe("Partial spawn.")
        return

    if text.startswith("SPAWN_ERR:"):
        _say_safe(f"Spawn error: {text[len('SPAWN_ERR:'):]}")
        return

    if text.startswith("ARRANGE_OK:"):
        try:
            n = int(text[len("ARRANGE_OK:"):])
        except ValueError:
            n = 0
        _say_safe(f"Arranged {n} {'vehicle' if n == 1 else 'vehicles'}.")
        return

    if text.startswith("ARRANGE_ERR:"):
        _say_safe(f"Arrangement error: {text[len('ARRANGE_ERR:'):]}")
        return

    if text.startswith("PLAYER_VEH_ID:"):
        global _player_veh_id
        try:
            vid = int(text[len("PLAYER_VEH_ID:"):])
            _player_veh_id = vid if vid >= 0 else None
        except ValueError:
            pass
        return


# =============================================================================
#  Catalog filtering
# =============================================================================

def _filtered_catalog() -> list[dict[str, Any]]:
    with _state_lock:
        if not _filters:
            return list(_catalog)
        # When the Police filter is active, also restrict the per-vehicle
        # configs list to police configs only — otherwise drilling into a
        # multi-config model (Bolide, Scintilla, Autobello, etc.) would show
        # every config and look like the filter wasn't applied. Each config
        # carries a `configType` field populated from the game's "Config Type"
        # metadata.
        police_only = "police" in _filters and _filters["police"]
        out = []
        for v in _catalog:
            keep = True
            for key, allowed in _filters.items():
                if not allowed:
                    continue
                if v.get(key, "") not in allowed:
                    keep = False
                    break
            if not keep:
                continue
            if police_only:
                police_cfgs = [c for c in v.get("configs", []) if c.get("configType") == "Police"]
                if not police_cfgs:
                    # Safety net: vehicle was flagged hasPolice in Lua but no
                    # config carries Config Type == Police. Skip it.
                    continue
                v = dict(v)
                v["configs"] = police_cfgs
            out.append(v)
        return out


def _filter_value_options(category: str) -> list[str]:
    """Distinct non-empty values for a filter category, sorted."""
    seen = set()
    for v in _catalog:
        val = v.get(category, "")
        if val:
            seen.add(val)
    return sorted(seen, key=lambda s: s.lower())


def _filter_dialog_lines() -> list[tuple[str, str, str]]:
    """Flat list of (category_key, value, label_with_state) for the filter dialog."""
    rows: list[tuple[str, str, str]] = []
    draft = _filter_draft or {}
    for cat_key, cat_label in FILTER_CATEGORIES:
        values = _filter_value_options(cat_key)
        if not values:
            continue
        selected = draft.get(cat_key, set())
        for val in values:
            checked = val in selected
            mark = "checked" if checked else "unchecked"
            rows.append((cat_key, val, f"{cat_label}: {val}, {mark}"))
    return rows


# =============================================================================
#  Arrangement helpers
# =============================================================================

import math as _math


def _compute_arrangement_offsets(arr_type: str, variant: str, n_others: int, spacing_m: float) -> list[tuple[float, float]]:
    """Return (fwd_m, right_m) offsets for each of the n_others vehicles relative to the anchor."""
    s = spacing_m
    N = n_others
    offsets: list[tuple[float, float]] = []

    if arr_type == "line":
        if variant == "start":
            for k in range(1, N + 1):
                offsets.append((-k * s, 0.0))
        elif variant == "end":
            for k in range(1, N + 1):
                offsets.append((k * s, 0.0))
        else:  # middle
            n_front = N // 2
            n_back = N - n_front
            for k in range(1, n_front + 1):
                offsets.append((k * s, 0.0))
            for k in range(1, n_back + 1):
                offsets.append((-k * s, 0.0))

    elif arr_type == "side_by_side":
        if variant == "left":
            for k in range(1, N + 1):
                offsets.append((0.0, k * s))
        elif variant == "right":
            for k in range(1, N + 1):
                offsets.append((0.0, -k * s))
        else:  # middle
            n_right = N // 2
            n_left = N - n_right
            for k in range(1, n_right + 1):
                offsets.append((0.0, k * s))
            for k in range(1, n_left + 1):
                offsets.append((0.0, -k * s))

    elif arr_type == "two_columns":
        n_total = N + 1
        n_rows = _math.ceil(n_total / 2)
        if variant == "front":
            anchor_row = 0
        elif variant == "back":
            anchor_row = n_rows - 1
        else:
            anchor_row = n_rows // 2
        anchor_col = 0
        for row in range(n_rows):
            for col in range(2):
                if row == anchor_row and col == anchor_col:
                    continue
                fwd_m = (anchor_row - row) * s
                right_m = (col * 2 - 1) * (s / 2)
                offsets.append((fwd_m, right_m))
                if len(offsets) >= N:
                    break
            if len(offsets) >= N:
                break

    elif arr_type == "three_columns":
        n_total = N + 1
        n_rows = _math.ceil(n_total / 3)
        if variant == "front":
            anchor_row = 0
        elif variant == "back":
            anchor_row = n_rows - 1
        else:
            anchor_row = n_rows // 2
        anchor_col = 1
        for row in range(n_rows):
            for col in range(3):
                if row == anchor_row and col == anchor_col:
                    continue
                fwd_m = (anchor_row - row) * s
                right_m = (col - 1) * s
                offsets.append((fwd_m, right_m))
                if len(offsets) >= N:
                    break
            if len(offsets) >= N:
                break

    elif arr_type == "boxed_in":
        ring = 1
        while len(offsets) < N:
            r = ring * s
            ring_slots = [
                (r, 0.0), (-r, 0.0), (0.0, -r), (0.0, r),
                (r, r),   (r, -r),   (-r, r),    (-r, -r),
            ]
            for sl in ring_slots:
                offsets.append(sl)
                if len(offsets) >= N:
                    break
            ring += 1
            if ring > 20:
                break

    return offsets[:N]


def _arrange_validate() -> tuple[bool, bool, str]:
    """Return (queue_ok, active_ok, status_msg) for the current arrangement selection."""
    type_key = ARRANGE_TYPES[_arrange_type_idx][0]
    with _state_lock:
        n_queue = len(_to_spawn)
        n_active = len(_active_vehicles)

    min_total = 5 if type_key == "boxed_in" else 2

    queue_ok = n_queue >= min_total

    if type_key == "boxed_in":
        active_ok = n_active >= min_total and _player_veh_id is not None
    else:
        active_ok = n_active >= min_total

    parts = [f"Queue: {n_queue}, Active: {n_active}."]
    if type_key == "boxed_in":
        if n_queue < min_total or n_active < min_total:
            parts.append(f"Boxed in needs at least {min_total} vehicles (1 anchor + 4 others).")
        if n_active >= min_total and _player_veh_id is None:
            parts.append("Boxed in requires a current vehicle for active mode.")
    elif n_queue < 2 and n_active < 2:
        parts.append("Need at least 2 vehicles.")

    return queue_ok, active_ok, " ".join(parts)


def _speak_arrange_item(idx: int):
    type_key, type_label, variants = ARRANGE_TYPES[_arrange_type_idx]
    variant_key, variant_label = variants[min(_arrange_variant_idx, len(variants) - 1)]
    spacing_ft = _spacing_field.value()
    queue_ok, active_ok, status = _arrange_validate()

    _ARRANGE_TOTAL = 5
    if idx == 0:
        content = f"Arrangement type: {type_label}"
    elif idx == 1:
        content = f"Variant: {variant_label}"
    elif idx == 2:
        content = f"Spacing: {spacing_ft} feet, press enter to edit"
    elif idx == 3:
        content = "Apply to spawn queue"
        if not queue_ok:
            content += ", unavailable"
    else:
        content = "Arrange active vehicles"
        if not active_ok:
            content += ", unavailable"

    if idx in (3, 4):
        _speak_position(idx, _ARRANGE_TOTAL, f"{content}. {status}")
    else:
        _speak_position(idx, _ARRANGE_TOTAL, content)


# =============================================================================
#  Speech helpers
# =============================================================================

def _vehicle_summary(v: dict[str, Any]) -> str:
    parts = []
    name = v.get("name") or v.get("model") or "unknown"
    brand = v.get("brand") or ""
    if brand and brand.lower() not in name.lower():
        parts.append(f"{brand} {name}")
    else:
        parts.append(name)
    ncfg = len(v.get("configs", []))
    if ncfg > 1:
        parts.append(f"{ncfg} configurations")
    return ", ".join(parts)


def _to_spawn_summary(item: dict[str, Any]) -> str:
    base = item.get("displayName") or item.get("model") or "vehicle"
    if item.get("replaceVehId") is not None:
        slot = item.get("replaceSlot")
        replaced = item.get("replaceVehName", "")
        target = f"slot {slot} {replaced}".strip() if slot and replaced else "vehicle"
        return f"Replace {target} with {base}"
    side = item.get("side")
    dist = item.get("distFt")
    ref = item.get("refMode")
    if side and dist:
        if ref == "auto" or ref == "prev":
            ref_text = "previous in queue"
        elif ref == "next":
            ref_text = "next in queue"
        elif ref == "vehicle" and item.get("refVehId") is not None:
            ref_text = f"marked vehicle {item.get('refVehId')}"
        else:
            ref_text = "current vehicle"
        return f"{base}, {SIDE_PHRASES.get(side, side)} {ref_text}, {dist} feet"
    return f"{base}, no placement set"


def _ref_options() -> list[tuple[str, str]]:
    """(label, key) options on the ref screen — filtered by the wizard target's queue position."""
    opts = [("current vehicle", "current"), ("mark a vehicle", "mark")]
    idx = _wizard_target_idx
    if idx is not None:
        if idx > 0:
            opts.append(("previous in queue", "prev"))
        if idx < len(_to_spawn) - 1:
            opts.append(("next in queue", "next"))
    return opts


_pending_prefix: str | None = None


def _consume_prefix() -> str:
    global _pending_prefix
    p = _pending_prefix or ""
    _pending_prefix = None
    return p


def _say_with_prefix(msg: str):
    prefix = _consume_prefix()
    _say_safe(f"{prefix}. {msg}" if prefix else msg)


def _speak_position(idx: int, total: int, content: str):
    if total <= 0:
        _say_with_prefix("Empty list.")
        return
    _say_with_prefix(f"{content}. {idx + 1} of {total}.")


# =============================================================================
#  Screen rendering — speak the current cursor item
# =============================================================================

def _speak_current():
    global _idx_main, _idx_configs, _idx_to_spawn, _idx_filter
    global _idx_side, _idx_distance, _idx_ref, _idx_mark, _idx_manage

    if _screen == "main":
        items = _filtered_catalog()
        if not items:
            _say_with_prefix("No vehicles match current filters.")
            return
        _idx_main = max(0, min(_idx_main, len(items) - 1))
        _speak_position(_idx_main, len(items), _vehicle_summary(items[_idx_main]))
        return

    if _screen == "configs":
        if not _drill_vehicle:
            _say_with_prefix("No vehicle selected.")
            return
        cfgs = _drill_vehicle.get("configs", [])
        if not cfgs:
            _say_with_prefix("No configurations.")
            return
        _idx_configs = max(0, min(_idx_configs, len(cfgs) - 1))
        cfg = cfgs[_idx_configs]
        label = cfg.get("name") or cfg.get("key") or "default"
        if cfg.get("isDefault"):
            label += ", default"
        _speak_position(_idx_configs, len(cfgs), label)
        return

    if _screen == "to_spawn":
        if not _to_spawn:
            _say_with_prefix("To be spawned list is empty.")
            return
        _idx_to_spawn = max(0, min(_idx_to_spawn, len(_to_spawn) - 1))
        _speak_position(_idx_to_spawn, len(_to_spawn), _to_spawn_summary(_to_spawn[_idx_to_spawn]))
        return

    if _screen == "manage":
        with _state_lock:
            mv = list(_manage_vehicles)
            sel = set(_manage_selected)
        if not mv:
            _say_with_prefix("No vehicles in the world.")
            return
        _idx_manage = max(0, min(_idx_manage, len(mv) - 1))
        vid, name = mv[_idx_manage]
        marker = "selected" if vid in sel else "not selected"
        _speak_position(_idx_manage, len(mv), f"{name} (id {vid}), {marker}")
        return

    if _screen == "filter":
        rows = _filter_dialog_lines()
        if not rows:
            _say_with_prefix("No filter options available.")
            return
        _idx_filter = max(0, min(_idx_filter, len(rows) - 1))
        _speak_position(_idx_filter, len(rows), rows[_idx_filter][2])
        return

    if _screen == "side":
        _idx_side = max(0, min(_idx_side, len(SIDES) - 1))
        _speak_position(_idx_side, len(SIDES), SIDE_PHRASES[SIDES[_idx_side]])
        return

    if _screen in ("distance", "spacing_edit"):
        f = _active_digit_field()
        _say_with_prefix(f"{f.spoken_value()} feet")
        return

    if _screen == "ref":
        opts = _ref_options()
        if not opts:
            _say_with_prefix("No reference options available.")
            return
        _idx_ref = max(0, min(_idx_ref, len(opts) - 1))
        _speak_position(_idx_ref, len(opts), opts[_idx_ref][0])
        return

    if _screen == "mark_picker":
        with _state_lock:
            avs = list(_active_vehicles)
        if not avs:
            _say_with_prefix("No active vehicles to mark.")
            return
        _idx_mark = max(0, min(_idx_mark, len(avs) - 1))
        vid, name = avs[_idx_mark]
        _speak_position(_idx_mark, len(avs), f"{name} (id {vid})")
        return

    if _screen == "replace_slot":
        slots = _get_slots()
        slot_nums = sorted(slots)
        if not slot_nums:
            _say_with_prefix("No vehicles in slots.")
            return
        global _idx_replace_slot
        _idx_replace_slot = max(0, min(_idx_replace_slot, len(slot_nums) - 1))
        sn = slot_nums[_idx_replace_slot]
        info = slots[sn]
        label = f"Slot {sn}: {info['name']}"
        if sn == 1:
            label += ", player vehicle"
        _speak_position(_idx_replace_slot, len(slot_nums), label)
        return

    if _screen == "arrange":
        _speak_arrange_item(_idx_arrange)
        return


def _enter_screen(screen: str, announce: bool = True, header: str | None = None):
    global _screen, _pending_prefix
    _screen = screen
    if announce:
        # Stage the header so _speak_position can combine it with the first item
        # into one utterance (otherwise interrupt=True drops the header).
        _pending_prefix = header
        _speak_current()


# =============================================================================
#  Key handlers
# =============================================================================

def _on_up(event):
    global _idx_main, _idx_configs, _idx_to_spawn, _idx_filter
    global _idx_side, _idx_distance, _idx_ref, _idx_mark, _idx_manage, _idx_replace_slot
    global _idx_arrange
    if _screen == "arrange":
        _idx_arrange = max(0, _idx_arrange - 1)
        _speak_current()
        return
    if _screen in ("distance", "spacing_edit"):
        f = _active_digit_field()
        f.inc()
        _say_safe(f"{f.spoken_value()} feet")
        return
    with _state_lock:
        if _screen == "main":
            _idx_main = max(0, _idx_main - 1)
        elif _screen == "configs":
            _idx_configs = max(0, _idx_configs - 1)
        elif _screen == "to_spawn":
            _idx_to_spawn = max(0, _idx_to_spawn - 1)
        elif _screen == "manage":
            _idx_manage = max(0, _idx_manage - 1)
        elif _screen == "filter":
            _idx_filter = max(0, _idx_filter - 1)
        elif _screen == "side":
            _idx_side = max(0, _idx_side - 1)
        elif _screen == "ref":
            _idx_ref = max(0, _idx_ref - 1)
        elif _screen == "mark_picker":
            _idx_mark = max(0, _idx_mark - 1)
        elif _screen == "replace_slot":
            _idx_replace_slot = max(0, _idx_replace_slot - 1)
    _speak_current()


def _on_down(event):
    global _idx_main, _idx_configs, _idx_to_spawn, _idx_filter
    global _idx_side, _idx_distance, _idx_ref, _idx_mark, _idx_manage, _idx_replace_slot
    global _idx_arrange
    if _screen == "arrange":
        _idx_arrange = min(4, _idx_arrange + 1)
        _speak_current()
        return
    if _screen in ("distance", "spacing_edit"):
        f = _active_digit_field()
        f.dec()
        _say_safe(f"{f.spoken_value()} feet")
        return
    with _state_lock:
        if _screen == "main":
            n = len(_filtered_catalog())
            _idx_main = min(max(0, n - 1), _idx_main + 1)
        elif _screen == "configs":
            n = len(_drill_vehicle.get("configs", [])) if _drill_vehicle else 0
            _idx_configs = min(max(0, n - 1), _idx_configs + 1)
        elif _screen == "to_spawn":
            n = len(_to_spawn)
            _idx_to_spawn = min(max(0, n - 1), _idx_to_spawn + 1)
        elif _screen == "manage":
            n = len(_manage_vehicles)
            _idx_manage = min(max(0, n - 1), _idx_manage + 1)
        elif _screen == "filter":
            n = len(_filter_dialog_lines())
            _idx_filter = min(max(0, n - 1), _idx_filter + 1)
        elif _screen == "side":
            _idx_side = min(len(SIDES) - 1, _idx_side + 1)
        elif _screen == "ref":
            n = len(_ref_options())
            _idx_ref = min(max(0, n - 1), _idx_ref + 1)
        elif _screen == "mark_picker":
            with _state_lock:
                n = len(_active_vehicles)
            _idx_mark = min(max(0, n - 1), _idx_mark + 1)
        elif _screen == "replace_slot":
            n = len(sorted(_get_slots()))
            _idx_replace_slot = min(max(0, n - 1), _idx_replace_slot + 1)
    _speak_current()


def _on_left(event):
    global _arrange_type_idx, _arrange_variant_idx
    if _screen in ("distance", "spacing_edit"):
        f = _active_digit_field()
        f.move_left()
        _say_safe(str(f.current_digit()))
        return
    if _screen == "arrange":
        if _idx_arrange == 0:
            _arrange_type_idx = (_arrange_type_idx - 1) % len(ARRANGE_TYPES)
            _arrange_variant_idx = 0
        elif _idx_arrange == 1:
            variants = ARRANGE_TYPES[_arrange_type_idx][2]
            _arrange_variant_idx = (_arrange_variant_idx - 1) % len(variants)
        _speak_current()
        return
    if _screen == "configs":
        _enter_screen("main")


def _on_right(event):
    global _arrange_type_idx, _arrange_variant_idx
    if _screen in ("distance", "spacing_edit"):
        f = _active_digit_field()
        f.move_right()
        _say_safe(str(f.current_digit()))
        return
    if _screen == "arrange":
        if _idx_arrange == 0:
            _arrange_type_idx = (_arrange_type_idx + 1) % len(ARRANGE_TYPES)
            _arrange_variant_idx = 0
        elif _idx_arrange == 1:
            variants = ARRANGE_TYPES[_arrange_type_idx][2]
            _arrange_variant_idx = (_arrange_variant_idx + 1) % len(variants)
        _speak_current()
        return
    if _screen == "main":
        _drill_into_selected()


def _drill_into_selected():
    global _drill_vehicle, _idx_configs
    items = _filtered_catalog()
    if not items:
        _say_safe("No vehicles.")
        return
    _drill_vehicle = items[_idx_main]
    _idx_configs = 0
    _enter_screen("configs", header=f"{_vehicle_summary(_drill_vehicle)}")


_PAGES = ["main", "to_spawn", "manage"]
_PAGE_HEADERS = {
    "main":     "Vehicle list",
    "to_spawn": "To be spawned",
    "manage":   "Vehicle control",
}


def _shift_pressed() -> bool:
    if not _KEYBOARD_OK:
        return False
    try:
        return bool(keyboard.is_pressed("shift"))
    except Exception:
        return False


def _ctrl_pressed() -> bool:
    if not _KEYBOARD_OK:
        return False
    try:
        return bool(keyboard.is_pressed("ctrl"))
    except Exception:
        return False


def _on_tab(event):
    if _screen not in _PAGES:
        return
    cur = _PAGES.index(_screen)
    direction = -1 if _shift_pressed() else 1
    nxt = _PAGES[(cur + direction) % len(_PAGES)]
    if nxt == "manage":
        global _pending_manage_announce
        _pending_manage_announce = True
        _active_vehicles_event.clear()
        _send_cmd("REQUEST_ACTIVE_VEHICLES")
    _enter_screen(nxt, header=_PAGE_HEADERS[nxt])


def _on_enter(event):
    global _wizard_target_idx, _wizard_side, _wizard_distance
    global _mark_target_idx
    global _pending_replace_item, _replace_editing_idx, _idx_replace_slot
    if _screen == "arrange":
        if _idx_arrange == 2:
            _spacing_field.home()
            _enter_screen("spacing_edit", header="Spacing, in feet")
        elif _idx_arrange == 3:
            _do_arrange_queue()
        elif _idx_arrange == 4:
            _do_arrange_active()
        return

    if _screen == "spacing_edit":
        _enter_screen("arrange")
        return
    if _screen == "main":
        _drill_into_selected()
        return

    if _screen == "configs":
        cfgs = _drill_vehicle.get("configs", []) if _drill_vehicle else []
        if not cfgs:
            return
        cfg = cfgs[_idx_configs]
        v = _drill_vehicle or {}
        display = v.get("name") or v.get("model") or "vehicle"
        cfg_name = cfg.get("name") or cfg.get("key") or ""
        if cfg_name and cfg_name.lower() not in ("default", "(default)"):
            display = f"{display} — {cfg_name}"
        item = {
            "model": v.get("model"),
            "config": cfg.get("key", ""),
            "displayName": display,
            # Default placement: 15 ft right of previous spawn in batch (or player vehicle
            # for the first item, or camera+ground if no player vehicle exists).
            "side": "right",
            "distFt": 15,
            "refMode": "auto",
            "refVehId": None,
            "replaceSlot": None,
            "replaceVehId": None,
            "replaceVehName": None,
        }
        if _shift_pressed():
            slots = _get_slots()
            if not slots:
                _say_safe("No slot data available yet.")
                return
            _pending_replace_item = item
            _idx_replace_slot = 0
            _enter_screen("replace_slot", header="Choose vehicle to replace")
            return
        with _state_lock:
            _to_spawn.append(item)
        _say_safe(f"Added {display}. {len(_to_spawn)} queued.")
        return

    if _screen == "replace_slot":
        slots = _get_slots()
        slot_nums = sorted(slots)
        if not slot_nums:
            return
        _idx_replace_slot = max(0, min(_idx_replace_slot, len(slot_nums) - 1))
        sn = slot_nums[_idx_replace_slot]
        info = slots[sn]
        if _pending_replace_item is not None:
            item = dict(_pending_replace_item)
            item.update(replaceSlot=sn, replaceVehId=info["id"], replaceVehName=info["name"])
            with _state_lock:
                _to_spawn.append(item)
            _say_safe(f"Will replace slot {sn} {info['name']} with {item['displayName']}. {len(_to_spawn)} queued.")
            _pending_replace_item = None
            _enter_screen("configs")
        elif _replace_editing_idx is not None:
            with _state_lock:
                if 0 <= _replace_editing_idx < len(_to_spawn):
                    _to_spawn[_replace_editing_idx].update(
                        replaceSlot=sn, replaceVehId=info["id"], replaceVehName=info["name"])
            _say_safe(f"Will replace slot {sn} {info['name']}.")
            _replace_editing_idx = None
            _enter_screen("to_spawn")
        return

    if _screen == "filter":
        # ENTER applies the draft and exits the dialog.
        global _filters, _filter_draft
        with _state_lock:
            _filters = {k: set(v) for k, v in (_filter_draft or {}).items() if v}
            _filter_draft = None
        _say_safe("Filters applied.")
        _enter_screen("main")
        return

    if _screen == "side":
        _wizard_side = SIDES[_idx_side]
        # Seed the digit editor from the item's current distance (default 15 ft).
        cur = 15
        with _state_lock:
            if _wizard_target_idx is not None and 0 <= _wizard_target_idx < len(_to_spawn):
                cur = _to_spawn[_wizard_target_idx].get("distFt") or 15
        _distance_field.set_value(cur)
        _distance_field.home()
        _enter_screen("distance", header="Distance from reference, in feet")
        return

    if _screen == "distance":
        _wizard_distance = _distance_field.value()
        _enter_screen("ref", header="Reference vehicle")
        return

    if _screen == "ref":
        opts = _ref_options()
        if not opts:
            return
        _, key = opts[_idx_ref]
        if key == "current":
            _commit_wizard(ref_mode="vehicle", ref_veh_id=None)
        elif key == "mark":
            _request_active_vehicles()
            _mark_target_idx = _wizard_target_idx
            _enter_screen("mark_picker", header="Mark a vehicle")
        elif key == "prev":
            _commit_wizard(ref_mode="prev", ref_veh_id=None, ref_name="previous in queue")
        elif key == "next":
            _commit_wizard(ref_mode="next", ref_veh_id=None, ref_name="next in queue")
        return

    if _screen == "manage":
        with _state_lock:
            mv = list(_manage_vehicles)
            if not mv:
                return
            vid, name = mv[_idx_manage]
            if vid in _manage_selected:
                _manage_selected.discard(vid)
                _say_safe(f"{name} unmarked.")
            else:
                _manage_selected.add(vid)
                _say_safe(f"{name} marked.")
        return

    if _screen == "mark_picker":
        with _state_lock:
            avs = list(_active_vehicles)
        if not avs:
            return
        vid, name = avs[_idx_mark]
        # Reject self-reference: the to-be-spawned item is not yet a real vehicle, so the only
        # collision is if multiple queued items try to anchor to themselves; not possible here
        # since marks are real in-world vehicles. Still, we guard against marking the same id
        # twice for the same item if re-entering the wizard.
        _commit_wizard(ref_mode="vehicle", ref_veh_id=vid, ref_name=name)
        return


def _commit_wizard(ref_mode: str, ref_veh_id: int | None, ref_name: str | None = None):
    global _wizard_target_idx, _wizard_side, _wizard_distance, _mark_target_idx
    if _wizard_target_idx is None or _wizard_side is None or _wizard_distance is None:
        _say_safe("Placement cancelled.")
        _enter_screen("to_spawn")
        return
    with _state_lock:
        if 0 <= _wizard_target_idx < len(_to_spawn):
            item = _to_spawn[_wizard_target_idx]
            item["side"] = _wizard_side
            item["distFt"] = _wizard_distance
            item["refMode"] = ref_mode
            item["refVehId"] = ref_veh_id
    if ref_mode in ("prev", "next"):
        ref_text = ref_name or ref_mode
    elif ref_veh_id is not None:
        ref_text = f"marked {ref_name or ref_veh_id}"
    else:
        ref_text = "current vehicle"
    _say_safe(f"Placement set: {SIDE_PHRASES[_wizard_side]} {ref_text}, {_wizard_distance} feet.")
    _wizard_target_idx = None
    _wizard_side = None
    _wizard_distance = None
    _mark_target_idx = None
    _enter_screen("to_spawn")


def _on_escape(event):
    global _filter_draft, _drill_vehicle, _pending_replace_item, _replace_editing_idx
    if _screen == "spacing_edit":
        _enter_screen("arrange")
        return
    if _screen == "arrange":
        _enter_screen(_arrange_return_screen)
        return
    if _screen == "configs":
        _drill_vehicle = None
        _enter_screen("main")
        return
    if _screen == "filter":
        with _state_lock:
            _filter_draft = None
        _say_safe("Filters cancelled.")
        _enter_screen("main")
        return
    if _screen in ("side", "distance", "ref", "mark_picker"):
        _say_safe("Placement cancelled.")
        _enter_screen("to_spawn")
        return
    if _screen == "replace_slot":
        came_from = "to_spawn" if _replace_editing_idx is not None else "configs"
        _pending_replace_item = None
        _replace_editing_idx = None
        _say_safe("Replacement cancelled.")
        _enter_screen(came_from)
        return
    # Top-level screens: close modal
    _close_modal()


def _on_space(event):
    global _filter_draft
    if _screen == "filter":
        rows = _filter_dialog_lines()
        if not rows:
            return
        cat, val, _ = rows[_idx_filter]
        with _state_lock:
            if _filter_draft is None:
                _filter_draft = {k: set(v) for k, v in _filters.items()}
            sel = _filter_draft.setdefault(cat, set())
            if val in sel:
                sel.remove(val)
                _say_safe("Unchecked.")
            else:
                sel.add(val)
                _say_safe("Checked.")
        return

    # On main, configs, or to_spawn: spawn all queued
    if _screen in ("main", "configs", "to_spawn"):
        _do_spawn_all()


def _on_delete(event):
    global _idx_to_spawn
    if _screen == "to_spawn":
        with _state_lock:
            if not _to_spawn:
                return
            if 0 <= _idx_to_spawn < len(_to_spawn):
                removed = _to_spawn.pop(_idx_to_spawn)
                if _idx_to_spawn >= len(_to_spawn):
                    _idx_to_spawn = max(0, len(_to_spawn) - 1)
                _say_safe(f"Removed {removed.get('displayName', 'vehicle')}.")
        _speak_current()
        return

    if _screen == "manage":
        with _state_lock:
            ids = sorted(_manage_selected)
        if not ids:
            _say_safe("No vehicles selected.")
            return
        _say_safe(f"Removing {len(ids)} {'vehicle' if len(ids) == 1 else 'vehicles'}.")
        _send_cmd("REMOVE:" + ",".join(str(i) for i in ids))
        with _state_lock:
            _manage_selected.clear()
        return


def _on_r(event):
    if _screen == "manage":
        with _state_lock:
            ids = sorted(_manage_selected)
        if not ids:
            _say_safe("No vehicles selected.")
            return
        _say_safe(f"Reloading {len(ids)} {'vehicle' if len(ids) == 1 else 'vehicles'}.")
        _send_cmd("RELOAD:" + ",".join(str(i) for i in ids))
        return

    if _screen == "main":
        items = _filtered_catalog()
        if not items:
            _say_safe("No vehicles available.")
            return
        v = random.choice(items)
        cfgs = v.get("configs", [])
        if not cfgs:
            _say_safe("Selected vehicle has no configurations.")
            return
        cfg = random.choice(cfgs)
        _add_random_config(v, cfg)
        return

    if _screen == "configs":
        if not _drill_vehicle:
            _say_safe("No vehicle selected.")
            return
        cfgs = _drill_vehicle.get("configs", [])
        if not cfgs:
            _say_safe("No configurations available.")
            return
        cfg = random.choice(cfgs)
        _add_random_config(_drill_vehicle, cfg)
        return


def _add_random_config(v: dict[str, Any], cfg: dict[str, Any]):
    display = v.get("name") or v.get("model") or "vehicle"
    cfg_name = cfg.get("name") or cfg.get("key") or ""
    if cfg_name and cfg_name.lower() not in ("default", "(default)"):
        display = f"{display} — {cfg_name}"
    item = {
        "model": v.get("model"),
        "config": cfg.get("key", ""),
        "displayName": display,
        "side": "right",
        "distFt": 15,
        "refMode": "auto",
        "refVehId": None,
    }
    with _state_lock:
        _to_spawn.append(item)
    _say_safe(f"Added random {display}. {len(_to_spawn)} queued.")


def _on_v(event):
    if _screen != "manage":
        return
    _send_cmd("IGNITION_OFF")
    _say_safe("Ignition off requested.")


def _on_a(event):
    # CTRL+A on manage: select all. Bare A: ignored.
    if _screen != "manage":
        return
    if not _ctrl_pressed():
        return
    with _state_lock:
        _manage_selected.clear()
        for vid, _name in _manage_vehicles:
            _manage_selected.add(vid)
        n = len(_manage_selected)
    _say_safe(f"Selected all, {n} {'vehicle' if n == 1 else 'vehicles'}.")


def _on_f(event):
    global _filter_draft, _idx_filter
    if _screen != "main":
        return
    with _state_lock:
        _filter_draft = {k: set(v) for k, v in _filters.items()}
    _idx_filter = 0
    _enter_screen("filter", header="Filters")


def _on_c(event):
    global _filters
    if _screen != "main":
        return
    with _state_lock:
        _filters = {}
    _say_safe("Filters cleared.")
    _speak_current()


_PAGE_JUMP = 20


def _on_home(event):
    global _idx_main, _idx_configs, _idx_to_spawn, _idx_filter
    global _idx_side, _idx_distance, _idx_ref, _idx_mark, _idx_manage, _idx_replace_slot
    global _idx_arrange
    if _screen == "arrange":
        _idx_arrange = 0
        _speak_current()
        return
    if _screen in ("distance", "spacing_edit"):
        f = _active_digit_field()
        f.home()
        _say_safe(str(f.current_digit()))
        return
    with _state_lock:
        if _screen == "main":
            _idx_main = 0
        elif _screen == "configs":
            _idx_configs = 0
        elif _screen == "to_spawn":
            _idx_to_spawn = 0
        elif _screen == "manage":
            _idx_manage = 0
        elif _screen == "filter":
            _idx_filter = 0
        elif _screen == "replace_slot":
            _idx_replace_slot = 0
    _speak_current()


def _on_end(event):
    global _idx_main, _idx_configs, _idx_to_spawn, _idx_filter
    global _idx_side, _idx_distance, _idx_ref, _idx_mark, _idx_manage, _idx_replace_slot
    global _idx_arrange
    if _screen == "arrange":
        _idx_arrange = 4
        _speak_current()
        return
    if _screen in ("distance", "spacing_edit"):
        f = _active_digit_field()
        f.end()
        _say_safe(str(f.current_digit()))
        return
    with _state_lock:
        if _screen == "main":
            _idx_main = max(0, len(_filtered_catalog()) - 1)
        elif _screen == "configs":
            n = len(_drill_vehicle.get("configs", [])) if _drill_vehicle else 0
            _idx_configs = max(0, n - 1)
        elif _screen == "to_spawn":
            _idx_to_spawn = max(0, len(_to_spawn) - 1)
        elif _screen == "manage":
            _idx_manage = max(0, len(_manage_vehicles) - 1)
        elif _screen == "filter":
            _idx_filter = max(0, len(_filter_dialog_lines()) - 1)
        elif _screen == "replace_slot":
            _idx_replace_slot = max(0, len(sorted(_get_slots())) - 1)
    _speak_current()


def _on_page_up(event):
    global _idx_main, _idx_configs, _idx_to_spawn, _idx_filter
    global _idx_side, _idx_distance, _idx_ref, _idx_mark, _idx_manage
    with _state_lock:
        if _screen == "main":
            _idx_main = max(0, _idx_main - _PAGE_JUMP)
        elif _screen == "configs":
            _idx_configs = max(0, _idx_configs - _PAGE_JUMP)
        elif _screen == "to_spawn":
            _idx_to_spawn = max(0, _idx_to_spawn - _PAGE_JUMP)
        elif _screen == "manage":
            _idx_manage = max(0, _idx_manage - _PAGE_JUMP)
        elif _screen == "filter":
            _idx_filter = max(0, _idx_filter - _PAGE_JUMP)
    _speak_current()


def _on_page_down(event):
    global _idx_main, _idx_configs, _idx_to_spawn, _idx_filter
    global _idx_side, _idx_distance, _idx_ref, _idx_mark, _idx_manage
    with _state_lock:
        if _screen == "main":
            n = len(_filtered_catalog())
            _idx_main = min(max(0, n - 1), _idx_main + _PAGE_JUMP)
        elif _screen == "configs":
            n = len(_drill_vehicle.get("configs", [])) if _drill_vehicle else 0
            _idx_configs = min(max(0, n - 1), _idx_configs + _PAGE_JUMP)
        elif _screen == "to_spawn":
            n = len(_to_spawn)
            _idx_to_spawn = min(max(0, n - 1), _idx_to_spawn + _PAGE_JUMP)
        elif _screen == "manage":
            n = len(_manage_vehicles)
            _idx_manage = min(max(0, n - 1), _idx_manage + _PAGE_JUMP)
        elif _screen == "filter":
            n = len(_filter_dialog_lines())
            _idx_filter = min(max(0, n - 1), _idx_filter + _PAGE_JUMP)
    _speak_current()


def _on_w(event):
    global _wizard_target_idx, _wizard_side, _wizard_distance
    if _screen != "to_spawn":
        return
    with _state_lock:
        if not _to_spawn:
            _say_safe("Queue is empty.")
            return
        item = _to_spawn[_idx_to_spawn]
        if item.get("replaceVehId") is not None:
            _say_safe("Placement is not needed for replacement vehicles.")
            return
        _wizard_target_idx = _idx_to_spawn
        _wizard_side = None
        _wizard_distance = None
    _enter_screen("side", header="Spawn position")


def _on_x(event):
    global _replace_editing_idx, _idx_replace_slot
    if _screen != "to_spawn":
        return
    cleared = False
    with _state_lock:
        if not _to_spawn:
            return
        item = _to_spawn[_idx_to_spawn]
        if item.get("replaceVehId") is not None:
            item["replaceSlot"] = None
            item["replaceVehId"] = None
            item["replaceVehName"] = None
            cleared = True
    if cleared:
        _say_safe("Changed to add new vehicle.")
        _speak_current()
        return
    slots = _get_slots()
    if not slots:
        _say_safe("No slot data available yet.")
        return
    _replace_editing_idx = _idx_to_spawn
    _idx_replace_slot = 0
    _enter_screen("replace_slot", header="Choose vehicle to replace")


def _do_spawn_all():
    with _state_lock:
        items = list(_to_spawn)
    if not items:
        _say_safe("Nothing to spawn.")
        return
    # Replacements first: ensures "current vehicle" refs in add-items resolve to the
    # post-replacement player vehicle. Relative order preserved within each group.
    replacements = [it for it in items if it.get("replaceVehId") is not None]
    additions    = [it for it in items if it.get("replaceVehId") is None]
    ordered = replacements + additions

    payload_items = []
    missing = 0
    for idx, it in enumerate(ordered):
        is_replace = it.get("replaceVehId") is not None
        if not is_replace and (it.get("side") is None or it.get("distFt") is None):
            missing += 1
            continue
        entry: dict[str, Any] = {
            "queueIdx":    idx,
            "model":       it["model"],
            "config":      it.get("config", ""),
            "replaceVehId": it.get("replaceVehId"),
        }
        if not is_replace:
            entry["side"]     = it["side"]
            entry["distFt"]   = it["distFt"]
            entry["refMode"]  = it.get("refMode", "vehicle")
            entry["refVehId"] = it.get("refVehId")
        payload_items.append(entry)
    if not payload_items:
        _say_safe("No items have placement set. Press W on each to configure.")
        return
    n = len(payload_items)
    msg = f"Spawning {n} {'vehicle' if n == 1 else 'vehicles'}"
    if missing:
        msg += f", skipping {missing} without placement"
    msg += ", please wait."
    _say_safe(msg)
    payload = json.dumps({"items": payload_items})
    _send_cmd(f"SPAWN:{payload}")
    # Clear the queue on dispatch so the user starts fresh next time.
    with _state_lock:
        _to_spawn.clear()
        global _idx_to_spawn
        _idx_to_spawn = 0
    # Close the modal silently — the SPAWN_OK announcement will follow when the game responds.
    _close_modal(silent=True)


# =============================================================================
#  Arrangement presets
# =============================================================================

def _on_g(event):
    global _idx_arrange, _arrange_return_screen
    if _screen not in ("main", "to_spawn", "manage"):
        return
    _arrange_return_screen = _screen
    _idx_arrange = 0
    _send_cmd("REQUEST_ACTIVE_VEHICLES")
    _send_cmd("REQUEST_PLAYER_VEH_ID")
    _enter_screen("arrange", header="Vehicle arrangement")


def _do_arrange_queue():
    with _state_lock:
        items = list(_to_spawn)
    queue_ok, _, status = _arrange_validate()
    if not queue_ok:
        _say_safe(f"Cannot apply: {status}")
        return

    type_key, _, variants = ARRANGE_TYPES[_arrange_type_idx]
    variant_key = variants[min(_arrange_variant_idx, len(variants) - 1)][0]
    spacing_m = _spacing_field.value() * 0.3048

    replacements = [it for it in items if it.get("replaceVehId") is not None]
    additions    = [it for it in items if it.get("replaceVehId") is None]
    ordered = replacements + additions

    payload_items = [
        {
            "queueIdx":    i,
            "model":       it["model"],
            "config":      it.get("config", ""),
            "replaceVehId": it.get("replaceVehId"),
        }
        for i, it in enumerate(ordered)
    ]
    arrangement = {
        "type":        type_key,
        "variant":     variant_key,
        "spacingM":    spacing_m,
        "anchorVehId": None,
    }

    n = len(payload_items)
    _say_safe(f"Spawning {n} {'vehicle' if n == 1 else 'vehicles'} in {type_key.replace('_', ' ')} arrangement, please wait.")
    _send_cmd(f"SPAWN:{json.dumps({'items': payload_items, 'arrangement': arrangement})}")
    with _state_lock:
        _to_spawn.clear()
        global _idx_to_spawn
        _idx_to_spawn = 0
    _close_modal(silent=True)


def _do_arrange_active():
    with _state_lock:
        avs = list(_active_vehicles)
    _, active_ok, status = _arrange_validate()
    if not active_ok:
        _say_safe(f"Cannot arrange: {status}")
        return

    type_key, _, variants = ARRANGE_TYPES[_arrange_type_idx]
    variant_key = variants[min(_arrange_variant_idx, len(variants) - 1)][0]
    spacing_m = _spacing_field.value() * 0.3048

    veh_ids = [vid for vid, _ in avs]
    arrangement = {
        "type":        type_key,
        "variant":     variant_key,
        "spacingM":    spacing_m,
        "anchorVehId": _player_veh_id,
    }

    n = len(veh_ids)
    _say_safe(f"Arranging {n} {'vehicle' if n == 1 else 'vehicles'} in {type_key.replace('_', ' ')} arrangement, please wait.")
    _send_cmd(f"TELEPORT_ARRANGE:{json.dumps({'arrangement': arrangement, 'vehicleIds': veh_ids})}")
    _close_modal(silent=True)


# =============================================================================
#  Active vehicles request
# =============================================================================

def _request_active_vehicles():
    _active_vehicles_event.clear()
    _send_cmd("REQUEST_ACTIVE_VEHICLES")
    # Don't block; the listener thread will populate _active_vehicles.
    # _speak_current() on entering mark_picker may report empty initially; arrow keys re-speak.


# =============================================================================
#  Modal open/close + key hook management
# =============================================================================

# (key_name, handler) pairs, all suppress=True
_MODAL_KEYS = [
    ("up",        _on_up),
    ("down",      _on_down),
    ("left",      _on_left),
    ("right",     _on_right),
    ("home",      _on_home),
    ("end",       _on_end),
    ("page up",   _on_page_up),
    ("page down", _on_page_down),
    ("tab",       _on_tab),
    ("enter",     _on_enter),
    ("esc",       _on_escape),
    ("space",     _on_space),
    ("delete",    _on_delete),
    ("f",         _on_f),
    ("c",         _on_c),
    ("g",         _on_g),
    ("w",         _on_w),
    ("r",         _on_r),
    ("v",         _on_v),
    ("a",         _on_a),
    ("x",         _on_x),
]


def _enqueue(handler: Callable[..., None]) -> Callable[..., None]:
    """Wrap a handler so it runs on the worker thread instead of the keyboard
    library's hook/listener thread. The wrapper returns immediately, which is
    essential for `suppress=True` hooks: any work done inline (UDP send, speech,
    Event.wait, etc.) can push the OS-level low-level keyboard hook past
    Windows' LowLevelHooksTimeout, causing the suppressed key to leak through
    to the foreground app (the game)."""
    def wrapper(event):
        try:
            _event_queue.put_nowait((handler, event))
        except Exception:
            pass
    return wrapper


def _worker_loop():
    while True:
        item = _event_queue.get()
        if item is None:
            return
        handler, event = item
        try:
            handler(event)
        except Exception as e:
            _logw(f"vehicle_spawner: handler error: {e}")


def _install_modal_hooks():
    global _hook_handles
    _hook_handles = []
    if not _KEYBOARD_OK:
        return
    for key, handler in _MODAL_KEYS:
        try:
            h = keyboard.on_press_key(key, _enqueue(handler), suppress=True)
            _hook_handles.append(h)
        except Exception as e:
            _logw(f"vehicle_spawner: failed to hook {key}: {e}")


def _uninstall_modal_hooks():
    global _hook_handles
    if not _KEYBOARD_OK:
        return
    for h in _hook_handles:
        try:
            keyboard.unhook(h)
        except Exception:
            pass
    _hook_handles = []


def _open_modal():
    global _modal_open, _screen, _drill_vehicle
    if _modal_open:
        return
    # Close any other exclusive UI session before taking over the keyboard.
    if _close_others is not None:
        try:
            _close_others()
        except Exception:
            pass
    # Catalog readiness is the Lua side's responsibility — it pre-builds it on
    # onWorldReadyState(state=2). If it's not ready when the user opens the
    # modal we just nudge the Lua side and announce; we never block the worker
    # waiting on it (and definitely never block the keyboard hook path).
    with _state_lock:
        ready = bool(_catalog)
    if not ready:
        _send_cmd("REQUEST_CATALOG")
        _say_safe("Catalog not ready yet. Try again shortly.")
        return
    _modal_open = True
    _drill_vehicle = None
    _install_modal_hooks()
    start_screen = _screen if _screen in _PAGES else "main"
    if start_screen == "manage":
        global _pending_manage_announce
        _pending_manage_announce = True
        _active_vehicles_event.clear()
        _send_cmd("REQUEST_ACTIVE_VEHICLES")
    _enter_screen(start_screen, header="Vehicle spawner")


def _close_modal(silent: bool = False):
    global _modal_open
    if not _modal_open:
        return
    _modal_open = False
    _uninstall_modal_hooks()
    if not silent:
        _say_safe("Vehicle spawner closed.")


def _toggle_modal():
    if _is_focused is not None and not _is_focused():
        return
    if _modal_open:
        _close_modal()
    else:
        _open_modal()


def _toggle_modal_async():
    """F11 hotkey entry — return immediately and let the worker thread do the
    actual modal toggle so neither catalog-readiness checks nor UDP sends ever
    run on the keyboard library's hook callback path."""
    try:
        _event_queue.put_nowait((lambda _e: _toggle_modal(), None))
    except Exception:
        pass


# =============================================================================
#  Public API
# =============================================================================

def init(say_fn, is_focused_fn, logger, get_slots_fn=None, close_others_fn=None):
    global _say, _is_focused, _log, _get_slots_fn, _close_others
    _say = say_fn
    _is_focused = is_focused_fn
    _log = logger
    _get_slots_fn = get_slots_fn
    _close_others = close_others_fn


def close_modal():
    _close_modal()


def start(stop_event: threading.Event):
    global _listener_thread, _stop_event, _worker_thread
    _stop_event = stop_event
    _listener_thread = threading.Thread(
        target=_listener_loop, args=(stop_event,), daemon=True
    )
    _listener_thread.start()
    _worker_thread = threading.Thread(target=_worker_loop, daemon=True)
    _worker_thread.start()
    if _KEYBOARD_OK:
        try:
            keyboard.add_hotkey("f11", _toggle_modal_async, suppress=True)
            _logi("Vehicle spawner F11 hotkey installed (suppress=True).")
        except Exception as e:
            _logw(f"vehicle_spawner: failed to add F11 hotkey: {e}")
    # Kick off an initial catalog request (Lua may have already pre-built it).
    _send_cmd("REQUEST_CATALOG")


def is_modal_open() -> bool:
    return _modal_open
