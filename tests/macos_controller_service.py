"""Temporary per-user launchd service test. Requires macOS and a release build."""
import json
import os
import pathlib
import plistlib
import re
import signal
import socket
import subprocess
import sys
import tempfile
import time
import urllib.request
import uuid

if sys.platform != "darwin":
    raise SystemExit("This test requires macOS")
script = pathlib.Path("scripts/macos-controller.py").resolve()
with tempfile.TemporaryDirectory(prefix="xenia-launchd-") as tmp:
    root = pathlib.Path(tmp)
    data = root / "private data"
    agents = root / "launch agents"
    label = "org.xenia.controller.test_" + uuid.uuid4().hex
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        port = sock.getsockname()[1]
    args = [sys.executable, str(script), "--data-dir", str(data), "--agents-dir", str(agents), "--label", label, "--listen", f"127.0.0.1:{port}"]
    domain = f"gui/{os.getuid()}/{label}"
    def command(action, check=True):
        return subprocess.run(args + [action], capture_output=True, text=True, check=check)
    def until(test, seconds=20):
        end = time.monotonic() + seconds
        while time.monotonic() < end:
            try:
                value = test()
                if value:
                    return value
            except (OSError, ValueError, KeyError):
                pass
            time.sleep(.2)
        raise AssertionError("Timed out waiting for controller service")
    def pid():
        result = subprocess.run(["/bin/launchctl", "print", domain], capture_output=True, text=True)
        match = re.search(r"^\s*pid = (\d+)$", result.stdout, re.MULTILINE)
        return int(match.group(1)) if match else None
    def state():
        token = (data / "controller-token").read_text().strip()
        request = urllib.request.Request(f"http://127.0.0.1:{port}/api/state", headers={"Authorization": "Bearer " + token})
        with urllib.request.urlopen(request, timeout=2) as response:
            return json.load(response)
    try:
        agents.mkdir()
        conflict = agents / (label + ".plist")
        conflict.write_bytes(plistlib.dumps({"Label": label, "ProgramArguments": ["/usr/bin/true"]}))
        original = conflict.read_bytes()
        refused = command("install", check=False)
        assert refused.returncode != 0 and conflict.read_bytes() == original
        conflict.unlink()
        command("install")
        assert (agents / (label + ".plist")).exists()
        command("start")
        until(state)
        old_pid = until(pid)
        refused = command("install", check=False)
        assert refused.returncode != 0, "upgrade unexpectedly replaced a running service"
        os.kill(old_pid, signal.SIGKILL)
        until(lambda: (new if (new := pid()) and new != old_pid else None), 30)
        until(state)
        print("LaunchAgent refuses unrelated configuration, starts, recovers after a crash, and refuses live replacement: PASS")
        command("stop")
        assert pid() is None
        command("uninstall")
        assert (data / "controller.db").exists() and (data / "controller-token").exists()
        assert not (agents / (label + ".plist")).exists()
        command("install")
        command("uninstall")
        print("Stop, uninstall and reinstall retain controller data: PASS")
    finally:
        # Always unload this unique disposable service, even after a failure.
        subprocess.run(["/bin/launchctl", "bootout", domain], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
