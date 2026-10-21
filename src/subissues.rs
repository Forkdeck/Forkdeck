//! Turn an accepted task list into forge issues, as a transaction that can be retried: progress is
//! saved before each forge write, so a crash or timeout never creates an issue twice.
use crate::{config::RepoCfg, orchestrator::Hub, planning, taskgraph, tracker};

const IN_FLIGHT: &str = "A sub-issue create may still be in flight; retry later. Nothing was duplicated.";

/// The API id of a task's issue, which linking needs; read back when the saved ids do not have it.
async fn api_id(r: &RepoCfg, c: &planning::Creation, t: &taskgraph::Task) -> Result<u64, String> {
    if let Some(id) = c.ids.get(&t.id) { return Ok(*id); }
    tracker::read(r, &t.number.ok_or("Task has no issue yet")?.to_string()).await.map(|i| i.id)
}

fn numbers_of(issues: &[tracker::RemoteIssue]) -> Vec<u64> {
    issues.iter().filter_map(|i| i.reference.key.parse().ok()).collect()
}

fn unmark(hub: &Hub, id: u32, task: &str) {
    planning::edit(hub, id, |p| { p.creation.as_mut().unwrap().attempted.remove(task); });
}

/// Create the issue for the task at graph index `i`, unless it already has one.
async fn create_one(hub: &Hub, id: u32, r: &RepoCfg, root: u64, i: usize) -> Result<(), String> {
    let p = planning::plan(hub, id)?;
    let (g, c) = (p.graph.as_ref().ok_or("This plan has no task list.")?, p.creation.as_ref().ok_or("Creation was not started.")?);
    let t = &g.tasks[i];
    if t.number.is_some() { return Ok(()); }
    let marker = taskgraph::task_marker(&c.token, &t.id, root);
    let created = if c.attempted.contains(&t.id) {
        tracker::find_created(r, &marker).await?.ok_or(IN_FLIGHT)?
    } else {
        // The forge answers now, so a failure after this is one the create may have survived.
        tracker::reachable(r).await?;
        planning::edit(hub, id, |p| { p.creation.as_mut().unwrap().attempted.insert(t.id.clone()); });
        if let Err(e) = planning::durable(hub, id) {
            unmark(hub, id, &t.id);
            return Err(e);
        }
        match tracker::create(r, &t.title, &taskgraph::task_body(t, g, &c.token, root)).await {
            Ok(issue) => issue,
            // Refused outright: nothing was created, so the next retry may create it.
            Err(e) if tracker::refused(&e) => {
                unmark(hub, id, &t.id);
                hub.persist(id)?;
                return Err(e);
            }
            Err(e) => return Err(e),
        }
    };
    let number = tracker::number(&created.reference.key)?;
    planning::edit(hub, id, |p| {
        if let Some(task) = p.graph.as_mut().and_then(|g| g.tasks.iter_mut().find(|x| x.id == t.id)) { task.number = Some(number); }
        p.creation.as_mut().unwrap().ids.insert(t.id.clone(), created.id);
    });
    planning::durable(hub, id)
}

/// Attach every task under its group (or the root) and link what it waits on. Both lists are read
/// first, so a retry adds only what is missing.
async fn link(hub: &Hub, id: u32, r: &RepoCfg, root: u64) -> Result<(), String> {
    planning::durable(hub, id)?;
    let p = planning::plan(hub, id)?;
    let (g, c) = (p.graph.as_ref().ok_or("This plan has no task list.")?, p.creation.as_ref().ok_or("Creation was not started.")?);
    let mut kids: std::collections::HashMap<u64, Vec<u64>> = Default::default();
    for t in &g.tasks {
        let n = t.number.ok_or("Task has no issue yet")?;
        let parent = match &t.parent { Some(pid) => g.task(pid).and_then(|x| x.number).ok_or("Group has no issue yet")?, None => root };
        if let std::collections::hash_map::Entry::Vacant(e) = kids.entry(parent) { e.insert(numbers_of(&tracker::sub_issues(r, &parent.to_string()).await?)); }
        if !kids[&parent].contains(&n) {
            planning::durable(hub, id)?;
            tracker::add_sub_issue(r, &parent.to_string(), api_id(r, c, t).await?).await?;
        }
        if t.after.is_empty() { continue; }
        let have = tracker::blocked_by(r, &n.to_string()).await?;
        for a in &t.after {
            let blocker = g.task(a).ok_or("Dependency is not in the task list")?;
            if !have.contains(&blocker.number.ok_or("Dependency has no issue yet")?) {
                planning::durable(hub, id)?;
                tracker::add_blocked_by(r, &n.to_string(), api_id(r, c, blocker).await?).await?;
            }
        }
    }
    planning::edit(hub, id, |p| p.creation.as_mut().unwrap().linked = true);
    planning::durable(hub, id)
}

