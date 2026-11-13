"""Shared helpers for the golden-fixture generators.

Every generator does ``from _common import ...`` first; importing this module puts
the repository root on ``sys.path`` so the reference Python modules import exactly as
they do in the application. Run generators from the repository root with the project
interpreter::

    uv run python rust/fixtures/gen/<name>.py

Output is deterministic: keys are sorted (put anything whose order matters in a
list), floats that must survive exactly go through :func:`fbits`, and the file is
written with ``\\n`` line endings and a trailing newline, so regenerating an
unchanged fixture is a byte-for-byte no-op.
"""

from __future__ import annotations

import json
import math
import os
import struct
import sys

GEN_DIR = os.path.dirname(os.path.abspath(__file__))
RUST_DIR = os.path.dirname(os.path.dirname(GEN_DIR))
REPO_ROOT = os.path.dirname(RUST_DIR)
DATA_DIR = os.path.join(RUST_DIR, "fixtures", "data")

if REPO_ROOT not in sys.path:
    sys.path.insert(0, REPO_ROOT)


def fbits(x):
    """A float as ``{"bits": <IEEE-754 u64>, "repr": repr(x)}``.

    JSON has no NaN, infinity or negative zero, and a decimal float round-trip
    through two different parsers is a parity risk of its own. The Rust side reads
    this with ``beam_testutil::f``; ``repr`` is only there for a human reading a diff.
    """
    x = float(x)
    (bits,) = struct.unpack("<Q", struct.pack("<d", x))
    return {"bits": bits, "repr": repr(x)}


def outcome(fn, *args, **kwargs):
    """Run ``fn`` and record either its value or the exception it raised.

    Returns ``{"ok": value}`` or ``{"err": "<ExceptionType>", "msg": str(exc)}``.
    The value is passed through unchanged, so wrap floats in :func:`fbits` yourself
    (or use :func:`deep_fbits`).
    """
    try:
        return {"ok": fn(*args, **kwargs)}
    except Exception as exc:  # noqa: BLE001 -- the point is to record it
        return {"err": type(exc).__name__, "msg": str(exc)}


def deep_fbits(obj):
    """Replace every float inside lists, tuples and dicts with :func:`fbits`."""
    if isinstance(obj, bool) or obj is None or isinstance(obj, (int, str)):
        return obj
    if isinstance(obj, float):
        return fbits(obj)
    if isinstance(obj, (list, tuple)):
        return [deep_fbits(v) for v in obj]
    if isinstance(obj, dict):
        return {str(k): deep_fbits(v) for k, v in obj.items()}
    raise TypeError(f"cannot encode {type(obj).__name__} in a fixture")


def write_fixture(name, obj):
    """Write ``rust/fixtures/data/<name>.json`` deterministically."""
    os.makedirs(DATA_DIR, exist_ok=True)
    text = json.dumps(obj, indent=1, sort_keys=True, ensure_ascii=False, allow_nan=False)
    path = os.path.join(DATA_DIR, f"{name}.json")
    with open(path, "w", encoding="utf-8", newline="\n") as fh:
        fh.write(text)
        fh.write("\n")
    print(f"wrote {os.path.relpath(path, REPO_ROOT)}")
    return path


__all__ = [
    "REPO_ROOT",
    "DATA_DIR",
    "fbits",
    "outcome",
    "deep_fbits",
    "write_fixture",
    "math",
]
