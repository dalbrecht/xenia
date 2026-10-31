// Job outcome and worktree lifecycle are independent, including shared turn worktrees.
const browserType=require('playwright')[process.env.XENIA_TEST_BROWSER||'chromium'];
const {spawn}=require('node:child_process'),{randomUUID}=require('node:crypto');
const fs=require('node:fs'),net=require('node:net'),os=require('node:os'),path=require('node:path');
(async()=>{
 const root=fs.mkdtempSync(path.join(os.tmpdir(),'xenia-worktree-ui-')),socket=net.createServer();await new Promise(r=>socket.listen(0,'127.0.0.1',r));const port=socket.address().port;await new Promise(r=>socket.close(r));
 const base=`http://127.0.0.1:${port}`,token='synthetic-worktree-ui-token';
 const server=spawn(path.resolve('target/debug/xenia'),['controller'],{env:{...process.env,XENIA_TOKEN:token,XENIA_LISTEN:`127.0.0.1:${port}`,XENIA_DB:path.join(root,'db')}});
 let browser;
 async function api(route,body,credential=token){const r=await fetch(base+'/api/'+route,{method:body?'POST':'GET',headers:{Authorization:'Bearer '+credential,'Content-Type':'application/json'},body:body?JSON.stringify(body):undefined});if(!r.ok)throw Error(`${route}: ${r.status} ${await r.text()}`);return r.json()}
 try{
  let ready=false;for(let i=0;i<100;i++){try{await api('workspace',{});ready=true;break}catch{}await new Promise(r=>setTimeout(r,50))}if(!ready)throw Error('Controller did not start');
  const credential=(await api('workers/enroll',{id:'test'})).token;
  await api('heartbeat',{id:'test',name:'Test',capacity:1,runners:['codex'],instance_id:'instance'},credential);
  const project=(await api('projects',{name:'example',repository:'repo'})).id;
  async function finish(id,outcome,worktree,branch='xenia/'+id){
   const job=await api('claim',{id:'test',instance_id:'instance',claim_id:randomUUID(),eligible:[project]},credential);if(job.id!==id)throw Error('Wrong claimed fixture');
   if(outcome==='canceled')await api('cancel',{id});
   await api('report',{id,node_id:'test',state:outcome,session_id:'synthetic-session',worktree,branch},credential);
  }
  const success=(await api('jobs',{project_id:project,runner:'codex',prompt:'Successful work'})).id;
  await finish(success,'succeeded','/tmp/synthetic-success-worktree');
  const follow=(await api('followup',{id:success,prompt:'Successful follow-up'})).id;
  await finish(follow,'succeeded','/tmp/synthetic-success-worktree','xenia/'+success);
  const canceled=(await api('jobs',{project_id:project,runner:'codex',prompt:'Canceled work'})).id;
  await finish(canceled,'canceled','/tmp/synthetic-canceled-worktree');
  browser=await browserType.launch({headless:true});const page=await browser.newPage({serviceWorkers:'block',viewport:{width:Number(process.env.XENIA_TEST_WIDTH||390),height:844}}),errors=[];page.on('pageerror',e=>errors.push(e.message));page.on('dialog',dialog=>dialog.accept());
  await page.goto(base);await page.getByLabel('Controller token').fill(token);await page.getByRole('button',{name:'Connect',exact:true}).click();
  async function badges(id,outcome,worktree){const row=page.locator(`[data-job="${id}"]`);await row.locator('.status').filter({hasText:new RegExp('^'+outcome+'$','i')}).waitFor();await row.locator('.worktree-state').filter({hasText:worktree}).waitFor();}
  await badges(success,'succeeded','Worktree retained');await badges(canceled,'canceled','Worktree retained');
  await page.locator(`[data-job="${success}"]`).click();await page.getByRole('button',{name:'Remove worktree',exact:true}).click();await badges(success,'succeeded','Cleanup queued');
  let cleanup=await api('cleanup/claim',{id:'test',instance_id:'instance',claim_id:randomUUID()},credential);await badges(success,'succeeded','Removing worktree');
  await api('cleanup/report',{id:cleanup.id,node_id:'test',state:'succeeded'},credential);await badges(success,'succeeded','Worktree removed');
  if(await page.getByRole('button',{name:'Remove worktree',exact:true}).count())throw Error('Removed worktree still offers cleanup');
  await page.locator(`[data-job="${canceled}"]`).click();await page.getByRole('button',{name:'Remove worktree',exact:true}).click();await badges(canceled,'canceled','Cleanup queued');
  cleanup=await api('cleanup/claim',{id:'test',instance_id:'instance',claim_id:randomUUID()},credential);
  await api('cleanup/report',{id:cleanup.id,node_id:'test',state:'failed',error:'Worktree contains uncommitted changes'},credential);await badges(canceled,'canceled','Cleanup failed');
  await page.getByRole('button',{name:'Remove worktree',exact:true}).click();await badges(canceled,'canceled','Cleanup queued');
  cleanup=await api('cleanup/claim',{id:'test',instance_id:'instance',claim_id:randomUUID()},credential);await api('cleanup/report',{id:cleanup.id,node_id:'test',state:'succeeded'},credential);await badges(canceled,'canceled','Worktree removed');
  await page.getByRole('button',{name:'Work queue',exact:false}).click();
  await badges(success,'succeeded','Worktree removed');await badges(follow,'succeeded','Worktree removed');await badges(canceled,'canceled','Worktree removed');
  if(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth))throw Error('Independent state badges overflow');
  if(process.env.XENIA_WORKTREE_SCREENSHOT)await page.screenshot({path:process.env.XENIA_WORKTREE_SCREENSHOT});
  if(errors.length)throw Error(errors.join('; '));console.log('Independent result/worktree states, cleanup transitions/failure/retry, shared turn worktrees, conversation and queue views: PASS');
 }finally{if(browser)await browser.close();if(server.exitCode===null&&server.signalCode===null){server.kill();await new Promise(r=>server.once('exit',r))}fs.rmSync(root,{recursive:true,force:true})}
})().catch(error=>{console.error(error);process.exitCode=1});
