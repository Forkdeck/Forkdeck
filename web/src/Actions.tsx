import { useState } from "preact/hooks";
import { useLocation } from "preact-iso";
import { action, actionError, actionBusy, writeReason, details } from "./state";
import { board } from "./live";

export function ActionStatus() {
  return <>{writeReason.value && <p class="action-notice">{writeReason.value}</p>}{actionError.value && <p class="action-notice" role="alert">{actionError.value}</p>}</>;
}
export function Intake() {
  const on = board.value?.automation;
  return <button type="button" class="intake" disabled={Boolean(writeReason.value) || actionBusy.value} title={writeReason.value || "Toggle issue intake"} onClick={() => void action({ action: "automation", on: !on })}><span class={`live-dot${on ? "" : " paused"}`} />Issue intake {on ? "on" : "paused"}</button>;
}
export function Composer({ issuePlan = false }: { issuePlan?: boolean }) {
  const { route } = useLocation();
  const [text, setText] = useState("");
  const [workspace, setWorkspace] = useState("");
  const [agent, setAgent] = useState("");
  const [plan, setPlan] = useState(false);
  const [issue, setIssue] = useState("");
  const snap = board.value?.snapshot;
  const all = snap ? Array.from({ length: snap.workspacesLength() }, (_, i) => snap.workspaces(i)!) : [];
  const choices = all.map(w => ({ id: w.id() ?? "", name: w.name() ?? "" })).filter(w => !issuePlan || w.id.startsWith("repo-"));
  const agents = snap?.agents();
  const ready = { claude: agents?.claude() ?? false, codex: agents?.codex() ?? false, copilot: agents?.copilot() ?? false };
  const selected = choices.find(w => w.id === workspace)?.id ?? choices[0]?.id ?? "";
  const reason = writeReason.value;
  const submit = async () => {
    const result = await action(issuePlan
      ? { action: "new_plan", text, workspace: selected, agent: agent || null, issue }
      : { action: "submit", text, workspace: selected, agent: agent || null, plan_first: plan });
    if (result) { setText(""); if (result.id) route(issuePlan ? `/plans/${result.id}` : `/jobs/${result.id}`); }
  };
  return <div><form class="composer writable-composer" onSubmit={e => { e.preventDefault(); void submit(); }}>
    <input aria-label={issuePlan ? "Planning request" : "New task"} placeholder={issuePlan ? "Planning request…" : "New task…"} class="task-input" value={text} onInput={e => setText(e.currentTarget.value)} disabled={Boolean(reason)} title={reason} required />
    <label class="agent-label">Workspace <select class="agent-select" value={selected} onChange={e => setWorkspace(e.currentTarget.value)} disabled={Boolean(reason)} title={reason}>{choices.map(w => <option value={w.id}>{w.name}</option>)}</select></label>
    <label class="agent-label">Agent <select class="agent-select" value={agent} onChange={e => setAgent(e.currentTarget.value)} disabled={Boolean(reason)} title={reason}><option value="">Auto</option>{Object.entries(snap ? ready : {}).map(([name, ready]) => <option value={name} disabled={!ready}>{name}{ready ? "" : " (unavailable)"}</option>)}</select></label>
    {issuePlan ? <input aria-label="Existing issue" placeholder="Issue URL / number (optional)" value={issue} onInput={e => setIssue(e.currentTarget.value)} disabled={Boolean(reason)} /> : <label class="plan-toggle"><input type="checkbox" class="plan-checkbox" checked={plan} onChange={e => setPlan(e.currentTarget.checked)} disabled={Boolean(reason)} title={reason} />Plan first</label>}
    <button class="start-button" type="submit" disabled={Boolean(reason) || actionBusy.value || !text.trim() || !selected} title={reason}>Start</button>
  </form><ActionStatus /></div>;
}
export function JobActions({ id }: { id: number }) {
  const detail = details.value[id];
  const [text, setText] = useState("");
  const reason = writeReason.value;
  const disabled = Boolean(reason) || actionBusy.value;
  const send = async (kind: string, extra = {}) => {
    if (await action({ action: kind, id, ...extra })) setText("");
  };
  if (!detail) return null;
  return <div class="job-actions"><ActionStatus />
    {!!detail.choices.length && <section class="choices" aria-label="Choices"><h2>Choose an approach</h2>{detail.choices.map((choice, option) => <button type="button" disabled={disabled} title={reason} onClick={() => void send("choose", { option, expected_revision: detail.revision })}><strong>{choice.label}</strong><span>{choice.detail}</span></button>)}</section>}
    {!!detail.queued.length && <div class="queued-messages"><strong>Queued</strong>{detail.queued.map(message => <p>{message}</p>)}</div>}
    <form class="reply-form" onSubmit={e => { e.preventDefault(); void send("reply", { text, expected_revision: detail.revision }); }}>
      <input aria-label="Reply to this task" class="reply-input" placeholder="Reply… (queued until the agent finishes)" value={text} onInput={e => setText(e.currentTarget.value)} onKeyDown={e => { if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) { e.preventDefault(); if (text.trim() && !disabled && detail.actions.reply) void send("steer", { text, expected_revision: detail.revision }); } }} disabled={disabled || !detail.actions.reply} title={reason || (!detail.actions.reply ? "Only typed tasks take replies" : "")} />
      <button type="submit" class="secondary-button" disabled={disabled || !text.trim() || !detail.actions.reply} title={reason}>Queue</button>
      <button type="button" class="primary-button" disabled={disabled || !text.trim() || !detail.actions.reply} title={reason} onClick={() => void send("steer", { text, expected_revision: detail.revision })}>Send now</button>
      {detail.actions.stop && <button type="button" class="stop-button" disabled={disabled} title={reason} onClick={() => void send("cancel")}>Stop</button>}
      {detail.actions.resume && <button type="button" class="secondary-button" disabled={disabled} title={reason} onClick={() => void send("resume")}>Resume</button>}
      {detail.actions.retry && <button type="button" class="secondary-button" disabled={disabled} title={reason} onClick={() => void send("retry")}>Retry</button>}
      {!!detail.choices.length && <button type="button" class="secondary-button" disabled={disabled || !text.trim()} title={reason} onClick={() => void send("choose", { text, expected_revision: detail.revision })}>Use my response</button>}
    </form>
  </div>;
}
