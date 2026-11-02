"""Assert the F9 Alt+T forced induction report, and the Lua half's wire contract.

Needs no game. Run after touching induction_report.py, the INDUCTION branch of
vehicleScanner.lua, or the boost fields of the telemetry protocol:

    python diagnostic/induction_report_sim.py
"""
import json
import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, ROOT)
import induction_report as ir  # noqa: E402

failures = 0


def check(name, ok, detail=""):
    global failures
    if not ok:
        failures += 1
    print(f"{'OK  ' if ok else 'FAIL'} {name}{(' -- ' + str(detail)) if detail else ''}")


def psi(value):
    return round(float(value), 1), "psi"


def bar(value):
    return round(float(value) / 14.504, 2), "bar"


# A roots-blown car at wide open throttle, near its torque peak.
SUPERCHARGER = {
    "boost": 7.26,
    "boostMax": 9.633,
    "type": "roots",
    "twisted": False,
    "blowerRPM": 7220.4,
    "lossCoef": 0.1083,
    "peakCoef": 1.4729,
    "peakRPM": 4225,
    "condition": 1,
}

# A turbo car idling in Park.
TURBO = {
    "boost": 0,
    "boostMax": 28.51,
    "wastegateTarget": 28.505,
    "turboRPM": 0,
    "maxTurboRPM": 250000,
    "peakCoef": 2.0786,
    "peakRPM": 5137,
    "heat": 320.7,
    "heatLimit": 650,
    "hot": False,
    "condition": 1,
    "blowOff": True,
    "antilag": False,
}


def turbo_report(**changes):
    return {"torqueCoef": 0.974, "damaged": False, "turbo": {**TURBO, **changes}}


def sc_report(**changes):
    return {"torqueCoef": 1.473, "damaged": False, "supercharger": {**SUPERCHARGER, **changes}}


# 1. The whole sentence for each charger.
phrase = ir.report_phrase(sc_report(), psi)
check(
    "Full supercharger report",
    phrase
    == "Roots supercharger, Boost 7.3 of 9.6 psi, Blower 7,220 RPM, Blower drag 11 percent, "
    "Engine torque up 47 percent, Best gain 47 percent at 4,225 RPM, Condition 100 percent",
    phrase,
)
phrase = ir.report_phrase(turbo_report(), psi)
check(
    "Full turbo report",
    phrase
    == "Turbo with blow-off valve, Boost 0.0 of 28.5 psi, Wastegate target 28.5 psi in this gear, "
    "Turbo speed 0 RPM, 0 percent of maximum, Engine torque down 3 percent, "
    "Best gain about 108 percent at 5,137 RPM, Heat 49 percent of damage limit, Condition 100 percent",
    phrase,
)

# 2. Pressure, including the wastegate target, goes through the caller's units.
phrase = ir.report_phrase(turbo_report(), bar)
check("Metric units", "Boost 0.0 of 1.97 bar" in phrase and "target 1.97 bar" in phrase, phrase)

# 3. Spinning up: speed reads as a share of maximum, and negative boost is passed through.
phrase = ir.report_phrase(turbo_report(turboRPM=125000, boost=-2.0), psi)
check("Turbo speed share", "Turbo speed 125,000 RPM, 50 percent of maximum" in phrase, phrase)
check("Vacuum reads negative", "Boost -2.0 of" in phrase, phrase)
phrase = ir.report_phrase(turbo_report(maxTurboRPM=0, turboRPM=100), psi)
check("No maximum, no share", "Turbo speed 100 RPM," in phrase and "of maximum" not in phrase, phrase)

# 4. What is fitted: named only when it is.
check(
    "Anti-lag named",
    ir.report_phrase(turbo_report(antilag=True), psi).startswith(
        "Turbo with blow-off valve and anti-lag,"
    ),
)
check(
    "Anti-lag alone",
    ir.report_phrase(turbo_report(blowOff=False, antilag=True), psi).startswith(
        "Turbo with anti-lag,"
    ),
)
check("Neither", ir.report_phrase(turbo_report(blowOff=False), psi).startswith("Turbo,"))
check(
    "Twisted roots",
    ir.report_phrase(sc_report(twisted=True), psi).startswith(
        "Roots supercharger with twisted lobes,"
    ),
)
check(
    "Twisted ignored off roots",
    ir.report_phrase(sc_report(type="centrifugal", twisted=True), psi).startswith(
        "Centrifugal supercharger,"
    ),
)
check(
    "A type this file has never heard of still reads",
    ir.report_phrase(sc_report(type="future"), psi).startswith("Supercharger,"),
)

