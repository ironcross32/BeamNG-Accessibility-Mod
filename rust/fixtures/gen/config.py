"""Golden cases for beam_base::config, from beamtel.load_config and configurator.load_config.

Both loaders are run against a table of config files in a throwaway LOCALAPPDATA, so the
user's real %LOCALAPPDATA%\\beamtel\\beamtel_config.json is never read or written. Each
case records, per loader, the returned dict (as the per-key ``json.dumps`` that differ
from that loader's defaults, plus the key order when it is not the merge order) and
every file left in the directory (``<input>`` / ``<defaults>`` when unchanged from the
input or equal to the loader's defaults file, otherwise the exact text).

DPAPI blobs are random, so ``dpapi:v1:<b64>`` is replaced by ``<sealed:PLAINTEXT>``
when it decrypts (the Rust test does the same with its own blobs); a blob that does not
decrypt is left as it is.
"""

from __future__ import annotations

import base64
import ctypes
import json
import os
import re
import shutil
import sys
import tempfile

TMP = tempfile.mkdtemp(prefix="beam_cfg_fx_")
# Before anything imports bnh_logger, beamtel or configurator: all of them resolve
# LOCALAPPDATA at import time.
os.environ["LOCALAPPDATA"] = TMP

from _common import write_fixture  # noqa: E402

import beamtel  # noqa: E402
import configurator  # noqa: E402
import secretstore  # noqa: E402

LOADERS = {"engine": beamtel, "configurator": configurator}
BLOB_RE = re.compile(r"dpapi:v1:[A-Za-z0-9+/=]+")
SEALED_PLAIN = "sealed-by-python"
PY_SEALED = secretstore.protect(SEALED_PLAIN)
assert secretstore.is_protected(PY_SEALED)


def norm(text):
    def sub(m):
        plain = secretstore.unprotect(m.group(0))
        return m.group(0) if plain is None else f"<sealed:{plain}>"

    return BLOB_RE.sub(sub, text)


def dumps(v):
    return json.dumps(v, indent=2, ensure_ascii=False)


def file_bytes(cfg):
    # What _write_config leaves on disk: json.dump through a text-mode file.
    return dumps(cfg).replace("\n", "\r\n").encode("utf-8")


DEFAULTS = {name: dict(mod.DEFAULT_CONFIG) for name, mod in LOADERS.items()}
DEFAULT_BYTES = {name: file_bytes(d) for name, d in DEFAULTS.items()}


def encode_result(res, default):
    dd = {k: dumps(v) for k, v in default.items()}
    diff = {}
    for k, v in res.items():
        t = norm(dumps(v))
        if dd.get(k) != t:
            diff[k] = t
    out = {"diff": diff, "missing": [k for k in default if k not in res]}
    expected = [k for k in default if k in res] + [k for k in res if k not in default]
    if list(res) != expected:
        out["order"] = list(res)
    return out


def encode_file(data, input_bytes, loader):
    if input_bytes is not None and data == input_bytes:
        return "<input>"
    if data == DEFAULT_BYTES[loader]:
        return "<defaults>"
    try:
        return norm(data.decode("utf-8"))
    except UnicodeDecodeError:
        return {"b64": base64.b64encode(data).decode("ascii")}


def lock(path):
    """Open the file with no sharing, so Python's open() fails as it does mid-save."""
    k32 = ctypes.windll.kernel32
    k32.CreateFileW.restype = ctypes.c_void_p
    h = k32.CreateFileW(path, 0x80000000, 0, None, 3, 0x80, None)
    assert h not in (None, ctypes.c_void_p(-1).value), "lock failed"
    return h


def run(loader, input_bytes, locked=False):
    mod = LOADERS[loader]
    d = tempfile.mkdtemp(dir=TMP)
    path = os.path.join(d, "beamtel_config.json")
    if input_bytes is not None:
        with open(path, "wb") as fh:
            fh.write(input_bytes)
    mod.CONFIG_PATH = path
    handle = lock(path) if locked else None
    try:
        res = encode_result(mod.load_config(), DEFAULTS[loader])
    except Exception as exc:  # noqa: BLE001
        res = {"err": type(exc).__name__}
    finally:
        if handle is not None:
            ctypes.windll.kernel32.CloseHandle(ctypes.c_void_p(handle))
    files = {}
    for name in sorted(os.listdir(d)):
        with open(os.path.join(d, name), "rb") as fh:
            files[name] = encode_file(fh.read(), input_bytes, loader)
    shutil.rmtree(d, ignore_errors=True)
    return {"result": res, "files": files}


def case(name, input_bytes, locked=False):
    c = {"name": name, "locked": locked}
    if input_bytes is None:
        c["input"] = None
    else:
        try:
            c["input"] = input_bytes.decode("utf-8")
        except UnicodeDecodeError:
            c["input_b64"] = base64.b64encode(input_bytes).decode("ascii")
    for loader in LOADERS:
        c[loader] = run(loader, input_bytes, locked)
    return c


def js(obj, indent=None):
    return json.dumps(obj, indent=indent).encode("utf-8")


VARIANTS = [
    None, True, False, 0, 1, -1, 2, 5, 7, 10, 25, 50, 60, 100, 4481, 3.7, -0.0, 0.5, 1e-05,
    -12.5, 6.4, 12.0, 1e300, -1e300, 10**30, -(10**30), float("nan"), float("inf"),
    float("-inf"), "", "  ", "abc", "5", " 12 ", "5.5", "1_0", "inf", "nan", "-inf", "1e400",
    "TRUE", "false", "metric", "Imperial", "M", "Vulkan", "OutGauge", "late", "EARLY", "audio",
    " F# ", "Bb", "c", "H", "rast", "NUMBER", "openai", "gpt-4o", "models/gemini-2.5-pro",
    "minimal", "HIGH", "  https://x/  ", "ünï", "it's", "\x01", "thk_ku100", [], [1, "a"], {},
    {"a": [1.5, None]},
]


