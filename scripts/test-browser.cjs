// Sequential, isolated browser regressions. Requires a built debug engine.
const {spawn,spawnSync}=require('node:child_process');
const path=require('node:path');
const browser=process.env.XENIA_TEST_BROWSER||'chromium';
if(!['chromium','webkit'].includes(browser))throw Error('XENIA_TEST_BROWSER must be chromium or webkit');
const root=path.resolve(__dirname,'..');let active;
function stop(child){
  if(!child?.pid)return;
  try{
    if(process.platform==='win32')spawnSync('taskkill',['/PID',String(child.pid),'/T','/F'],{stdio:'ignore'});
    else process.kill(-child.pid,'SIGTERM');
  }catch(error){if(error.code!=='ESRCH')throw error;}
}
for(const [signal,code] of [['SIGINT',130],['SIGTERM',143]])process.once(signal,()=>{stop(active);process.exit(code)});
const cases=[
 ['approval_ui.cjs',{XENIA_TEST_WIDTH:'390'}],
 ['approval_ui.cjs',{XENIA_TEST_WIDTH:'320'}],
 ['signout_ui.cjs',{}],
 ['signout_ui.cjs',{XENIA_TEST_VIEWPORT:'desktop'}],
 ['pairing_ui.cjs',{}],
 ['camera_pairing.cjs',{}],
 ['offline_pwa.cjs',{}],
 ['history_ui.cjs',{XENIA_TEST_WIDTH:'320'}],
 ['history_ui.cjs',{XENIA_TEST_WIDTH:'1440'}],
 ['worktree_status_ui.cjs',{XENIA_TEST_WIDTH:'320'}],
 ['worktree_status_ui.cjs',{XENIA_TEST_WIDTH:'1440'}],
];
(async()=>{
 for(const [file,options] of cases){
  console.log(`\n${browser}: ${file} ${JSON.stringify(options)}`);
  let timedOut=false;
  const child=spawn(process.execPath,[path.join(root,'tests',file)],{cwd:root,env:{...process.env,XENIA_TEST_BROWSER:browser,XENIA_TEST_WIDTH:'390',XENIA_TEST_VIEWPORT:'mobile',...options},stdio:'inherit',detached:process.platform!=='win32'});active=child;
  const timer=setTimeout(()=>{timedOut=true;stop(child)},180_000);
  try{
   const code=await new Promise((resolve,reject)=>{child.once('error',reject);child.once('exit',resolve)});
   if(timedOut||code!==0)throw Error(`${file} ${timedOut?'timed out':'failed (exit '+code+')'}`);
  }finally{clearTimeout(timer);if(process.platform!=='win32')stop(child);active=null;}
 }
 console.log(`\n${browser}: all ${cases.length} browser checks passed`);
})().catch(error=>{console.error(error.message);process.exitCode=1});
