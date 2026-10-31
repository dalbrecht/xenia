"""Synthetic Claude control integration; run after cargo build."""
import json, os, pathlib, socket, subprocess, tempfile, time, urllib.request, urllib.error
with tempfile.TemporaryDirectory(prefix='xenia-claude-e2e-') as tmp:
 root=pathlib.Path(tmp); repo=root/'projects'/'example'/'repo';repo.mkdir(parents=True)
 subprocess.run(['git','init','-q',str(repo)],check=True);subprocess.run(['git','-C',str(repo),'-c','user.name=Test','-c','user.email=test@example.invalid','commit','--allow-empty','-qm','initial'],check=True)
 bindir=root/'bin';bindir.mkdir();runner=bindir/'claude'
 runner.write_text('''#!/usr/bin/env python3
import os, sys, json, time
mode=""
if any(k.startswith('XENIA_') for k in os.environ): sys.exit(9)
def send(v): print(json.dumps(v),flush=True)
for line in sys.stdin:
 v=json.loads(line)
 if v.get('type')=='control_request': send({'type':'control_response','response':{'request_id':'xenia-initialize','subtype':'success','response':{}}})
 elif v.get('type')=='user':
  mode=v['message']['content']
  send({'type':'system','subtype':'init','session_id':'session-1'})
  send({'type':'system','subtype':'session_state_changed','state':'requires_action'})
  if mode in ('tool','accept'):
   send({'type':'control_request','request_id':'q1','request':{'subtype':'can_use_tool','tool_name':'Write' if mode=='tool' else 'Bash','input':{'command':'printf example'}}})
  else:
   send({'type':'control_request','request_id':'q1','request':{'subtype':'can_use_tool','tool_name':'AskUserQuestion','input':{'questions':[{'question':'Which?','options':[{'label':'A'},{'label':'B'}],'multiSelect':False}]}}})
   if mode == 'withdraw':
    time.sleep(3);send({'type':'control_cancel_request','request_id':'q1'});time.sleep(2)
    send({'type':'result','subtype':'success','is_error':False,'result':'withdrawn'})
    send({'type':'system','subtype':'session_state_changed','state':'idle'})
 elif v.get('type')=='control_response':
  answer=v['response']['response']; ok=(answer.get('behavior')=='deny' if mode=='tool' else (answer=={'behavior':'allow','updatedInput':{'command':'printf example'}} if mode=='accept' else answer.get('updatedInput',{}).get('answers',{}).get('Which?')=='custom'))
  send({'type':'result','subtype':'success' if ok else 'error','is_error':not ok,'result':'question answered'})
  send({'type':'system','subtype':'session_state_changed','state':'idle'})
''');runner.chmod(0o755)
 with socket.socket() as sock:sock.bind(('127.0.0.1',0));port=sock.getsockname()[1]
 binary=os.environ.get('XENIA_TEST_BINARY','target/debug/xenia')
 base=f'http://127.0.0.1:{port}';token='synthetic-claude-control-token-123456'; env=os.environ|{'XENIA_TOKEN':token,'XENIA_LISTEN':f'127.0.0.1:{port}','XENIA_DB':str(root/'db')};procs=[]
 def req(path,body=None):
  r=urllib.request.Request(base+'/api/'+path,data=None if body is None else json.dumps(body).encode(),headers={'Authorization':'Bearer '+token,'Content-Type':'application/json'})
  with urllib.request.urlopen(r,timeout=4) as response:return json.load(response)
 def until(f,seconds=30):
  end=time.monotonic()+seconds
  while time.monotonic()<end:
   try:
    value=f()
    if value:return value
   except (OSError,StopIteration):pass
   time.sleep(.2)
  raise AssertionError('Timed out')
 try:
  procs.append(subprocess.Popen([binary,'controller'],env=env,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL));until(lambda:req('state'))
  credential=req('workers/enroll',{'id':'test'})['token'];worker_env=env|{'XENIA_WORKER_TOKEN':credential,'XENIA_WORKER_DB':str(root/'worker.db'),'XENIA_CONTROLLER':base,'XENIA_NODE_ID':'test','XENIA_PROJECTS_ROOT':str(root/'projects'),'XENIA_CLAUDE_CONTROL':'1','PATH':str(bindir)+os.pathsep+os.environ['PATH']}
  procs.append(subprocess.Popen([binary,'worker'],env=worker_env,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL));until(lambda:req('state')['nodes'])
  project=req('projects',{'name':'example','repository':'repo'})['id'];job=req('jobs',{'project_id':project,'runner':'claude','prompt':'ask'})['id']
  q=until(lambda:req('approval/list',{'id':job}))[0];assert q['kind']=='question'
  procs[0].terminate();procs[0].wait(timeout=5)
  procs[0]=subprocess.Popen([binary,'controller'],env=env,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL);until(lambda:req('state'))
  assert until(lambda:req('approval/list',{'id':job}))[0]['id']==q['id']
  req('approval/answer',{'id':q['id'],'answers':{'Which?':'custom'}})
  done=until(lambda:next((j for j in req('state')['jobs'] if j['id']==job and j['state'] in ('succeeded','failed','unknown')),None))
  assert done['state']=='succeeded',done
  assert done['session_id']=='session-1'
  print('Claude question survives controller restart and completes: PASS')
  tool=req('jobs',{'project_id':project,'runner':'claude','prompt':'tool'})['id']
  approval=until(lambda:req('approval/list',{'id':tool}))[0];assert approval['kind']=='tool' and not approval['can_accept']
  try:req('approval/decide',{'id':approval['id'],'decision':'accept'});raise AssertionError('Unsafe tool accepted')
  except urllib.error.HTTPError as error:assert error.code==400
  req('approval/decide',{'id':approval['id'],'decision':'decline'})
  denied=until(lambda:next((j for j in req('state')['jobs'] if j['id']==tool and j['state'] in ('succeeded','failed','unknown')),None))
  assert denied['state']=='succeeded' and denied['permission_denied']==1,denied
  print('Claude unsupported tool is deny-only and denial is visible: PASS')
  accepted=req('jobs',{'project_id':project,'runner':'claude','prompt':'accept'})['id']
  command=until(lambda:req('approval/list',{'id':accepted}))[0]
  assert command['kind']=='tool' and command['can_accept'] and command['details']['request']['input']['command']=='printf example'
  req('approval/decide',{'id':command['id'],'decision':'accept'})
  approved=until(lambda:next((j for j in req('state')['jobs'] if j['id']==accepted and j['state'] in ('succeeded','failed','unknown')),None))
  assert approved['state']=='succeeded',approved
  print('Claude Bash approval preserves exact input without permission updates: PASS')
  withdrawn=req('jobs',{'project_id':project,'runner':'claude','prompt':'withdraw'})['id']
  old=until(lambda:req('approval/list',{'id':withdrawn}))[0]
  until(lambda:not req('approval/list',{'id':withdrawn}))
  try:req('approval/answer',{'id':old['id'],'answers':{'Which?':'late'}});raise AssertionError('Withdrawn question accepted')
  except urllib.error.HTTPError as error:assert error.code==400
  done=until(lambda:next((j for j in req('state')['jobs'] if j['id']==withdrawn and j['state'] in ('succeeded','failed','unknown')),None))
  assert done['state']=='succeeded',done
  print('Claude withdrawal invalidates the question: PASS')
  canceled=req('jobs',{'project_id':project,'runner':'claude','prompt':'cancel'})['id']
  until(lambda:req('approval/list',{'id':canceled}))
  req('cancel',{'id':canceled})
  stopped=until(lambda:next((j for j in req('state')['jobs'] if j['id']==canceled and j['state'] in ('canceled','failed','unknown')),None))
  assert stopped['state']=='canceled',stopped
  print('Cancellation stops Claude while waiting: PASS')
 finally:
  for p in reversed(procs):
   if p.poll() is None:p.terminate()
  for p in reversed(procs):p.wait(timeout=10)