def main():
    cases = []
    structural = [
        ("missing", None),
        ("empty", b""),
        ("whitespace", b"  \r\n "),
        ("garbage", b"not json{"),
        ("root list", b"[1, 2]"),
        ("root string", b'"x"'),
        ("root null", b"null"),
        ("root number", b"42"),
        ("empty object", b"{}"),
        ("bom", b"\xef\xbb\xbf{}"),
        ("invalid utf8", b'{"units": "\xff"}'),
        ("trailing data", b'{"units": "metric"} x'),
        ("duplicate keys", b'{"units": "metric", "speech_rate": 3, "units": "imperial"}'),
        ("non-finite literals",
         b'{"a": NaN, "b": Infinity, "c": -Infinity, "units": "metric"}'),
        ("overflowing float literal", b'{"speech_rate": 1e400}'),
        ("overflowing float literal unknown key", b'{"x": 1e400, "y": -1e400}'),
        ("big int literal", b'{"x": 123456789012345678901234567890, "mcp_server_port": 99999999999999999999}'),
        ("raw tab in string", b'{"x": "a\tb"}'),
        ("trailing comma", b'{"x": [1,]}'),
        ("leading zero", b'{"x": 01}'),
        ("bare minus", b'{"x": -}'),
        ("bare dot", b'{"x": 1.}'),
        ("leading dot", b'{"x": .5}'),
        ("exponent without digits", b'{"x": 1e}'),
        ("single quotes", b"{'x': 1}"),
        ("unquoted key", b"{x: 1}"),
        ("escapes", '{"x": "\\u00e9\\/\\b\\f\\n\\r\\t\\"\\\\ \\ud83d\\ude00 ☃"}'.encode()),
        ("bad escape", b'{"x": "\\q"}'),
        ("short unicode escape", b'{"x": "\\u12"}'),
        ("numbers", b'{"a": 1e5, "b": 1E+2, "c": -0.0, "d": 0.1, "e": 1e-7, "f": 1e16, "g": -0, "h": 2.50}'),
        ("nested 50", ('{"x": ' + "[" * 50 + "]" * 50 + "}").encode()),
        ("defaults as python writes them, engine", DEFAULT_BYTES["engine"]),
        ("defaults as python writes them, configurator", DEFAULT_BYTES["configurator"]),
        ("defaults with LF", dumps(DEFAULTS["configurator"]).encode()),
        ("legacy speech keys",
         js({"force_sapi": True, "sapi_voice_name": "David", "sapi_rate": 3, "sapi_volume": 150})),
        ("legacy speech keys kept settings",
         js({"speech_backend": "NVDA", "force_sapi": 1, "sapi_voice_name": "",
             "speech_rate": 70, "sapi_rate": -20, "sapi_volume": -4})),
        ("legacy speech bad values",
         js({"force_sapi": 0, "sapi_rate": "x", "sapi_volume": [1]})),
        ("legacy sapi rate overflow", b'{"sapi_rate": 1e400, "sapi_volume": "7"}'),
        ("legacy announce_gear false", js({"announce_gear": False})),
        ("legacy announce_gear true", js({"announce_gear": "yes", "units": "metric"})),
        ("legacy announce_gear keeps mode", js({"gear_report_mode": "audio", "announce_gear": 0})),
        ("plaintext secrets",
         js({"ai_describer_api_key": "  sk-plain  ", "ai_describer_openai_api_key": "sk-two",
             "my_password": 5, "token_x": "", "SECRET_THING": " ", "units": "metric"})),
        ("python-sealed secret", js({"ai_describer_api_key": PY_SEALED})),
        ("undecryptable secret",
         js({"ai_describer_api_key": "dpapi:v1:AAAA", "ai_describer_openai_api_key": "dpapi:v1:"})),
        ("write-back of odd values",
         js({"announce_gear": True,
             "weird": {"nested": [1, 2.5, None, True, {"k": "\x01\x1f\x7f  é \"q\" \\"}],
                       "empty_list": [], "empty_dict": {}, "float": -12.0, "tiny": 1e-05,
                       "big": 1e16, "nan": float("nan"), "inf": float("inf"),
                       "int": 10**25, "neg": -0.0}}, indent=4)),
        ("write-back keeps unknown key order",
         js({"zeta": 1, "announce_gear": True, "alpha": [], "units": "metric"})),
    ]
    for name, data in structural:
        cases.append(case(name, data))
    cases.append(case("locked file", DEFAULT_BYTES["configurator"].replace(b"imperial", b"metric"),
                      locked=True))

    keys = list(DEFAULTS["configurator"]) + [
        "force_sapi", "sapi_voice_name", "sapi_rate", "sapi_volume", "announce_gear",
        "my_token", "unknown_key",
    ]
    for vi, value in enumerate(VARIANTS):
        cases.append(case(f"all keys = variant {vi} ({value!r})",
                          js({k: value for k in keys}, indent=2)))
    for key in keys:
        for vi, value in enumerate(VARIANTS):
            cases.append(case(f"{key} = variant {vi} ({value!r})", js({key: value})))

    write_fixture("config", {
        "py_sealed_plaintext": SEALED_PLAIN,
        "default_file_bytes": {k: v.decode("utf-8") for k, v in DEFAULT_BYTES.items()},
        "cases": cases,
    })
    import logging

    logging.shutdown()
    shutil.rmtree(TMP, ignore_errors=True)


if __name__ == "__main__":
    main()
