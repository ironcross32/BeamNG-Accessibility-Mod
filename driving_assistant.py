"""Confirmed assistant state and session heartbeat; no steering runs in Python."""
import json
import threading
import time
import uuid


class DrivingAssistant:
    def __init__(self, send, speak, cue=None, diagnostics=None):
        self.send = send
        self.speak = speak
        self.cue = cue or (lambda kind, choices=(): None)
        self.diagnostics = diagnostics
        self.lock = threading.RLock()
        self.token = None
        self.active = False
        self.vehicle = None
        self.sequence = -1
        self.last_response = 0.0
        self.status_token = None

    def snapshot(self):
        with self.lock:
            return {"active": self.active, "pending": bool(self.token) and not self.active,
                    "vehicle": self.vehicle, "sequence": self.sequence,
                    "response_age_s": time.monotonic() - self.last_response if self.last_response else None}

    def _record(self, value):
        if self.diagnostics:
            self.diagnostics.record(value)

    def status(self):
        with self.lock:
            self.status_token = uuid.uuid4().hex
            self.send(f"ASSISTANT_STATUS:{self.status_token}")

    def toggle(self):
        with self.lock:
            if self.token:
                self.send(f"ASSISTANT_OFF:{self.token}")
                return
            self.token = uuid.uuid4().hex
            self.sequence = -1
            self.last_response = time.monotonic()
            self.send(f"ASSISTANT_TOGGLE:{self.token}")

    def receive(self, payload):
        try:
            value = json.loads(payload)
            token, kind = value["token"], value["kind"]
            sequence = int(value.get("sequence", -1))
            message = value.get("message", "")
            if not isinstance(message, str):
                return
        except (ValueError, TypeError, KeyError, OverflowError):
            return
        with self.lock:
            if kind == "status" and token == self.status_token and self.status_token:
                self.status_token = None
                self.speak(message, exclude_from_buffer=True)
                return
            if token != self.token or not self.token or sequence <= self.sequence:
                return
            self.sequence = sequence
            self.last_response = time.monotonic()
            self._record(value)
            if kind == "on":
                self.active = True
                self.vehicle = value.get("vehicle")
            elif kind == "off":
                self.active, self.token, self.vehicle = False, None, None
                self.cue("off")
            elif kind == "junction":
                choices = tuple(name for name in ("left", "straight", "right")
                                if name in message.split(","))
                self.cue("junction", choices)
                return
            elif kind in {"diagnostic", "choice"}:
                return
            elif kind not in {"say", "slow", "status", "alive"}:
                return
            if kind == "alive" and value.get("active") is True and not self.active:
                self.active = True
                self.vehicle = value.get("vehicle")
                self.speak("Driving assistant on. You control speed and stopping",
                           exclude_from_buffer=True)
            if message and kind != "alive":
                self.speak(message, interrupt=kind != "slow", exclude_from_buffer=True)

    def heartbeat(self):
        with self.lock:
            if not self.token:
                return
            if time.monotonic() - self.last_response > 5:
                self.send(f"ASSISTANT_OFF:{self.token}")
                self.active, self.token, self.vehicle = False, None, None
                message = "Driving assistant connection lost. Steering assistance off"
                self._record({"kind": "off", "message": message, "origin": "receiver_timeout"})
                self.cue("off")
                self.speak(message, exclude_from_buffer=True)
            else:
                self.send(f"ASSISTANT_HEARTBEAT:{self.token}")

    def run(self, stop_event):
        # A new receiver session never inherits an old assistant activation.
        self.send("ASSISTANT_OFF")
        while not stop_event.wait(0.5):
            self.heartbeat()
        self.send("ASSISTANT_OFF")
