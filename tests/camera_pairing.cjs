// Fault-injection tests block service workers; offline_pwa.cjs covers real caching.
// Browser regression with synthetic camera frames; never opens real hardware.
const browserType=require('playwright')[process.env.XENIA_TEST_BROWSER || 'chromium'];const {spawn}=require('node:child_process');
const fs=require('node:fs'),net=require('node:net'),os=require('node:os'),path=require('node:path');
(async()=>{
const root=fs.mkdtempSync(path.join(os.tmpdir(),'xenia-camera-')),socket=net.createServer();await new Promise(r=>socket.listen(0,'127.0.0.1',r));const port=socket.address().port;await new Promise(r=>socket.close(r));
const base=`http://127.0.0.1:${port}`,origin='https://controller.example',token='synthetic-camera-test-token';
const server=spawn(path.resolve('target/debug/xenia'),['controller'],{env:{...process.env,XENIA_TOKEN:token,XENIA_LISTEN:`127.0.0.1:${port}`,XENIA_DB:path.join(root,'db')}});
const api=async(route,body,credential=token)=>fetch(`${base}/api/${route}`,{method:body?'POST':'GET',headers:{Authorization:'Bearer '+credential,'Content-Type':'application/json'},body:body?JSON.stringify(body):undefined});let browser;
try{
for(let i=0;i<100;i++){try{if((await api('state')).ok)break}catch{}await new Promise(r=>setTimeout(r,50))}
const created=await(await api('pairing/create',{origin})).json();
browser=await browserType.launch({headless:true});const context=await browser.newContext({serviceWorkers:'block',viewport:{width:390,height:844}});
await context.route(origin+'/**',async route=>{const response=await route.fetch({url:base+new URL(route.request().url()).pathname});await route.fulfill({response})});
await context.addInitScript(({svg,syntheticVideo})=>{
 window.testCameraMode='denied';window.testStreams=[];
 // Linux WebKit's headless media pipeline can leave canvas-stream play pending.
 // Keep real MediaStreams/tracks and the real QR decoder, but supply deterministic
 // video frames in that fixture. Native playback still requires device validation.
 if(syntheticVideo){
  const originalPlay=HTMLMediaElement.prototype.play,draw=CanvasRenderingContext2D.prototype.drawImage;
  HTMLMediaElement.prototype.play=function(){
   if(this.id!=='scan-video')return originalPlay.call(this);
   Object.defineProperties(this,{readyState:{configurable:true,get:()=>2},videoWidth:{configurable:true,get:()=>640},videoHeight:{configurable:true,get:()=>480}});
   return Promise.resolve();
  };
  CanvasRenderingContext2D.prototype.drawImage=function(source,...args){return draw.call(this,source instanceof HTMLVideoElement && source.id==='scan-video'?window.testCanvas:source,...args)};
 }

 Object.defineProperty(navigator,'mediaDevices',{value:{getUserMedia:async()=>{
  window.testCameraFixtureError=null; try {
  if(window.testCameraMode==='denied')throw Error('test denied');
  const canvas=document.createElement('canvas');canvas.width=640;canvas.height=480;
  const ctx=canvas.getContext('2d');ctx.fillStyle='white';ctx.fillRect(0,0,640,480);
  if(window.testCameraMode==='qr') {const image=new Image();image.src='data:image/svg+xml;charset=utf-8,'+encodeURIComponent(svg);await image.decode();ctx.drawImage(image,80,0,480,480);}
  const stream=canvas.captureStream(5);window.testStreams.push(stream);window.testCanvas=canvas;
  // WebKit emits new canvas frames after a paint, including after video.play().
  const repaint=setInterval(()=>{ctx.fillStyle='white';ctx.fillRect(0,0,1,1)},100);
  for(const track of stream.getTracks()){const stop=track.stop.bind(track);track.stop=()=>{clearInterval(repaint);stop()};}
  if(window.testCameraMode==='late')await new Promise(r=>{window.testCameraRelease=r});
  return stream;
  } catch(error) {window.testCameraFixtureError=error.message;throw error;}
 }}});
},{svg:created.svg,syntheticVideo:process.env.XENIA_TEST_SYNTHETIC_VIDEO==='1' || process.platform==='linux' && process.env.XENIA_TEST_BROWSER==='webkit'});
const page=await context.newPage(),errors=[];page.on('pageerror',e=>errors.push(e.message));
await page.goto(origin);await page.getByRole('button',{name:'Scan QR code',exact:true}).click();await page.getByRole('alert').filter({hasText:'Camera unavailable'}).waitFor();
await page.evaluate(()=>window.testCameraMode='late');await page.getByRole('button',{name:'Scan QR code',exact:true}).click();await page.waitForFunction(()=>!!window.testCameraRelease,{},{timeout:3000}).catch(async()=>{throw Error('Late camera fixture: '+JSON.stringify(await page.evaluate(()=>({mode:window.testCameraMode,error:window.testCameraFixtureError,status:document.querySelector('#scan-status').textContent,previewHidden:document.querySelector('#scan-preview').hidden,mocked:navigator.mediaDevices.getUserMedia.toString().includes('testCameraMode')}))))});try {await page.getByRole('button',{name:'Stop camera'}).click({timeout:3000});}catch(error){throw Error('Synthetic camera startup: '+JSON.stringify(await page.evaluate(()=>({error:window.testCameraFixtureError,hidden:document.hidden,status:document.querySelector('#scan-status').textContent,previewHidden:document.querySelector('#scan-preview').hidden}))))}await page.evaluate(()=>window.testCameraRelease());await page.waitForFunction(()=>window.testStreams.every(s=>s.getTracks().every(t=>t.readyState==='ended')));
if(!await page.evaluate(()=>window.testStreams.every(s=>s.getTracks().every(t=>t.readyState==='ended'))))throw Error('Late camera stream leaked');
await page.evaluate(()=>window.testCameraMode='blank');await page.getByRole('button',{name:'Scan QR code',exact:true}).click();await page.getByText('Point the camera at a fresh Xenia pairing QR code.',{exact:true}).waitFor();await page.getByRole('button',{name:'Stop camera'}).click();
if(!await page.evaluate(()=>window.testStreams.every(s=>s.getTracks().every(t=>t.readyState==='ended'))))throw Error('Stop camera leaked');
await page.evaluate(()=>window.testCameraMode='qr');await page.getByRole('button',{name:'Scan QR code',exact:true}).click();
await page.getByText('Connected',{exact:true}).waitFor({timeout:10000}).catch(async()=>{throw Error('QR decode state: '+JSON.stringify(await page.evaluate(()=>{const video=document.querySelector('#scan-video'),canvas=window.testCanvas,frame=canvas?.getContext('2d').getImageData(0,0,640,480);return {hidden:document.hidden,fixtureError:window.testCameraFixtureError,previewHidden:document.querySelector('#scan-preview').hidden,status:document.querySelector('#scan-status').textContent,ready:video.readyState,width:video.videoWidth,time:video.currentTime,paused:video.paused,active:video.srcObject?.active,sourceDecodes:frame&&!!window.jsQR(frame.data,640,480),error:document.querySelector('#auth .dialog-error').textContent}})))});

if(!await page.evaluate(()=>window.testStreams.every(s=>s.getTracks().every(t=>t.readyState==='ended'))))throw Error('Camera leaked after pairing');
if(!await page.evaluate(()=>!!localStorage.getItem('xenia-device-token')))throw Error('No device credential');
await page.reload();await page.getByText('Connected',{exact:true}).waitFor();if(process.env.XENIA_CAMERA_CONNECTED_SCREENSHOT)await page.screenshot({path:process.env.XENIA_CAMERA_CONNECTED_SCREENSHOT});
await page.getByRole('button',{name:'Sign out',exact:true}).click();await page.getByRole('button',{name:'Scan QR code',exact:true}).waitFor();if(process.env.XENIA_CAMERA_LOGIN_SCREENSHOT)await page.screenshot({path:process.env.XENIA_CAMERA_LOGIN_SCREENSHOT});
if(errors.length)throw Error(errors.join('; '));console.log('QR camera decoding, denied permission, stop/late-stream cleanup, device pairing and persistence: PASS');
}finally{if(browser)await browser.close();if(server.exitCode===null&&server.signalCode===null){server.kill();await new Promise(r=>server.once('exit',r))}fs.rmSync(root,{recursive:true,force:true})}
})().catch(e=>{console.error(e);process.exitCode=1});
