"""
AI Describer for BEAM — captures a screenshot of the game, sends it to a vision
model with a blind-friendly description prompt, and returns the spoken text.

Several providers are supported (Google Gemini, OpenAI). Each one has its own
backend pair — a key validator and a describe call — and the rest of the app
only ever talks to the dispatchers at the bottom of this module, keyed by a
provider id. A provider also declares its own extra request parameters (see
``extras_for``), which the configurator turns into controls automatically.

This module is deliberately self-contained and stdlib-only (plus ``mss`` for the
screen capture) so it can be imported both by the wxPython configurator GUI (for
API-key validation) and by beamtel.py (for the in-game pipeline) without pulling
in a heavyweight SDK that would bloat the Nuitka onefile build.

All network access goes through the providers' REST APIs via urllib.
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

GEMINI_DEFAULT_MODEL = "models/gemini-3-flash-preview"

# Curated allowlist of vision-capable generateContent models (the API listing
# exposes no "vision" capability flag, so we maintain this list by hand from the
# published model list). Each entry is (model_name, display_name). Newest first.
# TTS, image-generation, embeddings, veo/imagen/lyria, robotics, computer-use,
# aqa, deep-research and antigravity models are intentionally excluded.
GEMINI_VISION_MODELS = [
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

MAX_OUTPUT_TOKENS = 1000
# Thinking tokens count against maxOutputTokens, so for thinking-capable models we
# add headroom on top of the answer budget; otherwise the model can spend the
# whole budget reasoning and the visible answer gets truncated after a few words.
THINKING_HEADROOM_TOKENS = 2048
REQUEST_TIMEOUT_SEC = 60
# Key validation is a one-off click, not a hot path, so it gets a generous
# budget rather than a tight one.
VALIDATE_TIMEOUT_SEC = 30


def _gemini_thinking_config(model):
    """Return a generationConfig.thinkingConfig that minimizes (or disables)
    thinking for the given model, or None if the model doesn't think."""
    m = (model or "").lower()
    if "gemini-3" in m:  # Gemini 3 / 3.1 / 3.5 families use thinkingLevel
        return {"thinkingLevel": "low"}
    if "gemini-2.5" in m and ("flash" in m or "lite" in m):
        # 2.5 Flash / Flash-Lite can turn thinking off entirely.
        return {"thinkingBudget": 0}
    return None


def _gemini_is_thinking_model(model):
    m = (model or "").lower()
    return "gemini-3" in m or "gemini-2.5" in m

# ai_descriptions.log lives beside beamtel_config.json under %LOCALAPPDATA%\beamtel.
LOG_PATH = os.path.join(
    os.getenv("LOCALAPPDATA") or os.path.expanduser("~"), "beamtel", "ai_descriptions.log"
)


def _extract_error_message(body_bytes):
    """Pull a human-readable message out of a JSON error body.

    Gemini and OpenAI both wrap failures as {"error": {"message": ...}}, so the
    same extraction serves every backend.
    """
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


