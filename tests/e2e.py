"""Local controller/worker smoke test; run after cargo build."""
import os, subprocess, tempfile, pathlib, socket, urllib.request, urllib.error, json, time, shutil, uuid, sqlite3, hashlib
os.environ['GIT_CONFIG_GLOBAL']=os.devnull
os.environ['GIT_CONFIG_NOSYSTEM']='1'
binary=os.environ.get('XENIA_TEST_BINARY','target/debug/xenia')
root=pathlib.Path(tempfile.mkdtemp(prefix='xenia-test-')); repo=root/'projects'/'example'/'repo'; repo.mkdir(parents=True); bindir=root/'bin'; bindir.mkdir()
subprocess.run(['git','init','-q',str(repo)],check=True); subprocess.run(['git','-C',str(repo),'-c','user.name=Test','-c','user.email=test@example.invalid','commit','--allow-empty','-qm','initial'],check=True)
runner=bindir/'codex'; runner.write_text('#!/bin/sh\nprompt=$(cat)\ncase "$*" in *prompt-marker*) echo prompt_in_argv; exit 1;; esac\nif [ -n "${XENIA_WORKER_TOKEN-}" ] || [ -n "${GIT_DIR-}" ] || [ -n "${GIT_INDEX_FILE-}" ]; then echo credential_leaked; fi\ncase "$prompt" in digest:*) printf "%s" "$prompt" | python3 -c \"import hashlib,sys; print(hashlib.sha256(sys.stdin.buffer.read()).hexdigest())\" ;; esac\ncase "$prompt" in *large*) python3 -c \"import sys; sys.stdout.write(chr(1)*180000 + chr(10))\" ;; *denial*) echo \'{"type":"system","subtype":"permission_denied","tool_name":"Bash"}\' ;; *parallel-a*) printf a > result.txt ;; *parallel-b*) printf b > result.txt ;; esac\necho \'{"type":"thread.started","thread_id":"synthetic-session"}\'\necho \'{"type":"item.completed","item":{"type":"agent_message","text":"early"}}\'\ncase "$prompt" in *hold*) sleep 20 ;; *parallel*) sleep 10 ;; *) sleep 4 ;; esac\necho \'{"type":"item.completed","item":{"type":"agent_message","text":"done"}}\'\n'); runner.chmod(0o755)
sock=socket.socket();sock.bind(('127.0.0.1',0));port=sock.getsockname()[1];sock.close(); url=f'http://127.0.0.1:{port}';token='synthetic-test-token-1234567890'; env=os.environ|{'XENIA_TOKEN':token,'XENIA_LISTEN':f'127.0.0.1:{port}','XENIA_DB':str(root/'db')};procs=[]
def req(path, body=None):
 data=None if body is None else json.dumps(body).encode(); r=urllib.request.Request(url+'/api/'+path,data,{'Authorization':'Bearer '+token,'Content-Type':'application/json'}); return json.load(urllib.request.urlopen(r,timeout=3))
def until(test,sec=12):
 end=time.time()+sec
 while time.time()<end:
  try:
   v=test()
   if v:return v
  except Exception: pass
  time.sleep(.2)
 raise RuntimeError('timed out')
