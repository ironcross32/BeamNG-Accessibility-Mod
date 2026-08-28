"""At-rest protection for the secrets in beamtel_config.json.

The config file is a plain JSON document in %LOCALAPPDATA%\beamtel, readable by
anything running as the user -- and an AI describer key is a billable credential,
not a preference. Windows already owns the only key store that needs no key of
our own: DPAPI's CryptProtectData ties the ciphertext to the *user account*, so
a copied config file, a synced profile or a backup carries nothing usable, while
beamtel itself needs no passphrase, no keyring and no first-run prompt.

The scope is deliberately the user and not the machine (no
CRYPTPROTECT_LOCAL_MACHINE): another account on the same PC must not be able to
read the key, and the mod only ever runs as the person who set it.

This is at-rest protection, not secrecy from code running as the user -- anything
with that privilege can call CryptUnprotectData too. It is the same guarantee the
browsers give a saved password, and it is the one that matters here: the key stops
being visible in a file people paste into bug reports.
"""

import base64
import ctypes
from ctypes import wintypes

from bnh_logger import get_logger

logger = get_logger()

# Version the marker rather than the payload: the day this has to move off DPAPI
# (a Linux port, a different scope) the reader must be able to tell the two
# ciphertexts apart, and a bare base64 blob cannot say what produced it.
PREFIX = "dpapi:v1:"

# Bound into the ciphertext, so a blob lifted out of this config cannot be
# decrypted by handing it to some other DPAPI consumer running as the same user.
# It is a constant in open source and therefore not itself a secret -- it narrows
# the blast radius, it does not add strength.
_ENTROPY = b"beamtel:config:v1"

CRYPTPROTECT_UI_FORBIDDEN = 0x01

# Setting NAMES whose values are secrets. Matched as a substring because the names
# are compound (`ai_describer_openai_api_key`), and by name rather than by value so
# that a provider added later is covered the day it is added rather than the day
# someone notices. `mcp_server.py` masks its `get_config` output off this same list.
SECRET_NAME_PARTS = ("api_key", "apikey", "token", "secret", "password", "passwd")


def is_secret_setting(name):
    low = str(name).lower()
    return any(part in low for part in SECRET_NAME_PARTS)


class _Blob(ctypes.Structure):
    _fields_ = [("cbData", wintypes.DWORD), ("pbData", ctypes.POINTER(ctypes.c_char))]


def _blob_in(data):
    buf = ctypes.create_string_buffer(data, len(data))
    return _Blob(len(data), ctypes.cast(buf, ctypes.POINTER(ctypes.c_char))), buf


def _blob_bytes(blob):
    return ctypes.string_at(blob.pbData, blob.cbData)


def _crypt32():
    try:
        return ctypes.windll.crypt32, ctypes.windll.kernel32
    except (AttributeError, OSError):
        return None, None


def available():
    """Whether DPAPI can be reached at all (it cannot off Windows)."""
    crypt32, _k32 = _crypt32()
    return crypt32 is not None


def is_protected(value):
    return isinstance(value, str) and value.startswith(PREFIX)


def protect(plaintext):
    """Encrypt `plaintext` for this Windows user. Returns a `PREFIX`-marked string.

    Falls back to returning the plaintext unchanged if DPAPI is unreachable --
    refusing to store the key at all would turn a hardening measure into a
    feature outage, and the caller has just validated that key against a live
    endpoint.
    """
    text = (plaintext or "").strip()
    if not text or is_protected(text):
        return text
    crypt32, kernel32 = _crypt32()
    if crypt32 is None:
        logger.warning("DPAPI unavailable; storing config secret unencrypted.")
        return text
    data_in, _keep = _blob_in(text.encode("utf-8"))
    ent_in, _keep_ent = _blob_in(_ENTROPY)
    out = _Blob()
    ok = crypt32.CryptProtectData(
        ctypes.byref(data_in), None, ctypes.byref(ent_in),
        None, None, CRYPTPROTECT_UI_FORBIDDEN, ctypes.byref(out),
    )
    if not ok:
        logger.error(
            "CryptProtectData failed (%s); storing config secret unencrypted."
            % ctypes.GetLastError()
        )
        return text
    try:
        blob = _blob_bytes(out)
    finally:
        kernel32.LocalFree(out.pbData)
    return PREFIX + base64.b64encode(blob).decode("ascii")


def unprotect(value):
    """Decrypt a stored secret.

    Returns the plaintext; `""` for an empty value; and **None** when a protected
    value could not be decrypted. None is a distinct answer on purpose: a blob
    that came from another Windows account (a copied config, a restored profile)
    is not the same situation as no key configured, and only the caller can say
    the one thing that helps -- set it again.

    A value with no marker is returned as-is, which is what carries configs
    written before this existed.
    """
    if not value:
        return ""
    if not is_protected(value):
        return value
    crypt32, kernel32 = _crypt32()
    if crypt32 is None:
        logger.error("Config secret is DPAPI-protected but DPAPI is unavailable.")
        return None
    try:
        raw = base64.b64decode(value[len(PREFIX):].encode("ascii"))
    except Exception:
        logger.error("Config secret is not valid base64; treating as unreadable.")
        return None
    data_in, _keep = _blob_in(raw)
    ent_in, _keep_ent = _blob_in(_ENTROPY)
    out = _Blob()
    ok = crypt32.CryptUnprotectData(
        ctypes.byref(data_in), None, ctypes.byref(ent_in),
        None, None, CRYPTPROTECT_UI_FORBIDDEN, ctypes.byref(out),
    )
    if not ok:
        logger.error(
            "CryptUnprotectData failed (%s). The stored secret was encrypted by a "
            "different Windows user account and must be set again."
            % ctypes.GetLastError()
        )
        return None
    try:
        plain = _blob_bytes(out)
    finally:
        kernel32.LocalFree(out.pbData)
    try:
        return plain.decode("utf-8")
    except UnicodeDecodeError:
        logger.error("Decrypted config secret is not UTF-8; treating as unreadable.")
        return None


def migrate_config(cfg):
    """Encrypt any still-plaintext secret in `cfg` in place. Returns True if changed.

    Same shape as `speech.migrate_config`, and called from the same place in both
    load paths, so an existing install is protected on the next run rather than
    only when the user next re-enters the key.
    """
    if not isinstance(cfg, dict) or not available():
        return False
    changed = False
    for name, value in list(cfg.items()):
        if not is_secret_setting(name):
            continue
        if not isinstance(value, str) or not value.strip() or is_protected(value):
            continue
        sealed = protect(value)
        if is_protected(sealed):
            cfg[name] = sealed
            changed = True
    return changed
