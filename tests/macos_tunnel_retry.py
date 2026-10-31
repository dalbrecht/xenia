"""Exercise macOS tunnel supervision with fake processes, without touching Keychain."""
import os, pathlib, subprocess, tempfile, uuid
with tempfile.TemporaryDirectory(prefix="xenia-macos-tunnel-") as tmp:
    root=pathlib.Path(tmp); app=root/"Probe.app"/"Contents"; bindir=app/"MacOS"; bindir.mkdir(parents=True)
    bundle="org.xenia.tunnel.probe."+uuid.uuid4().hex
    (app/"Info.plist").write_text(f'''<?xml version="1.0" encoding="UTF-8"?><!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd"><plist version="1.0"><dict><key>CFBundleIdentifier</key><string>{bundle}</string><key>CFBundleExecutable</key><string>probe</string><key>CFBundlePackageType</key><string>APPL</string></dict></plist>''')
    engine=bindir/"xenia-engine";engine.write_text('''#!/bin/sh
while [ ! -f "$XENIA_STOP_FILE" ]; do sleep .05; done
''');engine.chmod(0o755)
    ssh=root/"fake-ssh";ssh.write_text('''#!/bin/sh
printf 'started\\n' >> "$XENIA_WORKER_DATA_DIR/ssh-starts"
printf '%s\\n' "$@" > "$XENIA_WORKER_DATA_DIR/ssh-args"
[ "${TEST_ALWAYS_FAIL-}" != 1 ] || exit 1
[ "$(wc -l < "$XENIA_WORKER_DATA_DIR/ssh-starts")" -gt 1 ] || exit 1
exec sleep 30
''');ssh.chmod(0o755)
    data=root/"data";env=os.environ|{"CLANG_MODULE_CACHE_PATH":str(root/"clang-cache"),"XENIA_WORKER_DATA_DIR":str(data),"TEST_SSH":str(ssh)}
    subprocess.run(["swiftc","-module-cache-path",str(root/"swift-cache"),"apps/macos-worker/Sources/XeniaWorker/Services/WorkerManager.swift","apps/macos-worker/Sources/XeniaWorker/Services/SecretStore.swift","tests/macos_tunnel_retry.swift","-o",str(bindir/"probe")],env=env,check=True)
    for always_fail in (False, True):
        scenario_env=env|{"XENIA_WORKER_DATA_DIR":str(root/("always" if always_fail else "recovers")),"TEST_ALWAYS_FAIL":str(int(always_fail))}
        subprocess.run([str(bindir/"probe")],env=scenario_env,check=True,timeout=15)
        args = pathlib.Path(scenario_env["XENIA_WORKER_DATA_DIR"]).joinpath("ssh-args").read_text().splitlines()
        assert "BatchMode=yes" in args and "ExitOnForwardFailure=yes" in args
        assert "ServerAliveInterval=15" in args and "ServerAliveCountMax=3" in args
        assert "127.0.0.1:18080:127.0.0.1:8080" in args and "synthetic-token" not in args
    print("macOS worker reconnects SSH and stops retries after shutdown: PASS")
