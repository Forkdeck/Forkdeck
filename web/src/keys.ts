import { h, Fragment } from "preact";
import { useEffect, useRef, useState } from "preact/hooks";
import { useLocation } from "preact-iso";
import { board } from "./live";
import { action, actionBusy, details, writeReason } from "./state";
import { Help, KeyHints } from "./Help";

function textField(target: EventTarget | null): target is HTMLElement {
  if (!(target instanceof HTMLElement)) return false;
  const field = target.closest('input, textarea, select, [contenteditable]:not([contenteditable="false"]), [role="textbox"]');
  if (field instanceof HTMLInputElement) return !["button", "checkbox", "color", "file", "hidden", "image", "radio", "range", "reset", "submit"].includes(field.type);
  return Boolean(field);
}
function textSelected(target: EventTarget | null): boolean {
  if (getSelection()?.toString()) return true;
  return (target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement)
    && target.selectionStart !== target.selectionEnd;
}

/** Installed once: routing and actions read the current live board, never a second projection. */
export function Keys() {
  const { path, route } = useLocation();
  const [help, setHelp] = useState(false);
  const [choicesFor, setChoicesFor] = useState<number>();
  const [wide, setWide] = useState(matchMedia("(min-width:700px)").matches);
  const last = useRef<number>();
  const onBoard = path === "/" || /^\/jobs\/\d+$/.test(path);
  useEffect(() => {
    if (!choicesFor || actionBusy.value || path !== `/jobs/${choicesFor}`) return;
    const choices = document.querySelector<HTMLElement>('.detail.open .choices');
    if (!choices) return;
    const button = choices.querySelector<HTMLElement>('button:not(:disabled)');
    if (button) button.focus();
    else { choices.tabIndex = -1; choices.focus(); }
    setChoicesFor(undefined);
  }, [path, choicesFor, actionBusy.value]);
  useEffect(() => {
    const query = matchMedia("(min-width:700px)");
    const change = () => setWide(query.matches);
    query.addEventListener("change", change);
    return () => query.removeEventListener("change", change);
  }, []);
  useEffect(() => {
    const key = (e: KeyboardEvent) => {
      if (e.defaultPrevented || e.isComposing || e.metaKey) return;
      // Routing changes the address before effects replace this listener.
      const path = location.pathname;
      const onBoard = path === "/" || /^\/jobs\/\d+$/.test(path);
      const typing = textField(e.target);
      if (help) {
        if (e.key === "Escape" || (!typing && e.key === "?")) { e.preventDefault(); setHelp(false); }
        return;
      }
      const current = board.value;
      const match = path.match(/^\/(?:jobs|plans)\/(\d+)$/);
      const picked = match ? Number(match[1]) : last.current ?? current?.jobs.find(j => j.state() === "working")?.id() ?? current?.jobs[0]?.id();
      const job = current?.jobs.find(j => j.id() === picked);
      const open = (id: number) => {
        last.current = id;
        const next = current?.jobs.find(j => j.id() === id);
        route(`/${next?.kind() === "plan" ? "plans" : "jobs"}/${id}`);
      };
      const focus = (selector: string) => {
        const control = document.querySelector<HTMLElement>(selector);
        if (!control) return false;
        control.focus(); return document.activeElement === control;
      };
      let handled = false;
      if (!typing && !e.ctrlKey && !e.altKey && e.key === "?") {
        setHelp(true); handled = true;
      } else if (!typing && !e.ctrlKey && !e.altKey && e.key === "n") {
        if (onBoard) handled = focus('.task-input[aria-label="New task"]:not(:disabled)');
        else { route("/"); requestAnimationFrame(() => focus('.task-input[aria-label="New task"]:not(:disabled)')); handled = true; }
      } else if (e.ctrlKey && !e.altKey && e.key.toLowerCase() === "a" && !typing && current && !writeReason.value && !actionBusy.value) {
        void action({ action: "automation", on: !current.automation }, "page"); handled = true;
      // Stop only the job whose panel is open: with no panel there is no visible selection to stop.
      } else if (e.ctrlKey && !e.altKey && e.key.toLowerCase() === "x" && job && path.startsWith("/jobs/") && !textSelected(e.target) && !writeReason.value && !actionBusy.value && details.value[job.id()]?.actions.stop) {
        void action({ action: "cancel", id: job.id() }, `job-${job.id()}`); handled = true;
      } else if (e.altKey && !e.ctrlKey && onBoard && current?.jobs.length && ["ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight"].includes(e.key)) {
        let next = picked;
        if (e.key === "ArrowUp" || e.key === "ArrowDown") {
          const index = Math.max(0, current.jobs.findIndex(j => j.id() === picked));
          next = current.jobs[Math.max(0, Math.min(current.jobs.length - 1, index + (e.key === "ArrowUp" ? -1 : 1)))].id();
        } else {
          const lanes = Array.from({ length: current.snapshot.lanesLength() }, (_, i) => current.snapshot.lanes(i)!.id());
          const from = lanes.indexOf(job?.lane() ?? "");
          const step = e.key === "ArrowRight" ? 1 : -1;
          for (let i = from + step; i >= 0 && i < lanes.length; i += step) {
            const newest = current.jobs.filter(j => j.lane() === lanes[i]).at(-1);
            if (newest) { next = newest.id(); break; }
          }
        }
        if (next !== undefined) { open(next); handled = true; }
      } else if (!e.ctrlKey && !e.altKey && e.key === "F2" && onBoard && job) {
        if (path.startsWith("/jobs/")) { last.current = job.id(); route("/"); } else open(job.id());
        handled = true;
      } else if (!e.ctrlKey && !e.altKey && e.key === "F4") {
        route("/repos"); handled = true;
      } else if (!e.ctrlKey && !e.altKey && e.key === "F6") {
        route("/plans"); handled = true;
      } else if (!e.ctrlKey && !e.altKey && e.key === "F5" && onBoard && job && details.value[job.id()]?.choices.length) {
        if (!path.startsWith("/jobs/")) open(job.id());
        setChoicesFor(job.id()); handled = true;
      }
      if (handled) e.preventDefault();
    };
    addEventListener("keydown", key);
    return () => removeEventListener("keydown", key);
  }, [path, help]);
  return h(Fragment, null, help && h(Help, { close: () => setHelp(false) }), onBoard && wide && h(KeyHints, { open: () => setHelp(true) }));
}
