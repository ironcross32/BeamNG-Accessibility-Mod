"""Gear-change reporting: what is spoken, and the tone played instead of speech.

Pure like route_beacon.py and wheel_slip.py -- beamtel.py decides WHEN a gear changed,
audio.py owns the stream, and everything in between lives here so
                                            diagnostic/gear_report_sim.py can assert it with no game, no device and no clock.
"""

import math

try:
    import numpy as np
except ImportError:
    np = None

MODES = ("off", "speech", "audio")
NUMBER_STYLES = ("ordinal", "number")

ORDINAL_WORDS = {
    1: "first",
    2: "second",
    3: "third",
    4: "fourth",
    5: "fifth",
    6: "sixth",
    7: "seventh",
    8: "eighth",
    9: "ninth",
    10: "tenth",
    11: "eleventh",
    12: "twelfth",
}

# (config value, spoken label). Enharmonic pairs share one entry: they are one pitch in
# equal temperament, and two entries for it would read as two choices that differ.
ROOT_NOTES = (
    ("C", "C"),
    ("C#", "C sharp / D flat"),
    ("D", "D"),
    ("D#", "D sharp / E flat"),
    ("E", "E"),
    ("F", "F"),
    ("F#", "F sharp / G flat"),
    ("G", "G"),
    ("G#", "G sharp / A flat"),
    ("A", "A"),
    ("A#", "A sharp / B flat"),
    ("B", "B"),
)
ROOT_OCTAVES = (2, 3, 4, 5, 6)

# (config value, label, steps in semitones above the root within one octave). Steps are
# floats so the quarter-tone scales sit on the same arithmetic as the rest.
SCALES = (
    ("major", "Major", (0, 2, 4, 5, 7, 9, 11)),
    ("natural_minor", "Natural minor", (0, 2, 3, 5, 7, 8, 10)),
    ("harmonic_minor", "Harmonic minor", (0, 2, 3, 5, 7, 8, 11)),
    ("melodic_minor", "Melodic minor", (0, 2, 3, 5, 7, 9, 11)),
    ("dorian", "Dorian", (0, 2, 3, 5, 7, 9, 10)),
    ("phrygian", "Phrygian", (0, 1, 3, 5, 7, 8, 10)),
    ("lydian", "Lydian", (0, 2, 4, 6, 7, 9, 11)),
    ("mixolydian", "Mixolydian", (0, 2, 4, 5, 7, 9, 10)),
    ("major_pentatonic", "Major pentatonic", (0, 2, 4, 7, 9)),
    ("minor_pentatonic", "Minor pentatonic", (0, 3, 5, 7, 10)),
    ("blues", "Blues", (0, 3, 5, 6, 7, 10)),
    ("whole_tone", "Whole tone", (0, 2, 4, 6, 8, 10)),
    ("chromatic", "Chromatic", tuple(range(12))),
    ("quarter_tone", "Quarter-tone chromatic", tuple(i * 0.5 for i in range(24))),
    ("rast", "Maqam Rast (quarter tones)", (0, 2, 3.5, 5, 7, 9, 10.5)),
    ("bayati", "Maqam Bayati (quarter tones)", (0, 1.5, 3, 5, 7, 8, 10)),
)

DEFAULT_MODE = "speech"
DEFAULT_NUMBER_STYLE = "ordinal"
DEFAULT_ROOT = "C"
DEFAULT_OCTAVE = 4
DEFAULT_SCALE = "major"
DEFAULT_LEVEL_DB = -3.0

# Envelope. The attack is only long enough to keep the onset from clicking; the hold is
# what lets the ear lock onto a pitch (below ~30 ms of steady tone a note reads as a
# click with a colour, not a note); the cubic tail keeps it short in practice -- it is
# 18 dB down halfway through -- while never cutting off audibly.
TONE_ATTACK_MS = 3.0
TONE_HOLD_MS = 8.5
TONE_DECAY_MS = 32.0
# Harmonic partials (multiple, relative amplitude). A sine at a few hundred hertz
# disappears under engine noise; the upper partials put energy where the ear is most
# sensitive, so the note stands out without needing to be loud.
TONE_PARTIALS = ((1, 1.0), (2, 0.5), (3, 0.25), (4, 0.12))
# Sport gears: same pitch, brighter. Drive and sport 1 share a degree, so shifting D -> S
# would otherwise be a repeat of the note just heard; a richer top end marks the mode
# change without moving the pitch, which still has to say which gear it is.
TONE_PARTIALS_SPORT = ((1, 1.0), (2, 0.7), (3, 0.5), (4, 0.35), (5, 0.25), (6, 0.18))
TONE_MIN_HZ = 40.0
TONE_MAX_HZ = 8000.0


