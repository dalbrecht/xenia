"""Experimental Codex app-server approval integration; run after cargo build."""
import json
import os
import pathlib
import socket
import sqlite3
import subprocess
import tempfile
import time
import urllib.error
import urllib.request

with tempfile.TemporaryDirectory(prefix="xenia-codex-approval-") as tmp:
    root=pathlib.Path(tmp)
    repo=root/"projects"/"example"/"repo"; repo.mkdir(parents=True)
    subprocess.run(["git","init","-q",str(repo)],check=True)
    subprocess.run(["git","-C",str(repo),"-c","user.name=Test","-c","user.email=test@example.invalid","commit","--allow-empty","-qm","initial"],check=True)
    bindir=root/"bin"; bindir.mkdir(); runner=bindir/"codex"
    runner.write_text('''#!/usr/bin/env python3
import json, os, sys
if any(k.startswith("XENIA_") for k in os.environ): sys.exit(8)
def send(message): print(json.dumps(message), flush=True)
turn="synthetic-turn"
pending=set()
for line in sys.stdin:
    request=json.loads(line); method=request.get("method")
    if method == "initialize": send({"id":request["id"],"result":{}})
    elif method in ("thread/start","thread/resume"):
        send({"id":request["id"],"result":{"thread":{"id":"synthetic-thread"},"cwd":os.getcwd(),"sandbox":{"type":"workspaceWrite"},"approvalPolicy":"on-request"}})
    elif method == "turn/start":
        send({"id":request["id"],"result":{"turn":{"id":turn}}})
        send({"method":"item/agentMessage/delta","params":{"threadId":"synthetic-thread","turnId":turn,"itemId":"message","delta":"streamed text"}})
        if "followup" in request["params"]["input"][0]["text"]:
            send({"method":"turn/completed","params":{"threadId":"synthetic-thread","turn":{"id":turn,"status":"completed"}}})
        else:
            pending={"approval-rpc","approval-second"} if "parallel" in request["params"]["input"][0]["text"] else {"approval-rpc"}
            for approval_id in sorted(pending):
                send({"id":approval_id,"method":"item/commandExecution/requestApproval","params":{"threadId":"synthetic-thread","turnId":turn,"itemId":approval_id,"command":"printf example","cwd":os.getcwd(),"startedAtMs":1}})
    elif request.get("id") in pending:
        pending.remove(request["id"])
        decision=request.get("result",{}).get("decision")
        if not pending:
            send({"method":"item/completed","params":{"threadId":"synthetic-thread","turnId":turn,"item":{"id":"message","type":"agentMessage","text":"streamed text"}}})
            send({"method":"turn/completed","params":{"threadId":"synthetic-thread","turn":{"id":turn,"status":"completed" if decision in ("accept","decline") else "failed"}}})
''',encoding="utf-8"); runner.chmod(0o755)
    with socket.socket() as sock: sock.bind(("127.0.0.1",0)); port=sock.getsockname()[1]
    url=f"http://127.0.0.1:{port}/api/"; token="synthetic-app-server-token-123456"
    env=os.environ|{"XENIA_TOKEN":token,"XENIA_LISTEN":f"127.0.0.1:{port}","XENIA_DB":str(root/"controller.db"),"GIT_CONFIG_GLOBAL":os.devnull,"GIT_CONFIG_NOSYSTEM":"1"}
    binary=os.environ.get("XENIA_TEST_BINARY","target/debug/xenia"); processes=[]
    def request(path,body=None):
        data=None if body is None else json.dumps(body).encode(); req=urllib.request.Request(url+path,data,{"Authorization":"Bearer "+token,"Content-Type":"application/json"})
        with urllib.request.urlopen(req,timeout=4) as response:return json.load(response)
    def until(test,seconds=25):
        end=time.monotonic()+seconds
        while time.monotonic()<end:
            try:
                result=test()
                if result:return result
            except (OSError,StopIteration):pass
            time.sleep(.2)
        raise AssertionError("Timed out waiting for app-server integration")
    def start_controller():
        proc=subprocess.Popen([binary,"controller"],env=env,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL);until(lambda:request("state"));return proc
    def job(id):return next(item for item in request("state")["jobs"] if item["id"]==id)
    try:
        controller=start_controller(); processes.append(controller)
        credential=request("workers/enroll",{"id":"test"})["token"]
        worker_env=env|{"XENIA_WORKER_TOKEN":credential,"XENIA_WORKER_DB":str(root/"worker.db"),"XENIA_CONTROLLER":f"http://127.0.0.1:{port}","XENIA_NODE_ID":"test","XENIA_PROJECTS_ROOT":str(root/"projects"),"XENIA_CODEX_APP_SERVER":"1","PATH":str(bindir)+os.pathsep+os.environ["PATH"]}
        worker=subprocess.Popen([binary,"worker"],env=worker_env,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL);processes.append(worker)
        until(lambda:request("state")["nodes"])
        project=request("projects",{"name":"example","repository":"repo"})["id"]
        first=request("jobs",{"project_id":project,"runner":"codex","prompt":"first"})["id"]
        pending=until(lambda:request("approval/list",{"id":first}))
        assert pending[0]["can_accept"] and pending[0]["details"]["command"]=="printf example"
        until(lambda:"streamed text" in request("outputs",{"ids":[first]})[first])
        controller.terminate();controller.wait(timeout=5);controller=start_controller();processes[0]=controller
        request("approval/decide",{"id":pending[0]["id"],"decision":"accept"})
        completed=until(lambda:(j if (j:=job(first))["state"] in ("succeeded","failed","unknown") else None),35)
        assert completed["state"]=="succeeded",completed
        assert completed["session_id"]=="synthetic-thread"
        def delivered():
            with sqlite3.connect(root/"controller.db") as database:
                row=database.execute("SELECT delivered FROM approval_requests WHERE id=?",(pending[0]["id"],)).fetchone()
                return row and row[0]==1
        until(delivered)
        follow=request("followup",{"id":first,"prompt":"followup"})["id"]
        followup=until(lambda:(j if (j:=job(follow))["state"] in ("succeeded","failed","unknown") else None))
        assert followup["state"]=="succeeded" and followup["worktree"]==completed["worktree"]
        print("Codex app-server streams, recovers approval after controller restart, and resumes: PASS")
        canceled=request("jobs",{"project_id":project,"runner":"codex","prompt":"cancel"})["id"]
        approval=until(lambda:request("approval/list",{"id":canceled}))[0]
        request("cancel",{"id":canceled})
        stopped=until(lambda:(j if (j:=job(canceled))["state"] in ("canceled","failed","unknown") else None))
        assert stopped["state"]=="canceled",stopped
        try: request("approval/decide",{"id":approval["id"],"decision":"accept"}); raise AssertionError("canceled approval accepted")
        except urllib.error.HTTPError as error: assert error.code==400
        print("Cancellation stops a runner waiting for approval: PASS")
        parallel=request("jobs",{"project_id":project,"runner":"codex","prompt":"parallel"})["id"]
        approvals=until(lambda:(items if len(items:=request("approval/list",{"id":parallel}))==2 else None))
        request("approval/decide",{"id":approvals[0]["id"],"decision":"decline"})
        request("approval/decide",{"id":approvals[1]["id"],"decision":"accept"})
        done=until(lambda:(j if (j:=job(parallel))["state"] in ("succeeded","failed","unknown") else None),35)
        assert done["state"]=="succeeded" and done["permission_denied"]==1,done
        print("Concurrent approvals and a denied action remain visible: PASS")
    finally:
        for process in reversed(processes):
            if process.poll() is None:process.terminate()
        for process in reversed(processes):process.wait(timeout=10)
