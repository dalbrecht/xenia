#!/usr/bin/env python3
"""Install and control the per-user macOS Xenia controller LaunchAgent."""
import argparse
import os
import pathlib
import plistlib
import re
import shutil
import stat
import subprocess
import sys
import tempfile

ROOT = pathlib.Path(__file__).resolve().parent.parent
DEFAULT_LABEL = "org.xenia.controller"


def private_directory(path):
    path.mkdir(mode=0o700, parents=True, exist_ok=True)
    if not path.is_dir() or stat.S_IMODE(path.stat().st_mode) & 0o077:
        raise RuntimeError(f"Controller data directory must be private (mode 700): {path}")


def private_log(path):
    flags = os.O_WRONLY | os.O_CREAT | os.O_APPEND
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW
    fd = os.open(path, flags, 0o600)
    try:
        info = os.fstat(fd)
        if not stat.S_ISREG(info.st_mode) or stat.S_IMODE(info.st_mode) & 0o077:
            raise RuntimeError(f"Controller log must be a private regular file: {path}")
    finally:
        os.close(fd)


def atomic_copy(source, target, mode):
    fd, temporary = tempfile.mkstemp(prefix=".xenia-", dir=target.parent)
    try:
        with os.fdopen(fd, "wb") as dst, source.open("rb") as src:
            shutil.copyfileobj(src, dst)
        os.chmod(temporary, mode)
        os.replace(temporary, target)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


def atomic_plist(data, path):
    fd, temporary = tempfile.mkstemp(prefix=".xenia-", dir=path.parent)
    try:
        with os.fdopen(fd, "wb") as file:
            plistlib.dump(data, file)
        os.chmod(temporary, 0o600)
        os.replace(temporary, path)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("install", "start", "stop", "status", "uninstall"))
    parser.add_argument("--data-dir", type=pathlib.Path, default=pathlib.Path.home() / ".local/share/xenia")
    parser.add_argument("--agents-dir", type=pathlib.Path, default=pathlib.Path.home() / "Library/LaunchAgents")
    parser.add_argument("--label", default=DEFAULT_LABEL)
    parser.add_argument("--listen", default="127.0.0.1:8080")
    args = parser.parse_args()
    if sys.platform != "darwin":
        raise RuntimeError("This service manager requires macOS")
    if args.label != DEFAULT_LABEL and not re.fullmatch(r"org\.xenia\.controller\.[A-Za-z0-9_-]+", args.label):
        raise RuntimeError("Label must be org.xenia.controller or use its testing suffix")
    if not re.fullmatch(r"127\.0\.0\.1:[0-9]{1,5}", args.listen) or not 1 <= int(args.listen.rsplit(":", 1)[1]) <= 65535:
        raise RuntimeError("Listen address must be 127.0.0.1 with a valid port")
    data = args.data_dir.expanduser().resolve()
    agents = args.agents_dir.expanduser().resolve()
    plist = agents / (args.label + ".plist")
    domain = f"gui/{os.getuid()}"
    service = f"{domain}/{args.label}"
    launchctl = "/bin/launchctl"
    launcher = data / "bin/run-controller.sh"
    engine = data / "bin/xenia"
    marker = data / "controller-install.plist"

    def loaded():
        result = subprocess.run([launchctl, "print", service], stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
        if result.returncode != 0:
            return False
        if not any(line.strip() == "program = " + str(launcher) for line in result.stdout.splitlines()):
            raise RuntimeError("A different loaded service uses this label; refusing to control it")
        return True

    def owned():
        if not plist.is_file() or plist.is_symlink():
            return False
        try:
            with plist.open("rb") as file:
                existing = plistlib.load(file)
            return existing.get("Label") == args.label and existing.get("ProgramArguments") == [str(launcher)] and existing.get("EnvironmentVariables", {}).get("XENIA_DATA_DIR") == str(data)
        except (OSError, ValueError, plistlib.InvalidFileException):
            return False

    def managed_files():
        if not marker.is_file() or marker.is_symlink():
            return False
        try:
            with marker.open("rb") as file:
                return plistlib.load(file) == {"label": args.label, "format": 1}
        except (OSError, ValueError, plistlib.InvalidFileException):
            return False

    if args.action == "status":
        result = subprocess.run([launchctl, "print", service])
        return result.returncode
    if args.action == "stop":
        if not owned():
            raise RuntimeError("No matching Xenia service configuration; refusing to stop another service")
        if loaded():
            subprocess.run([launchctl, "bootout", service], check=True)
        print("Controller stopped. Its data is retained.")
        return 0
    if args.action == "uninstall":
        if not owned():
            raise RuntimeError("No matching Xenia service configuration; refusing to remove it")
        if loaded():
            subprocess.run([launchctl, "bootout", service], check=True)
        plist.unlink()
        print(f"Controller service removed. Data and installed files remain in {data}.")
        return 0
    if args.action == "start":
        if not owned():
            raise RuntimeError("No matching Xenia service configuration; run install first")
        if not loaded():
            subprocess.run([launchctl, "bootstrap", domain, str(plist)], check=True)
        subprocess.run([launchctl, "kickstart", service], check=True)
        print("Controller service started. Check status and the controller log if it does not come online.")
        return 0

    if plist.exists() or plist.is_symlink():
        if not owned():
            raise RuntimeError("An unrelated launch agent already uses this name; refusing to overwrite it")
        if loaded():
            raise RuntimeError("Stop the controller service before upgrading it")
    elif (launcher.exists() or engine.exists()) and not managed_files():
        raise RuntimeError("Installed controller files already exist without a matching service; inspect them before installing")
    elif loaded():
        raise RuntimeError("A controller with this label is already loaded; stop it before installing")
    release = ROOT / "target/release/xenia"
    source_launcher = ROOT / "scripts/run-controller.sh"
    if not release.is_file() or not os.access(release, os.X_OK):
        raise RuntimeError("Build the release engine first with ./scripts/build-release.sh")
    private_directory(data)
    private_directory(data / "bin")
    agents.mkdir(mode=0o700, parents=True, exist_ok=True)
    private_log(data / "controller.log")
    private_log(data / "controller.out.log")
    if marker.exists() and not managed_files():
        raise RuntimeError("Controller installation marker belongs to another service")
    atomic_plist({"label": args.label, "format": 1}, marker)
    atomic_copy(release, engine, 0o700)
    atomic_copy(source_launcher, launcher, 0o700)
    config = {"Label": args.label, "ProgramArguments": [str(launcher)], "RunAtLoad": True, "KeepAlive": True,
              "WorkingDirectory": str(data), "StandardOutPath": str(data / "controller.out.log"), "StandardErrorPath": str(data / "controller.log"),
              "EnvironmentVariables": {"HOME": str(pathlib.Path.home()), "PATH": "/usr/bin:/bin:/usr/sbin:/sbin:/opt/homebrew/bin:/usr/local/bin", "XENIA_DATA_DIR": str(data), "XENIA_ENGINE": str(engine), "XENIA_LISTEN": args.listen}}
    atomic_plist(config, plist)
    print(f"Installed {plist}. Start it with the start action; it will also load at login.")
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, RuntimeError, subprocess.CalledProcessError) as error:
        print(f"Controller service: {error}", file=sys.stderr)
        sys.exit(1)
