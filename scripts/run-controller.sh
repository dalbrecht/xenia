#!/bin/sh
set -eu
umask 077
ROOT=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
DATA_DIR=${XENIA_DATA_DIR:-"$HOME/.local/share/xenia"}
mkdir -p "$DATA_DIR"
TOKEN_FILE="$DATA_DIR/controller-token"
if [ ! -e "$TOKEN_FILE" ]; then
  command -v openssl >/dev/null || { echo 'OpenSSL is required to generate the controller token.' >&2; exit 1; }
  (set -C; openssl rand -hex 32 > "$TOKEN_FILE") || { echo 'Could not create controller token.' >&2; exit 1; }
  echo "Created a private controller token at $TOKEN_FILE" >&2
fi
XENIA_TOKEN=$(cat "$TOKEN_FILE")
XENIA_DB=${XENIA_DB:-"$DATA_DIR/controller.db"}
XENIA_LISTEN=${XENIA_LISTEN:-127.0.0.1:8080}
XENIA_ENGINE=${XENIA_ENGINE:-"$ROOT/target/release/xenia"}
export XENIA_TOKEN XENIA_DB XENIA_LISTEN
[ -x "$XENIA_ENGINE" ] || { echo 'Controller engine is missing; build it first.' >&2; exit 1; }
exec "$XENIA_ENGINE" controller
