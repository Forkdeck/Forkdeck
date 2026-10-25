[Documentation](README.md) · [Front page](../README.md)

# Install

```
cargo install --git https://github.com/forkdeck/forkdeck
```

Or take a static Linux binary (`linux-amd64` or `linux-arm64`) from the
[releases](https://github.com/forkdeck/forkdeck/releases). Each archive comes
with a `.sha256` checksum and a CycloneDX SBOM (`.cdx.json`, also inside the
archive as `SBOM.cdx.json`) listing every crate built into the binary:

```
sha256sum --check forkdeck-v0.3.0-linux-amd64.tar.gz.sha256
tar -xzf forkdeck-v0.3.0-linux-amd64.tar.gz
```

You need at least one of `claude`, `codex` and `copilot` on your PATH. With
`copilot` alone, it plays both of the others (see [agents](agents.md)). With only one of
`claude` and `codex` and no `copilot`, that one also reviews its own work. Issue
automation needs `gh` and/or `glab`, logged in.

```
forkdeck --demo      # simulated agents, nothing is executed
forkdeck doctor      # what is installed and reachable
forkdeck init        # writes ~/.config/forkdeck/config.toml, fully commented
forkdeck             # the interface
forkdeck once        # poll once, work through what is labelled, exit
```

See [configuration](configuration.md) for repository settings and
[development](development.md#releases) for cutting a release.
