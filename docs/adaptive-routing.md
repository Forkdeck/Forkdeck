# Model metrics and adaptive routing

Adaptive routing is **off by default**. It changes the starting worker model and
reasoning effort within the baseline CLI. Routing rules/Jev still select the CLI
and tier, and the baseline review model, checks, permissions and fix limits remain
in force. Manual agent selections bypass adaptation. Fix rounds return to the
baseline worker without restarting the task or switching back to the candidate.
A candidate worker error can also escalate once to the baseline, independently
of the local fix allowance (including when that allowance is zero). Benchmarks apply the same retry and include the
failed candidate plus baseline retry in their totals. Escalation stays in effect
for subsequent repairs. CI repairs have a separate allowance, so a local repair
does not consume a configured CI repair. Each review cycle after a CI repair
gets a fresh local allowance. Cancellation never triggers escalation. Disablement is
checked again before candidate dispatch and does not cancel running work.

## Inspect usage

```sh
forkdeck stats --days 30 --by-model
forkdeck stats --by-model --role worker --repo owner/repository --json
forkdeck adaptive --json
```

`stats --by-model` groups attempts by **reported model**, effort and role
(`worker`, `reviewer`, `knowledge`). Unresolved models stay `unknown/default`;
the configured alias is retained separately in the attempt record. Cache reads,
cache writes and reasoning tokens are subsets and are not added to inclusive
totals. Missing measurements remain null. Claude input totals require all three
input categories; incomplete totals do not qualify a candidate.

Task groups attribute end-to-end usage to the initial worker, including reviews
and fixes. Tokens per accepted task include evaluated failures. In this release,
acceptance requires completed work, passing checks and a passing review. Work
without both quality signals is reported as unknown. A PR's existence alone is
not acceptance. Partial spend is a lower bound. Historical journal entries stay
available in ordinary stats and are not added to the attempt totals.

Queue wait is measured once per job/turn, from admission until the shared-directory
lane and concurrency slot are acquired, before routing and invocation. Attempts
carry optional `queue` metadata: admission/dispatch Unix milliseconds, nullable
`wait_ms`, and `process-monotonic-admission-v1` provenance. Text and JSON task
reports expose p90 wait and measured-task coverage; retries/reviews do not add
another queue sample. Historical and benchmark attempts remain unknown.

The scheduler does not restore queued jobs automatically. A crash before dispatch
leaves the durable admission record with null dispatch/wait; re-admitted work gets
a fresh job interval. Durations use the live monotonic clock, never timestamp
subtraction across processes, so downtime is not counted as active queue wait.
Follow-up turns also start fresh intervals. Persistence failures log a queue audit
gap. These additive fields retain attempt schema 2 and schema-1 readability.

Records live under `$XDG_STATE_HOME/forkdeck` (default
`~/.local/state/forkdeck`):

- `queue-v1/*.json`: atomic admission and dispatch snapshots keyed by job/turn.
- `attempts-v1/*.json`: schema 1 and 2 content-free invocation snapshots, allocated before
  launch and atomically replaced as metadata arrives and when the call ends.
  A crash leaves an incomplete record; rewriting an attempt never adds usage.
- `journal.jsonl`: existing completed-job accounting, with optional job/turn and
  attempt links. Daily enforcement continues to use this journal.
- `adaptive-v1.json`: decisions, evidence identities, random draws, assignment
  probabilities, reservations, completed outcomes and persistent suspensions.

No prompts, source contents, tool arguments or raw output are stored in these
records. Typed tasks identify their local repository by working-directory path.
Damaged records and missing coverage are reported rather than treated as zero.

## Produce controlled evidence

Use a dedicated config with `review.mode = "always"`, an explicit reviewer,
nonempty checks, and `knowledge.enabled = false`. Pin the baseline model and
reasoning effort in the usual `[agents.<cli>.models]` and `.efforts` tables.
The benchmark keeps that reviewer fixed for both worker models.

A JSON manifest names a local Git repository, immutable base commits, task
prompts and one candidate. These prompts are inputs you provide, not telemetry.
For a manual-task cohort, use `source: "task"` and the repository's canonical
absolute path as `cohort_repo` (an empty value is filled from `repository`). For
an issue cohort, use `source: "issue"` and its configured `owner/repository` name.

