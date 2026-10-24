import { invalidation } from "./state";
import { Account } from "./Account";
import { useEffect, useState } from "preact/hooks";
import type { ComponentChildren } from "preact";
const pages = [
  ["/", "Board"],
  ["/plans", "Plans"],
  ["/repos", "Repositories"],
  ["/deploys", "Deployments"],
  ["/stats", "Stats"],
  ["/doctor", "Doctor & settings"],
];
export function Navigation() {
  return (
    <nav class="operations-nav" aria-label="Main">
      {pages.map(([href, title]) => (
        <a
          href={href}
          aria-current={location.pathname === href ? "page" : undefined}
        >
          {title}
        </a>
      ))}
    </nav>
  );
}
function Panel({
  title,
  children,
}: {
  title: string;
  children: ComponentChildren;
}) {
  return (
    <section class="operation-panel">
      <h2>{title}</h2>
      {children}
    </section>
  );
}
// Preserve nullable observations and every denominator, including newly added metrics fields.
function RecordView({ data }: { data: unknown }) {
  if (data == null || data === "")
    return <span class="mono-muted">unknown</span>;
  if (Array.isArray(data))
    return data.length ? (
      <div>
        {data.map((v) => (
          <div class="record-row">
            <RecordView data={v} />
          </div>
        ))}
      </div>
    ) : (
      <span class="mono-muted">None observed</span>
    );
  if (typeof data === "object")
    return (
      <dl class="fields">
        {Object.entries(data).map(([k, v]) => (
          <div>
            <dt>{k.replaceAll("_", " ")}</dt>
            <dd>
              <RecordView data={v} />
            </dd>
          </div>
        ))}
      </dl>
    );
  return <span>{String(data)}</span>;
}
const time = (v: number | null) =>
  v == null ? "unknown" : new Date(v * 1000).toLocaleString();
