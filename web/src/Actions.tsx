import { useRef, useState } from "preact/hooks";
import type { TargetedKeyboardEvent } from "preact";
import { useLocation } from "preact-iso";
import { action, actionError, actionSource, actionBusy, writeReason, details } from "./state";
import { board } from "./live";
export { Keys } from "./keys";

interface Draft { text: string; history: string[]; cursor?: number; saved: string }
function useDraft(workspace: string) {
  const drafts = useRef<Record<string, Draft>>({});
  const [, redraw] = useState(0);
  const key = `forkdeck.composer.${workspace}`;
  if (!drafts.current[key]) {
    let text = "", history: string[] = [];
    try {
      const stored = JSON.parse(localStorage.getItem(key) || "null");
      if (typeof stored?.text === "string") text = stored.text;
      if (Array.isArray(stored?.history)) history = stored.history.filter((v: unknown) => typeof v === "string").slice(-100);
    } catch { /* Saved input may be unavailable or contain an older draft. */ }
    drafts.current[key] = { text, history, saved: "" };
  }
  const draft = drafts.current[key];
  const save = () => {
    try { localStorage.setItem(key, JSON.stringify({ text: draft.text, history: draft.history })); } catch { /* The composer works without storage. */ }
    redraw(n => n + 1);
  };
  const setText = (text: string) => { draft.text = text; save(); };
  const clear = () => { draft.text = ""; draft.cursor = undefined; draft.saved = ""; save(); };
  const submitted = (message: string) => {
    if (message.trim()) draft.history = [...draft.history, message.trim()].slice(-100);
    if (draft.text === message) draft.text = "";
    draft.cursor = undefined;
    draft.saved = "";
    save();
  };
  const keyDown = (e: TargetedKeyboardEvent<HTMLTextAreaElement>) => {
    if (e.isComposing || e.altKey || e.metaKey) return;
    if (e.key === "Escape" || (e.ctrlKey && e.key.toLowerCase() === "u")) {
      e.preventDefault(); e.stopPropagation(); clear();
    } else if (!e.ctrlKey && !e.shiftKey && (e.key === "ArrowUp" || e.key === "ArrowDown")) {
      const input = e.currentTarget;
      if (input.selectionStart !== input.selectionEnd) return;
      const older = e.key === "ArrowUp";
      if (older ? input.value.slice(0, input.selectionStart).includes("\n") : input.value.slice(input.selectionEnd).includes("\n")) return;
      if (older && draft.history.length) {
        e.preventDefault();
        if (draft.cursor === undefined) { draft.saved = draft.text; draft.cursor = draft.history.length; }
        draft.cursor = Math.max(0, draft.cursor - 1);
        setText(draft.history[draft.cursor]);
      } else if (!older && draft.cursor !== undefined) {
        e.preventDefault();
        draft.cursor++;
        if (draft.cursor < draft.history.length) setText(draft.history[draft.cursor]);
        else { draft.cursor = undefined; setText(draft.saved); draft.saved = ""; }
      }
    }
  };
  return { text: draft.text, setText, submitted, keyDown };
}

