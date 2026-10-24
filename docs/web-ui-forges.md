# Optional repository picker

Configure a GitHub OAuth App or GitLab application to select repositories from your
account in the web UI. This does not sign you into Forkdeck: optional OIDC access
and its operator/admin role requirement still apply. Without forge configuration,
no picker routes or controls are available. Demo mode refuses forge configuration.

On GitHub, register an OAuth App under Settings → Developer settings → OAuth Apps.
Set its homepage to your Forkdeck origin and Authorization callback URL to exactly
`https://forkdeck.example/forges/github/callback`. GitHub Enterprise uses the same
paths on its own host. Public-only listing requests no scope. Set `private = true`
to request `repo` and include private repositories; GitHub's OAuth scope is broad,
although Forkdeck uses the token only to list repositories.

On GitLab, register an application under your profile's Applications settings,
mark it confidential, select `read_api`, and set its redirect URI to exactly
`https://forkdeck.example/forges/gitlab/callback`. This supports gitlab.com and
self-managed GitLab. The read_api scope permits reading repository metadata.
Both providers use authorization code with PKCE S256.

```toml
[web.forges.github]
client_id = "your-client-id"
client_secret_env = "FORKDECK_GITHUB_SECRET"
redirect_url = "https://forkdeck.example/forges/github/callback"
# host = "github.com"
# private = false
# clone_dir = "~/.local/share/forkdeck/repos"

[web.forges.gitlab]
client_id = "your-client-id"
client_secret_env = "FORKDECK_GITLAB_SECRET"
redirect_url = "https://forkdeck.example/forges/gitlab/callback"
# host = "gitlab.com"
```

Supply those secrets in the daemon's environment. The config stores only the
variable names. Missing/empty secrets and invalid endpoints fail startup. URLs
must use HTTPS except HTTP on loopback for local development. `base_url` is a
provider endpoint override for local tests; leave it unset in production. Provider
requests time out after 15 seconds and never follow redirects.

The user's provider token lives only inside the callback while repositories are
listed. It is never stored, logged, audited or returned. Only repository metadata
is retained in memory, bound to this browser for ten minutes. Connecting expires
after five minutes; restart the connection if it expires or the daemon restarts.
Up to 1000 repositories are listed.

Adding uses the daemon's own `gh` or `glab` login to clone under
`<clone_dir>/<host>/<owner>/<name>` (including nested GitLab groups). Authenticate
that CLI as the daemon's service user first. A repository visible to your browser
account may still be inaccessible to that CLI. A failed clone leaves the config
unchanged. Successful adds append a `[[repos]]` block without rewriting existing
configuration. **Restart Forkdeck to start watching it.**

See [GitHub OAuth authorization](https://docs.github.com/en/apps/oauth-apps/building-oauth-apps/authorizing-oauth-apps)
and [GitLab OAuth](https://docs.gitlab.com/api/oauth2/) for provider setup details.
