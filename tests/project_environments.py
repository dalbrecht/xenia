"""Project assignment, native/container/hybrid setup and local policy boundaries.

Default: native, no provider/network requests. XENIA_TEST_DOCKER=1 additionally
builds a disposable fixture image and exercises real Docker and Compose.
"""
import contextlib, json, os, pathlib, shutil, socket, sqlite3, subprocess, sys, tempfile, time, urllib.error, urllib.request, uuid
binary=str(pathlib.Path(os.environ.get('XENIA_TEST_BINARY','target/debug/xenia.exe' if os.name=='nt' else 'target/debug/xenia')).resolve())
docker=os.environ.get('XENIA_TEST_DOCKER')=='1'
with tempfile.TemporaryDirectory(prefix='xenia-project-env-',dir=os.environ.get('XENIA_TEST_ROOT')) as tmp:
    root=pathlib.Path(tmp);source=root/'source';source.mkdir();(source/'.xenia').mkdir()
    (source/'README.md').write_text('Synthetic source\n')
    (source/'.gitignore').write_text('.prepared\n')
    (source/'.xenia/setup.sh').write_text('set -eu\nif [ -f .prepared ]; then echo setup-reran; exit 1; fi\nprintf prepared > .prepared\n')
    (source/'.xenia/setup.ps1').write_text("$ErrorActionPreference='Stop'\nif(Test-Path .prepared){throw 'setup-reran'}\nSet-Content .prepared prepared\n")
    runner='''import json,pathlib,subprocess,sys,time,os
if '--version' in sys.argv: print('synthetic-codex-1');sys.exit(0)
prompt=sys.stdin.read()
assert not any(k.startswith('XENIA_') for k in os.environ), 'Xenia credentials leaked to runner'
assert pathlib.Path('.prepared').exists(), 'setup did not run in job worktree'
subprocess.run(['git','status','--porcelain'],check=True,stdout=subprocess.DEVNULL)
print(json.dumps({'type':'thread.started','thread_id':'fixture-session'}),flush=True)
if prompt=='cancel':
 print('waiting-for-cancel',flush=True);time.sleep(90)
else:
 p=pathlib.Path('result.txt')
 if 'resume' in sys.argv: assert p.read_text()=='initial'
 p.write_text('followup' if 'resume' in sys.argv else 'initial')
 subprocess.run(['git','add','result.txt'],check=True)
 subprocess.run(['git','-c','user.name=Fixture','-c','user.email=fixture@example.invalid','commit','-qm','fixture turn'],check=True)
 print(json.dumps({'type':'item.completed','item':{'type':'agent_message','text':'PROJECT_ENV_OK'}}),flush=True)
'''
    bindir=root/'bin';bindir.mkdir();fake=bindir/'runner.py';fake.write_text(runner)
    if os.name=='nt':(bindir/'codex.cmd').write_text(f'@"{sys.executable}" "{fake}" %*\r\n')
    else:
        f=bindir/'codex';f.write_text(f'#!{sys.executable}\n'+runner);f.chmod(0o755)
    image='xenia-fixture-'+uuid.uuid4().hex[:12]
    if docker:
        context=root/'image';context.mkdir();(context/'codex').write_text('#!/usr/bin/env python3\n'+runner);(context/'codex').chmod(0o755)
        (context/'Dockerfile').write_text('FROM python:3.12-alpine\nRUN apk add --no-cache git\nCOPY codex /usr/local/bin/codex\n')
        subprocess.run(['docker','build','-q','-t',image,str(context)],check=True)
        (source/'.xenia/compose.yaml').write_text(f'services:\n  dependency:\n    image: {image}\n    command: ["sh", "-c", "while :; do sleep 3600; done"]\n')
    def git(*args):return subprocess.check_output(['git','-C',str(source),*args],text=True).strip()
    git('init','-q');git('add','.');git('-c','user.name=Fixture','-c','user.email=fixture@example.invalid','commit','-qm','initial');commit=git('rev-parse','HEAD')
    # Git URL rewrite confines these fixtures to disk while the public API keeps
    # rejecting file://, embedded credentials and plain remote HTTP.
    gitconfig=root/'gitconfig';subprocess.run(['git','config','--file',str(gitconfig),'url.'+source.as_uri()+'.insteadOf','https://repo.example/fixture.git'],check=True)
    with socket.socket() as sock:sock.bind(('127.0.0.1',0));port=sock.getsockname()[1]
    db=root/'controller.db';url=f'http://127.0.0.1:{port}/api/';admin='synthetic-project-environment-admin'
    env=os.environ|{'XENIA_DB':str(db),'XENIA_TOKEN':admin,'XENIA_LISTEN':f'127.0.0.1:{port}','GIT_CONFIG_GLOBAL':str(gitconfig),'GIT_CONFIG_NOSYSTEM':'1','PATH':str(bindir)+os.pathsep+os.environ['PATH']}
    processes=[];workers=[];containers=[]
    def api(route,body=None,token=admin,status=200):
        r=urllib.request.Request(url+route,data=None if body is None else json.dumps(body).encode(),headers={'Authorization':'Bearer '+token,'Content-Type':'application/json'})
        try:
            with urllib.request.urlopen(r,timeout=5) as reply:assert reply.status==status;return json.load(reply)
        except urllib.error.HTTPError as e:
            error=e.read().decode();assert e.code==status,(route,e.code,error,status);return {'error':error}
    def until(fn,seconds=90):
        end=time.monotonic()+seconds
        while time.monotonic()<end:
            try:
                v=fn()
                if v:return v
            except OSError:pass
            time.sleep(.2)
        raise AssertionError('Timed out waiting for fixture')
    def assignments():return api('workspace',{'mode':'projects'})['assignments']
    def state(job):return next(j for j in api('state')['jobs'] if j['id']==job)
    def start_worker(node,allow=False,containers=False):
        token=api('workers/enroll',{'id':node})['token'];projects=root/node;projects.mkdir()
        stop=root/(node+'-stop')
        log=(root/(node+'.log')).open('w')
        p=subprocess.Popen([binary,'worker'],env=env|{'XENIA_CONTROLLER':url.removesuffix('/api/'),'XENIA_NODE_ID':node,'XENIA_WORKER_TOKEN':token,'XENIA_PROJECTS_ROOT':str(projects),'XENIA_WORKER_DB':str(root/(node+'.db')),'XENIA_STOP_FILE':str(stop),'XENIA_ALLOW_PROJECT_CHECKOUT':str(int(allow)),'XENIA_ALLOW_SETUP_SCRIPTS':str(int(allow)),'XENIA_ALLOW_CONTAINERS':str(int(containers)),'XENIA_CAPACITY':'1'},stdout=log,stderr=log)
        workers.append((p,stop,log));until(lambda:any(n['id']==node for n in api('workspace',{})['nodes']));return token,projects
    def config(project,mode='native',**extra):return {'project_id':project,'source':'https://repo.example/fixture.git','ref':'HEAD','mode':mode,'windows_setup':'.xenia/setup.ps1','unix_setup':'.xenia/setup.sh','image':image if mode=='container' else '', 'compose':'.xenia/compose.yaml'}|extra
    try:
        controller=subprocess.Popen([binary,'controller'],env=env,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL);processes.append(controller);until(lambda:api('workspace',{}))
        denied,_=start_worker('denied')
        worker_token,projects=start_worker('allowed',allow=True,containers=docker)
        project=api('projects',{'name':'example','repository':'repo'})['id']
        for bad in [{'source':'file:///private/fixture'},{'source':'https://user:secret@repo.example/repo'},{'unix_setup':'../outside.sh'},{'ref':'HEAD:refs/heads/main'}]:api('projects/environment',config(project,**bad),status=400)
        api('projects/environment',config(project))
        api('projects/assign',{'project_id':project,'node_id':'denied'},status=400)
        api('projects/assign',{'project_id':project,'node_id':'allowed'},token=denied,status=401)
        api('nodes/capacity',{'id':'allowed','capacity':2},status=400)
        modes=['native','container','hybrid'] if docker else ['native']
        for mode in modes:
            if mode!='native':api('projects/environment',config(project,mode))
            job=api('jobs',{'project_id':project,'runner':'codex','prompt':'initial'})['id']
            time.sleep(.3);assert state(job)['state']=='queued','managed project ran before assignment'
            # Environment changes cannot be approved with queued/active work.
            api('projects/environment',config(project,mode),status=400)
            api('projects/assign',{'project_id':project,'node_id':'allowed'})
            assignment=until(lambda:next((a for a in assignments() if a['project_id']==project and a['state'] in ['ready','failed','unknown']),None),180)
            assert assignment['state']=='ready',(assignment,(root/'allowed.log').read_text())
            assert assignment['revision']==commit
            preparation=pathlib.Path(assignment['worktree']);containers.append(preparation)
            checkout=projects/'example/repo';assert not (checkout/'.prepared').exists(),'setup modified primary checkout'
            assert subprocess.check_output(['git','-C',str(checkout),'rev-parse','HEAD'],text=True).strip()==commit
            first=until(lambda:(j if (j:=state(job))['state'] in ['succeeded','failed','unknown'] else None),120)
            assert first['state']=='succeeded',(first,(root/'allowed.log').read_text())
            assert 'PROJECT_ENV_OK' in api('outputs',{'ids':[job]})[job]
            path=pathlib.Path(first['worktree']);containers.append(path)
            follow=api('followup',{'id':job,'prompt':'followup'})['id']
            second=until(lambda:(j if (j:=state(follow))['state'] in ['succeeded','failed','unknown'] else None),120)
            assert second['state']=='succeeded' and second['worktree']==first['worktree'],second
            assert (path/'result.txt').read_text()=='followup'
            api('cleanup',{'id':follow});until(lambda:state(follow)['cleanup_state'] in ['succeeded','failed'],90);assert state(follow)['cleanup_state']=='succeeded',state(follow);assert not path.exists()
            cancel=api('jobs',{'project_id':project,'runner':'codex','prompt':'cancel','node_id':'allowed'})['id']
            until(lambda:'waiting-for-cancel' in api('outputs',{'ids':[cancel]})[cancel],120)
            cancelpath=pathlib.Path(state(cancel)['worktree']);containers.append(cancelpath)
            api('cancel',{'id':cancel});until(lambda:state(cancel)['state'] in ['canceled','failed','unknown'],60);assert state(cancel)['state']=='canceled',state(cancel)
            api('cleanup',{'id':cancel});until(lambda:state(cancel)['cleanup_state'] in ['succeeded','failed'],90);assert state(cancel)['cleanup_state']=='succeeded',state(cancel);assert not cancelpath.exists()
            api('projects/unassign',{'project_id':project,'node_id':'allowed'});assert checkout.exists()
            print(mode+': clone, pinned revision, setup in worktrees, scheduling, follow-up, cancellation, cleanup and preserved checkout: PASS',flush=True)
        print('Local permissions, source/path validation, capacity ceiling and worker authorization: PASS')
    finally:
        for p,stop,log in workers:stop.touch()
        for p,stop,log in workers:
            try:p.wait(timeout=30)
            except subprocess.TimeoutExpired:p.kill();p.wait(timeout=5)
            log.close()
        for p in processes:p.terminate();p.wait(timeout=5)
        if docker:
            import hashlib
            for worktree in containers:
                name='xenia-'+hashlib.sha256(str(worktree).encode()).hexdigest()[:24]
                subprocess.run(['docker','rm','-f',name],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
                subprocess.run(['docker','compose','--project-name',name,'--file',str(source/'.xenia/compose.yaml'),'down','--volumes'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
            subprocess.run(['docker','image','rm',image],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
