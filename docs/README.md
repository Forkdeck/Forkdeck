# Forkdeck documentation

[Front page and five-minute quick start](../README.md)

## Setup and daily use

- [Install](install.md) — Cargo, Linux archives, checksums and prerequisites.
- [Configuration](configuration.md) — global config and settings from the base branch.
- [How a job runs](jobs.md) — routing, work, checks and review.
- [Agents and GitHub Copilot](agents.md) — stand-ins, profiles and model availability.
- [Routing](routing.md) — rules, route memory and local Jev setup.
- [Terminal controls](terminal.md) — keys, conversations, history and resuming.
- [Planning](planning.md) — approaches first and issue-backed plans.
- [Plans and sub-issues](sub-issues.md) — task lists, dependencies and scope.
- [Issues and pull requests](issues-and-prs.md) — labels, worktrees, publishing and reviews.
- [Working as a team](team.md) — forge claims, leases and recovery.
- [Repository notes](knowledge.md) — shared notes and learned-rule proposals.

## Operations

- [Sandbox and network](sandbox-and-network.md) — agent permissions, registries and shutdown.
- [Limits](limits.md) — budgets, timeouts and failure pauses.
- [CI and deploys](ci-and-deploys.md) — MCP tools, CI feedback, staging and rollback.
- [Web UI and daemon](web-ui.md) — browser access, live views and server lifecycle.
- [Read-only operational screens](web-ui-operations.md) — API projections and browser verification.
- [OIDC setup](web-ui-oidc.md) — optional sign-in, groups, reverse proxy and providers.
- [Remote control from a phone](remote-phone.md) — Android pairing, TLS and revocation.
- [Metrics and stats](metrics-and-stats.md) — journal, usage and benchmarks.
- [Model metrics and adaptive routing](adaptive-routing.md) — qualification, policies and trial controls.
- Benchmark manifests: [worker](examples/worker-benchmark.json) and [reviewer](examples/reviewer-benchmark.json).
- [Troubleshooting](troubleshooting.md) — diagnostics and recovery paths.

## Development and testing

- [Development](development.md) — web builds, contracts, releases, integration status and license.
- [Testing](testing.md) — commit gate, documentation checks, terminal tests and captures.
- [Web UI mockup](web-ui-mockup/README.md) — approved visual language;
  [board](web-ui-mockup/Main.dc.html), [plans](web-ui-mockup/Plan.dc.html) and
  [phone](web-ui-mockup/Phone.dc.html) references.
- [Web UI captures](web-ui-mockup/screenshots/README.md) — seeded/real-history captures and reference differences.

## Design and implementation plans

These documents keep their original status and scope. A proposal is not a promise
that the feature is implemented; start with the guides above for current behaviour.

- [Copilot implementation plan](copilot-plan.md)
- [Model metrics plan](model-metrics-plan.md)
- [Web UI and server mode plan](web-ui-server-mode-plan.md)
- [Copilot CLI plan (2026-10-12)](superpowers/plans/2026-10-12-copilot-cli.md)
- [Plans-to-sub-issues design (2026-10-21)](superpowers/specs/2026-10-21-plans-to-sub-issues-design.md)
- [Plans-to-sub-issues core plan (2026-10-21)](superpowers/plans/2026-10-21-plans-to-sub-issues-core.md)
