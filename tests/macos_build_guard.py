"""The macOS build guard must fail closed if process inspection fails."""
import os
import pathlib
import subprocess
import tempfile

with tempfile.TemporaryDirectory(prefix="xenia-build-guard-") as tmp:
    fake = pathlib.Path(tmp) / "pgrep"
    fake.write_text('#!/bin/sh\nexit "$TEST_PGREP_STATUS"\n')
    fake.chmod(0o755)
    for status, allowed in ((0, False), (1, True), (2, False), (3, False)):
        env = os.environ | {"PATH": tmp + os.pathsep + os.environ.get("PATH", ""), "TEST_PGREP_STATUS": str(status)}
        result = subprocess.run(["scripts/check-macos-worker-stopped.sh"], env=env, capture_output=True, text=True)
        assert (result.returncode == 0) == allowed, (status, result.stderr)
    for entrypoint in ("scripts/build-macos-app.sh", "script/build_and_run.sh"):
        for status in (0, 3):
            env = os.environ | {"PATH": tmp + os.pathsep + os.environ.get("PATH", ""), "TEST_PGREP_STATUS": str(status)}
            result = subprocess.run([entrypoint], env=env, capture_output=True, text=True, timeout=5)
            assert result.returncode != 0, (entrypoint, status)
            assert "Xenia Worker" in result.stderr, (entrypoint, result.stderr)
    print("macOS build guard fails closed when the worker is running or process inspection fails: PASS")
