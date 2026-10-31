// Optional browser half of https_proxy.py. Requires Playwright and Chromium.
const { chromium } = require('playwright');

(async () => {
  const url = process.env.XENIA_HTTPS_TEST_URL;
  const executablePath = process.env.XENIA_CHROMIUM_BIN || undefined;
  const selfSigned = process.env.XENIA_HTTPS_TEST_SELF_SIGNED === '1';
  const browser = await chromium.launch({headless:true, executablePath, args:selfSigned ? ['--host-resolver-rules=MAP xenia.example.internal 127.0.0.1', '--ignore-certificate-errors'] : []});
  try {
    // The proxy test separately verifies the temporary CA with curl. Browser
    // certificate errors are ignored only in this disposable browser so the
    // test does not modify the host's trust store.
    const context = await browser.newContext({ignoreHTTPSErrors:selfSigned, viewport:{width:390,height:844}});
    const page = await context.newPage();
    const errors = [];
    page.on('pageerror', error => errors.push(error.message));
    await page.goto(url, {waitUntil:'networkidle'});
    await page.evaluate(token => sessionStorage.setItem('xenia-token', token), process.env.XENIA_HTTPS_TEST_TOKEN);
    await page.reload({waitUntil:'networkidle'});
    try { await page.waitForFunction(() => !!navigator.serviceWorker?.controller, null, {timeout:15000}); }
    catch (error) {
      const diagnostics = await page.evaluate(async () => {
        try { await navigator.serviceWorker.register('/sw.js'); return {secure:isSecureContext, registration:'requested'}; }
        catch (e) { return {secure:isSecureContext, registration:String(e)}; }
      });
      throw Error('Service worker did not take control: ' + JSON.stringify(diagnostics));
    }
    const details = await page.evaluate(async () => ({secure:isSecureContext, registration:!!await navigator.serviceWorker.getRegistration(), cache:(await caches.keys()).some(key => key.startsWith('xenia-shell-'))}));
    if (!details.secure || !details.registration || !details.cache) throw Error('HTTPS service worker or shell cache unavailable: ' + JSON.stringify(details));
    if (!await page.getByRole('heading', {name:'Conversations', exact:true}).count()) throw Error('Expected conversation screen');
    const client = await context.newCDPSession(page);
    const manifest = await client.send('Page.getAppManifest');
    if (!manifest.url || manifest.errors.length) throw Error('Manifest errors: ' + JSON.stringify(manifest.errors));
    await context.setOffline(true);
    await page.reload({waitUntil:'domcontentloaded'});
    if (!await page.getByRole('heading', {name:'Conversations', exact:true}).count()) throw Error('Offline shell did not load');
    if (errors.length) throw Error('Browser errors: ' + errors.join('; '));
    console.log('HTTPS secure context, PWA manifest, service worker and offline shell: PASS');
  } finally { await browser.close(); }
})().catch(error => { console.error(error); process.exitCode = 1; });
