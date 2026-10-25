# Plans become sub-issues, worked in dependency order

Date: 2026-10-21. Status: design, awaiting approval.

## Why

Large pieces of work get split into many issues, and today that split and its ordering are done by
hand. On semantic-context, a separate Claude session turned `IMPLEMENTATION.md` into epic #1 →
eleven gate issues (G0–G10) → task sub-issues. The order lived only in prose ("Depends on: Persisted
home records.", "G0 → G1 → … → G10") and invented `[G3]` title tags and `G3` labels. The user was
meant to label the next task `agent:ready` by hand once its dependencies merged. In practice
everything got labelled at once: Forkdeck opened 15 PRs across all gates with none merged, and
dependent work was built on a base without its dependency (#22 needs #21, whose PR !71 is still
open).

Forkdeck is a software factory; breaking a plan into ordered work items and feeding them through in
the right order is its job, not the user's.

## Goals

1. **Planning ends in sub-issues.** A plan Forkdeck produces is a list of tasks, and each task
   becomes a real issue: a native sub-issue of the plan's parent issue, nested one level deeper
   (parent → group → task) when the plan has groups.
2. **Real numbering.** Order and dependencies use the forge's own numbers and links: native
   sub-issues for hierarchy, native "blocked by" links for order, and a `Depends on #21` line in the
   body for people. No invented tags.
3. **Dependency means merged.** A task starts only when every task it depends on has its PR merged
   into the base branch. No stacking work on unmerged branches; every PR is based on the base branch.
4. **Hands-off once approved.** Approving the plan creates the sub-issues and immediately starts every
   task that has nothing blocking it. When a PR merges, the tasks it unblocks start by themselves.
5. **Plans can change.** The user can re-plan; Forkdeck shows what would change, and on approval
   applies it, including reworking open PRs.
6. **Adopt existing work.** The same scheduling takes over issue trees Forkdeck did not create, starting
   with semantic-context epic #1 and its 15 open PRs.
7. **Ask, don't guess.** Design/judgement tasks and tasks too vague to build wait for the user.

## Non-goals

- Stacked PRs, or building on unmerged branches.
- Merging PRs. Merging stays the user's action; Forkdeck only reacts to it.
- Gate-wide barriers. Independent tasks run in parallel (within existing slot limits); only explicit
  dependencies order work.
- GitLab in the first release (see Constraints).

## Anti-goals (would make this a failure even if it works)

- Any task starting while a dependency is unmerged — by stale label, hand-applied label, restart, or race.
- Duplicate issues after a crash or retry.
- The scheduler reading free prose to decide order.
- Forkdeck silently changing or closing issues the user did not approve changing.

## Decisions made by the user

- Sources of a plan: a plan file in the repo, one big issue, or text typed into Forkdeck — all three.
- Use the forge's real numbering and native sub-issues.
- A dependency is satisfied only when its PR is merged.
- After creating the issues, start straight away; no separate "go".
- Changing a plan is an option the user invokes; PRs already open for changed tasks get adapted.
- Adopt semantic-context's existing epic #1, not only new plans.
- (Earlier) the plan is reviewed before approval; Codex reviewed this design's draft.

## The design

### Task graph

A plan carries a task graph, emitted by the planner as versioned JSON after the human-readable plan
(the `PLAN: READY` line stays the conversation signal). Each node has:

- a stable task ID (internal, never shown in titles; survives re-plans so identity is kept),
- title, body, acceptance checks,
- kind: **work** (delivered by one PR), **group** (done when all its children are done; never gets a
  PR), or **decision** (needs the user; done when the user approves it),
- parent (a group, or the plan's parent issue),
- dependencies (task IDs).

Forkdeck validates the graph before plan review: no cycles, no missing or self references, groups
have children, only work tasks may be built. An invalid graph goes back to the planner like a failed
plan review. The plan reviewer sees the graph too.

`choices.rs` is not used for this; it parses menus for humans.

### Creating the issues (one recoverable transaction)

On approval, Forkdeck persists the approved graph and then, step by step, recording progress after
each forge write:

1. Create every issue **without** the ready label, each body carrying a hidden marker with the plan
   identity and task ID (so a retry finds issues it already created instead of duplicating them —
   reuse the publication-token / uncertain-create reconciliation in `planning.rs` and `tracker.rs`).
2. Attach sub-issues to their parents.
3. Add the blocked-by links and the human `Depends on #N` lines, now that numbers exist.
4. Read back and verify.
5. Label the tasks that are ready, which starts them.

The graph and its progress are kept until every task is finished, independent of the 200-job history
limit.

The approved graph is also written to a managed section of the parent issue (like the plan section
`tracker.rs` already manages), and that copy is authoritative (*Claude's call*). Teammates' Forkdeck
instances, which already claim work through the forge, read it there, so any instance enforces the
same order without a locally saved plan. The local copy is a cache plus transaction progress.

The section is visible (user's choice): a readable task list Forkdeck keeps current, with the
machine-readable JSON folded underneath in a collapsed `<details>` "Plan data" block:

```
- [x] #20 Persisted home records
- [ ] #21 Home record API · after #20
- [ ] #22 Home sync worker · after #21 · PR #71 (waiting on #21)
- [ ] #23 Pick storage backend · needs you
```

The list is Forkdeck's output, not an input: order changes go through a re-plan. If the list or the
data has been edited by hand and no longer matches, Forkdeck stops scheduling that plan and tells the
user instead of guessing.

### Scheduling

The scheduler is the authority; labels are its output, not its input.

- A work task is **ready** when all its dependencies are done. Done means: work → its delivering PR is
  merged into the base branch; group → all children done; decision → user answered.
- *Claude's call:* a dependency declared on a group applies to everything inside it (G1 depends on G0
  means every G1 task waits for all of G0). Cycle validation includes these inherited edges.
- Forkdeck re-checks readiness both when polling and immediately before claiming a task. Unknown
  state (forge unreadable) counts as not ready. A ready label on a task the scheduler says is blocked
  is removed, with a log line saying why.
- Forkdeck records which PR delivers each task and asks the forge whether it merged (GitHub
  `state == MERGED` / `mergedAt`, checking the base branch). "Closed" and "merged" are distinct; the
  existing `still_open` collapse is not enough. This must work for all hosts, not only URLs containing
  "github".
- Failed task, or its PR closed without merging: everything depending on it stays blocked and the user
  is told. Retrying is an explicit user action.
- Base branch: the configured `RepoCfg.base`, as worktrees already use.

### Decision tasks and vague tasks

Decision tasks get `needs-human` and wait. *Claude's call:* the user answers by commenting on the
decision issue (or from Forkdeck, which posts the comment). Forkdeck closes the decision issue, and
the answer is included in the instructions of every task that depends on it. If the answer changes
the plan itself, the user re-plans; Forkdeck does not re-plan on its own.

A work task the worker finds too vague to build posts its
question on the issue and enters a durable "waiting for reply" state that survives restarts; a reply
on the issue (detected by polling comments) resumes it. `needs-human` alone is not enough, since it
gives neither reply detection nor a durable state.

### Changing a plan

The user re-plans the parent issue (F6) and says what changed. The planner proposes a new graph,
matched to the old one by task ID. Forkdeck shows the difference before approval:

- new tasks → issues to create,
- removed tasks → issues to close (with a comment saying why); *Claude's call:* an open PR for a
  removed task is closed with a comment too, and its branch is kept,
- changed tasks → issue bodies to update,
- changed tasks with an open PR → that PR is reworked on its existing branch,
- changed tasks already merged → a follow-up task is added instead. *Claude's call:* the follow-up
  takes over the changed task's place in the graph, so unfinished tasks that depended on it now wait
  for the follow-up to merge too; their open PRs are reworked once it does. Tasks that already merged
  get no automatic corrective tasks; the planner may propose them in the same re-plan,
- running tasks affected → stopped and restarted on the new wording,
- dependency changes → readiness recomputed.

Approval applies the difference through the same recoverable transaction.

### Adopting an existing issue tree

The user points Forkdeck at an existing parent issue (first: semantic-context #1). Forkdeck reads the
tree (native sub-issues), and a planner turn infers task kinds and dependencies from the existing
prose ("Depends on: Persisted home records." → #21). The user sees and approves the inferred graph
like any plan; on approval Forkdeck writes native blocked-by links and `Depends on #N` lines, removes
invented `[Gn]` title tags and `Gn` labels, and starts scheduling.

Open PRs whose dependencies are not merged are held: marked blocked, not worked further. When their
dependencies merge, Forkdeck reworks each one onto the updated base branch (rebase, then re-run checks
and review) rather than leaving it built on a base missing its dependency.

## Constraints

- **GitHub first.** Native sub-issues and blocked-by links exist. The installed `gh` is 2.45, older
  than the 2.94 convenience flags, so Forkdeck uses the REST API through `gh api`
  (`/issues/{n}/sub_issues`, `/issues/{n}/dependencies/blocked_by`), extending `tracker.rs`'s existing
  `gh api` boundary. Probe support and fail clearly when the server lacks it.
- **GitLab later.** Blocking links require Premium/Ultimate and native nesting is limited to
  epic → issue → task. `glab` is not installed here, so it cannot be tested. The scheduler must not
  depend on native links (its own graph is authoritative), so GitLab can later use plain issues plus
  `Depends on` lines and, where available, native links.
- Existing mechanisms keep their meaning: claims, worktrees branching from `RepoCfg.base`, the
  automation pause, slot limits, review and checks.
- Remote writes only after the corresponding state is persisted (as `planning.rs` already does).
- Forkdeck is hand-formatted Rust; never run `cargo fmt`.

## Left to the builder

- Exact JSON schema and where the graph is stored (history table vs file).
- Hidden marker format in issue bodies.
- Label names for "blocked" (reuse configurable labels in `RepoCfg` style).
- UI for showing the graph and the re-plan difference.
- How the three plan sources enter the existing issue-plan lifecycle (`planning.rs`); typed text needs
  a target repository, taken from the configured repo for the working directory.
- Polling intervals for merge state and issue comments.
