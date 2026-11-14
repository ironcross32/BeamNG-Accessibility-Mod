"""Import beamtel.py for a fixture generator without touching the user's real files.

beamtel creates its config directory and several recorder directories at import time,
under %LOCALAPPDATA%\\beamtel. Generators must never write there, so LOCALAPPDATA is
pointed at a fresh temporary directory BEFORE the import. Importing also needs wx and
keyboard to be importable, which the project's virtualenv provides.
"""

from __future__ import annotations

import os
import tempfile

import _common  # noqa: F401  -- puts the repository root on sys.path

_TMP = tempfile.mkdtemp(prefix="beam_fixture_")
os.environ["LOCALAPPDATA"] = _TMP

import beamtel  # noqa: E402

assert beamtel.CONFIG_DIR.startswith(_TMP), beamtel.CONFIG_DIR

__all__ = ["beamtel"]
