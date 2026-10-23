import type { BoardJob } from './state';
export interface PresentedJob extends BoardJob { stages:number[]; log:string[][]; route:string; warn:string; group:string }
const tones: Record<string, [string,string]> = {
  queued:['var(--border)','var(--tone-c9cfda)'], waiting:['var(--border)','var(--tone-c9cfda)'], planning:['var(--tone-2b2440)','var(--purple)'],
  working:['var(--tone-123331)','var(--tone-7fe3d8)'], reviewing:['var(--tone-1b2640)','var(--blue)'], 'PR open':['var(--tone-1b2640)','var(--blue)'],
  done:['var(--tone-1c2a1c)','var(--green)'], unverified:['var(--tone-3a2a12)','var(--amber)'], failed:['var(--tone-3a1a18)','var(--red)'],
};
export function pill(state:string) {
  const [background,color] = tones[state] ?? tones.queued;
  return `padding:2px 8px;border-radius:10px;font-size:11px;background:${background};color:${color}`;
}
const stageTones = [['var(--selected)','var(--muted)','○ '],['var(--tone-123331)','var(--tone-7fe3d8)','● '],['var(--tone-1c2a1c)','var(--green)','✓ '],['var(--tone-3a2a12)','var(--amber)','– '],['var(--tone-3a1a18)','var(--red)','✕ ']];
export function stageChip(state:number, index:number) {
  const [background,color,mark] = stageTones[state] ?? stageTones[0];
  return {label:mark + ['Route','Work','Checks','Review','Deliver'][index], style:`padding:4px 10px;border-radius:12px;font-size:12px;background:${background};color:${color}`};
}
export function tabStyle(selected:boolean) {
  return `background: ${selected ? 'var(--selected)' : 'transparent'};color:${selected ? 'var(--text)' : 'var(--muted)'};border:1px solid ${selected ? 'var(--border)' : 'transparent'};border-radius:6px;padding:0 14px;min-height:36px;font:inherit;font-size:13px;cursor:pointer`;
}