```json
{
  "version": "worker-comparison-2026-10-12",
  "repository": "/absolute/path/to/repository",
  "cohort_repo": "owner/repository",
  "source": "issue",
  "tier": "standard",
  "cli": "claude",
  "candidate": {"model": "YOUR_CANDIDATE_MODEL", "effort": "high"},
  "repeats": 3,
  "max_invocations": 27,
  "fixture": false,
  "lifetime_secs": 604800,
  "cache_state": "Describe the provider cache conditions for these runs",
  "tasks": [
    {
      "id": "representative-task-1",
      "revision": "REPLACE_WITH_A_FULL_40_CHARACTER_COMMIT_HASH",
      "prompt": "The fixed task to execute and evaluate"
    }
  ]
}
```

```sh
forkdeck benchmark benchmark.json --output ./benchmark-results --preflight
forkdeck benchmark benchmark.json --output ./benchmark-results
forkdeck adaptive --evaluate ./benchmark-results/RUN_ID/evidence.json
```

The benchmark **executes agents and check commands**. Each arm gets a disposable
local clone with its remote removed; the harness does not claim issues or
publish changes. Arm order is randomized, repeats keep the same task ID, and
local checks/review and baseline escalation are identical. Attempts go into the
benchmark output directory, never the production journal. Evidence is produced
only when both arms finish with resolved worker models. Interrupted or
infrastructure-blocked runs retain their attempts but produce no qualifying
comparison. Provider caches are not reset by the harness; document their state.

The task-level bootstrap keeps repeats together. Qualification requires at least
30 distinct tasks, complete paired token data, the configured success floor,
quality non-inferiority, p90 latency compliance and a strictly positive savings
interval. Score bounds retain uncertainty when all observed outcomes agree;
30 perfect observations alone do not demonstrate a five-point quality margin.
Raw per-call savings cannot compensate for review/fix overhead.

Evidence pins configuration, prompt/adapter implementation, CLI versions,
reported models, cohort and expiry. Changing those requires fresh evidence.
Selection defaults to tokens per accepted task. `objective = "reported_cost_usd"`
selects complete CLI-reported USD cost per accepted task instead. Cross-provider
routing and reviewer-model adaptation are not enabled.

## Configure a bounded rollout

Add this table to your config, replacing the model, repository and evidence path.
The numbers are explicit policy choices, not a recommendation for every workload.

```toml
[adaptive]
mode = "advisory" # off, advisory, shadow, adaptive
policy = "worker-policy-1"
objective = "tokens" # or reported_cost_usd
evidence = "/absolute/path/to/evidence.json"
candidates = [{ model = "YOUR_CANDIDATE_MODEL", effort = "high" }]
repositories = ["owner/repository"]
sources = ["issue"]
tiers = ["standard"]
max_prompt_bytes = 4096
min_tasks = 30
min_coverage = 0.95
quality_floor = 0.95
noninferiority = 0.05
max_p90_ms = 600000
trial_probability = 0.10
trial_tasks = 100
trial_tokens = 10000000
reserve_tokens = 100000
window_secs = 604800
monitor_tasks = 30
```

Only the candidate named by the pinned evidence can run, and it must also be
allowlisted. Evidence from another repository, source, tier, configuration or
context limit is ineligible. The baseline CLI must already be available.
A configured model/effort CLI argument overrides exploration.

Both advisory and shadow modes record the qualified proposal while executing
the baseline. At least `min_tasks` completed, fully measured validation runs
are required before adaptive dispatch. Switch `mode` to `adaptive` after this
validation; keep the policy parameters unchanged so that validation applies.
Production shadow runs are **not** candidate quality evidence.

Initial candidate probability is capped at 10%; other eligible jobs form the
randomized baseline arm. The probability is an assignment probability, not a
promise that every small batch contains exactly 10% candidate jobs. Task and
token admission caps are absolute. Reservations are locked across processes,
fsynced before dispatch, and retained across restarts. Reservations cover the
whole task, including review, retries and escalation. They are admission limits,
not provider-enforced token ceilings: an in-flight job can overrun, and an overrun
suspends further assignments. Unknown spend, missing decisions and stale
unfinished jobs also suspend the cohort. Existing global limits still apply.

At the declared window boundary, pending/sparse data, quality regression,
latency violations or absent savings retain the baseline and persist a reason.
A completed window closes admission even when it passes. To promote, create a
new policy version and set `promotion_from = "worker-policy-1"`, with fresh
advisory/shadow validation for that new policy. A probability above 10% requires
a completed prior production window with both arms, quality/latency gates and
positive **observed** savings uncertainty. The maximum is 90%, retaining a 10%
baseline holdout. Promotion never uses shadow-only evidence.

```sh
forkdeck adaptive              # recent decisions, predictions and trial results
forkdeck adaptive --json       # full pinned audit records and production reports
forkdeck adaptive --disable    # durable switch for future routing decisions
forkdeck adaptive --enable     # validates evidence; existing suspensions still bind
```

