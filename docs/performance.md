# Resource-use baseline

A single local smoke measurement on macOS ARM64, using a release build and SQLite, sampled resident memory about two seconds after startup with `ps -o rss=`:

| Process | Idle resident memory |
| --- | ---: |
| Controller (current HTTP server) | 4,320 KiB |
| Worker (previous build) | 3,984 KiB |

These are separate single observations, not a budget or a cross-platform benchmark. No jobs were running. The numbers exclude the desktop app, browser, SSH tunnel, SQLite file cache outside the process, and Codex or Claude Code processes. Active-job memory, CPU, disk growth, long histories, and Windows resource use still need representative measurements.

The worker bounds its in-memory output channel and uses a durable disk outbox. The controller stores output in chunks and serves bounded pages so long conversations remain accessible without loading a whole turn into the browser. The compatibility output endpoint still returns a capped preview. The controller database, outbox and number of jobs are not yet subject to a retention policy or disk quota; monitor disk usage and protect stored output. Output discarded by older versions cannot be recovered.

## Current macOS ARM64 sample (2026-10-31)

Five one-second samples of the installed idle processes, with no active work,
produced these median resident sizes:

| Process | Median RSS |
| --- | ---: |
| Controller | 4,352 KiB |
| Worker engine | 3,680 KiB |
| Native desktop app | 55,872 KiB |

A separate disposable controller and worker ran one real Codex job and one real
Claude job concurrently, each instructed to respond without using tools or
changing files. Both completed successfully. Fifty samples at approximately
200 ms intervals observed these peaks:

| Process group | Peak sampled RSS |
| --- | ---: |
| Controller | 5,968 KiB |
| Worker engine | 5,440 KiB |
| Worker descendants combined | 1,424,432 KiB |

Descendants include the agent CLIs and their helpers. Summed RSS can count shared
pages multiple times; this is not unique physical memory. The sample excludes the
browser and desktop app from the active run and does not represent a long coding
session or establish a capacity budget. The earlier failed concurrency attempts
are not included as successful-workload baselines; they exposed a worktree-parent
creation race that was fixed before this sample.

To repeat the opt-in workload, build a release engine and run
`python3 scripts/measure-runner-resources.py` on macOS or a Unix host with `ps`.
Both CLIs must be installed and authenticated; the check consumes model usage.
Keep concurrency conservative until measurements on the target machines cover
representative coding tasks. Windows and longer-running workloads still need data.

## Synthetic long-history sample (2026-10-31)

A disposable release controller on macOS ARM64 held 480 jobs: 80 independent
roots and one 400-turn conversation. Each long-conversation turn had a 90,000-byte
prompt and 64,000 bytes of output. The SQLite file was 63,021,056 bytes. No agent
CLI or real project was used. Twenty request cycles were sampled using `ps` at
approximately 50 ms intervals, with a short pause between cycles.

The paged path fetched 25 list summaries, five full turns and the output page for
each displayed turn. Its workspace response was 14,805 bytes, median request-cycle
time was 14.6 ms, and peak sampled controller RSS was 11,232 KiB over 26 samples.
Before adding the compatibility-snapshot guard, requesting all metadata returned
36,316,783 bytes, took a median 124.4 ms per cycle, and reached a sampled peak of
542,704 KiB over 61 samples. The compatibility comparison fetched metadata only;
these timings are different request workloads rather than a general speed ratio.

The current compatibility endpoint rejects that oversized history before building
its response. The PWA uses the bounded APIs, and only keeps output for the displayed
turn page. Total persisted history and disk usage still grow; no retention policy
or disk quota is implied. RSS includes SQLite cache and allocator behavior, is not
a unique-physical-memory measurement, and sampling can miss peaks. This short
synthetic sample does not establish a long-running production or Windows budget.
Repeat the current paged sample and guard check with
`python3 scripts/measure-history-resources.py` after a release build.
