// PWA caching checks for a trusted HTTPS or loopback origin. Requires Playwright.
const browserName = process.env.XENIA_TEST_BROWSER || 'chromium';
const browserType = require('playwright')[browserName];

(async () => {
  const url = process.env.XENIA_HTTPS_TEST_URL;
  const executablePath = browserName === 'chromium' ? process.env.XENIA_CHROMIUM_BIN || undefined : undefined;
  const selfSigned = process.env.XENIA_HTTPS_TEST_SELF_SIGNED === '1';
  if(selfSigned && browserName !== 'chromium')throw Error('The disposable CA fixture requires Chromium hostname mapping; use offline_pwa.cjs or a trusted HTTPS URL for WebKit.');
  const browser = await browserType.launch({headless:true, executablePath, args:selfSigned && browserName === 'chromium' ? ['--host-resolver-rules=MAP xenia.example.internal 127.0.0.1', '--ignore-certificate-errors'] : []});
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
    const manifestPath = await page.locator('link[rel="manifest"]').getAttribute('href');
    const manifestResponse = await context.request.get(new URL(manifestPath,url).href);
    if(!manifestResponse.ok())throw Error('Manifest unavailable');
    const manifestBody = await manifestResponse.json();
    if(manifestBody.start_url!=='/' || manifestBody.display!=='standalone' || !manifestBody.icons?.some(icon=>icon.sizes==='512x512'))throw Error('Invalid install manifest');
    if(browserName === 'chromium') {
      const client = await context.newCDPSession(page);
      const manifest = await client.send('Page.getAppManifest');
      if (!manifest.url || manifest.errors.length) throw Error('Manifest errors: ' + JSON.stringify(manifest.errors));
    }
    await context.setOffline(true);
    await page.reload({waitUntil:'domcontentloaded'});
    if (!await page.getByRole('heading', {name:'Conversations', exact:true}).count()) throw Error('Offline shell did not load');
    if (errors.length) throw Error('Browser errors: ' + errors.join('; '));
    console.log(browserName + ': secure context, install manifest, service worker and offline shell: PASS');
  } finally { await browser.close(); }
})().catch(error => { console.error(error); process.exitCode = 1; });
