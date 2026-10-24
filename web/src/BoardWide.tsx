import { Composer, JobActions, Intake } from "./Actions";
import { Account } from "./Account";
import { Fragment } from "preact";
import { useEffect, useRef, useState } from "preact/hooks";
import { useLocation } from "preact-iso";
import { pill, stageChip, tabClass, type PresentedJob } from "./ui";

interface Props {
  data: PresentedJob[];
  picked?: number;
  /** A job was chosen by URL: its detail floats over the board until closed. */
  open: boolean;
  automation: boolean;
  agents: { claude: boolean; codex: boolean; copilot: boolean };
  live: string;
  busy: number;
  capacity: number;
  workspace: string;
  router: string;
  lanes: { id: string; label: string }[];
}
export function BoardWide({
  data,
  picked,
  open,
  agents,
  live,
  busy,
  capacity,
  workspace,
  router,
  lanes: laneNames,
}: Props) {
  const { route } = useLocation();
  const [activeTab, setTab] = useState("Worker");
  const board = useRef<HTMLDivElement>(null);
  useEffect(() => {
    board.current
      ?.querySelector(".job-card.selected")
      ?.scrollIntoView({ block: "nearest", inline: "nearest" });
  }, [picked]);
  // The detail floats over the board; Escape puts it away.
  useEffect(() => {
    const key = (e: KeyboardEvent) => {
      if (e.key === "Escape" && open) route("/");
    };
    addEventListener("keydown", key);
    return () => removeEventListener("keydown", key);
  }, [open]);
  const jobs = data.map((j) => ({
    ...j,
    pillClass: pill(j.state),
    cardClass: `job-card${open && j.id === picked ? " selected" : ""}`,
    pick: () => route(`/jobs/${j.id}`),
  }));
  const lanes = laneNames.map(({ id, label }) => {
    const inLane = jobs.filter((j) => j.lane === id);
    return { name: label, count: inLane.length, jobs: inLane };
  });
  const selected = jobs.find((j) => j.id === picked) ?? jobs[0];
  const sel = { ...selected, stages: selected.stages.map(stageChip) };
  const tabs = ["Worker", "Reviewer", "Conversation"].map((label) => ({
    label,
    pick: () => setTab(label),
    className: tabClass(label === activeTab),
  }));
  const lines = selected.log.filter((l) =>
    activeTab === "Reviewer"
      ? l[1] === "review"
      : activeTab === "Conversation"
        ? ["queue", "issues"].includes(l[1])
        : l[1] !== "review",
  );
  if (!lines.length) lines.push(["", "queue", "Nothing here yet."]);
  const log = lines.map((l) => ({
    t: l[0],
    text: (l[1] === "worker" ? "" : l[1] + "  ") + l[2],
    className: `log-text log-${l[1]}`,
  }));
  return (
    <div class="app-shell">
      <header class="app-header">
        <div class="brand">
          <svg
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
          <span class="brand-name">forkdeck</span>
        </div>
        <span class="workspace-label">{workspace}</span>
        <div class="grow"></div>
        <div class="connection">
          <span class="live-dot"></span>
          {`${live} · ${busy} slots of ${capacity} busy`}
        </div>
        <Intake />
        <Account />
      </header>
      <div class="app-body">
        <nav aria-label="Main" class="sidebar">
          <a href="/" class="nav-current">
            Board
          </a>
          <a href="/plans" class="nav-link">
            Plans
          </a>
          <a href="/repos" class="nav-link">
            Repositories
          </a>
          <a href="/deploys" class="nav-link">
            Deployments
          </a>
          <a href="/stats" class="nav-link">
            Stats
          </a>
          <a href="/doctor" class="nav-link">
            Doctor & settings
          </a>
          <div class="agent-status">
            <div class="agent-heading">Agents</div>
            {`claude · ${agents.claude ? "ready" : "unavailable"}`}
            <br />
            {`codex · ${agents.codex ? "ready" : "unavailable"}`}
            <br />
            {`copilot · ${agents.copilot ? "ready" : "no seat"}`}
            <br />
            {router}
          </div>
        </nav>
        <main class="board-main">
          <Composer />
          <div class="board-scroll" ref={board}>
            <div class="lanes">
              {lanes.map((lane) => (
                <Fragment key={lane.name}>
                  <section class="lane">
                    <div class="lane-heading">
                      <h2 class="section-label">{lane.name}</h2>
                      <span class="mono-muted">{lane.count}</span>
                    </div>
                    {lane.jobs.map((job) => (
                      <Fragment key={job.id}>
                        <button
                          type="button"
                          onClick={job.pick}
                          class={job.cardClass}
                        >
                          <div class="job-heading">
                            <span>{`#${job.id}`}</span>
                            <span class={job.pillClass}>{job.state}</span>
                          </div>
                          <div class="job-title">{job.title}</div>
                          <div class="muted-small">{job.meta}</div>
                        </button>
                      </Fragment>
                    ))}
                  </section>
                </Fragment>
              ))}
            </div>
          </div>
        </main>
        <aside aria-label="Job detail" class={`detail${open ? " open" : ""}`}>
          <div class="detail-heading">
            <div class="detail-meta">
              <span>{`#${sel.id}`}</span>
              <span class={sel.pillClass}>{sel.state}</span>
              <span>{sel.elapsed}</span>
              <button
                type="button"
                class="detail-close"
                aria-label="Close job detail"
                onClick={() => route("/")}
              >
                ×
              </button>
            </div>
            <h1 class="detail-title">{sel.title}</h1>
            <div class="mono-muted">{sel.route}</div>
            <div class="stage-list">
              {sel.stages.map((st) => (
                <Fragment key={st.label}>
                  <span class={st.className}>{st.label}</span>
                </Fragment>
              ))}
            </div>
            {sel.warn && (
              <>
                <div class="warning">{sel.warn}</div>
              </>
            )}
          </div>
          <div role="tablist" class="detail-tabs">
            {tabs.map((tab) => (
              <Fragment key={tab.label}>
                <button
                  type="button"
                  role="tab"
                  aria-selected={tab.label === activeTab}
                  onClick={tab.pick}
                  class={tab.className}
                >
                  {tab.label}
                </button>
              </Fragment>
            ))}
          </div>
          <div class="log">
            {log.map((line) => (
              <Fragment key={line.t + line.text}>
                <div class="log-row">
                  <span class="log-time">{line.t}</span>
                  <span class={line.className}>{line.text}</span>
                </div>
              </Fragment>
            ))}
          </div>
          <JobActions key={sel.id} id={sel.id} />
        </aside>
      </div>
    </div>
  );
}
