// Isolated browser proof for controller QR creation and mobile device login.
const {chromium}=require('playwright');
const {spawn}=require('node:child_process');
const fs=require('node:fs'),net=require('node:net'),os=require('node:os'),path=require('node:path');
(async()=>{
  const root=fs.mkdtempSync(path.join(os.tmpdir(),'xenia-pairing-ui-'));
  const socket=net.createServer();await new Promise(r=>socket.listen(0,'127.0.0.1',r));const port=socket.address().port;await new Promise(r=>socket.close(r));
  const base=`http://127.0.0.1:${port}`,token='synthetic-pairing-ui-token';
  const server=spawn(path.resolve('target/debug/xenia'),['controller'],{env:{...process.env,XENIA_TOKEN:token,XENIA_LISTEN:`127.0.0.1:${port}`,XENIA_DB:path.join(root,'db')}});
  const api=async(route,body,credential=token)=>fetch(`${base}/api/${route}`,{method:body?'POST':'GET',headers:{Authorization:'Bearer '+credential,'Content-Type':'application/json'},body:body?JSON.stringify(body):undefined});
  let browser;
  try{
    for(let i=0;i<100;i++){try{if((await api('state')).ok)break}catch{}await new Promise(r=>setTimeout(r,50))}
    browser=await chromium.launch({headless:true});const errors=[];
    const admin=await browser.newPage({viewport:{width:1440,height:1000}});admin.on('pageerror',e=>errors.push(e.message));
    await admin.goto(base);await admin.getByLabel('Controller token').fill(token);await admin.getByRole('button',{name:'Connect',exact:true}).click();
    await admin.getByRole('button',{name:'Pair phone',exact:true}).click();
    await admin.getByLabel('Controller HTTPS address').fill('https://controller.example:8443');
    const created=admin.waitForResponse('**/api/pairing/create');await admin.getByRole('button',{name:'Show QR code'}).click();const code=await(await created).json();
    const img=admin.getByAltText('Single-use phone pairing QR code');await img.waitFor();await img.evaluate(el=>el.decode());
    if(await img.evaluate(el=>el.naturalWidth)<320)throw Error('QR did not render');
    if(await admin.getByLabel('Single-use pairing link').inputValue()!==code.url)throw Error('Wrong pairing link');
    if(process.env.XENIA_PAIR_SCREENSHOT)await admin.screenshot({path:process.env.XENIA_PAIR_SCREENSHOT});
    const mobileContext=await browser.newContext({viewport:{width:320,height:740}}),phone=await mobileContext.newPage();phone.on('pageerror',e=>errors.push(e.message));
    // The controller enforces HTTPS origins in QR links. Local test transports the
    // same fragment over loopback; deployed trusted HTTPS gets separate coverage.
    await phone.goto(base+'/#'+code.url.split('#')[1]);await phone.getByText('Connected',{exact:true}).waitFor();
    const credential=await phone.evaluate(()=>localStorage.getItem('xenia-device-token'));
    if(!credential||credential===token||await phone.evaluate(()=>location.hash))throw Error('Pairing did not isolate credential or clear link');
    await phone.reload();await phone.getByText('Connected',{exact:true}).waitFor();
    if((await api('pairing/redeem',{code:code.url.split('pair=')[1]},'')).status!==400)throw Error('Code replay accepted');
    if(await phone.evaluate(()=>document.documentElement.scrollWidth>innerWidth))throw Error('Mobile header overflows');
    await phone.getByRole('button',{name:'Pair phone',exact:true}).click();await phone.getByLabel('Controller HTTPS address').fill('https://controller.example:8443');await phone.getByRole('button',{name:'Show QR code'}).click();await phone.getByAltText('Single-use phone pairing QR code').waitFor();
    if(await phone.evaluate(()=>document.documentElement.scrollWidth>innerWidth))throw Error('Mobile QR dialog overflows');
    if(process.env.XENIA_PAIR_MOBILE_SCREENSHOT)await phone.screenshot({path:process.env.XENIA_PAIR_MOBILE_SCREENSHOT});
    await phone.getByRole('button',{name:'Done',exact:true}).click();
    await phone.getByRole('button',{name:'Sign out',exact:true}).click();await phone.getByRole('heading',{name:'Connect to Xenia'}).waitFor();
    await phone.waitForTimeout(100);
    if((await api('state',null,credential)).status!==401)throw Error('Sign-out did not revoke device');
    if(await phone.evaluate(()=>localStorage.getItem('xenia-device-token')))throw Error('Sign-out kept credential');
    // Installed PWAs with separate Safari storage can redeem a fresh link locally.
    const link=await(await api('pairing/create',{origin:'https://controller.example:8443'})).json();
    await phone.getByText('Use a pairing link instead').click();await phone.getByLabel('Pairing link',{exact:true}).fill(base+'/#'+link.url.split('#')[1]);await phone.getByRole('button',{name:'Pair this device'}).click();await phone.getByText('Connected',{exact:true}).waitFor();
    const device=(await(await api('pairing/devices',{})).json())[0];await api('pairing/revoke',{id:device.id});await phone.reload();await phone.getByRole('heading',{name:'Connect to Xenia'}).waitFor();
    await phone.getByLabel('Controller token').fill(token);await phone.getByRole('button',{name:'Connect',exact:true}).click();await phone.reload();await phone.getByText('Connected',{exact:true}).waitFor();
    if(errors.length)throw Error(errors.join('; '));
    console.log('Desktop QR rendering; 320px pairing, persistence, one-use redemption, logout, link fallback, revocation and manual-login recovery: PASS');
  }finally{if(browser)await browser.close();if(server.exitCode===null&&server.signalCode===null){server.kill();await new Promise(r=>server.once('exit',r))}fs.rmSync(root,{recursive:true,force:true})}
})().catch(e=>{console.error(e);process.exitCode=1});