/// Show the task list on the root issue and label the plan and the tasks that need a person.
async fn list(hub: &Hub, id: u32, r: &RepoCfg, root: u64) -> Result<(), String> {
    planning::durable(hub, id)?;
    let g = planning::plan(hub, id)?.graph.ok_or("This plan has no task list.")?;
    let key = root.to_string();
    let remote = tracker::read(r, &key).await?;
    let section = taskgraph::render(&g, &|_| String::new());
    let current = tracker::section_between(&remote.body, taskgraph::OPEN, taskgraph::CLOSE)?;
    if current.as_ref() != Some(&section) {
        let body = tracker::replace_between(&remote.body, taskgraph::OPEN, taskgraph::CLOSE, &current, &section)?;
        // The forges have no conditional update for an issue body: read it once more right before
        // writing, so a person's edit made meanwhile is not overwritten. A retry starts from it.
        if tracker::read(r, &key).await?.body != remote.body {
            return Err("The issue was edited while the task list was being written; retry to write it against the latest text.".into());
        }
        planning::durable(hub, id)?;
        tracker::update(r, &key, &body).await?;
    }
    planning::durable(hub, id)?;
    tracker::set_label(r, &key, &r.plan_label, true).await?;
    for t in g.tasks.iter().filter(|t| t.kind == taskgraph::Kind::Decision) {
        planning::durable(hub, id)?;
        tracker::set_label(r, &t.number.ok_or("Task has no issue yet")?.to_string(), &r.human_label, true).await?;
    }
    planning::edit(hub, id, |p| p.creation.as_mut().unwrap().listed = true);
    planning::durable(hub, id)
}

