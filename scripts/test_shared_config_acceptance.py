"""Fixture protocol tests; no native agent or model is called."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


class McpFixtureTests(unittest.TestCase):
    def test_runner_rejects_real_execution_without_explicit_gate(self):
        with tempfile.TemporaryDirectory() as directory:
            output=Path(directory)/"must-not-be-created"
            result=subprocess.run([sys.executable,str(Path(__file__).parent/"shared_config_acceptance.py"),"--daemon","does-not-exist","--output",str(output)],text=True,capture_output=True,timeout=5)
            self.assertEqual(result.returncode,2)
            self.assertIn("requires --allow-real-providers",result.stderr)
            self.assertFalse(output.exists())

    def test_handshake_tool_discovery_and_audited_nonce(self):
        with tempfile.TemporaryDirectory() as directory:
            log = Path(directory) / "calls.jsonl"
            messages = [
                {"id": 1, "method": "initialize", "params": {"protocolVersion": "2024-11-05"}},
                {"method": "notifications/initialized"},
                {"id": 2, "method": "tools/list"},
                {"id": 3, "method": "tools/call", "params": {"name": "read_probe", "arguments": {"agent": "codex-1"}}},
                {"id": 4, "method": "tools/call", "params": {"name": "read_probe", "arguments": {"agent": "unknown"}}},
                {"id": 5, "method": "unknown"},
            ]
            result = subprocess.run([sys.executable, str(Path(__file__).parent / "fixtures/shared_config_mcp.py")], input="\n".join(map(json.dumps, messages)) + "\n", text=True, capture_output=True, env={**os.environ, "BAIZE_CONFIG_PROBE_LOG": str(log)}, check=True, timeout=5)
            values = [json.loads(line) for line in result.stdout.splitlines()]
            self.assertEqual(len(values), 5)
            self.assertEqual(values[0]["result"]["protocolVersion"], "2024-11-05")
            self.assertTrue(values[1]["result"]["tools"][0]["annotations"]["readOnlyHint"])
            nonce = json.loads(values[2]["result"]["content"][0]["text"])
            self.assertEqual(json.loads(log.read_text()), nonce)
            self.assertTrue(values[3]["result"]["isError"])
            self.assertEqual(values[4]["error"]["code"], -32601)


if __name__ == "__main__":
    unittest.main()
