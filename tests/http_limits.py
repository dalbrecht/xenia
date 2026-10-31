"""Exercise the controller's HTTP limits; run after `cargo build`."""
import os
import pathlib
import socket
import subprocess
import tempfile
import threading
import time
import urllib.error
import urllib.request

token = "synthetic-http-limit-token-123456"
with socket.socket() as probe:
    probe.bind(("127.0.0.1", 0))
    port = probe.getsockname()[1]
with tempfile.TemporaryDirectory(prefix="xenia-http-") as tmp:
    env = os.environ | {"XENIA_TOKEN": token, "XENIA_LISTEN": f"127.0.0.1:{port}", "XENIA_DB": str(pathlib.Path(tmp) / "db")}
    server = subprocess.Popen(["target/debug/xenia", "controller"], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    sockets = []
    try:
        url = f"http://127.0.0.1:{port}/api/state"
        request = urllib.request.Request(url, headers={"Authorization": "Bearer " + token})
        for _ in range(100):
            try:
                urllib.request.urlopen(request, timeout=2).close()
                break
            except OSError:
                time.sleep(.05)
        else:
            raise AssertionError("controller did not start")

        # More stalled authenticated bodies than the old four-thread pool.
        for _ in range(8):
            conn = socket.create_connection(("127.0.0.1", port), timeout=3)
            conn.sendall((f"POST /api/projects HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {token}\r\nContent-Length: 100\r\n\r\n{{").encode())
            sockets.append(conn)
        header = socket.create_connection(("127.0.0.1", port), timeout=3)
        header.sendall(b"GET /api/state HTTP/1.1\r\nHost: localhost\r\nX-Incomplete:")
        sockets.append(header)
        urllib.request.urlopen(request, timeout=2).close()
        print("eight stalled bodies do not block normal requests: PASS")

        # A trickle must not extend the total body deadline indefinitely.
        trickle = sockets[0]
        def drip():
            for _ in range(12):
                time.sleep(.8)
                try:
                    trickle.sendall(b" ")
                except OSError:
                    return
        sender = threading.Thread(target=drip, daemon=True)
        sender.start()
        trickle.settimeout(13)
        start = time.monotonic()
        data = trickle.recv(1024)
        assert b" 408 " in data and time.monotonic() - start < 12, data
        print("slow-drip body has a total deadline: PASS")
        header.settimeout(3)
        assert header.recv(1024) == b""
        print("incomplete headers time out: PASS")

        oversized = urllib.request.Request(url.replace("state", "projects"), data=b"x" * 1_000_001,
                                           headers={"Authorization": "Bearer " + token})
        try:
            urllib.request.urlopen(oversized, timeout=3)
            raise AssertionError("oversized body accepted")
        except urllib.error.HTTPError as error:
            assert error.code == 400, error.code
        print("oversized body rejected: PASS")
    finally:
        for conn in sockets:
            conn.close()
        server.terminate()
        server.wait(timeout=5)
