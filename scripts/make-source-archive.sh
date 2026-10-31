#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
mkdir -p dist
tar --exclude='__pycache__' --exclude='*.pyc' --exclude='apps/macos-worker/.build' --exclude='apps/windows-worker/bin' --exclude='apps/windows-worker/obj' -czf dist/xenia-source.tar.gz Cargo.toml Cargo.lock LICENSE README.md .gitignore .github src web apps docs scripts tests script
printf 'Created dist/xenia-source.tar.gz\n'
