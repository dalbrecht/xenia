"""Check native macOS worker settings reach its child without touching Keychain."""
import os
import pathlib
import subprocess
import tempfile
import uuid

with tempfile.TemporaryDirectory(prefix="xenia-macos-settings-") as tmp:
    root = pathlib.Path(tmp)
    app = root / "Probe.app" / "Contents"
    binary_dir = app / "MacOS"
    binary_dir.mkdir(parents=True)
    bundle_id = "org.xenia.settings.probe." + uuid.uuid4().hex
    (app / "Info.plist").write_text('''<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict><key>CFBundleIdentifier</key><string>{bundle_id}</string><key>CFBundleExecutable</key><string>probe</string><key>CFBundlePackageType</key><string>APPL</string></dict></plist>'''.replace("{bundle_id}", bundle_id))
    fake = binary_dir / "xenia-engine"
    fake.write_text('''#!/bin/sh
printf '%s %s\\n' "${XENIA_CODEX_APP_SERVER-unset}" "${XENIA_CLAUDE_CONTROL-unset}" > "$XENIA_WORKER_DATA_DIR/result"
printf '%s\\n' "$XENIA_PROJECTS_ROOT" > "$XENIA_WORKER_DATA_DIR/root"
''')
    fake.write_text(fake.read_text() + "printf '%s\\n' \"$XENIA_CONTROLLER\" > \"$XENIA_WORKER_DATA_DIR/controller\"\n")
    fake.write_text(fake.read_text() + "printf '%s,%s,%s,%s,%s,%s\\n' \"$XENIA_ALLOW_CODEX\" \"$XENIA_ALLOW_CLAUDE\" \"$XENIA_ALLOW_PROJECT_CHECKOUT\" \"$XENIA_ALLOW_SETUP_SCRIPTS\" \"$XENIA_ALLOW_CONTAINERS\" \"$XENIA_ALLOWED_PROJECTS\" > \"$XENIA_WORKER_DATA_DIR/policy\"\n")
    fake.chmod(0o755)
    ssh = root / "ssh"
    ssh.write_text('#!/bin/sh\ntouch "$XENIA_WORKER_DATA_DIR/ssh-started"\nexit 1\n')
    ssh.chmod(0o755)
    env = os.environ | {"CLANG_MODULE_CACHE_PATH": str(root / "clang-cache")}
    subprocess.run(["swiftc", "-module-cache-path", str(root / "swift-cache"),
                    "apps/macos-worker/Sources/XeniaWorker/Services/WorkerManager.swift",
                    "apps/macos-worker/Sources/XeniaWorker/Services/SecretStore.swift",
                    "tests/macos_worker_settings.swift", "-o", str(binary_dir / "probe")], env=env, check=True)
    for codex, claude, direct, policy in ((False,False,False,False),(True,False,False,False),(False,True,False,False),(True,True,False,False),(False,False,True,False),(False,False,False,True)):
        data = root / f"data-{int(codex)}-{int(claude)}-{int(direct)}-{int(policy)}"
        # Deliberately set both inherited flags to prove that unchecked settings clear them.
        probe_env = env | {"XENIA_WORKER_DATA_DIR": str(data), "XENIA_CODEX_APP_SERVER": "1",
                           "XENIA_CLAUDE_CONTROL": "1", "TEST_CODEX": str(int(codex)),
                           "TEST_CLAUDE": str(int(claude)), "TEST_HTTPS": str(int(direct)), "TEST_SSH": str(ssh), "TEST_POLICY": str(int(policy)), "XENIA_ALLOW_PROJECT_CHECKOUT":"1", "XENIA_ALLOW_SETUP_SCRIPTS":"1", "XENIA_ALLOW_CONTAINERS":"1"}
        subprocess.run([str(binary_dir / "probe")], env=probe_env, check=True, timeout=15)
        actual = (data / "result").read_text().strip()
        assert actual == f"{'1' if codex else 'unset'} {'1' if claude else 'unset'}", actual
        assert (data / "root").read_text().strip() == str(pathlib.Path.home() / "projects")
        assert (data / "controller").read_text().strip() == ("https://controller.example:8443" if direct else "http://127.0.0.1:8080")
        assert not (data / "ssh-started").exists(), "Direct HTTPS must not start SSH"
        assert (data / "policy").read_text().strip() == ("0,1,1,1,1,example" if policy else "1,1,0,0,0,")
    print("macOS worker applies both interactive settings and clears inherited flags; direct HTTPS takes precedence without SSH; node-local permissions override inherited flags: PASS")
