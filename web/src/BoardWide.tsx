import { Account } from "./Account";
import { Fragment } from "preact";
import { useEffect, useRef, useState } from "preact/hooks";
import { useLocation } from "preact-iso";
import { pill, stageChip, tabClass, type PresentedJob } from "./ui";

interface Props {
  data: PresentedJob[];
  picked?: number;
  automation: boolean;
  agents: { claude: boolean; codex: boolean; copilot: boolean };
  live: string;
  busy: number;
  capacity: number;
  workspace: string;
  router: string;
}
export function BoardWide({
  data,
  picked,
  automation,
  agents,
  live,
  busy,
  capacity,
  workspace,
  router,
}: Props) {
  const { route } = useLocation();
  const [activeTab, setTab] = useState("Worker");
  const board = useRef<HTMLDivElement>(null);
  useEffect(() => {
    board.current
      ?.querySelector(".job-card.selected")
      ?.scrollIntoView({ block: "nearest", inline: "nearest" });
  }, [picked]);
  const jobs = data.map((j) => ({
    ...j,
    pillClass: pill(j.state),
    cardClass: `job-card${j.id === picked ? " selected" : ""}`,
    pick: () => route(`/jobs/${j.id}`),
  }));
  const lanes = [
    "Queue",
    "Build",
    "Review",
    "Delivery",
    "Done",
    "Attention",
  ].map((name) => {
    const inLane = jobs.filter((j) => j.lane === name);
    return { name, count: inLane.length, jobs: inLane };
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
  const autoLabel = automation ? "on" : "paused";
  const autoDot = `live-dot${automation ? "" : " paused"}`;
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
        <label class="workspace-label">
          Workspace
          <select
            class="workspace-select"
            disabled
            aria-disabled="true"
            title="Coming in the next milestone"
          >
            <option>{workspace}</option>
          </select>
        </label>
        <div class="grow"></div>
        <div class="connection">
          <span class="live-dot"></span>
          {`${live} · ${busy} slots of ${capacity} busy`}
        </div>
        <button
          type="button"
          aria-disabled="true"
          title={automation ? "Issue intake is on" : "Issue intake is paused"}
          class="intake"
        >
          <span class={autoDot}></span>
          {`Issue intake ${autoLabel}`}
        </button>
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
          <form class="composer" onSubmit={(e) => e.preventDefault()}>
            <label htmlFor="task" class="sr-only">
              New task
            </label>
            <input
              id="task"
              placeholder="New task… (Enter to queue, Ctrl-Enter to send now)"
              class="task-input"
              disabled
              aria-disabled="true"
              title="Coming in the next milestone"
            />
            <label class="agent-label">
              Agent
              <select
                class="agent-select"
                disabled
                aria-disabled="true"
                title="Coming in the next milestone"
              >
                <option>Auto (fork layer)</option>
                <option>claude</option>
                <option>codex</option>
              </select>
            </label>
            <label class="plan-toggle">
              <input
                type="checkbox"
                checked
                onClick={(e) => e.preventDefault()}
                tabIndex={-1}
                class="plan-checkbox"
                aria-disabled="true"
                title="Coming in the next milestone"
              />
              Plan first
            </label>
            <button
              type="button"
              class="start-button"
              disabled
              aria-disabled="true"
              title="Coming in the next milestone"
            >
              Start
            </button>
          </form>
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
        <aside aria-label="Job detail" class="detail">
          <div class="detail-heading">
            <div class="detail-meta">
              <span>{`#${sel.id}`}</span>
              <span class={sel.pillClass}>{sel.state}</span>
              <span>{sel.elapsed}</span>
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
          <form class="reply-form" onSubmit={(e) => e.preventDefault()}>
            <label htmlFor="reply" class="sr-only">
              Reply to this task
            </label>
            <input
              id="reply"
              placeholder="Reply… (queued until the agent finishes)"
              class="reply-input"
              disabled
              aria-disabled="true"
              title="Coming in the next milestone"
            />
            <button
              type="button"
              class="secondary-button"
              disabled
              aria-disabled="true"
              title="Coming in the next milestone"
            >
              Queue
            </button>
            <button
              type="button"
              class="primary-button"
              disabled
              aria-disabled="true"
              title="Coming in the next milestone"
            >
              Send now
            </button>
            <button
              type="button"
              class="stop-button"
              disabled
              aria-disabled="true"
              title="Coming in the next milestone"
            >
              Stop
            </button>
          </form>
        </aside>
      </div>
    </div>
  );
}
