"""Route guidance wording and callout rules, without the GUI or audio application.

python diagnostic/assistant_route_sim.py
"""
import json
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from assistant_route import (
    RouteCallouts,
    arrival_phrase,
    relative_direction,
    spoken_distance,
    suppressed_toast,
    turn_instruction,
)
from driving_assistant import DrivingAssistant
from road_guidance import RoadGuidanceFeed

MILE = 1609.344
FOOT = 1 / 3.28084


def report(turn_distance=None, turn="left", turn_id=1, dest=5000.0, speed=10.0, off_road=None):
    value = {"destDistance": dest, "speed": speed}
    if turn_distance is not None:
        value.update(turnId=turn_id, turn=turn, turnDistance=turn_distance)
    if off_road:
        value["offRoad"] = True
    return value


class Wording(unittest.TestCase):
    def test_distances_are_fit_to_speak(self):
        self.assertEqual(spoken_distance(437 * FOOT, "imperial"), "450 feet")
        self.assertEqual(spoken_distance(MILE, "imperial"), "1 mile")
        self.assertEqual(spoken_distance(3.2 * MILE, "imperial"), "3.2 miles")
        self.assertEqual(spoken_distance(2 * MILE, "imperial"), "2 miles")
        self.assertEqual(spoken_distance(437, "metric"), "450 meters")
        self.assertEqual(spoken_distance(1000, "metric"), "1 kilometer")
        self.assertEqual(spoken_distance(3200, "metric"), "3.2 kilometers")
        self.assertEqual(spoken_distance(2000, "metric"), "2 kilometers")
        self.assertEqual(spoken_distance(3, "metric"), "50 meters")

    def test_instructions(self):
        self.assertEqual(turn_instruction("left"), "turn left")
        self.assertEqual(turn_instruction("sharp right"), "turn sharp right")
        self.assertEqual(turn_instruction("keep left"), "keep left")
        self.assertEqual(turn_instruction("roundabout"), "take the roundabout")

    def test_bearing_sign_is_positive_left(self):
        self.assertEqual(relative_direction(40), "40 degrees to your left")
        self.assertEqual(relative_direction(-40), "40 degrees to your right")
        self.assertEqual(relative_direction(2), "straight ahead")
        self.assertEqual(relative_direction(-178), "directly behind you")

    def test_arrival(self):
        self.assertEqual(
            arrival_phrase({"offRoad": False, "speed": 0.5}, "imperial"),
            "Destination reached. Assistant off",
        )
        self.assertEqual(
            arrival_phrase({"offRoad": True, "speed": 0, "distance": 140 * FOOT, "bearing": 40}, "imperial"),
            "Destination off road, 150 feet, 40 degrees to your left. Waypoint marked. Assistant off",
        )
        self.assertIn("100 meters, 90 degrees to your right",
                      arrival_phrase({"offRoad": True, "speed": 0, "distance": 100, "bearing": -90}, "metric"))

    def test_toasts(self):
        arrived = "You have reached your destination."
        u_turn = "Please make a U-turn when possible"
        self.assertTrue(suppressed_toast(arrived, False, 1.0))
        self.assertFalse(suppressed_toast(arrived, False, 10.0), "Long after: it is the game's to say")
        self.assertFalse(suppressed_toast(arrived, False, None))
        self.assertTrue(suppressed_toast(u_turn, True, None))
        self.assertFalse(suppressed_toast(u_turn, False, None), "Driving by hand it is still useful")
        self.assertFalse(suppressed_toast("Fuel low", True, 1.0))


class Callouts(unittest.TestCase):
    def setUp(self):
        self.units = "imperial"
        self.callouts = RouteCallouts(lambda: self.units)

    def spoken(self, distances, **kwargs):
        out = []
        for distance in distances:
            phrase = self.callouts.update(report(distance, **kwargs))
            if phrase:
                out.append(phrase)
        return out

    def test_route_set(self):
        phrase = self.callouts.route_set(report(1.4 * MILE, dest=3.2 * MILE))
        self.assertEqual(phrase, "Route set. 3.2 miles by road. Continue 1.4 miles, then turn left")
        self.assertTrue(self.callouts.following)

    def test_route_set_to_off_road_destination(self):
        phrase = self.callouts.route_set(report(dest=2000, off_road=True))
        self.assertIn("Destination is off the road", phrase)
        self.assertIn("to the closest road point", phrase)

    def test_each_threshold_once_and_only_ahead(self):
        self.callouts.route_set(report(1.4 * MILE))
        said = self.spoken([1.2 * MILE, 0.99 * MILE, 0.98 * MILE, 0.49 * MILE,
                            999 * FOOT, 499 * FOOT, 400 * FOOT])
        self.assertEqual(said, ["In 1 mile, turn left", "In half a mile, turn left",
                                "In 1000 feet, turn left", "In 500 feet, turn left"])

    def test_metric(self):
        self.units = "metric"
        self.callouts.route_set(report(3000))
        self.assertEqual(self.spoken([1990, 990, 490, 190]),
                         ["In 2 kilometers, turn left", "In 1 kilometer, turn left",
                          "In 500 meters, turn left", "In 200 meters, turn left"])

    def test_a_long_gap_speaks_only_the_nearest(self):
        self.callouts.route_set(report(1.4 * MILE))
        self.assertEqual(self.spoken([400 * FOOT]), ["In 500 feet, turn left"])

    def test_threshold_near_the_opening_sentence_is_not_repeated(self):
        self.callouts.route_set(report(1050 * FOOT))
        self.assertEqual(self.spoken([990 * FOOT]), [], "1000 feet would only repeat 1050")

    def test_now_scales_with_speed_and_ends_the_count(self):
        self.callouts.route_set(report(300))
        self.assertEqual(self.spoken([89], speed=30.0), ["Turn left now"])
        self.assertEqual(self.spoken([80, 20], speed=30.0), [], "Said once")
        self.callouts.route_set(report(100, turn_id=2))
        self.assertEqual(self.spoken([60], speed=5.0, turn_id=2), [], "40 m floor when slow")
        self.assertEqual(self.spoken([39], speed=5.0, turn_id=2), ["Turn left now"])

    def test_passing_a_turn_gives_the_next_stretch(self):
        self.callouts.route_set(report(300))
        self.assertEqual(self.callouts.update(report(600 * FOOT, turn="right", turn_id=2)),
                         "Continue 600 feet, then turn right")
        self.assertEqual(self.callouts.update(report(dest=2000 * FOOT)),
                         "Continue 0.4 miles to the destination")
        self.assertEqual(self.callouts.update(report(dest=490 * FOOT)), "Destination in 500 feet")

    def test_next_turn_right_after_the_last(self):
        self.callouts.route_set(report(300))
        self.assertEqual(self.callouts.update(report(20, turn="right", turn_id=2)), "Turn right now")
        self.assertIsNone(self.callouts.update(report(10, turn="right", turn_id=2)))

    def test_missed_route_set_counts_silently(self):
        self.assertIsNone(self.callouts.update(report(1000)))
        self.assertTrue(self.callouts.following)


