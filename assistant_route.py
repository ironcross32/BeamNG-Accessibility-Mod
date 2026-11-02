"""Spoken guidance while the driving assistant follows a map route.

Deliberately has no BeamTel, audio or speech imports, so the callout rules and wording
can be exercised without starting the application. Same premise as :mod:`route_beacon`.
Distances arrive in metres; the units are only decided here, at the speech boundary.
"""

from __future__ import annotations

from route_beacon import format_long_distance

_FEET_PER_METRE = 3.28084
_METRES_PER_MILE = 1609.344

# Largest first. Each is spoken at most once per stretch of road.
_IMPERIAL_CALLOUTS = (
    (2 * _METRES_PER_MILE, "2 miles"),
    (_METRES_PER_MILE, "1 mile"),
    (0.5 * _METRES_PER_MILE, "half a mile"),
    (1000 / _FEET_PER_METRE, "1000 feet"),
    (500 / _FEET_PER_METRE, "500 feet"),
)
_METRIC_CALLOUTS = (
    (2000.0, "2 kilometers"),
    (1000.0, "1 kilometer"),
    (500.0, "500 meters"),
    (200.0, "200 meters"),
)

# "Turn left now" comes this early: three seconds' notice, never under 40 m.
_NOW_MIN_M = 40.0
_NOW_SECONDS = 3.0
# A callout just under the distance already spoken would only repeat it.
_CALLOUT_MARGIN = 0.8

# Only the two toasts the assistant says better itself. The wording is matched as well
# as the key because the UI bridge can deliver either, and only for this language:
# in another language the toast is simply spoken as well, which costs nothing.
_ARRIVAL_TOASTS = ("you have reached your destination", "bigmap.info.reachedtarget")
_U_TURN_TOASTS = ("please make a u-turn when possible", "ui.groundmarkers.wrongdirection")
_ARRIVAL_TOAST_WINDOW_S = 3.0


def _imperial(units):
    return str(units).lower().startswith("imp")


def spoken_distance(metres, units):
    """A distance fit to speak: no "1437 feet", and no "1.0 miles"."""
    value = max(0.0, float(metres))
    if _imperial(units):
        feet = value * _FEET_PER_METRE
        if feet < 1000.0:
            return f"{max(50, round(feet / 50.0) * 50):.0f} feet"
        miles = feet / 5280.0
        if round(miles, 1) == 1.0:
            return "1 mile"
    elif value < 1000.0:
        return f"{max(50, round(value / 50.0) * 50):.0f} meters"
    elif round(value / 1000.0, 1) == 1.0:
        return "1 kilometer"
    return format_long_distance(value, units).replace(".0 ", " ")


def turn_instruction(name):
    """``left`` -> ``turn left``; forks and roundabouts have their own verbs."""
    if name == "roundabout":
        return "take the roundabout"
    if name.startswith("keep"):
        return name
    return f"turn {name}"


def _capitalised(text):
    return text[:1].upper() + text[1:]


def relative_direction(bearing):
    """Positive is the driver's left."""
    size = abs(float(bearing))
    if size < 5.0:
        return "straight ahead"
    if size > 175.0:
        return "directly behind you"
    return f"{size:.0f} degrees to your {'left' if bearing > 0 else 'right'}"


def arrival_phrase(arrival, units):
    """The sentence that ends a route."""
    if arrival.get("offRoad") and arrival.get("distance") is not None and arrival.get("bearing") is not None:
        return (
            f"Destination off road, {spoken_distance(arrival['distance'], units)}, "
            f"{relative_direction(arrival['bearing'])}. Waypoint marked. Assistant off"
        )
    return "Destination reached. Assistant off"


def suppressed_toast(text, following_route, seconds_since_arrival):
    """Whether a stock toast would only contradict or talk over the assistant.

    The arrival toast lands just after the assistant's own arrival sentence and would
    interrupt it. The U-turn toast is simply wrong while following a route: that route is
    forward only, and goes round the block rather than turning in the road.
    """
    lowered = str(text).strip().lower()
    if (
        seconds_since_arrival is not None
        and seconds_since_arrival <= _ARRIVAL_TOAST_WINDOW_S
        and any(toast in lowered for toast in _ARRIVAL_TOASTS)
    ):
        return True
    return bool(following_route) and any(toast in lowered for toast in _U_TURN_TOASTS)


class RouteCallouts:
    """Decides what to say from the assistant's twice-a-second route reports."""

    def __init__(self, units=None):
        self._units = units or (lambda: "imperial")
        self.reset()

    def reset(self):
        self.following = False
        self._target = None
        self._callouts = []
        self._now_said = False

    def _begin_stretch(self, target, distance):
        self._target = target
        self._now_said = False
        table = _IMPERIAL_CALLOUTS if _imperial(self._units()) else _METRIC_CALLOUTS
        self._callouts = [row for row in table if row[0] < distance * _CALLOUT_MARGIN]

    @staticmethod
    def _stretch(report):
        """Which piece of road is being counted down, and how long it is."""
        if report.get("turnId") is not None and report.get("turnDistance") is not None:
            return ("turn", report["turnId"]), float(report["turnDistance"])
        return ("destination",), float(report.get("destDistance") or 0.0)

    def _now_distance(self, report):
        return max(_NOW_MIN_M, float(report.get("speed") or 0.0) * _NOW_SECONDS)

    def _continue_phrase(self, report, distance):
        units = self._units()
        if report.get("turn"):
            if distance <= self._now_distance(report):
                self._now_said = True
                self._callouts = []
                return f"{_capitalised(turn_instruction(report['turn']))} now"
            return f"Continue {spoken_distance(distance, units)}, then {turn_instruction(report['turn'])}"
        place = "the closest road point" if report.get("offRoad") else "the destination"
        return f"Continue {spoken_distance(distance, units)} to {place}"

    def route_set(self, report):
        """A new route: the whole distance, then the first thing to do on it."""
        self.following = True
        target, distance = self._stretch(report)
        self._begin_stretch(target, distance)
        units = self._units()
        total = spoken_distance(report.get("destDistance") or 0.0, units)
        parts = [f"Route set. {total} by road."]
        if report.get("offRoad"):
            parts.append("Destination is off the road. Going to the closest road point.")
        parts.append(self._continue_phrase(report, distance))
        return " ".join(parts)

    def update(self, report):
        """Returns the sentence this report calls for, or None."""
        if not self.following:
            # A receiver that missed route_set picks up the count without a speech.
            self.following = True
            target, distance = self._stretch(report)
            self._begin_stretch(target, distance)
            return None
        target, distance = self._stretch(report)
        if target != self._target:
            self._begin_stretch(target, distance)
            return self._continue_phrase(report, distance)
        turn = report.get("turn")
        if turn and not self._now_said and distance <= self._now_distance(report):
            self._now_said = True
            self._callouts = []
            return f"{_capitalised(turn_instruction(turn))} now"
        passed = None
        while self._callouts and distance <= self._callouts[0][0]:
            passed = self._callouts.pop(0)[1]
        if passed is None:
            return None
        if turn:
            return f"In {passed}, {turn_instruction(turn)}"
        return f"{'Closest road point' if report.get('offRoad') else 'Destination'} in {passed}"
