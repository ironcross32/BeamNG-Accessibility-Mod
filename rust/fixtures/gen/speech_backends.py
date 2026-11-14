"""Print what speech.py sees on THIS machine, for comparison with
`cargo run -p beam-speech --example speak -- --list`.

Not a fixture: the backends, their availability and voices depend on the machine
and on which screen readers are running, so nothing is written to fixtures/data.
Speaks nothing. Run from the repository root:

    uv run python rust/fixtures/gen/speech_backends.py
"""

import _common  # noqa: F401  (puts the repository root on sys.path)

import speech


def bits(feats):
    return None if feats is None else feats.to_bits()


def main():
    print("prism ok:", speech.PRISM_OK)
    backends = speech.list_backends()
    print("list_backends:", backends)
    ctx = speech._shared_context()
    for name in backends:
        # Direct acquire by id, no fallback: the backend's own capability bits.
        try:
            b = ctx.acquire(ctx.id_of(name))
            raw = f"0x{b.features.to_bits():08x} name={b.name!r}"
            del b
        except Exception as e:
            raw = f"unavailable ({type(e).__name__}: {e})"
        names, feats = speech.list_voices(name)
        print(f"backend {name!r}: acquire {raw}")
        print(
            f"    list_voices -> {len(names)} voices, features "
            f"{'None' if feats is None else f'0x{bits(feats):08x}'}"
        )
    names, feats = speech.list_voices(speech.AUTO_BACKEND)
    print(
        f"list_voices('auto') -> features "
        f"{'None' if feats is None else f'0x{bits(feats):08x}'}, voices {names!r}"
    )
    print("init(auto):", speech.init({"speech_backend": "auto"}) is not None)
    print("backend_name:", speech.backend_name())
    print("describe_capabilities:", speech.describe_capabilities())


if __name__ == "__main__":
    main()
