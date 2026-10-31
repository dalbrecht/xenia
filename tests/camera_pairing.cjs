// Browser regression with synthetic camera frames; never opens real hardware.
const {chromium}=require('playwright');const {spawn}=require('node:child_process');
const fs=require('node:fs'),net=require('node:net'),os=require('node:os'),path=require('node:path');
(async()=>{
const root=fs.mkdtempSync(path.join(os.tmpdir(),'xenia-camera-')),socket=net.createServer();await new Promise(r=>socket.listen(0,'127.0.0.1',r));const port=socket.address().port;await new Promise(r=>socket.close(r));
const base=`http://127.0.0.1:${port}`,origin='https://controller.example',token='synthetic-camera-test-token';
const server=spawn(path.resolve('target/debug/xenia'),['controller'],{env:{...process.env,XENIA_TOKEN:token,XENIA_LISTEN:`127.0.0.1:${port}`,XENIA_DB:path.join(root,'db')}});
const api=async(route,body,credential=token)=>fetch(`${base}/api/${route}`,{method:body?'POST':'GET',headers:{Authorization:'Bearer '+credential,'Content-Type':'application/json'},body:body?JSON.stringify(body):undefined});let browser;
try{
for(let i=0;i<100;i++){try{if((await api('state')).ok)break}catch{}await new Promise(r=>setTimeout(r,50))}
const created=await(await api('pairing/create',{origin})).json();
browser=await chromium.launch({headless:true});const context=await browser.newContext({viewport:{width:390,height:844}});
await context.route(origin+'/**',async route=>{const response=await route.fetch({url:base+new URL(route.request().url()).pathname});await route.fulfill({response})});
await context.addInitScript(({svg})=>{
 window.testCameraMode='denied';window.testStreams=[];
 Object.defineProperty(navigator.mediaDevices,'getUserMedia',{value:async()=>{
  if(window.testCameraMode==='denied')throw Error('test denied');
  const canvas=document.createElement('canvas');canvas.width=640;canvas.height=480;
  const ctx=canvas.getContext('2d');ctx.fillStyle='white';ctx.fillRect(0,0,640,480);
  const image=new Image();image.src='data:image/svg+xml;charset=utf-8,'+encodeURIComponent(svg);await image.decode();
  if(window.testCameraMode!=='blank')ctx.drawImage(image,80,0,480,480);
  const stream=canvas.captureStream(5);window.testStreams.push(stream);window.testCanvas=canvas;
  if(window.testCameraMode==='late')await new Promise(r=>setTimeout(r,300));
  return stream;
 }});
},{svg:created.svg});
const page=await context.newPage(),errors=[];page.on('pageerror',e=>errors.push(e.message));
await page.goto(origin);await page.getByRole('button',{name:'Scan QR code',exact:true}).click();await page.getByRole('alert').filter({hasText:'Camera unavailable'}).waitFor();
await page.evaluate(()=>window.testCameraMode='late');await page.getByRole('button',{name:'Scan QR code',exact:true}).click();await page.getByRole('button',{name:'Stop camera'}).click();await page.waitForTimeout(500);
if(!await page.evaluate(()=>window.testStreams.every(s=>s.getTracks().every(t=>t.readyState==='ended'))))throw Error('Late camera stream leaked');
await page.evaluate(()=>window.testCameraMode='blank');await page.getByRole('button',{name:'Scan QR code',exact:true}).click();await page.getByText('Point the camera at a fresh Xenia pairing QR code.',{exact:true}).waitFor();await page.getByRole('button',{name:'Stop camera'}).click();
if(!await page.evaluate(()=>window.testStreams.every(s=>s.getTracks().every(t=>t.readyState==='ended'))))throw Error('Stop camera leaked');
await page.evaluate(()=>window.testCameraMode='qr');await page.getByRole('button',{name:'Scan QR code',exact:true}).click();await page.getByText('Connected',{exact:true}).waitFor();
if(!await page.evaluate(()=>window.testStreams.every(s=>s.getTracks().every(t=>t.readyState==='ended'))))throw Error('Camera leaked after pairing');
if(!await page.evaluate(()=>!!localStorage.getItem('xenia-device-token')))throw Error('No device credential');
await page.reload();await page.getByText('Connected',{exact:true}).waitFor();if(process.env.XENIA_CAMERA_CONNECTED_SCREENSHOT)await page.screenshot({path:process.env.XENIA_CAMERA_CONNECTED_SCREENSHOT});
await page.getByRole('button',{name:'Sign out',exact:true}).click();await page.getByRole('button',{name:'Scan QR code',exact:true}).waitFor();if(process.env.XENIA_CAMERA_LOGIN_SCREENSHOT)await page.screenshot({path:process.env.XENIA_CAMERA_LOGIN_SCREENSHOT});
if(errors.length)throw Error(errors.join('; '));console.log('QR camera decoding, denied permission, stop/late-stream cleanup, device pairing and persistence: PASS');
}finally{if(browser)await browser.close();if(server.exitCode===null&&server.signalCode===null){server.kill();await new Promise(r=>server.once('exit',r))}fs.rmSync(root,{recursive:true,force:true})}
})().catch(e=>{console.error(e);process.exitCode=1});
