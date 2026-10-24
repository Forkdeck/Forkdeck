# forkdeck

A terminal agent tree for the `claude`, `codex` and GitHub Copilot (`copilot`) CLIs.

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

Or take a static Linux binary (`linux-amd64` or `linux-arm64`) from the
[releases](https://github.com/forkdeck/forkdeck/releases). Each archive comes
with a `.sha256` checksum and a CycloneDX SBOM (`.cdx.json`, also inside the
archive as `SBOM.cdx.json`) listing every crate built into the binary:

```
sha256sum --check forkdeck-v0.2.0-linux-amd64.tar.gz.sha256
tar -xzf forkdeck-v0.2.0-linux-amd64.tar.gz
```

A release is cut by pushing a tag `vX.Y.Z` that matches the version in
`Cargo.toml`; `.github/workflows/release.yaml` tests, builds, writes the SBOMs
and publishes the release.

You need at least one of `claude`, `codex` and `copilot` on your PATH. With
`copilot` alone, it plays both of the others (see below). With only one of
`claude` and `codex` and no `copilot`, that one also reviews its own work. Issue
automation needs `gh` and/or `glab`, logged in.

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

## How a job runs

1. **Fork layer.** Three typed questions are answered before anything starts:
   which agent, how much effort, and whether the change needs a review. An
   answer with confidence under `router.threshold` is "split" and takes the
   cautious branch: the default agent, one effort tier up, and a review.
2. **Work.** The chosen CLI runs headless in the working directory.
3. **Checks.** Your own commands (`checks = ["cargo test"]`) run next. A
   failure goes back to the worker with the output.
4. **Review.** An agent running another vendor's model reads the diff, read-only, and ends with
   `VERDICT: PASS` or `VERDICT: FAIL`. It sees the task and the tree, not the
   worker's summary. A fail goes back to the worker with the review, and the
   worker keeps addressing it until the review passes. `review.max_fix_rounds`
   (5) only caps runaway loops, and fixing also stops when a fix changes
   nothing. A reviewer that crashes or gives no verdict is asked once more. Only the last line of the review counts as
   the verdict, so one quoted from a description or a code comment is ignored,
   and no verdict is never treated as a pass.

Work that starts from an issue is always reviewed, whatever the fork layer
answers, unless `review.mode = "never"`.

Effort tiers map to models and reasoning effort per CLI in the config, so
"light" can be a small model for a typo and "heavy" the largest one.

## GitHub Copilot CLI

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
token counts, so they are read from the usage file it writes when it exits; the
cost column stays empty, as Copilot bills per request.

## The fork layer

Two routers ship, and a default that picks between them:

- `auto`, the default: asks the decision server at `router.jev.base_url` when
  one answers, and uses the rules quietly when none is running. On this machine
  without `FORKDECK_JEV_KEY` set it uses the key `scripts/local-jev.sh` serves
  with, so starting that server is all it takes.
- `rules`: keyword and phrase lists from the config. No network. A keyword may
  be a phrase ("race condition", "stack trace"), and word forms match, so "fix"
  also matches "fixes", "fixed" and "fixing". Phrases such as "how should" or
  "pros and cons" in a typed task ask for a choice of approaches first. The
  built-in lists cover common phrasings; a list set in `[router.rules]`
  replaces the built-in one.
- `jev`: any server that speaks the Jev wire format (`POST /v1/systemone` with
  `choice`, `score` and `noul` questions). If it does not answer, the rules
  router is used and the interface says so.

Before either is asked, **route memory** looks at earlier jobs in the same
repository (or directory, for a typed task) and of the same kind. When at least
two tasks much like this one were routed by the decision server and went well
(done at the first try, with the review or the checks passing), and none like it
went badly, the task is routed the same way without asking. The router line says
`memory` and which jobs it followed; `forkdeck route "<task>"` shows the same.
Once a kind of task has gone well three times at a tier, the next one tries a
tier lower to spend fewer tokens. If that worker fails, it goes back to the
usual tier once, and that kind of task is not stepped down again for 30 days.
Only hashed words are kept, never the task's text. `[router.memory]` tunes it,
and `enabled = false` turns it off.

### Running the decision model locally

For a local RTX 3090, the repository includes a repeatable setup for
[AutoJev-27B Q4_K_M](https://huggingface.co/thomasgauthier/autojev-27b-GGUF).
It uses the [jev.cpp](https://github.com/thomasgauthier/jev.cpp) fork, which
preserves AutoJev's classifier head and serves `/v1/systemone`. This model
requires that fork; the publisher does not support standard llama.cpp or
LM Studio for it.

Requirements: Linux/WSL with GPU access, an RTX 3090 at GPU index 0, the CUDA
toolkit (`nvcc`), CMake, a C++ compiler, Git, curl, and `sha256sum`. Testing
also needs Rust/Cargo and Python 3. Allow at least 25 GB of free disk space;
the text model download is approximately 15.8 GB. No vision projector is needed.

```sh
./scripts/local-jev.sh setup   # build pinned CUDA source; download and verify weights
./scripts/local-jev.sh start   # foreground server; Ctrl-C stops it
```

In a second terminal, from this repository:

```sh
./scripts/local-jev.sh test

# Or route your own task (this does not launch coding agents):
export FORKDECK_JEV_KEY=forkdeck-local-dev
cargo run --locked -- --config .local/jev/forkdeck.toml route "Fix the pagination bug"
```

The script keeps source, builds, model weights, and its dedicated configuration
in ignored `.local/jev/`. Repeating `setup` reuses the build and verified model;
interrupted downloads resume. It never writes your normal Forkdeck configuration.
The server binds to `127.0.0.1:8000`, uses one request slot and an 8,192-token
context, and requests full GPU offload. The test configuration allows 60 seconds
per routing request and disables automation. The smoke test requires real Jev
answers for three tasks, then checks authentication failure and connection
failure both produce explicit rules-fallback diagnostics.

The development key defaults to `forkdeck-local-dev`. To change it, export
`FORKDECK_JEV_KEY` with the same value in both terminals. Set `JEV_BUILD_JOBS`
to change build concurrency (default 4). If `nvidia-smi` reports GPU access
blocked, run from a GPU-enabled terminal outside the sandbox. A working driver
alone is not sufficient to build: `nvcc` must also be installed and on PATH.
If port 8000 is occupied, stop the other service before starting this server.
If CUDA runs out of memory, check `nvidia-smi` and free GPU memory; the script
does not automatically reduce context or switch to CPU. Quantized accuracy
still needs evaluation on your tasks; passing the smoke test establishes API
integration, not routing quality.

#### Smaller GLiFormer alternative

[jeff](https://github.com/logan-markewich/jeff) is a self-hosted
implementation of that API using GLiFormer. Follow its upstream installation
and model-download instructions, start it, then point forkdeck at it:

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

Two more labels belong to plans that are split into sub-issues: `plan_label`
(default `forkdeck:plan`) marks the parent issue, and `human_label` (default
`needs-human`) marks a task that needs a person before it can go on. Like the
others they must be nonempty, comma-free and distinct from the issue labels.

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
head in a detached worktree and has every agent in `review.pr_reviewers` review
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

The CLIs load instruction files such as `CLAUDE.md` and `AGENTS.md` from the
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

`${NAME}` is filled from the environment, so tokens stay out of the file.
Stdio servers inherit only runtime settings (`PATH`, `HOME`, user, locale and
temporary-directory settings). Pass credentials and other required variables
explicitly through the server's `env` table.

To find out what a server offers and what its answers look like:

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

"Passed" has to mean that this commit passed, so the status call must be tied
to a commit. The config does not load unless one of these holds:

- `status.revision` is set: a pattern whose first group is the commit the
  answer is about. Only answers about the commit forkdeck pushed are believed.
  A result left over from the previous push, pass or fail, is treated as
  pending.
- or `args` pass `{sha}`, so the tool is asked about the exact commit.

With the second form and no `revision`, a failure that shows up within
`settle_secs` (60), before the pipeline was seen running, is not believed until
that time has passed.

When forkdeck is stopped while it waits for CI, the pull request is simply left
open as it is. Nothing is posted and no label changes.

### Deploying to staging, and rolling back

With `[repos.deploy]` set, forkdeck watches the base branch. When it moves, it:

1. asks the `status` tool what is going on already, and then calls the
   `trigger` tool, if there is one (leave it out when merging already deploys,
   as with GitOps auto-sync),
2. polls the `status` tool until that commit is healthy,
3. runs the `verify` commands, such as a smoke test,
4. calls the `rollback` tool with `{last_good}`, the last commit that deployed
   and verified cleanly, **if and only if there is evidence the new commit is
   bad**, then waits for the old one to be healthy again,
5. calls the `notify` tool, if there is one, with a one-line result.

**Not starting what is already running.** Before triggering, forkdeck asks the
deployment system, because it is the one place that knows. If the commit is
already deployed or rolling out (a teammate's machine started it, or the
system's own automation did), it is not triggered again: forkdeck watches it to
the end, verifies it and records it as usual. If a rollout of a different
revision is in progress (the answer matches `pending` or `paused`), forkdeck
waits for it to settle before starting, up to `timeout_mins`, and starts
nothing if it never does. If the status cannot be read, nothing is started.
Set `status.pending` to whatever your system says mid-rollout; without it
forkdeck cannot tell a rollout of something else from a settled environment.

`status.revision` is required: a pattern whose first group is the commit the
answer describes. Until the system reports the commit being deployed, its
answer is about the previous one and is treated as pending, whether that one is
healthy or degraded. Without this, "healthy" from a system that had not started
yet would be recorded as a successful deployment.

**Canary, percentage and regional rollouts.** How a commit is rolled out is the
deployment system's decision, not forkdeck's. forkdeck asks for a commit to be
deployed and then watches; it sends no strategy, no percentages and no regions.
Whatever your MCP server's tool does when called (an Argo Rollouts canary, a
Flagger analysis, a region-by-region pipeline) is what happens. Three optional
patterns on `status` let forkdeck follow such a rollout properly:

```toml
[repos.deploy.status]
tool = "get_rollout"
args = { name = "{name}" }
revision = 'rev=([0-9a-f]+)'
success = "Healthy"
failure = "Degraded"
pending = "Progressing"
paused = "Paused"                 # waiting for a person to promote it
reverted = "Aborted"              # the system stopped and restored the old version itself
progress = 'step=(\S+)'           # shown while waiting, for example "canary-20%"
```

- `progress` is display only: its first group is shown next to "pending" in
  the interface and the log, so you can see "canary 20%" or the region.
- `paused` means the rollout is waiting for a manual promotion or an approval.
  That time is not charged to `timeout_mins`; it has its own limit,
  `pause_timeout_mins` (a day). forkdeck does not promote anything.
- `reverted` means the system judged the rollout bad and put the old version
  back on its own. The job fails, and forkdeck's `rollback` tool is **not**
  called: two rollbacks would fight each other. With a system that reverts
  itself you can leave `rollback` out altogether. After an abort such systems
  go back to reporting the old commit, so this answer is believed even though
  it is no longer about the new one, but only once the new one was seen
  rolling out; before that it is left over from an earlier deployment.

A long rollout needs a long `timeout_mins`: the default of 15 is for a plain
deployment. `verify` runs once, after the whole rollout reports success;
checking each step is the deployment system's analysis, not forkdeck's.

**What counts as evidence, and what does not.** A rollback changes a running
environment, so forkdeck only does it when something positively says the new
commit is bad:

| What happened | Rolled back? | Result |
|---|---|---|
| The status tool says this commit failed | yes | failed |
| The deployment system reverted the rollout itself (`reverted`) | no, it already did | failed |
| The rollout waits for a person longer than `pause_timeout_mins` | no | failed, state reported as UNKNOWN |
| A `verify` command runs and exits non-zero | yes | failed |
| A `verify` command gives no answer (hangs past its time limit, cannot start, is killed) | no | failed, state reported as UNKNOWN |
| The branch cannot be read just before a rollback, so a newer commit cannot be ruled out | no | failed, needs attention |
| The status never arrives, or cannot be read five times running | no | failed, state reported as UNKNOWN |
| The trigger tool refuses | no | failed, nothing was deployed |
| The trigger call gets no answer | not by that alone | the status is watched as usual |
| A newer commit reaches the branch while waiting | no | this one is dropped, the newer one is deployed next |
| This commit is bad, but the branch has already moved on | no | failed; the newer commit is deployed next |
| forkdeck is stopped or the job is cancelled, at any point | no | cancelled, picked up at the next start |

Other things to know:

- **Staging only.** A deploy stage whose `environment` contains "prod" does not
  load. That is a check on a name you typed and nothing more; forkdeck cannot
  see what a tool really deploys to. Do not point this stage at production.
- **It runs unattended.** Every commit that reaches the base branch is deployed
  without asking. Branch protection and review are what keep that safe.
- **First start deploys nothing.** forkdeck notes where the branch is and asks
  the status tool once. Only if the deployment system reports that very commit
  as healthy does it become a rollback target. Otherwise there is nothing to
  roll back to until a deployment has succeeded, and a failure before then is
  reported without a rollback.
- **One forkdeck deploys.** The first process to start holds a lock next to the
  state file; any other (a cron `forkdeck once` while the interface is open,
  for example) still handles issues and reviews but leaves deployments alone
  until the first has exited. Each provider/repository pair may appear only
  once in `[[repos]]`.
- **Retries need fresh rollout evidence.** When retrying a failed deployment of
  the same commit, an old failure alone cannot trigger rollback. A matching
  `pending` or `paused` status establishes that the new attempt has started;
  success can finish it directly. Without fresh evidence, the wait times out
  with unknown status and no rollback.
- **State is in `~/.local/state/forkdeck/deploy.json`** (or `$XDG_STATE_HOME`).
  If the file is damaged, forkdeck says so and deploys nothing until it is
  repaired or deleted; it does not start over on its own. If a result cannot
  be written, the job fails and says so even when the deployment itself went
  through, because forkdeck's record no longer matches the environment.
- **An interrupted deployment is remembered.** If forkdeck is stopped or killed
  in the middle of one, the next start says so and deploys the branch head,
  which calls the trigger again. After three starts for the same commit it
  stops trying and says the state is unknown. An interrupted rollback is
  reported at the next start and left to you.
- **A commit is tried once.** After a failed or inconclusive deployment the
  same commit is not tried again; the next commit is.
- **`verify` runs in your local clone** (`path`), as you, not in a checkout of
  the deployed commit. Use it for commands that talk to the environment, such
  as `curl`, not for scripts that must match the deployed version.
- **Rollback is whatever your tool does.** If it cannot be confirmed, the job
  says so in capitals and stops; nothing else is attempted.
- **Only the newest commit is deployed** when several land between polls.

### What forkdeck does with MCP servers' answers

- Values taken from the environment (`${NAME}`) that are six characters or
  longer are blanked out of every error message and every tool answer before
  they are logged, shown to an agent or posted, in case a server echoes its own
  URL or environment. Patterns are matched against the answer as it was sent.
  A value that comes back encoded (base64, percent-encoded) is not caught.
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

## Working as a team

When several people run forkdeck against the same repositories, each machine
needs to know what the others have taken, and people want to see it too. With
`[team]` enabled, forkdeck keeps that on GitHub or GitLab itself. There is no
server to run and nothing to sign up for; whoever can see the repository can
see who is on what.

```toml
[team]
enabled = true
```

**What it does.** Before forkdeck takes a labelled issue or pull request, it
posts one comment saying so, and assigns the issue to the account `gh` or
`glab` is logged in as. That comment is edited in place as the job moves
through routing, working, review, fixing, publishing and watching CI, and once
more when it ends.
A deployment is announced the same way on the commit being deployed (GitHub
only: GitLab commit comments cannot be edited).

**What is shared.** The account name, the machine name, which agent, which
stage, when it started, when it was last updated, and the pull request link.
No prompt, no agent output, no logs. Tasks typed into the prompt are never
shared.

The machine name is your hostname on private repositories. On a public
repository no machine name is posted, and if forkdeck cannot find out whether a
repository is public it posts none. To show a name there anyway, set `host` to
a label of your choosing; it is used as written. Each machine also puts a
random id into its claims, kept in `~/.local/state/forkdeck/machine-id`, so a
restarted forkdeck can recognise a claim its previous run left behind. The id
is not derived from anything about the machine.

**Seeing it.** `F4` swaps the agent tree for the team pane, and
`forkdeck team` prints the same thing:

![the team pane in demo mode](docs/team.png)

```
alice on mbp-alice (you)
  payments#41        working       claude      4m  Retry loop keeps the long backoff after a success
maria on mbp-maria
  payments#38        watching ci   codex      31m  Webhook signature check rejects rotated keys
  payments!57        reviewing     claude      5m  Add --since flag to the export command
tomasz on build-02
  payments#35        working       claude  2h 33m  Paginate the ledger export
                     no update for 1h 8m · forkdeck team release acme/payments#35
  payments@9f3c2ab   deploying                 3m  Merge pull request #52
```

`#` is an issue, `!` a pull request, `@` a deployment. It reads the working
labels of every configured repository, so it also shows items someone claimed
by hand or with sharing off, as "no status". `--all` adds
issues whose pull request is open and waiting, and `--json` prints the rows
for scripts. Looking never changes anything, and works with sharing off.

**Not stepping on each other.** An issue or review that a teammate's forkdeck
holds is left alone: no label moves, no comment, and the job ends as "not
started". If two machines take the same issue in the same second, both see
both claims when they look again after moving the label, and the older
comment wins on both. If the claims cannot be read, nothing is taken.

**Who can claim and release.** The comments are read back from threads anyone
may write in, so a claim or a release only counts when the forge itself says
its author works on the repository: on GitHub the comment's author is the
owner, an organisation member or a collaborator; on GitLab a project member
with the Developer role or above. The comment also has to be written by the
account it names. Someone who can only comment cannot hold an issue or end a
teammate's claim by typing a marker.

**A claim is a lease.** Holding the item means holding the oldest claim that
is still being refreshed, and forkdeck checks exactly that before everything
it writes: the push, the pull request, the review, labels, and failure
reports. A job that finds its claim ended, overtaken, expired or unreadable
writes nothing more to the item and leaves its labels to whoever holds it now.

**When a machine goes away.** A running job refreshes its comment every
`heartbeat_secs`. A claim that has not been refreshed for `stale_mins` (the
forge's own "last edited" time, not a clock in the comment) is shown as stale
and no longer blocks anyone. Its label is still on the working one, so hand it
back:

```sh
forkdeck team release acme/payments#35      # an issue
forkdeck team release acme/payments!57      # a review
```

That ends the claim with a comment and puts the request label back, and the
next poll, yours or a teammate's, picks it up. A claim that is still being
refreshed needs `--force`. The machine that held it notices the next time it
is about to write anything, and stops without touching the labels.

Only an item that carries the working label is handed back, so releasing
something already finished does not queue it again. An item with that label
and no forkdeck claim (claimed by hand, or by a forkdeck with sharing off)
needs `--unclaimed`. An issue whose pull request is already open is not
handed back; close or take over the pull request instead.

A machine that was suspended, or cut off from the forge, for longer than
`stale_mins` comes back to an expired claim. It stops there rather than
publishing work someone else may have redone; hand the issue back and let it
run again.

Every machine that works a repository should have `[team]` enabled: a forkdeck
with sharing off neither announces what it takes nor looks at what others
have. Staleness compares the forge's time with this machine's clock, so a
clock that is far ahead makes teammates look stale. On GitHub, "organisation
member" is what the forge reports for the comment; a member the organisation
has given no write access to the repository still counts. Deployments are announced on the forge
but decided by the deployment system: each machine asks it before triggering
and does not start a commit that is already deployed or rolling out. A machine
that sees a teammate's announcement first also waits a moment before starting.
Two machines that ask in the same second can still both trigger, so the
`trigger` tool should be safe to call twice for the same commit, or the deploy
stage should live on one machine.

## Limits

forkdeck picks work up by itself, so it has brakes. All of them count over the
last 24 hours from the journal (next section), which means restarting forkdeck
resets nothing.

```toml
[limits]
jobs_per_day = 20          # issues and reviews picked up, all repositories together
tokens_per_day = 5000000   # in and out, both CLIs, typed tasks included
job_mins = 90              # one issue or review may run this long
pause_after_failures = 5   # default
pause_mins = 60            # default
```

- **`jobs_per_day`** and a repository's own `jobs_per_day` cap how many issues
  and reviews are picked up. Deployments and typed tasks are not counted: one
  follows from a merge and the other from you.
- **`tokens_per_day`** counts everything both CLIs report having used. When it
  is reached nothing more is picked up; a job already running finishes.
- **`job_mins`** stops an issue or review that runs too long, kills its agent,
  and hands the item to a person with the failure label. Deployments are
  exempt; they have their own timeouts and stopping one halfway is worse.
- **`pause_after_failures`** is on by default: after five issues or reviews in
  a row fail on a repository within a day, nothing more is picked up from it for an hour.
  Then one is tried; if that fails too, the pause starts again. Cancelled jobs
  count neither way.

The limits are only as good as the journal they count from. If it cannot be
read or written (a full disk, wrong permissions), nothing more is picked up
until it can, and the log says why: not knowing what was used is not treated
as nothing having been used. A job that ended while it could not be written is
recorded as soon as it works again. This applies whenever any limit is set,
which with the default failure pause is always.

What a limit holds back is not touched at all: the label stays, nothing is
claimed or commented, and it is picked up on the first poll with room again.
The queue panel shows what has been used and why a repository is on hold, the
session log says so once when it starts and once when it ends, and
`forkdeck doctor` prints the limits in force. Only the failure pause is set out
of the box; the other three are yours to choose.

## The journal, and `forkdeck stats`

Every finished job is one line in `~/.local/state/forkdeck/journal.jsonl`: what
it was, which CLIs ran and how often, how long it took, how many times the
work was sent back, the verdict, tokens and cost as the CLIs reported them,
the pull request, and the outcome category (done, failed or cancelled). No prompts and no agent output,
and for a typed task not even the title. Interrupted typed tasks are recorded once when they finish, excluding time spent stopped. The file stays on your machine.

```
$ forkdeck stats --days 30
last 30 day(s): 61 job(s)

issues      34   done   27   failed    6   cancelled   1
          82% ended with a pull request · 70% of those without being sent back · 11 fix round(s) in all
          time to a pull request: median 6m 12s, 90% within 19m 40s
reviews     19   done   18   failed    1   cancelled   0
          11 passed · 7 asked for changes
deploys      8   done    7   failed    1   cancelled   0
          1 rolled back

claude      71 run(s) · 9.8M tokens in · 412.0k out · $31.40 as reported by the CLI
codex       48 run(s) · 6.1M tokens in · 220.5k out
```

`--jobs 20` adds the latest jobs one per line, and `--json` prints the summary
for a dashboard or a script. The journal is also the audit trail: it is
append-only, each line stands alone, and a damaged line is skipped without
hiding the rest.

Model-level usage is available with `forkdeck stats --by-model --json`, with
`--role` and `--repo` filters. `forkdeck benchmark` produces paired evidence in
isolated local clones; `forkdeck adaptive` explains proposals, reservations and
suspensions. Adaptive worker routing is disabled by default. See
[model metrics and adaptive routing](docs/adaptive-routing.md) for qualification,
advisory/shadow validation, trial limits, promotion and disable controls.

Worker benchmarks support `--preflight` and explicit call budgets. Use
`forkdeck reviewer-benchmark MANIFEST --output DIRECTORY` for separate labeled
reviewer comparisons. Adaptive policies can select `reported_cost_usd` instead of
tokens when all end-to-end costs are reported; see the
[runtime guide](docs/adaptive-routing.md) and [manifest templates](docs/examples/).

## Notes about a repository

Each job would otherwise start knowing nothing about the project beyond what
the CLIs read for themselves. forkdeck puts two things in front of the worker
with every issue, ahead of the task:

- **The team's notes:** `.forkdeck/notes.md` in the repository, as it is on the
  base branch. It is an ordinary file: write conventions in it and change it
  through a pull request like anything else. Everyone's forkdeck reads the
  same one.
- **This machine's own notes:** a file in `~/.local/state/forkdeck/notes/`, for
  things that are nobody else's business.

```sh
forkdeck notes acme/payments          # both, and where they are
forkdeck notes acme/payments add "Staging needs the VPN."   # your own
forkdeck notes                        # every repository and its file
```

The worker is told the notes are background: they do not change the task and
cannot grant access to anything. They come first in the prompt because they are
the same for every job on the repository, which is the part a model's context
cache can reuse.

**Learning from mistakes.** With `learn = true`, a job whose work was sent
back by a check or a reviewer, or a review that asked for changes, ends with
one more short run: a CLI is shown the findings and asked for at most three
general rules that would have prevented them.

```toml
[knowledge]
learn = true
```

Those rules are **proposed, not installed**. forkdeck pushes a commit that adds
them to `.forkdeck/notes.md` on a branch of its own (`forkdeck/notes-…`) and
opens a pull request, listing each rule and the issue it came from. Nothing
changes for any job until a person merges it; edit or delete rules there first,
or close it to discard them. While that pull request is open, rules learned
later are added to it, so there is one thing to look at per repository. The
branch is only ever pushed to normally, never forced, and it is built without
touching the clone's working tree.

Why a pull request: a learned rule is read by every later job of everyone on
the team, and the findings it is distilled from come from work that started
with somebody's issue text. forkdeck narrows what can get in (one line, at
most 200 characters, no links, no mentions, three per job, only the newest
`max_learned` kept), but it cannot judge whether a rule is true. A person can.

Two things keep the file honest:

- An agent working on an issue cannot change the notes on the side. Whatever
  it does under `.forkdeck/` is left out of the commit, and the log says so.
- A labelled pull request that touches `.forkdeck/` gets the same warning on
  its review as one that touches `CLAUDE.md` or `AGENTS.md`.

If the pull request cannot be made (the forge is down, the push is refused),
the rules wait in a file of their own that no job reads, and are proposed again
on a later poll. With sharing on, a rule reaches agents through an approved
pull request or not at all. `forkdeck notes` says how many are waiting.

`share = false` keeps learned rules on this machine instead: they go into its
own notes file under `## Learned by forkdeck` and take effect at once, for that
machine only. `learn` is off by default either way. A job that went
well teaches nothing and costs no extra run. Reviewers of labelled pull
requests are not given the notes yet.

Repository labels must be nonempty, contain no commas, and be distinct within each issue or review label set. If several `[[repos]]` entries name the same provider and repository (for separate deployment environments), issue and review polling uses the first entry only. Active worktrees are locked across processes for the duration of their jobs.

Stdio MCP servers receive their configured `env` plus basic process settings (`PATH`, `HOME`, `USER`, `LANG`, `LC_ALL`, `TMPDIR`, `SYSTEMROOT`). Other credentials are not inherited; add required variables explicitly. A deployment whose status is unrecognizable is not triggered. A revision already reported failed or reverted is not triggered again on retry.

## Remote control from a phone

The forkdeck Android app shows the job list and each job's tree as it runs,
and can start a task, reply to one, cancel, pause polling or poll now. It
talks to forkdeck directly over the local network: there is no relay and
no account.

```toml
[remote]
enabled = true
listen = "0.0.0.0:7443"
```

Restart forkdeck, then pair a phone:

```
forkdeck remote pair            # shows a QR code; scan it in the app
forkdeck remote devices         # paired phones
forkdeck remote revoke pixel    # unpair one; it is disconnected within seconds
```

The connection is TLS in both directions. forkdeck keeps its own certificate
authority in `~/.config/forkdeck/remote` and trusts nothing else; the QR code
carries its fingerprint and a one-time code. When the phone pairs it makes a
key that never leaves it and gets a certificate signed for that key. A
connection without a paired certificate can do nothing but pair.

A code works once, for two minutes, and only from the subnet it was made for:
the phone has to be on the same network as this machine, not routed in from
elsewhere. With `lan_only = true` (the default) every connection is held to
that, not only pairing. Use `--iface` when the machine has more than one
network.

A paired phone can start agents that edit files in `workdir`: treat it like
a terminal on this machine. Tasks from the phone run in `[remote] workdir`, or
the directory forkdeck was started in.

## Before you leave it unattended

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

## Keys

| | |
|---|---|
| `Enter` | send what you typed to the task the input box names: a follow-up once it has finished, a queued message while it works, your choice while it waits for one. With nothing typed, an interrupted task resumes and a finished one carries on. After `Ctrl-T`, start a new task in the current directory |
| `Ctrl-T` / `Ctrl-N` | make the next `Enter` start a new task, keeping your draft |
| `Ctrl-P` / `Shift-Tab` | the next new task proposes approaches first, and you pick one |
| `Ctrl-R` | pin replies to the selected typed task, whatever is selected later |
| `Ctrl-S` | send what you typed to the task now: its agent stops and continues with the message |
| `Tab` | force the next task to claude, codex, copilot, or back to auto |
| `↑` `↓` | recall submitted messages; edit and press Enter to resend; Down restores your draft |
| `Alt-↑` `Alt-↓` | select a job; `Esc` goes back to following the newest |
| `Alt-←` `Alt-→` | select the newest job in the previous or next board lane that has one |
| `PgUp` `PgDn` | scroll the selected job's worker and reviewer output back and forth; it stops following new output until you page back to the end |
| `Shift-PgUp` `Shift-PgDn` | scroll the session log |
| `End` | follow the newest output and log lines again |
| click an edit line | full-screen diff of those files against the job's starting commit, refreshed while the agent works; `↑` `↓` `PgUp` `PgDn` or the wheel scroll, `Esc` closes. Hold `Shift` to select text with the mouse |
| `F2` | switch between the factory board and agent tree |
| `F5` | open planning choices for the selected task |
| `F6` | create/link an issue plan; inspect, accept, refine or mark it ready for build |
| `Ctrl-A` | issue and pull request polling on/off |
| `F4` | show what teammates are working on, and back |
| `Ctrl-X` | cancel the selected job |
| `Ctrl-C` / `Ctrl-Q` | quit from any view, saving the draft and stopping agents; again to quit at once |

Ctrl keys work with Shift or Caps Lock held.

The TUI opens on a factory board: Queue, Build, Review, Delivery, Done, and Attention. Select a job with Alt-Up/Down to inspect its checks, review result, and live worker/reviewer output. F2 switches to the detailed agent tree. Message recall keeps the last 100 accepted submissions across TUI restarts; Enter replies to the current task; Ctrl-T starts a new task and F4 opens the team view.

When a worker or reviewer reports exhausted usage before using tools, Forkdeck retries once with the other available CLI. The limited CLI is skipped for 15 minutes in that process and shown as LIMITED. A run that has used tools is not replayed automatically. If only one CLI remains available, its own review is explicitly logged.

A typed task keeps its conversation. The input box's title says where `Enter`
sends: once a task is started, to that task; `Alt-↑` `Alt-↓` selects another,
`Ctrl-R` pins replies to the selected one, and `Ctrl-T` starts a new task. The
same agent continues in the same tree with the earlier exchanges, and its change
goes through the checks and review again. Messages sent while the task runs are
queued on it and taken when its current step ends; `Ctrl-S` stops the agent to
take one at once. The input box shows how many are queued, and the board the
next one. A message the task cannot take (another forkdeck holds it, say) stays
on the input line.

Typed tasks, their conversations, queued messages and recent output are kept in
the history database (`~/.local/state/forkdeck/forkdeck.db`), with the last 100
messages you sent and an unfinished draft for each working directory. Tasks cut
off by quitting continue by themselves when forkdeck starts again; if one could
not, select it and press `Enter` with nothing typed.

Choices open a picker when the task is selected and nothing is typed: the
options of a plan waiting for its choice (`Ctrl-P`), or numbered choices in a
finished answer (lists introduced as choices or options, and option tables).
Up/Down highlight one and `Enter` sends it: a waiting plan takes the option
itself, and for an answer it is sent as a reply. **Other / custom response**
(or `c`) leaves the input line for your own words. Esc closes the picker; F5
reopens it. An answer that offers choices and changed nothing finishes without
checks or review, so you can decide what happens next; if the worker changed
files, they are checked and reviewed as usual. Interactive permission prompts of
the CLIs themselves are not handled by this picker.

### Issue-backed planning

Press **Ctrl-T**, then **F6** to start a plan for a configured GitHub or GitLab
repository. F6 on a completed typed task attaches issue planning to that same
conversation. Choose the repository with Left/Right; the current directory's
matching repository is selected by default. Tab moves between the repository,
existing issue URL/number, and planning request. Leave the issue field blank to
create a new issue after acceptance. Enter starts planning.

The planner and plan reviewer both use the configured read-only agent arguments.
They review the goal, steps, acceptance criteria, dependencies and test strategy;
repository code checks run later during Build. A complete plan goes to review
even when ordinary task review is disabled. A failed review goes back to the
planner with its findings, and the revised plan is reviewed again, up to
`review.max_fix_rounds` times; it stops sooner when the plan needs a decision from
you, a revision changes nothing, or you have sent a message. A failed or incomplete
review cannot be accepted. Numbered choices and custom replies continue on the same task.

F6 shows the complete saved plan and reviewer feedback. Tab switches between
them; Up/Down and PageUp/PageDown scroll. Left/Right or 1–3 selects an action,
and Enter applies it:

1. **Accept and publish / retry** creates or updates the issue only after the
   current revision passes review. This also reconciles an interrupted publication.
2. **Refine** returns to the same task's message field for a custom response.
   Queued feedback receives its own planning/review turn and invalidates approval.
3. **Ready for build** applies the repository's existing intake label only after
   the current revision is reviewed, accepted and successfully published.

Publishing never starts Build by itself. The ordinary issue poller handles Ready
issues, using its existing author, budget, claim and concurrency checks. The
planning card links to that issue and shows the resulting build task number in
the current session. If intake is paused, the card says **Ready — intake paused**;
Ctrl-A enables intake when desired. The workflow never enables it automatically.
Refining a linked issue removes its ready label before planning starts. Closed,
actively claimed, building and PR-open issues cannot be refined or republished.

Plans occupy a `<!-- forkdeck:plan:start -->` / `<!-- forkdeck:plan:end -->`
section in the issue description. Text outside that section is preserved, and
each accepted revision adds a short history comment. Forkdeck re-fetches before
publication and refuses to overwrite a changed managed section. Refine to load
the latest issue and review the new revision. These checks are optimistic:
GitHub/GitLab issue edits and labels are not a cross-client transaction.

The issue reference, full draft/review, revision approvals and publication
progress are saved in the history database before remote writes. Restarting never
publishes: unlike other typed tasks, an interrupted issue plan is not resumed by
itself. Retry from F6 after a partial failure.
Uncertain issue/comment creation is reconciled through durable hidden markers;
if the result cannot yet be found, retry reports uncertainty instead of issuing
a duplicate POST. Keep those markers while recovering an interrupted operation.
A failed save blocks remote writes. One forkdeck at a time works on a task (it
takes the task over in the database); separate machines still use the forge's
claim checks.

This uses existing `gh` / `glab` authentication and the selected clone's CLI host
configuration. URL links must match that forge and repository. New issue plans
are local until acceptance; linking an existing ready issue immediately holds it
out of intake. Jira and other providers, issue-comment commands and headless
planning commands are not part of this first version.

Offline acceptance tests use fake agents and forge CLIs:

```sh
cargo test
python3 tests/tui_issue_plans.py target/debug/forkdeck
python3 tests/tui_exit.py target/debug/forkdeck
```

### Conversations, history and resuming

A typed task keeps its conversation. Select it (`↑` `↓`) and type: once it has
finished, that is a follow-up, and the same agent continues in the same tree;
while it works, it is a message the agent gets as soon as its current step ends,
before checks and review. `Ctrl-S` sends it at once instead: the agent is stopped
and carries on with the message. Either way the change goes through the checks
and review again, and the reviewer is told what you asked for since the task.

The agent continues its own conversation in its CLI (`claude --resume`,
`codex exec resume`, `copilot --resume`), so nothing it was told is lost and
nothing is sent twice. When a CLI no longer has the conversation, the agent
starts a new one with the exchanges so far.

Jobs are kept in `~/.local/state/forkdeck/forkdeck.db` (SQLite, the newest 200).
The directory is private (0700), and the database is owner-readable and writable only (0600). They come back when forkdeck starts. A typed task that was cut off by quitting,
Ctrl-C or SIGTERM is marked interrupted and continues by itself from the stage it
had reached; `Ctrl-R` retries one by hand. Issue and pull request jobs come back
for reference only: their labels already put them back in the queue. Several
forkdecks can share the file. `forkdeck history` lists the saved jobs,
`forkdeck history clear` removes them, and `forkdeck resume` continues
interrupted typed tasks without the interface.

### Planning first

With `Ctrl-P` or `Shift-Tab` before `Enter` (or `forkdeck run --plan`), the agent first looks at
the code without changing anything and proposes two to four approaches, marking
the one it recommends. Type a number and press Enter, or type how you want it done
instead; the same agent then does it, in the conversation it planned in. With a
decision server, the fork layer can also ask for a plan on open-ended tasks
(`router.plan = "auto"`, the default; `"never"` turns that off). Issue jobs
have nobody to ask, so they take the recommended option, and the pull request
says which one and what else was proposed.

A number sent during planning is saved as an early choice; other messages remain guidance for the worker. When headless input closes, a waiting plan takes its recommended option.

If the agent asks a question instead of proposing approaches, its question is
shown in the worker's box; type the answer and press Enter. While a task waits
for your choice it holds nothing up: other typed tasks in the directory run
meanwhile, and the waiting task's change is measured from where the directory
is when it carries on.

After `Enter` starts a task, the input box replies to it; `Ctrl-T` starts another.

Typed tasks share the current directory, so they run one at a time. Issue and
review jobs each have a worktree and run `automation.max_parallel` at once.

`forkdeck run "task"` does one job without the interface: lines you type while it
runs are sent to the task, and a line starting with `!` stops the agent to take
it at once. `forkdeck once` does one pass over the configured repositories. Both exit non-zero if a job
fails and stop their agents on SIGTERM or SIGINT, which is enough for a script
or a cron entry.

## Status

Early. The MCP client has been run against the reference MCP server over both
transports, and the CI and deploy stages are tested against a stand-in server;
no stage has been run against a real CI or deployment system yet. The claude
path has been run against the real CLI. The GitHub flow, the
codex stream parser and the Jev wire format are tested against stand-ins that
follow the documented formats, and the `glab` commands have not been run
against a live GitLab yet. The team feature is tested against a stand-in for
`gh api`; its `gh` calls follow the documented REST endpoints and have not
been run by two real machines against one repository. Expect to adjust
`agents.codex` arguments for your codex version. The copilot path is tested
against the real `copilot` binary talking to a stand-in model server (its
bring-your-own-key mode; `cargo test --test copilot_byok -- --ignored`), and
its login and model probe against a real account without a Copilot seat. It
has not run against Copilot's own models yet.

Not there yet: feeding review comments on an open pull request back to the
worker for another round. Interrupted typed tasks already resume as described above.

## License

MIT

## Web UI and daemon

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
reverse proxy. See [OIDC setup and provider instructions](docs/web-ui-oidc.md).

The board, job details (`/jobs/:id`), plans (`/plans`, `/plans/:id`), log tabs, and
phone filters are live. SSE uses the native feed's sequence numbers, bounded replay,
Last-Event-ID, heartbeats, and resnapshot after gaps/restarts. The intake indicator
reflects actual automation state. Scheduler observations retain unknown forge status
and freshness. Finished work without checks appears as **unverified** in Attention.
Composer, replies, automation changes, and plan actions are read-only in this milestone;
disabled controls have the title “Coming in the next milestone”.

Build the frontend before compiling Rust after UI changes:

```sh
cd web
npm ci
npm run check
npm run build
cd ..
cargo build
cargo clippy --all-targets -- -D warnings
cargo test
```

`web/dist` is checked in so a Rust-only checkout builds without Node. Both debug and
release binaries embed the built assets, including local IBM Plex Sans/Mono fonts;
hashed assets receive immutable caching. Commit rebuilt `web/dist` with source changes.
No font CDN is used at runtime. `npm run contracts` in `web/` regenerates TypeScript
contracts from `crates/remote-proto`; the Rust protocol tests check for drift.

For frontend development, run `forkdeck serve --demo`, then `cd web && npm run dev` and
open http://localhost:5173. Vite proxies `/api` to the daemon on 7420; HMR and frontend
source stay on Vite.

After building both frontend and binary, `node web/scripts/screenshots.mjs` starts
temporary servers on ephemeral ports and opens them in Chromium. It captures native
seeded jobs with `forkdeck serve --demo`, then regular `forkdeck serve` against a
private copy of `~/.local/state/forkdeck/forkdeck.db` and its WAL in a temporary
`XDG_STATE_HOME`. The real-history capture disables agent executables and issue
intake; original database hashes must remain unchanged. Both paths use the same API
views. Screenshots at 1440×900 and 390×844, including unverified job detail, are saved
under `docs/web-ui-mockup/screenshots/`. The script also checks that reply controls
stay inside the viewport. Set `CHROMIUM_PATH` if Chromium is installed elsewhere.
It renders the approved mockups with the same local fonts into `/tmp`; run
`python3 web/scripts/compare-screenshots.py` (Pillow required) to refresh the measured
comparison. See [capture notes](docs/web-ui-mockup/screenshots/README.md) for the
remaining content differences from the references.
