[Documentation](README.md) · [Front page](../README.md)

# The journal, and `forkdeck stats`

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
[model metrics and adaptive routing](adaptive-routing.md) for qualification,
advisory/shadow validation, trial limits, promotion and disable controls.

Worker benchmarks support `--preflight` and explicit call budgets. Use
`forkdeck reviewer-benchmark MANIFEST --output DIRECTORY` for separate labeled
reviewer comparisons. Adaptive policies can select `reported_cost_usd` instead of
tokens when all end-to-end costs are reported; see the
[runtime guide](adaptive-routing.md) and [manifest templates](examples/).
