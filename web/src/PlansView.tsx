import { action, actionBusy, writeReason } from "./state";
import { ActionStatus, Composer, Intake } from "./Actions";
import { Account } from "./Account";
import { Markdown } from "./Markdown";
import { Navigation } from "./Operational";
import { Fragment } from "preact";
import { useEffect, useState } from "preact/hooks";
import { tabClass } from "./ui";
import type { PlanView } from "./state";
export function PlansView({
  plan,
  allPlans,
}: {
  plan: PlanView;
  allPlans: PlanView[];
}) {
  const [tab, setTab] = useState("Tasks");
  const [feedback, setFeedback] = useState("");
  const [newPlan, setNewPlan] = useState(false);
  useEffect(() => setFeedback(""), [plan.id]);
  const disabled = Boolean(writeReason.value) || actionBusy.value || !plan.revision;
  const apply = async (operation: string) => {
    if (await action({ action: "plan", id: plan.id, operation, text: feedback, expected_revision: plan.revision })) setFeedback("");
  };

  const tabs = ["Tasks", "Plan", "Review"];
  const tasks = plan.tasks.map(({ task, status, after }) => ({
    n: task.number ?? task.id,
    title: task.title,
    group: task.kind === "group",
    nested: Boolean(task.parent),
    after: after.length
      ? "after " +
        after
          .map(
            (id) =>
              "#" +
              (plan.plan.graph?.tasks.find((t) => t.id === id)?.number ?? id),
          )
          .join(", ")
      : "",
    status,
    tone:
      task.kind === "group"
        ? "group"
        : status.startsWith("merged in PR ")
          ? "merged"
          : status === "needs you"
            ? "you"
            : status.startsWith("working")
              ? "working"
              : status === "ready"
                ? "ready"
                : "waiting",
    done:
      status.startsWith("merged in PR ") ||
      status.endsWith(" merged") ||
      status === "done",
  }));
  const showTasks = tab === "Tasks",
    showDraft = tab === "Plan",
    showReview = tab === "Review";

  return (
    <div class="app-shell">
      <header class="app-header">
        <a href="/" class="brand-link">
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
          <span class="plan-brand">forkdeck</span>
        </a>
        <span class="muted-label">Plans</span>
        <div class="grow"></div>
        <Intake />
        <button type="button" class="new-plan" disabled={Boolean(writeReason.value)} title={writeReason.value} onClick={() => setNewPlan(!newPlan)}>New plan</button>
        <Account />
      </header>
      <div class="app-body">
        <nav aria-label="Plans" class="plans-sidebar">
          <Navigation />
          <label class="mobile-plan-picker">
            Issue plan
            <select value={String(plan.id)} onChange={(event) => location.assign(`/plans/${event.currentTarget.value}`)}>
              {allPlans.map((p) => <option value={String(p.id)}>{p.title}</option>)}
            </select>
          </label>
          <h2 class="plans-label">Issue plans</h2>
          {allPlans.map((p) => (
            <a
              href={`/plans/${p.id}`}
              class={`plan-link${p.id === plan.id ? " selected" : ""}`}
            >
              <span class="medium">{p.title}</span>
              <span class="plan-caption">
                {p.subtitle ??
                  `${p.plan.repo} · r${p.plan.revision} · ${p.status}`}
              </span>
            </a>
          ))}
        </nav>
        <main class="plans-main">
          {newPlan && <Composer issuePlan />}
          <ActionStatus />
          <div class="plan-heading">
            <div class="plan-heading-copy">
              <div class="mono-muted">{`${plan.plan.repo}${plan.plan.issue ? "#" + plan.plan.issue.key : ""} · revision ${plan.plan.revision} · ${plan.status}`}</div>
              <h1 class="plan-title">{plan.title}</h1>
              <div class="plan-subtitle">{plan.description}</div>
            </div>
            <div class="plan-actions">
              <button type="button" class="replan-button" disabled={disabled || Boolean(plan.actions?.refine) || !feedback.trim()} title={writeReason.value || plan.actions?.refine || "Enter feedback below to re-plan"} onClick={() => void apply("replan")}>Re-plan…</button>
              {plan.plan.issue?.url && <a
                href={plan.plan.issue.url}
                aria-disabled={!plan.plan.issue?.url}
                class="issue-link"
              >{`Open #${plan.plan.issue?.key ?? ""} on ${plan.plan.provider === "gitlab" ? "GitLab" : "GitHub"}`}</a>}
            </div>
          </div>
          <div class="plan-write-actions">
            <label>Feedback <textarea value={feedback} onInput={e => setFeedback(e.currentTarget.value)} disabled={Boolean(writeReason.value)} placeholder="Feedback to refine or re-plan…" /></label>
            <button type="button" class="secondary-button" disabled={disabled || Boolean(plan.actions?.publish)} title={writeReason.value || plan.actions?.publish || ""} onClick={() => void apply("accept")}>Accept and publish</button>
            <button type="button" class="secondary-button" disabled={disabled || Boolean(plan.actions?.refine) || !feedback.trim()} title={writeReason.value || plan.actions?.refine || ""} onClick={() => void apply("refine")}>Refine</button>
            <button type="button" class="secondary-button" disabled={disabled || Boolean(plan.actions?.ready)} title={writeReason.value || plan.actions?.ready || ""} onClick={() => void apply("ready")}>Ready for build</button>
          </div>
          <div class="revision-flags" aria-label="Revision status">
            {["reviewed", "accepted", "published"].map((flag) => (
              <span
                class={`task-pill ${plan.plan[flag as "reviewed"] === plan.plan.revision ? "tone-merged" : "tone-waiting"}`}
              >
                {flag}:{" "}
                {plan.plan[flag as "reviewed"] === plan.plan.revision
                  ? "yes"
                  : plan.plan[flag as "reviewed"] == null
                    ? "no"
                    : `r${plan.plan[flag as "reviewed"]}`}
              </span>
            ))}
          </div>
          <div role="tablist" class="plan-tabs">
            {tabs.map((label) => (
              <button
                key={label}
                type="button"
                role="tab"
                aria-selected={label === tab}
                onClick={() => setTab(label)}
                class={`plan-tab ${tabClass(label === tab)}`}
              >
                {label}
              </button>
            ))}
          </div>
          {showTasks && (
            <>
              <div class="task-layout">
                <section aria-label="Task list" class="task-list">
                  {!tasks.length && <p class="task-empty">No tasks observed for this plan.</p>}
                  {tasks.map((t) => (
                    <Fragment key={t.n}>
                      <div class={`task-row${t.nested ? " nested" : ""}`}>
                        <span
                          class={`task-box${t.done ? " done" : ""}`}
                          aria-hidden="true"
                        >
                          {t.done ? "✓" : ""}
                        </span>
                        <span class="task-number">{`#${t.n}`}</span>
                        <span class={`task-title${t.group ? " group" : ""}`}>
                          {t.title}
                        </span>
                        <span class="task-after">{t.after}</span>
                        <span class={`task-pill tone-${t.tone}`}>
                          {t.status}
                        </span>
                      </div>
                    </Fragment>
                  ))}
                </section>
                <section aria-label="Scheduler" class="scheduler">
                  <div class="scheduler-card">
                    <h2 class="small-heading">Scheduler</h2>
                    <div class="scheduler-copy">{plan.scheduler}</div>
                  </div>
                  {plan.decision && (
                    <div class="decision">
                      <h2 class="decision-heading">{`#${plan.decision?.number ?? ""} needs you: ${plan.decision?.title ?? ""}`}</h2>
                      <div class="decision-copy">
                        Answer this decision on the issue. Browser answers are not supported yet.
                      </div>
                      <label htmlFor="answer" class="decision-label">
                        Your decision
                      </label>
                      <input
                        id="answer"
                        placeholder="Your answer…"
                        class="decision-input"
                        disabled
                        aria-disabled="true"
                        title={writeReason.value || "Answer this decision on the issue; browser answers are not supported yet"}
                      />
                      <button
                        type="button"
                        class="decision-button"
                        disabled
                        aria-disabled="true"
                        title={writeReason.value || "Answer this decision on the issue; browser answers are not supported yet"}
                      >{`Answer and close #${plan.decision?.number ?? ""}`}</button>
                    </div>
                  )}
                </section>
              </div>
            </>
          )}
          {showDraft && (
            <article class="plan-prose">
              <Markdown text={plan.draft_display || "No draft yet."} />
            </article>
          )}
          {showReview && (
            <article class="plan-prose">
              {plan.plan.review.match(/^VERDICT:\s*(PASS|FAIL)\s*$/im)?.[1] && (
                <span
                  class={`task-pill ${/^VERDICT:\s*PASS\s*$/im.test(plan.plan.review) ? "tone-merged" : "tone-you"}`}
                >
                  {plan.plan.review
                    .match(/^VERDICT:\s*(PASS|FAIL)\s*$/im)?.[1]
                    ?.toUpperCase()}
                </span>
              )}
              {plan.plan.reviewed !== plan.plan.revision && (
                <p class="mono-muted">
                  This revision has no recorded review. Any text below is
                  retained from an earlier review.
                </p>
              )}
              <Markdown text={plan.plan.review || "No review yet."} />
            </article>
          )}
        </main>
      </div>
    </div>
  );
}
