"""Exercise durable approval authorization and replay; run after `cargo build`."""
import json
import os
import pathlib
import socket
import subprocess
import tempfile
import time
import urllib.error
import urllib.request
import uuid

with tempfile.TemporaryDirectory(prefix="xenia-approval-api-") as tmp:
    root = pathlib.Path(tmp)
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        port = sock.getsockname()[1]
    token = "synthetic-approval-test-token-123456"
    url = f"http://127.0.0.1:{port}/api/"
    env = os.environ | {"XENIA_TOKEN": token, "XENIA_LISTEN": f"127.0.0.1:{port}", "XENIA_DB": str(root / "controller.db")}
    binary = os.environ.get("XENIA_TEST_BINARY", "target/debug/xenia")
    process = None
    def request(path, body=None, credential=token):
        data = None if body is None else json.dumps(body).encode()
        req = urllib.request.Request(url + path, data, {"Authorization": "Bearer " + credential, "Content-Type": "application/json"})
        with urllib.request.urlopen(req, timeout=3) as response:
            return json.load(response)
    def status(path, body, credential, expected):
        try:
            request(path, body, credential)
            raise AssertionError("request unexpectedly succeeded")
        except urllib.error.HTTPError as error:
            assert error.code == expected, (error.code, expected)
    def start():
        proc = subprocess.Popen([binary, "controller"], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        for _ in range(100):
            try:
                request("state")
                return proc
            except OSError:
                time.sleep(.05)
        proc.terminate()
        proc.wait(timeout=5)
        raise AssertionError("controller did not start")
    try:
        process = start()
        first = request("workers/enroll", {"id": "one"})["token"]
        other = request("workers/enroll", {"id": "two"})["token"]
        request("heartbeat", {"id":"one", "name":"one", "capacity":1, "runners":["codex"], "instance_id":"instance"}, first)
        project = request("projects", {"name":"example", "repository":"repo"})["id"]
        job = request("jobs", {"project_id":project, "runner":"codex", "prompt":"work"})["id"]
        request("claim", {"id":"one", "instance_id":"instance", "claim_id":str(uuid.uuid4()), "eligible":[project]}, first)
        approval = {"node_id":"one", "job_id":job, "instance_id":"instance", "request_id":str(uuid.uuid4()), "kind":"command", "details":{"command":"echo example", "cwd":"/tmp/example"}}
        status("approval/request", approval, other, 403)
        status("approval/request", approval, token, 401)
        request("approval/request", approval, first)
        assert len(request("approval/list", {"id":job})) == 1
        status("approval/decide", {"id":approval["request_id"], "decision":"accept"}, first, 401)
        request("approval/decide", {"id":approval["request_id"], "decision":"accept"})
        process.terminate(); process.wait(timeout=5); process = start()
        assert request("approval/request", approval, first)["decision"] == "accept"
        assert request("approval/poll", approval, first) == {"decision":"accept", "active":True}
        assert request("approval/list", {"id":job})[0]["decision"] == "accept"
        status("approval/ack", approval, other, 403)
        request("approval/ack", approval, first)
        request("approval/ack", approval, first)
        assert not request("approval/list", {"id":job})
        incomplete = approval | {"request_id":str(uuid.uuid4()), "details":{"command":"echo incomplete"}}
        request("approval/request", incomplete, first)
        assert request("approval/list", {"id":job})[0]["can_accept"] is False
        status("approval/decide", {"id":incomplete["request_id"], "decision":"accept"}, token, 400)
        request("approval/decide", {"id":incomplete["request_id"], "decision":"decline"})
        assert request("approval/poll", incomplete, first)["decision"] == "decline"
        request("approval/ack", incomplete, first)
        status("approval/decide", {"id":approval["request_id"], "decision":"decline"}, token, 400)
        question = approval | {"request_id":str(uuid.uuid4()), "kind":"question", "details":{"questions":[{"question":"Which?", "multiSelect":False}]}}
        request("approval/request", question, first)
        answer = {"id":question["request_id"], "answers":{"Which?":"custom"}}
        status("approval/answer", answer, first, 401)
        request("approval/answer", answer)
        process.terminate(); process.wait(timeout=5); process = start()
        assert request("approval/request", question, first)["answers"] == answer["answers"]
        assert request("approval/poll", question, first)["answers"] == answer["answers"]
        request("approval/ack", question, first)
        withdrawn = question | {"request_id":str(uuid.uuid4())}
        request("approval/request", withdrawn, first)
        status("approval/withdraw", withdrawn, other, 403)
        status("approval/withdraw", withdrawn, token, 401)
        request("approval/withdraw", withdrawn, first)
        request("approval/withdraw", withdrawn, first)
        assert request("approval/poll", withdrawn, first)["active"] is False
        assert not request("approval/list", {"id":job})
        status("approval/answer", {"id":withdrawn["request_id"],"answers":{"Which?":"late"}}, token, 400)
        status("approval/decide", {"id":withdrawn["request_id"],"decision":"decline"}, token, 400)
        process.terminate(); process.wait(timeout=5); process = start()
        assert request("approval/poll", withdrawn, first)["active"] is False
        status("approval/request", withdrawn, first, 400)
        decided_then_withdrawn = question | {"request_id":str(uuid.uuid4())}
        request("approval/request", decided_then_withdrawn, first)
        request("approval/answer", {"id":decided_then_withdrawn["request_id"],"answers":{"Which?":"before withdrawal"}})
        request("approval/withdraw", decided_then_withdrawn, first)
        assert request("approval/poll", decided_then_withdrawn, first)["active"] is False
        status("approval/answer", {"id":decided_then_withdrawn["request_id"],"answers":{"Which?":"before withdrawal"}}, token, 400)
        request("cancel", {"id":job})
        assert request("approval/poll", approval, first)["active"] is False
        status("approval/decide", {"id":approval["request_id"], "decision":"accept"}, token, 400)
        print("Approval credential scope, decision replay after restart, and cancellation: PASS")
    finally:
        if process:
            process.terminate()
            process.wait(timeout=5)
