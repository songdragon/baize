#!/usr/bin/env python3
"""Opt-in real CLI A → B → A acceptance; never part of cargo test or CI.

Build: cargo build --release -p baize-daemon --example continuity_daemon
Run: python3 scripts/continuity_acceptance.py --allow-real-providers \
       --daemon target/release/examples/continuity_daemon --output target/audit/continuity/run-1
"""
import argparse
import json
import os
from pathlib import Path
import socket
import subprocess
import time
import urllib.error
import urllib.request
import uuid


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--allow-real-providers", action="store_true", help="Authorize three real provider prompts (uses model quota)")
    parser.add_argument("--daemon", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if not args.allow_real_providers:
        parser.error("Real provider execution requires --allow-real-providers")
    daemon = args.daemon.resolve(strict=True)
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False, mode=0o700)
    workspace = output / "workspace"
    workspace.mkdir(mode=0o700)
    data = output / "data"
    data.mkdir(mode=0o700)
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        port = sock.getsockname()[1]
    env = os.environ.copy()
    env.update(BAIZE_DATA_DIR=str(data), BAIZE_ACCEPTANCE_PORT=str(port), BAIZE_ACCEPTANCE_ALLOW_PROJECT="1")
    process = None
    nonce = uuid.uuid4().hex[:10]
    prefix = f"handoff-{nonce}-"
    password = f"接续-{uuid.uuid4().hex[:12]}"
    report = {"passed": False, "provider_prompts_submitted": 0, "port": port, "workspace": str(workspace), "checks": {}, "execution_policy":"allow_project"}
    # Loopback traffic must not pass through the user's outbound model proxy.
    local_http = urllib.request.build_opener(urllib.request.ProxyHandler({}))

    def save(name, value):
        (output / name).write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n")

    def api(method, path, body=None):
        request = urllib.request.Request(f"http://127.0.0.1:{port}{path}", data=None if body is None else json.dumps(body).encode(), method=method, headers={"Content-Type": "application/json"})
        try:
            with local_http.open(request, timeout=305) as response:
                return json.load(response)
        except urllib.error.HTTPError as error:
            result = json.load(error)
            if result.get("status") in ("failed", "canceled"):
                return result
            raise RuntimeError(f"HTTP {error.code}: {result.get('error', 'request rejected')}") from error

    def start():
        nonlocal process
        log = open(output / "daemon.log", "ab")
        process = subprocess.Popen([str(daemon)], env=env, cwd=workspace, stdout=log, stderr=subprocess.STDOUT)
        log.close()
        for _ in range(100):
            if process.poll() is not None:
                raise RuntimeError("Isolated daemon exited; inspect daemon.log")
            try:
                api("GET", "/health")
                return
            except (OSError, urllib.error.URLError):
                time.sleep(0.05)
        raise RuntimeError("Isolated daemon did not start")

    def stop():
        nonlocal process
        if process and process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)
        process = None

    def local(text):
        result = api("POST", f"/sessions/{session}/prompt", {"prompt": text})
        assert result.get("local_command"), result

    def turn(index, provider, text):
        preview = api("POST", f"/sessions/{session}/context-preview", {"provider_id": provider, "prompt": text})
        save(f"{index}-preview.json", preview)
        assert not preview["context"]["blocked"], preview["context"]["warnings"]
        report["provider_prompts_submitted"] += 1
        print(f"Starting turn {index}: {provider}", flush=True)
        response = api("POST", f"/sessions/{session}/prompt", {"provider_id": provider, "prompt": text, "timeout_seconds": 240})
        save(f"{index}-response.json", response)
        save(f"{index}-events.json", api("GET", f"/sessions/{session}/events?limit=100000"))
        assert response.get("turn_status") == "completed", f"{provider} did not complete; inspect {index}-response.json"
        denied = [event for event in response.get("events",[]) if ((event.get("raw") or {}).get("result") or {}).get("denied_actions")]
        assert not denied, f"{provider} had denied actions; terminal SUCCESS alone is insufficient"
        assert response.get("native_session_id"), f"{provider} did not expose a native session reference"
        print(f"Completed turn {index}: {provider}", flush=True)
        return response

    def run_tests(index):
        result = subprocess.run(["python3", "-m", "unittest", "discover", "-v"], cwd=workspace, capture_output=True, text=True, timeout=30)
        (output / f"{index}-tests.txt").write_text(result.stdout + result.stderr)
        assert result.returncode == 0, f"Fixture tests failed after {index}"

    try:
        (workspace / "AGENTS.md").write_text("This is an isolated Baize continuity acceptance workspace. Work only here. Use Python standard library and unittest. Do not install dependencies, access the network, commit, or modify SENTINEL.txt. Preserve the public API and obey current saved user requirements. The native CLI environment is under test; do not claim Desktop integration.\n")
        (workspace / "SENTINEL.txt").write_text("must remain unchanged\n")
        subprocess.run(["git", "init", "-q"], cwd=workspace, check=True)
        subprocess.run(["git", "add", "."], cwd=workspace, check=True)
        subprocess.run(["git", "-c", "user.name=Baize acceptance", "-c", "user.email=baize@example.invalid", "commit", "-qm", "acceptance fixture"], cwd=workspace, check=True)
        start()
        ws = api("POST", "/workspaces", {"path": str(workspace)})["workspace"]["id"]
        session = api("POST", "/sessions", {"workspace_id": ws, "objective": "在同一会话中实现并完善标签规范化函数", "provider_id": "codex"})["session"]["id"]
        report["session_id"] = session
        local("/remember api 实现 labeltools.py 的 normalize_label(value: str) -> str；只用标准库，保护 SENTINEL.txt")
        local("/remember prefix normalize_label 的返回值必须以 seed- 为前缀，后接去除首尾空白并转小写的文本，内部空格替换为连字符")
        first = turn(1, "codex", "实现当前保存要求中的函数及 unittest 测试，运行测试。请用中文简要说明改动和结果。")
        run_tests(1)
        local(f"/remember prefix 更新返回值前缀为 {prefix}，替代此前 seed- 要求；后接规范化文本")
        local("/remember response 所有最终说明使用中文；非字符串输入必须抛出 TypeError，错误信息为：标签必须是字符串")
        second = turn(2, "antigravity", f"接着完善已有实现：连续空白合并为一个连字符，按照当前有效要求补齐实现和测试。先检查文件，避免覆盖已完成工作。最终回复记录接续口令「{password}」，只放在对话，不写入任何文件。")
        run_tests(2)
        b_reply = "\n".join(event.get("text") or "" for event in second["events"] if event["kind"] == "Output")
        assert password in b_reply, "B did not produce the requested dialogue-only handoff reply"
        report["checks"]["b_records_dialogue_only_handoff_password"] = True
        # Validate B's own implementation before returning to A; old tests passing is insufficient.
        check_b = subprocess.run(["python3", "-c", "import labeltools; assert labeltools.normalize_label('  HeLLo   World  ') == " + repr(prefix + "hello-world") + ";\ntry: labeltools.normalize_label(None)\nexcept TypeError as e: assert str(e) == '标签必须是字符串'\nelse: raise AssertionError('TypeError not raised')"],cwd=workspace,capture_output=True,text=True,timeout=20)
        (output / "2-independent-validation.txt").write_text(check_b.stdout+check_b.stderr)
        assert check_b.returncode == 0, "B did not implement the updated constraints; inspect stage 2"
        report["checks"]["b_independently_satisfies_updated_constraints"] = True
        before_restart = api("GET", f"/sessions/{session}")
        save("before-restart.json", before_restart)
        # Introduce a real workspace change while Codex is away.
        cases = [{"input": "  HeLLo   World  ", "expected": prefix + "hello-world"}, {"input": "A\tB", "expected": prefix + "a-b"}]
        (workspace / "acceptance_cases.json").write_text(json.dumps(cases, indent=2))
        assert not any(password in path.read_text(errors="replace") for path in workspace.rglob("*") if path.is_file() and ".git" not in path.parts), "Handoff password leaked to files"
        stop()
        start()
        restarted = api("GET", f"/sessions/{session}")
        report["checks"]["native_references_survive_restart"] = before_restart["session"]["provider_native_session_ids"] == restarted["session"]["provider_native_session_ids"]
        third = turn(3, "codex", "继续完成边界测试：读取工作区新增 acceptance_cases.json 并将其案例加入 unittest；保留上一位执行者的修改和当前有效要求。最终中文回复包含上一位记录的接续口令、当前有效前缀和测试结果；缺失时明确说明，不要猜。")
        run_tests(3)
        validation = subprocess.run(["python3", "-c", "import labeltools; assert labeltools.normalize_label('  HeLLo   World  ') == " + repr(prefix + "hello-world") + ";\ntry: labeltools.normalize_label(None)\nexcept TypeError as e: assert str(e) == '标签必须是字符串'\nelse: raise AssertionError('TypeError not raised')"], cwd=workspace, capture_output=True, text=True, timeout=20)
        (output / "acceptance-validation.txt").write_text(validation.stdout + validation.stderr)
        final = api("GET", f"/sessions/{session}")
        events = api("GET", f"/sessions/{session}/events?limit=100000")
        save("final-session.json", final)
        save("final-events.json", events)
        starts = [event for event in events["events"] if event["event_type"] == "session.agent.started"]
        output_events = [event for event in events["events"] if event["event_type"] == "session.agent.output" and event.get("provider_id") == "codex" and event["timestamp"] > starts[-1]["timestamp"]]
        response_text = "\n".join(event["payload"].get("text") or "" for event in output_events)
        report["checks"].update({
            "same_baize_session": all(event["session_id"] == session for event in starts),
            "exact_a_b_a_sequence": [event["provider_id"] for event in starts] == ["codex", "antigravity", "codex"],
            "resumed_original_codex_reference": first["native_session_id"] == third["native_session_id"],
            "b_requirement_reaches_returning_a": prefix in response_text and validation.returncode == 0,
            "dialogue_only_password_reaches_returning_a": password in response_text,
            "external_workspace_change_visible": "acceptance_cases.json" in json.loads((output / "3-preview.json").read_text())["prompt_preview"],
            "sentinel_preserved": (workspace / "SENTINEL.txt").read_text() == "must remain unchanged\n",
            "password_never_written_to_workspace": not any(password in path.read_text(errors="replace") for path in workspace.rglob("*") if path.is_file() and ".git" not in path.parts),
            "all_three_turns_have_native_ids": all(turn["native_session_id"] for turn in (first, second, third)),
        })
        report["passed"] = all(report["checks"].values())
        assert report["passed"], report["checks"]
    except Exception as error:
        report["error"] = str(error)
        raise
    finally:
        stop()
        save("result.json", report)
        print(json.dumps(report, ensure_ascii=False), flush=True)


if __name__ == "__main__":
    main()