def _gemini_validate_key(api_key, timeout=VALIDATE_TIMEOUT_SEC, **_opts):
    """Validate a Gemini API key cheaply via the ListModels endpoint.

    Gemini's listing is fast and free, unlike OpenAI's, so this one stays.
    Returns (ok: bool, error: str | None).
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


def capture_region(region=None):
    """Grab a screen region and return PNG-encoded bytes.

    `region` is an mss-style {left, top, width, height}; None means the primary
    monitor, which is what the AI Describer has always used and still wants -- it fires
    while the user is playing, so the game fills that display.

    Raises on failure (the caller is expected to handle it).
    """
    import mss
    import mss.tools

    with mss.mss() as sct:
        # monitors[0] is the "all monitors" virtual screen; monitors[1] is the
        # primary physical display.
        monitor = region or sct.monitors[1]
        shot = sct.grab(monitor)
        return mss.tools.to_png(shot.rgb, shot.size)


def capture_primary_monitor():
    """Grab the primary monitor and return PNG-encoded bytes."""
    return capture_region(None)


def _gemini_describe(png_bytes, model, api_key, timeout=REQUEST_TIMEOUT_SEC, **_opts):
    """Send an image to Gemini and return (text, error).

    On success returns (description_text, None). On failure returns
    (None, error_message) where error_message includes any message returned by
    the API.
    """
    key = (api_key or "").strip()
    if not key:
        return None, "No API key set."
    if not model:
        model = GEMINI_DEFAULT_MODEL

    b64 = base64.b64encode(png_bytes).decode("ascii")
    gen_config = {"maxOutputTokens": MAX_OUTPUT_TOKENS}
    thinking = _gemini_thinking_config(model)
    if thinking is not None:
        gen_config["thinkingConfig"] = thinking
    if _gemini_is_thinking_model(model):
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


# --------------------------------------------------------------------------
# OpenAI backend
# --------------------------------------------------------------------------

OPENAI_BASE = "https://api.openai.com/v1"
OPENAI_DEFAULT_BASE_URL = OPENAI_BASE
OPENAI_DEFAULT_MODEL = "gpt-5.6-terra"

# Curated allowlist of vision-capable models. GET /v1/models returns only
# id/object/created/owned_by — there is no capability flag — so, as with Gemini
# above, this list is maintained by hand from the published model table. Image
# generation, audio/realtime, transcription, embeddings, moderation, codex and
# oss models are excluded; note that o4-mini looks vision-capable by naming
# convention but is not, so it is deliberately absent.
# Each entry is (model_id, display_name). Newest/best first.
OPENAI_VISION_MODELS = [
    ("gpt-5.6-terra", "GPT-5.6 Terra (balanced)"),
    ("gpt-5.6-sol", "GPT-5.6 Sol (frontier)"),
    ("gpt-5.6-luna", "GPT-5.6 Luna (fast, low cost)"),
    ("gpt-5.5", "GPT-5.5"),
    ("gpt-5.4", "GPT-5.4"),
    ("gpt-5.4-mini", "GPT-5.4 Mini"),
    ("gpt-5.2", "GPT-5.2"),
    ("gpt-5.1", "GPT-5.1"),
    ("gpt-5-mini", "GPT-5 Mini"),
    ("gpt-5-nano", "GPT-5 Nano"),
    ("gpt-4.1", "GPT-4.1"),
    ("gpt-4.1-mini", "GPT-4.1 Mini"),
    ("gpt-4o", "GPT-4o"),
    ("gpt-4o-mini", "GPT-4o Mini"),
    ("o3", "o3"),
]

# Vocabularies for the two OpenAI dropdowns. (value, display_name).
OPENAI_REASONING_EFFORTS = [
    ("none", "None (fastest)"),
    ("minimal", "Minimal"),
    ("low", "Low"),
    ("medium", "Medium"),
    ("high", "High"),
]
OPENAI_DETAIL_LEVELS = [
    ("auto", "Auto (full resolution)"),
    ("high", "High"),
    ("low", "Low"),
]


def _openai_base(base_url):
    """Normalize a user-supplied base URL, falling back to the official one.

    A pasted URL often carries a trailing slash; stripping it keeps the joined
    path from becoming ".../v1//responses".
    """
    base = (base_url or "").strip().rstrip("/")
    return base or OPENAI_BASE


def _openai_is_reasoning_model(model):
    """True if the model accepts a `reasoning` block.

    The gpt-4o and gpt-4.1 families predate reasoning and reject the parameter,
    so they must not receive one.
    """
    m = (model or "").lower()
    return m.startswith("gpt-5") or m.startswith("o3")


def _openai_reasoning_config(model, effort):
    """Return the `reasoning` payload block, or None if it must be omitted."""
    if not _openai_is_reasoning_model(model):
        return None
    eff = (effort or "low").strip().lower()
    if eff not in [v for v, _d in OPENAI_REASONING_EFFORTS]:
        eff = "low"
    if eff == "none":
        # "none" is our way of saying "don't think" — express it by leaving the
        # block off entirely rather than sending an unsupported effort value.
        return None
    return {"effort": eff}


def _openai_request(url, api_key, payload, timeout):
    """Issue a JSON request to OpenAI and return (parsed_body, error).

    `payload` of None makes it a GET. Shares the error ladder with the Gemini
    helpers so callers see identically-shaped messages whatever the provider.
    """
    headers = {"Authorization": f"Bearer {api_key}"}
    data = None
    if payload is not None:
        headers["Content-Type"] = "application/json"
        data = json.dumps(payload).encode("utf-8")
    req = urllib.request.Request(
        url, data=data, method="POST" if data else "GET", headers=headers
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
        if "timed out" in str(reason).lower():
            return None, "Request timed out."
        return None, f"Network error: {reason}"
    except Exception as e:
        if "timed out" in str(e).lower():
            return None, "Request timed out."
        return None, f"Request failed: {e}"

    try:
        return json.loads(body.decode("utf-8", errors="ignore")), None
    except Exception as e:
        return None, f"Could not parse API response: {e}"


def _openai_validate_key(
    api_key, timeout=VALIDATE_TIMEOUT_SEC, base_url=None, model=None, **_opts
):
    """Validate an OpenAI API key with a minimal Responses call.

    GET /v1/models would cost nothing, but it is erratically slow — measured at
    0.6s on one call and 20s on the next for the same account — which made key
    validation time out at random. /v1/responses answers in about a second, and
    it is the endpoint the describer actually uses, so this also proves the key
    carries the right scope and can reach the chosen model.

    The cost is a handful of tokens per validation, which only happens when the
    user sets a key. Returns (ok: bool, error: str | None).
    """
    key = (api_key or "").strip()
    if not key:
        return False, "No API key provided."
    payload = {
        "model": model or OPENAI_DEFAULT_MODEL,
        "input": "Say OK.",
        # 16 is the floor the API accepts.
        "max_output_tokens": 16,
    }
    _body, err = _openai_request(
        f"{_openai_base(base_url)}/responses", key, payload, timeout
    )
    if err:
        return False, err
    return True, None


def _openai_text_from_response(result):
    """Pull the assistant text out of a Responses API body.

    With a reasoning model the `output` array holds a `reasoning` item *before*
    the `message` item, so indexing output[0] would return the reasoning block.
    Walk the array and take only output_text parts of message items.
    """
    chunks = []
    for item in result.get("output") or []:
        if item.get("type") != "message":
            continue
        for part in item.get("content") or []:
            if part.get("type") == "output_text":
                chunks.append(part.get("text", ""))
    return "".join(chunks).strip()


def _openai_refusal(result):
    """Return the model's refusal text from a Responses body, if any."""
    for item in result.get("output") or []:
        for part in item.get("content") or []:
            if part.get("type") == "refusal":
                return part.get("refusal") or "no reason given"
    return None