def migrate_config(cfg):
    """Turn the old announce_gear boolean into gear_report_mode. True if it changed."""
    if "announce_gear" not in cfg:
        return False
    old = cfg.pop("announce_gear")
    if "gear_report_mode" not in cfg:
        cfg["gear_report_mode"] = "speech" if old else "off"
    return True


def _choice(value, allowed, fallback):
    value = str(value or "").strip().lower()
    return value if value in allowed else fallback


def settings_from_config(cfg):
    """The validated gear-report settings, whatever the config file holds."""
    try:
        level = max(-120.0, min(0.0, float(cfg.get("gear_tone_level_dbfs", DEFAULT_LEVEL_DB))))
    except (TypeError, ValueError):
        level = DEFAULT_LEVEL_DB
    try:
        octave = int(cfg.get("gear_tone_octave", DEFAULT_OCTAVE))
    except (TypeError, ValueError):
        octave = DEFAULT_OCTAVE
    root = str(cfg.get("gear_tone_root", DEFAULT_ROOT) or "").strip()
    return {
        "mode": _choice(cfg.get("gear_report_mode"), MODES, DEFAULT_MODE),
        "collapse_sport": bool(cfg.get("gear_speech_sport_once", False)),
        "number_style": _choice(
            cfg.get("gear_speech_number_style"), NUMBER_STYLES, DEFAULT_NUMBER_STYLE
        ),
        "root": root if note_semitone(root) is not None else DEFAULT_ROOT,
        "octave": octave if octave in ROOT_OCTAVES else DEFAULT_OCTAVE,
        "scale": _choice(cfg.get("gear_tone_scale"), [k for k, _l, _s in SCALES], DEFAULT_SCALE),
        "level_db": level,
    }


# =========================
#  Speech
# =========================
def _forward(n, number_style):
    if number_style == "number":
        return str(n)
    return ORDINAL_WORDS.get(n, f"{n}th")


def gear_to_phrase(gear_byte, number_style=DEFAULT_NUMBER_STYLE):
    """OutGauge byte: 0 is reverse, 1 neutral, 2 first gear."""
    if gear_byte == 0:
        return "reverse"
    if gear_byte == 1:
        return "neutral"
    if gear_byte >= 2:
        return _forward(gear_byte - 1, number_style)
    return "unknown"


def extended_gear_to_phrase(gear_str, number_style=DEFAULT_NUMBER_STYLE):
    s = (gear_str or "").strip().upper()
    if not s:
        return "unknown"

    if s == "P":
        return "park"
    if s == "D":
        return "drive"
    if s == "R":
        return "reverse"
    if s == "N":
        return "neutral"
    if s == "S":
        return "sport"
    if s == "M":
        return "manual"

    if len(s) > 1 and s[1:].isdigit():
        num_part = s[1:]
        if s.startswith("S"):
            return f"sport {num_part}"
        if s.startswith("M"):
            return f"manual {num_part}"

    if s.isdigit() or s.startswith("-"):
        try:
            num = int(s)
        except ValueError:
            return "unknown"
        if num == -1:
            return "reverse"
        if num == 0:
            return "neutral"
        return _forward(num, number_style)

    return "unknown"


def is_sport(gear_str):
    """S, or S followed by a gear number: what a CVT (or an automatic) in sport reports."""
    s = (gear_str or "").strip().upper()
    return s.startswith("S") and (len(s) == 1 or s[1:].isdigit())


def extended_change_phrase(
    gear_str, prev_gear_str, collapse_sport=False, number_style=DEFAULT_NUMBER_STYLE
):
    """What to say when the extended gear string changes, or None to stay silent.

    With collapse_sport, entering sport says "sport" once and the transmission stepping
    through S1, S2, S3 inside it says nothing: a CVT does that on its own, continuously,
    and naming each step is noise about something the driver did not do.
    """
    if collapse_sport and is_sport(gear_str):
        return None if is_sport(prev_gear_str) else "sport"
    if (gear_str or "").strip().upper() == "N":
        return "neutral"
    phrase = extended_gear_to_phrase(gear_str, number_style)
    return None if phrase in ("unknown", "neutral") else phrase


