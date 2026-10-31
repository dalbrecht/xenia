# Development verification

User installations need the Rust binary and the selected agent CLIs, not Node.js
or browser-test dependencies. The checks below are for contributors and release
verification. Run them from the source repository with disposable fixtures.

## Controller and worker checks

```sh
cargo fmt --check
cargo test --locked
cargo build --locked
python3 tests/e2e.py
python3 tests/approval_api.py
python3 tests/history_api.py
python3 tests/pairing_api.py
```

The integration tests start temporary controllers and synthetic runners. They do
not require an authenticated provider or submit model requests. Native Windows
launcher checks are separate: run `python tests/windows_runner.py` on Windows
after building the debug engine. Windows installation rollback checks use
`powershell -NoProfile -File tests/windows_install.ps1`. The latter mocks
publication failures and, on Windows, additionally checks a real temporary shortcut.

## Mobile and desktop browser checks

Install Node.js 20 or newer and use the checked-in npm lockfile. Playwright is
pinned as a development dependency; browser downloads happen only during setup.

```sh
cargo build --locked
npm ci
npx playwright install chromium webkit
npm run test:browser
XENIA_TEST_BROWSER=webkit npm run test:browser
```

On Linux, use `npx playwright install --with-deps chromium webkit` when the host
needs browser system libraries. In PowerShell, select WebKit with
`$env:XENIA_TEST_BROWSER = 'webkit'` before `npm run test:browser`, then remove that
environment variable to return to Chromium. Browser availability depends on the
host platform; use the supported download reported by Playwright.

Each suite runs approval and question interactions at 390 and 320 pixels,
mobile/desktop sign-out and stale-response checks, one-use device pairing,
camera decoding and cleanup, and history navigation at 320 and 1440 pixels.
Network fault-injection checks disable service workers so request interception
is deterministic in both engines. A separate case keeps service workers enabled
and verifies installation assets and offline shell loading on a trustworthy
loopback origin; it makes no TLS claim. Fixtures use temporary databases and synthetic credentials, not your running
controller or real projects. The camera test substitutes a canvas video stream;
it never requests access to real camera hardware. No external QR service or
model provider is contacted by these tests.

The test runner stops on failure, has a three-minute timeout per case, and
terminates its test process group on Unix during cancellation or timeout. Each
individual test also closes its browser and controller in its cleanup path.
The GitHub Actions browser matrix runs the same commands separately for Chromium
and WebKit on Ubuntu. Local success does not establish that a hosted CI run has
passed; record that run separately when available.

WebKit browser checks provide Safari-engine evidence. They do not verify iPhone
camera permissions, Home Screen storage isolation, VPN connectivity, installation,
or mobile process suspension. Complete those steps on a real device using the
[installation guide](installation.md). Cross-compiling the WPF app similarly does
not replace the native Windows checks in the [desktop guide](desktop-workers.md).
Keep screenshots and logs containing real hostnames or project data outside the
public repository.
