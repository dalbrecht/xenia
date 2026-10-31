"""Assignment identity, capacity, replay, restart and environment snapshot rules."""
import contextlib,json,os,pathlib,socket,sqlite3,subprocess,tempfile,time,urllib.error,urllib.request,uuid
with tempfile.TemporaryDirectory(prefix='xenia-assignment-api-') as tmp:
 root=pathlib.Path(tmp);db=root/'db'
 with socket.socket() as sock:sock.bind(('127.0.0.1',0));port=sock.getsockname()[1]
 admin='synthetic-assignment-api-admin';base=f'http://127.0.0.1:{port}/api/'
 binary=os.environ.get('XENIA_TEST_BINARY','target/debug/xenia.exe' if os.name=='nt' else 'target/debug/xenia')
 p=subprocess.Popen([binary,'controller'],env=os.environ|{'XENIA_TOKEN':admin,'XENIA_DB':str(db),'XENIA_LISTEN':f'127.0.0.1:{port}'},stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
 def api(route,body={},token=admin,status=200):
  r=urllib.request.Request(base+route,data=json.dumps(body).encode(),headers={'Authorization':'Bearer '+token,'Content-Type':'application/json'})
  try:
   with urllib.request.urlopen(r,timeout=5) as reply:assert reply.status==status;return json.load(reply)
  except urllib.error.HTTPError as e:assert e.code==status,(route,e.code,e.read().decode());return None
 def state():return api('workspace',{'mode':'projects'})['assignments'][0]
 try:
  for _ in range(100):
   try:api('workspace');break
   except OSError:time.sleep(.05)
  one=api('workers/enroll',{'id':'one'})['token'];two=api('workers/enroll',{'id':'two'})['token']
  policy={'project_checkout':True,'setup_scripts':True,'containers':True,'runners':['codex'],'projects':[],'max_workers':1,'platform':'windows'}
  def heartbeat(node,credential,instance):return api('heartbeat',{'id':node,'name':node,'instance_id':instance,'capacity':1,'runners':['codex'],'policy':policy},credential)
  heartbeat('one',one,'first');heartbeat('two',two,'second')
  project=api('projects',{'name':'example','repository':'repo'})['id']
  config={'project_id':project,'source':'https://repo.example/repo.git','ref':'HEAD','mode':'native'}
  api('projects/environment',config);api('projects/assign',{'project_id':project,'node_id':'one'})
  job=api('jobs',{'project_id':project,'runner':'codex','prompt':'queued until setup'})['id']
  def jobclaim():return api('claim',{'id':'one','instance_id':'first','claim_id':str(uuid.uuid4()),'eligible':[project]},one)
  assert jobclaim() is None
  claim=str(uuid.uuid4());body={'id':'one','instance_id':'first','claim_id':claim}
  item=api('provision/claim',body,one);assert api('provision/claim',body,one)==item
  assert api('provision/claim',body|{'claim_id':str(uuid.uuid4())},one) is None
  assert jobclaim() is None
  api('projects/unassign',{'project_id':project,'node_id':'one'},status=400)
  api('projects/environment',config|{'ref':'main'},status=400)
  report={'id':item['id'],'node_id':'one','instance_id':'first','claim_id':claim,'state':'preparing','step':'Cloning','event_id':str(uuid.uuid4()),'log':'fixture output'}
  api('provision/report',report|{'node_id':'two','instance_id':'second'},two,status=400)
  api('provision/report',report|{'log':'x'*16385},one,status=400)
  api('provision/report',report,one);api('provision/report',report,one)
  api('provision/report',report|{'event_id':str(uuid.uuid4()),'state':'ready'},one,status=400)
  ready=report|{'event_id':str(uuid.uuid4()),'state':'ready','revision':'a'*40,'runners':['codex'],'log':''}
  api('provision/report',ready,one);api('provision/report',ready,one);assert state()['state']=='ready' and state()['log']=='fixture output'
  chosen=jobclaim();assert chosen['id']==job and chosen['environment']['revision']=='a'*40
  api('report',{'id':job,'node_id':'one','state':'succeeded','session_id':'session','worktree':'/tmp/synthetic-assignment-worktree','branch':'xenia/test'},one)
  api('projects/environment',config|{'ref':'main'});assert state()['state']=='needs_setup'
  api('projects/assign',{'project_id':project,'node_id':'one'})
  claim2=str(uuid.uuid4());item2=api('provision/claim',body|{'claim_id':claim2},one)
  api('provision/report',ready|{'claim_id':claim2,'event_id':str(uuid.uuid4()),'revision':'b'*40},one)
  follow=api('followup',{'id':job,'prompt':'resume old environment'})['id'];chosen=jobclaim();assert chosen['id']==follow and chosen['environment']['revision']=='a'*40
  api('report',{'id':follow,'node_id':'one','state':'succeeded'},one)
  api('projects/assign',{'project_id':project,'node_id':'one'})
  claim3=str(uuid.uuid4());api('provision/claim',body|{'claim_id':claim3},one)
  with contextlib.closing(sqlite3.connect(db)) as c,c:c.execute("UPDATE nodes SET last_seen=0 WHERE id='one'")
  heartbeat('one',one,'restarted');assert state()['state']=='unknown'
  stale=ready|{'claim_id':claim3,'event_id':str(uuid.uuid4())}
  assert api('provision/report',stale,one)['ignored']=='stale setup attempt';assert state()['state']=='unknown'
  api('projects/assign',{'project_id':project,'node_id':'one'},status=400)
  api('projects/assign',{'project_id':project,'node_id':'one','process_stopped':True})
  claim4=str(uuid.uuid4());api('provision/claim',body|{'instance_id':'restarted','claim_id':claim4},one)
  api('provision/report',stale|{'event_id':str(uuid.uuid4())},one);assert state()['state']=='preparing'
  api('provision/report',report|{'instance_id':'restarted','claim_id':claim4,'state':'failed','error':'synthetic setup failure','event_id':str(uuid.uuid4())},one)
  api('projects/unassign',{'project_id':project,'node_id':'one'});assert state()['state']=='removed'
  print('Assignment scope, capacity, claim replay, report replay, readiness, environment snapshots, restart uncertainty and stale-outbox acknowledgement: PASS')
 finally:p.terminate();p.wait(timeout=5)
