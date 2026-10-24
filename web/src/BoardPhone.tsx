import { Composer, Intake } from "./Actions";
import { AgentChips, BoardStatus, SessionLog, RouterStatus } from "./BoardWide";
import { board } from "./live";
import { Account } from "./Account";
import type { Lane } from "./wire/forkdeck/web.ts";
import { Navigation } from "./Operational";
import { Fragment } from "preact";
import { useState } from "preact/hooks";
import { useLocation } from "preact-iso";
import { tabClass, pill, jobPath, type PresentedJob } from "./ui";
export function BoardPhone({
  data,
  live,
  lanes,
}: {
  data: PresentedJob[];
  live: string;
  lanes: Lane[];
}) {
  const { route } = useLocation();
  const [filter, setFilter] = useState(
    data.some((j) => j.group === "Active") ? "Active" : "All",
  );
  const named = (id: string) => lanes.find((l) => l.id() === id)?.label() ?? id;
  const filters = ["Active", "Attention", "Done", "All"].map((id) => ({
    id,
    label: named(id),
    pick: () => setFilter(id),
    className: `filter ${tabClass(id === filter)}`,
  }));
  const jobs = data
    .filter((j) => filter === "All" || j.group === filter)
    .map((j) => ({
      ...j,
      pillClass: pill(j.job.state()),
      laneLabel: named(j.job.lane() ?? ""),
    }));
  return (
    <div class="phone">
      <header class="phone-header">
        <svg
          width="20"
          height="20"
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
        <span class="phone-brand">forkdeck{board.value?.snapshot.demo() ? " (demo)" : ""}</span>
        <div class="grow"></div>
        <span class="phone-connection">
          <span class="live-dot"></span>
          {live}
        </span>
        <Intake />
        <Account />
      </header>
      <AgentChips agents={board.value?.snapshot.agents() ?? null} />
      <details class="phone-router">
        <summary>{board.value?.snapshot.router()?.label() || board.value?.snapshot.router()?.kind()}</summary>
        <RouterStatus router={board.value?.snapshot.router() ?? null} />
      </details>
      <BoardStatus />
      <Navigation />
      <div role="tablist" class="phone-tabs">
        {filters.map((f) => (
          <Fragment key={f.label}>
            <button
              type="button"
              role="tab"
              aria-selected={f.id === filter}
              onClick={f.pick}
              class={f.className}
            >
              {f.label}
            </button>
          </Fragment>
        ))}
      </div>
      <div class="phone-list">
        {jobs.map((job) => (
          <Fragment key={job.job.id()}>
            <button
              type="button"
              onClick={() => route(jobPath(job.job))}
              class="phone-card"
            >
              <div class="job-heading">
                <span>{`#${job.job.id()}${job.job.queued() ? ` ✉${job.job.queued()}` : ""}`}</span>
                <span class={job.pillClass}>{job.job.state()}</span>
                <span class="push-right">{job.laneLabel}</span>
              </div>
              <div class="phone-title">{job.job.title()}</div>
              <div class="muted-small">{job.job.meta()}</div>
            </button>
          </Fragment>
        ))}
      </div>
      <SessionLog />
      <Composer />
    </div>
  );
}
