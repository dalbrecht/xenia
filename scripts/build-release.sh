#!/bin/sh
# Build a release engine without embedding the builder's local source paths.
set -eu
ROOT=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd -P)
cd "$ROOT"
# The packaging scripts use this deterministic, project-local output directory.
CARGO_TARGET_DIR="$ROOT/target"
export CARGO_TARGET_DIR
separator=$(printf '\037')
flags=${CARGO_ENCODED_RUSTFLAGS-}
# Cargo treats RUSTFLAGS as whitespace-separated when encoded flags are absent.
if [ -z "$flags" ] && [ -n "${RUSTFLAGS-}" ]; then
  set -f
  for flag in $RUSTFLAGS; do flags="${flags:+$flags$separator}$flag"; done
  set +f
fi
append_remap() {
  from=$1
  to=$2
  [ -n "$from" ] && [ "$from" != / ] || return 0
  flags="${flags:+$flags$separator}--remap-path-prefix=$from=$to"
}
append_remap "${HOME-}" /xenia-build/home
append_remap "${CARGO_HOME:-${HOME-}/.cargo}" /xenia-build/cargo
append_remap "$ROOT" /xenia-build/source
export CARGO_ENCODED_RUSTFLAGS="$flags"
cargo build --release "$@"
# Fail rather than publish a binary that still contains these local paths.
for path in "${HOME-}" "${CARGO_HOME-}" "$ROOT"; do
  if [ -n "$path" ] && [ "$path" != / ] && LC_ALL=C grep -a -F -q "$path" target/release/xenia; then
    echo 'Release engine contains a local build path; inspect before sharing.' >&2
    exit 1
  fi
done
