"""Map limit validation, stable announcements, units, and stale-data checks."""

import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from road_guidance import RoadGuidanceFeed, map_speed_limit_phrase, parse_r2_packet


def packet(value=10, **extra):
    return parse_r2_packet("R2|" + json.dumps(dict(
        state="onRoad", speedLimit=value, signalContext="map:1", **extra
    )))


def main():
    assert parse_r2_packet('R2|{"state":"onRoad"}')["speedLimit"] is None
    for value in (True, False, 0, -1, "nan", float("inf"), [], {}):
        try:
            packet(value)
        except ValueError:
            pass
        else:
            raise AssertionError(f"accepted invalid limit: {value}")
    try:
        parse_r2_packet('R2|{"state":"offRoad","speedLimit":10}')
    except ValueError:
        pass
    else:
        raise AssertionError("accepted off-road limit")
    assert map_speed_limit_phrase(8.333333333333, "metric") == "Map speed limit 30 kilometers per hour."
    assert map_speed_limit_phrase(8.333333333333, "imperial") == "Map speed limit 19 mph."
    assert "unavailable" in map_speed_limit_phrase(None, "imperial")

    now = [0.0]
    feed = RoadGuidanceFeed(clock=lambda: now[0])

    def sample(value=10, enabled=True, **extra):
        now[0] += 0.2
        return feed.accept_r2(packet(value, **extra), speed_limits=enabled)["speedLimit"]

    def hold(value=10, count=5, enabled=True, **extra):
        return [event for _ in range(count)
                if (event := sample(value, enabled=enabled, **extra)) is not None]

    assert hold() == [{"value": 10}]
    assert hold(count=30) == [], "same limit on successive edges is silent"
    assert sample(20) is None
    assert hold() == [], "brief edge flicker does not announce"
    assert sample(None) is None
    assert hold() == [], "brief data gap does not repeat the old limit"
    assert hold(20) == [{"value": 20}]
    assert "45 mph" in feed.speed_limit_phrase(True, "imperial")
    assert "unavailable" in feed.speed_limit_phrase(False, "imperial")
    assert hold(20, enabled=False) == []
    assert hold(20) == [{"value": 20}], "reenabling announces the current limit"
    assert hold(None, count=18) == [{"value": None}]
    assert hold(None, count=20) == [], "unknown is announced once, only after a known limit"
    assert hold(10) == [{"value": 10}]
    now[0] += 2
    assert "unavailable" in feed.speed_limit_phrase(True, "imperial")
    assert hold(10) == [{"value": 10}], "reconnect must announce again"
    feed.reset()
    assert hold(None, count=20) == [], "unsupported maps do not chatter"
    assert hold() == [{"value": 10}]
    for _ in range(20):
        now[0] += 0.2
        assert feed.accept_r2(parse_r2_packet('R2|{"state":"offRoad","signalContext":"map:1"}'))["speedLimit"] is None
    assert hold() == [{"value": 10}], "rejoining after an extended departure reannounces"
    print("speed_limit_sim: all diagnostics passed")


if __name__ == "__main__":
    main()