# 5. Torque as a change, in both directions.
check(
    "Exactly 1 reads unchanged",
    "Engine torque unchanged" in ir.report_phrase({**sc_report(), "torqueCoef": 1.0}, psi),
)
check(
    "Idle reads as a loss",
    "Engine torque down 2 percent" in ir.report_phrase({**sc_report(), "torqueCoef": 0.983}, psi),
)

# 6. Heat and damage.
phrase = ir.report_phrase(
    {**turbo_report(heat=715.0, hot=True, condition=0.4), "damaged": True}, psi
)
check(
    "Hot and damaged",
    phrase.endswith(
        "Heat 110 percent of damage limit, Overheating, Condition 40 percent, Induction system damaged"
    ),
    phrase,
)
check("No heat, no claim", "Heat" not in ir.report_phrase(turbo_report(heat=None), psi))
no_peak = {k: v for k, v in TURBO.items() if k != "peakCoef"}
check(
    "No peak, no claim",
    "Best gain" not in ir.report_phrase({"torqueCoef": 1, "turbo": no_peak}, psi),
)

# 7. Two-step: only a car with the controller hears it, and off is a real answer.
check(
    "Two-step on",
    ir.report_phrase({**turbo_report(), "twoStep": True}, psi).endswith("Two-step on"),
)
check(
    "Two-step off",
    ir.report_phrase({**turbo_report(), "twoStep": False}, psi).endswith("Two-step off"),
)
check("No two-step, no claim", "Two-step" not in ir.report_phrase(turbo_report(), psi))

# 8. Twin-charged: both in turn, the engine-wide torque once and last, each with its own figures.
twin = {
    "torqueCoef": 1.9,
    "damaged": False,
    "twoStep": True,
    "turbo": {**TURBO, "boost": 10.0},
    "supercharger": SUPERCHARGER,
}
phrase = ir.report_phrase(twin, psi)
check("Twin leads with the turbo", phrase.startswith("Turbo with blow-off valve, Boost 10.0"), phrase)
check("Twin has the supercharger", ". Roots supercharger, Boost 7.3 of 9.6 psi" in phrase, phrase)
check("Twin says torque once", phrase.count("Engine torque") == 1, phrase)
check("Twin ends on the engine", phrase.endswith(". Engine torque up 90 percent, Two-step on"), phrase)

# 9. Parsing refuses what is not a report instead of throwing on the listener thread.
check("Parses a reply", ir.parse_reply("FI:" + json.dumps(twin)) == twin)
for bad in ("NONE", "DONE", "ERROR:boom", "FI:", "FI:{truncated", "FI:[1,2]", "SC:{}"):
    check(f"Rejects {bad!r}", ir.parse_reply(bad) is None)

# 10. The Lua half: the chunk crosses VMs, so a syntax error in it would fail silently.
with open(
    os.path.join(ROOT, "bng_mod", "lua", "ge", "extensions", "vehicleScanner.lua"),
    encoding="utf-8",
) as f:
    scanner = f.read()
match = re.search(r'cmd == "INDUCTION".*?queueLuaCommand\(\[\[(.*?)\]\]\)', scanner, re.DOTALL)
check("Chunk found", match is not None)
if match:
    chunk = match.group(1)
    check("Chunk has no percent sign", "%" not in chunk)
    check("Chunk reaches its own end", '_ds:close()' in chunk)
    check("Chunk sends the report prefix", f'"{ir.REPLY_PREFIX}"' in chunk)
    check("Chunk always terminates the reply", '_ds:send("DONE")' in chunk)
    for key in list(SUPERCHARGER) + list(TURBO) + ["torqueCoef", "damaged", "twoStep"]:
        check(f"Chunk sends {key}", re.search(rf"\b{key} = ", chunk) is not None)
    check(
        "Both chargers are gated on isExisting, not presence",
        chunk.count("supercharger.isExisting") >= 2 and chunk.count("turbocharger.isExisting") >= 2,
    )

# 11. The telemetry half: the stock combined electric, and the bit that gates the menu.
with open(
    os.path.join(ROOT, "bng_mod", "lua", "vehicle", "protocols", "796F6C6F313035.lua"),
    encoding="utf-8",
) as f:
    protocol = f.read()
check("Current boost is the combined electric", "o.turbo = (electrics.values.boost or 0)" in protocol)
check("Capability bit covers a supercharger", "electrics.values.superchargerBoost\n" in protocol)

print("\nFAILED" if failures else "\nAll passed")
sys.exit(1 if failures else 0)