def outgauge_change_phrase(gear_byte, number_style=DEFAULT_NUMBER_STYLE):
    phrase = gear_to_phrase(gear_byte, number_style)
    if gear_byte == 1:
        return "neutral"
    return None if phrase in ("unknown", "neutral") else phrase


# =========================
#  Tone
# =========================
# A gear's tone is a scale DEGREE counted from neutral, which sits on the root: the
# lever's own order (P, R, N, 1, 2, 3...) laid onto the scale, so the root is "home" and
# every forward gear is a step further up. Sport and manual gears take their number's
# degree; D sits where first gear does.
def extended_gear_degree(gear_str):
    s = (gear_str or "").strip().upper()
    if s == "P":
        return -2
    if s == "R":
        return -1
    if s == "N":
        return 0
    if s in ("D", "S", "M"):
        return 1
    if len(s) > 1 and s[0] in "SM" and s[1:].isdigit():
        return int(s[1:])
    try:
        num = int(s)
    except ValueError:
        return None
    # A bare "0" is a manual box's neutral, and it is silent here as it is in speech: the
    # game drops a manual into neutral for as long as a shift button is held, so it
    # would sound on every shift. Automatics report "N", which is handled above.
    if num == 0:
        return None
    return -1 if num < 0 else num


def outgauge_gear_degree(gear_byte):
    if gear_byte == 0:
        return -1
    if gear_byte >= 1:
        return gear_byte - 1
    return None


def note_semitone(name):
    """Semitones above C for a note name such as C, F#, Bb. None if it is not one."""
    s = str(name or "").strip()
    if not s:
        return None
    base = {"C": 0, "D": 2, "E": 4, "F": 5, "G": 7, "A": 9, "B": 11}.get(s[0].upper())
    if base is None:
        return None
    acc = s[1:]
    if acc in ("", "♮"):
        return base
    if acc in ("#", "♯"):
        return (base + 1) % 12
    if acc in ("b", "♭"):
        return (base - 1) % 12
    return None


def scale_steps(scale):
    for key, _label, steps in SCALES:
        if key == scale:
            return steps
    return SCALES[0][2]


def degree_semitones(degree, scale):
    """Semitones above the root for a scale degree; negative degrees go below it."""
    steps = scale_steps(scale)
    octave, idx = divmod(int(degree), len(steps))
    return 12.0 * octave + steps[idx]


def root_frequency(root, octave):
    """Equal-tempered, A4 = 440 Hz; octave numbering is scientific (C4 is middle C)."""
    pc = note_semitone(root)
    if pc is None:
        pc = 0
    midi = 12 * (int(octave) + 1) + pc
    return 440.0 * 2.0 ** ((midi - 69) / 12.0)


def degree_frequency(degree, root, octave, scale):
    hz = root_frequency(root, octave) * 2.0 ** (degree_semitones(degree, scale) / 12.0)
    return max(TONE_MIN_HZ, min(TONE_MAX_HZ, hz))


def tone_envelope(sr):
    attack = max(1, int(sr * TONE_ATTACK_MS / 1000.0))
    hold = int(sr * TONE_HOLD_MS / 1000.0)
    decay = max(1, int(sr * TONE_DECAY_MS / 1000.0))
    # Half-cosine onset: no corner at the start of the tail, unlike a linear ramp.
    rise = 0.5 - 0.5 * np.cos(np.pi * np.arange(attack) / attack)
    fall = (1.0 - np.arange(decay) / decay) ** 3
    return np.concatenate([rise, np.ones(hold), fall])


def render_tone(freq_hz, level_db, sr, sport=False):
    """One gear tone as a mono float32 buffer whose peak is exactly level_db dBFS.

    sport selects the brighter partial set; the pitch and level are unchanged."""
    env = tone_envelope(sr)
    t = np.arange(len(env)) / float(sr)
    wave = np.zeros(len(env))
    for mult, amp in TONE_PARTIALS_SPORT if sport else TONE_PARTIALS:
        # Partials above ~Nyquist would alias back down as an unrelated pitch.
        if freq_hz * mult < 0.45 * sr:
            wave += amp * np.sin(2.0 * math.pi * freq_hz * mult * t)
    wave *= env
    peak = float(np.max(np.abs(wave)))
    if peak > 0.0:
        wave *= (10.0 ** (level_db / 20.0)) / peak
    return wave.astype(np.float32)
