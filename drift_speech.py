"""Speech for the game's free-roam drift display (driftAccessible.lua events).

The Lua side relays only what the display presents, while it presents it. This
turns those events into short phrases and owns the quiet window: while the
display shows, beamtel holds back automatic speed and compass speech.
"""
import re
import threading
import time

# If "hidden" never arrives (bridge reconnect, game reload), stop muting speed
# and compass this long after the last drift event.
QUIET_TIMEOUT_S = 10.0

_PLUS_NUMBER = re.compile(r"\+\s*(\d+)")


def _points(value):
    try:
        number = int(value)
    except (TypeError, ValueError):
        return None
    return f"{number:,}"


def _clean(text):
    """The display's own text, with "+80" read as "plus 80"."""
    if not isinstance(text, str):
        return ""
    return _PLUS_NUMBER.sub(lambda m: f"plus {int(m.group(1)):,}", text).strip()


def drift_phrase(event):
    """(phrase, interrupt) for one event, or None when there is nothing to say."""
    if not isinstance(event, dict):
        return None
    kind = event.get("event")
    if kind == "shown":
        points = _points(event.get("points"))
        try:
            combo = float(event.get("combo", 1))
        except (TypeError, ValueError):
            combo = 1.0
        if points and points != "0":
            return f"Drift. {points} points, combo {combo:.1f}", False
        return "Drift", False
    if kind == "hidden":
        return "Drift over", False
    if kind == "slide":
        direction = event.get("direction")
        return (direction, False) if direction in ("left", "right") else None
    if kind == "combo":
        combo = _points(event.get("combo"))
        return (f"Combo {combo}", False) if combo else None
    if kind in ("tier", "quick", "spot_done", "spot_failed", "spot_highscore", "out_of_bounds"):
        text = _clean(event.get("text"))
        return (text, kind in ("spot_failed", "out_of_bounds")) if text else None
    if kind == "fail":
        text = _clean(event.get("text")) or "Drift lost"
        lost = _points(event.get("lost"))
        if lost and lost != "0":
            text = f"{text} {lost} points lost"
        total = _points(event.get("total"))
        if total and total != "0":
            text = f"{text}{' ' if text.endswith(('!', '.', '?')) else '. '}Total {total}"
        return text, True
    if kind == "scored":
        points = _points(event.get("points"))
        if not points:
            return None
        total = _points(event.get("total"))
        return (f"Plus {points} points. Total {total}" if total else f"Plus {points} points"), False
    if kind == "wrapup":
        points = _points(event.get("points"))
        return (f"Plus {points} points for this drift", False) if points else None
    if kind == "spot_start":
        parts = [f"Drift zone {event['name']}" if isinstance(event.get("name"), str) and event["name"]
                 else "Drift zone started"]
        high = _points(event.get("highScore"))
        if high is not None:
            parts.append(f"High score {high}" if high != "0" else "No high score yet")
        goals = event.get("goals")
        if isinstance(goals, list):
            open_goals = [_points(g.get("points")) for g in goals
                          if isinstance(g, dict) and not g.get("done")]
            open_goals = [g for g in open_goals if g]
            if open_goals:
                parts.append("Goal " + ", then ".join(open_goals) + " points")
        return ". ".join(parts), False
    return None


class DriftAnnouncer:
    def __init__(self, say, enabled=True, clock=time.monotonic):
        self._say = say
        self._clock = clock
        self._lock = threading.Lock()
        self.enabled = enabled
        self._shown = False
        self._last_event = 0.0

    def set_enabled(self, enabled):
        with self._lock:
            self.enabled = bool(enabled)

    def quiet(self):
        """True while the drift display shows: hold automatic speed and compass speech."""
        with self._lock:
            return (self.enabled and self._shown
                    and self._clock() - self._last_event < QUIET_TIMEOUT_S)

    def handle(self, event):
        if not isinstance(event, dict):
            return
        with self._lock:
            kind = event.get("event")
            # "alive" also means shown: a receiver restarted mid-drift missed "shown".
            if kind in ("shown", "alive"):
                self._shown = True
            elif kind == "hidden":
                self._shown = False
            self._last_event = self._clock()
            enabled = self.enabled
        if not enabled:
            return
        result = drift_phrase(event)
        if result:
            phrase, interrupt = result
            self._say(phrase, interrupt=interrupt, source="drift")
