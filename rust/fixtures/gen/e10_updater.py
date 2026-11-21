"""Golden cases for E10: updater.py and the mod installer half of config_ui.py.

Writes rust/fixtures/data/e10_updater.json. Run from the repository root:

    uv run python rust/fixtures/gen/e10_updater.py

Nothing here touches the network, the real game folders, the user's LOCALAPPDATA or a
real process: LOCALAPPDATA is pointed at a scratch directory BEFORE the reference
modules are imported (they create their config and log directories at import), urlopen,
Popen and wx.MessageBox are replaced, and every path is under a temp directory. Paths in
recorded messages are written with that directory replaced by ``<ROOT>``.
"""

from __future__ import annotations

import base64
import io
import json
import os
import re
import shutil
import sys
import tempfile
import zipfile

_SCRATCH = tempfile.mkdtemp(prefix="e10gen-")
os.environ["LOCALAPPDATA"] = os.path.join(_SCRATCH, "localappdata")
os.makedirs(os.environ["LOCALAPPDATA"], exist_ok=True)

from _common import write_fixture  # noqa: E402

import logging  # noqa: E402
import subprocess  # noqa: E402
import urllib.error  # noqa: E402

import wx  # noqa: E402

import config_ui  # noqa: E402
import configurator  # noqa: E402
import updater  # noqa: E402
from bnh_logger import get_logger  # noqa: E402

# Keep the reference logger quiet on the console; records are captured below.
_LOG = get_logger()
for _h in list(_LOG.handlers):
    if isinstance(_h, logging.StreamHandler) and not isinstance(_h, logging.FileHandler):
        _LOG.removeHandler(_h)


class _Capture(logging.Handler):
    def __init__(self):
        super().__init__(level=logging.DEBUG)
        self.records = []

    def emit(self, record):
        self.records.append([record.levelname, record.getMessage()])


CAPTURE = _Capture()
_LOG.addHandler(CAPTURE)

FIXED_ZIP_DATE = (2020, 1, 2, 3, 4, 6)


def make_zip(members, compress=zipfile.ZIP_DEFLATED):
    """members: list of (name, bytes or None for a directory entry)."""
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w") as zf:
        for name, data in members:
            info = zipfile.ZipInfo(name, date_time=FIXED_ZIP_DATE)
            info.compress_type = compress
            if data is None:
                info.external_attr = 0o40775 << 16 | 0x10
                zf.writestr(info, b"")
            else:
                zf.writestr(info, data)
    return buf.getvalue()


def b64(data):
    return base64.b64encode(data).decode("ascii")


def scrub(text, root):
    if text is None:
        return None
    text = str(text)
    text = text.replace(root, "<ROOT>")
    text = text.replace(repr(root)[1:-1], "<ROOTR>")
    return re.sub(r"\.pre-bnvda-removal-\d{8}-\d{6}-\d{6}\.bak", ".pre-bnvda-removal-<TS>.bak", text)


def tree(root_dir):
    """Every file under root_dir as {relative/posix/path: base64}, directories as None."""
    out = {}
    if not os.path.isdir(root_dir):
        return None
    for dirpath, dirnames, filenames in os.walk(root_dir):
        for d in dirnames:
            rel = os.path.relpath(os.path.join(dirpath, d), root_dir).replace("\\", "/")
            out[rel + "/"] = None
        for f in filenames:
            p = os.path.join(dirpath, f)
            rel = os.path.relpath(p, root_dir).replace("\\", "/")
            with open(p, "rb") as fh:
                out[rel] = b64(fh.read())
    return out


# ---------------------------------------------------------------------------
# Fakes
# ---------------------------------------------------------------------------

STYLE_KINDS = [
    (wx.ICON_ERROR, "error"),
    (wx.ICON_WARNING, "warning"),
    (wx.ICON_QUESTION, "question"),
    (wx.ICON_INFORMATION, "info"),
]


def describe_style(style):
    kind = next(k for flag, k in STYLE_KINDS if style & flag)
    if style & wx.YES_NO:
        buttons = "yes_no"
        default = "no" if style & wx.NO_DEFAULT else "yes"
    else:
        buttons = "ok"
        default = "ok"
    return kind, buttons, default


class Recorder:
    def __init__(self, root, answers=()):
        self.root = root
        self.events = []
        self.answers = list(answers)

    def message_box(self, message, caption, style, parent=None):
        kind, buttons, default = describe_style(style)
        self.events.append(
            {
                "event": "message",
                "kind": kind,
                "title": caption,
                "text": scrub(message, self.root),
                "buttons": buttons,
                "default": default,
            }
        )
        if self.answers:
            a = self.answers.pop(0)
        else:
            a = default
        return {"yes": wx.YES, "no": wx.NO, "ok": wx.OK}[a]


class FakeResponse:
    def __init__(self, body, content_length):
        self._buf = io.BytesIO(body)
        self.headers = {}
        if content_length is not None:
            self.headers["Content-Length"] = content_length

    def read(self, n=-1):
        return self._buf.read(n)

    def __enter__(self):
        return self

    def __exit__(self, *a):
        return False


