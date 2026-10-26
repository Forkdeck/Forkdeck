[Documentation](README.md) · [Front page](../README.md)

# Keys

| | |
|---|---|
| `Enter` | send what you typed to the task the input box names: a follow-up once it has finished, a queued message while it works, your choice while it waits for one. With nothing typed, an interrupted task resumes and a finished one carries on. After `Ctrl-T`, start a new task in the current directory |
| `Ctrl-T` / `Ctrl-N` | make the next `Enter` start a new task, keeping your draft |
| `Ctrl-P` / `Shift-Tab` | the next new task proposes approaches first, and you pick one |
| `Ctrl-R` | pin replies to the selected typed task, whatever is selected later |
| `Ctrl-S` | send what you typed to the task now: its agent stops and continues with the message |
| `Tab` | force the next task to claude, codex, copilot, or back to auto |
| `↑` `↓` | recall submitted messages; edit and press Enter to resend; Down restores your draft |
| `Alt-↑` `Alt-↓` | select a job; `Esc` goes back to following the newest |
| `Alt-←` `Alt-→` | select the newest job in the previous or next board lane that has one |
| `PgUp` `PgDn` | scroll the selected job's worker and reviewer output back and forth; it stops following new output until you page back to the end |
| `Shift-PgUp` `Shift-PgDn` | scroll the session log |
| `End` | follow the newest output and log lines again |
| click an edit line | full-screen diff of those files against the job's starting commit, refreshed while the agent works; `↑` `↓` `PgUp` `PgDn` or the wheel scroll, `Esc` closes. Hold `Shift` to select text with the mouse |
| `F2` | switch between the factory board and agent tree |
| `F5` | open planning choices for the selected task |
| `F6` | create/link an issue plan; inspect, accept, refine or mark it ready for build |
| `Ctrl-A` | issue and pull request polling on/off |
| `F4` | show what teammates are working on, and back |
| `Ctrl-X` | cancel the selected job |
| `Ctrl-C` / `Ctrl-Q` | quit from any view, saving the draft and stopping agents; again to quit at once |

Ctrl keys work with Shift or Caps Lock held.

The TUI opens on a factory board: Queue, Build, Review, Delivery, Done, and Attention. Select a job with Alt-Up/Down to inspect its checks, review result, and live worker/reviewer output. F2 switches to the detailed agent tree. Message recall keeps the last 100 accepted submissions across TUI restarts; Enter replies to the current task; Ctrl-T starts a new task and F4 opens the team view.

When a worker or reviewer reports exhausted usage before using tools, Forkdeck retries once with the other available CLI. The limited CLI is skipped for 15 minutes in that process and shown as LIMITED. A run that has used tools is not replayed automatically. If only one CLI remains available, its own review is explicitly logged.

A typed task keeps its conversation. The input box's title says where `Enter`
sends: once a task is started, to that task; `Alt-↑` `Alt-↓` selects another,
`Ctrl-R` pins replies to the selected one, and `Ctrl-T` starts a new task. The
same agent continues in the same tree with the earlier exchanges, and its change
goes through the checks and review again. Messages sent while the task runs are
queued on it and taken when its current step ends; `Ctrl-S` stops the agent to
take one at once. The input box shows how many are queued, and the board the
next one. A message the task cannot take (another forkdeck holds it, say) stays
on the input line.

Typed tasks, their conversations, queued messages and recent output are kept in
the history database (`~/.local/state/forkdeck/forkdeck.db`), with the last 100
messages you sent and an unfinished draft for each working directory. Tasks cut
off by quitting continue by themselves when forkdeck starts again; if one could
not, select it and press `Enter` with nothing typed.

Choices open a picker when the task is selected and nothing is typed: the
options of a plan waiting for its choice (`Ctrl-P`), or numbered choices in a
finished answer (lists introduced as choices or options, and option tables).
Up/Down highlight one and `Enter` sends it: a waiting plan takes the option
itself, and for an answer it is sent as a reply. **Other / custom response**
(or `c`) leaves the input line for your own words. Esc closes the picker; F5
reopens it. An answer that offers choices and changed nothing finishes without
checks or review, so you can decide what happens next; if the worker changed
files, they are checked and reviewed as usual. Interactive permission prompts of
the CLIs themselves are not handled by this picker.

## Conversations, history and resuming

A typed task keeps its conversation. Select it (`↑` `↓`) and type: once it has
finished, that is a follow-up, and the same agent continues in the same tree;
while it works, it is a message the agent gets as soon as its current step ends,
before checks and review. `Ctrl-S` sends it at once instead: the agent is stopped
and carries on with the message. Either way the change goes through the checks
and review again, and the reviewer is told what you asked for since the task.

The agent continues its own conversation in its CLI (`claude --resume`,
`codex exec resume`, `copilot --resume`), so nothing it was told is lost and
nothing is sent twice. When a CLI no longer has the conversation, the agent
starts a new one with the exchanges so far.

Jobs are kept in `~/.local/state/forkdeck/forkdeck.db` (SQLite, the newest 200).
The directory is private (0700), and the database is owner-readable and writable only (0600). They come back when forkdeck starts: the jobs of the folder it starts in
(or of another worktree of that repository) and of the configured repositories.
Jobs of other folders stay saved and come back when forkdeck starts there. A typed
task of this folder that was cut off by quitting, Ctrl-C or SIGTERM is marked
interrupted and continues by itself from the stage it had reached; `Ctrl-R`
retries one by hand. Issue and pull request jobs come back for reference only:
their labels already put them back in the queue. Several forkdecks can share the
file. `forkdeck history` lists the saved jobs, `forkdeck history clear` removes
them, and `forkdeck resume` continues every interrupted typed task, whatever its
folder, without the interface.

See [planning](planning.md) for F5/F6 and planning before work starts.
