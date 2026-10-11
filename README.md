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
forkdeck init        # writes ~/.config/forkdeck/config.toml, fully commented
forkdeck             # the interface
```

The config is read from `~/.config/forkdeck/config.toml` or the file given with
`--config`, and never from the working directory: agents edit files there, and
the config names commands to run.

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
   `review.max_fix_rounds` times. Only the last line of the review counts as
   the verdict, so one quoted from a description or a code comment is ignored,
   and no verdict is never treated as a pass.

Work that starts from an issue is always reviewed, whatever the fork layer
answers, unless `review.mode = "never"`.

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
4. commits what the agent changed with your git identity, pushes the branch,
   and opens a draft pull request that closes the issue, with the check and
   review results in the body,
5. comments on the issue and moves the label to `agent:pr-open`.

If the checks or the review fail, nothing is pushed. The attempt is committed
on the local branch, and the issue gets a comment saying why and the label
`agent:needs-human`. Set `automation.publish_on_failed_review = true` to open
the draft anyway.

It never force-pushes. If `forkdeck/issue-N` already exists on the remote from
an earlier attempt, no work is done: the issue gets a comment saying so and
`agent:needs-human`. Delete the branch and close its pull request before
labelling the issue again.

The agent's changes are staged before the checks run, and anything the checks
create or rewrite is removed again afterwards (ignored files such as build
output are left alone). So check leftovers are never committed, and the tree
the reviewer reads is the tree that gets pushed. It also means a check cannot
be used to modify the change: a formatter run as a check has no effect.

The labels have to exist in the repository first.

## Pull request reviews

Label any open pull or merge request `review:ready`. forkdeck checks out its
head in a detached worktree and has every CLI in `review.pr_reviewers` review
it independently and in parallel. The findings go up as one comment with a
section per reviewer, naming the commit that was reviewed, and the label moves
to `review:passed` or `review:changes`. If a reviewer did not finish and nobody
asked for changes, the comment says the review is incomplete and the request
label goes back.

It never approves and never merges. The comment is advice for whoever does.

The repo's `checks` are **not** run on pull requests unless you set
`pr_checks = true` for that repo. Running them executes the pull request's code
on your machine, as you, and the tail of a failing check's output is posted in
the comment. Turn it on only where everyone who can get the label applied is
trusted.

Both CLIs load instruction files such as `CLAUDE.md` and `AGENTS.md` from the
tree they work in. A pull request that changes one can steer its own review, so
the comment carries a warning when that is the case.

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

`checks` are shell commands from your config and run as you, outside any agent
sandbox, on code an agent has just written from the issue text. That is the
widest hole in the list above: keep `allowed_authors` tight on any repo that
has checks, or run forkdeck in a container or VM.

Quitting, closing the terminal or sending SIGTERM stops polling and every
running agent with the processes it started, then waits up to 45 seconds while
claimed issues are handed back with a comment. If forkdeck is killed outright
(SIGKILL, power loss), an issue can be left on `agent:working`; move the label
back by hand.

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
it fails, which is enough for a script or a cron entry. On SIGTERM or SIGINT it
stops the agent before exiting.

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
