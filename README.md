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
forkdeck once        # poll once, work through what is labelled, exit
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
4. commits what the agent changed as a single commit with your git identity,
   pushes the branch, and opens a draft pull request that closes the issue,
   with the check and review results in the body,
5. comments on the issue and moves the label to `agent:pr-open`.

If the checks or the review fail, nothing is pushed. The attempt is committed
on the local branch, and the issue gets a comment saying why and the label
`agent:needs-human`. Labelling the issue again starts over from the base
branch; whatever the earlier attempt left, committed or not, is first moved to
a local branch named `forkdeck/attempts/issue-N-<commit>` so it is never lost. Set `automation.publish_on_failed_review = true` to open
the draft anyway.

It never force-pushes. If `forkdeck/issue-N` already exists on the remote from
an earlier attempt, no work is done: the issue gets a comment saying so and
`agent:needs-human`. Delete the branch and close its pull request before
labelling the issue again.

What gets committed is exactly what was checked and reviewed:

- When the agent finishes, its work is taken as one snapshot. Commits the agent
  made by itself are folded into that snapshot, so the reviewer, who is shown
  everything since the commit the work started from, cannot miss them.
- After the checks, the index and the working tree are put back to the
  snapshot. Files a check creates, rewrites or stages are gone again (ignored
  files such as build output are left alone), so a formatter run as a check has
  no effect. A check that commits or switches branch stops the job.
- The commit is refused if the index no longer matches the snapshot.

The labels have to exist in the repository first.

## Pull request reviews

Label any open pull or merge request `review:ready`. forkdeck checks out its
head in a detached worktree and has every CLI in `review.pr_reviewers` review
it independently and in parallel. The findings go up as one comment with a
section per reviewer, naming the commit that was reviewed, and the label moves
to `review:passed` or `review:changes`.

A review takes minutes, and the author can push in the meantime. Before posting,
forkdeck looks up the pull request's head again. If it is no longer the commit
that was reviewed, the result is discarded, a "stale" comment says so, and the
new head is reviewed on the next poll. It looks once more after the label is
on and takes `review:passed` off again if the head moved in that moment. A
push after that cannot be caught, so **the label alone is not an authorisation**:
anything that acts on it should compare the head with the commit named in the
comment.

If a reviewer did not finish and nobody asked for changes, the comment says the
review is incomplete and the request label goes back. forkdeck retries by itself
`review.max_retries` times (3), waiting `review.retry_secs` (120) before the
first retry and three times longer before each later one. To retry by hand at
any point, remove the label and apply it again.

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

## CI and deployment, through MCP

