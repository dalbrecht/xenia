#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
./scripts/check-macos-worker-stopped.sh
./scripts/build-release.sh
swift build --package-path apps/macos-worker -c release
# The worker may have started while the compiler was running.
./scripts/check-macos-worker-stopped.sh
mkdir -p dist
STAGING=$(mktemp -d "dist/.xenia-build.XXXXXX")
APP="$STAGING/XeniaWorker.app"
TARGET=dist/XeniaWorker.app
PREVIOUS="$STAGING/previous.app"
published=0
cleanup() {
  if [ "$published" -eq 0 ] && { [ -e "$PREVIOUS" ] || [ -L "$PREVIOUS" ]; }; then
    if [ ! -e "$TARGET" ] && [ ! -L "$TARGET" ]; then
      if ! mv "$PREVIOUS" "$TARGET"; then
        echo "Could not restore the previous app; it remains in $PREVIOUS." >&2
        return 1
      fi
    else
      echo "The previous app remains in $PREVIOUS; restore it manually after inspecting $TARGET." >&2
      return 1
    fi
  fi
  rm -rf "$STAGING"
}
trap cleanup EXIT
trap 'exit 1' HUP INT TERM
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp apps/macos-worker/.build/release/XeniaWorker "$APP/Contents/MacOS/XeniaWorker"
# SwiftPM includes source and object paths in debug metadata even for release builds.
strip -S "$APP/Contents/MacOS/XeniaWorker"
for path in "${HOME-}" "$PWD"; do
  if [ -n "$path" ] && [ "$path" != / ] && LC_ALL=C grep -a -F -q "$path" "$APP/Contents/MacOS/XeniaWorker"; then
    echo 'macOS app contains a local build path; inspect before sharing.' >&2
    exit 1
  fi
done
cp target/release/xenia "$APP/Contents/MacOS/xenia-engine"
cp LICENSE "$APP/Contents/Resources/LICENSE"
cp assets/xenia.icns "$APP/Contents/Resources/Xenia.icns"
cat > "$APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict><key>CFBundleExecutable</key><string>XeniaWorker</string><key>CFBundleIdentifier</key><string>org.xenia.worker</string><key>CFBundleName</key><string>Xenia Worker</string><key>CFBundleIconFile</key><string>Xenia</string><key>CFBundlePackageType</key><string>APPL</string><key>CFBundleShortVersionString</key><string>0.1.0</string><key>LSMinimumSystemVersion</key><string>13.0</string><key>NSHighResolutionCapable</key><true/></dict></plist>
PLIST
# Use a consistent local identity when configured; otherwise ad-hoc sign.
# The identity is supplied locally and is never stored in the source tree.
codesign --force --deep --sign "${XENIA_SIGNING_IDENTITY:--}" "$APP"
codesign --verify --deep --strict "$APP"
for binary in "$APP/Contents/MacOS/XeniaWorker" "$APP/Contents/MacOS/xenia-engine"; do
  for path in "${HOME-}" "${CARGO_HOME-}" "$PWD"; do
    if [ -n "$path" ] && [ "$path" != / ] && LC_ALL=C grep -a -F -q "$path" "$binary"; then
      echo 'Signed bundle contains a local build path; inspect before sharing.' >&2
      exit 1
    fi
  done
done
# Never replace an app while a worker has started during packaging.
./scripts/check-macos-worker-stopped.sh
if [ -e "$TARGET" ] || [ -L "$TARGET" ]; then mv "$TARGET" "$PREVIOUS"; fi
if [ -e "$TARGET" ] || [ -L "$TARGET" ]; then
  echo 'The target changed while publishing; leaving the previous app in staging.' >&2
  exit 1
fi
mv "$APP" "$TARGET"
published=1
printf 'Built %s\n' "$TARGET"
