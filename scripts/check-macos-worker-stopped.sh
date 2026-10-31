#!/bin/sh
# Refuse to replace the application bundle if any worker app may be running.
set -u
status=0
pgrep -x XeniaWorker >/dev/null || status=$?
case "$status" in
  0) echo 'Xenia Worker is running; stop its jobs and quit it before rebuilding.' >&2; exit 1 ;;
  1) exit 0 ;;
  *) echo 'Could not check whether Xenia Worker is running; refusing to rebuild.' >&2; exit 1 ;;
esac
