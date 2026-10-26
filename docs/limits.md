[Documentation](README.md) · [Front page](../README.md)

# Limits

forkdeck picks work up by itself, so it has brakes. All of them count over the
last 24 hours from the [journal](metrics-and-stats.md), which means restarting forkdeck
resets nothing.

```toml
[limits]
jobs_per_day = 20          # issues and reviews picked up, all repositories together
tokens_per_day = 5000000   # in and out, both CLIs, typed tasks included
job_mins = 90              # one issue or review may run this long
tokens_per_job = 400000    # and use this many tokens
pause_after_failures = 5   # default
pause_mins = 60            # default
cache_read_weight = 1.0    # default; what a token read from the prompt cache counts for
```

- **`jobs_per_day`** and a repository's own `jobs_per_day` cap how many issues
  and reviews are picked up. Deployments and typed tasks are not counted: one
  follows from a merge and the other from you.
- **`tokens_per_day`** counts everything both CLIs report having used. When it
  is reached nothing more is picked up; a job already running finishes.
- **`job_mins`** stops an issue or review that runs too long, kills its agent,
  and hands the item to a person with the failure label. Deployments are
  exempt; they have their own timeouts and stopping one halfway is worse.
- **`cache_read_weight`** is what a token read from the provider's prompt cache
  counts for in `tokens_per_day`, from 0 to 1. Cache reads are cheap for the
  provider (Anthropic bills them at a tenth of new input), and a long or
  resumed session reads most of its input from the cache. `0.1` keeps the
  daily count close to what was billed; `1.0` counts them in full, as before.
  Cache writes always count in full.
- **`tokens_per_job`** stops an issue or review once it has used this many
  tokens (counted as `tokens_per_day` counts them, all its agents together,
  reviewers running side by side included), kills its agents and hands the
  item to a person with the failure label, like `job_mins`. Typed tasks,
  deployments and the closing knowledge run are exempt. Claude is checked as
  it works; Codex reports when its turn ends and Copilot when it exits, so one
  such run can go past the limit before the job is stopped.
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