def response_for(spec, url):
    """spec: {"kind": "body", "body": b64, "content_length": str|None}
    | {"kind": "status", "code": int, "reason": str}
    | {"kind": "url", "reason": str} | {"kind": "other", "msg": str}"""
    kind = spec["kind"]
    if kind == "body":
        body = base64.b64decode(spec["body"])
        cl = spec.get("content_length", "auto")
        if cl == "auto":
            cl = str(len(body))
        return FakeResponse(body, cl)
    if kind == "status":
        raise urllib.error.HTTPError(url, spec["code"], spec["reason"], {}, None)
    if kind == "url":
        raise urllib.error.URLError(spec["reason"])
    if kind == "other":
        raise TimeoutError(spec["msg"])
    raise AssertionError(kind)


def install_urlopen(routes):
    calls = []

    def fake_urlopen(req, timeout=None):
        url = req.full_url
        calls.append({"url": url, "headers": sorted(req.header_items()), "timeout": timeout})
        if url not in routes:
            raise urllib.error.URLError("no route for %s" % url)
        return response_for(routes[url], url)

    updater.urllib.request.urlopen = fake_urlopen
    return calls


def json_body(obj):
    return {"kind": "body", "body": b64(json.dumps(obj).encode("utf-8"))}


def raw_body(data):
    return {"kind": "body", "body": b64(data)}


# ---------------------------------------------------------------------------
# Versions
# ---------------------------------------------------------------------------

VERSION_INPUTS = [
    "", "v1.2.3", "1.2", "1", "v1.2.3-beta", "V1.2.3", " v0.2.1 ", "0.2.1", "x1",
    "1..2", "01.002.0003", "99999999999999999999.1", "\u0661.\u0662.\u0663",
    "\U0001d7cf.\U0001d7d0", "v", "1.2.3.4", "\t2.0\n", "v0.3.0", "0.2.10", "vv1",
    "1.a", "  ", "\u00a01.0", "0.2.1rc1", "-1.0", "1.-2",
]


def version_cases():
    out = []
    for text in VERSION_INPUTS:
        v = updater.parse_version(text)
        out.append({"text": text, "parsed": None if v is None else [str(x) for x in v]})
    return out


NEWER_PAIRS = [
    ("v0.3.0", "0.2.1"), ("0.2.1", "0.2.1"), ("v0.2.0", "0.2.1"), ("v0.10.0", "0.9.9"),
    ("garbage", "0.2.1"), ("v1", "0.2.1"), ("0.2.1", "garbage"), ("", "0.2.1"),
    ("v0.2.1-beta", "0.2.1"), ("v0.2.2-beta", "0.2.1"), ("5", "0.2.1"), ("True", "0.2.1"),
    ("1.5", "0.2.1"), ("99999999999999999999", "1"),
]


def newer_cases():
    return [{"remote": r, "local": l, "newer": updater.is_newer(r, l)} for r, l in NEWER_PAIRS]


# ---------------------------------------------------------------------------
# check_latest
# ---------------------------------------------------------------------------

ASSET = updater.ASSET_NAME
GOOD_ASSET = {"name": ASSET, "browser_download_url": "https://example.invalid/b.zip", "size": 1234}

CHECK_CASES = [
    ("valid", json_body({"tag_name": "v0.3.0", "assets": [GOOD_ASSET]})),
    ("valid_second_asset", json_body({"tag_name": "v0.3.0", "assets": [
        {"name": "other.zip", "browser_download_url": "https://x/o"}, GOOD_ASSET,
        {"name": ASSET, "browser_download_url": "https://x/later"}]})),
    ("no_assets", json_body({"tag_name": "v0.3.0"})),
    ("assets_null", json_body({"tag_name": "v0.3.0", "assets": None})),
    ("assets_empty", json_body({"tag_name": "v0.3.0", "assets": []})),
    ("wrong_asset_name", json_body({"tag_name": "v0.3.0", "assets": [{"name": "x.zip", "browser_download_url": "u"}]})),
    ("assets_dict", json_body({"tag_name": "v0.3.0", "assets": {"name": ASSET}})),
    ("assets_str", json_body({"tag_name": "v0.3.0", "assets": "abc"})),
    ("assets_int", json_body({"tag_name": "v0.3.0", "assets": 7})),
    ("assets_true", json_body({"tag_name": "v0.3.0", "assets": True})),
    ("assets_zero", json_body({"tag_name": "v0.3.0", "assets": 0})),
    ("asset_not_dict", json_body({"tag_name": "v0.3.0", "assets": [ASSET, 5, None]})),
    ("tag_missing", json_body({"assets": [GOOD_ASSET]})),
    ("tag_empty", json_body({"tag_name": "", "assets": [GOOD_ASSET]})),
    ("tag_null", json_body({"tag_name": None, "assets": [GOOD_ASSET]})),
    ("tag_int", json_body({"tag_name": 5, "assets": [GOOD_ASSET]})),
    ("tag_list", json_body({"tag_name": [1], "assets": [GOOD_ASSET]})),
    ("tag_upper_v", json_body({"tag_name": "VvV0.4.0", "assets": [GOOD_ASSET]})),
    ("tag_quote", json_body({"tag_name": "it's", "assets": []})),
    ("url_missing", json_body({"tag_name": "v0.3.0", "assets": [{"name": ASSET, "size": 3}]})),
    ("url_empty", json_body({"tag_name": "v0.3.0", "assets": [{"name": ASSET, "browser_download_url": ""}]})),
    ("url_int", json_body({"tag_name": "v0.3.0", "assets": [{"name": ASSET, "browser_download_url": 9}]})),
    ("size_missing", json_body({"tag_name": "v0.3.0", "assets": [{"name": ASSET, "browser_download_url": "u"}]})),
    ("size_null", json_body({"tag_name": "v0.3.0", "assets": [{"name": ASSET, "browser_download_url": "u", "size": None}]})),
    ("size_str", json_body({"tag_name": "v0.3.0", "assets": [{"name": ASSET, "browser_download_url": "u", "size": "12"}]})),
    ("not_dict", json_body([1, 2])),
    ("json_string", json_body("hello")),
    ("invalid_json", raw_body(b"{")),
    ("empty_body", raw_body(b"")),
    ("bom_json", raw_body(b"\xef\xbb\xbf" + json.dumps({"tag_name": "v1", "assets": [GOOD_ASSET]}).encode())),
    ("bad_utf8", raw_body(b'{"tag_name": "\xff"}')),
    ("bad_utf8_cont", raw_body(b'{"t": "\xe4\xb8x"}')),
    ("truncated_utf8", raw_body(b'{"t": "\xe4\xb8')),
    ("http_403", {"kind": "status", "code": 403, "reason": "rate limit exceeded"}),
    ("http_500", {"kind": "status", "code": 500, "reason": "Internal Server Error"}),
    ("url_error", {"kind": "url", "reason": "[Errno 11001] getaddrinfo failed"}),
    ("timeout", {"kind": "other", "msg": "timed out"}),
]


