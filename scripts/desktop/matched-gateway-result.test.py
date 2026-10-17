import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("matched", Path(__file__).with_name("matched-gateway-result.py"))
matched = importlib.util.module_from_spec(spec)
spec.loader.exec_module(matched)


class MatchedResult(unittest.TestCase):
    def test_only_typed_facts_leave_the_fresh_result(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "gateway-window").mkdir()
            provenance = dict(repository="nessalabs/nessa-agent", state="ready", source="a" * 40, tree="b" * 40, workflowCommit="c" * 40)
            (root / "provenance.json").write_text(json.dumps(provenance))
            row = dict(name="apps", engine="chromium", layout="columns", ok=False, failures=["PRIVATE_MARKER"], error="PRIVATE_MARKER", seen=dict(state="live", title="PRIVATE_MARKER"))
            (root / "gateway-window/result.json").write_text(json.dumps(dict(check="gateway-window", results=[row])))
            request = dict(event="failed", engine="chromium", endpoint="/mcp-resources", method="GET", resource="fetch", frame="child", sameOrigin=False, loopback=True, duringClose=True, pageClosed=False, sizeReported=True, aborted=True, status=200, contentLength=42, bodySize=42, url="https://private/PRIVATE_MARKER")
            requests = [request] + [{**request, "event": "finished", "endpoint": "/desktop.html", "engine": engine} for engine in ["chromium", "webkit"]]
            (root / "matched-requests.json").write_text(json.dumps(dict(setup=True, requests=requests)))
            value = matched.result(root, "candidate", "a" * 40, "d" * 40)
            self.assertNotIn("PRIVATE_MARKER", json.dumps(value))
            self.assertEqual(value["rows"][0]["failureCount"], 1)
            self.assertTrue(value["rows"][0]["liveMount"])
            self.assertFalse(value["requests"][0]["sameOrigin"])
            self.assertEqual(value["requests"][0]["bodySize"], 42)
            self.assertEqual(value["observerCoverage"], dict(chromium=True, webkit=True))
            partial = [request, requests[1]]
            launch = dict(name="launch", engine="webkit", ok=False, failures=[], cannotRun=True)
            (root / "gateway-window/result.json").write_text(json.dumps(dict(check="gateway-window", results=[row, launch])))
            (root / "matched-requests.json").write_text(json.dumps(dict(setup=True, requests=partial)))
            incomplete = matched.result(root, "candidate", "a" * 40, "d" * 40)
            self.assertEqual(len(incomplete["rows"]), 2)
            self.assertEqual(incomplete["rows"][1]["name"], "launch")
            self.assertEqual(incomplete["observerCoverage"], dict(chromium=True, webkit=False))
            run = subprocess.run([sys.executable, str(Path(__file__).with_name("matched-gateway-result.py")), str(root)], env=dict(os.environ, MATCHED_CASE="candidate", COMPARISON_CANDIDATE="a" * 40, COMPARISON_BASE="d" * 40), capture_output=True, text=True)
            self.assertEqual(run.returncode, 2)
            self.assertIn('"name":"launch"', run.stdout)
            self.assertIn('"observerCoverage":{"chromium":true,"webkit":false}', run.stdout)
            self.assertNotIn("PRIVATE_MARKER", run.stdout)
            with self.assertRaisesRegex(ValueError, "source does not match"):
                matched.result(root, "base", "a" * 40, "d" * 40)
            request["endpoint"] = "PRIVATE_MARKER"
            (root / "matched-requests.json").write_text(json.dumps(dict(setup=True, requests=requests)))
            with self.assertRaisesRegex(ValueError, "Unexpected diagnostic enum"):
                matched.result(root, "candidate", "a" * 40, "d" * 40)


if __name__ == "__main__":
    unittest.main()
