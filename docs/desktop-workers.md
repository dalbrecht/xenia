# Desktop workers

Xenia's worker engine can be controlled through native desktop applications on macOS and Windows. The app starts the worker and, if configured, an SSH tunnel to the controller. **Stop worker** stops accepting new work and waits for active jobs to finish; the app stays open while it drains. Cancel an individual job in the web interface if it needs to stop sooner. Pending reports remain in the worker database for the next start if the controller is unavailable.

These applications are early builds. The macOS app has been built and tested with a synthetic local controller, including connection and start/stop controls. A staged release bundle was built, replaced, and ad-hoc signature-verified in a disposable checkout; its bundled engine passed synthetic Codex and Claude control integration tests. The Windows WPF application has been cross-built, but the Windows engine, SSH integration, installation, and runtime have not yet been tested on a Windows machine. The macOS build is ad-hoc signed but not Developer ID signed or notarized; the Windows package is unsigned, and there is no automatic updater or operating-system service. Do not treat these as release-ready installers.

## Prerequisites on each worker

1. Install Git, Rust/Cargo, and at least one of Codex CLI or Claude Code; authenticate the runner as the worker user. Confirm that `git`, `codex`, and/or `claude` work in a terminal. Runner credentials stay on the worker.
2. Prepare Git checkouts at `<projects root>/<project>/<repository>`, each with a commit. Use disposable projects until the remaining recovery and security work is complete.
3. In the controller's web app, choose **Nodes → Enroll worker**, enter a unique ID, and copy its one-time credential. Share the credential securely with the worker owner. Do not use the controller token as a worker credential.
4. For a remote worker, install and configure the OpenSSH client, verify the controller's SSH host key, and configure key-based, noninteractive login before using the desktop app. Test it in a terminal first. The app starts `ssh` with `BatchMode=yes`; it cannot answer a password, key-passphrase, or host-trust prompt. SSH access and port forwarding must be permitted on the controller. The SSH destination uses normal SSH client configuration, including a configured username and key.

The app displays **Running** when the worker process is running; check the controller's Nodes page to confirm that the node is actually online. Startup and connection errors are written to the worker log. Release scripts remap common local build paths; the macOS script also strips debug metadata and checks its executables for the builder's home and source paths. Review artifacts before public distribution. The equivalent Windows path-remapping and packaging still need verification on Windows.

If an SSH tunnel exits, the desktop app reports the disconnect and retries with increasing delays, including while the worker is stopping and delivering queued reports. SSH keepalives help detect a stalled connection. The worker continues running while disconnected and persists reports for delivery after reconnection; an offline worker cannot receive cancellation requests. Reconnection does not undo lease takeover or automatically resolve work marked unknown. The macOS retry behavior has been exercised with fake processes; Windows runtime verification remains outstanding.

Both desktop apps have separate **Interactive Codex approvals** and **Interactive Claude questions and approvals** settings. They are off by default and take effect when the worker next starts. These transports are experimental: test only with disposable repositories, and review [their current limitations](approval-protocol.md) before enabling them. The Claude Bash approval display cannot determine the shell's current directory. Native Windows behavior has not yet been verified.

## macOS

1. Install the Xcode command-line tools and Rust. From the source repository on the Mac, run:

   ```sh
   ./scripts/build-macos-app.sh
   ```

2. The app is `dist/XeniaWorker.app`. Move or copy it to a user-controlled Applications folder if desired, taking care not to overwrite an existing installation. The build is ad-hoc signed but not Developer ID signed or notarized; distribution signing and Gatekeeper-ready packaging are not yet provided.
3. Open **Xenia Worker**. Enter the enrolled **Node ID**, a display name, projects directory, and credential. The credential is saved in macOS Keychain; other settings are stored in user preferences. The bundled engine stores its database and log in `~/Library/Application Support/XeniaWorker/`.
4. For a controller on the same Mac, leave **SSH host** blank and use local controller port `8080`. For a remote controller, enter its SSH destination and a free local port such as `18080`; the tunnel forwards that local port to the configured controller port on the SSH host, `8080` by default.
5. If the agent executable is outside the usual user and Homebrew paths, put its directory in **Additional runner PATH**. Select **Start worker**. The menu-bar item also provides start and stop controls once configuration is saved. Select **Stop worker** and wait for the status to become stopped before quitting.