def _openai_describe(
    png_bytes,
    model,
    api_key,
    timeout=REQUEST_TIMEOUT_SEC,
    base_url=None,
    reasoning_effort="low",
    detail="auto",
    **_opts,
):
    """Send an image to OpenAI's Responses API and return (text, error)."""
    key = (api_key or "").strip()
    if not key:
        return None, "No API key set."
    if not model:
        model = OPENAI_DEFAULT_MODEL
    det = (detail or "auto").strip().lower()
    if det not in [v for v, _d in OPENAI_DETAIL_LEVELS]:
        det = "auto"

    b64 = base64.b64encode(png_bytes).decode("ascii")
    payload = {
        "model": model,
        # The system prompt goes in the top-level `instructions` string, which
        # takes priority over anything in `input`.
        "instructions": SYSTEM_PROMPT,
        "input": [
            {
                "role": "user",
                "content": [
                    {"type": "input_text", "text": "Describe what is on screen."},
                    {
                        # Unlike Chat Completions, Responses takes image_url as a
                        # plain data-URL string, not an object.
                        "type": "input_image",
                        "image_url": f"data:image/png;base64,{b64}",
                        "detail": det,
                    },
                ],
            }
        ],
        "max_output_tokens": MAX_OUTPUT_TOKENS,
    }
    reasoning = _openai_reasoning_config(model, reasoning_effort)
    if reasoning is not None:
        payload["reasoning"] = reasoning
        # Reasoning tokens count against max_output_tokens, so give the answer
        # its own budget on top — same problem the Gemini path solves above.
        payload["max_output_tokens"] = MAX_OUTPUT_TOKENS + THINKING_HEADROOM_TOKENS

    result, err = _openai_request(
        f"{_openai_base(base_url)}/responses", key, payload, timeout
    )
    if err:
        return None, err

    text = _openai_text_from_response(result)
    if not text:
        if result.get("status") == "incomplete":
            reason = (result.get("incomplete_details") or {}).get("reason") or "unknown"
            if reason == "max_output_tokens":
                return None, (
                    "The model used its entire token budget before producing a "
                    "description (incomplete: max_output_tokens)."
                )
            return None, f"API returned an incomplete response ({reason})."
        refusal = _openai_refusal(result)
        if refusal:
            return None, f"The model declined to describe the image: {refusal}"
        return None, "API returned an empty description."
    return text, None


# --------------------------------------------------------------------------
# Provider registry and dispatch
# --------------------------------------------------------------------------

# (provider_id, display_name), in the order the configurator lists them.
PROVIDERS = [("gemini", "Google Gemini"), ("openai", "OpenAI")]
DEFAULT_PROVIDER = "gemini"

# Config keys that are not provider-specific.
PROVIDER_CFG_KEY = "ai_describer_provider"

