// Layout and inline styles follow Main.dc.html.
import { Fragment } from 'preact';
import { useState } from 'preact/hooks';
import { useLocation } from 'preact-iso';
import { pill, stageChip, tabStyle, type PresentedJob } from './ui';

interface Props { data: PresentedJob[]; picked?: number; automation: boolean; agents: {claude: boolean; codex: boolean; copilot: boolean}; live: string }
export function BoardWide({ data, picked, automation, agents, live }: Props) {
  const { route } = useLocation();
  const [activeTab, setTab] = useState('Worker');
  const jobs = data.map(j => ({
    ...j, pillStyle: pill(j.state),
    cardStyle: 'all: unset; box-sizing: border-box; width: 100%; display: flex; flex-direction: column; gap: 6px; padding: 10px 12px; border-radius: 8px; cursor: pointer; background: ' + (j.id === picked ? 'var(--selected)' : 'var(--card)') + '; border: 1px solid ' + (j.id === picked ? 'var(--accent)' : 'var(--border)'),
    pick: () => route(`/jobs/${j.id}`),
  }));
  const lanes = ['Queue', 'Build', 'Review', 'Delivery', 'Done', 'Attention'].map(name => {
    const inLane = jobs.filter(j => j.lane === name);
    return { name, count: inLane.length, jobs: inLane };
  });
  const selected = jobs.find(j => j.id === picked) ?? jobs[0];
  const sel = { ...selected, stages: selected.stages.map(stageChip) };
  const tabs = ['Worker', 'Reviewer', 'Conversation'].map(label => ({ label, pick: () => setTab(label), style: tabStyle(label === activeTab) }));
  const who: Record<string, string> = { worker:'var(--text)', review:'var(--blue)', router:'var(--purple)', checks:'var(--green)', agents:'var(--amber)', sched:'var(--amber)', issues:'var(--muted)', ci:'var(--muted)', queue:'var(--muted)' };
  const lines = selected.log.filter(l => activeTab === 'Reviewer' ? l[1] === 'review' : activeTab === 'Conversation' ? ['queue','issues'].includes(l[1]) : l[1] !== 'review');
  if (!lines.length) lines.push(['', 'queue', 'Nothing here yet.']);
  const log = lines.map(l => ({ t:l[0], text:(l[1] === 'worker' ? '' : l[1] + '  ') + l[2], style:'color:' + (who[l[1]] ?? 'var(--text)') + ';white-space:pre-wrap;overflow-wrap:anywhere' }));
  const autoLabel = automation ? 'on' : 'paused';
  const autoDot = 'width:8px;height:8px;border-radius:4px;background:' + (automation ? 'var(--live)' : 'var(--tone-7d8696)');
return (<div style="min-height: 100vh; background: var(--background); color: var(--text); font-family: 'IBM Plex Sans', system-ui, sans-serif; font-size: 14px; display: flex; flex-direction: column">
<header style="display: flex; flex-wrap: wrap; align-items: center; gap: 16px; padding: 12px 24px; border-bottom: 1px solid var(--border); background: var(--panel)">
<div style="display: flex; align-items: center; gap: 10px">
<svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="var(--accent)" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><circle cx="6" cy="5" r="2"></circle><circle cx="6" cy="19" r="2"></circle><circle cx="18" cy="12" r="2"></circle><path d="M6 7v10M6 12h4a4 4 0 0 0 4-4V7M14 12h2"></path></svg>
<span style="font-family: 'IBM Plex Mono', monospace; font-weight: 500; font-size: 16px; letter-spacing: 0.02em">forkdeck</span>
</div>
<label style="display: flex; align-items: center; gap: 8px; color: var(--muted); font-size: 13px">Workspace
<select style="background: var(--selected); color: var(--text); border: 1px solid var(--border); border-radius: 6px; padding: 6px 10px; font: inherit; min-height: 32px" disabled aria-disabled="true" title="Coming in the next milestone">
<option>justin · local</option>
<option>Forkdeck team</option>
</select>
</label>
<div style="flex: 1 1 auto"></div>
<div style="display: flex; align-items: center; gap: 8px; font-size: 13px; color: var(--muted)">
<span style="width: 8px; height: 8px; border-radius: 4px; background: var(--live)"></span>{`${live} · ${data.filter(j => ["Build","Review"].includes(j.lane)).length} slots of 3 busy`}
</div>
<button type="button" aria-disabled="true" title={automation ? "Issue intake is on" : "Issue intake is paused"} style="display: flex; align-items: center; gap: 8px; background: var(--selected); color: var(--text); border: 1px solid var(--border); border-radius: 6px; padding: 8px 12px; font: inherit; font-size: 13px; cursor: pointer; min-height: 36px">
<span style={autoDot}></span>{`Issue intake ${autoLabel}`}
</button>
<button type="button" aria-label="Account" style="width: 36px; height: 36px; border-radius: 18px; background: var(--border); color: var(--text); border: none; font: inherit; font-weight: 600; cursor: pointer" disabled aria-disabled="true" title="Coming in the next milestone">J</button>
</header>
<div style="display: flex; flex-wrap: wrap; flex: 1 1 auto; min-height: 0">
<nav aria-label="Main" style="flex: 0 0 200px; padding: 16px 12px; border-right: 1px solid var(--border); display: flex; flex-direction: column; gap: 4px">
<a href="/" style="display: flex; align-items: center; gap: 10px; padding: 10px 12px; border-radius: 6px; background: var(--selected); color: var(--text); text-decoration: none; font-weight: 500">Board</a>
<a href="/plans" style="display: flex; align-items: center; gap: 10px; padding: 10px 12px; border-radius: 6px; color: var(--muted); text-decoration: none">Plans</a>
<a href="#" aria-disabled="true" title="Coming in the next milestone" style="display: flex; align-items: center; gap: 10px; padding: 10px 12px; border-radius: 6px; color: var(--muted); text-decoration: none">Repositories</a>
<a href="#" aria-disabled="true" title="Coming in the next milestone" style="display: flex; align-items: center; gap: 10px; padding: 10px 12px; border-radius: 6px; color: var(--muted); text-decoration: none">Deployments</a>
<a href="#" aria-disabled="true" title="Coming in the next milestone" style="display: flex; align-items: center; gap: 10px; padding: 10px 12px; border-radius: 6px; color: var(--muted); text-decoration: none">Stats</a>
<a href="#" aria-disabled="true" title="Coming in the next milestone" style="display: flex; align-items: center; gap: 10px; padding: 10px 12px; border-radius: 6px; color: var(--muted); text-decoration: none">Doctor & settings</a>
<div style="margin-top: 24px; padding: 12px; border: 1px solid var(--border); border-radius: 8px; font-size: 12px; color: var(--muted); line-height: 1.5">
<div style="color: var(--text); font-weight: 500; margin-bottom: 4px">Agents</div>
{`claude · ${agents.claude ? "ready" : "unavailable"}`}<br/>{`codex · ${agents.codex ? "ready" : "unavailable"}`}<br/>{`copilot · ${agents.copilot ? "ready" : "no seat"}`}
</div>
</nav>
<main style="flex: 999 1 560px; min-width: 0; display: flex; flex-direction: column; gap: 16px; padding: 20px 24px">
<form style="display: flex; flex-wrap: wrap; gap: 10px; align-items: center; background: var(--card); border: 1px solid var(--border); border-radius: 10px; padding: 12px" onSubmit={(e) => e.preventDefault()}>
<label htmlFor="task" style="position: absolute; width: 1px; height: 1px; overflow: hidden">New task</label>
<input id="task" placeholder="New task… (Enter to queue, Ctrl-Enter to send now)" style="flex: 1 1 320px; min-width: 0; background: var(--background); color: var(--text); border: 1px solid var(--border); border-radius: 6px; padding: 10px 12px; font: inherit; min-height: 44px; box-sizing: border-box" disabled aria-disabled="true" title="Coming in the next milestone"/>
<label style="display: flex; align-items: center; gap: 6px; color: var(--muted); font-size: 13px">Agent
<select style="background: var(--selected); color: var(--text); border: 1px solid var(--border); border-radius: 6px; padding: 8px; font: inherit; min-height: 44px" disabled aria-disabled="true" title="Coming in the next milestone">
<option>Auto (fork layer)</option><option>claude</option><option>codex</option>
</select>
</label>
<label style="display: flex; align-items: center; gap: 8px; color: var(--text); font-size: 13px; min-height: 44px; cursor: pointer">
<input type="checkbox" checked onClick={e => e.preventDefault()} tabIndex={-1} style="width: 18px; height: 18px; accent-color: var(--accent)" aria-disabled="true" title="Coming in the next milestone"/>Plan first
</label>
<button type="button" style="background: var(--accent); color: var(--tone-141413); border: none; border-radius: 6px; padding: 0 20px; font: inherit; font-weight: 600; min-height: 44px; cursor: pointer" disabled aria-disabled="true" title="Coming in the next milestone">Start</button>
</form>
<div style="overflow-x: auto; flex: 1 1 auto">
<div style="display: grid; grid-template-columns: repeat(6, minmax(190px, 1fr)); gap: 12px; min-width: 1180px">
{lanes.map((lane) => <Fragment key={lane.name}>
<section style="background: var(--tone-13161c); border: 1px solid var(--tone-22262f); border-radius: 10px; padding: 10px; display: flex; flex-direction: column; gap: 8px; min-height: 420px">
<div style="display: flex; align-items: center; justify-content: space-between; padding: 4px 4px 6px">
<h2 style="margin: 0; font-size: 12px; font-weight: 600; letter-spacing: 0.08em; text-transform: uppercase; color: var(--muted)">{lane.name}</h2>
<span style="font-family: 'IBM Plex Mono', monospace; font-size: 12px; color: var(--muted)">{lane.count}</span>
</div>
{lane.jobs.map((job) => <Fragment key={job.id}>
<button type="button" onClick={job.pick} style={job.cardStyle}>
<div style="display: flex; align-items: center; gap: 8px; font-family: 'IBM Plex Mono', monospace; font-size: 12px; color: var(--muted)">
<span>{`#${job.id}`}</span><span style={job.pillStyle}>{job.state}</span>
</div>
<div style="font-size: 13px; line-height: 1.35; color: var(--text); font-weight: 500">{job.title}</div>
<div style="font-size: 12px; color: var(--muted)">{job.meta}</div>
</button>
</Fragment>)}
</section>
</Fragment>)}
</div>
</div>
</main>
<aside aria-label="Job detail" style="flex: 1 1 420px; max-width: 520px; border-left: 1px solid var(--border); background: var(--panel); display: flex; flex-direction: column; min-width: 0">
<div style="padding: 20px 20px 12px; display: flex; flex-direction: column; gap: 8px; border-bottom: 1px solid var(--border)">
<div style="display: flex; align-items: center; gap: 10px; font-family: 'IBM Plex Mono', monospace; font-size: 13px; color: var(--muted)">
<span>{`#${sel.id}`}</span><span style={sel.pillStyle}>{sel.state}</span><span>{sel.elapsed}</span>
</div>
<h1 style="margin: 0; font-size: 18px; line-height: 1.3; font-weight: 600">{sel.title}</h1>
<div style="font-family: 'IBM Plex Mono', monospace; font-size: 12px; color: var(--muted)">{sel.route}</div>
<div style="display: flex; gap: 6px; flex-wrap: wrap; margin-top: 6px">
{sel.stages.map((st) => <Fragment key={st.label}>
<span style={st.style}>{st.label}</span>
</Fragment>)}
</div>
{sel.warn && <>
<div style="margin-top: 8px; padding: 10px 12px; border-radius: 8px; background: var(--tone-2a1e10); border: 1px solid var(--tone-6b4a1c); color: var(--tone-ffd9a8); font-size: 13px; line-height: 1.45">{sel.warn}</div>
</>}
</div>
<div role="tablist" style="display: flex; gap: 4px; padding: 10px 20px 0">
{tabs.map((tab) => <Fragment key={tab.label}>
<button type="button" role="tab" aria-selected={tab.style.includes("background: var(--selected)")} onClick={tab.pick} style={tab.style}>{tab.label}</button>
</Fragment>)}
</div>
<div style="flex: 1 1 auto; margin: 10px 20px; background: var(--tone-0b0d11); border: 1px solid var(--tone-22262f); border-radius: 8px; padding: 12px; overflow: auto; min-height: 260px; font-family: 'IBM Plex Mono', monospace; font-size: 12px; line-height: 1.6">
{log.map((line) => <Fragment key={line.t + line.text}>
<div style="display: flex; gap: 10px"><span style="color: var(--tone-7d8696); flex: 0 0 auto">{line.t}</span><span style={line.style}>{line.text}</span></div>
</Fragment>)}
</div>
<form style="display: flex; flex-wrap: wrap; gap: 8px; padding: 0 20px 20px" onSubmit={(e) => e.preventDefault()}>
<label htmlFor="reply" style="position: absolute; width: 1px; height: 1px; overflow: hidden">Reply to this task</label>
<input id="reply" placeholder="Reply… (queued until the agent finishes)" style="flex: 1 1 200px; min-width: 0; background: var(--background); color: var(--text); border: 1px solid var(--border); border-radius: 6px; padding: 10px 12px; font: inherit; min-height: 44px; box-sizing: border-box" disabled aria-disabled="true" title="Coming in the next milestone"/>
<button type="button" style="background: var(--selected); color: var(--text); border: 1px solid var(--border); border-radius: 6px; padding: 0 14px; font: inherit; min-height: 44px; cursor: pointer" disabled aria-disabled="true" title="Coming in the next milestone">Queue</button>
<button type="button" style="background: var(--accent); color: var(--tone-141413); border: none; border-radius: 6px; padding: 0 14px; font: inherit; font-weight: 600; min-height: 44px; cursor: pointer" disabled aria-disabled="true" title="Coming in the next milestone">Send now</button>
<button type="button" style="background: transparent; color: var(--red); border: 1px solid var(--tone-5a2a24); border-radius: 6px; padding: 0 14px; font: inherit; min-height: 44px; cursor: pointer" disabled aria-disabled="true" title="Coming in the next milestone">Stop</button>
</form>
</aside>
</div>
</div>);
}
