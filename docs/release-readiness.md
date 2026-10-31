# First release readiness

Decision: **not ready to declare v1**. This assessment follows the scope in
[the product brief](product-brief.md); experimental features are not evidence of
supported release behavior. No public release tag has been created.

| Acceptance criterion | Current evidence | Remaining verification |
| --- | --- | --- |
| Phone submission and progress | Browser interaction checks at a 390 px viewport; HTTPS proxy and service-worker tests; installed private HTTPS endpoint passed with certificate validation; operator confirmed the login screen opens in iPhone Safari without a certificate warning | Install on a real phone over the intended private network, submit work, follow output, reconnect, and follow up |
| Isolated concurrent worktrees | Synthetic integration tests exercise parallel work and preserve the main checkout; installed macOS runner checks preserve checkout state | Repeat with native Windows runners and the intended multi-node deployment |
| Capacity and explicit node assignment | Controller tests and local integration exercise capacity, offline assignment, and checkout eligibility | Exercise two physical nodes, including lowering capacity while jobs run |
| Durable history and recovery | Controller-restart, lost-response, outbox replay, and instance-fencing tests; uncertain work requires explicit inspection | Native Windows crash and restart checks; document any platform-specific process survival |
| Runner and checkout eligibility | Native macOS worker reports both runners and validates registered checkouts | Native Windows discovery, including installed CLI launchers and authentication |
| Cancellation and explicit cleanup | Synthetic process-supervision tests, dirty-worktree refusal, retained branches; real macOS Claude waiting-run cancellation | Native Windows process-tree cancellation and cleanup; real-phone cancellation |
| Both runners complete work | Real macOS Codex and Claude initial conversations, follow-ups, and disposable edit checks; experimental approval transports tested separately | Native Windows real-provider tasks and supported-version compatibility |

## Operational gates

- Native apps: macOS release compilation, staging, rollback simulation, ad-hoc
  signature verification, and installed start/stop checks have passed. Windows
  WPF cross-compilation has passed; native build, installation, credentials,
  tunnel recovery, and runtime checks are outstanding.
- Private connections: the worker uses loopback HTTP through SSH for a remote
  controller. Local fake-process tests cover macOS tunnel retry and shutdown.
  A real cross-machine SSH connection still needs verification.
- Interactive transports: opt-in in the desktop apps. Codex command approvals
  and Claude Bash approvals/questions have narrow real-provider evidence on
  macOS. Unsupported requests remain explicit or deny-only. Keep their limits
  in [the protocol notes](approval-protocol.md) visible until compatibility is
  established.
- Resource use: [the existing measurements](performance.md) are individual idle
  observations. Desktop, active concurrency, longer histories, disk growth,
  and Windows measurements remain outstanding; they do not establish budgets.
- Source hygiene: AGPL-3.0-only license and package metadata are present. The
  local baseline uses generic author metadata. Source scans and archive checks
  exclude known machine-specific names, credentials, databases, and build output;
  this is not a comprehensive security or dependency audit.
- Installation and recovery: [the usage guide](installation.md) and
  [desktop guide](desktop-workers.md) describe current behavior and limitations.
  Controller upgrades retain a private backup; macOS packaging stages a signed
  replacement. The Windows installer still needs native upgrade verification.

## Evidence to record before changing this decision

Record platform and CLI versions, commands run, outcomes, and relevant failure
behavior for every outstanding check. Keep real machine names, repository data,
credentials, logs, and screenshots outside the public source tree. A passing
cross-build does not satisfy native runtime verification, and a viewport test
is not proof of phone installation. Update this document when evidence changes.
