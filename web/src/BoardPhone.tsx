import { Navigation } from "./Operational";
import { Fragment } from "preact";
import { useState } from "preact/hooks";
import { useLocation } from "preact-iso";
import { tabClass, pill, type PresentedJob } from "./ui";
export function BoardPhone({
  data,
  live,
}: {
  data: PresentedJob[];
  live: string;
}) {
  const { route } = useLocation();
  const [filter, setFilter] = useState(
    data.some((j) => j.group === "Active") ? "Active" : "All",
  );
  const filters = ["Active", "Attention", "Done", "All"].map((label) => ({
    label,
    pick: () => setFilter(label),
    className: `filter ${tabClass(label === filter)}`,
  }));
  const jobs = data
    .filter((j) => filter === "All" || j.group === filter)
    .map((j) => ({ ...j, pillClass: pill(j.state) }));
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
        <span class="phone-brand">forkdeck</span>
        <div class="grow"></div>
        <span class="phone-connection">
          <span class="live-dot"></span>
          {live}
        </span>
      </header>
      <Navigation />
      <div role="tablist" class="phone-tabs">
        {filters.map((f) => (
          <Fragment key={f.label}>
            <button
              type="button"
              role="tab"
              aria-selected={f.label === filter}
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
          <Fragment key={job.id}>
            <button
              type="button"
              onClick={() => route(`/jobs/${job.id}`)}
              class="phone-card"
            >
              <div class="job-heading">
                <span>{`#${job.id}`}</span>
                <span class={job.pillClass}>{job.state}</span>
                <span class="push-right">{job.lane}</span>
              </div>
              <div class="phone-title">{job.title}</div>
              <div class="muted-small">{job.meta}</div>
            </button>
          </Fragment>
        ))}
      </div>
      <form class="phone-composer" onSubmit={(e) => e.preventDefault()}>
        <label htmlFor="ptask" class="sr-only">
          New task
        </label>
        <input
          id="ptask"
          placeholder="New task…"
          class="phone-input"
          disabled
          aria-disabled="true"
          title="Coming in the next milestone"
        />
        <button
          type="button"
          class="phone-start"
          disabled
          aria-disabled="true"
          title="Coming in the next milestone"
        >
          Start
        </button>
      </form>
    </div>
  );
}
