"""Assert gear-change reporting: the spoken phrase, the sport collapse, and the tone.

gear_report.py is pure, so this needs no game, no audio device and no clock. Run after
touching it, or the gear block in beamtel.py's telemetry loop:

    python diagnostic/gear_report_sim.py
"""
import os
import sys

import numpy as np

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
import gear_report as gr  # noqa: E402

SR = 48000
failures = 0


def check(name, ok, detail=""):
    global failures
    if not ok:
        failures += 1
    print(f"{'OK  ' if ok else 'FAIL'} {name}{(' -- ' + detail) if detail else ''}")


def replay(sequence, **kw):
    """Speak a sequence of extended gear strings as the telemetry loop would."""
    spoken, prev = [], None
    for g in sequence:
        if g != prev:
            phrase = gr.extended_change_phrase(g, prev, **kw)
            if phrase:
                spoken.append(phrase)
        prev = g
    return spoken


# 1. The default style is byte-identical to the phrases spoken before this module.
seq = ["P", "R", "N", "1", "2", "3", "N", "D", "S3", "M5", "-1", "0", "80%"]
check(
    "default speech unchanged",
    replay(seq) == ["park", "reverse", "neutral", "first", "second", "third",
                    "neutral", "drive", "sport 3", "manual 5", "reverse"],
    str(replay(seq)),
)

# 2. Numbers style changes forward numeric gears only.
check(
    "numbers style",
    replay(["N", "1", "2", "S3", "R"], number_style="number")
    == ["neutral", "1", "2", "sport 3", "reverse"],
)
check("number style past the word table", gr.extended_gear_to_phrase("14", "number") == "14")
check("ordinal past the word table", gr.extended_gear_to_phrase("14") == "14th")

# 3. Sport collapse: one "sport" on entry, silence while the CVT steps, and leaving
#    sport is still spoken.
cvt = ["D", "S1", "S2", "S3", "S2", "S1", "D", "S1", "N"]
check(
    "sport said once per entry",
    replay(cvt, collapse_sport=True) == ["drive", "sport", "drive", "sport", "neutral"],
    str(replay(cvt, collapse_sport=True)),
)
check(
    "collapse off still names each step",
    replay(cvt) == ["drive", "sport 1", "sport 2", "sport 3", "sport 2", "sport 1",
                    "drive", "sport 1", "neutral"],
)
check("collapse leaves manual alone", replay(["D", "M1", "M2"], collapse_sport=True)
      == ["drive", "manual 1", "manual 2"])

# 4. OutGauge keeps its own encoding.
check("outgauge", [gr.outgauge_change_phrase(b) for b in (0, 1, 2, 3)]
      == ["reverse", "neutral", "first", "second"])
check("outgauge numbers", gr.outgauge_change_phrase(4, "number") == "3")

# 5. Degrees follow the lever: P < R < N < 1 < 2 ..., and both protocols agree.
degrees = [gr.extended_gear_degree(g) for g in ("P", "R", "N", "1", "2", "3")]
check("lever order", degrees == sorted(degrees) and len(set(degrees)) == 6, str(degrees))
check("protocols agree", [gr.outgauge_gear_degree(b) for b in (0, 1, 2, 3)]
      == [gr.extended_gear_degree(g) for g in ("R", "N", "1", "2")])
check("sport and manual share the number's degree",
      gr.extended_gear_degree("S3") == gr.extended_gear_degree("M3") == gr.extended_gear_degree("3"))
check("cannon strength has no degree", gr.extended_gear_degree("80%") is None)
# A manual box sits in "0" for as long as a shift button is held, so it must be as
# silent in audio as it is in speech; an automatic's "N" keeps its tone.
check("manual neutral has no tone", gr.extended_gear_degree("0") is None)
check("manual neutral is not spoken", replay(["2", "0", "3"]) == ["second", "third"])
check("automatic neutral keeps its tone", gr.extended_gear_degree("N") == 0)

# 6. Pitch: A4 is 440, neutral is the root, and every scale ascends with the gears.
check("A4 = 440", abs(gr.root_frequency("A", 4) - 440.0) < 1e-9)
check("C4 = middle C", abs(gr.root_frequency("C", 4) - 261.6256) < 1e-3)
check("flat spelling", gr.note_semitone("Db") == gr.note_semitone("C#") == 1)
for key, label, _steps in gr.SCALES:
    hz = [gr.degree_frequency(d, "C", 3, key) for d in range(-2, 19)]
    check(f"ascending: {label}", all(b > a for a, b in zip(hz, hz[1:])))
