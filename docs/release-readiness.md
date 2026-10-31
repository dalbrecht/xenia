# First release readiness

Decision: **not ready to declare v1**. This assessment follows the scope in
[the product brief](product-brief.md); experimental features are not evidence of
supported release behavior. No public release tag has been created.

| Acceptance criterion | Current evidence | Remaining verification |
| --- | --- | --- |
| Phone submission and progress | Chromium and WebKit interaction checks at phone and desktop widths; trusted HTTPS and offline-shell checks pass in both engines; operator confirmed QR login, successful Codex submission, a follow-up in the same conversation, retained login/history after closing and reopening, running-task cancellation, and Controller unavailable followed by Connected after network reconnection from the installed iPhone app | Phone walkthrough complete; continue regression checks as behavior changes |
| Isolated concurrent worktrees | Synthetic integration tests exercise parallel work and preserve the main checkout; installed macOS runner checks preserve checkout state | Repeat with native Windows runners and the intended multi-node deployment |
| Capacity and explicit node assignment | Controller tests and local integration exercise capacity, offline assignment, and checkout eligibility | Exercise two physical nodes, including lowering capacity while jobs run |
| Durable history and recovery | Controller-restart, lost-response, outbox replay, and instance-fencing tests; uncertain work requires explicit inspection; operator confirmed phone login/history survive closing and reopening | Native Windows crash and restart checks; document any platform-specific process survival |
| Runner and checkout eligibility | Native macOS worker reports both runners and validates registered checkouts | Native Windows discovery, including installed CLI launchers and authentication |
| Cancellation and explicit cleanup | Synthetic process-supervision tests, dirty-worktree refusal, retained branches; real macOS Claude waiting-run cancellation; operator confirmed running Codex work reaches Canceled and explicit cleanup reaches Succeeded from the installed iPhone app | Native Windows process-tree cancellation and cleanup |
| Both runners complete work | Real macOS Codex and Claude initial conversations, follow-ups, and disposable edit checks; experimental approval transports tested separately | Native Windows real-provider tasks and supported-version compatibility |

## Operational gates

- Native apps: macOS release compilation, staging, rollback simulation, ad-hoc
  signature verification, and installed start/stop checks have passed. A rebuilt
  installed bundle's engine also passed the standard synthetic integration suite,
  including concurrent worktrees, replay, cancellation and graceful shutdown; this
  CLI evidence is separate from desktop credential/launch verification. The
  updated native app also loaded its existing Keychain credential, started the
  bundled worker, rejoined the controller, and reported both supported runners
  with the registered checkout eligible. Windows
  WPF cross-compilation has passed; native build, installation, credentials,
  tunnel recovery, and runtime checks are outstanding.
- Browser regressions: a pinned development-only Playwright setup now defines thirteen
  cases per engine for Chromium and WebKit. The original nine-case suites passed locally,
  including 320 px approvals, history paging, camera decoding, sign-out and a
  separate service-worker offline-shell check. The added independent job/worktree
  state checks cover cleanup success, failure, retry and shared turn worktrees in
  Chromium and WebKit at 320 and 1440 px. GitHub Actions is configured to run
  the same matrix; hosted runs exposed Windows fixture cleanup and Linux
  WebKit synthetic playback issues, now addressed in the fixtures. A fully
  passing hosted matrix still needs confirmation. The installed trusted
  HTTPS endpoint also passed the WebKit PWA check with certificate validation.
  Node enrollment approve/reject journeys pass at 320 px in both engines and
  at 1440 px in WebKit, including dismissed approval and escaped device names.
- Private connections: direct HTTPS and node-initiated approval replace SSH as the
  normal remote-worker path. Protocol checks cover proof possession, approval,
  restart, expiry, cancellation, revocation and identity scoping; native Windows
  pairing, secret storage, connection and runtime checks remain outstanding.
  A disposable worker completed request/approval, activated its credential,
  appeared online, and stopped gracefully through the installed trusted HTTPS
  endpoint. Its credential was revoked and temporary enrollment removed.
  macOS subprocess checks cover code display, private credential receipt,
  cancellation and direct-URL precedence without SSH; Keychain pairing remains
  an operator check.
  Optional SSH remains supported; fake-process tests cover macOS tunnel retry.
- Interactive transports: opt-in in the desktop apps. Codex command approvals
  and Claude Bash approvals/questions have narrow real-provider evidence on
  macOS. Unsupported requests remain explicit or deny-only. Keep their limits
  in [the protocol notes](approval-protocol.md) visible until compatibility is
  established.
- Resource use: [the existing measurements](performance.md) include idle
  controller, worker and desktop RSS, plus a short concurrent real-runner sample.
  Representative coding sessions, longer histories, disk growth and Windows
  measurements remain outstanding; current samples do not establish budgets.
  A 400-turn synthetic history now verifies paged lists and turns, full-history
  filtering, and bounded output caching. The recorded paged controller sample
  peaked at 11,232 KiB RSS; oversized compatibility snapshots are rejected.
- Phone pairing: single-use two-minute QR links, separate 30-day device
  credentials, expiry, revocation, restart persistence and root-token rotation
  pass isolated API tests. Desktop and 320 px browser checks cover QR rendering,
  automatic login, persistence, sign-out and the installed-app link fallback.
  The operator confirmed iPhone Camera pairing succeeds and that the installed
  Home Screen app uses separate login storage. An in-app camera scanner now
  covers this case; synthetic camera checks cover QR decoding, denied permission,
  late-stream cleanup, stopping capture and persistent login. Actual in-app iPhone
  camera scanning still requires confirmation.
- Source hygiene: AGPL-3.0-only license and package metadata are present. The
  local baseline uses generic author metadata. Source scans and archive checks
  exclude known machine-specific names, credentials, databases, and build output;
  this is not a comprehensive security or dependency audit.
- Installation and recovery: [the usage guide](installation.md) and
  [desktop guide](desktop-workers.md) describe current behavior and limitations.
  Controller upgrades retain a private backup; macOS packaging stages a signed
  replacement. The Windows installer now stages and hashes package files,
  refuses a running app, and restores the previous app and shortcut on failure.
  Failure-injection tests on portable PowerShell cover copy, verification,
  publication and rollback errors, plus incomplete packages and lock contention.
  A cross-build validates the WPF shared-lock code; actual Windows upgrade and
  startup verification remain outstanding, including native shortcut checks in CI.

## Evidence to record before changing this decision

Record platform and CLI versions, commands run, outcomes, and relevant failure
behavior for every outstanding check. Keep real machine names, repository data,
credentials, logs, and screenshots outside the public source tree. A passing
cross-build does not satisfy native runtime verification, and a viewport test
is not proof of phone installation. Update this document when evidence changes.