interface Repo {
  repo: string;
  provider: string;
  base: string;
  intake_labels: string[];
  automation: boolean;
  last_poll: number | null;
  last_error: string | null;
  hold: string | null;
  limits: unknown;
  allowed_authors: string[];
  claims: unknown[];
  claims_observed: number | null;
  claims_error: string | null;
  plans: {
    id: number | string;
    title: string;
    status: string;
    scheduler: string;
  }[];
}
interface Deploy {
  repo: string;
  environment: string;
  current_revision: string | null;
  rollout_revision: string | null;
  last_attempted_revision: string | null;
  last_good_revision: string | null;
  saved: unknown;
  jobs: {
    id: number;
    revision: string;
    status: string;
    started: number;
    finished: number | null;
    health: boolean | null;
    outcome: string;
    pipeline: unknown;
    rollout: unknown;
    steps: unknown[];
  }[];
}
interface Outcome {
  jobs: number;
  done: number;
  failed: number;
  cancelled: number;
}
interface Stats {
  days: number;
  error?: string;
  summary?: Record<string, unknown> & {
    issues: Outcome;
    tasks: Outcome;
    reviews: Outcome;
    deploys: Outcome;
  };
  metrics?: {
    note: string;
    damaged_records: number;
    historical_jobs_without_attempts: number;
    groups: unknown[];
    tasks: unknown[];
  };
  metrics_error?: string;
  pull_requests: {
    count: number;
    issue_jobs: number;
    pr_rate: number | null;
    first_pass_rate: number | null;
    first_pass: number;
    median_secs: number | null;
    p90_secs: number | null;
  };
  router: unknown;
}
interface Doctor {
  checks: { name: string; status: string; detail: unknown }[];
  settings: unknown;
  audit: unknown;
}
export function Operational({
  page,
}: {
  page: "repos" | "deploys" | "stats" | "doctor";
}) {
  const [data, setData] = useState<unknown>(null),
    [error, setError] = useState("");
  const [updated, setUpdated] = useState<Date>();
  useEffect(() => {
    let alive = true;
    const load = async () => {
      try {
        const r = await fetch(`/api/v1/${page}`);
        if (!r.ok) throw new Error(`HTTP ${r.status}`);
        const next: unknown = await r.json();
        if (alive) {
          setData(next);
          setError("");
          setUpdated(new Date());
        }
      } catch (e) {
        if (alive) setError(String(e));
      }
    };
    void load();
    const unsubscribe = invalidation.subscribe(() => { void load(); });
    return () => {
      alive = false;
      unsubscribe();
    };
  }, [page]);
  const title = pages.find(([url]) => url === `/${page}`)![1];
  return (
    <div class="app-shell">
      <header class="app-header">
        <a class="brand-link plan-brand" href="/">
          forkdeck
        </a>
        <span class="muted-label">{title}</span>
        <span class="grow" />
        <span class="mono-muted">
          {updated ? `Observed ${updated.toLocaleTimeString()}` : "Connecting…"}
        </span>
        <Account />
      </header>
      <div class="app-body">
        <aside class="operations-sidebar">
          <Navigation />
        </aside>
        <main class="operations-main">
          <h1>{title}</h1>
          {error && <p role="alert">{error}</p>}
          {data == null ? (
            <p role="status">Loading…</p>
          ) : (
            <>
              {page === "repos" &&
                ((data as Repo[]).length ? (
                  (data as Repo[]).map((r) => (
                    <Panel title={r.repo}>
                      <span
                        class={`task-pill ${r.automation ? "tone-merged" : "tone-you"}`}
                      >
                        {r.automation ? "Automation on" : "Paused"}
                      </span>
                      <RecordView
                        data={{
                          provider: r.provider,
                          base_branch: r.base,
                          intake_labels: r.intake_labels,
                          last_poll: time(r.last_poll),
                          last_error: r.last_error,
                          hold:
                            r.hold ??
                            (r.last_poll
                              ? "No hold observed at last poll"
                              : "unknown"),
                          allowed_authors: r.allowed_authors.length
                            ? r.allowed_authors
                            : "Anyone",
                        }}
                      />
                      <h3>Limits · zero means unlimited</h3>
                      <RecordView data={r.limits} />
                      <h3>Teammate claims</h3>
                      <p class="mono-muted">
                        Observed {time(r.claims_observed)} ·{" "}
                        {r.claims_error || "No recorded error"}
                      </p>
                      <RecordView data={r.claims} />
                      <h3>Plan scheduling</h3>
                      {r.plans.length ? (
                        r.plans.map((p) => (
                          <p>
                            <a href={`/plans/${p.id}`}>{p.title}</a> ·{" "}
                            {p.status}
                            <br />
                            {p.scheduler}
                          </p>
                        ))
                      ) : (
                        <p>No plans observed.</p>
                      )}
                    </Panel>
                  ))
                ) : (
                  <p>No configured repositories.</p>
                ))}
              {page === "deploys" && (
                <>
                  {(data as { error: string | null }).error && (
                    <p role="alert">{(data as { error: string }).error}</p>
                  )}
                  {(data as { repositories: Deploy[] }).repositories.length ? (
                    (data as { repositories: Deploy[] }).repositories.map(
                      (r) => (
                        <Panel title={`${r.repo} · ${r.environment}`}>
                          <h3>Current & last good revision</h3>
                          <RecordView
                            data={{
                              current_revision: r.current_revision,
                              rollout_revision: r.rollout_revision,
                              last_attempted_revision:
                                r.last_attempted_revision,
                              last_good_revision: r.last_good_revision,
                            }}
                          />
                          <details>
                            <summary>Persistent rollout state</summary>
                            <RecordView data={r.saved} />
                          </details>
                          <h3>Pipeline, rollout & history</h3>
                          {r.jobs.length ? (
                            r.jobs.map((j) => (
                              <div class="record-row">
                                <p>Job #{j.id}</p>
                                <RecordView
                                  data={{
                                    revision: j.revision,
                                    status: j.status,
                                    started: time(j.started),
                                    finished: time(j.finished),
                                    health:
                                      j.health == null
                                        ? null
                                        : j.health
                                          ? "verified healthy"
                                          : "verification failed",
                                    outcome: j.outcome,
                                    pipeline: j.pipeline,
                                    rollout: j.rollout,
                                    steps: j.steps,
                                  }}
                                />
                              </div>
                            ))
                          ) : (
                            <p>No deployment jobs observed.</p>
                          )}
                        </Panel>
                      ),
                    )
                  ) : (
                    <p>No deployment environments configured.</p>
                  )}
                </>
              )}
              {page === "stats" && <StatsContent data={data as Stats} />}
              {page === "doctor" && (
                <>
                  {(data as Doctor).checks.map((c) => (
                    <Panel title={c.name}>
                      <span
                        class={`task-pill ${c.status === "ok" ? "tone-merged" : "tone-you"}`}
                      >
                        {c.status}
                      </span>
                      <RecordView data={c.detail} />
                    </Panel>
                  ))}
                  <Panel title="Action audit log"><RecordView data={(data as Doctor).audit} /></Panel>
                  <Panel title="Effective configuration · secrets redacted">
                    <RecordView data={(data as Doctor).settings} />
                  </Panel>
                </>
              )}
            </>
          )}
        </main>
      </div>
    </div>
  );
}
function StatsContent({ data: s }: { data: Stats }) {
  return (
    <>
      <p>
        Last {s.days} days
        {s.summary
          ? ` · ${s.summary.issues.jobs + s.summary.tasks.jobs + s.summary.reviews.jobs + s.summary.deploys.jobs} jobs`
          : ""}
      </p>
      {s.error && <p role="alert">{s.error}</p>}
      {s.summary && (
        <>
          <div class="stats-grid">
            {(["issues", "tasks", "reviews", "deploys"] as const).map((key) => {
              const o = s.summary![key];
              return (
                <Panel title={`${key} · ${o.jobs} jobs`}>
                  <div
                    class="outcome-chart"
                    role="img"
                    aria-label={`${o.done} done, ${o.failed} failed, ${o.cancelled} cancelled`}
                  >
                    {(["done", "failed", "cancelled"] as const).map((k) => (
                      <div
                        class={`bar ${k}`}
                        style={{
                          width: `${o.jobs ? (100 * o[k]) / o.jobs : 0}%`,
                        }}
                      />
                    ))}
                  </div>
                  <RecordView
                    data={{
                      done: o.done,
                      failed: o.failed,
                      cancelled: o.cancelled,
                    }}
                  />
                </Panel>
              );
            })}
          </div>
          <Panel title="Pull request rates & time">
            <RecordView
              data={{
                pull_requests: s.pull_requests.count,
                issue_jobs: s.pull_requests.issue_jobs,
                ended_with_pull_request:
                  s.pull_requests.pr_rate == null
                    ? null
                    : `${(s.pull_requests.pr_rate * 100).toFixed(0)}%`,
                first_pass_of_pull_requests:
                  s.pull_requests.first_pass_rate == null
                    ? null
                    : `${(s.pull_requests.first_pass_rate * 100).toFixed(0)}%`,
                median_seconds: s.pull_requests.median_secs,
                p90_seconds: s.pull_requests.p90_secs,
              }}
            />
            <p class="mono-muted">
              Durations in seconds. Unknown means no measurement.
            </p>
          </Panel>
          <Panel title="Job totals & reported CLI usage">
            <p class="mono-muted">
              Historical CLI costs are reported totals with unknown coverage.
              Attempt cost coverage is shown per model below.
            </p>
            <RecordView data={s.summary} />
          </Panel>
        </>
      )}
      {s.metrics_error && <p role="alert">{s.metrics_error}</p>}
      {s.metrics && (
        <>
          <p class="metrics-note">{s.metrics.note}</p>
          <RecordView
            data={{
              damaged_records: s.metrics.damaged_records,
              historical_jobs_without_attempts:
                s.metrics.historical_jobs_without_attempts,
            }}
          />
          <Panel title="Per model, CLI, effort & role">
            <RecordView data={s.metrics.groups} />
          </Panel>
          <Panel title="Task cohorts · quality, cost coverage & queue wait">
            <RecordView data={s.metrics.tasks} />
          </Panel>
        </>
      )}
      <Panel title="Router · current daemon session">
        <RecordView data={s.router} />
      </Panel>
    </>
  );
}
