[Documentation](README.md) · [Front page](../README.md)

# Plans and sub-issues

A single-issue plan waits for **Ready for build** after publication. A task-list
plan creates sub-issues and schedules eligible tasks when their dependencies have
merged. Materialization currently supports GitHub; ordinary issue planning also
supports GitLab.

Start with [issue-backed planning](planning.md#issue-backed-planning). The
[browser plans view](web-ui.md) shows the task graph and scheduler observations;
unknown forge facts stay unknown.

The existing design and implementation documents describe the task format,
publication recovery, dependency rules and the work still planned:

- [Plans-to-sub-issues design](superpowers/specs/2026-10-21-plans-to-sub-issues-design.md)
- [Core implementation plan](superpowers/plans/2026-10-21-plans-to-sub-issues-core.md)

Adoption, graph-changing re-plans and comment-driven decision workflows are later
work; the design document includes those proposals. The [server-mode plan](web-ui-server-mode-plan.md)
also distinguishes shipped foundations from proposed work.