/** Errors show beside the control that caused them; the page-level copy also says why writes are off. */
export function ActionStatus({ source, page = false }: { source: string; page?: boolean }) {
  return <>{page && writeReason.value && <p class="action-notice">{writeReason.value}</p>}{actionError.value && actionSource.value === source && <p class="action-notice" role="alert">{actionError.value}</p>}</>;
}
export function Intake() {
  const on = board.value?.automation;
  return <button type="button" class="intake" disabled={Boolean(writeReason.value) || actionBusy.value} title={writeReason.value || "Toggle issue intake"} onClick={() => void action({ action: "automation", on: !on }, "page")}><span class={`live-dot${on ? "" : " paused"}`} />Issue intake {on ? "on" : "paused"}</button>;
}
export function Composer({ issuePlan = false }: { issuePlan?: boolean }) {
  const { route } = useLocation();
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
  const { text, setText, submitted, keyDown } = useDraft(selected);
  const reason = writeReason.value;
  const submit = async () => {
    const result = await action(issuePlan
      ? { action: "new_plan", text, workspace: selected, agent: agent || null, issue }
      : { action: "submit", text, workspace: selected, agent: agent || null, plan_first: plan }, "page");
    if (result) { submitted(text); setPlan(false); if (result.id) route(issuePlan ? `/plans/${result.id}` : `/jobs/${result.id}`); }
  };
  return <div><form class="composer writable-composer" onKeyDown={e => {
    if (!issuePlan && !e.isComposing && ((e.ctrlKey && !e.altKey && e.key.toLowerCase() === "p") || (e.shiftKey && e.key === "Tab"))) { e.preventDefault(); setPlan(!plan); }
  }} onSubmit={e => { e.preventDefault(); void submit(); }}>
    <textarea rows={1} aria-label={issuePlan ? "Planning request" : "New task"} placeholder={issuePlan ? "Planning request…" : "New task…"} class="task-input" value={text} onInput={e => setText(e.currentTarget.value)} onKeyDown={e => {
      keyDown(e);
      if (!e.isComposing && e.key === "Escape") setPlan(false);
      if (!e.defaultPrevented && !e.isComposing && e.key === "Enter" && !e.shiftKey && !e.altKey && !e.ctrlKey && !e.metaKey) { e.preventDefault(); if (!reason && !actionBusy.value && text.trim() && selected) void submit(); }
    }} disabled={Boolean(reason)} title={reason} required />
    <label class="agent-label">Workspace <select class="agent-select" value={selected} onChange={e => setWorkspace(e.currentTarget.value)} disabled={Boolean(reason)} title={reason}>{choices.map(w => <option value={w.id}>{w.name}</option>)}</select></label>
    <label class="agent-label">Agent <select class="agent-select" value={agent} onChange={e => setAgent(e.currentTarget.value)} disabled={Boolean(reason)} title={reason}><option value="">Auto</option>{Object.entries(snap ? ready : {}).map(([name, ready]) => <option value={name} disabled={!ready}>{name}{ready ? "" : " (unavailable)"}</option>)}</select></label>
    {issuePlan ? <input aria-label="Existing issue" placeholder="Issue URL / number (optional)" value={issue} onInput={e => setIssue(e.currentTarget.value)} disabled={Boolean(reason)} /> : <label class="plan-toggle"><input type="checkbox" class="plan-checkbox" checked={plan} onChange={e => setPlan(e.currentTarget.checked)} disabled={Boolean(reason)} title={reason} />Plan first</label>}
    <button class="start-button" type="submit" disabled={Boolean(reason) || actionBusy.value || !text.trim() || !selected} title={reason}>Start</button>
  </form><ActionStatus source="page" page={!issuePlan} /></div>;
}
export function JobActions({ id }: { id: number }) {
  const { route } = useLocation();
  const detail = details.value[id];
  const [text, setText] = useState("");
  const [link, setLink] = useState(false);
  const [workspace, setWorkspace] = useState("");
  const [issue, setIssue] = useState("");
  const [request, setRequest] = useState("");
  const reason = writeReason.value;
  const disabled = Boolean(reason) || actionBusy.value;
  const send = async (kind: string, extra = {}) => {
    if (await action({ action: kind, id, ...extra }, `job-${id}`)) setText("");
  };
  if (!detail) return null;
  const title = (detail as typeof detail & { prompt_title?: string }).prompt_title || "Type a follow-up…";
  const snap = board.value?.snapshot;
  const repos = snap ? Array.from({ length: snap.workspacesLength() }, (_, i) => snap.workspaces(i)!).filter(w => w.id()?.startsWith("repo-")) : [];
  const repo = repos.find(w => w.id() === workspace)?.id() ?? repos[0]?.id() ?? "";
  return <div class="job-actions"><ActionStatus source={`job-${id}`} />
    {!!detail.choices.length && <section class="choices" aria-label="Choices"><h2>Choose an approach</h2>{detail.choices.map((choice, option) => <button type="button" disabled={disabled} title={reason} onClick={() => void send("choose", { option, text, expected_revision: detail.revision })}><strong>{choice.label}</strong><span>{choice.detail}</span></button>)}</section>}
    {!!detail.queued.length && <div class="queued-messages"><strong>Queued</strong>{detail.queued.map(message => <p>{message}</p>)}</div>}
    {/* A reply box that cannot take text reads as broken; say why instead. */}
    {!detail.actions.reply && <p class="action-notice">{detail.reply_reason || "This task does not take replies"}.</p>}
    <form class="reply-form" onSubmit={e => { e.preventDefault(); void send("reply", { text, expected_revision: detail.revision }); }}>
      {detail.actions.reply && <>
      <input aria-label={title} class="reply-input" placeholder={title} value={text} onInput={e => setText(e.currentTarget.value)} onKeyDown={e => { if (!e.isComposing && e.ctrlKey && !e.altKey && (e.key === "Enter" || e.key.toLowerCase() === "s")) { e.preventDefault(); if (text.trim() && !disabled && detail.actions.reply) void send("steer", { text, expected_revision: detail.revision }); } }} disabled={disabled || !detail.actions.reply} title={reason || (!detail.actions.reply ? "Only typed tasks take replies" : "")} />
      <button type="submit" class="secondary-button" disabled={disabled || !detail.actions.reply} title={reason}>Queue</button>
      <button type="button" class="primary-button" disabled={disabled || !text.trim() || !detail.actions.reply} title={reason} onClick={() => void send("steer", { text, expected_revision: detail.revision })}>Send now</button>
      </>}
      {detail.actions.stop && <button type="button" class="stop-button" disabled={disabled} title={reason} onClick={() => void send("cancel")}>Stop</button>}
      {detail.actions.resume && <button type="button" class="secondary-button" disabled={disabled} title={reason} onClick={() => void send("resume")}>Resume</button>}
      {detail.actions.retry && <button type="button" class="secondary-button" disabled={disabled} title={reason} onClick={() => void send("retry")}>Retry</button>}
      {!!detail.choices.length && detail.actions.reply && <button type="button" class="secondary-button" disabled={disabled || !text.trim()} title={reason} onClick={() => void send("choose", { text, expected_revision: detail.revision })}>Use my response</button>}
    </form>
    {detail.actions.reply && !detail.actions.stop && <>
      <button type="button" class="secondary-button" onClick={() => { setLink(!link); if (!link) setRequest(text.trim() || detail.prompt); }}>Plan as issue…</button>
      {link && <form class="choices" onSubmit={e => {
        e.preventDefault();
        void action({ action: "link_plan", id, workspace: repo, issue: issue.trim() || null, text: request, expected_revision: detail.revision }, `job-${id}`).then(result => { if (result?.id) route(`/plans/${result.id}`); });
      }}>
        <label>Repository <select aria-label="Repository" value={repo} onChange={e => setWorkspace(e.currentTarget.value)} disabled={disabled}>{repos.map(w => <option value={w.id() ?? ""}>{w.name()}</option>)}</select></label>
        {!repos.length && <p>Add a [[repos]] entry for GitHub or GitLab to configure issue planning.</p>}
        <label>Existing issue <input aria-label="Existing issue" value={issue} onInput={e => setIssue(e.currentTarget.value)} disabled={disabled} placeholder="Issue URL / number (optional)" /></label>
        <label>Planning request <textarea aria-label="Planning request" value={request} onInput={e => setRequest(e.currentTarget.value)} disabled={disabled} required /></label>
        <p>Publication and Ready for build each require your selection.</p>
        <button type="submit" class="primary-button" disabled={disabled || !repo || !request.trim()}>Start planning</button>
      </form>}
    </>}
  </div>;
}
