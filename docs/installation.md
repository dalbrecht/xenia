# Installation and usage (prototype)

Xenia is not yet a production-ready first release. This guide describes the current prototype and its limitations, so it can be evaluated with disposable repositories. The controller and workers permit HTTP on loopback only. Use an authenticated SSH tunnel for a remote worker. Each worker has a separate revocable credential; SSH encrypts traffic between machines. Local processes on each endpoint remain in the trust boundary. Do not expose the controller directly to a network.

## Requirements

- A machine for the controller and at least one worker. The controller and worker can be the same machine.
- Rust and Cargo, Git, and a recent browser. The prototype has been compiled on macOS; other operating systems have not been validated. Process-group cancellation is implemented for Unix.
- Codex CLI and/or Claude Code installed and authenticated **on each worker**, with their executables on the worker's `PATH`. Check `codex --version` and `claude --version`; authentication and permissions are configured with the respective CLI, not Xenia. The installed versions inspected during development were Codex 0.163.0 and Claude Code 2.1.304; initial prompts and follow-ups through Xenia succeeded with these versions in a disposable repository; broader compatibility is not yet verified. Xenia passes `--permission-prompts none`, which [requires Claude Code 2.1.259 or later](https://code.claude.com/docs/en/headless); older versions will reject that option.
- Existing Git checkouts, each with at least one commit, arranged as `<projects root>/<project>/<repository>`. Xenia does not clone or fetch repositories.

Use a disposable test checkout first. Agent runners can execute tools with the worker account's privileges and use its configured permissions. Review the runner's local settings before running work; Xenia does not request or manage interactive approvals. Codex runs in its workspace-write sandbox. Claude runs with its `acceptEdits` permission mode and permission prompts disabled, so actions still requiring a prompt are denied. The runners may still have access to files and credentials allowed by their own configuration and the worker account. Never put credentials in a job prompt.

## Build

From the repository directory:

```sh
./scripts/build-release.sh
cargo test
# Optional loopback integration test (uses a fake runner):
XENIA_TEST_BINARY=target/release/xenia python3 tests/e2e.py
# Optional event presentation tests (requires Node.js):
node --test tests/event-format.test.cjs
# Optional HTTPS proxy test (requires Caddy and curl; uses a temporary CA):
cargo build
python3 tests/https_proxy.py
```

The binary is `target/release/xenia`. Copy or build it on every worker machine. It contains the controller, worker, and web assets; it needs no separate JavaScript build. Linux and Windows builds have not yet been tested.

## Start the controller

From the repository on the controller machine, build the release binary and start it:

```sh
./scripts/build-release.sh
./scripts/run-controller.sh
```

On first launch the script uses OpenSSL to create a random token at `~/.local/share/xenia/controller-token` with a restrictive umask; it reuses that token on later launches and stores the controller database alongside it. It listens on `127.0.0.1:8080` by default. Keep the terminal open; stopping this process stops the controller. The data directory, database path, and listen address can be overridden with `XENIA_DATA_DIR`, `XENIA_DB`, and `XENIA_LISTEN`. Inspect the existing directory permissions if it was created previously.

### Run the controller at macOS login

Instead of keeping a terminal open, install the optional per-user LaunchAgent after building the release engine:

```sh
python3 scripts/macos-controller.py install
python3 scripts/macos-controller.py start
python3 scripts/macos-controller.py status
```

The service starts when this user logs in and launchd restarts it after an unexpected exit. It listens on `127.0.0.1:8080`, uses the same `~/.local/share/xenia` database and token, and writes diagnostic output to `~/.local/share/xenia/controller.log`. The installer copies the engine and launcher into that private directory; it does not store the token in the LaunchAgent. Do not also start the foreground controller on the same port. The LaunchAgent runs only in the logged-in user's GUI session; it is not a system service that runs before login.

