# speech.py — screen reader / TTS output for BEAM, backed by Prism (prismatoid).
#
# Replaces the old ctypes wrapper around SRAL.dll. SRAL was deprecated by its
# author in August 2026 in favour of Prism, which ships as a real wheel, so the
# native library is now resolved by the package instead of being copied next to
# the executable by hand.
#
# Everything Prism-specific lives here. The rest of the app talks to the module
# level functions below, which never raise: speech failing must never take down
# a telemetry callout or a keyboard handler.

import threading
import time

from bnh_logger import get_logger

logger = get_logger()

try:
    import prism

    PRISM_OK = True
except Exception as e:  # pragma: no cover - depends on the install
    PRISM_OK = False
    logger.warning(f"Prism not available; speech is disabled: {e}")

# "auto" means let Prism pick by backend priority (NVDA outranks JAWS outranks
# SAPI, and so on). Anything else is matched against a Prism backend name.
AUTO_BACKEND = "auto"

# A failed init used to latch permanently, which meant that starting beamtel
# before the screen reader left the session mute until it was restarted. Retry,
# but not on every utterance — loading the native library is slow enough that
# doing it per callout would be audible.
_RETRY_COOLDOWN_S = 10.0

_lock = threading.RLock()
_ctx = None
_backend = None
_config = {}
_next_retry_at = 0.0
# Set from Prism's availability thread when a backend appears or disappears;
# consumed on the next call so we never re-enter Prism from its own callback.
_stale = False


def migrate_config(cfg):
    """Fold pre-Prism SAPI keys into the speech_* keys, in place.

    The old keys were written by every configurator release up to the SRAL
    removal but never read by beamtel, so this mostly exists to stop stale
    files carrying dead settings forward. Returns True if anything changed.
    """
    if not isinstance(cfg, dict):
        return False
    changed = False
    if "force_sapi" in cfg:
        if cfg.pop("force_sapi"):
            cfg.setdefault("speech_backend", "SAPI")
        changed = True
    if "sapi_voice_name" in cfg:
        name = cfg.pop("sapi_voice_name")
        if name:
            cfg.setdefault("speech_voice_name", name)
        changed = True
    if "sapi_rate" in cfg:
        # SAPI's -10..10 becomes a 0..100 percentage.
        try:
            rate = max(-10, min(10, int(cfg.pop("sapi_rate"))))
            cfg.setdefault("speech_rate", int(round((rate + 10) * 5)))
        except Exception:
            cfg.pop("sapi_rate", None)
        changed = True
    if "sapi_volume" in cfg:
        try:
            cfg.setdefault("speech_volume", max(0, min(100, int(cfg.pop("sapi_volume")))))
        except Exception:
            cfg.pop("sapi_volume", None)
        changed = True
    return changed


def _read_config(cfg):
    """Pull the speech keys out of a full beamtel config dict."""
    if not isinstance(cfg, dict):
        return {}
    return {
        "backend": str(cfg.get("speech_backend", AUTO_BACKEND) or AUTO_BACKEND),
        "voice_name": str(cfg.get("speech_voice_name", "") or ""),
        "rate": cfg.get("speech_rate", None),
        "volume": cfg.get("speech_volume", None),
    }


def _on_availability(backend_id, name, available):
    """Runs on Prism's dispatch thread. Do no work here beyond flagging."""
    global _stale
    logger.info(
        "Speech backend %s %s", name, "became available" if available else "went away"
    )
    _stale = True


def _make_context():
    try:
        prism.install_logging("bnvdahook.prism")
        # ERROR, not WARN: probing for absent screen readers logs benign
        # warnings ("delay-load dummy handle pool exhausted") on every launch,
        # which would look like a fault in bnvdahook.log.
        prism.set_log_level(prism.LogLevel.ERROR)
    except Exception as e:
        logger.debug(f"Could not install Prism log bridge: {e}")
    return prism.Context(on_availability=_on_availability)


def _shared_context():
    """The one Context for this process, created on demand.

    Everything goes through this, enumeration included. beamtel.py embeds the
    configurator's ConfigPanel in its own window (beamtel.py:4956), so a
    throwaway Context built to list voices would live in the same process as
    the speaking one — and Context.__del__ calls prism_shutdown.
    """
    global _ctx
    if not PRISM_OK:
        return None
    with _lock:
        if _ctx is None:
            try:
                _ctx = _make_context()
            except Exception as e:
                logger.warning(f"Could not create Prism context: {e}")
                return None
        return _ctx


