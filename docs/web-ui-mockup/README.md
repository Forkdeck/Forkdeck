# Web UI mockup

The approved look for Forkdeck's web UI (https://claude.ai/artifact/YbEiF3JDTKN6NugvVJVSXZ):

- `Main.dc.html` — board with six lanes (Queue, Build, Review, Delivery, Done, Attention), composer, and the job detail panel (stages, Worker/Reviewer/Conversation log tabs, reply bar). 1440×900.
- `Plan.dc.html` — issue plans: plan list, a plan with Tasks/Plan/Review tabs, the sub-issue task list, scheduler state and a decision that needs the user. 1440×900.
- `Phone.dc.html` — the board at 390×844: filter chips, one job list, composer at the bottom.

These are Design Component files: the markup between `<x-dc>` tags is plain HTML with inline styles, `{{hole}}` values come from the `renderVals()` script at the bottom, `<sc-for>` repeats and `<sc-if>` branches. Colours, fonts (IBM Plex Sans / IBM Plex Mono), spacing, radii and copy are the reference: reproduce them exactly. The job data in them is sample data.
