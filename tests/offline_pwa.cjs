// Loopback is a trustworthy browser origin; this verifies caching, not TLS.
const {spawn}=require('node:child_process');
const fs=require('node:fs'),net=require('node:net'),os=require('node:os'),path=require('node:path');
(async()=>{
 const root=fs.mkdtempSync(path.join(os.tmpdir(),'xenia-offline-')),socket=net.createServer();await new Promise(r=>socket.listen(0,'127.0.0.1',r));const port=socket.address().port;await new Promise(r=>socket.close(r));
 const base=`http://127.0.0.1:${port}`,token='synthetic-offline-controller-token';
 const server=spawn(path.resolve('target/debug/xenia'),['controller'],{env:{...process.env,XENIA_TOKEN:token,XENIA_LISTEN:`127.0.0.1:${port}`,XENIA_DB:path.join(root,'db')}});
 try {
  let ready=false,last='not connected';
  for(let i=0;i<100;i++){try{const response=await fetch(base+'/api/workspace',{method:'POST',body:'{}',headers:{Authorization:'Bearer '+token,'Content-Type':'application/json'}});last=String(response.status)+' '+(response.ok?'':await response.text());if(response.ok){ready=true;break}}catch(error){last=error.message}await new Promise(r=>setTimeout(r,50))}
  if(!ready)throw Error('Controller did not start: '+last+' exit='+server.exitCode);
  const test=spawn(process.execPath,['tests/https_pwa.cjs'],{stdio:'inherit',env:{...process.env,XENIA_HTTPS_TEST_URL:base,XENIA_HTTPS_TEST_TOKEN:token,XENIA_HTTPS_TEST_SELF_SIGNED:'0'}});
  const code=await new Promise((resolve,reject)=>{test.once('error',reject);test.once('exit',resolve)});if(code!==0)throw Error('PWA offline test failed');
 }finally{if(server.exitCode===null&&server.signalCode===null){server.kill();await new Promise(r=>server.once('exit',r))}fs.rmSync(root,{recursive:true,force:true})}
})().catch(error=>{console.error(error);process.exitCode=1});
