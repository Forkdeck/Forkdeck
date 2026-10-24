import { Composer, JobActions, Intake } from "./Actions";
import { Account } from "./Account";
import type { Agents, Lane, Snapshot, LogEntry, Router } from "./wire/forkdeck/web.ts";
import { details, type JobDetail } from "./state";
import type { StageView } from "./contracts";
import { board as liveBoard } from "./live";
import { useEffect, useLayoutEffect, useRef, useState } from "preact/hooks";
import { useLocation } from "preact-iso";
import { pill, stageChip, tabClass, jobPath, type PresentedJob } from "./ui";

export function AgentChips({ agents }: { agents: Agents | null }) {
  return <div class="agent-chips">{Array.from({ length: agents?.wordsLength() ?? 0 }, (_, i) => {
    const chip = agents!.words(i)!;
    const word = chip.word() ?? "";
    const tone = word === "ON CALL" ? chip.agent() : word === "NO ACCESS" ? "red" : word === "VIA COPILOT" ? "blue" : "dim";
    return word && <span class={`agent-chip agent-${tone}`}>{`${chip.agent()?.toUpperCase() ?? ""} ${word}`}</span>;
  })}</div>;
}
export function RouterStatus({ router }: { router: Router | null }) {
  return router && <div class="agent-status router-status">
    <div class="agent-heading">fork layer</div>
    <div>{router.label() || router.kind()}</div>
    {router.endpoint() && <div>{router.endpoint()}</div>}
    {router.kind() !== "rules" && <div>{["not asked yet", "answering", "down, rules in use"][router.healthy()]}</div>}
    <div>{`calls ${router.calls()} · fallbacks ${router.fallbacks()} · last ${router.lastMs()}ms`}</div>
    <div>{`memory hits ${router.remembered()}`}</div>
  </div>;
}
export function BoardStatus() {
  const current = liveBoard.value;
  if (!current) return null;
  const snap = current.snapshot;
  return <div class="board-status">{`running: [${snap.running()}]  queued: [${snap.queued()}]  issues: [${current.automation ? "on" : "off"}] · 24h: ${snap.spentJobs()} jobs / ${snap.spentTokens()} tokens`}</div>;
}
export function SessionLog() {
  const lines = liveBoard.value?.log ?? [];
  return <details class="session-log"><summary>Session log</summary><FollowLog lines={lines} active /></details>;
}
type Line = string[] | LogEntry;
function parts(line: Line): (string | null)[] {
  return Array.isArray(line) ? line : [line.time(), line.who(), line.text()];
}
function FollowLog({ lines, active, reset }: { lines: Line[]; active: boolean; reset?: string }) {
  const pane = useRef<HTMLDivElement>(null);
  const following = useRef(true);
  const [back, setBack] = useState(0);
  const measure = () => {
    const el = pane.current;
    if (!el) return;
    const distance = Math.max(0, el.scrollHeight - el.clientHeight - el.scrollTop);
    following.current = distance < 3;
    const lineHeight = parseFloat(getComputedStyle(el).lineHeight) || 20;
    setBack(following.current ? 0 : Math.ceil(distance / lineHeight));
  };
  const latest = () => {
    following.current = true;
    if (pane.current) pane.current.scrollTop = pane.current.scrollHeight;
    setBack(0);
  };
  useLayoutEffect(latest, [reset]);
  useLayoutEffect(() => {
    if (following.current && pane.current) pane.current.scrollTop = pane.current.scrollHeight;
  }, [back]);
  useEffect(() => {
    const observer = new ResizeObserver(() => {
      if (following.current) latest();
      else measure();
    });
    if (pane.current) observer.observe(pane.current);
    return () => observer.disconnect();
  }, []);
  useLayoutEffect(() => {
    if (following.current) latest();
    else measure();
  }, [lines]);
  useEffect(() => {
    if (!active) return;
    const key = (event: KeyboardEvent) => {
      if (event.defaultPrevented && event.key !== "End") return;
      const el = pane.current;
      if (!el || !el.clientHeight || el.closest("details")?.open === false) return;
      if (event.key === "End") { event.preventDefault(); latest(); }
      if (event.key === "PageUp" || event.key === "PageDown") {
        // Shift pages the session log; plain PageUp/Down pages the job's output.
        if (Boolean(event.shiftKey) !== (reset === undefined)) return;
        event.preventDefault();
        el.scrollTop += (event.key === "PageUp" ? -1 : 1) * el.clientHeight;
        measure();
      }
    };
    addEventListener("keydown", key);
    return () => removeEventListener("keydown", key);
  }, [active, reset]);
  return <>
    {back > 0 && <button type="button" class="log-jump" onClick={latest}>{`↑ ${back} lines back · Jump to latest`}</button>}
    <div class="log" ref={pane} onScroll={measure} tabIndex={0} aria-label={reset === undefined ? "Session output" : "Job output"}>
      {!lines.length && <div class="log-row">Nothing here yet.</div>}
      {lines.map((line, i) => {
        const [time, who, text] = parts(line);
        return <div class="log-row" key={i}><span class="log-time">{time}</span><span class={`log-text log-${who}`}>{(who === "worker" ? "" : (who ?? "") + "  ") + (text ?? "")}</span></div>;
      })}
    </div>
  </>;
}
function StageDetails({ name, stage }: { name: string; stage: StageView }) {
  const duration = `${String(Math.floor(stage.secs / 60)).padStart(2, "0")}:${String(stage.secs % 60).padStart(2, "0")}`;
  const state = { Idle: "idle", Running: "running", Ok: "done", Failed: "failed", Skipped: "skipped" }[stage.state];
  return <section class="stage-details" aria-label={name}>
    <h3>{name}</h3>
    <div>{[stage.agent || stage.label, stage.model, stage.effort].filter(Boolean).join(" · ")}</div>
    <div class="mono-muted">{`${state} · ${duration} · ${stage.tools} tools · ${stage.tokens_in} in · ${stage.tokens_out} out · $${stage.cost.toFixed(2)}`}</div>
    {stage.last && <div class="muted-small">{stage.last}</div>}
  </section>;
}
// These additions are optional until the server detail changes have landed.
type ParityDetail = JobDetail & { prompt_title?: string; source?: { url?: string; branch?: string; workdir?: string } };
interface Props {
  data: PresentedJob[];
  picked?: number;
  /** A job was chosen by URL: its detail floats over the board until closed. */
  open: boolean;
  automation: boolean;
  snapshot: Snapshot;
  live: string;
  busy: number;
  capacity: number;
  workspace: string;
  lanes: Lane[];
}
export function BoardWide({ data, picked, open, snapshot, live, busy, capacity, workspace, lanes: laneNames }: Props) {
  const { route } = useLocation();
  const [activeTab, setTab] = useState("Worker");
  const board = useRef<HTMLDivElement>(null);
  useEffect(() => {
    board.current?.querySelector(".job-card.selected")?.scrollIntoView({ block: "nearest", inline: "nearest" });
  }, [picked]);
  useEffect(() => {
    const key = (e: KeyboardEvent) => { if (e.key === "Escape" && open) route("/"); };
    addEventListener("keydown", key);
    return () => removeEventListener("keydown", key);
  }, [open]);
  const jobs = data.map((j) => ({
    ...j,
    pillClass: pill(j.job.state()),
    cardClass: `job-card${open && j.job.id() === picked ? " selected" : ""}`,
    pick: () => route(jobPath(j.job)),
  }));
  const lanes = laneNames.map((lane) => {
    const inLane = jobs.filter((j) => j.job.lane() === lane.id());
    return { name: lane.label() ?? "", count: inLane.length, jobs: inLane };
  });
  const job = jobs.find((j) => j.job.id() === picked)?.job;
  const native: ParityDetail | undefined = job ? details.value[job.id()] : undefined;
  const detail = native?.board;
  const stages = Array.from(detail ? detail.stages : job?.stagesArray() ?? [], stageChip);
  const lines = detail ? detail.log : Array.from({ length: job?.logLength() ?? 0 }, (_, i) => job!.log(i)!);
  const log = lines.filter((line) => {
    const who = parts(line)[1];
    return activeTab === "Reviewer" ? who === "review"
      : activeTab === "Conversation" ? ["you", "agent"].includes(who ?? "")
      : who !== "review" && who !== "you" && who !== "agent";
  });
  const router = snapshot.router();
  return <div class="app-shell">
    <header class="app-header">
      <div class="brand">          <svg
            width="22"
            height="22"
            viewBox="0 0 24 24"
            fill="none"
            stroke="var(--accent)"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"
            aria-hidden="true"
          >
            <circle cx="6" cy="5" r="2"></circle>
            <circle cx="6" cy="19" r="2"></circle>
            <circle cx="18" cy="12" r="2"></circle>
            <path d="M6 7v10M6 12h4a4 4 0 0 0 4-4V7M14 12h2"></path>
          </svg>
<span class="brand-name">forkdeck{snapshot.titleNote() ? ` ${snapshot.titleNote()}` : ""}</span></div>
      <span class="workspace-label">{workspace}</span>
      <div class="grow" />
      <div class="connection"><span class="live-dot" />{`${live} · ${busy} slots of ${capacity} busy`}</div>
      <Intake /><Account />
      <AgentChips agents={snapshot.agents()} />
      {router?.label() && <span class="router-label">{router.label()}</span>}
    </header>
    <BoardStatus />
    <div class="app-body">
      <nav aria-label="Main" class="sidebar">
        <a href="/" class="nav-current">Board</a>
        <a href="/plans" class="nav-link">Plans</a>
        <a href="/repos" class="nav-link">Repositories</a>
        <a href="/deploys" class="nav-link">Deployments</a>
        <a href="/stats" class="nav-link">Stats</a>
        <a href="/configuration" class="nav-link">Configuration</a>
        <RouterStatus router={router} />
      </nav>
      <main class="board-main">
        <Composer />
        <div class="board-scroll" ref={board}><div class="lanes">
          {lanes.map((lane) => <section class="lane" key={lane.name}>
            <div class="lane-heading"><h2 class="section-label">{lane.name}</h2><span class="mono-muted">{lane.count}</span></div>
            {lane.jobs.map(({ job, pick, cardClass, pillClass }) => <button key={job.id()} type="button" onClick={pick} class={cardClass}>
              <div class="job-heading"><span>{`#${job.id()}${job.queued() ? ` ✉${job.queued()}` : ""}`}</span><span class={pillClass}>{job.state()}</span></div>
              <div class="job-title">{job.title()}</div><div class="muted-small">{job.meta()}</div>
            </button>)}
          </section>)}
        </div></div>
        <SessionLog />
      </main>
      <aside aria-label="Job detail" class={`detail${open ? " open" : ""}`}>
        <a href="/" class="phone-board-back">← Board</a>
        {!job ? <div class="detail-heading"><button type="button" class="detail-close" aria-label="Close job detail" onClick={() => route("/")}>×</button><h1 class="detail-title">{`Job #${picked} is not on the board`}</h1></div> : <>
          <div class="detail-overview">
            <div class="detail-heading">
              <div class="detail-meta"><span>{`#${job.id()}`}</span><span class={pill(detail ? detail.state : job.state())}>{detail ? detail.state : job.state()}</span><span>{detail ? detail.elapsed : job.elapsed()}</span><button type="button" class="detail-close" aria-label="Close job detail" onClick={() => route("/")}>×</button></div>
              <h1 class="detail-title">{detail ? detail.title : job.title()}</h1>
              <div class="detail-source">
                {native?.source?.url && <a href={native.source.url}>Source</a>}
                {native?.summary.pr_url && <a href={native.summary.pr_url}>Open PR</a>}
                {native?.source?.branch && <div>{native.source.branch}</div>}
                {native?.source?.workdir && <div>{native.source.workdir}</div>}
              </div>
              <div class="mono-muted">{detail ? detail.route : job.route()}</div>
              {native?.decision && <div class="routing-gates">
                {native.decision.gates.map((gate) => <div class="routing-gate"><span>{gate.name}</span><meter min={0} max={1} value={gate.value} aria-label={gate.name} /><span>{`${gate.value.toFixed(2)} ${gate.sharp ? "sharp" : "split"} · ${gate.answer}`}</span></div>)}
                {native.has_decision && <div class="muted-small">{native.decision.note || "sharp → runs as answered · split → cautious branch"}</div>}
              </div>}
              <div class="stage-list">{stages.map((st) => <span key={st.label} class={st.className}>{st.label}</span>)}</div>
              {(detail ? detail.warn : job.warn()) && <div class="warning">{detail ? detail.warn : job.warn()}</div>}
            </div>
            {native && <div class="detail-facts">
              <StageDetails name="Worker" stage={native.worker} /><StageDetails name="Reviewer" stage={native.reviewer} />
              <section class="outcome"><h3>Outcome</h3><div>{native.summary.outcome || "waiting on the stages above"}</div><div class="mono-muted">{`checks ${["not run", "passed", "failed"][native.checks]} · review ${["not run", "passed", "failed"][native.summary.verdict]}`}</div>{native.findings.map((finding) => <div>{finding}</div>)}</section>
            </div>}
          </div>
          <div role="tablist" class="detail-tabs">{["Worker", "Reviewer", "Conversation"].map((label) => <button type="button" role="tab" aria-selected={label === activeTab} onClick={() => setTab(label)} class={tabClass(label === activeTab)}>{label}</button>)}</div>
          <FollowLog lines={log} active={open} reset={`${job.id()}/${activeTab}`} />
          <JobActions key={job.id()} id={job.id()} />
        </>}
      </aside>
    </div>
  </div>;
}
