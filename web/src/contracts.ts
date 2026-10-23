// Generated from crates/remote-proto. Run npm run contracts to refresh.
export type BoardProjection = { lane: string, state: string, meta: string, elapsed: string, route: string, stages: Array<number>, log: Array<Array<string>>, warn: string, };
export type Agent = "Auto" | "Claude" | "Codex";
export type Status = "Queued" | "Routing" | "Working" | "Reviewing" | "Fixing" | "Publishing" | "Ci" | "Deploying" | "RollingBack" | "Done" | "Failed" | "Cancelled";
export type StageState = "Idle" | "Running" | "Ok" | "Failed" | "Skipped";
export type Which = "Worker" | "Reviewer";
export type JobSummary = { id: number, title: string, 
/**
 * "manual", "issue", "pr" or "deploy".
 */
kind: string, 
/**
 * What the job came from, for display: "owner/repo#12", a short sha, or empty.
 */
origin: string, url: string, status: Status, 
/**
 * Chosen agent once routed, else empty.
 */
agent: string, tier: string, 
/**
 * When the job (or its latest reply) started, Unix seconds.
 */
created_unix: number, 
/**
 * 0 = no verdict yet, 1 = pass, 2 = fail.
 */
verdict: number, pr_url: string, rounds: number, 
/**
 * A finished typed task: [`Call::Reply`] continues it.
 */
can_reply: boolean, outcome: string, };
export type StageView = { agent: string, label: string, model: string, effort: string, state: StageState, last: string, tools: number, tokens_in: number, tokens_out: number, cost: number, secs: number, 
/**
 * Lines ever pushed to this stage's tail; the tail holds the newest ones.
 */
pushed: number, };
export type GateView = { name: string, answer: string, value: number, sharp: boolean, };
export type DecisionView = { agent: string, tier: string, review: boolean, gates: Array<GateView>, source: string, ms: number, note: string, };
export type TurnView = { ask: string, answer: string, };
export type JobDetail = { summary: JobSummary, prompt: string, has_decision: boolean, decision: DecisionView, worker: StageView, reviewer: StageView, worker_tail: Array<string>, reviewer_tail: Array<string>, turns: Array<TurnView>, 
/**
 * 0 = not run, 1 = passed, 2 = failed.
 */
checks: number, findings: Array<string>, };
export type LogLine = { 
/**
 * Position in the session log, counting every line ever logged.
 */
seq: number, time: string, who: string, msg: string, };
export type RepoView = { name: string, label: string, last_error: string, };
export type Snapshot = { 
/**
 * Seq of the last event folded into this snapshot; subscribe from here.
 */
seq: number, automation: boolean, stopping: boolean, have_claude: boolean, have_codex: boolean, jobs: Array<JobSummary>, log: Array<LogLine>, repos: Array<RepoView>, };
export type ServerInfo = { proto: number, version: string, host: string, device: string, };
export type Event = "Resync" | { "Job": { job: JobSummary, } } | { "Stage": { id: number, which: Which, stage: StageView, } } | { "Tail": { id: number, which: Which, from: number, lines: Array<string>, } } | { "Log": { lines: Array<LogLine>, } } | { "Automation": { on: boolean, } };