/// Create the plan's sub-issues under its issue, linked in order. Safe to run again after any failure.
pub async fn materialize(hub: &Hub, id: u32) -> Result<String, String> {
    let p = planning::plan(hub, id)?;
    let r = planning::repo(hub, &p)?;
    if r.provider != crate::state::Provider::Github { return Err("Sub-issues are GitHub-only for now.".into()); }
    let graph = p.graph.as_ref().ok_or("This plan has no task list.")?;
    let root = tracker::number(&p.issue.as_ref().ok_or("Publish this plan first")?.key)?;
    let message = format!("Created {} sub-issues under #{root}.", graph.tasks.len());
    if p.creation.as_ref().is_some_and(|c| c.done) { return Ok(message); }
    if p.creation.is_none() {
        let token = format!("{}-{}", p.identity, p.revision);
        planning::edit(hub, id, |p| p.creation = Some(planning::Creation { token, ..Default::default() }));
    }
    for i in graph.creation_order() { create_one(hub, id, &r, root, i).await?; }
    if !planning::plan(hub, id)?.creation.is_some_and(|c| c.linked) { link(hub, id, &r, root).await?; }
    if !planning::plan(hub, id)?.creation.is_some_and(|c| c.listed) { list(hub, id, &r, root).await?; }
    planning::edit(hub, id, |p| p.creation.as_mut().unwrap().done = true);
    planning::durable(hub, id)?;
    Ok(message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::Config, orchestrator::Hub, planning::Action, state::*};
    use std::{path::{Path, PathBuf}, sync::Arc, time::Duration};

    const TASKS: &str = "Intro.\n\n```forkdeck-tasks\n{\"version\":1,\"tasks\":[\
 {\"id\":\"g0\",\"title\":\"Storage\",\"kind\":\"group\"},\
 {\"id\":\"a\",\"title\":\"Records\",\"kind\":\"work\",\"parent\":\"g0\"},\
 {\"id\":\"g1\",\"title\":\"API\",\"kind\":\"group\",\"after\":[\"g0\"]},\
 {\"id\":\"b\",\"title\":\"Endpoint\",\"kind\":\"work\",\"parent\":\"g1\"},\
 {\"id\":\"c\",\"title\":\"Sync\",\"kind\":\"work\",\"parent\":\"g1\",\"after\":[\"b\"]},\
 {\"id\":\"d\",\"title\":\"Pick backend\",\"kind\":\"decision\"}]}\n```\n\nPLAN: READY";

    /// Run `body` in a child process with a private environment: jobs write the journal and attempt
    /// records, and `gh` is the fake forge, whose state lives under `<root>/forge`.
    fn isolated(test: &str, body: impl FnOnce(&Path)) {
        const CHILD: &str = "FORKDECK_SUBISSUES_TEST_ROOT";
        let Ok(root) = std::env::var(CHILD) else {
            let root = std::env::temp_dir().join(crate::metrics::id());
            std::fs::create_dir_all(root.join("bin")).unwrap();
            std::fs::create_dir_all(root.join("forge")).unwrap();
            std::os::unix::fs::symlink(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake-forge"), root.join("bin/gh")).unwrap();
            let path = format!("{}:{}", root.join("bin").display(), std::env::var("PATH").unwrap_or_default());
            let out = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", test, "--nocapture"])
                .env(CHILD, &root).env("XDG_STATE_HOME", root.join("state")).env("FAKE_FORGE", root.join("forge")).env("PATH", path).output().unwrap();
            std::fs::remove_dir_all(&root).unwrap();
            assert!(out.status.success(), "{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
            return;
        };
        body(&PathBuf::from(root));
    }

    fn forge(root: &Path) -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(root.join("forge/state.json")).unwrap()).unwrap()
    }

    /// The number of the issue titled `title`.
    fn number(state: &serde_json::Value, title: &str) -> u64 {
        state["issues"].as_object().unwrap().iter().find(|(_, i)| i["title"] == title).map(|(n, _)| n.parse().unwrap()).unwrap()
    }

    fn numbers(state: &serde_json::Value, key: &str, owner: u64) -> Vec<u64> {
        let mut v: Vec<u64> = state[key][owner.to_string()].as_array().map(|a| a.iter().map(|n| n.as_u64().unwrap()).collect()).unwrap_or_default();
        v.sort();
        v
    }

    /// A reviewed task-list plan for root issue #1, linked and ready to publish.
    async fn reviewed(root: &Path) -> (Hub, u32) {
        use std::os::unix::fs::PermissionsExt;
        let plan = root.join("plan.json");
        std::fs::write(&plan, serde_json::to_string(TASKS).unwrap()).unwrap();
        let agent = root.join("agent");
        std::fs::write(&agent, format!(r#"#!/bin/sh
[ "$1" = --version ] && {{ echo fixture; exit 0; }}
prompt=$(cat)
case "$prompt" in
  *'Review this implementation PLAN'*) printf '{{"type":"result","is_error":false,"result":"Fine.\\nVERDICT: PASS"}}\n' ;;
  *'PLANNING ONLY'*) printf '{{"type":"result","is_error":false,"result":%s}}\n' "$(cat {plan:?})" ;;
  *) printf '{{"type":"result","is_error":false,"result":"Looked."}}\n' ;;
esac
"#)).unwrap();
        std::fs::set_permissions(&agent, std::fs::Permissions::from_mode(0o755)).unwrap();
        let dir = root.join("repo");
        std::fs::create_dir_all(&dir).unwrap();
        for args in [&["init", "-q"][..], &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "base"]] {
            assert!(std::process::Command::new("git").args(args).current_dir(&dir).status().unwrap().success());
        }
        std::fs::write(root.join("forge/state.json"), r#"{"issues":{"1":{"title":"Epic","body":"The epic.","state":"open","labels":[]}}}"#).unwrap();
        let cfg: Config = toml::from_str(&format!(r#"
[router]
kind = "rules"
default_agent = "claude"
[review]
mode = "never"
[knowledge]
enabled = false
[agents.claude]
cmd = {agent:?}
[agents.codex]
cmd = "/nonexistent"
[agents.copilot]
cmd = "/nonexistent"
[[repos]]
provider = "github"
repo = "o/r"
path = {dir:?}
"#, agent = agent.to_str().unwrap(), dir = dir.to_str().unwrap())).unwrap();
        let mut hub = Hub::new(cfg, tokio::runtime::Handle::current());
        hub.history = Some(Arc::new(crate::history::History::in_memory()));
        hub.detect_agents().await;
        let id = hub.submit("look".into(), "look around".into(), Source::Manual, dir, None, Planning::Never).await.unwrap();
        until(&hub, id).await;
        let plan = planning::Plan::new(&hub.cfg.repos[0], "1").unwrap();
        hub.link_plan(id, plan, "split it up".into()).await.unwrap();
        until(&hub, id).await;
        let p = hub.job(id, |j| j.issue_plan.clone().unwrap()).unwrap();
        assert_eq!(p.reviewed, Some(p.revision), "{}", hub.job(id, |j| j.outcome.clone()).unwrap());
        (hub, id)
    }

    async fn until(hub: &Hub, id: u32) {
        tokio::time::timeout(Duration::from_secs(60), async {
            while !hub.job(id, |j| j.status.finished()).unwrap() { tokio::time::sleep(Duration::from_millis(20)).await; }
        }).await.unwrap_or_else(|_| panic!("job #{id} did not finish: {:?}", hub.job(id, |j| (j.status, j.outcome.clone()))));
    }

    #[test]
    fn accepting_creates_linked_sub_issues_in_order() {
        isolated("subissues::tests::accepting_creates_linked_sub_issues_in_order", |root| {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            rt.block_on(async {
                let (hub, id) = reviewed(root).await;
                hub.plan_action(id, Action::Publish).await.unwrap();
                until(&hub, id).await;
                let outcome = hub.job(id, |j| j.outcome.clone()).unwrap();
                assert_eq!(outcome, "Created 6 sub-issues under #1.");
                let s = forge(root);
                assert_eq!(s["issues"].as_object().unwrap().len(), 7, "the root and six tasks");
                let n = |t: &str| number(&s, t);
                assert_eq!(numbers(&s, "sub_issues", 1), [n("Storage"), n("API"), n("Pick backend")]);
                assert_eq!(numbers(&s, "sub_issues", n("Storage")), [n("Records")]);
                assert_eq!(numbers(&s, "sub_issues", n("API")), [n("Endpoint"), n("Sync")]);
                assert_eq!(numbers(&s, "blocked_by", n("Sync")), [n("Endpoint")], "its own dependency only");
                assert_eq!(numbers(&s, "blocked_by", n("API")), [n("Storage")]);
                assert!(numbers(&s, "blocked_by", n("Records")).is_empty());
                let body = s["issues"]["1"]["body"].as_str().unwrap();
                assert!(body.contains(taskgraph::OPEN) && body.contains(&format!("- [ ] #{} Storage", n("Storage"))), "{body}");
                assert!(!body.contains("forkdeck-tasks"), "the plan text carries no task block");
                let labels = |k: &str| -> Vec<String> { s["issues"][k]["labels"].as_array().unwrap().iter().map(|l| l.as_str().unwrap().to_string()).collect() };
                assert_eq!(labels("1"), ["forkdeck:plan"]);
                assert_eq!(labels(&n("Pick backend").to_string()), ["needs-human"]);
                for t in ["Storage", "Records", "API", "Endpoint", "Sync", "Pick backend"] {
                    assert!(!labels(&n(t).to_string()).contains(&"agent:ready".to_string()), "{t}");
                }
                let task = s["issues"][n("Sync").to_string()]["body"].as_str().unwrap();
                assert!(task.contains("Part of #1") && task.contains(&format!("Depends on #{}", n("Endpoint"))), "{task}");
                let p = hub.job(id, |j| j.issue_plan.clone().unwrap()).unwrap();
                assert!(p.resolved() && p.graph.unwrap().tasks.iter().all(|t| t.number.is_some()));
            });
        });
    }

    #[test]
    fn a_create_that_may_have_landed_is_found_not_repeated() {
        isolated("subissues::tests::a_create_that_may_have_landed_is_found_not_repeated", |root| {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            rt.block_on(async {
                let (hub, id) = reviewed(root).await;
                // The third create reaches the forge and then reports a timeout.
                std::fs::write(root.join("forge/timeout_once"), "POST repos/o/r/issues #3").unwrap();
                hub.plan_action(id, Action::Publish).await.unwrap();
                until(&hub, id).await;
                assert_eq!(hub.job(id, |j| j.status).unwrap(), JobStatus::Failed);
                assert!(hub.job(id, |j| j.outcome.clone()).unwrap().contains("timed out"));
                hub.plan_action(id, Action::Publish).await.unwrap();
                until(&hub, id).await;
                assert_eq!(hub.job(id, |j| j.outcome.clone()).unwrap(), "Created 6 sub-issues under #1.");
                let s = forge(root);
                assert_eq!(s["issues"].as_object().unwrap().len(), 7, "six task issues and the root, none twice");
                assert!(hub.job(id, |j| j.issue_plan.clone().unwrap().resolved()).unwrap());
            });
        });
    }
}
