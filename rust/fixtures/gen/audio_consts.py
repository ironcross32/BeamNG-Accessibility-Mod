"""Golden values of every module-level constant in audio.py, for beam_audio::consts.

Writes rust/fixtures/data/audio_consts.json: one entry per top-level assignment before
``class AudioController``, in source order, with its value (floats as fbits). The Rust
test checks each `beam_audio::consts` item against it, so a constant retuned in Python
and not in Rust (or the other way round) fails by name.

``--emit-rust`` prints a draft of consts.rs to stdout instead (the starting point the
hand-written module was made from; not used by the tests).
"""

from __future__ import annotations

import ast
import os
import sys

from _common import REPO_ROOT, deep_fbits, write_fixture

import audio


def top_level_names():
    path = os.path.join(REPO_ROOT, "audio.py")
    with open(path, encoding="utf-8") as fh:
        tree = ast.parse(fh.read())
    out = []
    for node in tree.body:
        if isinstance(node, ast.ClassDef) and node.name == "AudioController":
            break
        if isinstance(node, ast.Assign):
            for target in node.targets:
                names = target.elts if isinstance(target, ast.Tuple) else [target]
                for n in names:
                    if isinstance(n, ast.Name):
                        out.append((n.id, ast.unparse(node.value)))
    return out


def kind_of(v):
    if isinstance(v, bool):
        return "bool"
    if isinstance(v, int):
        return "int"
    if isinstance(v, float):
        return "float"
    if isinstance(v, str):
        return "str"
    if isinstance(v, (tuple, list)):
        return "seq"
    if isinstance(v, dict):
        return "dict"
    if isinstance(v, (set, frozenset)):
        return "set"
    return type(v).__name__


def encode(v):
    if isinstance(v, (set, frozenset)):
        return sorted(v)
    if isinstance(v, dict):
        return [[k, deep_fbits(val)] for k, val in v.items()]
    return deep_fbits(v)


def main():
    entries = []
    for name, expr in top_level_names():
        if name in ("AUDIO_OK", "ScannerHRTFDiagnostic", "np", "sd"):
            continue
        v = getattr(audio, name)
        entries.append({"name": name, "kind": kind_of(v), "expr": expr, "value": encode(v)})

    if "--emit-rust" in sys.argv:
        for e in entries:
            print(f"// {e['name']} = {e['expr']}  [{e['kind']}] {getattr(audio, e['name'])!r}")
        return

    write_fixture("audio_consts", {"constants": entries})


if __name__ == "__main__":
    main()
