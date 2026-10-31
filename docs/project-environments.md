# Project assignment and development environments

A project can use Native, Container or Hybrid development. Assigning it to a node
prepares its code and environment before the node can claim work for that project.
Older projects without an environment remain in manual-checkout mode.

## Set permissions on each node

Stop the worker and edit **Local permissions** in its native desktop app:

- **Allow Codex** and **Allow Claude Code** control which adapters can start.
- **Allow automatic project checkouts** authorizes managed clones and fetches.
- **Allow repository setup scripts** authorizes the selected setup files.
- **Allow Docker environments and services** authorizes container and Compose use.
- **Allowed project directories** is a comma-separated list such as `example,tools`.
  Blank allows all project directory names. This applies to managed and manual projects.
- **Maximum concurrent workers** is the local hard ceiling, from 1 to 64. The
  controller can request a lower capacity but cannot increase it past this limit.

Checkout, setup-script and Docker permissions default off. Runner access defaults
on for compatibility; the corresponding CLI must still be available. Restart the
worker to apply changes. The management UI displays these settings read-only.
The worker checks its local settings before checkout, setup and agent startup,
even if a controller sends an assignment it should not have offered.

These are Xenia automation permissions, not an operating-system security sandbox.
Agent file access, network access and command permissions still follow the runner
and OS configuration. Setup runs as the signed-in worker user, without automatic
administrator elevation. Existing work can be stopped and its owned environment
cleaned up after permissions are disabled; disabling a permission does not kill
already-running work. Drain or cancel it before changing the node settings.

## Configure and assign a project

1. In **Projects**, add its project and repository directory names, for example
   `example` and `app`. Xenia uses `<projects-root>/example/app` on each node.
2. Select **Environment**. Enter a Git HTTPS URL without embedded credentials and
   choose a branch, tag or commit, with `HEAD` as the default. Authenticate Git on
   each worker beforehand for private repositories; credentials stay on the node.
3. Choose Native, Container or Hybrid and supply the optional recipe described below.
4. Select **Assign node**, then **Assign and prepare**. Assignment authorizes the
   selected repository and recipe within that node's local permissions. A node
   that is offline prepares after reconnecting; an old worker must be upgraded
   and report its local permissions before it can be assigned a managed project.
5. Watch **Queued → Preparing → Ready**, or inspect **Failed** and its latest
   setup output. The queue also shows active environment preparation. Retry is
   explicit. A setup interrupted by worker replacement becomes **Unknown**;
   inspect surviving processes before confirming a retry.

Xenia clones into a separate staging directory before publishing a missing
checkout. It validates an existing checkout's root and origin, and preserves its
working files and branch. It fetches the selected revision, resolves its commit,
and prepares an isolated detached worktree under
`<project>/.worktrees/<repository>/provision-<attempt-id>`. Failed clones and setup
worktrees are retained for inspection, with their paths reported in Projects.
There is no automatic deletion of those preparation worktrees.

Ready assignments pin the resolved commit for new jobs. Each job receives its
own worktree and environment setup. Follow-ups keep the same worktree, session
and original environment snapshot, even after a project environment is edited.
Native setup is not rerun for a follow-up. Environment edits require project work
to be idle and invalidate existing readiness until setup is approved again.

**Unassign** prevents further managed work on that node and keeps its checkout,
branches and worktrees. It refuses while setup or node-specific work is active
or uncertain. Explicitly assigned jobs must wait for a ready, capable node;
Xenia does not silently move them elsewhere.

## Native

Use Native for macOS/Windows GUI projects and tools that need the host SDKs.
Optional repository paths select a Windows PowerShell script and a macOS/Linux
shell script, for example `.xenia/setup.ps1` and `.xenia/setup.sh`. Paths must stay
inside the worktree. Scripts should be repeatable for fresh worktrees, install
without interactive prompts, and keep secrets out of output.

The scripts run once during assignment validation and once for each fresh job
worktree, so shared tools should detect existing installations and repository
packages should use their normal caches. They do not run again for follow-ups.
Use the node's installed package tools and user privileges. If a tool installation
changes the OS PATH, restart the worker before retrying. Xenia does not silently
install an administrator toolchain or override PowerShell execution policy.

## Container

Container mode runs the agent and development commands in a Linux Docker image.
The node still runs its native Xenia app; WSL is not a requirement of Xenia.
The chosen Docker engine must support Linux containers and bind mounts of the
node's projects directory. A remote daemon without a shared filesystem will
fail preparation rather than copying or silently running different code.

Provide an image tag or `@sha256:` digest. Install Docker separately and configure
registry authentication locally. Xenia uses a local image if present and pulls
it if absent; pin a digest for reproducibility. The image must support `sh`,
contain Git and the desired Codex/Claude CLI(s), and let its default user write
`/xenia-home`. The runner must also be enabled and discoverable on the native
node for its current capability reporting. An optional shell setup script runs
inside the container when the workspace container is first created.

A named container belongs to each worktree. The project directory is mounted so
Git's shared worktree metadata remains reachable; explicit in-container Git
paths avoid Windows/macOS host-path backlinks. This mount is writable and
includes the main checkout and sibling worktrees: do not treat this mode as
isolation from the project. Use only images and repositories you trust.

Container home/session files remain in the container across follow-ups. Xenia
stops it between turns to reduce idle memory, restarts it for the next turn, and
removes it during explicit worktree cleanup. Worker crashes may leave a container
running; inspect containers labelled `org.xenia.managed=true` before resolving
uncertain work. Failed shutdown leaves the job **Unknown** rather than claiming
cancellation or completion was fully supervised.

No host home directory or Xenia credential is mounted into the image. Existing
`OPENAI_API_KEY` and `ANTHROPIC_API_KEY` environment variables on the worker are
forwarded if present. Configure the runner's authentication in the image/setup
recipe, including any required login step; native OS login state is not copied.
Scripts must not print secrets. Interactive experimental runner adapters are
currently native-only and explicitly fail for Container projects.

## Hybrid

Hybrid runs the agent and native setup on the host, with Docker Compose services
from the selected repository file, for example `compose.yaml`. It fits native GUI
work with a containerized database or backend. Xenia uses a separate Compose
project name per worktree, runs `up --detach --wait`, and stops services between
turns with `down`. A failed readiness/health check blocks work. Use a current
Compose implementation supporting `--wait`.

Named volumes are preserved; cleanup does not issue `down --volumes`. Review and
remove retained data explicitly when appropriate. Compose resources and mounts
are part of the trusted project recipe. Containerized GUI support does not imply
support for native Windows/macOS desktops inside Linux containers.

## Operational bounds and verification

One setup attempt at a time can run on a node, and it occupies a concurrency slot.
Other projects may use remaining slots. Commands have bounded output buffers,
a 20-minute deadline, noninteractive Git prompting, and process-tree supervision.
The controller stores the latest 16 KiB of setup output; workspace summaries
include the latest 2 KiB. Output may contain repository paths or script data and
belongs in private runtime databases, not public issues or source.

Protocol and recovery checks are in `tests/project_assignment_api.py`. Native
lifecycle checks are in `tests/project_environments.py`; set
`XENIA_TEST_DOCKER=1` for the real Docker/Compose fixture. Set `XENIA_TEST_ROOT` to
a writable directory already shared with Docker when system temp is not shared.
These fixtures use synthetic runners, not provider requests. Real Windows
Codex/Claude and installed desktop UI acceptance remain separate checks.
