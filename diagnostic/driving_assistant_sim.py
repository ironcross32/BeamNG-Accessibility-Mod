"""Session protocol checks without importing the GUI or audio application."""
import json
import sys
import unittest
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from driving_assistant import DrivingAssistant
from assistant_diagnostics import AssistantDiagnosticRecorder


class AssistantSessionChecks(unittest.TestCase):
    def setUp(self):
        self.sent, self.spoken = [], []
        self.client = DrivingAssistant(self.sent.append, lambda text, **kw: self.spoken.append(text))

    def response(self, kind, sequence, token=None):
        self.client.receive(json.dumps({"token": token or self.client.token,
                                       "kind": kind, "sequence": sequence,
                                       "vehicle": 42, "message": kind}))

    def test_activation_requires_confirmation(self):
        self.client.toggle()
        self.assertFalse(self.client.active)
        self.assertEqual(self.spoken, [])
        self.response("on", 1)
        self.assertTrue(self.client.active)
        self.assertEqual(self.client.vehicle, 42)

    def test_old_session_cannot_reactivate_or_speak(self):
        self.client.toggle()
        old = self.client.token
        self.response("off", 3)
        self.client.toggle()
        count = len(self.spoken)
        self.response("on", 100, old)
        self.response("say", 101, old)
        self.assertFalse(self.client.active)
        self.assertEqual(len(self.spoken), count)

    def test_duplicate_and_reordered_packets(self):
        self.client.toggle()
        self.response("on", 2)
        self.response("on", 2)
        self.response("say", 1)
        self.assertEqual(self.spoken, ["on"])
        token = self.client.token
        self.response("off", 4)
        self.response("on", 3, token)
        self.assertFalse(self.client.active)

    def test_toggle_off_uses_current_token(self):
        self.client.toggle()
        token = self.client.token
        self.response("on", 1)
        self.client.toggle()
        self.assertEqual(self.sent[-1], f"ASSISTANT_OFF:{token}")

    def test_malformed_response(self):
        self.client.toggle()
        for payload in ["bad", "null", "[]", '{"token":false}', '{"sequence":null}']:
            self.client.receive(payload)
        self.assertFalse(self.client.active)
        self.assertEqual(self.spoken, [])

    def test_heartbeat_recovers_lost_confirmation(self):
        self.client.toggle()
        self.client.receive(json.dumps({"token": self.client.token, "kind": "alive",
                                        "sequence": 2, "active": True, "vehicle": 42}))
        self.assertTrue(self.client.active)
        self.assertEqual(len(self.spoken), 1)

    def test_disabled_status_does_not_start_session(self):
        self.client.status()
        self.response("status", 0, self.client.status_token)
        self.assertEqual(self.spoken, ["status"])
        self.assertFalse(self.client.active)
        self.assertIsNone(self.client.token)

    def test_cues_are_ordered_and_disengagement_ignores_duplicate(self):
        cues = []
        self.client.cue = lambda kind, choices=(): cues.append((kind, choices))
        self.client.toggle()
        token = self.client.token
        self.response("on", 1)
        self.client.receive(json.dumps({"token": token, "kind": "junction",
                                       "sequence": 2, "message": "right,left"}))
        self.assertEqual(cues, [("junction", ("left", "right"))])
        self.assertEqual(self.spoken, ["on"])
        self.response("off", 3, token)
        self.response("off", 3, token)
        self.assertEqual(cues[-1], ("off", ()))
        self.assertEqual(len(cues), 2)

    def test_timeout_plays_off_once(self):
        cues = []
        self.client.cue = lambda kind, choices=(): cues.append(kind)
        self.client.toggle()
        self.response("on", 1)
        self.client.last_response -= 6
        self.client.heartbeat()
        self.client.heartbeat()
        self.assertEqual(cues, ["off"])
        self.assertFalse(self.client.active)

    def test_diagnostics_persist_and_stay_silent(self):
        with tempfile.TemporaryDirectory() as directory:
            recorder = AssistantDiagnosticRecorder(directory)
            self.client.diagnostics = recorder
            recorder.start("test")
            self.client.toggle()
            self.response("on", 1)
            self.client.receive(json.dumps({"token": self.client.token, "kind": "diagnostic",
                "sequence": 2, "data": {"vehicle": {"locked": "j", "chosen": "left", "automatic": True}}}))
            self.assertEqual(self.spoken, ["on"])
            self.assertEqual(recorder.status()["latest"]["vehicle"]["chosen"], "left")
            first = recorder.read(limit=1)
            second = recorder.read(since_seq=first["next_seq"])
            self.assertGreater(second["records"][0]["seq"], first["next_seq"])
            recorder.stop()
            review = recorder.review()
            self.assertEqual(review["samples"], 1)
            self.assertEqual(review["automatic_choices_observed"], 1)
            with self.assertRaises(ValueError):
                recorder.review("../escape.ndjson")


if __name__ == "__main__":
    unittest.main()
