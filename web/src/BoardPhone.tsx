// The board's narrow presentation follows Phone.dc.html.
import { Fragment } from 'preact';
import { useState } from 'preact/hooks';
import { useLocation } from 'preact-iso';
import { pill, type PresentedJob } from './ui';
export function BoardPhone({ data, live }: {data: PresentedJob[]; live: string}) {
  const { route } = useLocation();
  const [filter,setFilter] = useState('Active');
  const filters = ['Active','Attention','Done','All'].map(label => ({ label, pick: () => setFilter(label), style:'flex:0 0 auto;background:' + (label===filter ? 'var(--selected)' : 'transparent') + ';color:' + (label===filter ? 'var(--text)' : 'var(--muted)') + ';border:1px solid ' + (label===filter ? 'var(--border)' : 'transparent') + ';border-radius:18px;padding:0 14px;min-height:36px;font:inherit;font-size:13px;cursor:pointer' }));
  const jobs = data.filter(j => filter==='All' || j.group===filter).map(j => ({ ...j, pillStyle:pill(j.state) }));
return (<div style="width: 100%; height: 100dvh; box-sizing: border-box; background: var(--background); color: var(--text); font-family: 'IBM Plex Sans', system-ui, sans-serif; font-size: 14px; display: flex; flex-direction: column">
<header style="display: flex; align-items: center; gap: 10px; padding: 14px 16px; border-bottom: 1px solid var(--border); background: var(--panel)">
<svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="var(--accent)" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><circle cx="6" cy="5" r="2"></circle><circle cx="6" cy="19" r="2"></circle><circle cx="18" cy="12" r="2"></circle><path d="M6 7v10M6 12h4a4 4 0 0 0 4-4V7M14 12h2"></path></svg>
<span style="font-family: 'IBM Plex Mono', monospace; font-weight: 500">forkdeck</span>
<div style="flex: 1 1 auto"></div>
<span style="display: flex; align-items: center; gap: 6px; font-size: 12px; color: var(--muted)"><span style="width: 8px; height: 8px; border-radius: 4px; background: var(--live)"></span>{live}</span>
</header>
<div role="tablist" style="display: flex; gap: 6px; padding: 10px 12px; overflow-x: auto; border-bottom: 1px solid var(--tone-22262f)">
{filters.map((f) => <Fragment key={f.label}>
<button type="button" role="tab" aria-selected={f.label === filter} onClick={f.pick} style={f.style}>{f.label}</button>
</Fragment>)}
</div>
<div style="flex: 1 1 auto; overflow-y: auto; padding: 12px; display: flex; flex-direction: column; gap: 8px">
{jobs.map((job) => <Fragment key={job.id}>
<button type="button" onClick={() => route(`/jobs/${job.id}`)} style="all: unset; box-sizing: border-box; width: 100%; display: flex; flex-direction: column; gap: 6px; padding: 12px 14px; border-radius: 10px; background: var(--card); border: 1px solid var(--border); cursor: pointer; min-height: 44px">
<div style="display: flex; align-items: center; gap: 8px; font-family: 'IBM Plex Mono', monospace; font-size: 12px; color: var(--muted)"><span>{`#${job.id}`}</span><span style={job.pillStyle}>{job.state}</span><span style="margin-left: auto">{job.lane}</span></div>
<div style="font-size: 14px; line-height: 1.35; font-weight: 500">{job.title}</div>
<div style="font-size: 12px; color: var(--muted)">{job.meta}</div>
</button>
</Fragment>)}
</div>
<form style="display: flex; gap: 8px; padding: 12px; border-top: 1px solid var(--border); background: var(--panel)" onSubmit={(e) => e.preventDefault()}>
<label htmlFor="ptask" style="position: absolute; width: 1px; height: 1px; overflow: hidden">New task</label>
<input id="ptask" placeholder="New task…" style="flex: 1 1 auto; min-width: 0; background: var(--background); color: var(--text); border: 1px solid var(--border); border-radius: 8px; padding: 10px 12px; font: inherit; min-height: 44px; box-sizing: border-box" disabled aria-disabled="true" title="Coming in the next milestone"/>
<button type="button" style="background: var(--accent); color: var(--tone-141413); border: none; border-radius: 8px; padding: 0 16px; font: inherit; font-weight: 600; min-height: 44px; cursor: pointer" disabled aria-disabled="true" title="Coming in the next milestone">Start</button>
</form>
</div>);
}