class Session(unittest.TestCase):
    def setUp(self):
        self.sent, self.spoken, self.arrivals = [], [], []
        self.client = DrivingAssistant(
            self.sent.append, lambda text, **kw: self.spoken.append((text, kw)),
            units=lambda: "metric", on_arrival=self.arrivals.append)
        self.client.toggle()
        self.sequence = 0

    def send(self, kind, message="", data=None):
        self.sequence += 1
        self.client.receive(json.dumps({"token": self.client.token, "kind": kind, "sequence": self.sequence,
                                        "vehicle": 42, "message": message, "data": data}))

    def test_route_flow(self):
        self.send("on", "Driving assistant on")
        self.send("route_set", "", report(1400, dest=3000))
        self.assertEqual(self.spoken[-1][0], "Route set. 3 kilometers by road. Continue 1.4 kilometers, then turn left")
        self.assertFalse(self.spoken[-1][1]["interrupt"], "It must not cut off 'Driving assistant on'")
        self.assertTrue(self.client.snapshot()["following_route"])
        count = len(self.spoken)
        self.send("route", "", report(1300, dest=2900))
        self.assertEqual(len(self.spoken), count, "No threshold crossed: silence")
        self.send("route", "", report(990, dest=2790))
        self.assertEqual(self.spoken[-1][0], "In 1 kilometer, turn left")

    def test_arrival_off_road_hands_over_the_position(self):
        self.send("on")
        self.send("route_set", "", report(dest=3000, off_road=True))
        arrival = {"offRoad": True, "speed": 0, "distance": 100, "bearing": -90, "x": 5.0, "y": 6.0}
        self.send("off", "Destination reached. Assistant off", {"arrival": arrival})
        self.assertIn("100 meters, 90 degrees to your right", self.spoken[-1][0])
        self.assertEqual(self.arrivals, [arrival])
        self.assertFalse(self.client.active)
        self.assertFalse(self.client.snapshot()["following_route"])
        self.assertIsNotNone(self.client.seconds_since_arrival())

    def test_ordinary_off_is_no_arrival(self):
        self.send("on")
        self.send("off", "Driving assistant off")
        self.assertEqual(self.spoken[-1][0], "Driving assistant off")
        self.assertEqual(self.arrivals, [])
        self.assertIsNone(self.client.seconds_since_arrival())

    def test_route_cleared(self):
        self.send("on")
        self.send("route_set", "", report(1000))
        self.send("route_clear", "Route cleared. Turns are yours")
        self.assertEqual(self.spoken[-1][0], "Route cleared. Turns are yours")
        self.assertFalse(self.client.snapshot()["following_route"])

    def test_turn_instructions_protect_themselves(self):
        cues = []
        self.client.cue = lambda kind, choices=(): cues.append(kind)
        self.send("say", "Three point turn. Forward", {"protect": True})
        self.assertEqual(self.spoken[-1][0], "Three point turn. Forward")
        self.assertEqual(cues, ["protect"], "The compass must not talk over a gear change")
        self.send("say", "Back on the road")
        self.assertEqual(cues, ["protect"], "Ordinary speech opens no window")

    def test_malformed_reports_are_dropped(self):
        self.send("on")
        for data in (None, "x", {"turnId": 1, "turnDistance": "far"}):
            self.send("route", "", data)
        self.assertTrue(self.client.active)


class SignalsWithoutRoadGuidance(unittest.TestCase):
    def test_feed_reports_with_assistant_gate(self):
        # beamtel.py passes `road_mode_active or driving_assistant.active`: road guidance
        # off with the assistant on is the True case, and the same event must come out.
        feed = RoadGuidanceFeed(clock=lambda: 0.0)
        packet = {"state": "onRoad", "signalContext": "c", "signalStatus": "available", "junction": None,
                  "signal": {"id": "s", "kind": "stopSign", "state": "stop", "phase": "approach",
                             "distance": 100.0, "groupId": "s", "preview": False}}
        self.assertEqual(feed.accept_r2(packet, now=0.0, stop_signs=True)["signal"]["kind"], "stopSign")
        source = (Path(__file__).resolve().parents[1] / "beamtel.py").read_text(encoding="utf-8")
        self.assertIn("awareness_active = controls_ready and (road_mode_active or driving_assistant.active)", source)


if __name__ == "__main__":
    unittest.main()
