"""Verify the runner launch gate on the host platform after `cargo build`."""
import hashlib
import os
import pathlib
import subprocess
import sys
import tempfile
import time

with tempfile.TemporaryDirectory(prefix="xenia-runner-host-") as tmp:
    root = pathlib.Path(tmp)
    marker = root / "started"
    runner = root / "runner.py"
    runner.write_text('''import hashlib, pathlib, sys\npathlib.Path(sys.argv[1]).write_text("started")\ndata = sys.stdin.buffer.read()\nprint(hashlib.sha256(data).hexdigest(), flush=True)\nsys.exit(7)\n''', encoding="utf-8")
    binary = pathlib.Path("target/debug/xenia.exe" if os.name == "nt" else "target/debug/xenia").resolve()
    command = [str(binary), "__runner-host", sys.executable, str(runner), str(marker)]
    child = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    time.sleep(.3)
    assert not marker.exists(), "runner started before launch gate"
    prompt = ("a" * 80000 + '\n" & % ! 🦀').encode()
    stdout, stderr = child.communicate(b"\x01" + prompt, timeout=15)
    assert child.returncode == 7, (child.returncode, stderr)
    assert marker.exists() and stdout.decode().strip() == hashlib.sha256(prompt).hexdigest()
    marker.unlink()
    child = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    child.communicate(b"", timeout=10)
    assert child.returncode != 0 and not marker.exists(), "runner started after gate closed"
    child = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    child.communicate(b"\x00ignored", timeout=10)
    assert child.returncode != 0 and not marker.exists(), "runner started after invalid gate"
    print("Runner waits for gate, preserves prompt bytes and exit code, and refuses a closed or invalid gate: PASS")