def check_cases():
    out = []
    for name, spec in CHECK_CASES:
        calls = install_urlopen({updater.LATEST_URL: spec})
        CAPTURE.records.clear()
        try:
            rel = updater.check_latest(timeout=8)
            result = {
                "ok": None
                if rel is None
                else {
                    "tag": repr(rel.tag),
                    "url": repr(rel.url),
                    "size": repr(rel.size),
                    "version": _outcome(lambda: rel.version),
                }
            }
        except Exception as e:  # noqa: BLE001
            result = {"err": str(e)}
        out.append(
            {
                "name": name,
                "response": spec,
                "result": result,
                "logs": list(CAPTURE.records),
                "request_headers": [list(h) for h in calls[0]["headers"]] if calls else None,
                "timeout": calls[0]["timeout"] if calls else None,
            }
        )
    return out


def _outcome(fn):
    try:
        return {"ok": fn()}
    except Exception as e:  # noqa: BLE001
        return {"err": str(e)}


# ---------------------------------------------------------------------------
# Bundles, download and stage
# ---------------------------------------------------------------------------

MOD_ZIP_BYTES = make_zip([("bnvda_mod_version.json", b'{"version": "0.3.0"}'), ("lua/x.lua", b"-- x\n")])

GOOD_BUNDLE = make_zip(
    [
        ("beamtel.exe", b"MZ fake exe"),
        ("README.md", b"# readme\n"),
        ("bng_screenreader_mod.zip", MOD_ZIP_BYTES),
        ("docs/", None),
        ("docs/sub/notes.txt", b"notes"),
    ]
)


def bundle_with(extra, keep=("beamtel.exe", "bng_screenreader_mod.zip")):
    members = []
    if "beamtel.exe" in keep:
        members.append(("beamtel.exe", b"MZ"))
    if "bng_screenreader_mod.zip" in keep:
        members.append(("bng_screenreader_mod.zip", MOD_ZIP_BYTES))
    return make_zip(members + extra)


def corrupt_crc_bundle():
    data = bytearray(make_zip([("beamtel.exe", b"AAAAAAAAAAAAAAAA"), ("bng_screenreader_mod.zip", b"BBBB")], compress=zipfile.ZIP_STORED))
    i = data.find(b"AAAAAAAAAAAAAAAA")
    data[i] = ord("Z")
    return bytes(data)


STAGE_CASES = [
    ("good", GOOD_BUNDLE),
    ("parent_dir", bundle_with([("../evil.txt", b"x")])),
    ("parent_mid", bundle_with([("a/../b.txt", b"x")])),
    ("abs_slash", bundle_with([("/abs.txt", b"x")])),
    ("abs_drive", bundle_with([("C:/abs.txt", b"x")])),
    ("drive_relative", bundle_with([("C:rel.txt", b"x")])),
    ("backslash_name", bundle_with([("dir\\file.txt", b"x")])),
    ("illegal_chars", bundle_with([("we:ird?<n>.txt", b"x"), ('q"u|o*te.txt', b"y")])),
    ("trailing_dots", bundle_with([("name. .", b"x"), ("dir. /f.txt", b"y")])),
    ("dot_components", bundle_with([("./x.txt", b"x"), ("a/./b//c.txt", b"y")])),
    ("empty_name", bundle_with([(".", b"x")])),
    ("missing_exe", bundle_with([("README.md", b"r")], keep=("bng_screenreader_mod.zip",))),
    ("missing_both", make_zip([("README.md", b"r")])),
    ("exe_is_dir", make_zip([("beamtel.exe/", None), ("bng_screenreader_mod.zip", b"z")])),
    ("not_zip", b"<html>not a zip</html>"),
    ("dir_entries_only", make_zip([("a/", None), ("a/b/", None)])),
]


