"""Checks drift_speech: phrases, interrupt choice, the quiet window and its timeout."""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from drift_speech import QUIET_TIMEOUT_S, DriftAnnouncer, drift_phrase  # noqa: E402


def main():
    # Phrases mirror the display, with "+80" read as "plus 80".
    assert drift_phrase({"event": "shown", "points": 450, "combo": 1.62}) == ("Drift. 450 points, combo 1.6", False)
    assert drift_phrase({"event": "shown", "points": 0, "combo": 1}) == ("Drift", False)
    assert drift_phrase({"event": "hidden"}) == ("Drift over", False)
    assert drift_phrase({"event": "slide", "direction": "left"}) == ("left", False)
    assert drift_phrase({"event": "slide", "direction": "up"}) is None
    assert drift_phrase({"event": "combo", "combo": 2}) == ("Combo 2", False)
    assert drift_phrase({"event": "tier", "text": "Great Drift!"}) == ("Great Drift!", False)
    assert drift_phrase({"event": "quick", "text": "Close call! +80"}) == ("Close call! plus 80", False)
    assert drift_phrase({"event": "fail", "text": "Spinout!", "lost": 640}) == ("Spinout! 640 points lost", True)
    assert drift_phrase({"event": "fail", "text": "Crashed!", "lost": 0}) == ("Crashed!", True)
    assert drift_phrase({"event": "fail", "text": "Spinout!", "lost": 640, "total": 18916}) == (
        "Spinout! 640 points lost. Total 18,916", True)
    assert drift_phrase({"event": "fail", "text": "Spinout!", "total": 2000}) == ("Spinout! Total 2,000", True)
    assert drift_phrase({"event": "fail", "text": "Spinout!", "lost": 0, "total": 0}) == ("Spinout!", True)
    assert drift_phrase({"event": "scored", "points": 1240, "total": 5300}) == ("Plus 1,240 points. Total 5,300", False)
    assert drift_phrase({"event": "scored", "points": None}) is None
    assert drift_phrase({"event": "wrapup", "points": 300}) == ("Plus 300 points for this drift", False)
    assert drift_phrase({"event": "spot_done", "text": "New record! Score: 12000"}) == ("New record! Score: 12000", False)
    assert drift_phrase({"event": "spot_failed", "text": "Drift zone exited"}) == ("Drift zone exited", True)
    assert drift_phrase({"event": "out_of_bounds", "text": "Out of bounds!"}) == ("Out of bounds!", True)
    start = drift_phrase({"event": "spot_start", "name": "Docks", "highScore": 3200,
                          "goals": [{"points": 1000, "done": True}, {"points": 5000, "done": False}]})
    assert start == ("Drift zone Docks. High score 3,200. Goal 5,000 points", False), start
    assert drift_phrase({"event": "spot_start"}) == ("Drift zone started", False)
    assert drift_phrase({"event": "spot_start", "highScore": 0}) == ("Drift zone started. No high score yet", False)
    for bad in (None, "x", {}, {"event": "alive"}, {"event": "nope"}, {"event": "quick", "text": 5}):
        assert drift_phrase(bad) is None, bad

    now = [100.0]
    spoken = []
    announcer = DriftAnnouncer(lambda text, interrupt, source: spoken.append((text, interrupt, source)),
                               clock=lambda: now[0])
    assert not announcer.quiet()
    announcer.handle({"event": "shown", "points": 0, "combo": 1})
    assert announcer.quiet(), "the display mutes speed and compass speech"
    assert spoken[-1] == ("Drift", False, "drift")
    now[0] += QUIET_TIMEOUT_S - 1
    announcer.handle({"event": "alive"})
    now[0] += QUIET_TIMEOUT_S - 1
    assert announcer.quiet(), "keepalives hold the window through a long slide"
    assert len(spoken) == 1, "keepalives are silent"
    announcer.handle({"event": "hidden"})
    assert not announcer.quiet() and spoken[-1][0] == "Drift over"

    announcer.handle({"event": "shown"})
    now[0] += QUIET_TIMEOUT_S + 0.1
    assert not announcer.quiet(), "a lost 'hidden' cannot mute speed and compass for good"

    # A receiver restarted mid-drift missed "shown"; the keepalive restores the window.
    late = DriftAnnouncer(lambda *a, **k: None, clock=lambda: now[0])
    late.handle({"event": "alive"})
    assert late.quiet()

    # Off means silent and nothing muted.
    spoken.clear()
    announcer.set_enabled(False)
    announcer.handle({"event": "shown", "points": 10, "combo": 1})
    announcer.handle({"event": "tier", "text": "Drift!"})
    assert not spoken and not announcer.quiet()
    announcer.set_enabled(True)
    assert announcer.quiet(), "turning it back on mid-display restores the window"
    announcer.handle("garbage")
    announcer.handle({"event": 3})
    print("drift_speech_sim: all diagnostics passed")


if __name__ == "__main__":
    main()
