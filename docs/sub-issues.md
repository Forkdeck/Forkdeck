[Documentation](README.md) · [Front page](../README.md)

# Plans and sub-issues

A single-issue plan waits for **Ready for build** after publication. A task-list
plan creates sub-issues and schedules eligible tasks when their dependencies have
merged. Materialization currently supports GitHub; ordinary issue planning also
supports GitLab.

Start with [issue-backed planning](planning.md#issue-backed-planning). The
[browser plans view](web-ui.md) shows the task graph and scheduler observations;
unknown forge facts stay unknown.

Adoption, graph-changing re-plans and comment-driven decision workflows are later
work. The [server-mode plan](web-ui-server-mode-plan.md) distinguishes shipped
foundations from proposed work.