def stage_cases():
    out = []
    for name, data in STAGE_CASES:
        root = tempfile.mkdtemp(prefix="e10stage-")
        zip_path = os.path.join(root, "bundle.zip")
        with open(zip_path, "wb") as fh:
            fh.write(data)
        staged = os.path.join(root, "staged")
        res = _outcome(lambda: updater.stage(zip_path, staged))
        out.append(
            {
                "name": name,
                "zip": b64(data),
                "result": {k: scrub(v, root) for k, v in res.items()},
                "tree": tree(staged),
            }
        )
        shutil.rmtree(root, ignore_errors=True)
    return out


DOWNLOAD_CASES = [
    ("good", {"kind": "body", "body": b64(GOOD_BUNDLE)}, "https://example.invalid/b.zip", False),
    ("good_no_length", {"kind": "body", "body": b64(GOOD_BUNDLE), "content_length": None}, "https://example.invalid/b.zip", False),
    ("big_body", {"kind": "body", "body": b64(bundle_with([("pad.bin", bytes(range(256)) * 600)]))}, "https://example.invalid/b.zip", False),
    ("not_zip", raw_body(b"<html>404</html>"), "https://example.invalid/b.zip", False),
    ("missing_mod", {"kind": "body", "body": b64(make_zip([("beamtel.exe", b"x")]))}, "https://example.invalid/b.zip", False),
    ("missing_both", {"kind": "body", "body": b64(make_zip([("x", b"x")]))}, "https://example.invalid/b.zip", False),
    ("corrupt_crc", {"kind": "body", "body": b64(corrupt_crc_bundle())}, "https://example.invalid/b.zip", False),
    ("http_404", {"kind": "status", "code": 404, "reason": "Not Found"}, "https://example.invalid/b.zip", False),
    ("url_error", {"kind": "url", "reason": "[Errno 11001] getaddrinfo failed"}, "https://example.invalid/b.zip", False),
    ("bad_length", {"kind": "body", "body": b64(GOOD_BUNDLE), "content_length": "abc"}, "https://example.invalid/b.zip", False),
    ("spaced_length", {"kind": "body", "body": b64(GOOD_BUNDLE), "content_length": " 1_0 "}, "https://example.invalid/b.zip", False),
    ("empty_length", {"kind": "body", "body": b64(GOOD_BUNDLE), "content_length": ""}, "https://example.invalid/b.zip", False),
    ("cancelled", {"kind": "body", "body": b64(GOOD_BUNDLE)}, "https://example.invalid/b.zip", True),
    ("not_a_url", {"kind": "body", "body": b64(GOOD_BUNDLE)}, "not a url", False),
    ("wrapped_url", {"kind": "body", "body": b64(GOOD_BUNDLE)}, " <URL:https://example.invalid/b.zip> ", False),
]


def download_cases():
    out = []
    for name, spec, url, cancel in DOWNLOAD_CASES:
        root = tempfile.mkdtemp(prefix="e10dl-")
        dest = os.path.join(root, "update", ASSET)
        key = url.strip().strip("<>").replace("URL:", "")
        calls = install_urlopen({key: spec})
        progress = []
        res = _outcome(
            lambda: updater.download(
                url,
                dest,
                progress=lambda d, t: progress.append([d, t]),
                cancelled=(lambda: cancel),
            )
        )
        out.append(
            {
                "name": name,
                "url": url,
                "response": spec,
                "cancel": cancel,
                "result": {k: scrub(v, root) for k, v in res.items()},
                "progress": progress,
                "dest_exists": os.path.isfile(dest),
                "part_exists": os.path.isfile(dest + ".part"),
                "request_headers": [list(h) for h in calls[0]["headers"]] if calls else None,
                "timeout": calls[0]["timeout"] if calls else None,
            }
        )
        shutil.rmtree(root, ignore_errors=True)
    return out


# ---------------------------------------------------------------------------
# Helper script and command line
# ---------------------------------------------------------------------------


