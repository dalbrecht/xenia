// Long-history navigation regression; isolated controller, no provider usage.
const {chromium}=require('playwright'),{spawn,execFileSync}=require('node:child_process');
const fs=require('node:fs'),net=require('node:net'),os=require('node:os'),path=require('node:path');
(async()=>{
 const root=fs.mkdtempSync(path.join(os.tmpdir(),'xenia-history-ui-')),db=path.join(root,'db');
 const socket=net.createServer();await new Promise(r=>socket.listen(0,'127.0.0.1',r));const port=socket.address().port;await new Promise(r=>socket.close(r));
 const base=`http://127.0.0.1:${port}`,token='synthetic-history-browser-token';let server,browser;
 const api=async(route,body)=>{const response=await fetch(`${base}/api/${route}`,{method:body?'POST':'GET',headers:{Authorization:'Bearer '+token,'Content-Type':'application/json'},body:body?JSON.stringify(body):undefined});if(!response.ok)throw Error(`${route}: ${response.status}`);return response.json()};
 async function start(){server=spawn(path.resolve('target/debug/xenia'),['controller'],{env:{...process.env,XENIA_TOKEN:token,XENIA_LISTEN:`127.0.0.1:${port}`,XENIA_DB:db}});for(let i=0;i<100;i++){try{await api('workspace',{});return}catch{}await new Promise(r=>setTimeout(r,50))}throw Error('Controller did not start')}
 async function stop(){if(server&&server.exitCode===null&&server.signalCode===null){server.kill();await new Promise(r=>server.once('exit',r))}}
 try{
  await start();await stop();
  execFileSync(process.env.PYTHON||'python3',['-c',`import sqlite3,sys
with sqlite3.connect(sys.argv[1]) as c:
 c.execute("INSERT INTO projects VALUES('p','example','repo')")
 c.execute("INSERT INTO projects VALUES('q','another','app')")
 for n in range(40):c.execute("INSERT INTO jobs(id,project_id,prompt,runner,state,created) VALUES(?,? ,?,'codex','queued',0)",(f'root-{n}','p' if n%2==0 else 'q',f'Older task {n} '+('x'*200)+f' END-{n}'))
 for n in range(12):
  c.execute("INSERT INTO jobs(id,project_id,prompt,runner,node_id,state,created,parent_id,session_id,worktree,branch) VALUES(?,'p',?,'codex','worker','succeeded',0,?,'session','/tmp/synthetic-worktree','xenia/test')",(f'turn-{n}',('Long conversation' if n==0 else f'Turn {n}')+('x'*200)+f' END-TURN-{n}',None if n==0 else f'turn-{n-1}'))
  c.execute("INSERT INTO job_output_chunks(job_id,body) VALUES(?,?)",(f'turn-{n}',f'Output {n}\\n'))
`,db]);await start();
  browser=await chromium.launch({headless:true});const page=await browser.newPage({viewport:{width:Number(process.env.XENIA_TEST_WIDTH||390),height:844}}),errors=[];page.on('pageerror',e=>errors.push(e.message));
  await page.goto(base);await page.getByLabel('Controller token').fill(token);await page.getByRole('button',{name:'Connect',exact:true}).click();
  await page.getByText('41 total',{exact:true}).waitFor();if(await page.locator('[data-job]').count()!==25)throw Error('Unbounded conversation list');
  await page.locator('[data-job="turn-0"]').click();await page.locator('.message pre').filter({hasText:'Output 11'}).waitFor();
  if(await page.locator('.message').count()!==5)throw Error('Unbounded turn list');
  await page.getByRole('button',{name:'Earlier turns',exact:true}).click();await page.locator('.message pre').filter({hasText:'Output 6'}).waitFor();
  await page.getByRole('button',{name:'Earlier turns',exact:true}).click();await page.locator('.message pre').filter({hasText:'Output 0'}).waitFor();
  if(await page.locator('.message').count()!==2)throw Error('Wrong oldest turn page');
  if(await page.evaluate(()=>Object.keys(outputCache).length)>5)throw Error('Hidden turn output retained');
  if(await page.locator('[data-follow]').getAttribute('data-follow')!=='turn-11')throw Error('Earlier turn page offered stale follow-up');
  await page.getByRole('button',{name:'Latest turns',exact:true}).click();await page.locator('.message pre').filter({hasText:'Output 11'}).waitFor();
  await page.getByRole('button',{name:'Follow up',exact:true}).click();await page.getByLabel('Follow-up message').fill('A new follow-up');const sent=page.waitForResponse('**/api/followup');await page.getByRole('button',{name:'Send follow-up'}).click();const response=await sent;if(!response.ok())throw Error('Follow-up failed: '+await response.text());await page.getByText('13 turns',{exact:false}).waitFor();
  await page.getByRole('button',{name:'Earlier work',exact:true}).click();await page.locator('[data-job="root-0"]').waitFor();await page.locator('[data-job="root-0"]').click();await page.locator('.message p').filter({hasText:'END-0'}).waitFor();
  await page.getByRole('button',{name:'Latest work',exact:true}).click();await page.locator('[data-job="turn-0"]').waitFor();
  await page.getByLabel('Project filter',{exact:true}).selectOption('q');await page.getByText('20 total',{exact:true}).waitFor();if(await page.locator('[data-job]').count()!==20)throw Error('Server-side project filter lost older conversations');
  await page.getByRole('button',{name:'Work queue',exact:false}).click();await page.getByText('20 total',{exact:true}).waitFor();await page.getByLabel('Project filter',{exact:true}).selectOption('');await page.getByText('53 total',{exact:true}).waitFor();
  if(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth))throw Error('Mobile pagination overflows');
  if(process.env.XENIA_HISTORY_SCREENSHOT)await page.screenshot({path:process.env.XENIA_HISTORY_SCREENSHOT});
  // A slow old-view response must not replace the new view's state.
  await page.getByRole('button',{name:'Nodes',exact:false}).click();await page.getByRole('heading',{name:'Worker nodes'}).waitFor();
  let release,caught;const gate=new Promise(r=>release=r),started=new Promise(r=>caught=r);
  await page.route('**/api/workspace',async route=>{const response=await route.fetch();caught();await gate;await route.fulfill({response})},{times:1});
  await page.getByRole('button',{name:'Work queue',exact:false}).click();await started;await page.getByRole('button',{name:'Projects',exact:false}).click();await page.getByText('example',{exact:true}).waitFor();release();await page.waitForTimeout(150);
  if(!await page.evaluate(()=>view==='projects'&&state.jobs.length===0))throw Error('Stale workspace response restored previous view data');
  if(errors.length)throw Error(errors.join('; '));console.log('Bounded mobile conversation/queue lists, turn/output paging, full prompts, latest follow-up, filters and navigation races: PASS');
 }finally{if(browser)await browser.close();await stop();fs.rmSync(root,{recursive:true,force:true})}
})().catch(e=>{console.error(e);process.exitCode=1});
