"""
AI Describer for BEAM — captures a screenshot of the game, sends it to Google
Gemini with a blind-friendly description prompt, and returns the spoken text.

This module is deliberately self-contained and stdlib-only (plus ``mss`` for the
screen capture) so it can be imported both by the wxPython configurator GUI (for
API-key validation) and by beamtel.py (for the in-game pipeline) without pulling
in a heavyweight SDK that would bloat the Nuitka onefile build.

All network access goes through the Gemini REST API via urllib.
"""

import base64
import json
import os
import urllib.error
import urllib.parse
import urllib.request

GEMINI_BASE = "https://generativelanguage.googleapis.com/v1beta"

# Verbatim system prompt — describes the scene for a blind listener in plain text
# suitable for a screen reader (no Markdown).
SYSTEM_PROMPT = (
    "Describe this image succinctly, but in as much detail as possible as if to a "
    "blind person. Be sure to include details about the environment, such as time "
    "of day and lighting, as well as details about the scene overall. Include "
    "details regarding the positioning of objects relative to one another, starting "
    "from what's closest to the center, and working outward. The response you "
    "provide will be processed through a screen reader as synthetic speech. As "
    "such, please use plain text which is devoid of special formatting syntax such "
    "as Markdown. Line breaks are fine where appropriate."
)

DEFAULT_MODEL = "models/gemini-3-flash-preview"

# Curated allowlist of vision-capable generateContent models (the API listing
# exposes no "vision" capability flag, so we maintain this list by hand from the
# published model list). Each entry is (model_name, display_name). Newest first.
# TTS, image-generation, embeddings, veo/imagen/lyria, robotics, computer-use,
# aqa, deep-research and antigravity models are intentionally excluded.
VISION_MODELS = [
    ("models/gemini-3-flash-preview", "Gemini 3 Flash Preview"),
    ("models/gemini-3-pro-preview", "Gemini 3 Pro Preview"),
    ("models/gemini-3.1-pro-preview", "Gemini 3.1 Pro Preview"),
    ("models/gemini-3.1-flash-lite-preview", "Gemini 3.1 Flash Lite Preview"),
    ("models/gemini-3.1-flash-lite", "Gemini 3.1 Flash Lite"),
    ("models/gemini-3.5-flash", "Gemini 3.5 Flash"),
    ("models/gemini-2.5-flash", "Gemini 2.5 Flash"),
    ("models/gemini-2.5-pro", "Gemini 2.5 Pro"),
    ("models/gemini-2.5-flash-lite", "Gemini 2.5 Flash-Lite"),
    ("models/gemini-2.0-flash", "Gemini 2.0 Flash"),
    ("models/gemini-2.0-flash-001", "Gemini 2.0 Flash 001"),
    ("models/gemini-2.0-flash-lite", "Gemini 2.0 Flash-Lite"),
    ("models/gemini-2.0-flash-lite-001", "Gemini 2.0 Flash-Lite 001"),
    ("models/gemini-flash-latest", "Gemini Flash Latest"),
    ("models/gemini-flash-lite-latest", "Gemini Flash-Lite Latest"),
    ("models/gemini-pro-latest", "Gemini Pro Latest"),
]

VISION_MODEL_NAMES = [name for name, _disp in VISION_MODELS]

MAX_OUTPUT_TOKENS = 1000
# Thinking tokens count against maxOutputTokens, so for thinking-capable models we
# add headroom on top of the answer budget; otherwise the model can spend the
# whole budget reasoning and the visible answer gets truncated after a few words.
THINKING_HEADROOM_TOKENS = 2048
REQUEST_TIMEOUT_SEC = 60


def _thinking_config(model):
    """Return a generationConfig.thinkingConfig that minimizes (or disables)
    thinking for the given model, or None if the model doesn't think."""
    m = (model or "").lower()
    if "gemini-3" in m:  # Gemini 3 / 3.1 / 3.5 families use thinkingLevel
        return {"thinkingLevel": "low"}
    if "gemini-2.5" in m and ("flash" in m or "lite" in m):
        # 2.5 Flash / Flash-Lite can turn thinking off entirely.
        return {"thinkingBudget": 0}
    return None


def _is_thinking_model(model):
    m = (model or "").lower()
    return "gemini-3" in m or "gemini-2.5" in m

# ai_descriptions.log lives beside beamtel_config.json under %LOCALAPPDATA%\beamtel.
LOG_PATH = os.path.join(
    os.getenv("LOCALAPPDATA") or os.path.expanduser("~"), "beamtel", "ai_descriptions.log"
)


def display_name_for(model_name):
    """Return the friendly display name for a model name, or the name itself."""
    for name, disp in VISION_MODELS:
        if name == model_name:
            return disp
    return model_name


def _extract_error_message(body_bytes):
    """Pull a human-readable message out of a Gemini JSON error body."""
    try:
        data = json.loads(body_bytes.decode("utf-8", errors="ignore"))
        err = data.get("error") or {}
        msg = err.get("message")
        if msg:
            return str(msg)
    except Exception:
        pass
    try:
        return body_bytes.decode("utf-8", errors="ignore").strip()
    except Exception:
        return "Unknown API error."