def helper_cases():
    out = {}
    out["template_body"] = (
        updater._HELPER_TEMPLATE.replace("@PID@", "4242")
        .replace("@PROG@", r"C:\Program Files\BEAM")
        .replace("@STAGED@", r"C:\Users\x\AppData\Local\beamtel\update\staged")
        .replace("@LOG@", r"C:\Users\x\AppData\Local\beamtel\update\apply_update.log")
    )
    root = tempfile.mkdtemp(prefix="e10helper-")
    prog = os.path.join(root, "Program Files", "BEAM")
    staged = os.path.join(root, "update", "staged")
    update_dir = os.path.join(root, "update")
    real_getpid = os.getpid
    os.getpid = lambda: 4242
    try:
        path = updater.write_helper(staged, prog=prog, update_dir=update_dir)
    finally:
        os.getpid = real_getpid
    with open(path, "rb") as fh:
        data = fh.read()
    out["written"] = {
        "path": scrub(path, root),
        "bytes": scrub(data.decode("ascii"), root),
    }
    shutil.rmtree(root, ignore_errors=True)

    encode = []
    for text in ["a\n\u00e9", "abc\n\n\u00f1\u00e9 x", "x\U0001f600", "\u0101b", "\u00e9\n\u00e9", "plain\n"]:
        root = tempfile.mkdtemp(prefix="e10enc-")
        p = os.path.join(root, "h.cmd")
        try:
            with open(p, "w", encoding="ascii", newline="\r\n") as f:
                f.write(text)
            with open(p, "rb") as fh:
                encode.append({"text": text, "ok": b64(fh.read())})
        except UnicodeEncodeError as e:
            encode.append({"text": text, "err": str(e)})
        shutil.rmtree(root, ignore_errors=True)
    out["encode"] = encode

    seqs = [
        [r"C:\Windows\System32\cmd.exe", "/d", "/c", r"C:\Users\x\AppData\Local\beamtel\update\apply_update.cmd"],
        [r"C:\Windows\System32\cmd.exe", "/d", "/c", r"C:\Users\John Smith\AppData\Local\beamtel\update\apply_update.cmd"],
        ["a b\\", "", "q\"uote", "back\\\\\"slash", "tab\there", "end\\\\"],
    ]
    out["list2cmdline"] = [{"seq": s, "line": subprocess.list2cmdline(s)} for s in seqs]
    return out


# ---------------------------------------------------------------------------
# Mod installer
# ---------------------------------------------------------------------------


def stamped_zip(version_json, extra=b""):
    members = [("lua/x.lua", b"-- x\n" + extra)]
    if version_json is not None:
        members.append(("bnvda_mod_version.json", version_json))
    return make_zip(members)


MOD_VERSION_CASES = [
    ("stamped", stamped_zip(b'{"version": "0.3.0"}')),
    ("stamped_v", stamped_zip(b'{"version": "v1.2"}')),
    ("unstamped", stamped_zip(None)),
    ("not_dict", stamped_zip(b'["0.3.0"]')),
    ("version_int", stamped_zip(b'{"version": 3}')),
    ("version_garbage", stamped_zip(b'{"version": "beta"}')),
    ("invalid_json", stamped_zip(b"{")),
    ("bad_utf8", stamped_zip(b'{"version": "\xff"}')),
    ("not_zip", b"nope"),
]


def mod_version_cases():
    out = []
    for name, data in MOD_VERSION_CASES:
        root = tempfile.mkdtemp(prefix="e10mv-")
        p = os.path.join(root, "m.zip")
        with open(p, "wb") as fh:
            fh.write(data)
        v = config_ui._mod_zip_version(p)
        out.append({"name": name, "zip": b64(data), "result": None if v is None else [[str(x) for x in v[0]], v[1]]})
        shutil.rmtree(root, ignore_errors=True)
    return out


def layout_doc(entries, extra_apps=()):
    apps = list(extra_apps) + [{"appName": "bnvdaHook", "placement": {"x": i}} for i in range(entries)]
    return {"apps": apps, "version": 1.5, "name": "Freeroam \u00e9"}


SRC_NEW = stamped_zip(b'{"version": "0.3.0"}')
SRC_OLD = stamped_zip(b'{"version": "0.1.0"}')
DST_MID = stamped_zip(b'{"version": "0.2.0"}', extra=b"dst")
UNSTAMPED_A = stamped_zip(None, extra=b"a")
UNSTAMPED_B = stamped_zip(None, extra=b"b")

