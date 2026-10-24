[Documentation](README.md) · [Front page](../README.md)

# CI and deployment, through MCP

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

## CI feedback

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

## Deploying to staging, and rolling back

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

## What forkdeck does with MCP servers' answers

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
