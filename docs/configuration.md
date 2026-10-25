[Documentation](README.md) · [Front page](../README.md)

# Configuration

The config is read from `~/.config/forkdeck/config.toml` or the file given with
`--config`, and never from the working directory: agents edit files there, and
the config names commands to run.

A repository can keep its own settings in `.forkdeck/config.toml`, so the team
shares one set of checks and labels and nobody needs a `[[repos]]` entry for it.
The file holds the keys of a `[[repos]]` entry, without the table header, plus
`automation`:

```toml
checks = ["cargo test --locked"]
allowed_authors = ["alice", "bob"]
automation = true     # start polling when forkdeck is started in this clone
```

Start forkdeck anywhere inside the clone and the repository is added by itself:
GitHub or GitLab and its path from the `origin` remote, the clone from the
working directory, the base branch from `origin/HEAD`. A clone whose base branch
has no such file is not added. The file is read from the base branch as last
fetched (`origin/<base>`), never from the working tree, so an agent cannot
change the commands its own work is checked with. `provider`, `repo`, `path`
and `base` are refused in it.

A repository you list in `[[repos]]` yourself also gets its file's settings,
and any key you set there wins. `automation` only takes effect for the clone
forkdeck was started in. A change to the file takes effect the next time
forkdeck starts.

Related settings: [agents](agents.md), [routing](routing.md), [limits](limits.md),
[sandbox and network](sandbox-and-network.md), [CI and deploys](ci-and-deploys.md),
and [web UI](web-ui.md).
