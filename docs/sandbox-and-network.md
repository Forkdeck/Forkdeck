[Documentation](README.md) · [Front page](../README.md)

# Before you leave it unattended

Issue text is input from other people, and an agent that reads it can be
talked into things. The defaults are set with that in mind:

- Polling is off until you turn it on.
- Only labelled issues are picked up, and `allowed_authors` narrows that
  further. Use it on public repositories.
- `claude` runs with `--permission-mode acceptEdits`, `codex` with
  `--sandbox workspace-write` and `copilot` with `--allow-tool write
  --deny-tool shell`: all can edit files in the worktree, none gets an
  unrestricted shell. A copilot reviewer has write and shell denied and no
  skills. Reviewers are read-only and start in an empty private directory, with
  the checkout supplied as an explicit inspection path. Claude loads user settings
  only and an empty strict MCP configuration; Copilot disables custom instructions
  and built-in MCP servers. Checkout hooks and agent configuration are not loaded
  on startup, including when a worker has edited them. Loosen `write_args`
  only for repositories you trust.
- Workers can build and test in any language. A `claude` worker runs every
  shell command inside Claude Code's own sandbox (bubblewrap and socat on Linux,
  built in on macOS), approved without asking: it writes only to its worktree,
  temp files and the shared build caches, reaches only the package registries
  of the languages the repository uses (crates.io, npm, PyPI, Maven Central,
  NuGet, Go's proxy, RubyGems, Packagist, Hex, Hackage, CRAN and others), your
  mirrors, and the hosts its `agent_domains` adds, and a command that fails in
  there is never retried outside it. Mirrors such as Nexus or Artifactory that
  your package managers already use are found in your own settings and opened
  too: `.npmrc`, `pip.conf`, `uv.toml`, Maven's `settings.xml`, Gradle init
  scripts, Cargo's `config.toml`, `NuGet.Config`, `.gemrc`, Composer's config,
  and variables such as `GOPROXY`, `PIP_INDEX_URL` and `npm_config_registry`.
  Only files in your home directory and `/etc` are read, never a repository's,
  since a pull request could add one. List others under `[agents.network]`
  `mirrors`, and set `public_registries = false` to use the mirrors alone.
  `forkdeck doctor` shows what is open. The sandbox is passed on the command
  line, so the repository's own Claude settings cannot loosen it. Without
  bubblewrap and socat, or with `agents.sandbox = false`, a `claude` worker may
  run only the build and test commands found for the repository by name.
  `forkdeck doctor` says which applies. `codex` workers run in Codex's sandbox.
  When a worker is refused something, the job log and its outcome name it.
- Work happens in a worktree on its own branch and arrives as a draft.

`checks` are shell commands from your config and run as you, outside any agent
sandbox, on code an agent has just written from the issue text. That is the
widest hole in the list above: keep `allowed_authors` tight on any repo that
has checks, or run forkdeck in a container or VM.

Quitting, closing the terminal or sending SIGTERM stops polling and every
running agent with the processes it started, then waits up to 45 seconds while
claimed issues are handed back with a comment. If forkdeck is killed outright
(SIGKILL, power loss), an issue can be left on `agent:working`; move the label
back by hand, or with `forkdeck team release`.
