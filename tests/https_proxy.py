"""Exercise Caddy's internal-CA proxy over loopback; run after `cargo build`.

Set CADDY_BIN to an installed Caddy executable when it is not on PATH. This
test uses a temporary CA and explicitly disables trust-store installation.
"""
import json
import os
import pathlib
import shutil
import socket
import subprocess
import tempfile
import time

caddy = os.environ.get("CADDY_BIN") or shutil.which("caddy")
if not caddy:
    raise SystemExit("Install Caddy or set CADDY_BIN")
binary = pathlib.Path(os.environ.get("XENIA_TEST_BINARY", "target/debug/xenia")).resolve()
token = "synthetic-proxy-test-token-123456"
host = "xenia.example.internal"

def free_port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]

with tempfile.TemporaryDirectory(prefix="xenia-proxy-") as tmp:
    root = pathlib.Path(tmp)
    controller_port, proxy_port = free_port(), free_port()
    while controller_port == proxy_port:
        proxy_port = free_port()
    config = root / "Caddyfile"
    config.write_text(f'''{{
    admin off
    auto_https disable_redirects
    skip_install_trust
}}
{host}:{proxy_port} {{
    bind 127.0.0.1
    tls internal
    reverse_proxy 127.0.0.1:{controller_port}
}}
''')
    env = os.environ | {"XENIA_TOKEN": token, "XENIA_LISTEN": f"127.0.0.1:{controller_port}", "XENIA_DB": str(root / "controller.db"), "XDG_DATA_HOME": str(root / "data"), "XDG_CONFIG_HOME": str(root / "config")}
    ca = root / "data" / "caddy" / "pki" / "authorities" / "local" / "root.crt"
    processes = []
    def curl(path, *args):
        return subprocess.run(["curl", "-fsS", "--noproxy", "*", "--connect-timeout", "2", "--max-time", "5", "--resolve", f"{host}:{proxy_port}:127.0.0.1", "--cacert", str(ca), *args, f"https://{host}:{proxy_port}{path}"], capture_output=True, text=True)
    try:
        processes.append(subprocess.Popen([str(binary), "controller"], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL))
        processes.append(subprocess.Popen([caddy, "run", "--config", str(config), "--adapter", "caddyfile"], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL))
        for _ in range(100):
            if any(p.poll() is not None for p in processes):
                raise AssertionError("controller or proxy exited before becoming ready")
            if ca.exists() and curl("/").returncode == 0:
                break
            time.sleep(.1)
        else:
            raise AssertionError("HTTPS proxy did not become ready")
        assert "Xenia" in curl("/").stdout
        manifest = json.loads(curl("/manifest.webmanifest").stdout)
        assert manifest["start_url"] == "/" and manifest["display"] == "standalone"
        assert "serviceWorker" in curl("/app.js").stdout
        unauth = curl("/api/state")
        assert unauth.returncode != 0 and "401" in unauth.stderr
        project = curl("/api/projects", "-H", "Authorization: Bearer " + token, "-H", "Content-Type: application/json", "--data", '{"name":"example","repository":"repo"}')
        assert project.returncode == 0 and json.loads(project.stdout)["id"]
        state = curl("/api/state", "-H", "Authorization: Bearer " + token)
        assert state.returncode == 0 and json.loads(state.stdout)["projects"][0]["name"] == "example"
        wrong_name = subprocess.run(["curl", "-fsS", "--noproxy", "*", "--max-time", "5", "--resolve", f"wrong.example.internal:{proxy_port}:127.0.0.1", "--cacert", str(ca), f"https://wrong.example.internal:{proxy_port}/"], capture_output=True, text=True)
        assert wrong_name.returncode != 0, "TLS accepted an incorrect hostname"
        print("HTTPS proxy, certificate validation, authentication, API and PWA assets: PASS")
        if os.environ.get("XENIA_PWA_BROWSER_TEST"):
            browser_env = os.environ | {"XENIA_HTTPS_TEST_URL": f"https://{host}:{proxy_port}/", "XENIA_HTTPS_TEST_TOKEN": token, "XENIA_HTTPS_TEST_SELF_SIGNED": "1"}
            subprocess.run(["node", "tests/https_pwa.cjs"], env=browser_env, check=True)
    finally:
        for process in reversed(processes):
            process.terminate()
        for process in reversed(processes):
            process.wait(timeout=10)
