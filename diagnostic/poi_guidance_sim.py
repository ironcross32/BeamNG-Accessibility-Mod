"""Offline checks for the F9, Shift+W nearest-POI readout.

Run with::

    uv run python diagnostic/poi_guidance_sim.py
"""

import os
import sys


ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, ROOT)

from poi_guidance import parse_poi_packet, poi_phrase  # noqa: E402


failures = []


def check(label, condition, detail=""):
    print(("OK   " if condition else "FAIL ") + label)
    if not condition:
        failures.append(f"{label}: {detail}")


packet = (
    'POI:{"name":"Hill Climb - Base","description":"Foot &amp; start<br/>Asphalt",'
    '"distance":3.2,"bearing":91,"radius":0}'
)
poi = parse_poi_packet(packet)
parked = poi_phrase(poi, "imperial")
check("HTML is converted to clean speech", poi["description"] == "Foot & start. Asphalt", poi)
check("inside the fallback footprint says parked", parked.endswith("Parked on it."), parked)
check("parked wording ends the announcement", "feet" not in parked and "bearing" not in parked, parked)

poi = parse_poi_packet(
    'POI:{"name":"Garage","description":"Repairs","distance":108,'
    '"bearing":-27.4,"radius":8}'
)
imperial = poi_phrase(poi, "imperial")
metric = poi_phrase(poi, "metric")
check("distance is measured to the POI footprint", "328 feet" in imperial, imperial)
check("imperial wording includes a right bearing", "bearing 27 degrees right" in imperial, imperial)
check("metric wording uses metres", "100 meters" in metric, metric)

ahead = poi_phrase(
    parse_poi_packet(
        'POI:{"name":"Fuel","distance":18,"bearing":0.2,"radius":0}'
    ),
    "metric",
)
check("near-zero bearing is unambiguous", "bearing 0 degrees, straight ahead" in ahead, ahead)

for malformed in (
    "SCAN,1",
    "POI:not-json",
    'POI:{"name":"Broken"}',
    'POI:{"distance":-1,"bearing":0}',
):
    try:
        parse_poi_packet(malformed)
    except ValueError:
        pass
    else:
        failures.append(f"malformed packet accepted: {malformed}")
check("malformed navigation packets are rejected", not any("malformed packet" in f for f in failures))

with open(os.path.join(ROOT, "beamtel.py"), encoding="utf-8") as source_file:
    source = source_file.read()
check(
    "F9 Shift+W is documented in input help and sends the POI request",
    '("w", False, True, False): "Nearest point of interest"' in source
    and '_send_scan_cmd("NEAREST_POI")' in source,
)
check(
    "scan and POI requests keep independent reply latches",
    "_last_scan_reply_ts" in source and "_last_poi_reply_ts" in source,
)

if failures:
    print(f"{len(failures)} FAILURE(S)")
    for failure in failures:
        print(" - " + failure)
    raise SystemExit(1)
print("all checks passed")
