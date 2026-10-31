# Node request and approval protocol

This flow authenticates a worker independently of network membership. A private
network/VPN supplies reachability; HTTPS supplies server identity and encryption.
The existing root or paired-phone operator credential authorizes approvals.
No account password, SSH key or controller admin credential is given to a worker.

## Enrollment lifecycle

1. The native app starts its bundled engine's `pair` command with the controller
   HTTPS origin, node ID and display name. The engine generates a 256-bit random
   credential locally and retains it in memory while the request is outstanding.
2. `POST /api/node-pairing/request` accepts `node_id`, `name`, `platform` and the
   SHA-256 credential digest as `proof`. It returns a random request `id`, an
   eight-character comparison `code`, `expires` and `poll_seconds: 3`. This
   endpoint needs no operator login. The request lasts five minutes.
3. Authenticated operators see active requests in `POST /api/workspace` and
   `POST /api/node-pairing/list`. They compare the code against the worker's
   screen. Names and platform strings are unverified labels, not identity proof.
4. `POST /api/node-pairing/decide` requires operator authorization and the request
   `id`, matching `code`, and `decision` (`approved` or `rejected`). Approval
   reserves the node ID. It cannot overwrite an existing credential or another
   approved request for that identity. A decision is final.
5. The worker polls `POST /api/node-pairing/poll` with `id` and its generated
   credential in the Authorization Bearer header. Before approval it receives
   `pending`. After approval, a transaction installs the credential digest and
   returns `completed` and `node_id`. No secret is returned over this API.
   Repeated polling can recover a lost completion response until request expiry;
   revocation prevents that recovery from reactivating a revoked credential.
6. The native app privately reads the engine's final JSON line and saves the
   credential to macOS Keychain or Windows Credential Manager. It then enables
   **Start worker**. Settings and logs do not contain the credential.

The engine's JSON-lines stdout is a private desktop integration channel: its final
line contains the credential. Do not pipe `xenia pair` into logs, paste its final
output into chats, or treat it as a user-facing key exchange command.

`POST /api/node-pairing/cancel` requires the same proof-of-possession credential.
It cancels pending or approved requests before activation. Desktop **Cancel
request** uses a stop file so the engine can notify the controller. Closing or
killing the app unexpectedly may leave the request visible until expiry. An
expired, rejected or cancelled request cannot activate credentials. Start a fresh
request to retry. After activation, use worker revocation rather than cancellation.
If OS secret-store saving fails, the app retains the credential in its in-memory
field so starting the worker can retry saving; do not close it before retrying or
revoke the enrollment before requesting a replacement.

Workers with credentials appear in **Nodes** immediately after enrollment, even
before their first heartbeat, with **Never connected** status. An unused manual
enrollment can therefore be revoked there before requesting access again. If the
desktop already has a saved credential, start the worker instead of enrolling
again. Capacity controls become available after its first connection.

## Bounds and recovery

SQLite stores only request/credential digests, comparison codes and bounded
metadata. Requests persist across controller restarts. Changing the controller
root credential invalidates unfinished requests; existing worker credentials
retain their existing explicit revocation lifecycle. Expired requests are pruned
on successful protocol operations. At most 32 unfinished requests and 128 total
unexpired requests are accepted. Metadata fields are bounded, and digests must be
64 lowercase hexadecimal characters. Proofs and installed credential digests
are unique. Global limits bound resource use; they are not a substitute for
restricting endpoint access to the intended private network.

## Transport and compatibility

Both native apps accept a controller HTTPS origin. The Rust worker verifies the
server certificate and hostname using rustls and bundled public CA roots. It
refuses redirects, credentials in URLs, paths, queries and fragments. Requests
have a total timeout and bounded response bodies; one shared client reuses
connections. Custom CA settings and certificate-verification bypasses are not
provided. Configure your HTTPS proxy to forward Authorization unchanged and
never log it. Tailscale Serve can proxy to the controller's loopback listener.

The raw controller listener remains loopback-only. Plain HTTP is accepted by the
worker only for loopback diagnostics/local workers. A blank desktop controller
URL selects the old local-port/optional-SSH configuration, preserving existing
installations. A supplied HTTPS URL takes precedence and starts no SSH tunnel.
The manual enrollment API remains available for legacy/headless setup.
