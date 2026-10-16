"""Row parsing, speech and stepping for the audio debug browser (F9 then J).

The game half is ``audioDebugAccessible.lua`` (GE) and ``beamtelAudioDebug.lua`` (vehicle VM);
see ``docs/lua-audio-debug.md``. Everything here is pure so ``diagnostic/audio_debug_row_sim.py``
can test it without a game, beamtel or a speech backend. The browser glue that owns sockets,
speech and the virtual browser lives in beamtel.py.
"""


def parse_fields(payload):
    """Parse a ``k=v;k=v`` row body into a dict.

    Split on the FIRST '=' per field and keep unknown keys, as environment rows do: the Lua side
    sanitizes ';' and '=' out of every value, and a field this build does not know about must be
    ignored rather than shifting anything after it. A bare field (the leading count on a BEGIN
    line) is kept as ``count``.
    """
    out = {}
    for field in payload.split(";"):
        if not field:
            continue
        key, sep, value = field.partition("=")
        if sep:
            out[key.strip()] = value.strip()
        elif "count" not in out:
            out["count"] = field.strip()
    return out


def _float(row, key):
    try:
        return float(row.get(key, ""))
    except (TypeError, ValueError):
        return None


def _num(v):
    """A number as speech reads it best: no trailing zeros, no "-0", minus spelled out."""
    v = round(v, 2)
    if v == 0:
        return "0"
    text = f"{abs(v):g}"
    return f"minus {text}" if v < 0 else text


def spoken_value(row):
    """The value part of a number row, in the unit the user hears."""
    unit = row.get("unit", "")
    if unit == "Hz":
        # The wire value is FMOD's 0-1000 scale; the Lua side sends the Hz it maps to.
        hz = _float(row, "hz")
        if hz is None:
            return ""
        if hz >= 1000:
            return f"{float(f'{hz / 1000:.3g}'):g} kilohertz"
        return f"{float(f'{hz:.3g}'):g} hertz"
    value = _float(row, "value")
    if value is None:
        return ""
    if unit == "dB":
        return f"{_num(value)} decibels"
    if unit == "count":
        return str(int(round(value)))
    return _num(value)


def row_line(row):
    """One spoken line for an engine-sound row."""
    kind = row.get("kind", "")
    label = row.get("label", "")
    if kind == "select":
        try:
            index = int(row.get("index", "0")) + 1
            count = int(row.get("count", "1"))
        except ValueError:
            index, count = 1, 1
        parts = [row.get("ref", ""), row.get("device", ""), row.get("blend", "")]
        name = ", ".join(p for p in parts if p)
        line = f"Sound {index} of {count}: {name}"
        if row.get("modified") == "1":
            line += ", changed"
        return line
    if kind == "number":
        value = spoken_value(row)
        return f"{label}, {value}" if value else label
    if kind == "action" and row.get("key") == "restore" and row.get("editable") == "0":
        return f"{label}, nothing changed"
    return label


def stepped_value(row, delta):
    """The value one press of left/right (delta steps) moves a number row to.

    Returns None when the row cannot move that way: not a number row, or already at the end of
    its range. Not snapped to the step grid -- a value the vehicle shipped with (382.35 on the
    FMOD scale, say) stays reachable by stepping back.
    """
    if row.get("kind") != "number":
        return None
    value = _float(row, "value")
    step = _float(row, "step")
    if value is None or not step:
        return None
    target = value + delta * step
    lo = _float(row, "min")
    hi = _float(row, "max")
    if lo is not None:
        target = max(lo, target)
    if hi is not None:
        target = min(hi, target)
    target = round(target, 4)
    if target == round(value, 4):
        return None
    return target


def wire_number(v):
    """A number for a SET command: short, and never in exponent form for these ranges."""
    return f"{v:.6g}"


def select_target(row, delta):
    """The sound index left/right moves to on the selector row, wrapping."""
    try:
        index = int(row.get("index", "0"))
        count = int(row.get("count", "0"))
    except ValueError:
        return None
    if count <= 1:
        return None
    return (index + (1 if delta > 0 else -1)) % count


def sfx_line(row):
    """One spoken line for a sound-library row."""
    name = row.get("name", "")
    if row.get("kind") == "dir":
        count = row.get("count", "")
        noun = "item" if count == "1" else "items"
        return f"{name}, folder, {count} {noun}" if count else f"{name}, folder"
    return name
