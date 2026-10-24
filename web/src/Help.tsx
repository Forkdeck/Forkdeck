import { useEffect, useRef } from "preact/hooks";

const bindings = [
  ["Enter", "Start a task or queue a reply; an empty reply resumes or carries on."],
  ["Ctrl-S / Ctrl-Enter", "Send the reply now."],
  ["↑ / ↓", "Recall history at the first / last line of New task; restore the unfinished draft after the newest entry."],
  ["Ctrl-P / Shift-Tab", "Toggle Plan first while the task composer has focus."],
  ["Esc / Ctrl-U", "Clear the task draft; Esc also resets Plan first. Outside the composer, Esc closes detail or help."],
  ["Alt-↑ / Alt-↓", "Previous / next job, stopping at either end."],
  ["Alt-← / Alt-→", "Newest job in the previous / next occupied lane. Handled on the board; elsewhere the browser keeps history navigation."],
  ["F2", "Open / close job detail."],
  ["F4", "Repositories (the TUI team view)."],
  ["F5", "Focus the selected job's choices. With no choices the browser keeps Reload; click an option or Use my response to choose."],
  ["F6", "Issue plans; Plan as issue… on a finished typed task attaches its conversation."],
  ["Ctrl-A", "Toggle issue intake outside text fields. Inside text fields the browser keeps Select all."],
  ["Ctrl-X", "Stop the job whose panel is open, when no text is selected; otherwise the browser keeps Cut."],
  ["Ctrl-T / Ctrl-N", "Browser keeps New tab / New window. Use n outside text fields, or click New task."],
  ["Ctrl-W", "Browser keeps Close tab. Use Ctrl-U to clear the task draft, or Backspace to edit a word."],
  ["Ctrl-R", "Browser keeps Reload. Use F2 and click the reply box to reply to a typed task."],
  ["Ctrl-C / Ctrl-Q", "Browser keeps Copy / Quit browser. Close the tab to leave the web UI; daemon shutdown stays with its owner."],
  ["Tab", "Browser keeps focus navigation. Use the Agent selector to pick Auto, claude, codex or copilot."],
  ["PgUp / PgDn / End", "Scroll the open job output / jump to latest. Shift-PgUp / Shift-PgDn: use the Session log strip to read session output."],
  ["Choices: ↑ / ↓ / Tab / 1–9 / Enter / c / Esc / F5", "Tab to an option, Enter to choose; type in the reply box and use Use my response for a custom response. F2 closes detail."],
  ["Plans: Tab / ← / → / 1–3 / Enter", "Use the Plan / Review tabs and Accept / retry, Refine and Ready for build controls; Tab and Enter navigate and activate controls."],
  ["Plans: ↑ / ↓ / PgUp / PgDn / Home / End / Esc", "Scroll the plan with the browser; use Board navigation to leave."],
  ["Diff: ↑ / ↓ / PgUp / PgDn / Space / Home / End / q / Esc", "Open the source or PR link to read its diff; use that page's scrolling and Back to return."],
  ["Typing / Backspace", "Type and edit in the task or reply field using the browser's text editing keys."],
  ["?", "Open / close this help outside text fields."],
];

export function Help({ close }: { close: () => void }) {
  const dialog = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    const previous = document.activeElement;
    dialog.current?.showModal();
    return () => { dialog.current?.close(); if (previous instanceof HTMLElement && previous.isConnected) previous.focus(); };
  }, []);
  return <dialog ref={dialog} aria-label="Keyboard shortcuts" onCancel={e => { e.preventDefault(); close(); }} style={{ background: "var(--background)", color: "var(--text)", border: "1px solid var(--border)", maxWidth: "min(760px, 90vw)", maxHeight: "80vh", overflow: "auto" }}>
    <h1>Keyboard shortcuts</h1>
    <button type="button" class="secondary-button" onClick={close}>Close help</button>
    <p>The browser keeps its tab, window and editing keys. Their Forkdeck substitutes are listed here.</p>
    <dl>{bindings.map(([key, meaning]) => <div key={key}><dt><strong>{key}</strong></dt><dd>{meaning}</dd></div>)}</dl>
  </dialog>;
}
export function KeyHints({ open }: { open: () => void }) {
  return <footer aria-label="Keyboard shortcuts" class="mono-muted" style={{ position: "fixed", bottom: 0, left: 0, right: 0, zIndex: 25, background: "var(--background)", padding: "2px 8px", whiteSpace: "nowrap", overflow: "auto", fontSize: "11px" }}>
    ⏎ send · ↑↓ history · Alt↑↓ job · Alt←→ lane · F2 view · F4 repos · F5 choices · F6 issue plan · n new · ^P plan · ^S now · ^X stop · <button type="button" onClick={open}>? Help</button>
  </footer>;
}
