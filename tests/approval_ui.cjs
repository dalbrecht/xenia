// Optional browser integration test. Requires Playwright and Chromium.
const { chromium } = require('playwright');
const { spawn } = require('node:child_process');
const { randomUUID } = require('node:crypto');
const fs = require('node:fs');
const net = require('node:net');
const os = require('node:os');
const path = require('node:path');

async function availablePort() {
  const server = net.createServer();
  await new Promise((resolve, reject) => server.once('error', reject).listen(0, '127.0.0.1', resolve));
  const port = server.address().port;
  await new Promise(resolve => server.close(resolve));
  return port;
}

(async () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'xenia-approval-ui-'));
  const port = await availablePort(), base = `http://127.0.0.1:${port}`;
  const token = 'synthetic-browser-approval-token-123456';
  const server = spawn(path.resolve('target/debug/xenia'), ['controller'], {env:{...process.env, XENIA_TOKEN:token, XENIA_LISTEN:`127.0.0.1:${port}`, XENIA_DB:path.join(root, 'db')}});
  let browser;
  async function api(route, body, auth = token) {
    const response = await fetch(`${base}/api/${route}`, {method:body ? 'POST' : 'GET', headers:{Authorization:'Bearer '+auth, 'Content-Type':'application/json'}, body:body ? JSON.stringify(body) : undefined});
    if (!response.ok) throw Error(`${route} returned ${response.status}`);
    return response.json();
  }
  try {
    let ready = false;
    for (let i=0; i<100; i++) { try { await api('state'); ready = true; break; } catch { await new Promise(resolve => setTimeout(resolve, 50)); } }
    if (!ready) throw Error('Controller did not start');
    const credential = (await api('workers/enroll', {id:'test'})).token;
    await api('heartbeat', {id:'test',name:'Test',capacity:1,runners:['codex'],instance_id:'instance'}, credential);
    const project = (await api('projects', {name:'example',repository:'repo'})).id;
    const job = (await api('jobs', {project_id:project,runner:'codex',prompt:'Review this task'})).id;
    await api('claim', {id:'test',instance_id:'instance',claim_id:randomUUID(),eligible:[project]}, credential);
    const approval = {node_id:'test',job_id:job,instance_id:'instance',request_id:randomUUID(),kind:'command',details:{command:'printf "<example>"',cwd:'/tmp/example',reason:'<img src=x onerror="window.injected=true">\u202e'}};
    await api('approval/request', approval, credential);
    browser = await chromium.launch({headless:true,executablePath:process.env.XENIA_CHROMIUM_BIN || undefined});
    const desktop = process.env.XENIA_TEST_VIEWPORT === 'desktop';
    const page = await browser.newPage({viewport:desktop ? {width:1440,height:900} : {width:Number(process.env.XENIA_TEST_WIDTH || 390),height:844}}), errors=[];
    page.on('pageerror', error => errors.push(error.message));
    await page.goto(base);
    await page.evaluate(value => sessionStorage.setItem('xenia-token', value), token);
    await page.reload();
    await page.getByText('awaiting approval', {exact:true}).waitFor();
    if (await page.evaluate(() => document.documentElement.scrollWidth > innerWidth)) throw Error('Layout overflows the viewport');
    await page.getByRole('button', {name:/Review this task/}).click();
    await page.getByRole('region', {name:'Command approval'}).waitFor();
    if (!(await page.locator('.approval pre').textContent()).includes('<example>')) throw Error('Command missing');
    if (!(await page.locator('.approval pre').textContent()).includes('\\u202e')) throw Error('Bidirectional control character was not exposed');
    if (await page.locator('.approval img').count() || await page.evaluate(() => !!window.injected)) throw Error('Approval details were interpreted as HTML');
    if (process.env.XENIA_TEST_SCREENSHOT) await page.screenshot({path:process.env.XENIA_TEST_SCREENSHOT,fullPage:true});
    await page.getByRole('button', {name:'Allow once'}).click();
    await page.getByText('Approved; waiting for the worker to acknowledge delivery.').waitFor();
    if ((await api('approval/poll', approval, credential)).decision !== 'accept') throw Error('Decision was not recorded');
    await api('approval/ack', approval, credential);
    await page.getByRole('region', {name:'Command approval'}).waitFor({state:'detached', timeout:12000});
    const incomplete = {...approval, request_id:randomUUID(), details:{command:'echo incomplete'}};
    await api('approval/request', incomplete, credential);
    await page.getByRole('region', {name:'Command approval'}).waitFor({timeout:12000});
    if (await page.getByRole('button', {name:'Allow once'}).isEnabled()) throw Error('Incomplete request can be approved');
    await page.getByRole('button', {name:'Deny'}).click();
    await page.getByText('Denied; waiting for the worker to acknowledge delivery.').waitFor();
    if ((await api('approval/poll', incomplete, credential)).decision !== 'decline') throw Error('Denial was not recorded');
    await api('approval/ack', incomplete, credential);
    await page.getByRole('region', {name:'Command approval'}).waitFor({state:'detached', timeout:12000});
    const tool = {...approval, request_id:randomUUID(), kind:'tool', details:{request:{subtype:'can_use_tool',tool_name:'Bash',input:{command:'printf example'}},working_directory:'Not provided by Claude Code'}};
    await api('approval/request', tool, credential);
    const toolCard = page.getByRole('region', {name:'Tool approval'});
    await toolCard.waitFor({timeout:12000});
    if (!await toolCard.getByText(/does not provide the shell’s current directory/).isVisible()) throw Error('Missing working-directory limitation');
    await toolCard.getByRole('button', {name:'Allow once'}).click();
    await page.getByText('Approved; waiting for the worker to acknowledge delivery.').waitFor();
    if ((await api('approval/poll', tool, credential)).decision !== 'accept') throw Error('Tool approval was not recorded');
    await api('approval/ack', tool, credential);
    await toolCard.waitFor({state:'detached', timeout:12000});
    const question = {...approval, request_id:randomUUID(), kind:'question', details:{questions:[{question:'Which format?',options:[{label:'Summary',description:'Short'}, {label:'Detailed',description:'Long'}],multiSelect:false},{question:'Which sections?',options:[{label:'Intro'},{label:'End'}],multiSelect:true}]}};
    await api('approval/request', question, credential);
    const card = page.getByRole('region', {name:'Agent question'});
    await card.waitFor({timeout:12000});
    await card.getByLabel('Which format?').fill('Custom format');
    await card.getByLabel('Which sections?').fill('Intro, End');
    if (await page.evaluate(() => document.documentElement.scrollWidth > innerWidth)) throw Error('Question form overflows the viewport');
    await page.waitForTimeout(2500);
    if (await card.getByLabel('Which format?').inputValue() !== 'Custom format') throw Error('Question draft lost during refresh');
    if (process.env.XENIA_QUESTION_SCREENSHOT) await page.screenshot({path:process.env.XENIA_QUESTION_SCREENSHOT,fullPage:true});
    await card.getByRole('button', {name:'Send answers'}).click();
    await page.getByText('Answered; waiting for the worker to acknowledge delivery.').waitFor();
    const result = await api('approval/poll', question, credential);
    if (result.decision !== 'answer' || result.answers['Which format?'] !== 'Custom format' || result.answers['Which sections?'] !== 'Intro, End') throw Error('Question answers were not stored');
    await api('approval/ack', question, credential);
    await card.waitFor({state:'detached', timeout:12000});
    const canceledQuestion = {...question, request_id:randomUUID()};
    await api('approval/request', canceledQuestion, credential);
    await card.waitFor({timeout:12000});
    await card.getByLabel('Which format?').fill('draft');
    await api('cancel', {id:job});
    await page.waitForTimeout(2500);
    await card.getByLabel('Which format?').blur();
    await card.waitFor({state:'detached', timeout:12000});
    if (errors.length) throw Error(errors.join('; '));
    console.log(`${desktop ? 'Desktop' : 'Mobile'} approval review, escaping, allow, acknowledgement, and deny-only incomplete request: PASS`);
  } finally {
    if (browser) await browser.close();
    if (server.exitCode === null && server.signalCode === null) {
      server.kill();
      await new Promise(resolve => server.once('exit', resolve));
    }
    fs.rmSync(root, {recursive:true,force:true});
  }
})().catch(error => { console.error(error); process.exitCode=1; });