def _acquire_backend(ctx, name):
    """Create the configured backend, falling back to Prism's best choice."""
    if name and name != AUTO_BACKEND:
        try:
            return ctx.acquire(ctx.id_of(name))
        except Exception as e:
            logger.warning(
                f"Configured speech backend '{name}' unavailable ({e}); "
                "falling back to the best available one."
            )
    return ctx.acquire_best()


def _build_locked():
    """Construct context + backend. Caller holds _lock."""
    global _ctx, _backend, _next_retry_at, _stale

    now = time.monotonic()
    if _backend is None and now < _next_retry_at:
        return None

    try:
        if _ctx is None:
            _ctx = _make_context()
        _backend = _acquire_backend(_ctx, _config.get("backend", AUTO_BACKEND))
        _stale = False
    except Exception as e:
        logger.warning(f"Failed to initialize speech: {e}")
        _backend = None
        _next_retry_at = now + _RETRY_COOLDOWN_S
        return None

    _apply_params_locked()
    return _backend


def init(cfg=None):
    """Return the shared backend, constructing it at most once.

    Called from every thread that speaks (telemetry, scanner, keyboard worker,
    timers, the wx UI, the WebSocket bridge), so construction is serialized:
    two threads racing here would each build a Context, and the loser being
    garbage collected would call prism_shutdown out from under the winner.

    Loading the native library is slow enough to matter, so callers on latency
    sensitive paths should make sure it is already warm (see install_hotkeys).
    """
    global _config
    if not PRISM_OK:
        return None
    with _lock:
        if cfg is not None:
            _config = _read_config(cfg)
        if _backend is not None and not _stale:
            return _backend
        return _build_locked()


def configure(cfg):
    """Apply a new config dict, rebuilding the backend if the choice changed."""
    global _config
    if not PRISM_OK:
        return
    with _lock:
        previous = _config.get("backend")
        _config = _read_config(cfg)
        if _backend is not None and _config.get("backend") != previous:
            _build_locked()
        else:
            _apply_params_locked()


def _find_voice_index(backend, name):
    want = (name or "").strip().lower()
    if not want:
        return None
    try:
        backend.refresh_voices()
    except Exception:
        pass
    try:
        for i in range(backend.voices_count):
            if backend.get_voice_name(i).strip().lower() == want:
                return i
    except Exception:
        return None
    return None


def _apply_params_locked():
    """Apply voice/rate/volume, skipping whatever this backend cannot do.

    Screen reader backends generally own their own speech parameters — NVDA
    reports supports_set_rate=False — so this is a no-op for most users and
    only bites for the software synths (SAPI, OneCore).
    """
    backend = _backend
    if backend is None:
        return
    try:
        feats = backend.features
    except Exception:
        return

    voice_name = _config.get("voice_name") or ""
    if voice_name and feats.supports_set_voice:
        idx = _find_voice_index(backend, voice_name)
        if idx is None:
            logger.info(f"Configured voice '{voice_name}' not found on {backend.name}")
        else:
            try:
                backend.voice = idx
            except Exception as e:
                logger.debug(f"Could not set voice: {e}")

    for key, supported, attr in (
        ("rate", feats.supports_set_rate, "rate"),
        ("volume", feats.supports_set_volume, "volume"),
    ):
        value = _config.get(key)
        if value is None or not supported:
            continue
        try:
            # Config stores 0-100 for the UI; Prism normalizes to 0.0-1.0.
            setattr(backend, attr, max(0.0, min(1.0, float(value) / 100.0)))
        except Exception as e:
            logger.debug(f"Could not set {key}: {e}")


def _call(action, *args):
    """Run a backend method, re-acquiring once if the backend has gone away.

    A screen reader can exit mid-session (BeamNG sessions run for hours), which
    leaves the handle we hold pointing at a dead client. One retry against a
    freshly acquired backend covers both that and the reverse case where the
    reader started after we did.
    """
    with _lock:
        backend = init()
        if backend is None:
            return False
        try:
            getattr(backend, action)(*args)
            return True
        except Exception as e:
            logger.debug(f"Speech {action} failed on {_backend_name_locked()}: {e}")
            if _build_locked() is None:
                return False
            try:
                getattr(_backend, action)(*args)
                return True
            except Exception as e2:
                logger.warning(f"Speech {action} failed after re-acquire: {e2}")
                return False