To stop it for the current login, run `python3 scripts/macos-controller.py stop`. To upgrade, stop it, rebuild, run `install`, then `start`. To remove the LaunchAgent, run `python3 scripts/macos-controller.py uninstall`; the database, token, logs, and installed files are retained. A stopped but still installed agent can load again at the next login. The installer refuses to overwrite an unrelated agent or upgrade while the service is running. The optional `--data-dir`, `--agents-dir`, and `--listen` flags select an alternate private data directory, LaunchAgent directory, or loopback port; use the same values for later commands. The HTTPS proxy and worker configuration must use the selected controller port. Run `python3 tests/macos_controller_service.py` after building the release engine to exercise a separate temporary LaunchAgent, including crash recovery and cleanup.

Display the token locally when the browser asks for it:

```sh
cat ~/.local/share/xenia/controller-token
```

Do not share the token in chat or commit it to source control. Restrict database and directory permissions to the controller user. The database contains prompts, output, paths, and runner session IDs. Back it up using a SQLite-aware method while the service runs, or stop the controller first and copy the database and any associated WAL files together.

Open `http://127.0.0.1:8080/` on the controller machine and enter the token. It is stored in the browser tab's session storage. Select **Sign out** to clear the token and displayed workspace data from the current tab. Closing the tab also clears its session storage. For another device, see the HTTPS proxy example below. A PWA served over plain remote HTTP is not installable in most browsers.

## Access from a phone over HTTPS

### Existing Tailscale network