Both the app build script and `./script/build_and_run.sh --verify` refuse to rebuild while a Xenia Worker process is running or if they cannot check its status. The build stages and signs a replacement before publishing it; if publishing fails it attempts to restore the previous bundle. Stop active work and quit the worker before rebuilding.

For local development, `XENIA_SIGNING_IDENTITY` can select an existing code-signing identity when running the macOS build script. Keep that value in your local environment, outside the repository. Consistently signing updates with the same trusted identity and bundle identifier can preserve Keychain recognition; changing from ad-hoc signing may require another approval. A modified executable cannot reuse its previous signature. The default remains ad-hoc signing, and this option does not notarize the app or guarantee Gatekeeper acceptance.

## Windows

1. Transfer a copy of the source tree to Windows over a trusted channel. From the controller source tree, `./scripts/make-source-archive.sh` creates `dist/xenia-source.tar.gz` without build output, databases, or the Git directory. Extract it on Windows. Install Git for Windows, Rust using the Windows MSVC toolchain, Visual Studio C++ Build Tools, the .NET 8 SDK, and the desired agent CLI(s). OpenSSH Client must be installed and available on `PATH` for a remote controller. Verify the setup in PowerShell.
2. From the source repository in PowerShell, build and install the x64 app:

   ```powershell
   .\scripts\build-windows-app.ps1
   .\scripts\install-windows-app.ps1
   ```

   The build creates `dist\windows-worker`; the installer copies it to the current user's Local AppData Programs folder and creates a Start-menu shortcut. It does not require administrator rights. It replaces files in that destination; stop the existing app before updating. The current script targets `win-x64` and is not tested on Windows ARM. The package is unsigned.
   To exercise the native Windows command launcher with a disposable fake runner, install Python 3 and run `cargo build` followed by `python tests\windows_runner.py`. This checks prompt delivery and follow-ups without consuming agent credits; it does not replace a test with the installed Codex or Claude CLI.
3. Open **Xenia Worker** from Start. Enter the enrolled node ID, display name, projects root such as `C:\Users\<user>\projects`, capacity, and one-time worker credential. The credential is saved in Windows Credential Manager. Settings, worker database, and `worker.log` are in `%LOCALAPPDATA%\XeniaWorker`.
4. For a controller on another machine, enter its SSH destination and a free local port such as `18080`. The app forwards that port to the configured controller port on the SSH host, `8080` by default. Verify the host key and key-based SSH login separately before starting; the app will not display interactive SSH prompts.
5. Select **Start worker**, then check Nodes in the web app. To turn it off, select **Stop worker** and wait for it to finish active jobs. Closing the window while it is running requests the same graceful stop.

The Windows worker uses a Windows job object for process-tree cancellation. A small helper waits for job assignment before starting the runner, so the runner's normal child processes inherit the job. Prompts are sent over standard input rather than passed through the command line, including when an installed runner uses a `.cmd` launcher. Its behavior with installed agent versions, Windows job policies, and shell-script launchers must be verified on the target machine before production use.

## Recovery and security

If the app or computer is killed instead of stopped normally, an agent may outlive its supervisor on some platforms. The controller holds the node lease for up to 30 seconds after the last heartbeat; after takeover it marks active work uncertain and prevents the previous instance from claiming more work. Inspect and stop any surviving processes before resolving the job or starting follow-up work. Avoid running two worker instances with the same node ID or database: the lease protects claims, but it does not stop an already running agent. The database is bound to the configured node ID. To change identities, first stop the worker and ensure its pending reports have reached the controller, then retain the old database as a backup and configure a separate database for the new node. The desktop apps use a fixed database location, so move the old database and its SQLite WAL and shared-memory files together before starting the new identity; do not move them while the worker is running.

Both apps place the worker credential in the child process environment while it runs. Protect the operating-system account and logs. SSH encrypts the cross-machine connection but local processes on either endpoint remain trusted. The controller is single-user and its browser token gives full operator access. See [installation and usage](installation.md) for the controller, HTTPS proxy example, detailed limitations, and backup guidance.