_PROVIDER_INFO = {
    "gemini": {
        "display": "Google Gemini",
        "models": GEMINI_VISION_MODELS,
        "default_model": GEMINI_DEFAULT_MODEL,
        "key_cfg": "ai_describer_api_key",
        "model_cfg": "ai_describer_model",
        "key_help": "Paste your Google Gemini API key:",
        "validate": _gemini_validate_key,
        "describe": _gemini_describe,
        "extras": [],
    },
    "openai": {
        "display": "OpenAI",
        "models": OPENAI_VISION_MODELS,
        "default_model": OPENAI_DEFAULT_MODEL,
        "key_cfg": "ai_describer_openai_api_key",
        "model_cfg": "ai_describer_openai_model",
        "key_help": "Paste your OpenAI API key:",
        "validate": _openai_validate_key,
        "describe": _openai_describe,
        # Provider-specific request parameters. The configurator builds one
        # control per entry and hides them all when another provider is active,
        # so adding a parameter here is all it takes to surface it in the GUI.
        "extras": [
            {
                "key": "ai_describer_openai_base_url",
                "arg": "base_url",
                "kind": "text",
                "label": "Base URL:",
                "default": OPENAI_DEFAULT_BASE_URL,
                "help": (
                    "The API endpoint. Change this only to reach a proxy or an "
                    "OpenAI-compatible server."
                ),
            },
            {
                "key": "ai_describer_openai_reasoning_effort",
                "arg": "reasoning_effort",
                "kind": "choice",
                "label": "Reasoning effort:",
                "default": "low",
                "values": OPENAI_REASONING_EFFORTS,
                "help": (
                    "How long the model thinks before answering. Higher settings "
                    "are slower and cost more. Ignored by models without reasoning."
                ),
            },
            {
                "key": "ai_describer_openai_detail",
                "arg": "detail",
                "kind": "choice",
                "label": "Image detail:",
                "default": "auto",
                "values": OPENAI_DETAIL_LEVELS,
                "help": (
                    "How much image resolution is sent. Low costs less but loses "
                    "on-screen text."
                ),
            },
        ],
    },
}


def provider_info(provider):
    """Return the registry entry for a provider id, falling back to the default."""
    return _PROVIDER_INFO.get(provider) or _PROVIDER_INFO[DEFAULT_PROVIDER]


def provider_display_name(provider):
    return provider_info(provider)["display"]


def vision_models_for(provider):
    """Return [(model_name, display_name)] for a provider."""
    return provider_info(provider)["models"]


def model_names_for(provider):
    return [name for name, _disp in vision_models_for(provider)]


def default_model_for(provider):
    return provider_info(provider)["default_model"]


def config_keys_for(provider):
    """Return (api_key_config_name, model_config_name) for a provider."""
    info = provider_info(provider)
    return info["key_cfg"], info["model_cfg"]


def extras_for(provider):
    """Return the provider's extra-parameter descriptors (possibly empty)."""
    return provider_info(provider)["extras"]


def all_config_keys():
    """Every config key the AI Describer owns, across all providers.

    Both configurator panels use this to avoid clobbering each other's writes,
    so a newly added provider key is picked up automatically.
    """
    keys = [PROVIDER_CFG_KEY, "ai_describer_disable_ui_toggle"]
    for pid, _disp in PROVIDERS:
        info = provider_info(pid)
        keys.append(info["key_cfg"])
        keys.append(info["model_cfg"])
        keys.extend(ex["key"] for ex in info["extras"])
    return tuple(keys)


def extra_args_for(provider, cfg):
    """Build the **opts dict for a provider from a config mapping.

    Missing or invalid values fall back to the descriptor's default, so a
    hand-edited config file can't produce a malformed request.
    """
    opts = {}
    for ex in extras_for(provider):
        val = (cfg or {}).get(ex["key"], ex["default"])
        if ex["kind"] == "choice":
            if val not in [v for v, _d in ex["values"]]:
                val = ex["default"]
        else:
            val = (val or "").strip() or ex["default"]
        opts[ex["arg"]] = val
    return opts


def display_name_for(model_name, provider=DEFAULT_PROVIDER):
    """Return the friendly display name for a model name, or the name itself."""
    for name, disp in vision_models_for(provider):
        if name == model_name:
            return disp
    return model_name


def validate_api_key(
    api_key, provider=DEFAULT_PROVIDER, timeout=VALIDATE_TIMEOUT_SEC, **opts
):
    """Validate an API key against the given provider.

    Pass `model=` so providers that validate by making a real (tiny) request
    check the model the user actually selected. Returns (ok, error).
    """
    return provider_info(provider)["validate"](api_key, timeout=timeout, **opts)


def describe_image(
    png_bytes, model, api_key, provider=DEFAULT_PROVIDER,
    timeout=REQUEST_TIMEOUT_SEC, **opts
):
    """Send an image to the given provider and return (text, error).

    Extra keyword arguments are the provider's own request parameters (see
    ``extra_args_for``); backends ignore any they don't use, so a caller can
    pass the whole dict unconditionally.
    """
    return provider_info(provider)["describe"](
        png_bytes, model, api_key, timeout=timeout, **opts
    )


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