def validate_api_key(api_key, timeout=10):
    """Validate a Gemini API key cheaply via the ListModels endpoint.

    This costs no generation tokens. Returns (ok: bool, error: str | None).
    """
    key = (api_key or "").strip()
    if not key:
        return False, "No API key provided."
    url = f"{GEMINI_BASE}/models?key={urllib.parse.quote(key)}"
    req = urllib.request.Request(url, method="GET")
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            if 200 <= resp.status < 300:
                return True, None
            return False, f"Unexpected status {resp.status}."
    except urllib.error.HTTPError as e:
        try:
            body = e.read()
        except Exception:
            body = b""
        return False, _extract_error_message(body) or f"HTTP {e.code}."
    except urllib.error.URLError as e:
        return False, f"Network error: {e.reason}"
    except Exception as e:
        return False, f"Validation failed: {e}"


def capture_primary_monitor():
    """Grab the primary monitor and return PNG-encoded bytes.

    Raises on failure (the caller is expected to handle it).
    """
    import mss
    import mss.tools

    with mss.mss() as sct:
        # monitors[0] is the "all monitors" virtual screen; monitors[1] is the
        # primary physical display.
        monitor = sct.monitors[1]
        shot = sct.grab(monitor)
        return mss.tools.to_png(shot.rgb, shot.size)


def describe_image(png_bytes, model, api_key, timeout=REQUEST_TIMEOUT_SEC):
    """Send an image to Gemini and return (text, error).

    On success returns (description_text, None). On failure returns
    (None, error_message) where error_message includes any message returned by
    the API.
    """
    key = (api_key or "").strip()
    if not key:
        return None, "No API key set."
    if not model:
        model = DEFAULT_MODEL

    b64 = base64.b64encode(png_bytes).decode("ascii")
    gen_config = {"maxOutputTokens": MAX_OUTPUT_TOKENS}
    thinking = _thinking_config(model)
    if thinking is not None:
        gen_config["thinkingConfig"] = thinking
    if _is_thinking_model(model):
        # Give the answer its full budget on top of whatever the (minimized)
        # thinking consumes, so the description isn't cut off mid-sentence.
        gen_config["maxOutputTokens"] = MAX_OUTPUT_TOKENS + THINKING_HEADROOM_TOKENS
    payload = {
        "systemInstruction": {"parts": [{"text": SYSTEM_PROMPT}]},
        "contents": [
            {
                "parts": [
                    {"inline_data": {"mime_type": "image/png", "data": b64}}
                ]
            }
        ],
        "generationConfig": gen_config,
    }
    url = f"{GEMINI_BASE}/{model}:generateContent?key={urllib.parse.quote(key)}"
    data = json.dumps(payload).encode("utf-8")
    req = urllib.request.Request(
        url, data=data, method="POST", headers={"Content-Type": "application/json"}
    )

    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            body = resp.read()
    except urllib.error.HTTPError as e:
        try:
            err_body = e.read()
        except Exception:
            err_body = b""
        return None, _extract_error_message(err_body) or f"HTTP {e.code}."
    except urllib.error.URLError as e:
        reason = getattr(e, "reason", e)
        # socket.timeout surfaces here as the reason on a read timeout.
        if "timed out" in str(reason).lower():
            return None, "Request timed out."
        return None, f"Network error: {reason}"
    except Exception as e:
        if "timed out" in str(e).lower():
            return None, "Request timed out."
        return None, f"Request failed: {e}"

    try:
        result = json.loads(body.decode("utf-8", errors="ignore"))
    except Exception as e:
        return None, f"Could not parse API response: {e}"

    # A top-level promptFeedback block usually means the request was blocked.
    feedback = result.get("promptFeedback") or {}
    block_reason = feedback.get("blockReason")

    candidates = result.get("candidates") or []
    if not candidates:
        if block_reason:
            return None, f"Request blocked by safety filters ({block_reason})."
        return None, "API returned no candidates."

    cand = candidates[0]
    parts = (cand.get("content") or {}).get("parts") or []
    # Skip "thought" parts (model reasoning) — only the answer parts are spoken.
    text = "".join(
        p.get("text", "") for p in parts if not p.get("thought")
    ).strip()
    if not text:
        finish = cand.get("finishReason") or "unknown"
        if finish == "MAX_TOKENS":
            return None, (
                "The model used its entire token budget before producing a "
                "description (finishReason: MAX_TOKENS)."
            )
        return None, f"API returned an empty description (finishReason: {finish})."
    return text, None


def _append_log(prefix, text):
    try:
        import datetime

        os.makedirs(os.path.dirname(LOG_PATH), exist_ok=True)
        stamp = datetime.datetime.now().strftime("%Y-%m-%d %H:%M:%S")
        with open(LOG_PATH, "a", encoding="utf-8") as f:
            f.write(f"[{stamp}] {prefix}: {text}\n")
    except Exception:
        pass


def log_description(text):
    """Append a successful description to ai_descriptions.log."""
    _append_log("DESCRIPTION", (text or "").replace("\n", " ").strip())


def log_error(text):
    """Append an error message to ai_descriptions.log."""
    _append_log("ERROR", (text or "").strip())
