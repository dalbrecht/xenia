#!/usr/bin/env python3
"""Disposable Unix/macOS long-history sample, no model usage or private projects.
Run after scripts/build-release.sh. Prints sampled process RSS, sizes and latency.
Also verifies that the compatibility snapshot rejects this oversized history.
"""
import json, os, pathlib, socket, sqlite3, statistics, subprocess, tempfile, threading, time, urllib.error, urllib.request

binary=str(pathlib.Path('target/release/xenia').resolve())
with tempfile.TemporaryDirectory(prefix='xenia-history-resources-') as tmp:
    root=pathlib.Path(tmp); db=root/'db'
    with socket.socket() as sock:
        sock.bind(('127.0.0.1',0)); port=sock.getsockname()[1]
    token='synthetic-history-resource-token'
    env=os.environ|{'XENIA_TOKEN':token,'XENIA_LISTEN':f'127.0.0.1:{port}','XENIA_DB':str(db)}
    base=f'http://127.0.0.1:{port}/api/'
    def request(route,body=None):
        req=urllib.request.Request(base+route,None if body is None else json.dumps(body).encode(),{'Authorization':'Bearer '+token,'Content-Type':'application/json'})
        with urllib.request.urlopen(req,timeout=20) as response:return response.read()
    def start():
        p=subprocess.Popen([binary,'controller'],env=env,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
        for _ in range(100):
            try: request('workspace',{}); return p
            except OSError:time.sleep(.05)
        p.terminate();p.wait(timeout=5);raise RuntimeError('startup failed')
    p=start();p.terminate();p.wait(timeout=5)
    with sqlite3.connect(db) as connection:
        connection.execute("INSERT INTO projects VALUES('p','example','repo')")
        for n in range(80):connection.execute("INSERT INTO jobs(id,project_id,prompt,runner,state,created) VALUES(?,'p',?,'codex','queued',0)",(f'root-{n}',f'Task {n} '+('x'*2000)))
        for n in range(400):
            connection.execute("INSERT INTO jobs(id,project_id,prompt,runner,state,created,parent_id) VALUES(?,'p',?,'codex','succeeded',0,?)",(f'turn-{n}',f'Turn {n} '+('x'*90_000),None if n==0 else f'turn-{n-1}'))
            connection.execute("INSERT INTO job_output_chunks(job_id,body) VALUES(?,?)",(f'turn-{n}','x'*63_999+'\n'))
    print(json.dumps({'fixture':{'independent_conversations':81,'total_jobs':480,'long_conversation_turns':400,'prompt_bytes_per_turn':90_000,'output_bytes_per_turn':64_000,'database_bytes':db.stat().st_size}}))
    for mode in ('paged','compatibility_snapshot'):
        p=start();done=threading.Event();samples=[]
        def sample():
            while not done.is_set():
                result=subprocess.run(['ps','-o','rss=','-p',str(p.pid)],text=True,capture_output=True)
                if result.returncode==0 and result.stdout.strip():samples.append(int(result.stdout.strip()))
                done.wait(.05)
        sampler=threading.Thread(target=sample);sampler.start();times=[];sizes=[]
        try:
            for _ in range(20):
                started=time.monotonic()
                if mode=='paged':
                    body=request('workspace',{});sizes.append(len(body));assert len(json.loads(body)['jobs'])==25
                    turns=json.loads(request('conversation-page',{'id':'turn-0'}));assert len(turns['jobs'])==5
                    for job in turns['jobs']:request('output-page',{'id':job['id']})
                else:
                    try: body=request('state')
                    except urllib.error.HTTPError as error:
                        assert error.code==400 and b'/api/workspace' in error.read()
                        print(json.dumps({'mode':mode,'oversized_snapshot_rejected':True}));break
                    raise RuntimeError('Compatibility snapshot unexpectedly accepted the oversized history')
                times.append(time.monotonic()-started)
                time.sleep(.05)
            if times: print(json.dumps({'mode':mode,'iterations':20,'median_request_cycle_ms':round(statistics.median(times)*1000,1),'workspace_response_bytes':max(sizes),'peak_sampled_controller_rss_kib':max(samples,default=0),'rss_samples':len(samples)}))
        finally:
            done.set();sampler.join();p.terminate();p.wait(timeout=5)
