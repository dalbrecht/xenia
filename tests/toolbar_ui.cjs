// Single-row app toolbar, accessible compact controls, bottom tabs and favicon.
const {spawn}=require('node:child_process'),fs=require('node:fs'),net=require('node:net'),os=require('node:os'),path=require('node:path');
const type=require('playwright')[process.env.XENIA_TEST_BROWSER||'chromium'];
(async()=>{
 const root=fs.mkdtempSync(path.join(os.tmpdir(),'xenia-toolbar-')),socket=net.createServer();await new Promise(r=>socket.listen(0,'127.0.0.1',r));const port=socket.address().port;await new Promise(r=>socket.close(r));
 const base=`http://127.0.0.1:${port}`,token='synthetic-toolbar-test-token';let browser;
 const server=spawn(path.resolve('target/debug/xenia'),['controller'],{env:{...process.env,XENIA_TOKEN:token,XENIA_DB:path.join(root,'db'),XENIA_LISTEN:`127.0.0.1:${port}`}});
 const api=async(route,body={})=>{const r=await fetch(base+'/api/'+route,{method:'POST',headers:{Authorization:'Bearer '+token,'Content-Type':'application/json'},body:JSON.stringify(body)});if(!r.ok)throw Error(route+': '+r.status);return r.json()};
 try{
  for(let i=0;i<100;i++){try{await api('workspace');break}catch{}await new Promise(r=>setTimeout(r,50))}
  await api('projects',{name:'example',repository:'repo'});
  browser=await type.launch({headless:true});const page=await browser.newPage({serviceWorkers:'block',viewport:{width:390,height:844}}),errors=[];page.on('pageerror',e=>errors.push(e.message));
  await page.goto(base);await page.getByLabel('Controller token').fill(token);await page.getByRole('button',{name:'Connect',exact:true}).click();
  const icon=await page.locator('link[rel="icon"]').getAttribute('href');const r=await page.request.get(new URL(icon,base).href);const bytes=await r.body();
  if(!r.ok()||!r.headers()['content-type'].includes('image/x-icon')||bytes.readUInt16LE(0)!==0||bytes.readUInt16LE(2)!==1||bytes.readUInt16LE(4)<1)throw Error('Favicon not served as a valid ICO');
  for(const width of [320,390,768,1440]){
   await page.setViewportSize({width,height:844});
   for(const [view,title,action,dialog] of [['conversations','Conversations','New work','compose'],['queue','Work queue','New work','compose'],['nodes','Nodes','Connect worker','worker-dialog'],['projects','Projects','Add project','project-dialog']]){
    await page.locator(`nav [data-view="${view}"]`).click();await page.locator('#title').filter({hasText:title}).waitFor();
    const geometry=await page.evaluate(()=>{const h=document.querySelector('header.app-toolbar').getBoundingClientRect();return {height:h.height,width:innerWidth,overflow:document.documentElement.scrollWidth>innerWidth,buttons:[...document.querySelectorAll('.header-actions button')].map(b=>{const r=b.getBoundingClientRect();return {top:r.top,bottom:r.bottom,width:r.width,height:r.height,icon:!!b.querySelector('svg'),name:b.getAttribute('aria-label')}}),nav:document.querySelector('aside').getBoundingClientRect().toJSON()}});
    if(geometry.overflow||geometry.height>80||Math.max(...geometry.buttons.map(b=>b.top))-Math.min(...geometry.buttons.map(b=>b.top))>1)throw Error(`Toolbar wraps at ${width}: ${JSON.stringify(geometry)}`);
    if(geometry.buttons.some(b=>!b.icon||!b.name||b.width<(width<=700?44:40)||b.height<(width<=700?44:40)))throw Error('Missing icon/name or inadequate hit area');
    if(width<=700 && Math.abs(geometry.nav.bottom-844)>1)throw Error('Mobile navigation is not docked at the bottom');
    await page.getByRole('button',{name:action,exact:true}).click();await page.locator('#'+dialog).waitFor({state:'visible'});await page.locator('#'+dialog+' .close').first().click();
   }
   if(process.env.XENIA_TOOLBAR_SCREENSHOTS)await page.screenshot({path:path.join(process.env.XENIA_TOOLBAR_SCREENSHOTS,`xenia-toolbar-${process.env.XENIA_TEST_BROWSER||'chromium'}-${width}.png`),fullPage:true});
  }
  await page.getByRole('button',{name:'Pair phone',exact:true}).click();await page.locator('#pair-dialog').waitFor({state:'visible'});await page.locator('#pair-dialog .close').first().click();
  await page.getByRole('button',{name:'Sign out',exact:true}).click();await page.locator('#auth').waitFor({state:'visible'});
  if(errors.length)throw Error(errors.join('\n'));
  console.log('Toolbar one row at 320/390/768/1440px in all views; favicon, touch targets, navigation and primary/pair/sign-out actions: PASS');
 }finally{await browser?.close();if(server.exitCode===null){const exited=new Promise(r=>server.once('exit',r));server.kill();await exited;}fs.rmSync(root,{recursive:true,force:true})}
})().catch(e=>{console.error(e);process.exitCode=1});
