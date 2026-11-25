#!/usr/bin/env python3
"""Local read-only MCP fixture. Never invokes providers or accesses the network."""
import json
import os
from pathlib import Path
import secrets
import sys


def response(message):
    method = message.get("method")
    if method == "initialize":
        return {"protocolVersion": message.get("params", {}).get("protocolVersion", "2024-11-05"), "capabilities": {"tools": {}}, "serverInfo": {"name": "baize-shared-config-fixture", "version": "1"}}
    if method == "tools/list":
        return {"tools": [{"name": "read_probe", "description": "Read a new local configuration validation nonce. No network or workspace modifications.", "inputSchema": {"type": "object", "properties": {"agent": {"type": "string", "enum": ["codex-1", "antigravity-2", "codex-3"]}}, "required": ["agent"], "additionalProperties": False}, "annotations": {"readOnlyHint": True, "destructiveHint": False}}]}
    if method == "tools/call":
        params = message.get("params", {})
        arguments = params.get("arguments", {})
        if params.get("name") != "read_probe" or set(arguments) != {"agent"} or arguments["agent"] not in ("codex-1", "antigravity-2", "codex-3"):
            return {"isError": True, "content": [{"type": "text", "text": "Invalid fixture request"}]}
        value = {"agent": arguments["agent"], "nonce": "MCP-" + secrets.token_hex(12)}
        with Path(os.environ["BAIZE_CONFIG_PROBE_LOG"]).open("a") as log:
            log.write(json.dumps(value) + "\n")
        return {"isError": False, "content": [{"type": "text", "text": json.dumps(value)}]}
    if method == "resources/list":
        return {"resources": []}
    if method == "resources/templates/list":
        return {"resourceTemplates": []}
    if method == "ping":
        return {}
    raise ValueError("Unsupported fixture method")


def main():
    for line in sys.stdin:
        message = json.loads(line)
        if "id" not in message:
            continue
        try:
            value = {"jsonrpc": "2.0", "id": message["id"], "result": response(message)}
        except ValueError:
            value = {"jsonrpc": "2.0", "id": message["id"], "error": {"code": -32601, "message": "Unsupported fixture method"}}
        print(json.dumps(value), flush=True)


if __name__ == "__main__":
    main()
