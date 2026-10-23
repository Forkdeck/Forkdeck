// Approved mockup styles are kept verbatim to preserve its geometry.
import { Fragment } from "preact";
import { useState } from "preact/hooks";
import type { PlanView } from "./state";
export function PlansView({plan, allPlans}: {plan:PlanView; allPlans:PlanView[]}) {
const [tab,setTab] = useState("Tasks");

    
    
    const tabs = ['Tasks', 'Plan', 'Review'].map(function (n) {
      var on = n === tab;
      return { label: n, pick: function () { setTab(n); }, style: 'background: ' + (on ? 'var(--selected)' : 'transparent') + '; color: ' + (on ? 'var(--text)' : 'var(--muted)') + '; border: 1px solid ' + (on ? 'var(--border)' : 'transparent') + '; border-radius: 6px; padding: 0 16px; min-height: 36px; font: inherit; font-size: 13px; cursor: pointer' };
    });
    var TONE: Record<string, string[]> = {
      merged: ['var(--tone-1c2a1c)', 'var(--green)'], working: ['var(--tone-123331)', 'var(--tone-7fe3d8)'], ready: ['var(--tone-2b2440)', 'var(--purple)'],
      waiting: ['var(--border)', 'var(--tone-c9cfda)'], you: ['var(--tone-3a2a12)', 'var(--amber)'], group: ['var(--selected)', 'var(--muted)']
    };
    var DEMO_T = [
      { n: 26, title: 'Storage', depth: 0, group: true, after: '', status: '1 of 1 merged', tone: 'group', done: true },
      { n: 21, title: 'Persisted home records', depth: 1, after: '', status: 'merged in PR !71', tone: 'merged', done: true },
      { n: 27, title: 'API', depth: 0, group: true, after: 'after #26', status: '0 of 2', tone: 'group' },
      { n: 22, title: 'Home record API', depth: 1, after: '', status: 'working · job #72', tone: 'working' },
      { n: 24, title: 'Home sync worker', depth: 1, after: 'after #22', status: 'waiting for #22', tone: 'waiting' },
      { n: 25, title: 'Pick storage backend', depth: 0, after: '', status: 'needs you', tone: 'you' },
      { n: 28, title: 'Docs for the home API', depth: 0, after: 'after #22', status: 'waiting for #22', tone: 'waiting' }
    ];
    var T: {n:number|string; title:string; depth:number; group?:boolean; after:string; status:string; tone:string; done?:boolean}[] = plan.demo ? DEMO_T : plan.tasks.map(({task, status, after}) => ({n:task.number ?? task.id, title:task.title, depth:task.parent ? 1:0, group:task.kind === "group", after: after.length ? "after " + after.map(id => "#"+(plan.plan.graph?.tasks.find(t => t.id === id)?.number ?? id)).join(", ") : "", status, tone:status.includes("merged") ? "merged" : status === "needs you" ? "you" : status === "working" ? "working" : "waiting", done:status.includes("merged") || status === "done"}));
    const tasks = T.map(function (t) {
      var c = TONE[t.tone];
      return {
        n: t.n, title: t.title, after: t.after, status: t.status,
        box: t.done ? '✓' : '',
        boxStyle: 'flex: 0 0 18px; height: 18px; border-radius: 4px; border: 1px solid ' + (t.done ? 'var(--green)' : 'var(--tone-4a5160)') + '; color: var(--green); font-size: 12px; display: flex; align-items: center; justify-content: center',
        rowStyle: 'display: flex; flex-wrap: wrap; align-items: center; gap: 10px; padding: 10px 12px; padding-left: ' + (12 + t.depth * 28) + 'px; border-bottom: 1px solid var(--selected)',
        titleStyle: 'flex: 1 1 200px; min-width: 0; font-weight: ' + (t.group ? '600' : '400') + '; color: var(--text)',
        pillStyle: 'padding: 3px 10px; border-radius: 10px; font-size: 12px; background: ' + c[0] + '; color: ' + c[1]
      };
    });
const showTasks=tab==='Tasks', showDraft=tab==='Plan', showReview=tab==='Review';

return (<div style="min-height: 100vh; background: var(--background); color: var(--text); font-family: 'IBM Plex Sans', system-ui, sans-serif; font-size: 14px; display: flex; flex-direction: column">
<header style="display: flex; flex-wrap: wrap; align-items: center; gap: 16px; padding: 12px 24px; border-bottom: 1px solid var(--border); background: var(--panel)">
<a href="/" style="display: flex; align-items: center; gap: 10px; color: var(--text); text-decoration: none; min-height: 36px">
<svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="var(--accent)" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><circle cx="6" cy="5" r="2"></circle><circle cx="6" cy="19" r="2"></circle><circle cx="18" cy="12" r="2"></circle><path d="M6 7v10M6 12h4a4 4 0 0 0 4-4V7M14 12h2"></path></svg>
<span style="font-family: 'IBM Plex Mono', monospace; font-weight: 500; font-size: 16px">forkdeck</span>
</a>
<span style="color: var(--muted); font-size: 13px">Plans</span>
<div style="flex: 1 1 auto"></div>
<button type="button" style="background: var(--accent); color: var(--tone-141413); border: none; border-radius: 6px; padding: 0 16px; font: inherit; font-weight: 600; min-height: 40px; cursor: pointer" disabled aria-disabled="true" title="Coming in the next milestone">New plan</button>
</header>
<div style="display: flex; flex-wrap: wrap; flex: 1 1 auto; min-height: 0">
<nav aria-label="Plans" style="flex: 0 0 280px; padding: 16px 12px; border-right: 1px solid var(--border); display: flex; flex-direction: column; gap: 6px">
<h2 style="margin: 0 8px 6px; font-size: 12px; font-weight: 600; letter-spacing: 0.08em; text-transform: uppercase; color: var(--muted)">Issue plans</h2>
{allPlans.map(p => <a href={`/plans/${p.id}`} style={`display:flex;flex-direction:column;gap:4px;padding:10px 12px;border-radius:8px;${p.id===plan.id ? 'background:var(--selected);border:1px solid var(--accent)' : 'border:1px solid var(--border)'};color:var(--text);text-decoration:none`}><span style="font-weight:500">{p.title}</span><span style="font-size:12px;color:var(--muted)">{p.subtitle ?? `${p.plan.repo} · r${p.plan.revision} · ${p.status}`}</span></a>)}</nav>
<main style="flex: 999 1 560px; min-width: 0; padding: 24px 28px; display: flex; flex-direction: column; gap: 18px">
<div style="display: flex; flex-wrap: wrap; align-items: flex-start; gap: 16px">
<div style="flex: 1 1 400px; min-width: 0; display: flex; flex-direction: column; gap: 6px">
<div style="font-family: 'IBM Plex Mono', monospace; font-size: 12px; color: var(--muted)">{plan.demo ? "o/semantic-context#20 · revision 3 · plan review passed (codex)" : `${plan.plan.repo}#${plan.plan.issue?.key ?? ""} · revision ${plan.plan.revision} · ${plan.status}`}</div>
<h1 style="margin: 0; font-size: 24px; font-weight: 600">{plan.title}</h1>
<div style="font-size: 13px; color: var(--muted)">{plan.demo ? "Accepted and created as 5 sub-issues. Tasks start by themselves when what they depend on has merged." : plan.status}</div>
</div>
<div style="display: flex; gap: 8px; flex-wrap: wrap">
<button type="button" style="background: var(--selected); color: var(--text); border: 1px solid var(--border); border-radius: 6px; padding: 0 14px; font: inherit; min-height: 40px; cursor: pointer" disabled aria-disabled="true" title="Coming in the next milestone">Re-plan…</button>
<a href={plan.plan.issue?.url || undefined} aria-disabled={!plan.plan.issue?.url} style="display: flex; align-items: center; background: var(--selected); color: var(--text); border: 1px solid var(--border); border-radius: 6px; padding: 0 14px; min-height: 40px; text-decoration: none">{`Open #${plan.plan.issue?.key ?? 20} on ${plan.plan.provider === "gitlab" ? "GitLab" : "GitHub"}`}</a>
</div>
</div>
<div role="tablist" style="display: flex; gap: 4px; border-bottom: 1px solid var(--border); padding-bottom: 8px">
{tabs.map((tab) => <Fragment key={tab.label}>
<button type="button" role="tab" aria-selected={tab.style.includes("background: var(--selected)")} onClick={tab.pick} style={tab.style}>{tab.label}</button>
</Fragment>)}
</div>
{showTasks && <>
<div style="display: flex; flex-wrap: wrap; gap: 18px; align-items: flex-start">
<section aria-label="Task list" style="flex: 2 1 520px; min-width: 0; background: var(--tone-13161c); border: 1px solid var(--tone-22262f); border-radius: 10px; padding: 8px">
{tasks.map((t) => <Fragment key={t.n}>
<div style={t.rowStyle}>
<span style={t.boxStyle} aria-hidden="true">{t.box}</span>
<span style="font-family: 'IBM Plex Mono', monospace; font-size: 13px; color: var(--muted); flex: 0 0 auto">{`#${t.n}`}</span>
<span style={t.titleStyle}>{t.title}</span>
<span style="font-size: 12px; color: var(--muted); flex: 0 0 auto">{t.after}</span>
<span style={t.pillStyle}>{t.status}</span>
</div>
</Fragment>)}
</section>
<section aria-label="Scheduler" style="flex: 1 1 280px; min-width: 0; display: flex; flex-direction: column; gap: 12px">
<div style="background: var(--card); border: 1px solid var(--border); border-radius: 10px; padding: 14px 16px; display: flex; flex-direction: column; gap: 8px">
<h2 style="margin: 0; font-size: 13px; font-weight: 600">Scheduler</h2>
<div style="font-size: 13px; color: var(--muted); line-height: 1.5">{plan.demo ? "Last pass 40 s ago · list matches its data (sha256 verified) · 1 ready, 1 working, 2 waiting, 1 needs you" : plan.scheduler}</div>
</div>
{(plan.demo || plan.tasks.some(t => t.task.kind === "decision" && t.status === "needs you")) && <div style="background: var(--tone-2a1e10); border: 1px solid var(--tone-6b4a1c); border-radius: 10px; padding: 14px 16px; display: flex; flex-direction: column; gap: 10px">
<h2 style="margin: 0; font-size: 13px; font-weight: 600; color: var(--tone-ffd9a8)">{`#${plan.decision?.number ?? 25} needs you: ${plan.decision?.title ?? "Pick storage backend"}`}</h2>
<div style="font-size: 13px; color: var(--tone-ffd9a8); line-height: 1.5">Your answer goes into the instructions of every task that depends on it.</div>
<label htmlFor="answer" style="font-size: 12px; color: var(--tone-ffd9a8)">Your decision</label>
<input id="answer" placeholder="e.g. SQLite now, Postgres behind a trait" style="background: var(--background); color: var(--text); border: 1px solid var(--tone-6b4a1c); border-radius: 6px; padding: 10px 12px; font: inherit; min-height: 44px; box-sizing: border-box" disabled aria-disabled="true" title="Coming in the next milestone"/>
<button type="button" style="background: var(--accent); color: var(--tone-141413); border: none; border-radius: 6px; font: inherit; font-weight: 600; min-height: 44px; cursor: pointer" disabled aria-disabled="true" title="Coming in the next milestone">{`Answer and close #${plan.decision?.number ?? 25}`}</button>
</div>}
</section>
</div>
</>}
{showDraft && <>
{plan.demo ? (<article style="background: var(--tone-13161c); border: 1px solid var(--tone-22262f); border-radius: 10px; padding: 20px 24px; line-height: 1.6; max-width: 820px">
<h2 style="margin: 0 0 8px; font-size: 16px">Goal</h2>
<p style="margin: 0 0 16px; color: var(--tone-c9cfda)">Persist home records, expose them through the API, and keep peers in sync, delivered as reviewable pull requests that each merge on their own.</p>
<h2 style="margin: 0 0 8px; font-size: 16px">Steps</h2>
<ol style="margin: 0 0 16px; padding-left: 20px; color: var(--tone-c9cfda)">
<li>Storage: persisted home records with migrations.</li>
<li>API: endpoint over the records, then the sync worker.</li>
<li>Decision: which storage backend to commit to.</li>
</ol>
<div style="font-family: 'IBM Plex Mono', monospace; font-size: 12px; color: var(--muted)">forkdeck-tasks: 5 tasks · validated · no cycles</div>
</article>) : <article class="plan-prose">{plan.plan.draft || "No draft yet."}</article>}
</>}
{showReview && <>
{plan.demo ? (<article style="background: var(--tone-13161c); border: 1px solid var(--tone-22262f); border-radius: 10px; padding: 20px 24px; line-height: 1.6; max-width: 820px; font-size: 13px; color: var(--tone-c9cfda)">
<div style="font-family: 'IBM Plex Mono', monospace; font-size: 12px; color: var(--blue); margin-bottom: 8px">codex · plan review · revision 3</div>
<p style="margin: 0 0 10px">Every work task is one reviewable pull request, and each `after` lists what must merge first. The sync worker correctly waits on the endpoint.</p>
<p style="margin: 0; font-family: 'IBM Plex Mono', monospace; color: var(--green)">VERDICT: PASS</p>
</article>) : <article class="plan-prose">{plan.plan.review || "No review yet."}</article>}
</>}
</main>
</div>
</div>);
}
