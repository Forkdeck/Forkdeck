[Documentation](README.md) · [Front page](../README.md)

# Troubleshooting

Start with `forkdeck doctor`: it reports installed agents, login/model availability,
router reachability, sandbox support, configured MCP tools and limits.

| Symptom | Where to look |
|---|---|
| An agent is missing, limited or cannot use a model | [Copilot and model fallback](agents.md), [terminal usage limits](terminal.md) |
| Decision server is unreachable, CUDA fails, or port 8000 is occupied | [Local decision model setup and diagnostics](routing.md#running-the-decision-model-locally) |
| A labelled issue stays queued | [Labels and checks](issues-and-prs.md), [daily limits and failure pauses](limits.md), [team claims](team.md) |
| An earlier issue branch already exists | [Retry and preserved attempts](issues-and-prs.md#issues) |
| A machine died with an issue on its working label | [Claim release](team.md), [shutdown behaviour](sandbox-and-network.md) |
| An interrupted typed task needs continuing | [History and resuming](terminal.md#conversations-history-and-resuming) |
| A plan publication is uncertain or interrupted | [Publication recovery](planning.md#issue-backed-planning); retain the hidden markers and retry from F6 |
| CI gives no result, or a deployment reports UNKNOWN | [Revision matching and rollback evidence](ci-and-deploys.md) |
| The browser cannot connect or sign in | [Bind address and access](web-ui.md), [OIDC proxy/provider setup](web-ui-oidc.md) |
| Browser changes are missing from the binary | [Rebuild the embedded frontend](development.md#web-development) |
| A documentation link fails | [Run the link checker](testing.md#documentation-links) |

For known integration gaps, see [development status](development.md#status).
