"""The F9 Alt+T forced induction report: what is spoken for one reply from the game.

Pure like gear_report.py. vehicleScanner.lua sends raw figures as JSON and beamtel.py owns the
socket, so diagnostic/induction_report_sim.py can assert the wording with no game.
"""

import json

REPLY_PREFIX = "FI:"
TYPE_NAMES = {"roots": "Roots", "screws": "Screw", "centrifugal": "Centrifugal"}


def parse_reply(text):
    """The figures from one packet, or None when it is not a usable report."""
    if not text.startswith(REPLY_PREFIX):
        return None
    try:
        report = json.loads(text[len(REPLY_PREFIX) :])
    except ValueError:
        return None
    return report if isinstance(report, dict) else None


def _percent(coef):
    return int(round(float(coef) * 100.0))


def _rpm(value):
    return f"{int(round(float(value))):,}"


def _torque_phrase(coef):
    """The live multiplier as a change. Idling it is below 1: blower drag, or a turbo in vacuum."""
    change = _percent(coef) - 100
    if change > 0:
        return f"Engine torque up {change} percent"
    if change < 0:
        return f"Engine torque down {-change} percent"
    return "Engine torque unchanged"


def _best_gain(charger, about=""):
    """Absent on an engine with no torque curve entries; never read out as a gain of nothing."""
    peak_coef, peak_rpm = charger.get("peakCoef"), charger.get("peakRPM")
    if peak_coef is None or peak_rpm is None:
        return []
    return [f"Best gain {about}{_percent(peak_coef) - 100} percent at {_rpm(peak_rpm)} RPM"]


def _supercharger_parts(charger, fmt_pressure):
    """(what changes while driving, what describes the part), so torque can sit between them."""
    kind = TYPE_NAMES.get(str(charger.get("type", "")).lower())
    if kind:
        twisted = " with twisted lobes" if kind == "Roots" and charger.get("twisted") else ""
        name = f"{kind} supercharger{twisted}"
    else:
        name = "Supercharger"
    boost, unit = fmt_pressure(charger.get("boost", 0))
    boost_max, _ = fmt_pressure(charger.get("boostMax", 0))
    live = [
        name,
        f"Boost {boost} of {boost_max} {unit}",
        f"Blower {_rpm(charger.get('blowerRPM', 0))} RPM",
        f"Blower drag {_percent(charger.get('lossCoef', 0))} percent",
    ]
    spec = _best_gain(charger)
    spec.append(f"Condition {_percent(charger.get('condition', 1))} percent")
    return live, spec


def _turbo_parts(charger, fmt_pressure):
    """As _supercharger_parts. The best gain is the game's estimate, hence "about"."""
    fitted = [
        label
        for key, label in (("blowOff", "blow-off valve"), ("antilag", "anti-lag"))
        if charger.get(key)
    ]
    name = f"Turbo with {' and '.join(fitted)}" if fitted else "Turbo"
    boost, unit = fmt_pressure(charger.get("boost", 0))
    boost_max, _ = fmt_pressure(charger.get("boostMax", 0))
    target, _ = fmt_pressure(charger.get("wastegateTarget", 0))
    speed = f"Turbo speed {_rpm(charger.get('turboRPM', 0))} RPM"
    if charger.get("maxTurboRPM"):
        share = float(charger.get("turboRPM", 0)) / float(charger["maxTurboRPM"])
        speed += f", {_percent(share)} percent of maximum"
    live = [
        name,
        f"Boost {boost} of {boost_max} {unit}",
        f"Wastegate target {target} {unit} in this gear",
        speed,
    ]
    spec = _best_gain(charger, about="about ")
    if charger.get("heat") is not None and charger.get("heatLimit"):
        share = float(charger["heat"]) / float(charger["heatLimit"])
        spec.append(f"Heat {_percent(share)} percent of damage limit")
    if charger.get("hot"):
        spec.append("Overheating")
    spec.append(f"Condition {_percent(charger.get('condition', 1))} percent")
    return live, spec


def report_phrase(report, fmt_pressure):
    """One spoken report. fmt_pressure takes PSI and returns (value, unit) in the user's units."""
    chargers = []
    if report.get("turbo"):
        chargers.append(_turbo_parts(report["turbo"], fmt_pressure))
    if report.get("supercharger"):
        chargers.append(_supercharger_parts(report["supercharger"], fmt_pressure))

    engine = []
    if report.get("damaged"):
        engine.append("Induction system damaged")
    if report.get("twoStep") is not None:
        engine.append(f"Two-step {'on' if report['twoStep'] else 'off'}")
    torque = _torque_phrase(report.get("torqueCoef", 1))

    if len(chargers) == 1:
        live, spec = chargers[0]
        return ", ".join(live + [torque] + spec + engine)
    # The torque multiplier is the whole engine's, so a twin-charged car hears it once, after both.
    sections = [", ".join(live + spec) for live, spec in chargers]
    sections.append(", ".join([torque] + engine))
    return ". ".join(sections)
