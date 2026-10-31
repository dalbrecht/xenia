#!/usr/bin/env python3
"""Opt-in macOS/Unix resource sample using authenticated Codex and Claude CLIs.
Run from the repository after cargo build --release. Makes billable model requests.
Uses disposable repositories; prints resource totals, never writes runner output.
"""
import os,json,pathlib,socket,subprocess,tempfile,time,urllib.request
binary=str(pathlib.Path('target/release/xenia').resolve())
with tempfile.TemporaryDirectory(prefix='xenia-resources-') as tmp:
 root=pathlib.Path(tmp);repo=root/'projects/example/repo';repo.mkdir(parents=True)
 subprocess.run(['git','init','-q',str(repo)],check=True);subprocess.run(['git','-C',str(repo),'-c','user.name=Test','-c','user.email=test@example.invalid','commit','--allow-empty','-qm','initial'],check=True)
 with socket.socket() as s:s.bind(('127.0.0.1',0));port=s.getsockname()[1]
 token='synthetic-resource-check-token';url=f'http://127.0.0.1:{port}';env=os.environ|{'XENIA_TOKEN':token,'XENIA_LISTEN':f'127.0.0.1:{port}','XENIA_DB':str(root/'db')};processes=[]
 def api(path,body=None):
  r=urllib.request.Request(url+'/api/'+path,data=None if body is None else json.dumps(body).encode(),headers={'Authorization':'Bearer '+token,'Content-Type':'application/json'})
  with urllib.request.urlopen(r,timeout=4) as response:return json.load(response)
 def until(f):
  for _ in range(50):
   try:
    v=f()
    if v:return v
   except OSError:pass
   time.sleep(.2)
  raise RuntimeError('startup timeout')
 try:
  controller=subprocess.Popen([binary,'controller'],env=env,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL);processes.append(controller);until(lambda:api('state'))
  credential=api('workers/enroll',{'id':'test'})['token'];worker_env=env|{'XENIA_WORKER_TOKEN':credential,'XENIA_WORKER_DB':str(root/'worker.db'),'XENIA_CONTROLLER':url,'XENIA_NODE_ID':'test','XENIA_PROJECTS_ROOT':str(root/'projects'),'XENIA_CAPACITY':'2'}
  worker=subprocess.Popen([binary,'worker'],env=worker_env,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL);processes.append(worker);until(lambda:api('state')['nodes'])
  project=api('projects',{'name':'example','repository':'repo'})['id']
  for runner in ('codex','claude'):api('jobs',{'project_id':project,'runner':runner,'prompt':'Resource measurement setup check. Do not use tools or read or modify files. Reply with exactly XENIA_RESOURCE_OK.'})
  peaks={'controller':0,'worker engine':0,'worker descendants combined':0};samples=0;overlap=False;deadline=time.monotonic()+120
  while time.monotonic()<deadline:
   rows={}
   for line in subprocess.check_output(['ps','-axo','pid=,ppid=,rss='],text=True).splitlines():
    parts=line.split()
    if len(parts)==3:rows[int(parts[0])]=(int(parts[1]),int(parts[2]))
   descendants={worker.pid}
   while True:
    new={pid for pid,(parent,rss) in rows.items() if parent in descendants}-descendants
    if not new:break
    descendants.update(new)
   peaks['controller']=max(peaks['controller'],rows.get(controller.pid,(0,0))[1]);peaks['worker engine']=max(peaks['worker engine'],rows.get(worker.pid,(0,0))[1]);peaks['worker descendants combined']=max(peaks['worker descendants combined'],sum(rows[p][1] for p in descendants if p!=worker.pid and p in rows));samples+=1
   jobs=api('state')['jobs'];overlap|=sum(j['state']=='running' for j in jobs)==2
   if all(j['state'] in ('succeeded','failed','unknown','canceled') for j in jobs):
    print('terminal outcomes:',[(j['runner'],j['state'],j.get('error')) for j in jobs]);
    if any(j['state']!='succeeded' for j in jobs):raise RuntimeError('A runner failed; measurement is not a successful-workload baseline')
    break
   time.sleep(.2)
  else:raise RuntimeError('measurement timed out')
  print('samples:',samples,'two jobs running observed:',overlap,'peak RSS KiB:',json.dumps(peaks))
 finally:
  for p in reversed(processes):
   if p.poll() is None:p.terminate()
  for p in reversed(processes):p.wait(timeout=5)