The disable switch is read by running instances at their next routing decision.
Other config edits take effect on restart. Healthy in-flight work is not
cancelled and code changes are not undone. Recover a suspended cohort by fixing
its telemetry/evidence problem and explicitly choosing a newly qualified policy
version; `--enable` does not erase suspension history. Corrupt or unwritable
policy state always executes the baseline and emits an audit-gap message.

## Cohorts, provenance and cost

New attempts and worker evidence use schema 2. Schema 1 records remain readable;
missing cohort, provenance and timing fields stay unknown. Task reports split by
cohort as well as repository, source, model and effort. `prompt-proxy-v1` uses
case-insensitive whole words: fix/bug/regression first, then docs/document/documentation,
then test/tests, otherwise general. Complexity is the baseline routing tier;
context is prompt-small (up to 1024 bytes), prompt-medium (up to 4096), or
prompt-large. These are pre-run proxies, not measured provider context or task difficulty.
The complete repository/tool context is not exposed by the adapters.

Each worker manifest must contain one cohort. Runtime matching requires the same
cohort definition and values. Optionally restrict policy cohorts explicitly:

```toml
cohorts = [{ definition = "prompt-proxy-v1", category = "documentation", complexity = "standard", context = "prompt-small" }]
```

Changes to cohort code, adapters, benchmark implementation or invocation policy
invalidate the evidence fingerprint or policy identity. Attempts include adapter
provenance, routing duration where available, wall time and first nonempty output
latency for streaming production calls. Buffered benchmark output has no first-output
measurement. Queue wait is not currently measured separately.

`reported_cost_usd` requires finite, nonnegative reported costs on **every**
invocation in both arms, including failures, reviews and fixes. Missing cost prevents
qualification, observed savings and promotion, and suspends further cost trials.
Token coverage and token admission budgets still apply. No prices are inferred;
this release does not estimate costs or combine estimates with reported costs.
The objective is pinned in decisions and shown in qualification/production JSON.

## Reproducible benchmark inputs

Start with [the worker template](examples/worker-benchmark.json) or
[the reviewer template](examples/reviewer-benchmark.json). Replace all model/path/
commit placeholders. Templates have schema tests; preflight checks real local
commits, CLI availability, fixed reviewer/check configuration, cohorts and budgets.
Worker `max_invocations` must cover tasks × repeats × (4 × (max_fix_rounds + 1) + 1).
This is a maximum call-count authorization, not a currency or token ceiling.
Preflight runs only local Git inspection and CLI version probes, without executing
tasks or checks. Each run retains a copy of its manifest next to its attempts.

Mark test/fake-agent manifests `fixture: true`. Their evidence is explicitly
nonqualifying, regardless of sample size or apparent savings. Evidence files are
trusted local inputs; do not relabel synthetic results as real measurements.

## Reviewer comparisons

```sh
forkdeck reviewer-benchmark reviewers.json --output ./review-results --preflight
forkdeck reviewer-benchmark reviewers.json --output ./review-results
```

Each case pins a base commit and a change commit and supplies the original task.
A nonempty `defect` labels one known defect; `null` labels a clean change. Include
both kinds. Labels are withheld from the reviewers. The harness compares explicit
baseline/candidate reviewers in randomized order, using disposable clones and the
same read-only review prompt. It reports case-level missed defects, false positives,
unknown verdicts, token coverage, reported USD cost and p90 invocation latency.
A FAIL on a defective case measures case rejection, not proof of locating the
specific defect. Human-audited labels remain necessary. Reviewer results are
`reviewers.json`, never worker qualification evidence, and cannot enable routing.

## Invocation limits

Set `invocation_max_usd` under `[adaptive]` to request a per-call reported-spend
ceiling. This explicit setting also applies while routing is off, and applies to
benchmark, review and fix calls. For Claude, Forkdeck probes the installed CLI's
version and help once per executable per process (cached across concurrent calls)
and adds `--max-budget-usd` only when advertised. Attempt records
name the version, flag and bounded quantity. Restart Forkdeck after a CLI upgrade.
Invalid budgets and conflicting explicit flags are rejected during config loading. A probe failure stops that invocation;
an unsupported flag is recorded as unsupported. Codex currently has no supported
hard-spend/token ceiling in this adapter. Neither adapter claims a hard token cap.
Unsupported versions retain wall-time limits and durable admission reservations;
known task overruns or unknown usage suspend future trials. Provider/tool billing
may exceed a CLI's own reported spend; the flag does not bound external charges.
