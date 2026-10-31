// Browser integration test against an isolated controller. Requires Playwright.
const {chromium} = require('playwright');
const {spawn} = require('node:child_process');
const fs = require('node:fs'), net = require('node:net'), os = require('node:os'), path = require('node:path');
(async () => {
  const root=fs.mkdtempSync(path.join(os.tmpdir(),'xenia-signout-'));
  const socket=net.createServer(); await new Promise(resolve=>socket.listen(0,'127.0.0.1',resolve));
  const port=socket.address().port; await new Promise(resolve=>socket.close(resolve));
  const base=`http://127.0.0.1:${port}`, token='synthetic-signout-controller-token';
  const server=spawn(path.resolve('target/debug/xenia'),['controller'],{env:{...process.env,XENIA_TOKEN:token,XENIA_LISTEN:`127.0.0.1:${port}`,XENIA_DB:path.join(root,'db')}});
  let browser;
  const api=async(route,body)=>{const r=await fetch(`${base}/api/${route}`,{method:body?'POST':'GET',headers:{Authorization:'Bearer '+token,'Content-Type':'application/json'},body:body?JSON.stringify(body):undefined}); if(!r.ok)throw Error(`${route}: ${r.status}`);return r.json()};
  try {
    let connected=false; for(let i=0;i<100;i++){try{await api('state');connected=true;break}catch{await new Promise(resolve=>setTimeout(resolve,50))}} if(!connected)throw Error('Controller did not start');
    await api('projects',{name:'example',repository:'repo'});
    browser=await chromium.launch({headless:true,executablePath:process.env.XENIA_CHROMIUM_BIN||undefined});
    const page=await browser.newPage({viewport:process.env.XENIA_TEST_VIEWPORT==='desktop'?{width:1440,height:900}:{width:390,height:844}}), errors=[];
    page.on('pageerror',e=>errors.push(e.message));
    await page.goto(base); await page.getByLabel('Controller token').fill(token); await page.getByRole('button',{name:'Connect'}).click();
    await page.getByRole('button',{name:'Projects',exact:false}).click(); await page.getByText('example', {exact:true}).waitFor();
    await page.route('**/api/state', route => route.abort(), {times:1});
    await page.getByText(/Controller unavailable. Showing previously received data/).waitFor({timeout:12000});
    await page.getByText('Connected',{exact:true}).waitFor({timeout:12000});
    // Delay an already-authorized refresh until after sign-out to verify stale responses are ignored.
    let release; const gate=new Promise(resolve=>{release=resolve}); let intercepted; const caught=new Promise(resolve=>{intercepted=resolve});
    await page.route('**/api/state',async route=>{const response=await route.fetch();intercepted();await gate;await route.fulfill({response})},{times:1});
    await Promise.race([caught,new Promise((_,reject)=>setTimeout(()=>reject(Error('Refresh did not start')),10000))]);
    await page.getByRole('button',{name:'Sign out'}).click();
    if(!await page.getByRole('dialog').getByText('Connect to Xenia').isVisible())throw Error('Sign-out did not show login');
    if(await page.evaluate(()=>sessionStorage.getItem('xenia-token')))throw Error('Token remained in session storage');
    if(await page.getByText('example',{exact:true}).count())throw Error('Workspace data remained visible');
    if(process.env.XENIA_SIGNOUT_SCREENSHOT)await page.screenshot({path:process.env.XENIA_SIGNOUT_SCREENSHOT});
    release(); await page.waitForTimeout(500);
    if(await page.getByText('example',{exact:true}).count())throw Error('Late response restored signed-out data');
    await page.getByLabel('Controller token').fill(token);await page.getByRole('button',{name:'Connect'}).click();
    await page.getByRole('button',{name:'Projects',exact:false}).click(); await page.getByText('example',{exact:true}).waitFor();
    if(errors.length)throw Error(errors.join('; '));
    console.log(`${process.env.XENIA_TEST_VIEWPORT==='desktop'?'Desktop':'Mobile'} sign-out, stale response rejection, and reconnect: PASS`);
  } finally {if(browser)await browser.close();if(server.exitCode===null&&server.signalCode===null){server.kill();await new Promise(resolve=>server.once('exit',resolve))}fs.rmSync(root,{recursive:true,force:true})}
})().catch(e=>{console.error(e);process.exitCode=1});
