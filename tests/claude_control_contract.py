"""Opt-in real Claude Code control-channel check; uses an authenticated CLI.

Run manually: python3 tests/claude_control_contract.py
This makes a model request, denies every tool request, and uses a temporary checkout.
"""
import json
import os
import pathlib
import queue
import subprocess
import tempfile
import threading
import time

with tempfile.TemporaryDirectory(prefix="xenia-claude-contract-") as tmp:
    repo = pathlib.Path(tmp)
    subprocess.run(["git", "init", "-q", str(repo)], check=True)
    command = ["claude", "--print", "--verbose", "--input-format", "stream-json",
               "--output-format", "stream-json", "--permission-mode", "manual",
               "--permission-prompts", "host", "--permission-prompt-tool", "stdio", "--setting-sources", ""]
    env = {k: v for k, v in os.environ.items() if k != "CLAUDECODE" and not k.startswith("XENIA_")}
    process = subprocess.Popen(command, cwd=repo, env=env, stdin=subprocess.PIPE,
                               stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
    frames = queue.Queue()
    def reader():
        for line in process.stdout:
            frames.put(line)
        frames.put(None)
    threading.Thread(target=reader, daemon=True).start()
    def send(value):
        process.stdin.write(json.dumps(value) + "\n")
        process.stdin.flush()
    def receive(deadline):
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise AssertionError("Claude control test timed out")
        try:
            line = frames.get(timeout=remaining)
        except queue.Empty as error:
            raise AssertionError("Claude control test timed out") from error
        if line is None:
            raise AssertionError("Claude exited before completing the control test")
        return json.loads(line)
    try:
        send({"type": "control_request", "request_id": "xenia-init", "request": {"subtype": "initialize", "hooks": None}})
        deadline = time.monotonic() + 30
        while True:
            frame = receive(deadline)
            if frame.get("type") == "control_response" and frame.get("response", {}).get("request_id") == "xenia-init":
                assert frame["response"].get("subtype") == "success", "Claude initialization failed"
                break
        print("Claude control initialization: PASS", flush=True)
        send({"type": "user", "message": {"role": "user", "content": "Use the Bash tool to run exactly `printf XENIA_TEST > marker.txt`. Do not use any other tool. If permission is denied, stop."}, "parent_tool_use_id": None, "session_id": ""})
        deadline = time.monotonic() + 120
        permission_seen = False
        expected_command_seen = False
        while True:
            frame = receive(deadline)
            if frame.get("type") == "control_request":
                request = frame.get("request", {})
                request_id = frame.get("request_id")
                if request.get("subtype") == "can_use_tool":
                    permission_seen = True
                    expected_command_seen |= (request.get("tool_name") == "Bash" and request.get("input", {}).get("command") == "printf XENIA_TEST > marker.txt")
                    response = {"subtype": "success", "request_id": request_id,
                                "response": {"behavior": "deny", "message": "Denied by Xenia contract test"}}
                else:
                    response = {"subtype": "error", "request_id": request_id, "error": "Unsupported request"}
                send({"type": "control_response", "response": response})
            if frame.get("type") == "result":
                break
        assert permission_seen, "Claude did not request permission; test is inconclusive"
        assert expected_command_seen, "Claude requested a different tool or command; test is inconclusive"
        assert not (repo / "marker.txt").exists(), "Denied command unexpectedly created a file"
        print("Claude permission callback and denial: PASS", flush=True)
    finally:
        if process.poll() is None:
            process.terminate()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=5)
