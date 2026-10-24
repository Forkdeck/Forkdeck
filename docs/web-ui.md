[Documentation](README.md) · [Front page](../README.md)

# Web UI and daemon

`forkdeck serve` owns one workspace's Hub, restores and resumes eligible jobs, starts
issue polling and team watchers, and serves the browser UI at
`http://127.0.0.1:7420`. `forkdeck remote serve` remains an alias; enabling `[remote]`
also starts the existing paired-phone transport over the same event feed. A second
daemon in the same working directory refuses to start. SIGINT/SIGTERM perform the
normal job shutdown and persistence.

```sh
forkdeck serve --demo
# Forkdeck web UI: http://127.0.0.1:7420/
```

Login is optional. Without `[web.oidc]`, the UI listens on 127.0.0.1, so only this machine can open
it, and requests naming any other host are refused, which stops another site's page from
reaching it through DNS rebinding. On a server, set the address to listen on:

```toml
[web]
bind = "0.0.0.0"   # default 127.0.0.1
port = 7420
```

Without OIDC, anyone who can reach that address can read and operate the workspace, and `forkdeck serve`
prints a warning saying so. Keep it behind a firewall, a VPN, or a reverse proxy that
does the authentication. No CORS is enabled.

To require sign-in and group-based access, configure `[web.oidc]` and an HTTPS
reverse proxy. See [OIDC setup and provider instructions](web-ui-oidc.md).

The board, job details (`/jobs/:id`), plans (`/plans`, `/plans/:id`), log tabs, and
phone filters are live. The live WebSocket uses the native feed's sequence numbers, bounded replay,
a `since` cursor, pings, and resnapshot after gaps/restarts. The intake indicator
reflects actual automation state. Scheduler observations retain unknown forge status
and freshness. Finished work without checks appears as **unverified** in Attention.
The composer starts a typed task in the daemon workspace or a configured repository.
Workspace IDs resolve on the server; the browser never supplies a directory, executable,
arguments or environment. Agent selection uses detected availability. Plan first uses
`Planning::Always`; leaving it unchecked uses `Planning::IfRouted`, as in the TUI.

Queue uses `Hub::reply`, delivering at the current step boundary. Send now uses
`Hub::steer_now`, stopping the running agent so it takes the message immediately.
Stop, Retry, Resume and offered/custom F5 choices use the native Hub operations.
Replies are available only for typed tasks, as in the terminal. Queued messages stay
visible. The header pauses or resumes issue intake.

Plans support new issue planning, acceptance/publication, feedback/refinement and
Ready for build through the F6 Hub methods. Task-list acceptance creates sub-issues;
those schedule automatically rather than using Ready for build. Publication remains
disabled in the isolated server demo, exactly as in the TUI. Re-plan sends feedback
for an unpublished plan; changing an already materialized graph and answering decision
tasks have no existing TUI/Hub operation and remain unavailable with an explanation.

Browser actions use `POST /api/v1/actions` with a typed JSON `action` command. Only
`application/json` is accepted, bodies are limited to 32 KiB, and every action needs
an `Idempotency-Key` of 1–128 characters. An explicit foreign Origin or cross-site
fetch metadata is rejected. OIDC uses the configured public callback origin; local
HTTP uses its Host origin, retaining the loopback Host protection. OIDC viewers see
controls disabled with the operator/admin requirement, and receive 403 on writes.

SQLite saves admission before a Hub action starts and stores the first status/body
for replay, scoped by actor and workspace. Reusing a key with a different body gives
409. Keys are retained for seven days; the isolated demo keeps the latest 4096 in
memory. If a daemon stops between admission and recording the result, the key stays
uncertain and returns 409 rather than executing again. Inspect the job/forge outcome
before taking another action with a new key. Submission persistence failure launches
no agent. Choices and plan actions require the displayed `expected_revision` token;
stale decisions return 409, including when another client wins Hub admission. The
same revision requirement applies when Reply or Send now implicitly answers a
planning choice, so those message routes cannot bypass choice protection.

Doctor shows the latest 200 audit entries, including admissions, outcomes, denied
requests and replays. Records identify the OIDC subject or `local`, action, target,
revision, request ID and time, without raw prompt/message bodies. Audit history is
append-only in SQLite. The live WebSocket carries actionable job/plan changes and intake events;
the browser refreshes projections from events rather than periodic polling.

See [operational screens and API projections](web-ui-operations.md),
[development](development.md#web-development), and [testing and captures](testing.md#web-captures).

![Web board with a selected job](screenshots/board-1440x900.png)

See the [desktop, phone and real-history captures](screenshots/README.md).
