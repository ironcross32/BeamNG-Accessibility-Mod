"""Run the production road listener across manual/assistant mode handoffs."""
import ast
import json
import logging
import socket
import sys
import threading
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import MagicMock

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from road_guidance import RoadGuidanceFeed, parse_r2_packet, signal_phrase


def main():
    source = Path(__file__).resolve().parents[1] / "beamtel.py"
    tree = ast.parse(source.read_text(encoding="utf-8"))
    functions = [n for n in tree.body if isinstance(n, ast.FunctionDef)
                 and n.name in {"road_listener", "toggle_road_mode"}]
    stop, audio = threading.Event(), MagicMock()
    spoken, commands, pending = [], [], []
    assistant = SimpleNamespace(active=False)
    feed = RoadGuidanceFeed()
    env = dict(ROAD_GUIDANCE_FEED=feed, driving_assistant=assistant,
               _loading_lock=threading.Lock(), _loading_active=False, _loading_settling=False,
               road_mode_active=False, road_stop_sign_speech_enabled=True,
               road_traffic_light_speech_enabled=True, road_speed_limit_speech_enabled=False,
               road_follow_guidance_enabled=False, road_junction_speech_enabled=False,
               road_junction_earcon_enabled=False, UNITS_MODE="imperial", ROAD_LISTEN_PORT=0,
               HILL_CLIMB_CHALLENGE=MagicMock(), ASSISTANT_DIAGNOSTICS=MagicMock(),
               ROAD_DIAGNOSTICS=MagicMock(), _road_diagnostic_telemetry_snapshot=lambda: {},
               parse_r2_packet=parse_r2_packet, signal_phrase=signal_phrase,
               _send_road_command=commands.append, _send_road_configuration=lambda: None,
               logger=logging.getLogger("traffic_mode_sim"),
               say=lambda text, **kw: spoken.append((text, kw.get("source"))))

    def packet(state="stop", identity="sign", distance=35):
        return ("R2|" + json.dumps({"state": "onRoad", "signalContext": "test:1",
            "signalStatus": "available", "signal": {"id": identity,
            "kind": "stopSign" if state == "stop" else "trafficLight", "state": state,
            "distance": distance, "phase": "near" if distance <= 12 else "approach"}})).encode()

    def toggle():
        env["toggle_road_mode"](audio)

    def assist(value):
        return lambda: setattr(assistant, "active", value)

    def check(count, fragment=None):
        def run():
            signals = [text for text, source in spoken if source == "traffic_control"]
            assert len(signals) == count, signals
            if fragment:
                assert fragment in signals[-1], signals
        return run

    # Each assertion executes before the next packet: this catches both the
    # listener's speech gate and resets performed by the real mode toggle.
    pending.extend([
        packet(), check(0),
        assist(True), packet(), check(1, "Stop sign"),
        toggle, packet(), check(1),  # both on, preserve the existing sign
        toggle, packet(), check(1),  # road off, assistance remains
        packet(distance=8), check(2, "Stop sign"),
        packet("red", "light"), check(3, "red"),
        toggle, packet("red", "light"), check(3),
        assist(False), packet("green", "light"), check(4, "Traffic light green."),
        toggle, packet("yellow", "light"), check(4),  # neither mode on
        assist(True), packet("yellow", "light"), check(5, "yellow"),
        lambda: env.update(road_stop_sign_speech_enabled=False), packet(identity="other"), check(5),
        packet("green", "light"), check(6, "green"),
        lambda: env.update(road_traffic_light_speech_enabled=False), packet("red", "light"), check(6),
        lambda: env.update(road_stop_sign_speech_enabled=True), packet(identity="other"), check(7),
        lambda: env.update(_loading_active=True), packet(identity="loading"), check(7),
    ])

    class Socket:
        def bind(self, address): pass
        def settimeout(self, timeout): pass
        def close(self): pass
        def recvfrom(self, size):
            while pending:
                event = pending.pop(0)
                if callable(event):
                    event()
                else:
                    return event, ("127.0.0.1", 1234)
            stop.set()
            raise socket.timeout()

    env["socket"] = SimpleNamespace(socket=Socket, AF_INET=2, SOCK_DGRAM=2, timeout=socket.timeout)
    env["socket"].socket = lambda *args: Socket()
    exec(compile(ast.Module(body=functions, type_ignores=[]), str(source), "exec"), env)
    env["road_listener"](audio, stop)
    assert commands == ["ON", "OFF", "ON", "OFF"]
    print("traffic_mode_sim: all mode handoffs and preference gates passed")


if __name__ == "__main__":
    main()
