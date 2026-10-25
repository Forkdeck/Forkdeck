[Documentation](README.md) · [Front page](../README.md)

# Issues and pull requests

## Issues

With `[[repos]]` configured and polling on (`Ctrl-A`, or
`automation.enabled = true`), forkdeck watches for open issues carrying a
label, `agent:ready` by default. For each one it:

1. swaps the label to `agent:working`,
2. creates a git worktree on `forkdeck/issue-N` from the base branch,
3. runs the job as above,
4. commits what the agent changed as a single commit with your git identity,
   pushes the branch, and opens a draft pull request that closes the issue,
   with the check and review results in the body,
5. comments on the issue and moves the label to `agent:pr-open`.

If the checks or the review fail, nothing is pushed. The attempt is committed
on the local branch, and the issue gets a comment saying why and the label
`agent:needs-human`. Labelling the issue again starts over from the base
branch; whatever the earlier attempt left, committed or not, is first moved to
a local branch named `forkdeck/attempts/issue-N-<commit>` so it is never lost. Set `automation.publish_on_failed_review = true` to open
the draft anyway.

Two more labels belong to plans that are split into sub-issues: `plan_label`
(default `forkdeck:plan`) marks the parent issue, and `human_label` (default
`needs-human`) marks a task that needs a person before it can go on. Like the
others they must be nonempty, comma-free and distinct from the issue labels.

It never force-pushes. If `forkdeck/issue-N` already exists on the remote from
an earlier attempt, no work is done: the issue gets a comment saying so and
`agent:needs-human`. Delete the branch and close its pull request before
labelling the issue again.

What gets committed is exactly what was checked and reviewed:

- When the agent finishes, its work is taken as one snapshot. Commits the agent
  made by itself are folded into that snapshot, so the reviewer, who is shown
  everything since the commit the work started from, cannot miss them.
- After the checks, the index and the working tree are put back to the
  snapshot. Files a check creates, rewrites or stages are gone again (ignored
  files such as build output are left alone), so a formatter run as a check has
  no effect. A check that commits or switches branch stops the job.
- The commit is refused if the index no longer matches the snapshot.

The labels have to exist in the repository first.

## Pull request reviews

Label any open pull or merge request `review:ready`. forkdeck checks out its
head in a detached worktree and has every agent in `review.pr_reviewers` review
it independently and in parallel. The findings go up as one comment with a
section per reviewer, naming the commit that was reviewed, and the label moves
to `review:passed` or `review:changes`.

A review takes minutes, and the author can push in the meantime. Before posting,
forkdeck looks up the pull request's head again. If it is no longer the commit
that was reviewed, the result is discarded, a "stale" comment says so, and the
new head is reviewed on the next poll. It looks once more after the label is
on and takes `review:passed` off again if the head moved in that moment. A
push after that cannot be caught, so **the label alone is not an authorisation**:
anything that acts on it should compare the head with the commit named in the
comment.

If a reviewer did not finish and nobody asked for changes, the comment says the
review is incomplete and the request label goes back. forkdeck retries by itself
`review.max_retries` times (3), waiting `review.retry_secs` (120) before the
first retry and three times longer before each later one. To retry by hand at
any point, remove the label and apply it again.

It never approves and never merges. The comment is advice for whoever does.

The repo's `checks` are **not** run on pull requests unless you set
`pr_checks = true` for that repo. Running them executes the pull request's code
on your machine, as you, and the tail of a failing check's output is posted in
the comment. Turn it on only where everyone who can get the label applied is
trusted.

The CLIs load instruction files such as `CLAUDE.md` and `AGENTS.md` from the
tree they work in. A pull request that changes one can steer its own review, so
the comment carries a warning when that is the case.

This works on any pull request, including the ones forkdeck opened, which gets
you a second look by both models on the pushed commit rather than the working
tree.