def _backend_name_locked():
    try:
        return _backend.name if _backend is not None else "none"
    except Exception:
        return "unknown"


def speak(text, interrupt=True):
    """Speak text. Prism defaults interrupt to False; BEAM's default is True."""
    if not text:
        return False
    return _call("speak", text, bool(interrupt))


def stop():
    """Interrupt whatever is currently being spoken."""
    return _call("stop")


def braille(text):
    """Send text to a connected braille display, if the backend supports it."""
    if not text:
        return False
    if not supports("supports_braille"):
        return False
    return _call("braille", text)


def output(text, interrupt=True):
    """Speak and braille in one call, where the backend supports both."""
    if not text:
        return False
    return _call("output", text, bool(interrupt))


def backend_name():
    with _lock:
        if init() is None:
            return None
        return _backend_name_locked()


def features():
    """The active backend's capability bits, or None if there is no backend."""
    with _lock:
        backend = init()
        if backend is None:
            return None
        try:
            return backend.features
        except Exception:
            return None


def supports(feature_name):
    feats = features()
    return bool(feats is not None and getattr(feats, feature_name, False))


def describe_capabilities():
    """One-line summary of the active backend, for the startup log."""
    feats = features()
    if feats is None:
        return "speech unavailable"
    supported = [
        short
        for short, attr in (
            ("speak", "supports_speak"),
            ("braille", "supports_braille"),
            ("stop", "supports_stop"),
            ("rate", "supports_set_rate"),
            ("volume", "supports_set_volume"),
            ("voices", "supports_set_voice"),
            ("pcm", "supports_speak_to_memory"),
        )
        if getattr(feats, attr, False)
    ]
    return f"{backend_name()} [{', '.join(supported) or 'no capabilities reported'}]"


# --- Enumeration helpers, used by the configurator ---


def list_backends():
    """Names of every backend Prism knows about on this machine."""
    ctx = _shared_context()
    if ctx is None:
        return []
    try:
        return [ctx.name_of(ctx.id_of(i)) for i in range(ctx.backends_count)]
    except Exception as e:
        logger.warning(f"Could not enumerate speech backends: {e}")
        return []


def list_voices(backend_name_or_auto=AUTO_BACKEND):
    """(names, features) for a backend, without disturbing the active one.

    Returns a fresh Backend so the configurator can probe SAPI while beamtel
    keeps speaking through NVDA.
    """
    ctx = _shared_context()
    if ctx is None:
        return [], None
    try:
        backend = _acquire_backend(ctx, backend_name_or_auto)
        feats = backend.features
        names = []
        if feats.supports_count_voices:
            try:
                backend.refresh_voices()
            except Exception:
                pass
            names = [backend.get_voice_name(i) for i in range(backend.voices_count)]
        return names, feats
    except Exception as e:
        logger.warning(f"Could not enumerate voices for {backend_name_or_auto}: {e}")
        return [], None


def test_speak(backend_name_or_auto, voice_name, rate, volume, text):
    """Speak a test line on an arbitrary backend. Returns (ok, error_message)."""
    ctx = _shared_context()
    if ctx is None:
        return False, "No speech library is available."
    try:
        backend = _acquire_backend(ctx, backend_name_or_auto)
        feats = backend.features
        if voice_name and feats.supports_set_voice:
            idx = _find_voice_index(backend, voice_name)
            if idx is not None:
                backend.voice = idx
        if rate is not None and feats.supports_set_rate:
            backend.rate = max(0.0, min(1.0, float(rate) / 100.0))
        if volume is not None and feats.supports_set_volume:
            backend.volume = max(0.0, min(1.0, float(volume) / 100.0))
        backend.speak(text, True)
        # The Backend must outlive the utterance; speak() is asynchronous and
        # freeing the handle here would cut the test off mid-word.
        _hold_test_backend(backend)
        return True, None
    except Exception as e:
        return False, str(e)


_test_backend = None


def _hold_test_backend(backend):
    global _test_backend
    _test_backend = backend


def shutdown():
    """Release the backend and context. Safe to call more than once."""
    global _ctx, _backend
    with _lock:
        _backend = None
        _ctx = None
