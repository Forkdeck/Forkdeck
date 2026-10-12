# Plan: GitHub Copilot CLI as a third agent

Status: implemented on branch `copilot-cli` (2026-10-12); step-by-step plan in
`docs/superpowers/plans/2026-10-12-copilot-cli.md`. Where this proposal and the code differ, the
"Decisions made while building" section at the end wins.

## Goal

Let `copilot` (the GitHub Copilot CLI) be an end agent next to `claude` and
`codex`: it can be forced with `--agent copilot`, chosen by the fork layer,
used as reviewer, and listed in `review.pr_reviewers`. Like the other two it is
driven as a subprocess and uses its own login; forkdeck makes no model API
calls for it.

Copilot also serves models from several vendors behind one login (which ones
depends on plan, org policy and client). So a user who has only `copilot`
should still get the two-vendor setup forkdeck is built around: copilot
**stands in** for a missing `claude` or `codex` by running a Claude or GPT
model, and any other model the account has can be a named profile for extra
reviews (section 1b).

## Step 0: CLI findings

There is no Copilot subscription to test with, so these come from four sources, each marked:
**[help]** `copilot` 1.0.95 `--help` / `help permissions` / `help environment`;
**[sdk]** type definitions in `@github/copilot-sdk` 1.0.19 (`dist/generated/session-events.d.ts`,
`dist/types.d.ts`, `dist/client.js`), the official client for the same CLI;
**[byok]** real runs of the installed CLI against a fake OpenAI-compatible server on localhost
(`COPILOT_OFFLINE=true COPILOT_PROVIDER_BASE_URL=http://127.0.0.1:<port>/v1`, no GitHub auth);
**[live]** calls against GitHub with an account that has no Copilot seat;
**[docs]** docs.github.com (CLI programmatic reference, enterprise admin page, supported models);
**[issue]** github/copilot-cli issues (#193, #400, #635, #4107, #4337). All checked 2026-10-12.

| Question | Finding | Source |
|---|---|---|
| Prompt from stdin | Piped stdin without `-p` runs one non-interactive turn and exits. Prompt stays out of argv. | byok |
| Stream format | `--output-format json`: JSONL, one event per line, envelope `{"type","data","id","timestamp","parentId","ephemeral"?,"agentId"?}`. | byok, sdk |
| Closing line | Last line is `{"type":"result","sessionId","exitCode","usage":{"premiumRequests","totalApiDurationMs","sessionDurationMs","codeChanges":{"linesAdded","linesRemoved","filesModified"}}}`, with no `data` field. `exitCode` 0 = ok. Process exit code matches. | byok |
| Answer text | `assistant.message` `data.content` (may be `""` when the turn only requests tools; `data.toolRequests[]` then lists them). Streaming deltas (`assistant.message_delta`) are ephemeral and duplicate the final message. | byok, sdk |
| Tools | `tool.execution_start` `data.toolName` (`view`, `create`, `edit`, `bash`, `grep`, `glob`, ...), `data.arguments` (object, e.g. `{"path":...}`, `{"command":...}`), `data.toolTitle`. `tool.execution_complete` `data.success`, `data.error.message`. | byok, sdk |
| Errors | `session.error` `data.{errorType, message, statusCode?}` (`errorType`: authentication, authorization, quota, rate_limit, context_limit, query). Per call: `model.call_failure` `data.{statusCode, errorCode, errorMessage, model}`. Then `result` with `exitCode:1`. | byok, sdk |
| Startup failure | Auth/seat failure before the session starts: plain text on stderr (`Error: Access denied by policy settings ...`), exit 1, no `result` line. | live |
| Model used | `assistant.message` `data.model`, `tool.execution_start` `data.model`, `assistant.usage` `data.model`. Rate-limit auto switch shows as `session.model_change` with `data.cause = "rate_limit_auto_switch"`. | byok, sdk |
| Usage | JSON output has **no tokens**: the only usage is on `result` (`premiumRequests`, durations, code changes); `assistant.usage` is not emitted (issue #4107, open, confirmed by byok). `--usage-output-file <f>` writes, at exit, `modelMetrics.<model>.usage.{inputTokens,outputTokens,cacheReadTokens,cacheWriteTokens}`, `totalNanoAiu`, `totalPremiumRequestCost`, `currentModel`. The `modelMetrics` keys are the models that actually ran. | issue, byok |
| Undocumented types | `model.call_start`, `model.call_final_result` appear in real output but not in the SDK union: CLI and SDK versions drift. Unknown types must be ignored. | byok |
| Model list + policy | Headless JSON-RPC: `copilot --headless --no-auto-update --stdio`, LSP-style `Content-Length` framing; `connect` → `{protocolVersion:3, version}`; `auth.getStatus` → `{isAuthenticated, authType, login}`; `models.list` → `ModelInfo[]` with `id` (e.g. `claude-sonnet-4.5`), `policy.state` (`enabled`/`disabled`/`unconfigured`), `supportedReasoningEfforts`, `billing.multiplier`. No seat: `models.list` fails with JSON-RPC error -32603 containing `"status":403` and `not authorized to use this Copilot feature`. 1.2 s round trip, no premium request. | sdk, live |
| Rejected model | CAPI answers HTTP 400 `{"message":"The requested model is not supported.","code":"model_not_supported","param":"model","type":"invalid_request_error"}` (also for a model not reachable on that endpoint, #4337). In JSON output: `model.call_failure` `data.errorCode:"model_not_supported"`, `statusCode:400`, then `session.error` `errorType:"query"`, `result.exitCode:1`, no tool ran. | issue, byok |
| No model at all | When the account has no enabled model (or CLI access is off), the CLI fails with `No model available. Check policy enablement under GitHub Settings > Copilot` (sometimes prefixed `Execution failed:`). | issue |
| Admin policy | Copilot CLI itself is a client policy: Enabled everywhere / Disabled everywhere / Let organizations decide. "Users can only access AI models that are enabled at the enterprise level." Individual plans enable models in their own Copilot settings. BYOK models are not governed by enterprise policy. | docs, issue |
| Model IDs | Lowercase display name with dashes, dots kept: `gpt-5.4`, `gpt-5.3-codex`, `claude-haiku-4.5`, `claude-sonnet-4.5`, `gpt-5.6-luna`. Docs: the valid list per account is what `/model` (or `models.list`) shows. | docs, issue |
| Permissions | `--allow-tool` / `--deny-tool` with `shell(cmd:*)`, `write(path?)`, `<mcp>(tool)`, `url(...)`; deny beats allow. `--available-tools` / `--excluded-tools` hide tools. Tool names offered to the model: `bash`, `read_bash`, `stop_bash`, `list_bash`, `view`, `create`, `edit`, `grep`, `glob`, `task`, `skill`, `sql`, ... | help, byok |
| Unattended | `--no-ask-user`, `--no-auto-update`. | help |
| Model / effort flags | `--model <id>` (`auto` = Copilot picks), `--reasoning-effort none\|minimal\|low\|medium\|high\|xhigh\|max`. | help |
| Instructions / skills | Loads `AGENTS.md` etc. unless `--no-custom-instructions`. **Also loads personal skills** from `~/.agents/skills` and built-in skills even with that flag (`session.skills_loaded`). For PR reviews of untrusted code: `--excluded-tools skill`. | help, byok |
| Auth env | `COPILOT_GITHUB_TOKEN` > `GH_TOKEN` > `GITHUB_TOKEN` > stored login > `gh` CLI login (`authType:"gh-cli"`). | help, live |
| `--version` | Exit 0, no login or network. | live |

Raw captures (worker run, rejected model) are in the session scratchpad; task 4 copies sanitised
versions to `tests/fixtures/copilot/`.

Still unverified without a seat: which exact models a given account offers (by design: the
probe answers that per account), and the `models.list` response body on a seated account (shape
from the SDK types only).

## 1. Configuration (`src/config.rs`, `forkdeck.example.toml`)

- Add `copilot: AgentCfg` to `AgentsCfg`; `get()` gets a third arm and
  `AgentCfg::copilot()` defaults: `cmd = "copilot"`, empty models, efforts
  via `--reasoning-effort`, `write_args`/`review_args` from step 0, plus
  `stand_in` and `profiles` (1b; `deny_unknown_fields` on these too). `deny_unknown_fields` stays, so a typo is still an error.
- Validation loop (`agents.{}.cmd must not be empty`) iterates all kinds. Use
  one `AgentKind::ALL` constant instead of repeating `[Claude, Codex]` in
  `config.rs`, `main.rs` (doctor) and `orchestrator.rs`.
- `review.pr_reviewers` becomes `Vec<AgentRef>` (1b): accepts `"copilot"` and
  `"copilot:<profile>"`, and old values load unchanged. Default stays
  `["claude", "codex"]`: adding a third reviewer by default would change
  behaviour and cost for existing users.
- Router config: unchanged. The router stays two-way (claude/codex); copilot is reached through
  stand-ins, profiles, `--agent` and the reviewer lists.
- Document the new table in `forkdeck.example.toml` with the usual
  fully-commented block, and fix the README intro ("claude and codex").

## 1b. Copilot profiles (copilot runs other vendors' models)

Copilot serves models from several vendors behind one login. Two uses:

1. **Stand-in.** With only `copilot` installed (or one of claude/codex
   missing), routing, `reviewer = "other"` and `pr_reviewers` keep working, and
   work and review still come from different vendors.
2. **Extra reviewers.** Any other model the account has (whatever `/model`
   lists) can be a named profile used as a reviewer or forced with
   `--agent copilot:<name>`. The router stays two-way (claude/codex).

Which models exist is **per account**: it depends on the plan, the org's model
policies, and the client (the supported-models page marks some models as not
available in some clients, and the CLI column could not be confirmed). So
forkdeck ships built-in profiles only for `claude` and `codex` (Anthropic and
OpenAI models, the ones the stand-in needs) and assumes nothing about Gemini,
Grok, Kimi, MAI or others: those exist only if the user configures them.

### Config

```toml
[agents.copilot]
stand_in = true          # default true: only acts when the real CLI is missing

# Built-in profiles (these values are the defaults; override any tier).
# A tier is a string or a preference list: the first model the account accepts wins.
# IDs follow the documented pattern (gpt-5.4, claude-haiku-4.5); which exist is per account.
[agents.copilot.profiles.claude.models]
light = ["claude-haiku-5.5", "claude-haiku-4.5"]
standard = ["claude-sonnet-5.5", "claude-sonnet-5", "claude-sonnet-4.6"]
heavy = ["claude-opus-5.5", "claude-opus-5", "claude-opus-4.8"]
[agents.copilot.profiles.codex.models]
light = ["gpt-5.4-mini", "gpt-5-mini", "gpt-5.3-codex"]
standard = ["gpt-5.5", "gpt-5.4", "gpt-5.3-codex"]
heavy = ["gpt-5.5", "gpt-5.4", "gpt-5.3-codex"]

# A user profile: any model your `/model` picker lists. Name is free-form.
# [agents.copilot.profiles.second]
# vendor = "xai"         # needed when the ID prefix is not recognised
# models.standard = ["<model id from /model>"]

[review]
pr_reviewers = ["claude", "codex"]           # may add "copilot:second"
reviewer_order = ["claude", "codex"]         # may add "copilot:second"
```

- Each profile takes `models`, `efforts` (tier maps) and optional `vendor`.
  Write/review args are copilot's own, shared by all profiles.
- `profiles.claude` / `profiles.codex` are the stand-ins: they run only when the
  real CLI of that name is missing and `stand_in = true`. Their vendor is fixed
  (anthropic / openai); validation rejects a model of another vendor there.
- A tier as a single string keeps today's configs valid (serde untagged).

### Identity and vendor

- New `AgentRef` (in `state.rs`), what jobs, config and the CLI flag name:
  `Cli(AgentKind)` for `claude` / `codex` / `copilot`, and
  `Profile(String)` for `copilot:<name>`. Parsed from and shown as those
  strings; `pr_reviewers`, `reviewer_order` and `--agent` take it, so the
  old values still load.
- `Vendor` enum: `Anthropic`, `OpenAI`, `Other(String)`, `Unknown`.
  Real `claude` = Anthropic, real `codex` = OpenAI. A copilot model's vendor:
  prefix `claude-` = Anthropic, `gpt-` / `o<digit>` = OpenAI, else the
  profile's `vendor` (free text, compared case-insensitively), else
  `Unknown`. Validation: a user profile whose IDs are not recognised needs
  `vendor`. Copilot's own models (plain `copilot`, may be `auto`) = `Unknown`.
- **"Other" reviewer = different vendor.** `reviewer_for(worker, ..)` takes
  the first entry of `reviewer_order` that resolves and whose vendor differs
  from the worker's. `Unknown` never counts as different (it might be the same
  model), so a review is only cross-vendor when both vendors are known. None
  found: same as worker, flagged as same-vendor review in the job log.

### Resolve

- `resolve(ref, installed, unavailable_models, cfg) -> Option<Exec>` in one
  place; `Exec { cli: AgentKind, label: String, vendor: Vendor, models: Tiers }`.
  - `claude` / `codex`: real CLI if installed; else profile `claude`/`codex`
    via copilot if copilot installed, `stand_in`, and the profile has a model
    not yet rejected; else `None`.
  - `copilot`: copilot installed.
  - `copilot:<name>`: copilot installed, profile defined, a model not yet
    rejected; else `None` (log once, "profile <name> unknown/unavailable").
- `RunSpec` (`src/agents.rs:18`) carries the `Exec` instead of `kind`.
  `command_line` copilot arm uses `exec.models` for the tier.
- `has(..)` in the orchestrator becomes `resolve(..).is_some()`. The fallback
  (`orchestrator.rs:386-394`), reviewer choice (`:616`, `:783`) and PR
  reviewers (`:869-872`) then need no copilot special case: with only
  copilot, claude and codex both resolve, work runs as claude-via-copilot,
  review as codex-via-copilot.
- An unresolvable entry in `pr_reviewers` is skipped with a log line; if none
  resolve, the PR review fails as today when no reviewer is installed.

### Org model policies

Org and enterprise admins turn models on and off, and some are off until an
admin enables them. Any configured model may be missing for a user, and so
may a whole vendor. Source:
https://docs.github.com/en/copilot/reference/ai-models/supported-models

- **Check up front (main path).** `copilot_probe` (section 3) returns every model with its
  `policy.state`. `resolve` treats a profile model as available only if the probe lists it as
  `enabled`; a `disabled` or missing model is skipped without spending a run. The probe result
  is cached for the session; `doctor` and the TUI can refresh it.
- **No probe result** (probe failed or timed out): fall back to learning from use, below.
- **Fallback down the list (backup).** A run counts as "model rejected" when it ends without
  `ok`, no `tool.execution_start` was seen, and `model.call_failure` has
  `errorCode: "model_not_supported"` (or `session.error` / stderr contains "The requested model is
  not supported"). Then add the model to
  `State.unavailable_models`, log it, and rerun with the next model in the tier list. No tool
  ran, so a rerun never redoes edits. Any other failure is a normal failure.
- **No model at all.** Stderr or `session.error` contains "No model available. Check policy
  enablement": treat like no seat (below) for the rest of the session, no rerun.
- **No seat at all.** Probe says logged in but `models.list` 403: copilot counts as installed
  but unusable; `resolve` returns `None` for every copilot profile; `doctor` and the header say
  "copilot: no Copilot access for <login>".
- **Never `auto`** in a profile. Auto picks any enabled model (GPT-5.3-Codex
  as last resort), so a claude profile could quietly run GPT. `auto` is
  allowed only in copilot's own `models`, whose vendor is `Unknown`.
- **Rate-limit auto switch.** User setting `continueOnAutoMode = true` makes the CLI switch to
  auto when rate-limited (`session.model_change`, `cause: "rate_limit_auto_switch"`). "Models
  used" (the `modelMetrics` keys from `usage.json`, plus every `data.model` in the stream) must
  all match the profile's vendor; otherwise the review is flagged same-vendor / unknown, never a
  silent pass.
- **Doctor** shows login, seat, each profile's tier lists with the probe's state per model
  (enabled / disabled / not offered), and models rejected this session.
- Persisting rejected models across restarts is left out: policies change,
  and one fast failure per session is cheap.

### Surface

- Job row and log: `claude via copilot (claude-sonnet-5.5)`,
  `copilot:second (<model>)`.
- Header chips: `CLAUDE via COPILOT` when that is how it resolves; `doctor`
  the same. Startup log: "using copilot for claude and codex" instead of
  "neither claude nor codex was found" when stand-in applies.
- TUI Tab cycle stays `auto → claude → codex → copilot`; profiles are reached
  with `--agent copilot:<name>` and config, not the Tab key.
- Cost: with only copilot, default `pr_reviewers` means two copilot runs per
  PR (two premium requests); each extra profile reviewer is one more.
  Document; don't change defaults.

### Tests

- `AgentRef` parse/display: `claude`, `codex`, `copilot`, `copilot:x`;
  rejects `copilot:` and unknown names.
- Vendor: prefixes, explicit `vendor`, `Unknown` for `auto`; validation
  rejects a GPT ID in `profiles.claude` and an unrecognised ID without
  `vendor`.
- `resolve`: every installed set (none, each alone, each pair, all three)
  with `stand_in` on and off; undefined profile; exhausted profile.
- `reviewer_for`: picks first different vendor in order; `Unknown` worker
  or candidate never counts as different; falls back to same with flag.
- Model fallback: fake copilot rejects the first model with the policy error,
  run retries with the second; rejecting all claude models makes
  `resolve(claude)` fail; other errors don't retry; `auto` never chosen for a
  profile.
- Config: tier as string or list; `pr_reviewers = ["claude","copilot:x"]`
  loads; old configs load unchanged.
- `command_line`: each profile's model per tier; empty model omits `--model`.
- Integration: only a fake `copilot` on PATH; typed task routes to claude,
  runs `copilot --model <claude medium>`; reviewer runs with the codex
  profile model; a `copilot:x` PR reviewer runs with its model; assert on the
  argv the fake script records.

## 2. Routing (`src/state.rs`, `src/router.rs`, `src/orchestrator.rs`)

The code assumes exactly two agents; that is the real work.

- `AgentKind`: add `Copilot`, `name()` = `"copilot"`, `ALL`.
- Replace `other()` with `reviewer_for(worker, ..) -> AgentRef` (rules in
  1b: first entry of `review.reviewer_order`, default `claude, codex`, that
  resolves and has a different vendor; else the worker itself, flagged). Today's two-agent behaviour is the special case. Used at
  `orchestrator.rs:387-389` (fallback when the chosen agent is missing),
  `:616` (reviewer, typed/issue jobs) and `:783` (reviewer, follow-ups).
  Honour `review.reviewer = "same"` as today. "Installed" here means
  `resolve(..)` succeeds (1b), so stand-ins count.
- Availability: replace `have_claude`/`have_codex` bools and the `(bool,bool)`
  from `available()` with `HashSet<AgentKind>` (or a small array) in `State`;
  `detect_agents` probes all three in parallel; the "neither ... installed"
  message lists all kinds.
- Router: unchanged, two-way (see section 1).
- Low confidence ("split") keeps its meaning: default agent, next tier up,
  review.
- TUI: Tab cycles `None → Claude → Codex → Copilot → None` (`main.rs:651`);
  header chip `COPILOT ok/--` and a colour in `ui.rs`; the help text that says
  "picks claude or codex". CLI `--agent` parser and its error message accept
  `copilot`.

## 3. CLI execution (`src/agents.rs`)

- New arm in `command_line` for `AgentKind::Copilot`:
  `--output-format json --no-ask-user --no-auto-update`,
  `--usage-output-file <run tmp dir>/usage.json`, `--model <m>` when set,
  `--reasoning-effort <e>` when set, then `write_args` or `review_args`. The prompt goes to stdin
  (no `-p`), as for codex.
- Defaults: `write_args = ["--allow-tool", "write", "--deny-tool", "shell"]` (like claude's
  acceptEdits); `review_args = ["--deny-tool", "write", "--deny-tool", "shell",
  "--excluded-tools", "skill"]`. Never `--allow-all` / `--yolo`.
- Spawn path stays shared: process group kill, timeout, cancel, stderr tail, and "stream must
  reach its closing event" all apply unchanged.
- `on_path` uses `--version` (no network). New `agents::copilot_probe(cmd) -> Probe` speaks the
  headless JSON-RPC (step 0 row "Model list + policy"): `connect`, `auth.getStatus`,
  `models.list`, then kills the process; 10 s timeout. Returns `{ logged_in, login, entitled,
  models: Vec<(id, PolicyState)> }`; a 403 from `models.list` = logged in but no seat. Run once at
  startup in the background (after `detect_agents`) and by `doctor`; cached in `State`. Needs a
  small Content-Length reader/writer over the child's stdio (serde_json only, no new crate).
- Environment: pass through `COPILOT_*`, `GH_TOKEN`, `GITHUB_TOKEN`, `GH_HOST`; never log them.
  Existing MCP secret blanking already covers values named in `${NAME}` config.
- After the process exits, read `usage.json` (if present): sum `modelMetrics.*.usage` into the
  run's tokens and keep the `modelMetrics` keys as "models used". Missing or unreadable file:
  tokens stay 0, models used = those seen in the stream.
- Demo mode: `demo::run` needs nothing beyond handling the new variant.

## 4. Output parsing (`src/agents.rs`)

`parse_copilot(line) -> Vec<Parsed>`, same contract as the others, mapping from step 0:

| Line | Parsed |
|---|---|
| not JSON | `Text(line)` |
| `assistant.message`, `content` non-empty | `Text(content)` |
| `assistant.message`, empty content | `Nothing` (its tools arrive as `tool.execution_start`) |
| `tool.execution_start` | `Tool(tool_summary(toolName, arguments))`; existing `tool_summary` already reads `path`, `command`, `pattern` |
| `tool.execution_complete`, `success:false` | `Note("<tool> failed: <error.message>")` |
| `assistant.usage` (not emitted today, #4107; kept for when it is) | `Usage { inputTokens, outputTokens }` |
| `session.error` | `Error(message)` |
| `session.warning` | `Note(message)` |
| `session.model_change` with `cause` | `Note("model switched to <newModel> (<cause>)")` |
| `result` | `Final { ok: exitCode == 0, text: "", tokens 0, cost: 0.0 }`; tokens are filled from `usage.json` after exit (section 3). AI credits are not dollars, so cost stays 0 |
| anything else, incl. all `ephemeral` deltas and unknown types | `Nothing` |

- `Final.text` is empty; the answer is the last `Text`, as for codex.
- Without a `result` line the run is not `ok` even on exit 0, so a truncated stream cannot count
  as a pass. "No verdict is never a pass" stays intact for reviews.
- Reviewer verdict logic (`VERDICT: PASS|FAIL`, last line) is agent-agnostic and untouched.
- Dispatch in `spawn_and_read` becomes a `match` over three kinds.
- The parser also records, per run, the models seen in `data.model` and whether any
  `tool.execution_start` happened; section 1b uses both (retry safety, vendor check).

## 5. Validation

Unit tests (same style as `claude_stream_lines`, `codex_stream_lines`):
- `copilot_stream_lines`: the captured lines from `tests/fixtures/copilot/`
  (worker run, rejected model), one assertion per row of the section 4
  table, plus plain text, an unknown type (`model.call_start`) and an
  ephemeral delta (all `Nothing` except plain text).
- `copilot_probe` framing: Content-Length encode/decode round trip; the 403
  error body from step 0 maps to "logged in, no seat"; a `models.list`
  sample maps `policy.state` correctly.
- `command_line` snapshot per mode: worker vs reviewer args, empty model
  omitted, prompt not in argv.
- Config: defaults, overrides, `deny_unknown_fields` rejects a typo,
  empty `cmd` rejected, `pr_reviewers = ["copilot"]` loads.
- Router: rules with copilot keywords; with none it never picks copilot;
  `from_answers` accepts `"copilot"` and rejects unknown names; low
  confidence still goes to the default agent.
- `reviewer_for`: all 3 present, 2 present, 1 present (self-review),
  configured order.

Integration (`tests/automation.rs`): it already fakes agent CLIs; add a fake
`copilot` script emitting the fixture streams and cover a worker run, a review
that fails and one that passes, a truncated stream (must fail), a rejected
model followed by a rerun with the next model, and copilot-as-PR-reviewer next
to claude.

Real binary without a seat (`tests/copilot_byok.rs`, `#[ignore]`, run with
`cargo test -- --ignored` when `copilot` is on PATH): a fake OpenAI-compatible
server on a free localhost port (std `TcpListener`, SSE chat completions:
first a tool call, then text ending in a verdict), and the real CLI pointed
at it with `COPILOT_OFFLINE=true COPILOT_PROVIDER_BASE_URL=...`. It checks the
parser against the CLI version actually installed, that reviewer args block
`edit`/`create`/`bash` (fake server requests them; expect
`tool.execution_complete` `success:false` or no execution), and that a 400
from the server gives a non-`ok` run with no tool executed. This catches
CLI/SDK schema drift without a subscription.

Manual (needs a Copilot seat, so done by someone who has one, recorded in the PR):
1. `forkdeck doctor`: login, seat, per-model policy state; also logged out and
   no-seat (no-seat checked already: 403 from `models.list`).
2. `forkdeck --agent copilot` on a throwaway repo: edits a file, review runs.
3. Review run in `review_args` mode: confirm it cannot write (try a task that
   asks it to) and cannot run shell.
4. Kill mid-run: no orphan processes (process group).
6. Only `copilot` on PATH (hide claude/codex): a typed task runs as
   claude-via-copilot, review as codex-via-copilot; the job row shows both
   models; `doctor` and header chips say "via copilot".
7. Policy-denied account: `doctor` and the job error show the stderr reason.
8. Org with a Claude model disabled (or all Claude models off): run falls
   back down the list, or reviews same-vendor with the flag shown.

Gate for merge: `cargo fmt --check`, `cargo clippy -- -D warnings`,
`cargo test`, plus the manual list above recorded in the PR.

## Risks and decisions

- **CLI churn.** Copilot CLI is young; flags and event names change (real
  output already has types the SDK doesn't list). Keep parsing tolerant
  (unknown → `Nothing`), put all flag knowledge in the one `command_line` arm
  and config defaults, and keep the BYOK test as the drift alarm.
- **Built without a seat.** Error codes, ID format and usage come from GitHub
  docs, issues and BYOK runs; only a seated account's real `models.list` body
  and model set are untested. The probe's policy state is the main check,
  rerun on `model_not_supported` the backup. First seat holder runs the
  manual list.
- **Usage file.** `--usage-output-file` is undocumented outside `--help`; if
  it disappears, tokens fall back to 0 and the vendor check to stream
  `data.model` only.
- **Permissions.** Unattended write access is the main risk. Default to the
  narrowest tool allow-list that lets edits work; document that loosening it
  is the user's call, as with `acceptEdits`.
- **Cost model differs.** Copilot is per-seat/premium-request, so
  `Final.cost` is 0 and the cost column shows nothing for it; do not invent
  numbers.
- **Backwards compatibility.** With no config change, routing, reviewer choice
  and PR reviewers behave exactly as today; copilot is opt-in via `--agent`,
  `pr_reviewers`, or non-empty router descriptions/keywords.

## Order of work

1. Step 0 (done, from docs/SDK/BYOK runs). Copy sanitised captures into
   `tests/fixtures/copilot/` as part of task 4.
2. `AgentKind::Copilot` + `ALL` + availability set + `reviewer_for`
   (refactor with claude/codex tests still green, no behaviour change).
3. Config + example file, including `stand_in` and copilot profiles.
4. `command_line` + `parse_copilot` + fixtures + tests; `copilot_probe`.
5. Profiles: `AgentRef`, `Vendor`, `resolve`, `Exec` in `RunSpec`,
   model fallback, surface (1b).
6. Router (rules, then jev) + TUI/CLI surface.
7. Integration tests (incl. copilot-only PATH), BYOK real-binary test,
   docs, manual checks (seat holder).

## Decisions made while building

- **Router stays two-way.** No `copilot_is_for` / `copilot_keywords`; `router.default_agent` must
  be claude or codex (validated).
- **Tier names** are `light` / `standard` / `heavy`, as everywhere else in the config.
- **Worker gets no shell**, matching claude's `acceptEdits`. Checked against the real binary
  (`tests/copilot_byok.rs`): copilot refuses with `code: "denied"`, it does not prompt or hang.
- **No `--no-custom-instructions` for reviews.** claude and codex read `CLAUDE.md` / `AGENTS.md` in
  reviews too; forkdeck's protection is the existing steering warning on the PR comment, which
  copilot reviews get as well. Skills are excluded for reviewers (`--excluded-tools skill`).
- **Same-vendor reviews** are flagged in the job log, not in the posted comment.
- **Model refusal** is detected from `model.call_failure` `errorCode: "model_not_supported"` (or the
  same text on stderr) before any `tool.execution_start`; the run is repeated with the next model
  in the tier list, and the refused model is skipped until restart. "No model available" marks
  copilot unusable for the session.
- **PR review jobs** route (for the effort tier) before choosing reviewers, so the tier is known
  when a profile's model is picked.
- **Formatting:** the repository is hand-formatted; never run `cargo fmt` on it.

