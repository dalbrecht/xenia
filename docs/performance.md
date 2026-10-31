# Resource-use baseline

A single local smoke measurement on macOS ARM64, using a release build and SQLite, sampled resident memory about two seconds after startup with `ps -o rss=`:

| Process | Idle resident memory |
| --- | ---: |
| Controller (current HTTP server) | 4,320 KiB |
| Worker (previous build) | 3,984 KiB |

These are separate single observations, not a budget or a cross-platform benchmark. No jobs were running. The numbers exclude the desktop app, browser, SSH tunnel, SQLite file cache outside the process, and Codex or Claude Code processes. Active-job memory, CPU, disk growth, long histories, and Windows resource use still need representative measurements.

The worker bounds its in-memory output channel and uses a durable disk outbox. The controller stores output in chunks and serves bounded pages so long conversations remain accessible without loading a whole turn into the browser. The compatibility output endpoint still returns a capped preview. The controller database, outbox and number of jobs are not yet subject to a retention policy or disk quota; monitor disk usage and protect stored output. Output discarded by older versions cannot be recovered.