qt = [gr.degree_semitones(d, "quarter_tone") for d in range(0, 4)]
check("quarter-tone steps are 50 cents", qt == [0.0, 0.5, 1.0, 1.5], str(qt))
check("pentatonic wraps to the octave", gr.degree_semitones(5, "major_pentatonic") == 12)
check("negative degrees go below the root", gr.degree_semitones(-1, "major") == -1)

# 7. The tone: exact peak level, short, a gentle onset, and a cubic tail ending at zero.
for db in (-3.0, -20.0):
    tone = gr.render_tone(440.0, db, SR)
    peak = 20 * np.log10(np.max(np.abs(tone)))
    check(f"peak at {db} dBFS", abs(peak - db) < 0.01, f"{peak:.3f}")
tone = gr.render_tone(440.0, -3.0, SR)
dur_ms = 1000 * len(tone) / SR
want_ms = gr.TONE_ATTACK_MS + gr.TONE_HOLD_MS + gr.TONE_DECAY_MS
check("duration matches the envelope", abs(dur_ms - want_ms) < 0.1, f"{dur_ms:.1f} ms")
env = gr.tone_envelope(SR)
check("onset starts silent", env[0] == 0.0)
check("onset jump below a click", np.max(np.abs(np.diff(env[: int(SR * 0.005)]))) < 0.02)
decay = int(SR * gr.TONE_DECAY_MS / 1000.0)
tail = env[-decay:]
check("tail is cubic", abs(tail[decay // 2] - 0.125) < 0.01, f"{tail[decay // 2]:.3f}")
check("tail ends at silence", abs(tone[-1]) < 1e-4)
check("high root drops aliasing partials", np.all(np.isfinite(gr.render_tone(8000.0, -3, SR))))

# 7b. Sport is the same pitch and peak as the plain tone, only brighter. Drive and sport 1
#     share a degree, so timbre is the only thing that can mark that shift.
def spectrum(buf):
    padded = np.zeros(SR)
    padded[: len(buf)] = buf
    mag = np.abs(np.fft.rfft(padded))
    return np.fft.rfftfreq(SR, 1.0 / SR), mag


check("drive and sport 1 share a degree",
      gr.extended_gear_degree("D") == gr.extended_gear_degree("S1"))
plain = gr.render_tone(330.0, -3.0, SR)
sport = gr.render_tone(330.0, -3.0, SR, sport=True)
f, mp = spectrum(plain)
_, ms = spectrum(sport)
c_plain = float(np.sum(f * mp) / np.sum(mp))
c_sport = float(np.sum(f * ms) / np.sum(ms))
check("sport is brighter", c_sport > c_plain * 1.25, f"centroid {c_plain:.0f} -> {c_sport:.0f} Hz")
check("sport is only a bit brighter", c_sport < c_plain * 2.0)
check("sport keeps the pitch", abs(f[np.argmax(ms)] - 330.0) < 2 and abs(f[np.argmax(mp)] - 330.0) < 2)
check("sport keeps the peak", abs(20 * np.log10(np.max(np.abs(sport))) + 3.0) < 0.01)
check("sport tone only for sport gears",
      [gr.is_sport(g) for g in ("D", "S", "S1", "S12", "M1", "3", "SPORT")]
      == [False, True, True, True, False, False, False])

# 8. Config: the old boolean migrates, and garbage falls back to defaults.
cfg = {"announce_gear": False}
check("migrate off", gr.migrate_config(cfg) and cfg == {"gear_report_mode": "off"})
cfg = {"announce_gear": True}
gr.migrate_config(cfg)
check("migrate on", cfg == {"gear_report_mode": "speech"})
cfg = {"announce_gear": False, "gear_report_mode": "audio"}
gr.migrate_config(cfg)
check("migrate keeps an explicit mode", cfg == {"gear_report_mode": "audio"})
s = gr.settings_from_config({"gear_report_mode": "loud", "gear_tone_root": "H",
                             "gear_tone_octave": 11, "gear_tone_scale": "x",
                             "gear_tone_level_dbfs": 9})
check("garbage falls back", s["mode"] == "speech" and s["root"] == "C"
      and s["octave"] == 4 and s["scale"] == "major" and s["level_db"] == 0.0, str(s))
check("default level -3 dBFS", gr.settings_from_config({})["level_db"] == -3.0)

print()
print("ALL OK" if not failures else f"{failures} FAILED")
sys.exit(1 if failures else 0)