try:
 procs.append(subprocess.Popen([binary,'controller'],env=env,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)); until(lambda:req('state'))
 slow=socket.create_connection(('127.0.0.1',port),timeout=3)
 slow.sendall((f'POST /api/projects HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: 100\r\nConnection: close\r\n\r\n{{').encode());time.sleep(.3)
 try:assert 'projects' in req('state');print('a stalled request does not block other clients: PASS')
 finally:slow.close()
 worker_token=req('workers/enroll',{'id':'test'})['token']
 def expect_status(auth,path,body,status):
  data=json.dumps(body).encode();request=urllib.request.Request(url+'/api/'+path,data,{'Authorization':'Bearer '+auth,'Content-Type':'application/json'})
  try:urllib.request.urlopen(request,timeout=3);raise AssertionError('request unexpectedly succeeded')
  except urllib.error.HTTPError as e:assert e.code==status,(e.code,status)
 expect_status(worker_token,'projects',{'name':'evil','repository':'repo'},401)
 expect_status(worker_token,'heartbeat',{'id':'other','name':'other','capacity':1,'runners':[],'instance_id':'other'},403)
 expect_status(token,'heartbeat',{'id':'test','name':'test','capacity':1,'runners':[],'instance_id':'other'},401)
 print('worker credential scope: PASS')
 def worker_req(path, body):
  data=json.dumps(body).encode(); request=urllib.request.Request(url+'/api/'+path,data,{'Authorization':'Bearer '+worker_token,'Content-Type':'application/json'}); return json.load(urllib.request.urlopen(request,timeout=3))
 worker_req('heartbeat',{'id':'test','name':'test','capacity':1,'runners':['codex'],'instance_id':'before-start'})
 expect_status(worker_token,'heartbeat',{'id':'test','name':'duplicate','capacity':1,'runners':['codex'],'instance_id':'duplicate'},400)
 project=req('projects',{'name':'example','repository':'repo'})['id'];lost_job=req('jobs',{'project_id':project,'runner':'codex','prompt':'claim recovery'})['id'];claim={'id':'test','instance_id':'before-start','eligible':[project],'claim_id':str(uuid.uuid4())}
 payload=json.dumps(claim).encode(); connection=socket.create_connection(('127.0.0.1',port),timeout=3)
 connection.sendall((f'POST /api/claim HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {worker_token}\r\nContent-Type: application/json\r\nContent-Length: {len(payload)}\r\nConnection: close\r\n\r\n').encode()+payload)
 # Headers are sent after the durable claim commits. Drop the connection before
 # consuming its assignment body; an immediate close can discard the request
 # before the server ever handles it, particularly with a fast release binary.
 headers=b''
 while not headers.endswith(b'\r\n\r\n'):
  byte=connection.recv(1)
  if not byte: raise AssertionError('claim response closed before headers')
  headers+=byte
 assert b' 200 ' in headers.split(b'\r\n',1)[0], headers
 connection.close()
 until(lambda:next((j for j in req('state')['jobs'] if j['id']==lost_job and j['state']=='running'),None))
 procs[0].terminate();procs[0].wait(timeout=5)
 procs[0]=subprocess.Popen([binary,'controller'],env=env,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL);until(lambda:req('state'))
 assert worker_req('claim',claim)['id']==lost_job and worker_req('claim',claim)['id']==lost_job
 worker_req('report',{'id':lost_job,'node_id':'test','state':'succeeded'});print('lost claim response survives controller restart: PASS')
 worker_req('retire',{'id':'test','instance_id':'before-start'})
 worker_req('heartbeat',{'id':'test','name':'test','capacity':1,'runners':['codex'],'instance_id':'replacement'})
 expect_status(worker_token,'claim',claim,400)
 worker_req('retire',{'id':'test','instance_id':'replacement'});print('duplicate and stale worker instances are fenced: PASS')
 env=env|{'XENIA_CAPACITY':'2','XENIA_WORKER_TOKEN':worker_token,'XENIA_WORKER_DB':str(root/'worker.db'),'XENIA_STOP_FILE':str(root/'stop'),'GIT_DIR':str(root/'wrong-git-dir'),'GIT_WORK_TREE':str(root/'wrong-worktree'),'GIT_INDEX_FILE':str(root/'wrong-index'),'XENIA_CONTROLLER':url,'XENIA_NODE_ID':'test','XENIA_PROJECTS_ROOT':str(root/'projects'),'PATH':str(bindir)+':'+os.environ['PATH']};procs.append(subprocess.Popen([binary,'worker'],env=env,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL));until(lambda:req('state')['nodes']);req('nodes/capacity',{'id':'test','capacity':1})
 job=req('jobs',{'project_id':project,'runner':'codex','prompt':'prompt-marker first'})['id']
 find=lambda:next(j | {'output':req('outputs',{'ids':[job]})[job]} for j in req('state')['jobs'] if j['id']==job)
 until(lambda:find()['state']=='running' and 'early' in find()['output']); print('streamed output before exit: PASS')
 procs[0].terminate(); procs[0].wait(timeout=5)
 time.sleep(5)
 procs[0]=subprocess.Popen([binary,'controller'],env=env,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
 until(lambda:find()['state']=='succeeded'); j=find(); assert j['output'].count('early')==1 and j['output'].count('done')==1 and 'credential_leaked' not in j['output']; print('offline report replay without duplicates: PASS'); assert j['session_id']=='synthetic-session'; print('session captured: PASS')
 follow=req('followup',{'id':job,'prompt':'follow up'})['id']; followjob=lambda:next(j for j in req('state')['jobs'] if j['id']==follow)
 until(lambda:followjob()['state']=='running'); assert followjob()['worktree']==j['worktree'];req('cancel',{'id':follow}); until(lambda:followjob()['state']=='canceled');print('follow-up worktree and cancellation: PASS')
 path=pathlib.Path(followjob()['worktree']); (path/'untracked.txt').write_text('keep me')
 req('cleanup',{'id':follow});until(lambda:followjob()['cleanup_state']=='failed',20);assert path.exists(); print('dirty worktree preserved: PASS')
 (path/'untracked.txt').unlink();req('cleanup',{'id':follow});until(lambda:followjob()['cleanup_state']=='succeeded',20);assert not path.exists()
 subprocess.run(['git','-C',str(repo),'show-ref','--verify','--quiet','refs/heads/'+j['branch']],check=True);print('clean worktree removed, branch retained: PASS')
 large=req('jobs',{'project_id':project,'runner':'codex','prompt':'large'})['id']; largejob=lambda:next(j for j in req('state')['jobs'] if j['id']==large)
 until(lambda:largejob()['state']=='succeeded',25); large_output=req('outputs',{'ids':[large]})[large];assert large_output.count(chr(1))==180000 and 'done' in large_output;print('escaped output is delivered without blocking the outbox: PASS')
 prompt='digest:'+('x'*80000)+'\nquotes " & % ! $(literal) 🦀'
 promptjob=req('jobs',{'project_id':project,'runner':'codex','prompt':prompt})['id']; promptstate=lambda:next(j for j in req('state')['jobs'] if j['id']==promptjob)
 until(lambda:promptstate()['state'] in ('succeeded','failed'),25)
 assert promptstate()['state']=='succeeded' and hashlib.sha256(prompt.encode()).hexdigest() in req('outputs',{'ids':[promptjob]})[promptjob]
 print('long prompt and shell characters arrive intact on stdin: PASS')
 denial=req('jobs',{'project_id':project,'runner':'codex','prompt':'denial'})['id']; denialjob=lambda:next(j for j in req('state')['jobs'] if j['id']==denial)
 until(lambda:denialjob()['state']=='succeeded',20);assert denialjob()['permission_denied']==1;print('permission denial remains visible after completion: PASS')
 hold=req('jobs',{'project_id':project,'runner':'codex','prompt':'hold'})['id']; holdjob=lambda:next(j for j in req('state')['jobs'] if j['id']==hold)
 until(lambda:holdjob()['state']=='running' and 'early' in req('outputs',{'ids':[hold]})[hold],20)
 second_repo=root/'projects'/'second'/'repo';second_repo.mkdir(parents=True)
 subprocess.run(['git','init','-q',str(second_repo)],check=True);subprocess.run(['git','-C',str(second_repo),'-c','user.name=Test','-c','user.email=test@example.invalid','commit','--allow-empty','-qm','initial'],check=True)
 second_project=req('projects',{'name':'second','repository':'repo'})['id']
 until(lambda:second_project in json.loads(req('state')['nodes'][0]['eligible']),12)
 eligible_job=req('jobs',{'project_id':second_project,'runner':'codex','node_id':'test','prompt':'explicit'})['id'];req('cancel',{'id':eligible_job});print('busy worker refreshes checkout eligibility: PASS')
 with sqlite3.connect(root/'db') as db:db.execute("UPDATE jobs SET state='succeeded' WHERE id=?",(hold,))
 queued=req('jobs',{'project_id':project,'runner':'codex','prompt':'capacity check'})['id'];queuedjob=lambda:next(j for j in req('state')['jobs'] if j['id']==queued)
 time.sleep(5);assert queuedjob()['state']=='queued';req('cancel',{'id':queued})
 with sqlite3.connect(root/'db') as db:db.execute("UPDATE jobs SET state='running' WHERE id=?",(hold,))
 until(lambda:holdjob()['state']=='succeeded',30);print('worker enforces local capacity with stale controller state: PASS')
 req('nodes/capacity',{'id':'test','capacity':2})
 parallel=[req('jobs',{'project_id':project,'runner':'codex','prompt':'parallel-'+suffix})['id'] for suffix in ('a','b')]
 def parallel_jobs():return [next(j for j in req('state')['jobs'] if j['id']==job_id) for job_id in parallel]
 def active_parallel():
  jobs=parallel_jobs();return jobs if all(j['state']=='running' and j['worktree'] for j in jobs) else None
 both=until(active_parallel,20)
 assert both[0]['worktree']!=both[1]['worktree'] and both[0]['branch']!=both[1]['branch']
 until(lambda:all((pathlib.Path(j['worktree'])/'result.txt').exists() for j in both),10)
 assert [(pathlib.Path(j['worktree'])/'result.txt').read_text() for j in both]==['a','b']
 assert not (repo/'result.txt').exists() and not subprocess.check_output(['git','-C',str(repo),'status','--porcelain'],text=True)
 until(lambda:all(j['state']=='succeeded' for j in parallel_jobs()),20);req('nodes/capacity',{'id':'test','capacity':1});print('parallel jobs use isolated worktrees and preserve the main checkout: PASS')
 draining=req('jobs',{'project_id':project,'runner':'codex','prompt':'drain'})['id']; drainjob=lambda:next(j for j in req('state')['jobs'] if j['id']==draining)
 until(lambda:drainjob()['state']=='running'); (root/'stop').touch()
 waiting=req('jobs',{'project_id':project,'runner':'codex','prompt':'after stop'})['id']; waitingjob=lambda:next(j for j in req('state')['jobs'] if j['id']==waiting)
 procs[1].wait(timeout=20);assert drainjob()['state']=='succeeded' and waitingjob()['state']=='queued'
 assert req('state')['nodes'][0]['last_seen']==0
 worker_req('heartbeat',{'id':'test','name':'test','capacity':1,'runners':['codex'],'instance_id':'after-stop'})
 worker_req('retire',{'id':'test','instance_id':'after-stop'});print('graceful stop drains active work and releases the lease: PASS')
finally:
 for p in reversed(procs):p.terminate()
 for p in reversed(procs):p.wait(timeout=5)
 shutil.rmtree(root,ignore_errors=True)
