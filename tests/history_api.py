import contextlib
"""Bounded history APIs, legacy migration and paging invariants; no real runners."""
import json, os, pathlib, socket, sqlite3, subprocess, tempfile, time, urllib.error, urllib.request

with tempfile.TemporaryDirectory(prefix='xenia-history-') as tmp:
    root = pathlib.Path(tmp); db = root/'db'
    with socket.socket() as sock:
        sock.bind(('127.0.0.1',0)); port=sock.getsockname()[1]
    token='synthetic-history-test-token'; base=f'http://127.0.0.1:{port}/api/'
    env=os.environ|{'XENIA_TOKEN':token,'XENIA_LISTEN':f'127.0.0.1:{port}','XENIA_DB':str(db)}
    binary=os.environ.get('XENIA_TEST_BINARY','target/debug/xenia')
    def api(route, body=None, expected=200):
        request=urllib.request.Request(base+route,None if body is None else json.dumps(body).encode(),{'Authorization':'Bearer '+token,'Content-Type':'application/json'})
        try:
            with urllib.request.urlopen(request,timeout=5) as r:
                data=r.read(); assert r.status==expected; assert len(data)<1_000_000 or route=='state', (route,len(data))
                return json.loads(data)
        except urllib.error.HTTPError as error:
            assert error.code==expected, (route,error.code,error.read().decode())
    def start():
        p=subprocess.Popen([binary,'controller'],env=env,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
        for _ in range(100):
            try:
                api('workspace',{}); return p
            except OSError:
                time.sleep(.05)
        p.terminate();p.wait(timeout=5);raise AssertionError('startup failed')
    p=start()
    try:
        p.terminate();p.wait(timeout=5)
        with contextlib.closing(sqlite3.connect(db)) as con, con:
            # Reconstruct a database predating the history index, retaining output migration.
            con.executescript("DROP TRIGGER history_job_insert; DROP TABLE history_heads; DROP TABLE history_members; DELETE FROM schema_migrations WHERE name='history_pages_v1'; INSERT INTO projects VALUES('p','example','repo'); INSERT INTO projects VALUES('q','another','app');")
            for n in range(63):
                con.execute("INSERT INTO jobs(id,project_id,prompt,runner,state,created) VALUES(?,? ,?,'codex','succeeded',0)",(f'root-{n}','p' if n%2==0 else 'q',f'Task {n} '+('x'*2000)))
            for n in range(14):
                con.execute("INSERT INTO jobs(id,project_id,prompt,runner,node_id,state,created,parent_id,session_id,worktree,branch) VALUES(?,'p',?,'codex','worker','succeeded',0,?,'session','/tmp/synthetic-worktree','xenia/test')",(f'turn-{n}',f'Turn {n} '+('x'*90_000),None if n==0 else f'turn-{n-1}'))
                con.execute("INSERT INTO job_output_chunks(job_id,body) VALUES(?,?)",(f'turn-{n}',f'Output {n}\n'))
            con.execute("INSERT INTO nodes(id,name,capacity,last_seen,runners,eligible) VALUES(?,?,?,?,?,?)",('other','Other',2,0,'["codex"]','["q"]'))
            con.execute("UPDATE jobs SET node_id='other',state='unknown' WHERE id='root-1'")
        p=start()
        api('state',expected=400)
        page=api('workspace',{}); assert page['total']==64 and len(page['jobs'])==25 and page['has_more']
        assert page['nodes'][0]['occupied']==1
        latest=page['jobs'][0]; assert latest['id']=='turn-13' and latest['conversation_id']=='turn-0' and latest['turn_count']==14
        assert len(latest['prompt'])==91 and latest['conversation_prompt'].startswith('Turn 0')
        seen=[]; cursor=None
        while True:
            page=api('workspace',{'before':cursor});seen += [j['conversation_id'] for j in page['jobs']]
            if not page['has_more']:break
            cursor=page['next_before']
        assert len(seen)==len(set(seen))==64
        for mode,count in [('conversations',64),('queue',77)]:
            page=api('workspace',{'mode':mode});assert page['total']==count
            filtered=api('workspace',{'mode':mode,'project_id':'q'});assert filtered['total']==31 and all(j['project_id']=='q' for j in filtered['jobs'])
        page=api('workspace',{'node_id':'worker'});assert page['total']==1 and page['jobs'][0]['turn_count']==14
        pinned=api('workspace',{'before':30,'selected':'turn-0'});assert pinned['pinned']['conversation_id']=='turn-0' and len(pinned['jobs'])<=25
        assert api('workspace',{'before':30,'selected':'turn-0','project_id':'q'}).get('pinned') is None
        turns=[];cursor=None
        while True:
            page=api('conversation-page',{'id':'turn-0','before':cursor});assert len(page['jobs'])<=5
            assert all(len(j['prompt'])>90_000 for j in page['jobs']); turns = [j['id'] for j in page['jobs']] + turns
            if not page['has_more']:break
            cursor=page['next_before']
        assert turns==[f'turn-{n}' for n in range(14)]
        one=api('conversation-page',{'id':'turn-2','single':True});assert [j['id'] for j in one['jobs']]==['turn-2'] and one['jobs'][0]['has_followup']==1
        for invalid in [-1,0,'bad',1.5]: api('workspace',{'before':invalid},400)
        api('workspace',{'project_id':[]},400); api('workspace',{'mode':'invalid'},400)
        # Normal API writes maintain the index atomically, including follow-ups.
        follow=api('followup',{'id':'turn-13','prompt':'A fresh follow-up'})['id']
        latest=api('workspace',{})['jobs'][0];assert latest['id']==follow and latest['turn_count']==15 and latest['conversation_id']=='turn-0'
        p.terminate();p.wait(timeout=5);p=start()
        assert api('workspace',{})['jobs'][0]['turn_count']==15
        assert api('workspace',{'mode':'nodes'})['jobs']==[]
        print('Legacy history migration, summary bounds, conversation/queue filters, full turn paging, cursors, atomic follow-ups and restart: PASS')
    finally:
        p.terminate();p.wait(timeout=5)
