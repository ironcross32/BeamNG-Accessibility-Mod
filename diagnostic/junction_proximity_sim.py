"""Exercise distance cue rendering, freshness, mode gates, and wire validation."""
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
from audio import AudioController
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

    with patch("audio.time.monotonic", return_value=100):
        # With road mode off, assist can still enable the shared cue.
        controller.set_road_mode(False)
        controller.update_junction_proximity(target, enabled=True)
        far = starts(render(4))
        assert len(far) == 3 and 1.45 < far[1] - far[0] < 1.55
        controller.update_junction_proximity(None)
        controller.update_junction_proximity(dict(target, distance=12))
        near = starts(render(1))
        assert len(near) == 4 and 0.29 < near[1] - near[0] < 0.31
        controller.update_junction_proximity(dict(target, source="stopPoint", action="stop"))
        stop = starts(render(0.3))
        assert len(stop) == 2 and 0.09 < stop[1] - stop[0] < 0.11
        controller.update_junction_proximity(target)
        assert len(starts(render(0.3))) == 1, "green change removes second pip immediately"
        controller.trigger_assistant_cue("junction", ("left", "right"))
        assert controller._junction_proximity, "exit tones do not cancel distance cue"
        controller.update_junction_proximity(target, enabled=False)
        assert not np.any(render(1)), "neither driving mode is enabled"
        controller.set_road_mode(True)
        controller.update_junction_proximity(target)
        assert np.any(render(0.1)), "manual road guidance"
        controller.update_junction_proximity(dict(target, speed=0))
        assert not np.any(render(1)), "quiet while stopped at a red light"
        controller.update_junction_proximity(target)
        controller.clear_road_audio()
        assert not np.any(render(1)), "loading or off-road reset"
        controller.update_junction_proximity(target)
    with patch("audio.time.monotonic", return_value=101):
        assert not np.any(render(1)), "dead UDP feed silences cue"
        controller.apply_config({"road_junction_proximity_enabled": False})
        controller.update_junction_proximity(target)
        assert not np.any(render(1)), "configuration disables cue"

    def parse(value, state="onRoad"):
        return parse_r2_packet("R2|" + json.dumps({"state": state, "proximity": value}))

    assert parse(target)["proximity"] == target
    assert parse(None)["proximity"] is None
    for bad in ([], dict(target, distance=-1), dict(target, speed=float("nan")),
                dict(target, distance=True), dict(target, action="stop"),
                dict(target, id=""), dict(target, source="unknown")):
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