# (name, src bytes or None, mods dir exists, dst bytes / "DIR" / None, layout bytes or None, answers)
MOD_CASES = [
    ("no_src", None, True, None, None, []),
    ("no_mods_dir", SRC_NEW, False, None, None, []),
    ("fresh", SRC_NEW, True, None, None, []),
    ("identical", SRC_NEW, True, SRC_NEW, None, []),
    ("newer", SRC_NEW, True, DST_MID, None, []),
    ("same_version_rebuilt", stamped_zip(b'{"version": "0.2.0"}'), True, DST_MID, None, []),
    ("older_declined", SRC_OLD, True, DST_MID, None, ["no"]),
    ("older_accepted", SRC_OLD, True, DST_MID, None, ["yes"]),
    ("both_unstamped_yes", UNSTAMPED_A, True, UNSTAMPED_B, None, ["yes"]),
    ("both_unstamped_no", UNSTAMPED_A, True, UNSTAMPED_B, None, ["no"]),
    ("src_unstamped_dst_stamped", UNSTAMPED_A, True, DST_MID, None, ["no"]),
    ("src_stamped_dst_unstamped", SRC_NEW, True, UNSTAMPED_B, None, []),
    ("dst_is_dir", SRC_NEW, True, "DIR", None, []),
    ("layout_invalid_json", SRC_NEW, True, None, b"{not json", []),
    ("layout_empty", SRC_NEW, True, None, b"", []),
    ("layout_not_object", SRC_NEW, True, None, b"[1, 2]", []),
    ("layout_no_apps", SRC_NEW, True, None, b'{"apps": {"a": 1}}', []),
    ("layout_bad_utf8", SRC_NEW, True, None, b'{"apps": ["\xff"]}', []),
    ("layout_zero", SRC_NEW, True, None, json.dumps(layout_doc(0)).encode(), []),
    ("layout_one_no", SRC_NEW, True, None, json.dumps(layout_doc(1)).encode(), ["no"]),
    ("layout_two_yes", SRC_NEW, True, None, json.dumps(layout_doc(2, [{"appName": "other"}, "str", {"appName": "bnvdaHook2"}])).encode(), ["yes"]),
    ("layout_bom_yes", SRC_NEW, True, SRC_NEW, b"\xef\xbb\xbf" + json.dumps(layout_doc(1), ensure_ascii=False).encode("utf-8"), ["yes"]),
    ("layout_floats_yes", SRC_NEW, True, None, b'{"apps": [{"appName": "bnvdaHook"}, {"w": 1e100, "h": 0.1, "big": 123456789012345678901234567890, "neg": -0.0, "s": "\\u2603 \\n"}], "z": []}', ["yes"]),
]


def mod_cases():
    out = []
    real_get_program_dir = config_ui._get_program_dir
    real_box = wx.MessageBox
    try:
        for name, src, mods_exists, dst, layout, answers in MOD_CASES:
            root = tempfile.mkdtemp(prefix="e10mod-")
            prog = os.path.join(root, "prog")
            lad = os.path.join(root, "lad")
            os.makedirs(prog)
            os.makedirs(lad)
            if src is not None:
                with open(os.path.join(prog, "bng_screenreader_mod.zip"), "wb") as fh:
                    fh.write(src)
            mods = os.path.join(lad, "BeamNG", "BeamNG.drive", "current", "mods")
            if mods_exists:
                os.makedirs(mods)
                if dst == "DIR":
                    os.makedirs(os.path.join(mods, "bng_screenreader_mod.zip"))
                elif dst is not None:
                    with open(os.path.join(mods, "bng_screenreader_mod.zip"), "wb") as fh:
                        fh.write(dst)
            layout_path = config_ui._freeroam_layout_path(lad)
            if layout is not None:
                os.makedirs(os.path.dirname(layout_path))
                with open(layout_path, "wb") as fh:
                    fh.write(layout)
            rec = Recorder(root, answers)
            wx.MessageBox = rec.message_box
            config_ui._get_program_dir = lambda prog=prog: prog
            saved = os.environ.get("LOCALAPPDATA")
            os.environ["LOCALAPPDATA"] = lad
            try:
                result = config_ui.install_mod_interactive(None)
            finally:
                os.environ["LOCALAPPDATA"] = saved
            out.append(
                {
                    "name": name,
                    "src": None if src is None else b64(src),
                    "mods_exists": mods_exists,
                    "dst": dst if dst in (None, "DIR") else b64(dst),
                    "layout": None if layout is None else b64(layout),
                    "answers": answers,
                    "result": result,
                    "events": rec.events,
                    "mods_tree": tree(mods),
                    "layout_tree": {
                        scrub(k, root): v
                        for k, v in (tree(os.path.dirname(layout_path)) or {}).items()
                    }
                    if layout is not None
                    else None,
                }
            )
            shutil.rmtree(root, ignore_errors=True)
    finally:
        config_ui._get_program_dir = real_get_program_dir
        wx.MessageBox = real_box
    return out


# ---------------------------------------------------------------------------
# The flow
# ---------------------------------------------------------------------------

NEW_TAG_JSON = lambda tag="v0.3.0", url="https://example.invalid/b.zip": json_body(  # noqa: E731
    {"tag_name": tag, "assets": [{"name": ASSET, "browser_download_url": url, "size": 10}]}
)
ASSET_URL = "https://example.invalid/b.zip"
GOOD_DL = {"kind": "body", "body": b64(GOOD_BUNDLE)}

