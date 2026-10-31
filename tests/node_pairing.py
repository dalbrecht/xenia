"""Node-owned credential approval, scoping, expiry, retry and bounded requests."""
import hashlib, json, os, pathlib, secrets, socket, sqlite3, subprocess, tempfile, time, urllib.error, urllib.request
with tempfile.TemporaryDirectory(prefix='xenia-node-pair-') as tmp:
    root=pathlib.Path(tmp); db=root/'controller.db'
    with socket.socket() as sock: sock.bind(('127.0.0.1',0)); port=sock.getsockname()[1]
    admin='synthetic-enrollment-admin'; url=f'http://127.0.0.1:{port}'
    binary=str(pathlib.Path(os.environ.get('XENIA_TEST_BINARY','target/debug/xenia')).resolve())
    env=os.environ|{'XENIA_DB':str(db),'XENIA_TOKEN':admin,'XENIA_LISTEN':f'127.0.0.1:{port}'}
    def api(route,body=None,token=None,status=200):
        request=urllib.request.Request(url+'/api/'+route,data=json.dumps(body or {}).encode(),headers={'Authorization':'Bearer '+(admin if token is None else token),'Content-Type':'application/json'})
        try:
            with urllib.request.urlopen(request,timeout=5) as response:
                assert status==response.status,(route,response.status,status)
                assert response.headers['Cache-Control']=='no-store'
                return json.load(response)
        except urllib.error.HTTPError as e: assert e.code==status,(route,e.code,status)
    def sql(query,args=()):
        with sqlite3.connect(db) as c: return c.execute(query,args).fetchall()
    def start():
        p=subprocess.Popen([binary,'controller'],env=env,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
        for _ in range(100):
            try: api('workspace'); return p
            except OSError: time.sleep(.05)
        raise AssertionError('Controller did not start')
    def request(node='test',secret=None):
        secret=secret or secrets.token_hex(32)
        body={'node_id':node,'name':'Example worker','platform':'windows','proof':hashlib.sha256(secret.encode()).hexdigest()}
        return api('node-pairing/request',body,token=''),secret
    def decide(r,decision='approved',status=200): return api('node-pairing/decide',{'id':r['id'],'code':r['code'],'decision':decision},status=status)
    def poll(r,secret,status=200): return api('node-pairing/poll',{'id':r['id']},token=secret,status=status)
    p=start();child=None
    try:
        r,secret=request(); assert 295<r['expires']-time.time()<=300 and len(r['code'])==8
        assert poll(r,secret)['state']=='pending'
        api('node-pairing/request',{'node_id':'duplicate-proof','name':'Other','platform':'windows','proof':hashlib.sha256(secret.encode()).hexdigest()},token='',status=400)
        assert not sql('SELECT * FROM worker_credentials')
        api('node-pairing/list',token='',status=401)
        api('node-pairing/decide',{'id':r['id'],'code':r['code'],'decision':'approved'},token=secret,status=401)
        poll(r,secrets.token_hex(32),400)
        api('node-pairing/decide',{'id':r['id'],'code':'wrong','decision':'approved'},status=400)
        assert secret not in json.dumps(api('node-pairing/list'))
        assert api('workspace')['enrollments'][0]['id']==r['id']
        competitor,_=request(); decide(r);decide(competitor,status=400)
        assert not sql('SELECT * FROM worker_credentials'), 'Approval must await proof of possession'
        p.terminate();p.wait(timeout=5);p=start()
        assert poll(r,secret)['state']=='completed' and poll(r,secret)['state']=='completed'
        assert sql('SELECT digest FROM worker_credentials')[0][0]==hashlib.sha256(secret.encode()).hexdigest()
        assert secret.encode() not in db.read_bytes()
        decide(r,status=400)
        api('node-pairing/cancel',{'id':r['id']},secret,status=400)
        api('node-pairing/list',token=secret,status=401)
        api('workspace',token=secret,status=401)
        heartbeat={'id':'test','name':'Example','capacity':1,'runners':[],'instance_id':'instance'}
        api('heartbeat',heartbeat,secret)
        api('heartbeat',heartbeat|{'id':'other'},secret,status=403)
        api('workers/revoke',{'id':'test'});poll(r,secret,400);api('heartbeat',heartbeat,secret,status=401)
        expired,proof=request('expired');decide(expired);sql('UPDATE node_enrollments SET expires=0 WHERE id=?',(expired['id'],));decide(expired,status=400);poll(expired,proof,400)
        denied,proof=request('denied');decide(denied,'rejected');assert poll(denied,proof)['state']=='rejected';decide(denied,status=400)
        cancelled,proof=request('cancelled');decide(cancelled);api('node-pairing/cancel',{'id':cancelled['id']},proof);decide(cancelled,status=400)
        # Drive the same JSON-lines protocol consumed by both desktop apps.
        stop=root/'pair-stop';pairenv=env|{'XENIA_CONTROLLER':url,'XENIA_NODE_ID':'cli','XENIA_PAIR_STOP_FILE':str(stop)}
        child=subprocess.Popen([binary,'pair'],env=pairenv,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)
        waiting=json.loads(child.stdout.readline());decide(waiting)
        completed=json.loads(child.stdout.readline());assert completed['state']=='completed' and completed['token']!=admin
        child.wait(timeout=10);assert child.returncode==0
        api('heartbeat',heartbeat|{'id':'cli'},completed['token'])
        api('workers/revoke',{'id':'cli'})
        child=subprocess.Popen([binary,'pair'],env=pairenv|{'XENIA_NODE_ID':'cancel-cli'},stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)
        waiting=json.loads(child.stdout.readline());stop.touch();child.communicate(timeout=20);assert child.returncode!=0
        assert not any(x['id']==waiting['id'] for x in api('node-pairing/list'))
        # Root rotation invalidates unfinished requests, without enrolling them.
        stale,proof=request('stale');decide(stale);p.terminate();p.wait(timeout=5);env['XENIA_TOKEN']='synthetic-rotated-enrollment-admin';admin=env['XENIA_TOKEN'];p=start();poll(stale,proof,400)
        for i in range(32): request('bounded-'+str(i))
        api('node-pairing/request',{'node_id':'overflow','name':'Example','platform':'windows','proof':secrets.token_hex(32)},token='',status=400)
        assert len(api('node-pairing/list'))==32
        print('Node pairing: approval, proof possession, restart, scope, revoke, expiry, denial, cancellation, rotation and bounds: PASS')
    finally:
        if child and child.poll() is None:child.kill();child.wait(timeout=5)
        p.terminate();p.wait(timeout=5)
