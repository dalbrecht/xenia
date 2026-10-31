#!/usr/bin/env bash
set -euo pipefail
MODE="${1:-run}"
cd "$(dirname "$0")/.."
./scripts/check-macos-worker-stopped.sh
./scripts/build-macos-app.sh
APP="$PWD/dist/XeniaWorker.app"
case "$MODE" in
  run) /usr/bin/open -n "$APP" ;;
  --debug|debug) lldb -- "$APP/Contents/MacOS/XeniaWorker" ;;
  --logs|logs) /usr/bin/open -n "$APP"; /usr/bin/log stream --info --style compact --predicate 'process == "XeniaWorker"' ;;
  --telemetry|telemetry) /usr/bin/open -n "$APP"; /usr/bin/log stream --info --style compact --predicate 'subsystem == "org.xenia.worker"' ;;
  --verify|verify) /usr/bin/open -n "$APP"; sleep 1; pgrep -x XeniaWorker >/dev/null ;;
  *) echo "Usage: $0 [run|--debug|--logs|--telemetry|--verify]" >&2; exit 2 ;;
esac