# Each scenario: manual, config (extra keys), check response, download response,
# answers, cancel, popen ("ok" | "raise"), mod install setup ("none" | "ready").
FLOW_CASES = [
    dict(name="manual_check_http_error", manual=True, check={"kind": "status", "code": 500, "reason": "x"}),
    dict(name="manual_check_unreachable", manual=True, check={"kind": "url", "reason": "offline"}),
    dict(name="manual_up_to_date", manual=True, check=NEW_TAG_JSON("v0.2.1")),
    dict(name="manual_older_remote", manual=True, check=NEW_TAG_JSON("v0.1.0")),
    dict(name="startup_disabled", config={"update_check_enabled": False}, check=NEW_TAG_JSON()),
    dict(name="manual_ignores_disabled", manual=True, config={"update_check_enabled": False}, check=NEW_TAG_JSON("v0.2.1")),
    dict(name="startup_up_to_date", check=NEW_TAG_JSON("v0.2.0")),
    dict(name="startup_check_fails_silently", check={"kind": "status", "code": 403, "reason": "x"}),
    dict(name="startup_decline", check=NEW_TAG_JSON(), answers=["no"]),
    dict(name="download_404", check=NEW_TAG_JSON(), download={"kind": "status", "code": 404, "reason": "Not Found"}),
    dict(name="download_not_zip", check=NEW_TAG_JSON(), download=raw_body(b"<html/>")),
    dict(name="download_missing_member", check=NEW_TAG_JSON(), download=raw_body(make_zip([("beamtel.exe", b"x")]))),
    dict(name="stage_zip_slip", check=NEW_TAG_JSON(), download=raw_body(bundle_with([("../evil.txt", b"x")]))),
    dict(name="success", check=NEW_TAG_JSON(), download=GOOD_DL),
    dict(name="success_manual", manual=True, check=NEW_TAG_JSON("V0.4.0-rc1"), download=GOOD_DL),
    dict(name="popen_fails", check=NEW_TAG_JSON(), download=GOOD_DL, popen="raise"),
    dict(name="cancelled", check=NEW_TAG_JSON(), download=GOOD_DL, cancel=True),
    dict(name="phase_two_decline", config={"pending_update_version": "0.2.1"}, answers=["no"]),
    dict(name="phase_two_install", config={"pending_update_version": " v0.2.1 "}, answers=["yes"], mods="ready"),
    dict(name="phase_two_install_no_zip", config={"pending_update_version": "0.2.1"}, answers=["yes"]),
    dict(name="stale_pending", config={"pending_update_version": "0.2.0"}, check=NEW_TAG_JSON("v0.2.1")),
    dict(name="stale_pending_garbage", config={"pending_update_version": "garbage"}, check={"kind": "url", "reason": "offline"}),
    dict(name="manual_with_pending", manual=True, config={"pending_update_version": "0.2.1"}, check=NEW_TAG_JSON("v0.2.1")),
    dict(name="tag_int", check=json_body({"tag_name": 5, "assets": [{"name": ASSET, "browser_download_url": ASSET_URL}]})),
    dict(name="tag_true", manual=True, check=json_body({"tag_name": True, "assets": [{"name": ASSET, "browser_download_url": ASSET_URL}]})),
    dict(name="assets_not_iterable", manual=True, check=json_body({"tag_name": "v9", "assets": 7})),
    dict(name="url_not_a_url", check=NEW_TAG_JSON(url="not a url")),
]


