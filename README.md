# forkdeck

A terminal agent tree for the `claude` and `codex` CLIs.

You give it a task, or label an issue, and it decides which CLI should do the
work and at what effort, runs it, has the *other* CLI review the result, and
shows the whole thing as a live tree.

![forkdeck running in demo mode](docs/screenshot.png)

It drives the official CLIs as subprocesses, so it uses whatever login they
already have, subscription or API key. It never talks to a model API itself.
The one network call it makes on its own is to the decision endpoint, and that
can be a server on your own machine.

## Install

```
cargo install --git https://github.com/forkdeck/forkdeck
```

You need at least one of `claude` and `codex` on your PATH. With only one, it
also reviews its own work. Issue automation needs `gh` and/or `glab`, logged in.

```
forkdeck --demo      # simulated agents, nothing is executed
forkdeck doctor      # what is installed and reachable
forkdeck init        # writes a commented forkdeck.toml
forkdeck             # the interface
```

## How a job runs

1. **Fork layer.** Three typed questions are answered before anything starts:
   which agent, how much effort, and whether the change needs a review. An
   answer with confidence under `router.threshold` is "split" and takes the
   cautious branch: the default agent, one effort tier up, and a review.
2. **Work.** The chosen CLI runs headless in the working directory.
3. **Checks.** Your own commands (`checks = ["cargo test"]`) run next. A
   failure goes back to the worker with the output.
4. **Review.** The other CLI reads the diff, read-only, and ends with
   `VERDICT: PASS` or `VERDICT: FAIL`. It sees the task and the tree, not the
   worker's summary. A fail goes back to the worker, up to
   `review.max_fix_rounds` times. No verdict is never treated as a pass.

Effort tiers map to models and reasoning effort per CLI in the config, so
"light" can be a small model for a typo and "heavy" the largest one.

## The fork layer

Two routers ship:

- `rules`: keyword lists from the config. No network. The default.
- `jev`: any server that speaks the Jev wire format (`POST /v1/systemone` with
  `choice`, `score` and `noul` questions). If it does not answer, the rules
  router is used and the interface says so.

### Running the decision model locally

[jeff](https://github.com/logan-markewich/jeff) is a self-hosted
implementation of that API. Start it, then point forkdeck at it:

```
JEFF_API_KEYS=devkey uv run jeff        # listens on :8000
```

```toml
[router]
kind = "jev"

[router.jev]
base_url = "http://127.0.0.1:8000"
api_key_env = "FORKDECK_JEV_KEY"        # export FORKDECK_JEV_KEY=devkey
```

`forkdeck route "some task"` prints the decision as JSON, which is the quickest
way to see how a model answers. The two descriptions the model chooses between
(`claude_is_for`, `codex_is_for`) are in the config; the defaults are a
starting point, not a finding about either tool.

## Issues

With `[[repos]]` configured and polling on (`Ctrl-A`, or
`automation.enabled = true`), forkdeck watches for open issues carrying a
label, `agent:ready` by default. For each one it:

1. swaps the label to `agent:working`,
2. creates a git worktree on `forkdeck/issue-N` from the base branch,
3. runs the job as above,
4. commits with your git identity, pushes the branch, and opens a draft pull
   request that closes the issue, with the check and review results in the body,
5. comments on the issue and moves the label to `agent:pr-open`.

If the checks or the review fail, nothing is pushed: the issue gets a comment
saying why and the label `agent:needs-human`. Set
`automation.publish_on_failed_review = true` to open the draft anyway.

The labels have to exist in the repository first.

## Pull request reviews

Label any open pull or merge request `review:ready`. forkdeck checks out its
head in a detached worktree, runs the repo's `checks`, and has every CLI in
`review.pr_reviewers` review it independently and in parallel. The findings go
up as one comment with a section per reviewer, and the label moves to
`review:passed` or `review:changes`.

It never approves and never merges. The comment is advice for whoever does.

This works on any pull request, including the ones forkdeck opened, which gets
you a second look by both models on the pushed commit rather than the working
tree.

## Before you leave it unattended

Issue text is input from other people, and an agent that reads it can be
talked into things. The defaults are set with that in mind:

- Polling is off until you turn it on.
- Only labelled issues are picked up, and `allowed_authors` narrows that
  further. Use it on public repositories.
- `claude` runs with `--permission-mode acceptEdits` and `codex` with
  `--sandbox workspace-write`: both can edit files in the worktree, neither
  gets an unrestricted shell. Reviewers are read-only. Loosen `write_args`
  only for repositories you trust.
- Work happens in a worktree on its own branch and arrives as a draft.

`checks` are shell commands from your config and run as you.

## Keys

| | |
|---|---|
| `Enter` | run the typed task in the current directory |
| `Tab` | force the next task to claude, codex, or back to auto |
| `↑` `↓` | select a job; `Esc` goes back to following the newest |
| `Ctrl-A` | issue and pull request polling on/off |
| `Ctrl-X` | cancel the selected job |
| `Ctrl-C` | quit, stopping any running agent |

Typed tasks share the current directory, so they run one at a time. Issue and
review jobs each have a worktree and run `automation.max_parallel` at once.

`forkdeck run "task"` does one job without the interface and exits non-zero if
it fails, which is enough for a script or a cron entry.

## Status

Early. The claude path has been run against the real CLI. The GitHub flow, the
codex stream parser and the Jev wire format are tested against stand-ins that
follow the documented formats, and the `glab` commands have not been run
against a live GitLab yet. Expect to adjust `agents.codex` arguments for your
codex version.

Not there yet: feeding review comments on an open pull request back to the
worker for another round.

## License

MIT
