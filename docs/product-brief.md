# Product brief

## Purpose

Xenia lets a person coordinate coding agents across several computers and projects through conversations, without managing terminal sessions. A controller presents an installable web app and coordinates work; a service on each worker machine runs and supervises agents in isolated Git worktrees.

## Initial user and environment

The initial release targets one operator on a trusted private network, using a phone or desktop browser to manage several projects and worker machines. Workers may have different capacity and different repositories available. The system should remain useful on modest machines and avoid unnecessary background resource consumption.

## First release

- Register projects and repositories, and discover or validate their local checkouts on worker nodes.
- Start and revisit conversations, choose Codex or Claude Code, submit work, and follow streamed progress and results.
- View queued, running, completed, failed, and canceled work; filter by project and node.
- Assign work to a specific eligible node or let the controller select one. Set a per-node limit on simultaneous jobs.
- Execute work in a dedicated Git worktree, preserve the resulting branch and worktree for review, and provide an explicit cleanup action.
- Show connected, disconnected, and unavailable nodes and explain why work cannot start.
- Provide an installable, responsive PWA with a first-class phone experience and usable desktop layouts.

## Acceptance criteria

1. From a phone, the operator can select a project and repository, choose a runner, submit work, and follow its progress without horizontal scrolling or a terminal.
2. Two jobs can run concurrently in separate worktrees without modifying the main checkout or one another's worktree. Their branches and paths are visible for review.
3. The queue respects each node's configured concurrency limit. A job explicitly assigned to an unavailable or full node waits for that node and is not silently reassigned.
4. Conversation history and job outcomes remain available after reconnecting or restarting the controller. Interrupted work has an explicit state and retains its worktree.
5. A node without the required checkout or runner is not offered as eligible, and the reason is visible.
6. The operator can cancel work and can explicitly remove a retained worktree after seeing its state; cleanup does not silently delete uncommitted changes.
7. Both supported runners can complete a basic task with progress and final outcome visible through the same interface. Runner-specific limitations are surfaced rather than hidden.

## Out of scope initially

Multi-user accounts and authorization, public-internet exposure, automatic repository cloning or synchronization, automatic merging, autonomous deletion of work, and support for every agent CLI are not required for the first release. Offline job submission and mobile push notifications are also deferred; an installed PWA need not work offline beyond a clear disconnected state.

## Principles and open decisions

Rust is the preferred foundation for the controller and worker where practical. Favor low idle memory, bounded resource use, and straightforward operation over unnecessary services. Measure resource usage rather than assuming a language choice alone guarantees efficiency.

The source is licensed under AGPL-3.0-only. Documentation, fixtures, examples, logs, screenshots, and defaults must use generic names and contain no personal paths, credentials, hostnames, or project data.

Open decisions include supported operating systems, deployment and enrollment details, runner authentication, the exact semantics of conversation continuation, retention limits, and whether projects may contain multiple repositories in a single job. The first release assumes one repository per job and locally provisioned runner credentials.
