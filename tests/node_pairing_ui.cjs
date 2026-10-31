// Real controller: approve and reject node requests at narrow and desktop widths.
const {spawn}=require('node:child_process'),{randomBytes,createHash}=require('node:crypto');
const fs=require('node:fs'),net=require('node:net'),os=require('node:os'),path=require('node:path');
const browserType=require('playwright')[process.env.XENIA_TEST_BROWSER||'chromium'];
(async()=>{
 const root=fs.mkdtempSync(path.join(os.tmpdir(),'xenia-node-ui-')),sock=net.createServer();await new Promise(r=>sock.listen(0,'127.0.0.1',r));const port=sock.address().port;await new Promise(r=>sock.close(r));
 const base=`http://127.0.0.1:${port}`,admin='synthetic-node-ui-admin-token';
 const controller=spawn(path.resolve('target/debug/xenia'),['controller'],{env:{...process.env,XENIA_TOKEN:admin,XENIA_LISTEN:`127.0.0.1:${port}`,XENIA_DB:path.join(root,'db')}});let browser;
 async function api(route,body={},token=admin){const r=await fetch(base+'/api/'+route,{method:'POST',headers:{Authorization:'Bearer '+token,'Content-Type':'application/json'},body:JSON.stringify(body)});if(!r.ok)throw Error(`${route}: ${r.status}`);return r.json()}
 try {
  let ready=false;for(let i=0;i<100;i++){try{await api('workspace');ready=true;break}catch{}await new Promise(r=>setTimeout(r,50))}if(!ready)throw Error('Controller failed to start');
  const secret=randomBytes(32).toString('hex');
  const request=await api('node-pairing/request',{node_id:'example-node',name:'<img src=x onerror="window.injected=true">',platform:'windows',proof:createHash('sha256').update(secret).digest('hex')},'');
  browser=await browserType.launch({headless:true});const page=await browser.newPage({serviceWorkers:'block',viewport:{width:Number(process.env.XENIA_TEST_WIDTH||320),height:900}}),errors=[];page.on('pageerror',e=>errors.push(e.message));
  await page.goto(base);await page.getByLabel('Controller token').fill(admin);await page.getByRole('button',{name:'Connect',exact:true}).click();await page.getByRole('button',{name:'Nodes',exact:false}).click();
  await page.getByText(request.code,{exact:true}).waitFor();
  if(await page.locator('.enrollment-card img').count()||await page.evaluate(()=>!!window.injected))throw Error('Device name interpreted as HTML');
  if(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth))throw Error('Mobile pairing overflows');
  if(await page.title()!=='Xenia')throw Error('Wrong page identity');
  if(process.env.XENIA_TEST_SCREENSHOT)await page.screenshot({path:process.env.XENIA_TEST_SCREENSHOT,fullPage:true});
  page.once('dialog',d=>d.dismiss());await page.getByRole('button',{name:'Approve worker',exact:true}).click();
  if((await api('node-pairing/poll',{id:request.id},secret)).state!=='pending')throw Error('Dismissed approval changed state');
  page.once('dialog',d=>{if(!d.message().includes(request.code))throw Error('Confirmation omits matching code');return d.accept()});
  await page.getByRole('button',{name:'Approve worker',exact:true}).click();await page.getByText('Approved · waiting for worker',{exact:true}).waitFor();
  if((await api('node-pairing/poll',{id:request.id},secret)).state!=='completed')throw Error('Credential not activated');
  await page.locator('.enrollment-card').waitFor({state:'detached',timeout:12000});
  await api('heartbeat',{id:'example-node',name:'Example node',capacity:1,runners:[],instance_id:'instance'},secret);await page.getByText('Example node',{exact:true}).waitFor({timeout:12000});
  const denied=await api('node-pairing/request',{node_id:'denied-node',name:'Rejected worker',platform:'macos',proof:createHash('sha256').update('other-'+secret).digest('hex')},'');
  await page.getByText(denied.code,{exact:true}).waitFor({timeout:12000});page.once('dialog',d=>d.accept());await page.getByRole('button',{name:'Reject',exact:true}).click();await page.locator('.enrollment-card').waitFor({state:'detached'});
  if(errors.length)throw Error(errors.join('\n'));
  console.log(`Node request, matching-code confirmation, dismiss, approve, activate, online and reject at ${page.viewportSize().width}px: PASS`);
 } finally {await browser?.close();if(controller.exitCode===null){const exited=new Promise(r=>controller.once('exit',r));controller.kill();await exited;}fs.rmSync(root,{recursive:true,force:true})}
})().catch(e=>{console.error(e);process.exitCode=1});
