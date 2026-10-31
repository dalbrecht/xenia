// Project environment selection, assignment, local permission display and setup states.
const {spawn}=require('node:child_process'),{randomUUID}=require('node:crypto');
const fs=require('node:fs'),net=require('node:net'),os=require('node:os'),path=require('node:path');
const browserType=require('playwright')[process.env.XENIA_TEST_BROWSER||'chromium'];
(async()=>{
 const root=fs.mkdtempSync(path.join(os.tmpdir(),'xenia-project-ui-')),socket=net.createServer();await new Promise(r=>socket.listen(0,'127.0.0.1',r));const port=socket.address().port;await new Promise(r=>socket.close(r));
 const base=`http://127.0.0.1:${port}`,admin='synthetic-project-ui-admin-token';let browser;
 const server=spawn(path.resolve('target/debug/xenia'),['controller'],{env:{...process.env,XENIA_TOKEN:admin,XENIA_DB:path.join(root,'db'),XENIA_LISTEN:`127.0.0.1:${port}`}});
 async function api(route,body={},token=admin){const r=await fetch(base+'/api/'+route,{method:'POST',headers:{Authorization:'Bearer '+token,'Content-Type':'application/json'},body:JSON.stringify(body)});if(!r.ok)throw Error(`${route}: ${r.status} ${await r.text()}`);return r.json()}
 try{
  let ready=false;for(let i=0;i<100;i++){try{await api('workspace');ready=true;break}catch{}await new Promise(r=>setTimeout(r,50))}if(!ready)throw Error('Controller did not start');
  const worker=(await api('workers/enroll',{id:'allowed'})).token,denied=(await api('workers/enroll',{id:'denied'})).token;
  const permissions={project_checkout:true,setup_scripts:true,containers:true,runners:['codex'],projects:['example'],max_workers:2,platform:'windows'};
  await api('heartbeat',{id:'allowed',name:'Allowed node',capacity:2,runners:['codex'],instance_id:'instance',policy:permissions},worker);
  await api('heartbeat',{id:'denied',name:'Restricted node',capacity:1,runners:['codex'],instance_id:'instance',policy:{...permissions,project_checkout:false,max_workers:1}},denied);
  const project=(await api('projects',{name:'example',repository:'repo'})).id;
  browser=await browserType.launch({headless:true});const page=await browser.newPage({serviceWorkers:'block',viewport:{width:Number(process.env.XENIA_TEST_WIDTH||320),height:900}}),errors=[];page.on('pageerror',e=>errors.push(e.message));
  await page.goto(base);await page.getByLabel('Controller token').fill(admin);await page.getByRole('button',{name:'Connect',exact:true}).click();await page.getByRole('button',{name:'Projects',exact:false}).click();
  await page.getByRole('button',{name:'Environment',exact:true}).click();const form=page.locator('#environmentform');
  await form.getByLabel('Git HTTPS repository').fill('https://repo.example/project.git');await form.getByLabel('Windows setup script (optional)').fill('.xenia/setup.ps1');
  await form.getByRole('button',{name:'Save environment',exact:true}).click();
  await page.getByRole('button',{name:'Assign node',exact:true}).click();
  const node=page.locator('#assignmentform select[name="node_id"]');
  if(!await node.locator('option[value="denied"]').evaluate(e=>e.disabled))throw Error('Restricted node offered for automatic checkout: '+await node.evaluate(e=>e.outerHTML)+' '+JSON.stringify((await api('workspace')).nodes));
  await node.selectOption('allowed');await page.getByRole('button',{name:'Assign and prepare',exact:true}).click();await page.locator('.assignment-card .status').filter({hasText:/^queued$/}).waitFor();
  const item=await api('provision/claim',{id:'allowed',instance_id:'instance',claim_id:randomUUID()},worker);
  const report={id:item.id,node_id:'allowed',instance_id:'instance',claim_id:item.claim_id,event_id:randomUUID(),state:'preparing',step:'Provisioning environment',log:'<img src=x onerror="window.injected=true">'};
  await api('provision/report',report,worker);await page.getByText('Provisioning environment',{exact:true}).waitFor({timeout:12000});
  await page.locator('.assignment-card summary').click();
  if(await page.locator('.assignment-card img').count()||await page.evaluate(()=>!!window.injected))throw Error('Setup log interpreted as HTML');
  await api('provision/report',{...report,event_id:randomUUID(),state:'ready',step:'Ready',revision:'a'.repeat(40),runners:['codex']},worker);await page.locator('.assignment-card .status').filter({hasText:/^ready$/}).waitFor({timeout:12000});
  if(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth))throw Error('Project layout overflows');
  if(process.env.XENIA_TEST_SCREENSHOT)await page.screenshot({path:process.env.XENIA_TEST_SCREENSHOT,fullPage:true});
  await page.getByRole('button',{name:'Environment',exact:true}).click();
  await form.getByLabel('Development environment').selectOption('container');await form.getByLabel('Container image').fill('example/dev:1');
  if(!await page.locator('#environment-windows').isHidden() || await page.locator('#environment-container').isHidden())throw Error('Container fields incorrect');
  await form.getByRole('button',{name:'Save environment',exact:true}).click();await page.getByText('needs setup',{exact:true}).waitFor();
  if((await api('workspace',{mode:'projects'})).projects[0].environment.mode!=='container')throw Error('Container choice was not saved');
  await page.getByRole('button',{name:'Environment',exact:true}).click();await form.getByLabel('Development environment').selectOption('hybrid');
  if(await page.locator('#environment-hybrid').isHidden() || !await page.locator('#environment-container').isHidden())throw Error('Hybrid fields incorrect');
  await form.getByLabel('Docker Compose file').fill('compose.yaml');await form.getByRole('button',{name:'Save environment',exact:true}).click();
  await page.getByRole('button',{name:'Nodes',exact:false}).click();await page.getByText('Local permissions · windows · maximum 2 workers',{exact:true}).click();await page.getByText('Change these settings in the desktop app on this node.',{exact:true}).first().waitFor();
  if(await page.locator('.node-policy input,.node-policy select').count())throw Error('Controller allows editing local permissions');
  if(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth))throw Error('Permission details overflow');
  if(errors.length)throw Error(errors.join('\n'));
  console.log(`Native/Container/Hybrid selection, restricted assignment, setup states, escaped logs and read-only local permissions at ${page.viewportSize().width}px: PASS`);
 }finally{await browser?.close();if(server.exitCode===null){const exited=new Promise(r=>server.once('exit',r));server.kill();await exited;}fs.rmSync(root,{recursive:true,force:true})}
})().catch(e=>{console.error(e);process.exitCode=1});
