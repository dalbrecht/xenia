"""Windows-only native .cmd runner smoke test. Run after `cargo build`."""
import hashlib
import json
import os
import pathlib
import socket
import subprocess
import sys
import tempfile
import time
import urllib.request

if os.name != "nt":
    raise SystemExit("This test requires native Windows")

with tempfile.TemporaryDirectory(prefix="xenia-windows-") as tmp:
    root = pathlib.Path(tmp)
    repo = root / "projects" / "example" / "repo"
    repo.mkdir(parents=True)
    subprocess.run(["git", "init", "-q", str(repo)], check=True)
    subprocess.run(["git", "-C", str(repo), "-c", "user.name=Test", "-c", "user.email=test@example.invalid", "commit", "--allow-empty", "-qm", "initial"], check=True)
    bin_dir = root / "bin"
    bin_dir.mkdir()
    fake = bin_dir / "fake.py"
    fake.write_text('''import hashlib, json, pathlib, subprocess, sys\nprompt = sys.stdin.buffer.read()\nassert not any("prompt-marker" in x for x in sys.argv)\nif prompt == b"spawn-child":\n    child = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(20)"])\n    pathlib.Path("child.pid").write_text(str(child.pid))\nprint(json.dumps({"type":"thread.started","thread_id":"synthetic-session"}), flush=True)\nprint(json.dumps({"type":"item.completed","item":{"type":"agent_message","text":hashlib.sha256(prompt).hexdigest()}}), flush=True)\n''', encoding="utf-8")
    (bin_dir / "codex.cmd").write_text(f'@"{sys.executable}" "{fake}" %*\r\n', encoding="utf-8")
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        port = sock.getsockname()[1]
    url = f"http://127.0.0.1:{port}/api/"
    token = "synthetic-windows-test-token-123456"
    env = os.environ | {"XENIA_TOKEN": token, "XENIA_LISTEN": f"127.0.0.1:{port}", "XENIA_DB": str(root / "controller.db"), "GIT_CONFIG_GLOBAL": os.devnull, "GIT_CONFIG_NOSYSTEM": "1"}
    binary = str(pathlib.Path("target/debug/xenia.exe").resolve())
    processes = []
    def request(path, body=None):
        data = None if body is None else json.dumps(body).encode()
        req = urllib.request.Request(url + path, data, {"Authorization": "Bearer " + token, "Content-Type": "application/json"})
        with urllib.request.urlopen(req, timeout=4) as response:
            return json.load(response)
    def until(test, seconds=40):
        end = time.monotonic() + seconds
        while time.monotonic() < end:
            try:
                result = test()
                if result:
                    return result
            except (OSError, StopIteration):
                pass
            time.sleep(.2)
        raise AssertionError("Timed out waiting for native Windows worker")
    try:
        processes.append(subprocess.Popen([binary, "controller"], env=env))
        until(lambda: request("state"), 15)
        worker_token = request("workers/enroll", {"id": "test"})["token"]
        env.update({"XENIA_WORKER_TOKEN": worker_token, "XENIA_WORKER_DB": str(root / "worker.db"), "XENIA_CONTROLLER": f"http://127.0.0.1:{port}", "XENIA_NODE_ID": "test", "XENIA_PROJECTS_ROOT": str(root / "projects"), "PATH": str(bin_dir) + os.pathsep + os.environ["PATH"]})
        processes.append(subprocess.Popen([binary, "worker"], env=env))
        until(lambda: request("state")["nodes"], 15)
        project = request("projects", {"name": "example", "repository": "repo"})["id"]
        prompt = "prompt-marker " + "x" * 80000 + '\n" & % ! $(literal) 🦀'
        job = request("jobs", {"project_id": project, "runner": "codex", "prompt": prompt})["id"]
        def state(job_id):
            return next(j for j in request("state")["jobs"] if j["id"] == job_id)
        first = until(lambda: (j if (j := state(job))["state"] in ("succeeded", "failed") else None))
        assert first["state"] == "succeeded", first.get("error")
        assert hashlib.sha256(prompt.encode()).hexdigest() in request("outputs", {"ids": [job]})[job]
        follow_prompt = 'prompt-marker follow-up & % !'
        follow = request("followup", {"id": job, "prompt": follow_prompt})["id"]
        second = until(lambda: (j if (j := state(follow))["state"] in ("succeeded", "failed") else None))
        assert second["state"] == "succeeded" and second["worktree"] == first["worktree"], second.get("error")
        assert hashlib.sha256(follow_prompt.encode()).hexdigest() in request("outputs", {"ids": [follow]})[follow]
        supervised = request("jobs", {"project_id": project, "runner": "codex", "prompt": "spawn-child"})["id"]
        third = until(lambda: (j if (j := state(supervised))["state"] in ("succeeded", "failed", "unknown") else None))
        assert third["state"] == "succeeded", third.get("error")
        pid = int((pathlib.Path(third["worktree"]) / "child.pid").read_text())
        import ctypes
        kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        kernel.OpenProcess.argtypes = [ctypes.c_uint32, ctypes.c_int, ctypes.c_uint32]
        kernel.OpenProcess.restype = ctypes.c_void_p
        kernel.WaitForSingleObject.argtypes = [ctypes.c_void_p, ctypes.c_uint32]
        kernel.CloseHandle.argtypes = [ctypes.c_void_p]
        handle = kernel.OpenProcess(0x00100000, False, pid)  # SYNCHRONIZE
        if handle:
            try:
                assert kernel.WaitForSingleObject(handle, 5000) == 0, "runner child still active"
            finally:
                kernel.CloseHandle(handle)
        else:
            assert ctypes.get_last_error() == 87, "could not inspect child process"  # ERROR_INVALID_PARAMETER: PID gone
        print("Native Windows .cmd launcher, stdin, follow-up, and child cleanup: PASS")
    finally:
        for process in reversed(processes):
            process.terminate()
        for process in reversed(processes):
            process.wait(timeout=10)