forkdeck plugs into the CI/CD you already run by calling tools on
[MCP](https://modelcontextprotocol.io) servers. It has no code for any
particular system. For each stage the config names a server, a tool, its
arguments, and regular expressions that say what the answer means. Anything
with an MCP server works the same way: GitHub Actions, GitLab CI, Argo CD,
Flux, Tekton, Woodpecker, Crow CI, Jenkins, CircleCI, or your own.

```toml
[mcp.github]                       # a command to start (stdio) ...
command = "docker"
args = ["run", "-i", "--rm", "-e", "GITHUB_PERSONAL_ACCESS_TOKEN", "ghcr.io/github/github-mcp-server"]
env = { GITHUB_PERSONAL_ACCESS_TOKEN = "${GITHUB_TOKEN}" }

[mcp.pipelines]                    # ... or a URL (Streamable HTTP)
url = "http://127.0.0.1:3000/mcp"
headers = { Authorization = "Bearer ${PIPELINES_TOKEN}" }
```

`${NAME}` is filled from the environment, so tokens stay out of the file. To
find out what a server offers and what its answers look like:

```
forkdeck mcp tools github
forkdeck mcp call github pull_request_read '{"owner":"o","repo":"r","pullNumber":12,"method":"get_check_runs"}'
```

`forkdeck doctor` starts every configured server and checks that each tool a
stage names exists on it.

### CI feedback

With `[repos.ci]` set, a job does not end when its pull request opens. forkdeck
polls the status tool until the pipeline passes or fails. A failure goes back
to the worker together with the log; the fix goes through the same local checks
and review as the first attempt and is pushed as a further commit on the same
branch (a plain push, never forced). After `max_fix_rounds` failures, or if CI
gives no result within `timeout_mins`, the pull request is left open with a
comment and the issue goes to `agent:needs-human`.

```toml
[repos.ci]
server = "github"
[repos.ci.status]
tool = "pull_request_read"
args = { owner = "{owner}", repo = "{name}", pullNumber = "{pr}", method = "get_check_runs" }
failure = '"conclusion":\s*"(failure|timed_out|cancelled)"'
pending = '"status":\s*"(queued|in_progress)"'
success = '"conclusion":\s*"success"'
```

The patterns are tried in the order failure, pending, success, and an answer
that matches none of them counts as pending.

A tool that reports an error has not answered the question, so an error is
never read as a red pipeline or a failed deployment. It is retried on the next
poll, and after five in a row the stage stops with "no result". The same goes
for a status that never arrives before `timeout_mins`. In that case no code is
changed and nothing is rolled back; the job fails and says what it could not
find out.

Placeholders in `args`: `{repo}`, `{owner}`, `{name}`, `{base}`, `{branch}`,
`{sha}`, `{pr}`, `{issue}`, `{env}`, `{last_good}`, `{message}`. A value that is
only `"{pr}"` or `"{issue}"` is sent as a number. `capture` takes values out of
an answer for later calls in the stage, for example the id of the failing run
for the log call; the example config shows it.

Right after a fix is pushed, most systems still show the previous run's
failure for a moment. Two things keep that from being read as the fix failing.
If `status.revision` is set (a pattern whose first group is the commit the
answer is about), only answers about the pushed commit count. Without it, a
failure that shows up within `settle_secs` (60), before the pipeline was seen
running, is not believed until that time has passed. Set `revision` where the
tool's answer includes the commit; it is the exact one.

When forkdeck is stopped while it waits for CI, the pull request is simply left
open as it is. Nothing is posted and no label changes.

### Deploying to staging, and rolling back

With `[repos.deploy]` set, forkdeck watches the base branch. When it moves, it:

1. calls the `trigger` tool, if there is one (leave it out when merging already
   deploys, as with GitOps auto-sync),
2. polls the `status` tool until that commit is healthy,
3. runs the `verify` commands, such as a smoke test,
4. calls the `rollback` tool with `{last_good}`, the last commit that deployed
   and verified cleanly, **if and only if there is evidence the new commit is
   bad**, then waits for the old one to be healthy again,
5. calls the `notify` tool, if there is one, with a one-line result.

`status.revision` is required: a pattern whose first group is the commit the
answer describes. Until the system reports the commit being deployed, its
answer is about the previous one and is treated as pending, whether that one is
healthy or degraded. Without this, "healthy" from a system that had not started
yet would be recorded as a successful deployment.

**What counts as evidence, and what does not.** A rollback changes a running
environment, so forkdeck only does it when something positively says the new
commit is bad:

| What happened | Rolled back? | Result |
|---|---|---|
| The status tool says this commit failed | yes | failed |
| A `verify` command fails | yes | failed |
| The status never arrives, or cannot be read five times running | no | failed, state reported as UNKNOWN |
| The trigger tool refuses | no | failed, nothing was deployed |
| The trigger call gets no answer | not by that alone | the status is watched as usual |
| A newer commit reaches the branch while waiting | no | this one is dropped, the newer one is deployed next |
| forkdeck is stopped or the job is cancelled, at any point | no | cancelled, picked up at the next start |

Other things to know:

- **Staging only.** A deploy stage whose `environment` contains "prod" does not
  load. That is a check on a name you typed and nothing more; forkdeck cannot
  see what a tool really deploys to. Do not point this stage at production.
- **It runs unattended.** Every commit that reaches the base branch is deployed
  without asking. Branch protection and review are what keep that safe.
- **First start deploys nothing.** forkdeck notes where the branch is and takes
  that commit as what is running, which makes it the first rollback target.
- **One forkdeck deploys.** The first process to start holds a lock next to the
  state file; any other (a cron `forkdeck once` while the interface is open,
  for example) still handles issues and reviews but leaves deployments alone.
- **State is in `~/.local/state/forkdeck/deploy.json`** (or `$XDG_STATE_HOME`).
  If the file is damaged, forkdeck says so and deploys nothing until it is
  repaired or deleted; it does not start over on its own.
- **An interrupted deployment is remembered.** If forkdeck is stopped or killed
  in the middle of one, the next start says so and deploys the branch head.
- **A commit is tried once.** After a failed or inconclusive deployment the
  same commit is not tried again; the next commit is.
- **`verify` runs in your local clone** (`path`), as you, not in a checkout of
  the deployed commit. Use it for commands that talk to the environment, such
  as `curl`, not for scripts that must match the deployed version.
- **Rollback is whatever your tool does.** If it cannot be confirmed, the job
  says so in capitals and stops; nothing else is attempted.
- **Only the newest commit is deployed** when several land between polls.

### What forkdeck does with MCP servers' answers

- Values taken from the environment (`${NAME}`) are blanked out of every error
  message and every tool answer before they are logged, shown to an agent or
  posted, in case a server echoes its own URL or environment.
- HTTP redirects are not followed. Configure the final URL.
- `capture` and the patterns work on the text of an answer; with several runs
  or resources in one answer they see the first match. `forkdeck mcp call`
  shows exactly what they will be matched against.

The example config has an Argo CD stage as a starting point. Its tool names
come from the Argo CD MCP server's documentation. Its argument names and its
patterns have not been run against a live Argo CD: an application's answer
carries a sync status and a health status, for the application and for each
resource, and after a rollback its reported revision may be the branch's rather
than what is live. Check each pattern against `forkdeck mcp call` output first.
A small tool of your own that answers with one line, such as
`rev=<commit> Healthy`, is sturdier than patterns over a large JSON document.

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

`forkdeck run "task"` does one job without the interface, and `forkdeck once`
does one pass over the configured repositories. Both exit non-zero if a job
fails and stop their agents on SIGTERM or SIGINT, which is enough for a script
or a cron entry.

## Status

Early. The MCP client has been run against the reference MCP server over both
transports, and the CI and deploy stages are tested against a stand-in server;
no stage has been run against a real CI or deployment system yet. The claude
path has been run against the real CLI. The GitHub flow, the
codex stream parser and the Jev wire format are tested against stand-ins that
follow the documented formats, and the `glab` commands have not been run
against a live GitLab yet. Expect to adjust `agents.codex` arguments for your
codex version.

Not there yet: feeding review comments on an open pull request back to the
worker for another round.

## License

MIT