If the controller and phone already use the same Tailscale network, [Tailscale
Serve](https://tailscale.com/docs/features/tailscale-serve) can provide private
HTTPS while Xenia stays bound to loopback. Inspect `tailscale serve status` first
and choose an unused HTTPS port so another service is preserved. For a controller
on its default port, one example is:

```sh
tailscale serve --bg --https=8443 http://127.0.0.1:8080
```

Use the actual local controller port if it differs. Open the HTTPS URL printed by
Tailscale on the phone with Tailscale connected. Use QR pairing below to sign in
without copying the permanent controller token. This uses Serve within the tailnet;
public Funnel is not needed. To remove just this example listener later, use
`tailscale serve --https=8443 off`; do not reset other Serve services.

On iPhone, open the verified HTTPS URL in Safari, use **Share → Add to Home
Screen**, enable **Open as Web App** when offered, then add it. See
[Apple's installation instructions](https://support.apple.com/guide/iphone/turn-a-website-into-an-app-iphea86e5236/ios).
Launch Xenia from the new icon. Safari and the installed web app may use separate
storage; if it asks you to connect again, use **Scan QR code** inside the installed app as described below. Verify a conversation,
follow-up, cancellation, and reconnect on the actual phone before considering
mobile setup complete. A trusted Tailscale HTTPS deployment has passed the browser
manifest, service-worker, and offline-shell checks; that is not proof of iPhone
installation or interaction.

### Pair a phone with a QR code

1. On the controller computer, open the controller's **HTTPS URL** and sign in
   once with the controller token. Select **Pair phone**.
2. Confirm the controller HTTPS address, then select **Show QR code**. Opening
   Xenia using its HTTPS URL fills this address automatically. If using loopback
   HTTP on the computer, enter the phone-reachable HTTPS origin once; Xenia
   remembers it on that computer.
3. Connect the phone to the same private network or Tailscale. Scan the QR with
   the phone's Camera and open the link in Safari. Xenia signs this device in
   automatically. A QR code cannot install or configure the phone's VPN.
4. Use **Share → Add to Home Screen**, then launch Xenia from its icon. Verify
   conversations load. If the installed app asks for a login because it has
   separate storage, select **Scan QR code** inside the installed app, allow camera
   access, and scan a fresh code displayed on the controller. Camera frames are
   processed locally; only the one-use code is sent to your controller. The camera
   stops after decoding, when you select **Stop camera**, or when the app leaves
   the foreground. If camera access is unavailable, copy the **Single-use pairing
   link** to the phone and paste it under **Use a pairing link instead**. The link
   must point to that same controller origin.

Each QR/link expires after two minutes and works once. Generating another code
or closing the pairing dialog invalidates the outstanding code. Keep the dialog
open until the phone connects. The permanent controller token is never embedded
in the QR. Pairing grants full operator access, including running work and
managing workers; pair only devices you trust.

The paired device credential lasts 30 days and is stored in that browser's local
storage so it survives closing the app. **Sign out** clears it locally and asks
the controller to revoke it. If offline during sign-out, revoke it from another
connected device under **Pair phone → Paired devices → Revoke**. This list shows
pairing times; up to 32 active device credentials are supported. Revocation takes
effect on the next API request. Rotating the permanent controller token
invalidates all paired credentials and outstanding links. Clearing browser data
also removes the local login. The controller stores credential digests in its
private database; include that database in your usual backup policy.

### Caddy with a private certificate authority

A remote phone needs a secure origin for PWA installation. One possible deployment uses Caddy as an HTTPS reverse proxy on the controller machine while Xenia remains bound to loopback. A disposable loopback test with Caddy 2.11.7 verified the proxy and certificate chain; phone installation and network configuration still need verification on your devices. Follow the [Caddy reverse-proxy guide](https://caddyserver.com/docs/quick-starts/reverse-proxy) and [internal TLS documentation](https://caddyserver.com/docs/caddyfile/directives/tls), and restrict access to your private network with your firewall or VPN.

1. Give the controller a private DNS name that resolves from the phone and install Caddy using its official instructions.
2. Create a Caddyfile like the following, replacing the example name with your private DNS name:

   ```caddyfile
   {
       admin off
       auto_https disable_redirects
       skip_install_trust
   }

   xenia.example.internal:8443 {
       tls internal
       reverse_proxy 127.0.0.1:8080
   }
   ```

   Save this as a private `Caddyfile` outside the source tree, for example in a directory you control. `skip_install_trust` prevents Caddy from modifying the controller's trust stores; `disable_redirects` avoids opening a separate HTTP redirect listener. The site itself listens on available interfaces, so restrict port 8443 to the private network.
3. With the Xenia controller running, validate and start the proxy in a separate terminal, replacing the path with your Caddyfile:

   ```sh
   caddy validate --config /path/to/Caddyfile --adapter caddyfile
   caddy run --config /path/to/Caddyfile --adapter caddyfile
   ```

   Keep Caddy running. On macOS with the default data directory, its public root certificate is `~/Library/Application Support/Caddy/pki/authorities/local/root.crt`. If `XDG_DATA_HOME` is set or Caddy runs as another account, consult `caddy environ` in that same environment and look under its `caddy.AppDataDir`. Securely transfer only `root.crt` to the phone and install and trust it using the phone's certificate settings. On iPhone, manually installed roots also require enabling full trust under **Settings → General → About → Certificate Trust Settings**; see [Apple's instructions](https://support.apple.com/en-us/102390). Protect Caddy's data directory and private keys: a trusted CA can issue certificates for other names. Do not bypass certificate warnings.
4. Open `https://xenia.example.internal:8443` on the phone using your own hostname, sign in with QR pairing, and use the browser's install or add-to-home-screen action. Installation behavior depends on the browser and has not yet been verified on real devices. The cached offline shell can open without a network, but conversations and queue state require the controller. The connection indicator labels previously received data when the controller is unavailable and clears after a successful refresh.

The browser token grants full operator access. Keep it private. A reverse proxy exposes the login page to whichever network interfaces it listens on, so configure firewall or VPN rules deliberately. A production deployment still needs a full security review, operational hardening and verified mobile installation.

## Desktop worker apps

See the [desktop worker guide](desktop-workers.md) for native macOS and Windows setup and start/stop controls. The CLI instructions below remain useful for diagnostics and headless operation.

## Start a worker on the controller machine

In the web app, open **Nodes**, select **Enroll worker**, and enter a unique ID such as `worker-one`. Copy the worker credential shown once. Store it securely; it is distinct from the controller token. In a separate terminal, set `XENIA_WORKER_TOKEN` to that credential privately and run:

```sh
export XENIA_WORKER_TOKEN
export XENIA_WORKER_DB=/path/to/private/worker-one.db
export XENIA_CONTROLLER=http://127.0.0.1:8080
export XENIA_NODE_ID=worker-one
export XENIA_NODE_NAME='Worker one'
export XENIA_PROJECTS_ROOT="$HOME/projects"
export XENIA_CAPACITY=1
./target/release/xenia worker
```

The node ID must be stable and unique. Never run two worker processes with the same ID. The initial capacity is an integer from 1 to 64; after the node connects, use **Set capacity** in Nodes to change it. Lowering capacity does not stop existing jobs. The worker stores pending reports in the private worker database and replays them when the controller reconnects. Keep its database between restarts; choose a separate path per node, protect it, and back it up with the same care as the controller database. The database is bound to its node ID, and startup fails if configured for another node; use a separate database when changing identities. Existing databases are adopted only if every pending report identifies the configured node. The worker reports available executables and periodically checks that each checkout is a Git repository with a commit. The job composer lists unavailable nodes with reasons; a previously eligible offline node can be selected and its job will wait for it to reconnect. Keep its terminal open; service-manager installation and automatic restart are not yet provided.

## Connect a remote worker for evaluation

Keep the controller bound to loopback. From the worker machine, establish a verified SSH connection to the controller machine, forwarding a local port to the controller's loopback port:

```sh
ssh -N -L 127.0.0.1:18080:127.0.0.1:8080 controller-host
```

Replace `controller-host` with the SSH host you administer and verify its host key according to your SSH setup. In another terminal on the worker, use the worker instructions above with `XENIA_CONTROLLER=http://127.0.0.1:18080` and a different `XENIA_NODE_ID`. Keep the tunnel running. The credential and job traffic pass through the encrypted SSH tunnel. Enroll each worker separately and provision its own credential securely. Revoke a credential in Nodes if it is compromised; revocation also prevents that worker from reporting active work. Native TLS and automated enrollment are not implemented.

## Submit, inspect, follow up, and cancel

1. In **Projects**, add the project and repository directory names. Both names must be simple path components; the checkout must exist on the worker.
2. Select **New work**, choose the project, runner, and either automatic assignment or a specific node, then enter a task. The queue persists in SQLite.
3. Select a job to view its status, worktree path, branch, and output. Runner events are forwarded as lines arrive; the page polls for updates. The display extracts some agent messages from the runner's JSON output. Not all event types are presented yet.
4. Select **Cancel work** for a queued or running job. Queued jobs cancel immediately. A running job becomes `cancel_requested`; the connected worker attempts to terminate its process group on Unix or its job object on Windows. It reports `canceled` only after the runner has exited and its output pipes have closed. An offline worker cannot receive the request, and a request may race with normal completion. Xenia also terminates remaining supervised descendants when a runner exits normally; do not use an agent job to launch a service intended to outlive that job.
5. When a finished job has a captured session ID, select **Follow up** and enter a message. Xenia queues a new job on the original node and reuses the worktree and runner session. Only one queued or active follow-up per worktree is permitted. If the CLI cannot resume the session, the job fails; Xenia does not silently start a fresh session.

If Claude reports a denied permission, the conversation shows the denial. A job whose process otherwise succeeds is labeled **Needs review** because the requested action did not run. With the default runner mode, prompts are denied automatically; inspect the result and the runner's configured permissions before deciding on a follow-up. Experimental interactive prompts can be enabled as described in the [approval protocol guide](approval-protocol.md). Older stored jobs may show the denial in their conversation without the newer status label.

Each initial job creates a branch named `xenia/<job-id>` at the local checkout's current HEAD and a worktree under `<project>/.worktrees/<repository>/<job-id>`. The initial checkout is not intentionally modified. Worktrees are retained on success, failure, and cancellation. Review changes with ordinary Git tools. Use **Remove worktree** after reviewing it. Xenia asks the owning worker to run `git worktree remove` without force, so Git refuses to remove a dirty worktree. The branch is retained. Cleanup waits for an offline node; if a worker restarts during cleanup, inspect the path and use **Resolve cleanup** to record whether it was removed or retained.

## Known gaps and recovery

This is an experimental implementation, not a complete first release. A worker persists reports in a local outbox and retries delivery; the controller deduplicates report IDs. Claims also carry a durable request ID, so a worker can recover the same assignment if its response is lost, including after a controller restart. The controller grants each node identity to one worker instance at a time. A second instance waits until the current one stops cleanly or its 30-second heartbeat lease expires. On takeover, previously running jobs become `unknown`, hold their capacity, and are not automatically run again; the old instance cannot claim new jobs or cleanups. A partitioned worker may still be running an existing job, and can still deliver its outcome. Inspect the worker and stop any surviving agent and child processes before selecting **Resolve interrupted work**. A confirmed resolution changes the job to `interrupted`; a recorded session can then be followed up. A late report does not change an explicitly resolved state. If local storage fails, output may be incomplete and manual investigation is required. Update controller and worker binaries together: older workers do not support instance-fenced claims.

An assignment can still be uncertain if a worker dies around startup, and process inspection is manual; automatic supervision and complete crash reconciliation are not implemented. Processes can escape a Unix process group or a Windows job object; if an output pipe remains open after supervised cleanup, Xenia marks the job `unknown` and blocks follow-ups until inspection and resolution. An escaped process that closes its pipes may not be detected. On Windows a gated helper waits for assignment to the job object before it starts the runner; this path still needs native verification. The outbox and controller store potentially sensitive output and have no retention limit; monitor available disk space and protect their databases. A runner output line over 1 MB stops the job and is reported as a failure when storage remains available. The PWA loads 25 conversations or queue items per list page; use **Earlier work**, **Newer work**, and **Latest work** to navigate. Project and node filters search the full stored history. Opening a conversation loads five turns at a time; use **Earlier turns**, **Newer turns**, and **Latest turns** to revisit it. Worktree and follow-up actions apply to the latest turn even while reading earlier turns. An open item can remain pinned when new work moves it off the current list page. Full prompts are fetched with the turn page rather than the list summary. Conversation output is retained in the controller database and displayed in bounded pages; use **Earlier output**, **Newer output**, and **Latest output** to navigate. Older controller versions truncated output at approximately 2 MB: the upgrade preserves the existing marker, but cannot recover previously discarded text. Checkout status is cached briefly and can change after a check; the worker validates the checkout again when starting work. The prototype has no mobile installation verification, native TLS, or multi-user access control. Both installed CLIs completed synthetic file edits and follow-up edits; an earlier Codex attempt failed for a reason that was not captured. Streaming, permissions, cancellation, longer-running tasks, and other versions require broader real-provider testing. Avoid production data and do not rely on it for unattended work.

The controller limits simultaneous connections to 64 and concurrent database requests to four. It limits request headers to 16 KiB, request bodies to 1 MB, and gives headers and bodies each a 10-second total read deadline. A connection also has a 30-second lifetime. Once the connection limit is reached, further connections are closed until capacity is available, so restrict access to trusted devices and configure limits in any reverse proxy as well. These controls are not a substitute for a security review.

The controller and worker must be rebuilt and upgraded together when the worker protocol changes. In particular, workers built before durable claim IDs cannot claim jobs from this version; stop and rebuild each worker app before reconnecting it. Retain its worker database when updating so pending reports can be delivered.

To stop the prototype, interrupt the worker and controller processes. Stopping a worker does not guarantee that every child process has stopped; inspect the system's processes before restarting or manually cleaning a worktree. Retain the database and worktrees until their contents have been reviewed.

### History API compatibility

The PWA uses `POST /api/workspace` (`mode`, filters and optional `before` cursor)
and `POST /api/conversation-page` (conversation `id`, optional `before`, or
`single: true` for an individual queue job). Both return `jobs`, `has_more` and
`next_before`; conversation turns are ordered oldest to newest within each page.
Workspace summaries include total matching counts and node occupancy across all
stored jobs. Lists carry prompt previews; turn pages carry full prompts.

`GET /api/state` remains a compatibility endpoint for small histories. It returns
an explicit error above 200 jobs or 500,000 bytes of combined prompt, error,
session, worktree and branch metadata, directing the client to the paged APIs.
It never silently truncates a history. Worker scheduling and report endpoints are
unchanged. Back up the controller database before upgrading: the new history index
is rebuilt transactionally from existing parent relationships.
