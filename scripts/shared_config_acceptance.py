#!/usr/bin/env python3
"""Opt-in A→B→A shared instruction, skill and actual MCP-call acceptance.

Uses a dedicated worktree of an already trusted repository, without granting
trust, replacing native prompts or changing global provider configuration.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import select
import shutil
import socket
import subprocess
import sys
import time
import urllib.error
import urllib.request
import uuid


def native_inventory(workspace, server, output):
    """No turn/start: read native configuration and the skill discovery catalog."""
    with (output / "native-inspection-stderr.log").open("wb") as log:
        process = subprocess.Popen(["codex", "app-server", "--listen", "stdio://"], cwd=workspace, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=log, bufsize=0)
        def send(value):
            process.stdin.write(json.dumps(value).encode() + b"\n")
            process.stdin.flush()
        def receive(identifier):
            deadline = time.monotonic() + 20
            while time.monotonic() < deadline:
                if select.select([process.stdout], [], [], 1)[0]:
                    line = process.stdout.readline()
                    if not line:
                        raise RuntimeError("Native inspection disconnected")
                    value = json.loads(line)
                    if value.get("id") == identifier:
                        assert "error" not in value, "Native inspection request failed"
                        return value["result"]
            raise TimeoutError("Native inspection timed out")
        try:
            send({"id": "init", "method": "initialize", "params": {"clientInfo": {"name": "baize_config_acceptance", "version": "0.1.0"}, "capabilities": {"experimentalApi": False}}})
            receive("init")
            send({"method": "initialized"})
            send({"id": "config", "method": "config/read", "params": {"cwd": str(workspace), "includeLayers": True}})
            config = receive("config")
            send({"id": "skills", "method": "skills/list", "params": {"cwds": [str(workspace)], "forceReload": True}})
            skills = receive("skills")
            layers = [layer for layer in config.get("layers", []) if layer["name"].get("dotCodexFolder") == str(workspace / ".codex")]
            return {
                "project_layer_loaded": any(not layer.get("disabledReason") for layer in layers),
                "project_mcp_loaded": server in (config["config"].get("mcp_servers") or {}),
                "shared_skill_discovered": any(skill["name"] == "baize-shared-probe" for entry in skills.get("data", []) for skill in entry.get("skills", [])),
                "provider_prompts_submitted": 0,
            }
        finally:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=3)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--allow-real-providers", action="store_true")
    parser.add_argument("--daemon", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if not args.allow_real_providers:
        parser.error("Real provider validation requires --allow-real-providers")
    repository = Path(__file__).resolve().parent.parent
    daemon = args.daemon.resolve(strict=True)
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False, mode=0o700)
    workspace = output / "workspace"
    subprocess.run(["git", "worktree", "add", "--detach", str(workspace), "HEAD"], cwd=repository, check=True, capture_output=True)
    data = output / "data"
    data.mkdir(mode=0o700)
    marker = uuid.uuid4().hex[:12]
    rule = "RULE-" + marker
    skill = "SKILL-" + marker
    server = "baize_config_probe_" + marker
    (workspace / "AGENTS.md").write_text(f"Isolated Baize acceptance. Start every final answer with {rule}. Only use the requested project skill and read-only MCP fixture. Never edit files, run shell commands, install software, commit, change settings/permissions, or inspect files outside this worktree. Do not access credentials or network.\n")
    skill_root = workspace / ".agents/skills/baize-shared-probe"
    skill_root.mkdir(parents=True)
    (skill_root / "SKILL.md").write_text(f"---\nname: baize-shared-probe\ndescription: Use this skill only when explicitly asked to validate shared Baize configuration.\n---\nInclude the exact evidence marker {skill} in your final answer. Use the requested MCP read_probe tool once and report its nonce. Never simulate a tool call, use shell commands, or edit files.\n")
    server_script = workspace / ".agents/config-probe.py"
    shutil.copyfile(repository / "scripts/fixtures/shared_config_mcp.py", server_script)
    calls_path = output / "mcp-calls.jsonl"
    canonical = {"mcpServers": {server: {"command": sys.executable, "args": [str(server_script)], "env": {"BAIZE_CONFIG_PROBE_LOG": str(calls_path)}}}}
    (workspace / ".agents/mcp_config.json").write_text(json.dumps(canonical, indent=2) + "\n")
    (workspace / "SENTINEL.txt").write_text("keep unchanged\n")
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        port = sock.getsockname()[1]
    env = {**os.environ, "BAIZE_DATA_DIR": str(data), "BAIZE_ACCEPTANCE_PORT": str(port), "BAIZE_ACCEPTANCE_ALLOW_PROJECT": "1", "BAIZE_ACCEPTANCE_POLICY": "allow_project"}
    http = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    report = {"passed": False, "provider_prompts_submitted": 0, "checks": {}, "fixture": "isolated worktree of an already trusted repository", "execution_policy": "allow_project", "global_configuration_modified": False, "daemon_sha256": hashlib.sha256(daemon.read_bytes()).hexdigest()}
    process = None
    session = None
    config_files = [Path(os.environ.get("CODEX_HOME", str(Path.home() / ".codex"))) / "config.toml", Path.home() / ".gemini/antigravity-cli/settings.json", Path.home() / ".gemini/config/mcp_config.json"]
    def global_config_hashes():
        return [hashlib.sha256(path.read_bytes()).hexdigest() if path.exists() else None for path in config_files]
    original_global_hashes = global_config_hashes()
    def save(name, value):
        (output / name).write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n")
    def api(method, path, body=None):
        request = urllib.request.Request(f"http://127.0.0.1:{port}{path}", data=None if body is None else json.dumps(body).encode(), method=method, headers={"Content-Type": "application/json"})
        try:
            with http.open(request, timeout=180) as response:
                return response.status, json.load(response)
        except urllib.error.HTTPError as error:
            return error.code, json.load(error)
    try:
        report["provider_versions"] = {name: subprocess.check_output([name if name == "codex" else "agy", "--version"], text=True).strip() for name in ["codex", "antigravity"]}
        with (output / "daemon.log").open("wb") as log:
            process = subprocess.Popen([str(daemon)], cwd=workspace, env=env, stdout=log, stderr=subprocess.STDOUT)
        for _ in range(100):
            try:
                if api("GET", "/health")[0] == 200:
                    break
            except OSError:
                time.sleep(0.05)
        workspace_id = api("POST", "/workspaces", {"path": str(workspace)})[1]["workspace"]["id"]
        path = f"/workspaces/{workspace_id}/project-config"
        status, preview = api("GET", path)
        save("plan.json", preview)
        assert status == 200 and preview["project_config"]["status"] == "ready", preview
        status, applied = api("POST", path, {"action": "apply", "plan_id": preview["project_config"]["plan_id"]})
        save("applied.json", applied)
        assert status == 200 and applied["project_config"]["status"] == "in_sync", applied
        report["checks"]["reviewed_plan_applies_exact_mapping"] = True
        inventory = native_inventory(workspace, server, output)
        save("native-inventory.json", inventory)
        assert inventory["project_layer_loaded"] and inventory["project_mcp_loaded"], "Native Codex did not load the fixture project config; trust must be reviewed in native Codex, not granted by Baize"
        assert inventory["shared_skill_discovered"], "Shared skill not found in the native catalog"
        report["checks"]["native_codex_config_and_skill_discovery"] = True
        # Commit only this runner's fixtures so Baize Git hunks cannot supply the markers.
        subprocess.run(["git", "add", "--", "AGENTS.md", ".agents", ".codex", "SENTINEL.txt"], cwd=workspace, check=True)
        subprocess.run(["git", "-c", "user.name=Baize acceptance", "-c", "user.email=baize@example.invalid", "commit", "-qm", "shared configuration fixture"], cwd=workspace, check=True)
        baseline = subprocess.check_output(["git", "status", "--porcelain"], cwd=workspace)
        assert not baseline, "Fixture must start with a clean worktree"
        session = api("POST", "/sessions", {"workspace_id": workspace_id, "provider_id": "codex", "objective": "验证原生项目指令、单份 skill 及真实 MCP 工具在 Agent 间复用"})[1]["session"]["id"]
        native_ids = []
        for index, provider in enumerate(["codex", "antigravity", "codex"], 1):
            if index > 1:
                status, handoff = api("POST", f"/sessions/{session}/handoff", {"to_provider_id": provider})
                assert status == 200, handoff
                status, accepted = api("POST", f"/sessions/{session}/handoff/{handoff['handoff']['id']}/accept", {})
                assert status == 200 and accepted["session"]["id"] == session, accepted
            agent = f"{provider}-{index}"
            prompt = f"按项目指令的回复约定，使用项目 skill baize-shared-probe，再调用 MCP server {server} 的 read_probe 工具一次，参数 agent={agent}，报告本次返回的 nonce。只用原生 skill/MCP，不编辑文件、不运行终端命令、不改权限或设置，不使用过去回合的 nonce。中文回答；没有可用工具时明确说明，不模拟调用。"
            preview = api("POST", f"/sessions/{session}/context-preview", {"provider_id": provider, "prompt": prompt})[1]
            save(f"{index}-context-preview.json", preview)
            assert not preview["context"]["blocked"], preview
            report["provider_prompts_submitted"] += 1
            print(f"Starting shared-config turn {index}: {provider}", flush=True)
            status, reply = api("POST", f"/sessions/{session}/prompt", {"provider_id": provider, "prompt": prompt, "timeout_seconds": 120})
            save(f"{index}-response.json", reply)
            assert status == 200 and reply.get("turn_status") == "completed", reply
            native_ids.append(reply["native_session_id"])
            text = "\n".join(event.get("text") or "" for event in reply.get("events", []) if event["kind"] == "Output")
            calls = [json.loads(line) for line in calls_path.read_text().splitlines()] if calls_path.exists() else []
            actual = [call for call in calls if call["agent"] == agent]
            assert len(actual) == 1, f"Expected exactly one audited MCP call for {agent}"
            report["checks"][f"{agent}_shared_rule_observed"] = rule in text
            report["checks"][f"{agent}_shared_skill_observed"] = skill in text
            report["checks"][f"{agent}_actual_mcp_nonce_observed"] = actual[0]["nonce"] in text
            raw_tools = [event for event in reply.get("events", []) if event["kind"] == "ToolCall"]
            if provider == "codex":
                native_mcp = any((event.get("raw") or {}).get("item", {}).get("type") == "mcp_tool_call" and event["raw"]["item"].get("server") == server for event in raw_tools)
            else:
                native_mcp = any(server in json.dumps(event) and "read_probe" in json.dumps(event) for event in raw_tools)
            report["checks"][f"{agent}_native_mcp_event_recorded"] = native_mcp
            assert all(report["checks"].values()), report["checks"]
            print(f"Completed shared-config turn {index}: {provider}", flush=True)
        report["checks"]["returning_codex_resumes_original_native_id"] = native_ids[0] == native_ids[2]
        report["checks"]["workspace_and_canonical_sources_unchanged"] = subprocess.check_output(["git", "status", "--porcelain"], cwd=workspace) == baseline
        receipts = api("POST", path, {"action": "history"})[1]
        save("history.json", receipts)
        report["checks"]["backup_discoverable_after_roundtrip"] = any(item["backup_id"] == applied["change"]["backup_id"] for item in receipts["backups"])
        status, restored = api("POST", path, {"action": "restore", "backup_id": applied["change"]["backup_id"]})
        save("restored.json", restored)
        report["checks"]["checked_restore_removes_only_generated_codex_config"] = status == 200 and not (workspace / ".codex/config.toml").exists() and (workspace / ".agents/mcp_config.json").exists() and (workspace / "SENTINEL.txt").read_text() == "keep unchanged\n"
        report["checks"]["global_provider_configuration_unchanged"] = original_global_hashes == global_config_hashes()
        report["passed"] = all(report["checks"].values())
        assert report["passed"], report["checks"]
    except Exception as error:
        report["error"] = str(error)
        raise
    finally:
        if process and process.poll() is None:
            if session:
                try:
                    api("POST", f"/sessions/{session}/execution/cancel", {})
                except OSError:
                    pass
            process.terminate()
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=3)
        save("result.json", report)
        print(json.dumps(report, ensure_ascii=False), flush=True)


if __name__ == "__main__":
    main()
