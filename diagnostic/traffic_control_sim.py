"""Exercise signal event delivery, compatibility, and stale-feed behavior."""
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from road_guidance import RoadGuidanceFeed, parse_r2_packet, signal_phrase


def packet(state="stop", distance=35, **overrides):
    value = {"state": "onRoad", "signalStatus": "available", "signalContext": "italy:1",
             "signal": {"id": "1", "kind": "stopSign" if state == "stop" else "trafficLight",
                        "state": state, "distance": distance,
                        "phase": "near" if distance <= 12 else "approach"}}
    value.update(overrides)
    return parse_r2_packet("R2|" + json.dumps(value))


def main():
    feed = RoadGuidanceFeed()
    assert feed.accept_r2(packet(), now=0)["signal"]
    for i in range(1, 21):
        assert feed.accept_r2(packet(distance=35-i), now=i/20)["signal"] is None
    assert feed.accept_r2(packet(distance=10), now=1.1)["signal"]
    for i in range(1, 31):
        assert feed.accept_r2(packet(distance=10), now=1.1+i/10)["signal"] is None
    assert "Stop sign in 33 feet" in feed.status_phrase(True, "imperial")
    gap = packet(signal=None, signalStatus="none")
    feed.accept_r2(gap, now=4.2)
    assert feed.accept_r2(packet(distance=10), now=4.3)["signal"] is None
    # A later circuit of the same approach re-arms the sign.
    for i in range(1, 61):
        feed.accept_r2(gap, now=4.3+i/10)
    assert feed.accept_r2(packet(), now=10.4)["signal"]
    assert feed.accept_r2(packet(signalContext="westcoast:2"), now=10.5)["signal"]
    assert feed.check_timeout(now=12)
    assert feed.accept_r2(packet(), now=12.1)["signal"]
    feed.reset()
    assert feed.accept_r2(packet(), now=0, stop_signs=False)["signal"] is None
    assert feed.accept_r2(packet(), now=0.1, stop_signs=True)["signal"]
    feed.reset()
    assert feed.accept_r2(packet("red", 8), now=0)["signal"]
    event = feed.accept_r2(packet("green", 8), now=0.1)["signal"]
    assert event["change"] and signal_phrase(event, "metric", change=True) == "Traffic light green."
    assert feed.accept_r2(packet("green", 8), now=0.2)["signal"] is None
    assert feed.accept_r2(packet("yellow", 8), now=0.3)["signal"]["change"]
    for state in ("flashingRed", "flashingYellow", "flashingGreen", "redYellow", "off", "unknown"):
        assert signal_phrase(packet(state)["signal"], "metric")
    old = parse_r2_packet('R2|{"state":"onRoad"}')
    assert old["signalStatus"] == "unsupported" and old["signal"] is None
    feed.reset()
    preview = dict(packet()["signal"], id="preview:stop:1", groupId="stop:1", preview=True)
    event = feed.accept_r2(packet(signal=preview), now=0)["signal"]
    assert "ahead, about" in signal_phrase(event, "metric")
    exact = dict(preview, id="42", preview=False)
    assert feed.accept_r2(packet(signal=exact), now=0.1)["signal"] is None
    light_preview = dict(packet("unknown")["signal"], groupId="lights:2", preview=True)
    assert feed.accept_r2(packet("unknown", signal=light_preview), now=0.2)["signal"]
    red = dict(light_preview, preview=False, state="red")
    event = feed.accept_r2(packet("red", signal=red), now=0.3)["signal"]
    assert event["change"] and signal_phrase(event, "metric", True) == "Traffic light red."
    for fields in ({"distance": -1}, {"distance": float("nan")}, {"distance": True},
                   {"kind": "stopSign", "state": "red"}, {"id": []}, {"state": {}},
                   {"phase": []}):
        bad = dict(packet()["signal"], **fields)
        try:
            packet(signal=bad)
        except ValueError:
            pass
        else:
            raise AssertionError(fields)
    print("traffic_control_sim: all diagnostics passed")


if __name__ == "__main__":
    main()
