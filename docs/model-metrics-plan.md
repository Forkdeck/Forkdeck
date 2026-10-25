# Model metrics and adaptive routing: implementation scope

Status: implemented in the working tree, default off; live qualification and
production activation remain pending. The acceptance criteria below cover the
attempt ledger, model reports, worker/reviewer benchmarks, cohort and cost
qualification, invocation limits, and durable routing budgets and rollback.
See [the runtime guide](adaptive-routing.md) for commands, supported behavior,
fixture manifests, and limitations. Fixtures cannot qualify production models.

Remaining operational work requires concrete baseline/candidate models, pinned
representative tasks, a fixed reviewer, and explicit operating budgets. Run real
benchmarks, qualify evidence, then exercise bounded rollout before activation.
No live qualification or activation is implied by passing fixture tests.

## 1. Model and token metrics (recommended first)

Goal: show which model and reasoning effort use the fewest tokens per accepted
task, so routing choices rest on evidence rather than tier names.

- Per-invocation attempt record, written before each CLI call and replaced
  atomically as usage arrives. Covers workers, fixes, reviews and knowledge runs.
  No prompts, source, tool arguments or raw output are stored.
- Nullable measurements. Missing usage is unknown, never zero, and an unresolved
  model is `unknown/default`, never the configured alias.
- Cache-read, cache-write and reasoning tokens are subsets of provider totals
  where the provider says so; only add categories that are disjoint.
- Existing journal and daily limits stay authoritative. The ledger is analysis
  only until reconciliation tests justify more.
- `forkdeck stats --by-model` in text and JSON with identical counts and
  denominators: tokens per accepted task (including failed work, reviews and
  rework), fix rounds, p90 latency, incomplete-telemetry rate.
- Observational data is biased (harder tasks go to larger models), so reports
  are descriptive only.

## 2. Controlled benchmarks

Goal: compare candidate models on fixed tasks before trusting them.

- Manifest of pinned base commits and prompts, disposable clones, randomized
  arm order, repeats grouped by task, fixed reviewer and checks.
- Benchmarks never publish changes or write to the production journal.
- **The benchmark must run the same escalation as production**: a candidate
  worker failure escalates to the baseline exactly as it would live, so measured
  acceptance and spend describe the policy being qualified.
- Reviewer benchmarks use labeled defective and clean changes and report missed
  defects and false positives. Their results never enable routing.

## 3. Advisory and adaptive routing (opt-in, default off)

Goal: use the cheapest qualified model without lowering quality.

- Modes `off`, `advisory`, `shadow`, `adaptive`. Baseline route (rules/Jev/manual)
  is computed first; manual overrides always win.
- Adapt only the worker's starting model/effort. Checks, reviewer and
  permissions stay fixed.
- Candidate must pass qualification: enough distinct tasks, complete token data,
  quality non-inferiority, latency ceiling, credible savings. Stale, sparse or
  incompatible evidence keeps the baseline.
- Bounded rollout: low initial candidate share, a baseline holdout, task and
  token admission caps reserved atomically, persistent suspension on quality,
  latency, coverage or budget failure, and a disable switch read at the next
  routing decision.
- Escalation to the baseline happens at most once per task. Track it as an
  explicit per-job flag. Do not infer it from the fix-round counter, otherwise
  a later repair can reuse the candidate model.

## 4. Fix-loop accounting (design constraint for any retry feature)

Local-review repairs and CI repairs must use separate allowances. With both
limits at one, a change repaired after local review must still get its one CI
repair. Every review cycle, including one after CI repair, gets its full local allowance.
Candidate worker failure may escalate once even when local repairs are disabled;
escalation consumes neither repair allowance. Tests cover these boundaries.

## 5. Other candidate features (unscoped)

- Reply and follow-up on finished tasks with context carry-over.
- Per-repository notes surfaced in worker prompts.
- A TUI view over the same aggregates as the CLI reports.

## Out of scope for this change

The unscoped features in §5, live model qualification without operating inputs,
and production activation. Source changes, CLI commands, configuration, persistent
telemetry, and benchmark tooling are included.
