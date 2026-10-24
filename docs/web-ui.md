[Documentation](README.md) · [Front page](../README.md)

# Web UI and daemon

`forkdeck serve` owns one workspace's Hub, restores and resumes eligible jobs, starts
issue polling and team watchers, and serves the read-only browser UI at
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

Without OIDC, anyone who can reach that address sees every job, log and plan, and `forkdeck serve`
prints a warning saying so. Keep it behind a firewall, a VPN, or a reverse proxy that
does the authentication. No CORS is enabled.

To require sign-in and group-based access, configure `[web.oidc]` and an HTTPS
reverse proxy. See [OIDC setup and provider instructions](web-ui-oidc.md).

The board, job details (`/jobs/:id`), plans (`/plans`, `/plans/:id`), log tabs, and
phone filters are live. SSE uses the native feed's sequence numbers, bounded replay,
Last-Event-ID, heartbeats, and resnapshot after gaps/restarts. The intake indicator
reflects actual automation state. Scheduler observations retain unknown forge status
and freshness. Finished work without checks appears as **unverified** in Attention.
Composer, replies, automation changes, and plan actions are read-only in this milestone;
disabled controls have the title “Coming in the next milestone”.

See [operational screens and API projections](web-ui-operations.md),
[development](development.md#web-development), and [testing and captures](testing.md#web-captures).
