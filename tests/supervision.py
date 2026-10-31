"""Unix runner descendant and pipe supervision; run after `cargo build`."""
import json
import os
import pathlib
import signal
import socket
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request

if os.name != "posix":
    raise SystemExit("This test requires a Unix process group")

with tempfile.TemporaryDirectory(prefix="xenia-supervision-") as tmp:
    root = pathlib.Path(tmp)
    repo = root / "projects" / "example" / "repo"
    repo.mkdir(parents=True)
    subprocess.run(["git", "init", "-q", str(repo)], check=True)
    subprocess.run(["git", "-C", str(repo), "-c", "user.name=Test", "-c", "user.email=test@example.invalid", "commit", "--allow-empty", "-qm", "initial"], check=True)
    bindir = root / "bin"
    bindir.mkdir()
    runner = bindir / "codex"
    runner.write_text(f'''#!{sys.executable}\nimport json, pathlib, subprocess, sys\nprompt=sys.stdin.read()\nredirect = dict(stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL) if "silent" in prompt else {{}}\nchild=subprocess.Popen([sys.executable,"-c","import time; time.sleep(60)"], start_new_session="escape" in prompt, **redirect)\npathlib.Path("child.pid").write_text(str(child.pid))\nprint(json.dumps({{"type":"thread.started","thread_id":"synthetic-session"}}), flush=True)\n''')
    runner.chmod(0o755)
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        port = sock.getsockname()[1]
    token = "synthetic-supervision-token-123456"
    url = f"http://127.0.0.1:{port}/api/"
    env = os.environ | {"XENIA_TOKEN": token, "XENIA_LISTEN": f"127.0.0.1:{port}", "XENIA_DB": str(root / "controller.db"), "GIT_CONFIG_GLOBAL": os.devnull, "GIT_CONFIG_NOSYSTEM": "1"}
    processes = []
    escaped = None
    def request(path, body=None):
        data = None if body is None else json.dumps(body).encode()
        req = urllib.request.Request(url + path, data, {"Authorization": "Bearer " + token, "Content-Type": "application/json"})
        with urllib.request.urlopen(req, timeout=3) as response:
            return json.load(response)
    def until(test, seconds=20):
        end = time.monotonic() + seconds
        while time.monotonic() < end:
            try:
                value = test()
                if value:
                    return value
            except (OSError, StopIteration):
                pass
            time.sleep(.2)
        raise AssertionError("Timed out")
    def job(job_id):
        return next(j for j in request("state")["jobs"] if j["id"] == job_id)
    try:
        processes.append(subprocess.Popen(["target/debug/xenia", "controller"], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL))
        until(lambda: request("state"))
        worker_token = request("workers/enroll", {"id": "test"})["token"]
        env.update({"XENIA_WORKER_TOKEN": worker_token, "XENIA_WORKER_DB": str(root / "worker.db"), "XENIA_CONTROLLER": f"http://127.0.0.1:{port}", "XENIA_NODE_ID": "test", "XENIA_PROJECTS_ROOT": str(root / "projects"), "PATH": str(bindir) + os.pathsep + os.environ["PATH"]})
        processes.append(subprocess.Popen(["target/debug/xenia", "worker"], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL))
        until(lambda: request("state")["nodes"])
        project = request("projects", {"name": "example", "repository": "repo"})["id"]
        first = request("jobs", {"project_id": project, "runner": "codex", "prompt": "in group"})["id"]
        done = until(lambda: (j if (j := job(first))["state"] in ("succeeded", "failed", "unknown") else None))
        assert done["state"] == "succeeded", done
        pid = int((pathlib.Path(done["worktree"]) / "child.pid").read_text())
        def stopped():
            try:
                os.kill(pid, 0)
                # A Linux container's init may leave a terminated child as a
                # zombie temporarily; it is no longer executing the worktree.
                if sys.platform.startswith("linux"):
                    stat = pathlib.Path(f"/proc/{pid}/stat").read_text()
                    return stat.rsplit(") ", 1)[1].startswith("Z ")
                return False
            except (ProcessLookupError, FileNotFoundError):
                return True
        until(stopped, 5)
        print("runner descendants are stopped before completion: PASS")

        silent = request("jobs", {"project_id": project, "runner": "codex", "prompt": "silent"})["id"]
        silent_job = until(lambda: (j if (j := job(silent))["state"] in ("succeeded", "failed", "unknown") else None))
        assert silent_job["state"] == "succeeded", silent_job
        pid = int((pathlib.Path(silent_job["worktree"]) / "child.pid").read_text())
        until(stopped, 5)
        print("descendants are stopped even when they close runner pipes: PASS")

        second = request("jobs", {"project_id": project, "runner": "codex", "prompt": "escape"})["id"]
        started = until(lambda: (j if (j := job(second))["worktree"] and (pathlib.Path(j["worktree"]) / "child.pid").exists() else None))
        escaped = int((pathlib.Path(started["worktree"]) / "child.pid").read_text())
        uncertain = until(lambda: (j if (j := job(second))["state"] == "unknown" else None))
        assert "pipes remain open" in uncertain["error"], uncertain
        try:
            request("followup", {"id": second, "prompt": "continue"})
            raise AssertionError("uncertain worktree accepted a follow-up")
        except urllib.error.HTTPError as error:
            assert error.code == 400
        print("escaped pipe holder is marked uncertain and cannot be followed up: PASS")
    finally:
        if escaped:
            try:
                os.kill(escaped, signal.SIGKILL)
            except ProcessLookupError:
                pass
        for process in reversed(processes):
            process.terminate()
        for process in reversed(processes):
            process.wait(timeout=5)
