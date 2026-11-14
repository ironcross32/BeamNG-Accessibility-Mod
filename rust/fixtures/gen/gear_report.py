"""Golden cases for beam_base::gear_report, from gear_report.py."""

from __future__ import annotations

import json

import numpy as np

from _common import fbits, outcome, write_fixture

import gear_report as gr

GEARS = [
    "", " ", "P", "p", "D", "R", "N", "n ", "S", "M", "S1", "s3", "S12", "M5", "m 5", "SPORT",
    "S-1", "S+1", "S1_0", "1", "2", "12", "13", "14", "0", "00", "-0", "-1", "-2", "-10", "-",
    "--1", "+3", "1_0", "-1_0", "80%", "1.5", " 3 ", "X", "Ś1", "ß", "99999999999999999999999",
    "S99999999999999999999999", "-99999999999999999999999", "D1", "\t2\n", "S²", "N1",
]
STYLES = ["ordinal", "number", "other"]


def main():
    phrases = []
    for g in GEARS:
        for st in STYLES:
            phrases.append({"gear": g, "style": st,
                            "phrase": gr.extended_gear_to_phrase(g, st)})
    singles = [{"gear": g, "is_sport": gr.is_sport(g),
                "degree": outcome(gr.extended_gear_degree, g)} for g in GEARS]
    for rec in singles:
        if "ok" in rec["degree"] and isinstance(rec["degree"]["ok"], int):
            if abs(rec["degree"]["ok"]) >= 2**63:
                rec["degree"] = {"big": str(rec["degree"]["ok"])}
    bytes_ = []
    for b in range(-3, 16):
        for st in STYLES:
            bytes_.append({"byte": b, "style": st, "phrase": gr.gear_to_phrase(b, st),
                           "change": gr.outgauge_change_phrase(b, st),
                           "degree": gr.outgauge_gear_degree(b)})
    pairs = []
    prevs = [None, "", "D", "S", "S1", "S2", "N", "M1", "R", "0", "s4"]
    for g in ["D", "S", "S1", "S2", "s3", "N", "n", "M1", "R", "0", "-1", "3", "80%", "", "SPORT"]:
        for p in prevs:
            for collapse in (False, True):
                for st in ("ordinal", "number"):
                    pairs.append({
                        "gear": g, "prev": p, "collapse": collapse, "style": st,
                        "phrase": gr.extended_change_phrase(g, p, collapse, st),
                        "silenced": gr.sport_step_silenced(g, p, collapse),
                    })
    notes = [{"name": n, "semitone": gr.note_semitone(n)} for n in [
        "", " ", "C", "c", "C#", "c#", "Db", "DB", "B#", "Cb", "E♯", "F♭", "G♮", "A♯", "Bb ",
        " F# ", "H", "C##", "Cbb", "ß", "1", "C4", "#",
    ]]
    semis = []
    for key, _label, _steps in gr.SCALES + (("nonexistent", "", ()),):
        for d in range(-30, 31):
            semis.append({"degree": d, "scale": key,
                          "semitones": fbits(gr.degree_semitones(d, key))})
    freqs = []
    for root in ["C", "A", "F#", "Bb", "H", ""]:
        for octave in [-1, 0, 2, 4, 6, 10]:
            freqs.append({"root": root, "octave": octave,
                          "root_hz": fbits(gr.root_frequency(root, octave)),
                          "degrees": [fbits(gr.degree_frequency(d, root, octave, sc))
                                      for d in (-40, -2, -1, 0, 1, 5, 9, 40)
                                      for sc in ("major", "quarter_tone", "rast")]})
    cfgs = [
        {}, {"gear_report_mode": "loud", "gear_tone_root": "H", "gear_tone_octave": 11,
             "gear_tone_scale": "x", "gear_tone_level_dbfs": 9},
        {"gear_report_mode": " AUDIO ", "gear_speech_sport_once": "no",
         "gear_speech_number_style": "Number", "gear_tone_root": " f# ",
         "gear_tone_octave": "6", "gear_tone_scale": "RAST", "gear_tone_level_dbfs": "-20"},
        {"gear_report_mode": None, "gear_speech_sport_once": 0, "gear_tone_root": None,
         "gear_tone_octave": None, "gear_tone_level_dbfs": None},
        {"gear_report_mode": 0, "gear_tone_root": 0, "gear_tone_octave": 4.9,
         "gear_tone_level_dbfs": "nan"},
        {"gear_tone_octave": True, "gear_tone_level_dbfs": float("-inf")},
        {"gear_tone_octave": "4.0", "gear_tone_level_dbfs": [1]},
        {"gear_tone_octave": float("nan"), "gear_tone_level_dbfs": -200},
        {"gear_tone_octave": float("inf")},
        {"gear_tone_level_dbfs": 10**400},
        {"gear_tone_root": ["C"], "gear_tone_scale": ["major"], "gear_report_mode": ["off"]},
        {"gear_tone_root": "Bb", "gear_tone_octave": 2, "gear_report_mode": "off"},
    ]
    settings = []
    for c in cfgs:
        res = outcome(gr.settings_from_config, c)
        if "ok" in res:
            res["ok"] = {k: (fbits(v) if isinstance(v, float) else v) for k, v in res["ok"].items()}
        settings.append({"cfg": json.dumps(c), **res})
    migrations = []
    for c in [{}, {"announce_gear": False}, {"announce_gear": True}, {"announce_gear": "x", "a": 1},
              {"a": 1, "announce_gear": 0, "gear_report_mode": "audio", "b": 2},
              {"announce_gear": None, "z": 1}]:
        before = json.dumps(c)
        changed = gr.migrate_config(c)
        migrations.append({"before": before, "after": json.dumps(c), "changed": changed})

    tones = []
    for freq, db, sr, sport in [
        (440.0, -3.0, 48000, False), (440.0, -3.0, 48000, True), (330.0, -20.0, 48000, True),
        (8000.0, -3.0, 48000, False), (40.0, 0.0, 48000, False), (261.6255653005986, -3.0, 44100, False),
        (1000.0, -60.0, 22050, True), (3000.0, -3.0, 8000, True),
    ]:
        buf = gr.render_tone(freq, db, sr, sport)
        assert buf.dtype == np.float32
        tones.append({"freq": fbits(freq), "db": fbits(db), "sr": sr, "sport": sport,
                      "f32_hex": buf.astype("<f4").tobytes().hex()})
    envelopes = [{"sr": sr, "f64_hex": gr.tone_envelope(sr).astype("<f8").tobytes().hex()}
                 for sr in (48000, 44100, 8000, 100)]

    write_fixture("gear_report", {
        "constants": {
            "MODES": list(gr.MODES), "NUMBER_STYLES": list(gr.NUMBER_STYLES),
            "ORDINAL_WORDS": [[k, v] for k, v in gr.ORDINAL_WORDS.items()],
            "ROOT_NOTES": [list(x) for x in gr.ROOT_NOTES],
            "ROOT_OCTAVES": list(gr.ROOT_OCTAVES),
            "SCALES": [[k, label, [fbits(float(s)) for s in steps]] for k, label, steps in gr.SCALES],
            "TONE_PARTIALS": [[m, fbits(a)] for m, a in gr.TONE_PARTIALS],
            "TONE_PARTIALS_SPORT": [[m, fbits(a)] for m, a in gr.TONE_PARTIALS_SPORT],
            "floats": {k: fbits(getattr(gr, k)) for k in (
                "DEFAULT_LEVEL_DB", "TONE_ATTACK_MS", "TONE_HOLD_MS", "TONE_DECAY_MS",
                "TONE_MIN_HZ", "TONE_MAX_HZ")},
            "strs": {k: getattr(gr, k) for k in (
                "DEFAULT_MODE", "DEFAULT_NUMBER_STYLE", "DEFAULT_ROOT", "DEFAULT_SCALE")},
            "DEFAULT_OCTAVE": gr.DEFAULT_OCTAVE,
        },
        "phrases": phrases, "singles": singles, "bytes": bytes_, "pairs": pairs,
        "notes": notes, "semitones": semis, "frequencies": freqs, "settings": settings,
        "migrations": migrations, "tones": tones, "envelopes": envelopes,
    })


if __name__ == "__main__":
    main()
