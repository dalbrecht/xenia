"""Exercise native macOS pairing subprocess and cancellation without Keychain."""
import os, pathlib, subprocess, tempfile, uuid
with tempfile.TemporaryDirectory(prefix='xenia-macos-pair-') as tmp:
    root=pathlib.Path(tmp); app=root/'Probe.app'/'Contents'; binary=app/'MacOS';binary.mkdir(parents=True)
    (app/'Info.plist').write_text(f'''<?xml version="1.0"?><plist version="1.0"><dict><key>CFBundleIdentifier</key><string>org.xenia.pairing.probe.{uuid.uuid4().hex}</string><key>CFBundleExecutable</key><string>probe</string><key>CFBundlePackageType</key><string>APPL</string></dict></plist>''')
    fake=binary/'xenia-engine';fake.write_text('''#!/bin/sh
[ "$1" = pair ] || exit 2
[ -z "${XENIA_WORKER_TOKEN-}" ] || exit 3
[ "$XENIA_CONTROLLER" = https://controller.example:8443 ] || exit 4
printf '%s\\n' '{"state":"pending","code":"ABC12345","expires":9999999999}'
if [ "$TEST_CANCEL" = 1 ]; then
  while [ ! -f "$XENIA_PAIR_STOP_FILE" ]; do sleep .02; done
  echo 'Pairing cancelled' >&2
  exit 1
fi
sleep .2
printf '%s\\n' '{"state":"completed","token":"cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"}'
''');fake.chmod(0o755)
    subprocess.run(['swiftc','-module-cache-path',str(root/'swift-cache'),'apps/macos-worker/Sources/XeniaWorker/Services/WorkerManager.swift','apps/macos-worker/Sources/XeniaWorker/Services/SecretStore.swift','tests/macos_pairing.swift','-o',str(binary/'probe')],check=True)
    for cancel in ('0','1'):
        subprocess.run([str(binary/'probe')],env=os.environ|{'TEST_CANCEL':cancel,'XENIA_WORKER_TOKEN':'synthetic-inherited-token-must-be-cleared'},check=True,timeout=15)
    print('macOS pairing code display, private credential receipt, inherited secret clearing and cancellation: PASS')