def flow_cases():
    out = []
    real_box = wx.MessageBox
    real_get_program_dir = config_ui._get_program_dir
    real_popen = updater.subprocess.Popen
    real_rwp = updater._run_with_progress
    real_program_dir = updater.program_dir
    saved = dict(
        cfg_u=updater.CONFIG_PATH,
        cfg_c=configurator.CONFIG_PATH,
        upd=updater.UPDATE_DIR,
        stage_def=updater.stage.__defaults__,
        apply_def=updater.apply_and_restart.__defaults__,
    )
    real_getpid = os.getpid
    real_sysroot = os.environ.get("SystemRoot")
    try:
        for case in FLOW_CASES:
            root = tempfile.mkdtemp(prefix="e10flow-")
            cfg_path = os.path.join(root, "config", "beamtel_config.json")
            upd = os.path.join(root, "config", "update")
            staged = os.path.join(upd, "staged")
            prog = os.path.join(root, "prog")
            lad = os.path.join(root, "lad")
            os.makedirs(os.path.dirname(cfg_path))
            os.makedirs(prog)
            os.makedirs(lad)
            cfg = dict(case.get("config", {}))
            if cfg:
                with open(cfg_path, "w", encoding="utf-8") as fh:
                    json.dump(cfg, fh)
            if case.get("mods") == "ready":
                with open(os.path.join(prog, "bng_screenreader_mod.zip"), "wb") as fh:
                    fh.write(SRC_NEW)
                os.makedirs(os.path.join(lad, "BeamNG", "BeamNG.drive", "current", "mods"))

            updater.CONFIG_PATH = cfg_path
            configurator.CONFIG_PATH = cfg_path
            updater.UPDATE_DIR = upd
            updater.stage.__defaults__ = (staged,)
            updater.apply_and_restart.__defaults__ = (staged,)
            updater.program_dir = lambda prog=prog: prog
            config_ui._get_program_dir = lambda prog=prog: prog
            os.getpid = lambda: 4242
            os.environ["SystemRoot"] = r"C:\Windows"

            rec = Recorder(root, case.get("answers", []))
            wx.MessageBox = rec.message_box
            routes = {}
            if "check" in case:
                routes[updater.LATEST_URL] = case["check"]
            if "download" in case:
                routes[ASSET_URL] = case["download"]
            install_urlopen(routes)

            cancel = bool(case.get("cancel"))

            def fake_rwp(parent, title, message, work, cancellable=False, rec=rec, cancel=cancel):
                rec.events.append({"event": "progress", "title": title, "text": message, "cancellable": cancellable})
                st = {"cancel": cancel and cancellable, "result": None, "error": None}
                try:
                    st["result"], st["error"] = work(lambda d, t: None, lambda: st["cancel"])
                except Exception as e:  # noqa: BLE001
                    st["error"] = str(e)
                if st["cancel"] and not st["error"]:
                    st["error"] = "cancelled"
                return st["result"], st["error"]

            updater._run_with_progress = fake_rwp

            popen_calls = []

            def fake_popen(args, cwd=None, creationflags=0, rec=rec, case=case, **kw):
                popen_calls.append(
                    {
                        "args": [scrub(a, root) for a in args],
                        "line": scrub(subprocess.list2cmdline(args), root),
                        "cwd": scrub(cwd, root),
                        "creationflags": creationflags,
                        "stdin": kw.get("stdin") == subprocess.DEVNULL,
                        "stdout": kw.get("stdout") == subprocess.DEVNULL,
                        "stderr": kw.get("stderr") == subprocess.DEVNULL,
                        "close_fds": kw.get("close_fds"),
                    }
                )
                if case.get("popen") == "raise":
                    raise OSError("boom")
                return object()

            updater.subprocess.Popen = fake_popen

            gate_calls = []

            class Gate:
                def allow(self):
                    gate_calls.append("allow")

                def deny(self):
                    gate_calls.append("deny")

            class Frame:
                def Close(self, force=False):
                    rec.events.append({"event": "exit"})

            saved_lad = os.environ.get("LOCALAPPDATA")
            os.environ["LOCALAPPDATA"] = lad
            CAPTURE.records.clear()
            try:
                updater.run_startup_flow(Frame(), Gate(), manual=bool(case.get("manual")))
            finally:
                os.environ["LOCALAPPDATA"] = saved_lad
            with open(cfg_path, "r", encoding="utf-8") as fh:
                final_cfg = json.load(fh)
            helper_path = os.path.join(upd, updater.HELPER_NAME)
            helper = None
            if os.path.isfile(helper_path):
                with open(helper_path, "rb") as fh:
                    helper = scrub(fh.read().decode("ascii"), root)
            out.append(
                {
                    "case": {k: v for k, v in case.items() if k != "name"},
                    "name": case["name"],
                    "events": rec.events,
                    "logs": [[lvl, scrub(m, root)] for lvl, m in CAPTURE.records],
                    "gate": gate_calls,
                    "popen": popen_calls,
                    "pending_after": final_cfg.get("pending_update_version"),
                    "helper": helper,
                    "staged_tree": tree(staged),
                    "zip_exists": os.path.isfile(os.path.join(upd, ASSET)),
                    "mods_tree": tree(os.path.join(lad, "BeamNG", "BeamNG.drive", "current", "mods")),
                }
            )
            shutil.rmtree(root, ignore_errors=True)
    finally:
        wx.MessageBox = real_box
        config_ui._get_program_dir = real_get_program_dir
        updater.subprocess.Popen = real_popen
        updater._run_with_progress = real_rwp
        updater.program_dir = real_program_dir
        updater.CONFIG_PATH = saved["cfg_u"]
        configurator.CONFIG_PATH = saved["cfg_c"]
        updater.UPDATE_DIR = saved["upd"]
        updater.stage.__defaults__ = saved["stage_def"]
        updater.apply_and_restart.__defaults__ = saved["apply_def"]
        os.getpid = real_getpid
        if real_sysroot is not None:
            os.environ["SystemRoot"] = real_sysroot
    return out


def main():
    data = {
        "app_version": updater.APP_VERSION,
        "constants": {
            "REPO": updater.REPO,
            "LATEST_URL": updater.LATEST_URL,
            "ASSET_NAME": updater.ASSET_NAME,
            "REQUIRED_MEMBERS": list(updater.REQUIRED_MEMBERS),
            "HELPER_NAME": updater.HELPER_NAME,
            "HELPER_LOG": updater.HELPER_LOG,
            "USER_AGENT": updater.USER_AGENT,
            "CHECK_TIMEOUT_S": updater.CHECK_TIMEOUT_S,
            "DOWNLOAD_TIMEOUT_S": updater.DOWNLOAD_TIMEOUT_S,
            "PENDING_KEY": updater.PENDING_KEY,
            "ENABLED_KEY": updater.ENABLED_KEY,
            "MOD_ZIP": config_ui._MOD_ZIP,
            "MOD_VERSION_MEMBER": config_ui._MOD_VERSION_MEMBER,
            "BNVDA_APP_NAME": config_ui._BNVDA_APP_NAME,
            "freeroam_layout_path": config_ui._freeroam_layout_path(r"C:\LAD"),
        },
        "versions": version_cases(),
        "newer": newer_cases(),
        "check_latest": check_cases(),
        "stage": stage_cases(),
        "download": download_cases(),
        "helper": helper_cases(),
        "mod_version": mod_version_cases(),
        "mod_install": mod_cases(),
        "flow": flow_cases(),
    }
    write_fixture("e10_updater", data)


if __name__ == "__main__":
    try:
        main()
    finally:
        logging.shutdown()
        shutil.rmtree(_SCRATCH, ignore_errors=True)
