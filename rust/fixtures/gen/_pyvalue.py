"""Tagged encoding of JSON-shaped Python values for beam-reports fixtures.

``beam_reports::pyops::PyValue`` keeps what plain JSON loses (int vs float, NaN and the
infinities, -0.0, dict order), so fixtures carry values as ``{"t": tag, "v": ...}``:
none, bool, int (decimal text), float (``fbits``), str, list, dict (a list of
``[key, value]`` pairs, in order). The Rust side decodes the same tags
(``tests/common/mod.rs``).
"""

from __future__ import annotations

from _common import fbits


def enc(v):
    if v is None:
        return {"t": "none"}
    if isinstance(v, bool):
        return {"t": "bool", "v": v}
    if isinstance(v, int):
        return {"t": "int", "v": str(v)}
    if isinstance(v, float):
        return {"t": "float", "v": fbits(v)}
    if isinstance(v, str):
        return {"t": "str", "v": v}
    if isinstance(v, (list, tuple)):
        return {"t": "list", "v": [enc(x) for x in v]}
    if isinstance(v, dict):
        return {"t": "dict", "v": [[str(k), enc(x)] for k, x in v.items()]}
    raise TypeError(f"cannot encode {type(v).__name__}")


def outcome_enc(fn, *args, **kwargs):
    """``{"ok": enc(value)}`` or ``{"err": type, "msg": str}``."""
    try:
        return {"ok": enc(fn(*args, **kwargs))}
    except Exception as exc:  # noqa: BLE001
        return {"err": type(exc).__name__, "msg": str(exc)}
