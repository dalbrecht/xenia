#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
mkdir -p dist
set -- Cargo.toml Cargo.lock LICENSE README.md .gitignore .github src web apps docs scripts tests script
if [ -d assets ]; then set -- "$@" assets; fi
# BSD tar and GNU tar spell ownership overrides differently. Do not publish
# builder account names, ACLs, resource forks, or extended filesystem metadata.
case "$(tar --version 2>/dev/null)" in
  *bsdtar*) set -- --uid 0 --gid 0 --uname root --gname root "$@" ;;
  *) set -- --owner=0 --group=0 "$@" ;;
esac
COPYFILE_DISABLE=1 tar --no-xattrs --no-acls --exclude='.DS_Store' --exclude='._*' --exclude='__pycache__' --exclude='*.pyc' --exclude='apps/macos-worker/.build' --exclude='apps/windows-worker/bin' --exclude='apps/windows-worker/obj' -czf dist/xenia-source.tar.gz "$@"
printf 'Created dist/xenia-source.tar.gz\n'
