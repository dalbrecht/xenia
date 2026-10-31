"""Test macOS app packaging rollback with fake tools in a disposable workspace."""
import os
import pathlib
import shutil
import subprocess
import tempfile

with tempfile.TemporaryDirectory(prefix="xenia-package-test-") as tmp:
    root = pathlib.Path(tmp)
    (root / "scripts").mkdir()
    for name in ("build-macos-app.sh", "build-release.sh", "check-macos-worker-stopped.sh"):
        shutil.copy2(pathlib.Path("scripts") / name, root / "scripts" / name)
    (root / "LICENSE").write_text("test license")
    (root / "assets").mkdir()
    (root / "assets/xenia.icns").write_bytes(b"synthetic icon")
    tools = root / "fake-bin"
    tools.mkdir()
    commands = {
        "pgrep": '''#!/bin/sh
n=0; [ ! -f "$TEST_COUNTER" ] || n=$(cat "$TEST_COUNTER")
n=$((n+1)); printf '%s' "$n" > "$TEST_COUNTER"
if [ "${TEST_START_DURING_BUILD-}" = 1 ] && [ "$n" -ge 3 ]; then exit 0; fi
exit 1
''',
        "cargo": '''#!/bin/sh
mkdir -p target/release
printf 'synthetic engine' > target/release/xenia
chmod +x target/release/xenia
''',
        "swift": '''#!/bin/sh
mkdir -p apps/macos-worker/.build/release
printf 'synthetic app' > apps/macos-worker/.build/release/XeniaWorker
''',
        "strip": "#!/bin/sh\nexit 0\n",
        "codesign": '''#!/bin/sh
[ "${TEST_SIGN_FAIL-}" != 1 ] || exit 1
if [ "$1" = --force ]; then printf '%s\\n' "$@" > "$TEST_SIGNATURE_ARGS"; fi
exit 0
''',
        "mv": '''#!/bin/sh
case "$1" in
  dist/.xenia-build.*/XeniaWorker.app) [ "${TEST_PUBLISH_FAIL-}" != 1 ] || exit 1 ;;
  dist/.xenia-build.*/previous.app) [ "${TEST_RESTORE_FAIL-}" != 1 ] || exit 1 ;;
esac
exec /bin/mv "$@"
''',
    }
    for name, source in commands.items():
        tool = tools / name
        tool.write_text(source)
        tool.chmod(0o755)
    target = root / "dist" / "XeniaWorker.app"
    target.mkdir(parents=True)
    sentinel = target / "previous-version"
    sentinel.write_text("preserve me")
    counter = root / "counter"
    base = os.environ | {"PATH": str(tools) + os.pathsep + os.environ.get("PATH", ""), "TEST_COUNTER": str(counter), "TEST_SIGNATURE_ARGS": str(root / "signature-args")}
    def run(**flags):
        counter.unlink(missing_ok=True)
        return subprocess.run(["scripts/build-macos-app.sh"], cwd=root, env=base | flags, capture_output=True, text=True, timeout=15)
    for scenario in ({"TEST_SIGN_FAIL":"1"}, {"TEST_PUBLISH_FAIL":"1"}, {"TEST_START_DURING_BUILD":"1"}):
        result = run(**scenario)
        assert result.returncode != 0, scenario
        assert sentinel.read_text() == "preserve me", scenario
        assert not list((root / "dist").glob(".xenia-build.*")), scenario
    result = run(TEST_PUBLISH_FAIL="1", TEST_RESTORE_FAIL="1")
    assert result.returncode != 0
    backups = list((root / "dist").glob(".xenia-build.*/previous.app"))
    assert len(backups) == 1 and (backups[0] / "previous-version").read_text() == "preserve me"
    assert "remains in" in result.stderr
    staging = backups[0].parent
    backups[0].rename(target)
    shutil.rmtree(staging)
    result = run(XENIA_SIGNING_IDENTITY="Synthetic Local Signing Identity")
    assert result.returncode == 0, result.stderr
    signing_args = (root / "signature-args").read_text().splitlines()
    assert signing_args[signing_args.index("--sign") + 1] == "Synthetic Local Signing Identity"
    assert (target / "Contents" / "MacOS" / "XeniaWorker").read_text() == "synthetic app"
    assert (target / "Contents" / "MacOS" / "xenia-engine").read_text() == "synthetic engine"
    assert not sentinel.exists()
    assert not list((root / "dist").glob(".xenia-build.*"))
    print("macOS staging preserves the previous app on signing, publishing, and late-start failures: PASS")
