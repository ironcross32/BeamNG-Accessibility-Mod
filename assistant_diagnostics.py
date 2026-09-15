"""Opt-in driving-assistant event and vehicle-state recording for MCP observation."""
import collections
import json
import re
import threading
import time
from datetime import datetime, timezone
from pathlib import Path


class AssistantDiagnosticRecorder:
    def __init__(self, directory):
        self.directory = Path(directory)
        self.lock = threading.RLock()
        self.stream = None
        self.path = None
        self.started = 0.0
        self.sequence = 0
        self.rows = collections.deque(maxlen=1500)
        self.latest = None
        self.latest_time = 0.0
        self.error = None
        self.road_sample_time = float("-inf")

    def record_road(self, packet, audio_state):
        """Keep cue timing alongside the drive, including manual driving."""
        with self.lock:
            now = time.monotonic()
            if not self.stream or now - self.road_sample_time < 0.2:
                return
            self.road_sample_time = now
            self.record({"kind": "road", "data": {
                "state": packet["state"], "proximity": packet.get("proximity"),
                "speedLimit": packet.get("speedLimit"),
                "signal": packet.get("signal"), "audio": audio_state}})

    def record(self, event):
        with self.lock:
            if not self.stream:
                return
            self.sequence += 1
            row = dict(event, seq=self.sequence, t=round(time.monotonic() - self.started, 3))
            row.pop("token", None)
            if event.get("kind") == "diagnostic":
                self.latest = row.get("data")
                self.latest_time = time.monotonic()
            self.rows.append(row)
            try:
                self.stream.write(json.dumps(row, ensure_ascii=False) + "\n")
                self.stream.flush()
            except OSError as exc:
                # Disk trouble must never delay control release or its audio cue.
                self.error = str(exc)
                try:
                    self.stream.close()
                except OSError:
                    pass
                self.stream = None

    def status(self):
        with self.lock:
            return {"recording": self.stream is not None,
                    "session": self.path.name if self.path else None,
                    "last_seq": self.sequence, "latest": self.latest,
                    "sample_age_s": round(time.monotonic() - self.latest_time, 3) if self.latest_time else None,
                    "error": self.error}

    def start(self, label=None):
        with self.lock:
            if self.stream:
                return self.status()
            self.directory.mkdir(parents=True, exist_ok=True)
            label = re.sub(r"[^A-Za-z0-9_-]+", "-", label or "driving-assistant")[:60]
            stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%S-%fZ")
            self.path = self.directory / f"{stamp}-{label}.ndjson"
            self.stream = self.path.open("x", encoding="utf-8")
            self.started = time.monotonic()
            self.rows.clear()
            self.latest, self.latest_time, self.error = None, 0.0, None
            self.road_sample_time = float("-inf")
            self.record({"kind": "start", "schema": 1})
            return self.status()

    def stop(self):
        with self.lock:
            if self.stream:
                self.record({"kind": "stop"})
                if self.stream:
                    self.stream.close()
                    self.stream = None
            return self.status()

    def read(self, since_seq=0, limit=100):
        with self.lock:
            rows = [r for r in self.rows if r["seq"] > int(since_seq or 0)]
            selected = rows[:max(1, min(int(limit), 500))]
            return {"records": selected, "next_seq": selected[-1]["seq"] if selected else int(since_seq or 0),
                    "truncated_before": self.rows[0]["seq"] if self.rows and int(since_seq or 0) < self.rows[0]["seq"] - 1 else None}

    def review(self, session=None):
        with self.lock:
            if session is None:
                path = self.path
                if path is None:
                    path = next(iter(sorted(self.directory.glob("*.ndjson"), reverse=True)), None)
            else:
                if Path(session).name != session or not session.endswith(".ndjson"):
                    raise ValueError("session must be a bare .ndjson file name")
                path = self.directory / session
            if path is None:
                return {"session": None, "samples": 0, "events": []}
            samples, road_samples, automatic, events = 0, 0, 0, []
            previous_choice = None
            with path.open(encoding="utf-8") as stream:
                for line in stream:
                    try:
                        row = json.loads(line)
                    except ValueError:
                        continue  # tolerate an interrupted final write
                    kind = row.get("kind")
                    if kind == "diagnostic":
                        samples += 1
                        vm = (row.get("data") or {}).get("vehicle") or {}
                        choice = (vm.get("locked"), vm.get("chosen"))
                        if choice != previous_choice and vm.get("automatic") and vm.get("locked"):
                            automatic += 1
                        previous_choice = choice
                    elif kind == "road":
                        road_samples += 1
                    elif kind not in {"alive", "start", "stop"}:
                        events.append(row)
            return {"session": path.name, "samples": samples, "road_samples": road_samples,
                    "automatic_choices_observed": automatic, "events": events[-200:],
                    "event_count": len(events)}

    def control(self, action="status", label=None, session=None, note=None, limit=100, since_seq=0):
        if action == "start":
            return self.start(label)
        if action == "stop":
            return self.stop()
        if action == "read":
            return self.read(since_seq, limit)
        if action == "review":
            return self.review(session)
        if action == "list":
            return {"sessions": [p.name for p in sorted(self.directory.glob("*.ndjson"), reverse=True)[:max(1, min(int(limit), 100))]]}
        if action == "mark":
            if not self.stream:
                raise ValueError("no assistant recording is active")
            self.record({"kind": "marker", "note": str(note or "")})
            return self.status()
        if action == "status":
            return self.status()
        raise ValueError("action must be start, stop, status, read, mark, review, or list")
