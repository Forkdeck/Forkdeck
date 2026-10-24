[Documentation](README.md) · [Front page](../README.md)

# Notes about a repository

Each job would otherwise start knowing nothing about the project beyond what
the CLIs read for themselves. forkdeck puts two things in front of the worker
with every issue, ahead of the task:

- **The team's notes:** `.forkdeck/notes.md` in the repository, as it is on the
  base branch. It is an ordinary file: write conventions in it and change it
  through a pull request like anything else. Everyone's forkdeck reads the
  same one.
- **This machine's own notes:** a file in `~/.local/state/forkdeck/notes/`, for
  things that are nobody else's business.

```sh
forkdeck notes acme/payments          # both, and where they are
forkdeck notes acme/payments add "Staging needs the VPN."   # your own
forkdeck notes                        # every repository and its file
```

The worker is told the notes are background: they do not change the task and
cannot grant access to anything. They come first in the prompt because they are
the same for every job on the repository, which is the part a model's context
cache can reuse.

**Learning from mistakes.** With `learn = true`, a job whose work was sent
back by a check or a reviewer, or a review that asked for changes, ends with
one more short run: a CLI is shown the findings and asked for at most three
general rules that would have prevented them.

```toml
[knowledge]
learn = true
```

Those rules are **proposed, not installed**. forkdeck pushes a commit that adds
them to `.forkdeck/notes.md` on a branch of its own (`forkdeck/notes-…`) and
opens a pull request, listing each rule and the issue it came from. Nothing
changes for any job until a person merges it; edit or delete rules there first,
or close it to discard them. While that pull request is open, rules learned
later are added to it, so there is one thing to look at per repository. The
branch is only ever pushed to normally, never forced, and it is built without
touching the clone's working tree.

Why a pull request: a learned rule is read by every later job of everyone on
the team, and the findings it is distilled from come from work that started
with somebody's issue text. forkdeck narrows what can get in (one line, at
most 200 characters, no links, no mentions, three per job, only the newest
`max_learned` kept), but it cannot judge whether a rule is true. A person can.

Two things keep the file honest:

- An agent working on an issue cannot change the notes on the side. Whatever
  it does under `.forkdeck/` is left out of the commit, and the log says so.
- A labelled pull request that touches `.forkdeck/` gets the same warning on
  its review as one that touches `CLAUDE.md` or `AGENTS.md`.

If the pull request cannot be made (the forge is down, the push is refused),
the rules wait in a file of their own that no job reads, and are proposed again
on a later poll. With sharing on, a rule reaches agents through an approved
pull request or not at all. `forkdeck notes` says how many are waiting.

`share = false` keeps learned rules on this machine instead: they go into its
own notes file under `## Learned by forkdeck` and take effect at once, for that
machine only. `learn` is off by default either way. A job that went
well teaches nothing and costs no extra run. Reviewers of labelled pull
requests are not given the notes yet.

Repository labels must be nonempty, contain no commas, and be distinct within each issue or review label set. If several `[[repos]]` entries name the same provider and repository (for separate deployment environments), issue and review polling uses the first entry only. Active worktrees are locked across processes for the duration of their jobs.

Stdio MCP servers receive their configured `env` plus basic process settings (`PATH`, `HOME`, `USER`, `LANG`, `LC_ALL`, `TMPDIR`, `SYSTEMROOT`). Other credentials are not inherited; add required variables explicitly. A deployment whose status is unrecognizable is not triggered. A revision already reported failed or reverted is not triggered again on retry.
