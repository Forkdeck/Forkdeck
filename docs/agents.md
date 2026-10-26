[Documentation](README.md) · [Front page](../README.md)

# GitHub Copilot CLI

Copilot runs models from several vendors behind one login, so it can do two
jobs here.

**Standing in.** When `claude` or `codex` is not installed, copilot plays its
part with a Claude or a GPT model, and the router, the reviewer choice and
`pr_reviewers` work as if both were there. With only `copilot` installed, work
and review still come from different vendors. Turn it off with
`stand_in = false` under `[agents.copilot]`.

**Profiles.** Any other model your copilot `/model` picker lists can be named
as a profile and used as `copilot:<name>` in `review.pr_reviewers`,
`review.reviewer_order` or `--agent`. The router itself still chooses between
claude and codex.

```toml
[agents.copilot.profiles.second]
vendor = "xai"            # needed when the model name does not say it
models = { light = "grok-4.7", standard = "grok-4.7", heavy = "grok-4.7" }

[review]
pr_reviewers = ["claude", "codex", "copilot:second"]
```

Each tier takes a list of models in order of preference. Organisations switch
models on and off, so at startup forkdeck asks copilot which models the account
may use (this costs no request) and skips the others. A model refused during a
run, before it did anything, is skipped for the rest of the session and the run
starts again with the next one. A profile never falls back to `auto`, which
could pick any vendor's model. When no model from another vendor is available,
the review runs anyway and the log says it is a same-vendor review.

`forkdeck doctor` shows the login, whether the account can use Copilot from the
CLI at all, and the model each part would run. Copilot's JSON output carries no
token counts, so they are read from the usage file it writes when it exits.
Its `inputTokens` already include cached input, which earlier versions of
forkdeck counted a second time. The cost column stays empty, as Copilot bills
per request.

See [how a job runs](jobs.md) for worker/reviewer selection and effort tiers,
and [sandbox and network](sandbox-and-network.md) for agent arguments and permissions.
