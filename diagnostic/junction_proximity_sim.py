"""Exercise stop bar rendering, freshness, mode gates, and wire validation."""
import json
import ast
import io
import logging
import sys
import threading
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import numpy as np
from audio import AudioController, STOP_BAR_HZ, STOP_BAR_SUB_HZ
from road_guidance import RoadGuidanceFeed, parse_r2_packet
from assistant_diagnostics import AssistantDiagnosticRecorder


def main():
    controller = AudioController(logging.getLogger("junction_proximity_sim"))
    controller.apply_config({"road_junction_volume_db": -14})
    sr = controller.samplerate
    target = {"id": "junction:T", "distance": 120, "speed": 20,
              "action": "approach", "source": "junctionBoundary"}

    def render(duration):
        left, right = np.zeros(int(sr * duration)), np.zeros(int(sr * duration))
        controller._mix_junction_proximity(left, right, len(left))
        assert np.array_equal(left, right), "distance cue stays centred"
        return left

    def starts(wave):
        audible = np.abs(wave) > 0.002
        # Group samples across sine-wave zero crossings but preserve pip gaps.
        indices = np.flatnonzero(audible)
        return indices[np.r_[True, np.diff(indices) > sr * 0.015]] / sr

    def rms(wave):
        return float(np.sqrt(np.mean(wave ** 2)))

    def at(signed, speed):
        return dict(target, distance=max(0.0, signed), signedDistance=signed, speed=speed)

    with patch("audio.time.monotonic", return_value=100):
        # With road mode off, assist can still enable the shared cue.
        controller.set_road_mode(False)
        controller.update_junction_proximity(target, enabled=True)
        far = starts(render(4))
        assert len(far) == 3 and 1.45 < far[1] - far[0] < 1.55, far
        controller.update_junction_proximity(at(12, 20))
        near = starts(render(1))
        assert 0.17 < near[1] - near[0] < 0.19, "pulses quicken to a distinct 90 ms on, 90 ms off"

        # A slow approach with a fresh distance every 50 ms: the tempo rises
        # smoothly and no pulse is cut short by an update.
        controller.update_junction_proximity(None)
        render(0.05)
        approach = []
        for step in range(360):
            controller.update_junction_proximity(at(20 - step * 0.05, 1.0))
            approach.append(render(0.05))
        approach = np.concatenate(approach)
        idx = np.flatnonzero(np.abs(approach) > 0.002)
        breaks = np.flatnonzero(np.diff(idx) > sr * 0.015)
        begins, ends = idx[np.r_[0, breaks + 1]], idx[np.r_[breaks, len(idx) - 1]]
        lengths = (ends - begins) / sr
        intervals = np.diff(begins) / sr
        assert len(lengths) > 20 and np.all(lengths[:-1] > 0.075), lengths.min()
        assert np.all(np.diff(intervals) < 0.02), "tempo only ever rises on approach"

        # On the bar: one unbroken tone, D#3 over A2 at half its level.
        controller.update_junction_proximity(at(0.0, 5))
        render(0.1)
        solid = render(1)
        assert len(starts(solid)) == 1, "solid while on the bar"
        t = np.arange(len(solid)) / sr

        def amplitude(hz):
            return 2 * abs(np.mean(solid * np.exp(-2j * np.pi * hz * t)))

        top, sub = amplitude(STOP_BAR_HZ), amplitude(STOP_BAR_SUB_HZ)
        assert abs(STOP_BAR_HZ - 311.13) < 0.01 and abs(STOP_BAR_SUB_HZ - 155.56) < 0.01, "D#4 over D#3"
        assert 0.48 < sub / top < 0.52, "sub oscillator at half volume"
        assert abs(rms(solid) ** 2 / ((top ** 2 + sub ** 2) / 2) - 1) < 0.02, \
            "nothing but the two oscillators"
        moving_level = rms(solid)

        # Stopped on the bar: still solid, 18 dB down after the fade.
        controller.update_junction_proximity(at(-0.5, 0))
        render(1)
        stopped = render(1)
        assert len(starts(stopped)) == 1, "still solid once stopped"
        drop = 20 * np.log10(rms(stopped) / moving_level)
        assert -18.5 < drop < -17.5, f"stopped fade is {drop:.1f} dB"
        # Pulling away past the line: a short release, not a click, then silence.
        controller.update_junction_proximity(at(0.3, 0))
        render(0.1)
        controller.update_junction_proximity(at(0.3, 3))
        render(1)
        assert abs(rms(render(0.5)) / moving_level - 1) < 0.02, "full level once moving again"
        before = controller._stop_bar_env
        controller.update_junction_proximity(at(-0.2, 3))
        release = np.concatenate([render(0.004) for _ in range(10)])
        assert before == 1.0 and controller._stop_bar_env == 0.0
        assert np.max(np.abs(np.diff(release))) < 0.02, "the release does not click"
        audible = np.flatnonzero(np.abs(release) > 1e-4)
        assert 0.015 < audible[-1] / sr < 0.021, "released over about 20 ms"
        assert not np.any(render(0.5)), "silent once departing past the line"
        controller.update_junction_proximity(at(-1.5, 0))
        render(0.1)
        assert not np.any(render(0.5)), "stopping just past the line does not bring the bar back"

        # Rolled onto the bar and stopped past the line: the quiet solid tone holds.
        controller.update_junction_proximity(dict(at(0.2, 0), id="junction:S"))
        render(0.1)
        controller.update_junction_proximity(dict(at(-0.6, 0.1), id="junction:S"))
        render(0.1)
        assert controller._stop_bar_mode[0] == "solid", "stopped past the line still on the bar"
        controller.update_junction_proximity(dict(at(-0.6, 3), id="junction:S"))
        render(0.1)
        assert controller._stop_bar_mode[0] == "silent", "pulling away from it is departing"

        controller.update_junction_proximity(at(-3.0, 3))
        render(0.05)
        assert not np.any(render(0.5)), "silent once the nose has cleared the bar"
        # Distance jitter at the edge of the bar does not flip it once on it.
        controller.update_junction_proximity(dict(at(0.3, 1.0), id="junction:edge"))
        render(0.1)
        for signed in (0.9, 0.4, 1.4, 0.6, 1.2):
            controller.update_junction_proximity(dict(at(signed, 1.0), id="junction:edge"))
            render(0.1)
            assert controller._stop_bar_mode[0] == "solid", f"flipped off the bar at {signed} m"
        controller.update_junction_proximity(dict(at(2.0, 0), id="junction:edge"))
        render(0.05)
        assert not np.any(render(0.5)), "silent once reversed back off the bar"
        # Stopped short of a bar not yet reached: one quiet pulse every 2 s.
        controller.update_junction_proximity(dict(at(10, 0), id="junction:W"))
        waiting = render(4.2)
        w = starts(waiting)
        assert len(w) == 3 and np.all(np.abs(np.diff(w) - 2.0) < 0.05), w
        third = waiting[int(w[2] * sr):int((w[2] + 0.12) * sr)]
        drop = 20 * np.log10(rms(third) / moving_level)
        assert -19.5 < drop < -16.5, f"waiting pulse level {drop:.1f} dB"
        for speed in (0.35, 0.1, 0.45, 0.3):
            controller.update_junction_proximity(dict(at(10, speed), id="junction:W"))
            render(0.1)
            assert controller._stop_bar_mode[0] == "waiting", "creeping noise does not restart the pulses"
        controller.update_junction_proximity(at(10, 0.8))
        render(0.1)
        assert controller._stop_bar_mode[0] == "pulse", "pulses resume once really moving"
        controller.update_junction_proximity(dict(at(12, 20), source="stopPoint", action="stop"))
        # Same target id, so the pulse clock carries on: skip the pulse in progress.
        assert 0.17 < np.diff(starts(render(1)))[1] < 0.19, "stop signs use the same bar"

        controller.trigger_assistant_cue("junction", ("left", "right"))
        assert controller._junction_proximity, "exit tones do not cancel the stop bar"
        controller.update_junction_proximity(target, enabled=False)
        render(0.05)
        assert not np.any(render(1)), "neither driving mode is enabled"
        controller.set_road_mode(True)
        controller.update_junction_proximity(target)
        assert np.any(render(0.1)), "manual road guidance"
        # A private-road entry is spoken, not a place to stop: no bar.
        controller.update_junction_proximity(dict(target, kind="privateEntry"))
        render(0.05)
        assert not np.any(render(1)), "a private-road entry sounds no bar"
        controller.update_junction_proximity(target)
        controller.clear_road_audio()
        render(0.05)
        assert not np.any(render(1)), "loading or off-road reset"
        controller.update_junction_proximity(target)
    with patch("audio.time.monotonic", return_value=101):
        assert not np.any(render(1)), "dead UDP feed silences cue"
        controller.apply_config({"road_junction_proximity_enabled": False})
        controller.update_junction_proximity(target)
        assert not np.any(render(1)), "configuration disables cue"

    def parse(value, state="onRoad"):
        return parse_r2_packet("R2|" + json.dumps({"state": state, "proximity": value}))

    assert parse(target)["proximity"] == dict(target, signedDistance=120, kind="",
                                              privateEntry=""), "old mods"
    passed = dict(target, distance=0, signedDistance=-1.5, kind="gate", privateEntry="")
    assert parse(passed)["proximity"] == passed
    entering = dict(target, kind="junction", privateEntry="private:g2")
    assert parse(entering)["proximity"]["privateEntry"] == "private:g2"
    assert parse(None)["proximity"] is None
    for bad in ([], dict(target, distance=-1), dict(target, speed=float("nan")),
                dict(target, distance=True), dict(target, action="stop"),
                dict(target, id=""), dict(target, source="unknown"),
                dict(target, signedDistance="near"), dict(target, signedDistance=-80),
                dict(target, kind=5), dict(target, privateEntry=7)):
        try:
            parse(bad)
        except ValueError:
            pass
        else:
            raise AssertionError(f"accepted malformed proximity {bad}")
    try:
        parse(target, "offRoad")
    except ValueError:
        pass
    else:
        raise AssertionError("accepted off-road target")

    # Exercise the actual MCP snapshot body without starting the GUI/receiver.
    source = Path(__file__).resolve().parents[1] / "beamtel.py"
    tree = ast.parse(source.read_text(encoding="utf-8"))
    function = next(node for node in tree.body if isinstance(node, ast.FunctionDef)
                    and node.name == "_mcp_snapshot_state")
    namespace = {"state_lock": threading.Lock(), "ROAD_GUIDANCE_FEED": RoadGuidanceFeed(),
                 "audio_controller_ref": controller}
    exec(compile(ast.Module(body=[function], type_ignores=[]), str(source), "exec"), namespace)
    snapshot = namespace["_mcp_snapshot_state"](["road"])
    assert snapshot["road"]["proximity_audio"]["enabled"] is False
    namespace["audio_controller_ref"] = None
    assert namespace["_mcp_snapshot_state"](["road"])["road"]["proximity_audio"] is None

    recorder = AssistantDiagnosticRecorder(Path("unused"))
    recorder.record_road(parse(target), {"active": True})
    assert recorder.sequence == 0, "no recording without an observation session"
    recorder.stream = io.StringIO()
    with patch("assistant_diagnostics.time.monotonic", return_value=100):
        recorder.record_road(parse(target), {"active": True})
        recorder.record_road(parse(target), {"active": True})
    with patch("assistant_diagnostics.time.monotonic", return_value=100.21):
        recorder.record_road(parse(target), {"active": False})
    rows = [json.loads(line) for line in recorder.stream.getvalue().splitlines()]
    assert len(rows) == 2 and rows[0]["data"]["proximity"]["distance"] == 120
    assert rows[1]["data"]["audio"]["active"] is False
    print("junction_proximity_sim: all diagnostics passed")


if __name__ == "__main__":
    main()
