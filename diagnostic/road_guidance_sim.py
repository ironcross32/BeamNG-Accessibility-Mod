"""Deterministic R2 protocol, wording, fallback, and event diagnostics."""

from __future__ import annotations

import json
import os
import sys
import ast
from pathlib import Path

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from road_guidance import (  # noqa: E402
    RoadGuidanceFeed,
    direction_list,
    junction_phrase,
    parse_r2_packet,
)


class Clock:
    def __init__(self):
        self.now = 0.0

    def __call__(self):
        return self.now


def packet(**overrides):
    value = {
        "state": "onRoad",
        "oneWay": False,
        "roadDirections": [4, -176],
        "offRoad": None,
        "correction": {"active": False, "bearing": 0, "severity": 0},
        "junction": None,
    }
    value.update(overrides)
    return parse_r2_packet("R2|" + json.dumps(value))


def expect_bad(payload):
    try:
        parse_r2_packet(payload)
    except ValueError:
        return
    raise AssertionError(f"accepted malformed packet: {payload}")


def read_defaults(path):
    tree = ast.parse(Path(path).read_text(encoding="utf-8"))
    for node in tree.body:
        if isinstance(node, ast.Assign) and any(
            isinstance(target, ast.Name) and target.id == "DEFAULT_CONFIG"
            for target in node.targets
        ):
            return ast.literal_eval(node.value)
    raise AssertionError(f"DEFAULT_CONFIG not found in {path}")


def main():
    root = Path(__file__).resolve().parent.parent
    runtime_defaults = read_defaults(root / "beamtel.py")
    tool_defaults = read_defaults(root / "configurator.py")
    road_keys = (
        "road_follow_guidance_enabled",
        "road_junction_speech_enabled",
        "road_junction_earcon_enabled",
        "road_include_private",
        "road_beep_volume_db",
        "road_correction_volume_db",
        "road_junction_volume_db",
    )
    assert all(runtime_defaults[key] == tool_defaults[key] for key in road_keys)

    valid = packet(
        correction={"active": True, "bearing": 22, "severity": 1.8},
        junction={
            "id": "n1+n2",
            "phase": "approach",
            "kind": "crossroads",
            "distance": 60.96,
            "exits": [90, 0, -90],
        },
    )
    assert valid["correction"]["severity"] == 1.0
    expect_bad("R2|not json")
    expect_bad('R2|{"state":"flying"}')
    expect_bad('R2|{"state":"onRoad","roadDirections":[NaN]}')
    expect_bad('R2|{"state":"onRoad","junction":{"id":"x","phase":"later"}}')
    expect_bad(
        'R2|{"state":"onRoad","junction":{"id":"x","phase":"near","entered":1}}'
    )

    assert direction_list([90, 0, -90]) == "left, straight, and right"
    assert direction_list([30, -30], "or") == "slight left or slight right"
    assert junction_phrase(valid["junction"], "imperial") == (
        "Crossroads in 200 feet: left, straight, and right."
    )
    metric_t = {
        "id": "t",
        "phase": "approach",
        "kind": "tJunction",
        "distance": 40,
        "exits": [90, -90],
    }
    assert junction_phrase(metric_t, "metric") == (
        "T-junction in 40 meters: left or right."
    )
    assert junction_phrase(
        {"id": "d", "phase": "approach", "kind": "deadEnd", "distance": 9.1, "exits": []},
        "metric",
    ) == "Road ends in 9 meters."

    clock = Clock()
    feed = RoadGuidanceFeed(clock)
    first = feed.accept_r2(packet())
    assert first["orientation"]
    assert not feed.accept_r2(packet())["orientation"]
    rearmed = packet(orientation=True, oneWay=True, roadDirections=[-12])
    assert feed.accept_r2(rearmed)["orientation"]

    event_packet = packet(junction=valid["junction"])
    assert feed.accept_r2(event_packet)["junction"] is not None
    assert feed.accept_r2(event_packet)["junction"] is None
    near = dict(valid["junction"], phase="near", distance=20)
    assert feed.accept_r2(packet(junction=near))["junction"]["phase"] == "near"
    assert feed.accept_r2(packet(junction=near))["junction"] is None
    entered = dict(near, entered=True, distance=0)
    assert feed.accept_r2(packet(junction=entered))["junction"]["phase"] == "entered"
    assert feed.accept_r2(packet(junction=entered))["junction"] is None
    for _ in range(3):
        feed.accept_r2(packet(junction=None))
    assert feed.accept_r2(event_packet)["junction"] is not None

    clock.now = 1.01
    assert feed.check_timeout()
    assert feed.snapshot()["mode"] == "stale"
    assert "stale" in feed.status_phrase(True, "metric").lower()

    # A legacy datagram arriving after the grace period immediately enables fallback.
    assert feed.accept_legacy("OFF_ROAD", bearing=45, distance=10)
    assert feed.snapshot()["mode"] == "legacy"
    legacy_status = feed.status_phrase(True, "imperial")
    assert "Legacy guidance only" in legacy_status
    assert "33 feet" in legacy_status

    fresh = RoadGuidanceFeed(clock)
    assert fresh.status_phrase(False, "metric") == "Road guidance is off."
    assert "unavailable" in fresh.status_phrase(True, "metric")
    fresh.accept_legacy("ON_ROAD")
    assert "Legacy road detector only" in fresh.status_phrase(True, "metric")

    no_match = RoadGuidanceFeed(clock)
    no_match.accept_r2(packet(state="offRoad", offRoad=None, correction=None))
    assert "No vertically compatible road" in no_match.status_phrase(True, "metric")

    status_feed = RoadGuidanceFeed(clock)
    status_feed.accept_r2(
        packet(
            oneWay=True,
            roadDirections=[10],
            correction={"active": True, "bearing": -50, "severity": 0.4},
            junction=metric_t,
        )
    )
    status = status_feed.status_phrase(True, "metric")
    for phrase in ("One-way road", "Correction needed: right", "T-junction"):
        assert phrase in status

    print("road_guidance_sim: all diagnostics passed")


if __name__ == "__main__":
    main()
