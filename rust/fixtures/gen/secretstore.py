"""Golden cases for beam_base::secretstore, from secretstore.py.

Everything here is independent of the Windows account: DPAPI ciphertext only opens for
the user that sealed it, so the round trip is tested live in Rust rather than through
a committed blob. What is fixed is the name list, the marker, and how a stored value is
base64-decoded before it reaches DPAPI.
"""

from __future__ import annotations

import base64

from _common import write_fixture

import secretstore as ss

NAMES = [
    "ai_describer_api_key", "ai_describer_openai_api_key", "AI_DESCRIBER_API_KEY", "apikey",
    "myApiKey", "github_token", "TOKEN", "client_secret", "Password", "passwd_x", "pass_wd",
    "api-key", "units", "", "speech_backend", "tokenizer", "İtoken", "SECRET",
]

B64 = [
    "", "AAAA", "AAA=", "AA==", "A===", "A", "AA", "AAA", "AAAAA", "QUJD", "QUJDRA==", "QUJDRA",
    "QUJDRA=", "QUJDRA===", "Q U J D", "QUJD\n", "QU=JD", "=QUJD", "QUJD====QUJD", "QUJ*D",
    "QUJDRA==QUJD", "-_-_", "QUJéD", "QQ==QQ==", "QUI=x", "QUI==x", "QUI=QUJD",
    "QUI=QUI=", "AA=\n=", "AA= =", "AA==\n", "QUI=*", "QQ=x=", "QQ=xQ=", "QUJD=", "QUJD=A",
    "AA==AA==", "A=AA", "====", "QUJ=D===",
]


def main():
    b64 = []
    for s in B64:
        try:
            raw = base64.b64decode(s.encode("ascii"))
            b64.append({"text": s, "ok": raw.hex()})
        except Exception as exc:  # noqa: BLE001
            b64.append({"text": s, "err": type(exc).__name__})
    write_fixture("secretstore", {
        "PREFIX": ss.PREFIX,
        "SECRET_NAME_PARTS": list(ss.SECRET_NAME_PARTS),
        "CRYPTPROTECT_UI_FORBIDDEN": ss.CRYPTPROTECT_UI_FORBIDDEN,
        "names": [{"name": n, "secret": ss.is_secret_setting(n)} for n in NAMES],
        "protected": [{"value": v, "protected": ss.is_protected(v)}
                      for v in ["", "dpapi:v1:", "dpapi:v1:AAAA", "DPAPI:v1:x", " dpapi:v1:x",
                                "dpapi:v2:x", "plain"]],
        "unprotect_plain": [{"value": v, "out": ss.unprotect(v)}
                            for v in ["", "plain", " spaced ", "dpapi:v1", "dpapi:v1:",
                                      "dpapi:v1:A", "dpapi:v1:AAAA", "dpapi:v1:é"]],
        "b64decode": b64,
    })


if __name__ == "__main__":
    main()
