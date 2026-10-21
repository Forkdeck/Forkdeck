//! Decides which tasks of a plan may start: the task list on the parent issue says what waits on
//! what, the forge says what merged, and the ready label is the output.
use crate::{config::RepoCfg, orchestrator::Hub, state::Provider, team, taskgraph::{self, Graph, Kind, Task}, tracker::{self, PrState}};
use std::{collections::HashMap, sync::{LazyLock, Mutex}};

/// What the forge says about one issue of a plan.
#[derive(Clone, Debug, Default)]
pub struct Fact { pub open: bool, pub completed: bool, pub labels: Vec<String>, pub pr: Option<PrState> }  // pr None = not asked/unknown

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change { Label(u64), Unlabel(u64, String), Close(u64) }   // Unlabel carries the log reason

/// Every answer must be a yes for a yes; one unknown makes the whole thing unknown, never a guess.
fn all(items: impl Iterator<Item = Option<bool>>) -> Option<bool> {
    let (mut unknown, mut no) = (false, false);
    for i in items {
        match i { None => unknown = true, Some(false) => no = true, Some(true) => {} }
    }
    if unknown { None } else { Some(!no) }
}

/// Is the task finished? Work is finished by a merged pull request, a decision by being closed as
/// completed, a group by everything in it. None: the facts needed are not known.
pub fn done(g: &Graph, id: &str, facts: &HashMap<u64, Fact>) -> Option<bool> {
    let t = g.task(id)?;
    match t.kind {
        Kind::Group => all(g.children(Some(id)).iter().map(|c| done(g, &c.id, facts))),
        Kind::Decision => facts.get(&t.number?).map(|f| !f.open && f.completed),
        Kind::Work => match facts.get(&t.number?)?.pr {
            Some(PrState::Merged(_)) => Some(true),
            Some(_) => Some(false),
            None => None,
        },
    }
}

/// Has everything the task waits on finished?
fn ready(g: &Graph, id: &str, facts: &HashMap<u64, Fact>) -> Option<bool> {
    all(g.effective_after(id).iter().map(|a| done(g, a, facts)))
}

fn has(r: &RepoCfg, f: &Fact, label: &str) -> bool { tracker::has_label(r, &f.labels, label) }

/// The dependencies of a task that are not finished, as "#a, #b".
fn unmet(g: &Graph, id: &str, facts: &HashMap<u64, Fact>) -> String {
    let waits: Vec<String> = g.effective_after(id).into_iter().filter(|a| done(g, a, facts) != Some(true)).collect();
    waits.iter().map(|a| g.task(a).and_then(|t| t.number).map_or(format!("#{a}"), |n| format!("#{n}"))).collect::<Vec<_>>().join(", ")
}

/// What to do on the forge. The plan's own issue is the one fact that belongs to no task.
pub fn changes(r: &RepoCfg, g: &Graph, facts: &HashMap<u64, Fact>) -> Vec<Change> {
    let mut out = Vec::new();
    for t in &g.tasks {
        let Some(n) = t.number else { continue };
        let Some(f) = facts.get(&n) else { continue };
        // Only work is ever built: a ready label on anything else is never right.
        if t.kind != Kind::Work && has(r, f, &r.label) { out.push(Change::Unlabel(n, "is not a task to build".into())); }
        match t.kind {
            Kind::Work => match (ready(g, &t.id, facts), done(g, &t.id, facts)) {
                (Some(false), Some(_)) if has(r, f, &r.label) => out.push(Change::Unlabel(n, format!("waits for {} to merge", unmet(g, &t.id, facts)))),
                (Some(true), Some(false)) if f.open && ![&r.label, &r.working_label, &r.done_label, &r.failed_label, &r.human_label].iter().any(|l| has(r, f, l)) => out.push(Change::Label(n)),
                _ => {}
            },
            Kind::Group if f.open && done(g, &t.id, facts) == Some(true) => out.push(Change::Close(n)),
            _ => {}
        }
    }
    let numbered: Vec<u64> = g.tasks.iter().filter_map(|t| t.number).collect();
    let mut roots = facts.iter().filter(|(n, _)| !numbered.contains(n));
    if let (Some((n, f)), None) = (roots.next(), roots.next()) {
        if f.open && all(g.children(None).iter().map(|t| done(g, &t.id, facts))) == Some(true) { out.push(Change::Close(*n)); }
    }
    out
}

/// The words after a task on the plan's issue.
pub fn status(r: &RepoCfg, g: &Graph, t: &Task, facts: &HashMap<u64, Fact>) -> String {
    let Some(f) = t.number.and_then(|n| facts.get(&n)) else { return String::new() };
    match t.kind {
        Kind::Group => if done(g, &t.id, facts) == Some(true) { "done".into() } else { String::new() },
        Kind::Decision => if done(g, &t.id, facts) == Some(true) { "done".into() } else { "needs you".into() },
        Kind::Work => match f.pr {
            Some(PrState::Merged(n)) => format!("merged in PR #{n}"),
            Some(PrState::Closed(n)) => format!("PR #{n} closed unmerged — needs you"),
            Some(PrState::Open(n)) => format!("PR #{n} open"),
            _ if has(r, f, &r.failed_label) => "failed — needs you".into(),
            _ if has(r, f, &r.human_label) => "needs you".into(),
            _ if has(r, f, &r.working_label) => "working".into(),
            _ if has(r, f, &r.done_label) => "done".into(),
            _ if has(r, f, &r.label) => "ready".into(),
            _ if !f.open => "needs you".into(),
            _ => match ready(g, &t.id, facts) {
                Some(true) => "ready".into(),
                Some(false) => format!("waiting for {}", unmet(g, &t.id, facts)),
                None => String::new(),
            },
        },
    }
}

/// The last problem logged for each plan, so a problem that stays is said once, not on every poll.
static LOGGED: LazyLock<Mutex<HashMap<(String, u64), String>>> = LazyLock::new(Default::default);

fn log(hub: &Hub, msg: String) {
    hub.state.lock().unwrap_or_else(|e| e.into_inner()).log("schedule", msg);
}

fn note(hub: &Hub, r: &RepoCfg, root: u64, problem: Option<String>) {
    let mut seen = LOGGED.lock().unwrap_or_else(|e| e.into_inner());
    let key = (r.repo.clone(), root);
    match problem {
        None => { seen.remove(&key); }
        Some(p) if seen.get(&key) != Some(&p) => { log(hub, p.clone()); seen.insert(key, p); }
        Some(_) => {}
    }
}

fn fact_of(i: &tracker::RemoteIssue) -> Fact {
    Fact { open: i.open, completed: i.completed, labels: i.labels.clone(), pr: None }
}

/// The pull request that came of the task's issue. A merge is final, so it is asked about once;
/// the cache is keyed by forge host, repository, base and branch, with the pull request's number.
async fn pull_request(hub: &Hub, r: &RepoCfg, issue: u64) -> Result<PrState, String> {
    let key = format!("{}|{}|{}:forkdeck/issue-{issue}", r.host, r.repo, r.base);
    let cached = hub.merged.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|(k, _)| *k == key).map(|(_, pr)| *pr);
    if let Some(pr) = cached { return Ok(PrState::Merged(pr)); }
    let state = tracker::pr_for_branch(r, &format!("forkdeck/issue-{issue}")).await?;
    if let PrState::Merged(pr) = state { hub.merged.lock().unwrap_or_else(|e| e.into_inner()).insert((key, pr)); }
    Ok(state)
}

/// Schedule one plan: read its list, ask the forge, apply what follows, show the progress.
async fn plan(hub: &Hub, r: &RepoCfg, root: &tracker::RemoteIssue) {
    let Ok(rn) = tracker::number(&root.reference.key) else { return };
    let listed = match tracker::section_between(&root.body, taskgraph::OPEN, taskgraph::CLOSE) {
        Ok(Some(s)) => taskgraph::read(&s).map(|g| (s, g)),
        Ok(None) => Err("The issue has no task list.".to_string()),
        Err(e) => Err(e),
    };
    let (section, g) = match listed {
        Ok(v) => v,
        Err(e) => return note(hub, r, rn, Some(format!("#{rn}: scheduling stopped: {e}"))),
    };
    let mut problems: Vec<String> = Vec::new();
    let mut facts: HashMap<u64, Fact> = HashMap::new();
    facts.insert(rn, fact_of(root));
    let ours: Vec<u64> = g.tasks.iter().filter_map(|t| t.number).collect();
    let parents: Vec<u64> = std::iter::once(rn).chain(g.tasks.iter().filter(|t| t.kind == Kind::Group).filter_map(|t| t.number)).collect();
    for p in parents {
        match tracker::sub_issues(r, &p.to_string()).await {
            // Only the plan's own tasks are facts; any other sub-issue of these parents is not ours.
            Ok(list) => for i in &list {
                if let Some(n) = tracker::number(&i.reference.key).ok().filter(|n| ours.contains(n)) { facts.insert(n, fact_of(i)); }
            },
            Err(e) => problems.push(format!("#{rn}: could not read the sub-issues of #{p}: {e}")),
        }
    }
    for t in g.tasks.iter().filter(|t| t.kind == Kind::Work) {
        let Some(n) = t.number else { continue };
        let Some(f) = facts.get(&n) else { continue };
        // Only a task that has run, or whose issue is closed, can have a pull request worth asking about.
        let state = if !f.open || has(r, f, &r.done_label) || has(r, f, &r.working_label) {
            match pull_request(hub, r, n).await {
                Ok(s) => Some(s),
                Err(e) => { problems.push(format!("#{rn}: could not read the pull request of #{n}: {e}")); None }
            }
        } else {
            Some(PrState::None)
        };
        if let Some(f) = facts.get_mut(&n) { f.pr = state; }
    }
    for c in changes(r, &g, &facts) {
        let (key, result) = match &c {
            Change::Label(n) => { log(hub, format!("#{n}: ready to start (#{rn})")); (*n, tracker::set_label(r, &n.to_string(), &r.label, true).await) }
            Change::Unlabel(n, why) => { log(hub, format!("#{n}: {why}")); (*n, tracker::set_label(r, &n.to_string(), &r.label, false).await) }
            Change::Close(n) => { log(hub, format!("#{n}: finished, closing")); (*n, tracker::close(r, &n.to_string()).await) }
        };
        match result {
            Err(e) => problems.push(format!("#{key}: {e}")),
            // What the list shows must follow what was just done, or the next pass has to correct it.
            Ok(()) => if let Some(f) = facts.get_mut(&key) {
                match &c {
                    Change::Label(_) => f.labels.push(r.label.clone()),
                    Change::Unlabel(..) => f.labels.retain(|l| !tracker::has_label(r, std::slice::from_ref(l), &r.label)),
                    Change::Close(_) => { f.open = false; f.completed = true; }
                }
            },
        }
    }
    // With any fact missing the statuses would read as if the work had not moved: leave the list as it is.
    if problems.is_empty() {
        let shown = taskgraph::render(&g, &|t| status(r, &g, t, &facts));
        if shown != section {
            if let Err(e) = show(r, root, &section, &shown).await { problems.push(format!("#{rn}: {e}")); }
        }
    }
    note(hub, r, rn, if problems.is_empty() { None } else { Some(problems.join("; ")) });
}

/// The tasks whose facts decide whether `id` is done: itself, or what is under it.
fn leaves<'a>(g: &'a Graph, id: &str, out: &mut Vec<&'a Task>) {
    let Some(t) = g.task(id) else { return };
    if t.kind == Kind::Group { for c in g.children(Some(id)) { leaves(g, &c.id, out); } } else { out.push(t); }
}

/// Everything needed to judge the dependencies of the task `id`, read fresh from the forge.
async fn dependency_facts(hub: &Hub, r: &RepoCfg, g: &Graph, id: &str) -> Result<HashMap<u64, Fact>, String> {
    let mut needed = Vec::new();
    for a in g.effective_after(id) { leaves(g, &a, &mut needed); }
    let mut facts = HashMap::new();
    for t in needed {
        let Some(n) = t.number else { continue };
        let mut f = fact_of(&tracker::read(r, &n.to_string()).await?);
        if t.kind == Kind::Work { f.pr = Some(pull_request(hub, r, n).await?); }
        facts.insert(n, f);
    }
    Ok(facts)
}

/// Right before a task is claimed: is everything it depends on merged? A label alone is not proof,
/// it may be stale or put on by hand. Issues that are not tasks of a plan always pass.
pub async fn gate(hub: &Hub, r: &RepoCfg, n: u64) -> Result<(), String> {
    if r.provider != Provider::Github { return Ok(()); }
    let unreadable = |e: String| format!("{}#{n} cannot start: its task list cannot be read: {e}", team::TAKEN);
    let issue = tracker::read(r, &n.to_string()).await.map_err(unreadable)?;
    let Some((_, id, root)) = taskgraph::parse_task_marker(&issue.body) else { return Ok(()) };
    let parent = tracker::read(r, &root.to_string()).await.map_err(unreadable)?;
    let g = match tracker::section_between(&parent.body, taskgraph::OPEN, taskgraph::CLOSE) {
        Ok(Some(s)) => taskgraph::read(&s),
        Ok(None) => Err("The issue has no task list.".to_string()),
        Err(e) => Err(e),
    }.map_err(unreadable)?;
    let Some(task) = g.task(&id) else { return Err(unreadable(format!("it has no task {id}"))) };
    if task.kind != Kind::Work {
        log(hub, format!("#{n}: is not a task to build"));
        if let Err(e) = tracker::set_label(r, &n.to_string(), &r.label, false).await { log(hub, format!("#{n}: could not remove {}: {e}", r.label)); }
        return Err(format!("{}#{n} is not a task to build", team::TAKEN));
    }
    let facts = dependency_facts(hub, r, &g, &id).await.map_err(unreadable)?;
    let verdict = ready(&g, &id, &facts);
    if verdict == Some(true) { return Ok(()); }
    let list = unmet(&g, &id, &facts);
    // Only a definite "not yet" takes the label away; a fact that could not be read leaves it for the next try.
    if verdict == Some(false) {
        log(hub, format!("#{n}: waits for {list} to merge"));
        // Not started either way; a label that stays is taken away again by the next scheduling pass.
        if let Err(e) = tracker::set_label(r, &n.to_string(), &r.label, false).await { log(hub, format!("#{n}: could not remove {}: {e}", r.label)); }
    }
    Err(format!("{}#{n} waits for {list} to merge", team::TAKEN))
}

/// Write the list into the issue, unless a person edited the issue since it was read.
async fn show(r: &RepoCfg, root: &tracker::RemoteIssue, section: &str, shown: &str) -> Result<(), String> {
    let body = tracker::replace_between(&root.body, taskgraph::OPEN, taskgraph::CLOSE, &Some(section.to_string()), shown)?;
    let key = &root.reference.key;
    if tracker::read(r, key).await?.body != root.body { return Err("The issue was edited meanwhile; the task list is updated on the next poll.".into()); }
    tracker::update(r, key, &body).await
}

/// One scheduling pass over every plan in the repository.
pub async fn pass(hub: &Hub, idx: usize) {
    let r = &hub.cfg.repos[idx];
    if r.provider != Provider::Github { return; }
    match tracker::labelled(r, &r.plan_label).await {
        Ok(roots) => for root in &roots { plan(hub, r, root).await },
        Err(e) => note(hub, r, 0, Some(format!("{}: scheduling stopped: {e}", r.repo))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::taskgraph;

    const PLAN: &str = "```forkdeck-tasks\n{\"version\":1,\"tasks\":[\
 {\"id\":\"g0\",\"title\":\"Storage\",\"kind\":\"group\"},\
 {\"id\":\"a\",\"title\":\"Records\",\"kind\":\"work\",\"parent\":\"g0\"},\
 {\"id\":\"g1\",\"title\":\"API\",\"kind\":\"group\",\"after\":[\"g0\"]},\
 {\"id\":\"b\",\"title\":\"Endpoint\",\"kind\":\"work\",\"parent\":\"g1\"},\
 {\"id\":\"c\",\"title\":\"Sync\",\"kind\":\"work\",\"parent\":\"g1\",\"after\":[\"b\"]},\
 {\"id\":\"d\",\"title\":\"Pick backend\",\"kind\":\"decision\"}]}\n```";

    const ROOT: u64 = 10;

    // g0=#20 a=#21 g1=#22 b=#23 c=#24 d=#25 under root #10
    fn numbered() -> Graph {
        let mut g = taskgraph::extract(PLAN).unwrap().unwrap();
        for (i, t) in g.tasks.iter_mut().enumerate() { t.number = Some(20 + i as u64); }
        g
    }

    fn repo() -> RepoCfg {
        let cfg: crate::config::Config = toml::from_str("[[repos]]\nprovider='github'\nrepo='o/r'\npath='/tmp'\n").unwrap();
        cfg.repos[0].clone()
    }

    fn fact(open: bool, labels: &[&str], pr: Option<PrState>) -> Fact {
        Fact { open, completed: !open, labels: labels.iter().map(|l| l.to_string()).collect(), pr }
    }

    /// Everything open and not started, as freshly created; the plan's own issue is #10.
    fn fresh() -> HashMap<u64, Fact> {
        let mut f = HashMap::new();
        f.insert(ROOT, fact(true, &["forkdeck:plan"], None));
        for n in [20, 22, 25] { f.insert(n, fact(true, &[], None)); }
        for n in [21, 23, 24] { f.insert(n, fact(true, &[], Some(PrState::None))); }
        f
    }

    #[test]
    fn only_tasks_with_merged_dependencies_get_the_label() {
        let (r, g) = (repo(), numbered());
        let mut f = fresh();
        f.insert(21, fact(false, &[], Some(PrState::Merged(71))));
        f.insert(24, fact(true, &["agent:ready"], Some(PrState::None)));
        assert_eq!(changes(&r, &g, &f), vec![Change::Close(20), Change::Label(23), Change::Unlabel(24, "waits for #23 to merge".into())]);
    }

    #[test]
    fn a_closed_unmerged_pr_is_not_done() {
        let (r, g) = (repo(), numbered());
        let mut f = fresh();
        f.insert(21, fact(false, &[], Some(PrState::Closed(5))));
        assert_eq!(changes(&r, &g, &f), vec![]);
        assert_eq!(done(&g, "a", &f), Some(false));
        assert_eq!(status(&r, &g, g.task("a").unwrap(), &f), "PR #5 closed unmerged — needs you");
        assert_eq!(status(&r, &g, g.task("b").unwrap(), &f), "waiting for #20");
    }

    #[test]
    fn unknown_facts_change_nothing() {
        let (r, g) = (repo(), numbered());
        let mut f = fresh();
        f.insert(21, fact(false, &[], None));
        f.insert(24, fact(true, &["agent:ready"], Some(PrState::None)));
        assert_eq!(done(&g, "a", &f), None);
        assert_eq!(changes(&r, &g, &f), vec![]);
        // A blocked task whose own pull request could not be read keeps its label too.
        f.insert(21, fact(false, &[], Some(PrState::Closed(5))));
        f.insert(24, fact(true, &["agent:ready"], None));
        assert_eq!(changes(&r, &g, &f), vec![]);
    }

    #[test]
    fn a_finished_group_and_plan_are_closed() {
        let (r, g) = (repo(), numbered());
        let mut f = fresh();
        for (n, pr) in [(21, 71), (23, 72), (24, 73)] { f.insert(n, fact(false, &[], Some(PrState::Merged(pr)))); }
        f.insert(25, fact(false, &[], None));
        assert_eq!(changes(&r, &g, &f), vec![Change::Close(20), Change::Close(22), Change::Close(ROOT)]);
        assert_eq!(status(&r, &g, g.task("a").unwrap(), &f), "merged in PR #71");
        assert_eq!(status(&r, &g, g.task("g1").unwrap(), &f), "done");
    }

    #[test]
    fn labelled_working_or_failed_tasks_are_left_alone() {
        let (r, g) = (repo(), numbered());
        let mut f = fresh();
        f.insert(21, fact(false, &[], Some(PrState::Merged(71))));
        f.insert(23, fact(true, &["agent:working"], Some(PrState::None)));
        assert_eq!(changes(&r, &g, &f), vec![Change::Close(20)]);
        assert_eq!(status(&r, &g, g.task("b").unwrap(), &f), "working");
        f.insert(23, fact(true, &["agent:needs-human"], Some(PrState::None)));
        assert_eq!(changes(&r, &g, &f), vec![Change::Close(20)]);
    }

    #[test]
    fn a_ready_label_on_a_group_or_decision_is_removed() {
        let (r, g) = (repo(), numbered());
        let mut f = fresh();
        f.insert(22, fact(true, &["agent:ready"], None));
        f.insert(25, fact(true, &["agent:ready"], None));
        let c = changes(&r, &g, &f);
        assert!(c.iter().any(|c| matches!(c, Change::Unlabel(22, _))) && c.iter().any(|c| matches!(c, Change::Unlabel(25, _))), "{c:?}");
    }

    use crate::{planning::Action, subissues::tests::{forge, isolated, number, reviewed, until}};
    use std::path::Path;

    fn mutations(root: &Path) -> usize {
        std::fs::read_to_string(root.join("forge/calls.log")).unwrap_or_default().lines().filter(|l| ["POST", "PATCH", "DELETE"].iter().any(|m| l.starts_with(m))).count()
    }

    fn logged(hub: &Hub, needle: &str) -> bool {
        hub.state.lock().unwrap().log.iter().any(|l| l.msg.contains(needle))
    }

    #[test]
    fn an_unreadable_list_changes_no_labels() {
        isolated("schedule::tests::an_unreadable_list_changes_no_labels", |root| {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            rt.block_on(async {
                let mut g = taskgraph::extract("```forkdeck-tasks\n{\"version\":1,\"tasks\":[{\"id\":\"a\",\"title\":\"Records\",\"kind\":\"work\"}]}\n```").unwrap().unwrap();
                g.tasks[0].number = Some(2);
                let edited = taskgraph::render(&g, &|_| String::new()).replace("#2 Records", "#2 Records faster");
                let state = serde_json::json!({
                    "issues": {
                        "1": {"title": "Epic", "body": format!("The epic.\n\n{edited}"), "state": "open", "labels": ["forkdeck:plan"]},
                        "2": {"title": "Records", "body": "", "state": "open", "labels": ["agent:ready"]},
                    },
                    "sub_issues": {"1": [2]},
                });
                std::fs::write(root.join("forge/state.json"), state.to_string()).unwrap();
                let cfg: crate::config::Config = toml::from_str(&format!("[[repos]]\nprovider='github'\nrepo='o/r'\npath={:?}\n", root.to_str().unwrap())).unwrap();
                let hub = Hub::new(cfg, tokio::runtime::Handle::current());
                pass(&hub, 0).await;
                pass(&hub, 0).await;
                let s = forge(root);
                assert_eq!(s["issues"]["2"]["labels"], serde_json::json!(["agent:ready"]));
                assert_eq!(mutations(root), 0);
                assert!(logged(&hub, "scheduling stopped"));
                let n = hub.state.lock().unwrap().log.iter().filter(|l| l.msg.contains("scheduling stopped")).count();
                assert_eq!(n, 1, "said once, not on every poll");
            });
        });
    }

    #[test]
    fn a_merge_starts_what_it_unblocked() {
        isolated("schedule::tests::a_merge_starts_what_it_unblocked", |root| {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            rt.block_on(async {
                let (hub, id) = reviewed(root).await;
                hub.plan_action(id, Action::Publish).await.unwrap();
                until(&hub, id).await;
                let mut s = forge(root);
                let (records, endpoint, sync) = (number(&s, "Records"), number(&s, "Endpoint"), number(&s, "Sync"));
                // The worker finished and its pull request merged, which closed the issue.
                s["issues"][records.to_string()]["state"] = "closed".into();
                s["pulls"] = serde_json::json!([{"number": 71, "state": "closed", "merged_at": "2026-10-01T00:00:00Z", "base": {"ref": "main"}, "head": {"ref": format!("forkdeck/issue-{records}")}}]);
                std::fs::write(root.join("forge/state.json"), s.to_string()).unwrap();
                pass(&hub, 0).await;
                let s = forge(root);
                let labels = |n: u64| s["issues"][n.to_string()]["labels"].clone();
                assert_eq!(labels(endpoint), serde_json::json!(["agent:ready"]));
                assert_eq!(labels(sync), serde_json::json!([]), "still waits for Endpoint");
                assert_eq!(s["issues"][number(&s, "Storage").to_string()]["state"], "closed", "its only task merged");
                let body = s["issues"]["1"]["body"].as_str().unwrap();
                assert!(body.contains(&format!("- [x] #{records} Records — merged in PR #71")), "{body}");
                assert!(body.contains(&format!("#{sync} Sync · after #{endpoint} — waiting for #{endpoint}")), "{body}");
                let before = mutations(root);
                pass(&hub, 0).await;
                assert_eq!(mutations(root), before, "nothing left to do, nothing written");
            });
        });
    }

    #[test]
    fn a_stray_sub_issue_does_not_stop_a_finished_plan_closing() {
        isolated("schedule::tests::a_stray_sub_issue_does_not_stop_a_finished_plan_closing", |root| {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            rt.block_on(async {
                let (hub, id) = reviewed(root).await;
                hub.plan_action(id, Action::Publish).await.unwrap();
                until(&hub, id).await;
                let mut s = forge(root);
                let mut pulls = Vec::new();
                for (i, t) in ["Records", "Endpoint", "Sync"].iter().enumerate() {
                    let n = number(&s, t);
                    s["issues"][n.to_string()]["state"] = "closed".into();
                    pulls.push(serde_json::json!({"number": 70 + i, "state": "closed", "merged_at": "2026-10-01T00:00:00Z", "base": {"ref": "main"}, "head": {"ref": format!("forkdeck/issue-{n}")}}));
                }
                s["pulls"] = pulls.into();
                let d = number(&s, "Pick backend").to_string();
                s["issues"][&d]["state"] = "closed".into();
                s["issues"][&d]["state_reason"] = "completed".into();
                s["issues"]["99"] = serde_json::json!({"title": "Unrelated", "body": "", "state": "open", "labels": []});
                s["sub_issues"]["1"].as_array_mut().unwrap().push(99.into());
                std::fs::write(root.join("forge/state.json"), s.to_string()).unwrap();
                pass(&hub, 0).await;
                let s = forge(root);
                assert_eq!(s["issues"]["1"]["state"], "closed");
                assert_eq!(s["issues"]["99"]["state"], "open", "not ours to close");
            });
        });
    }

    #[test]
    fn a_stale_label_is_removed_and_the_list_shows_it_in_the_same_pass() {
        isolated("schedule::tests::a_stale_label_is_removed_and_the_list_shows_it_in_the_same_pass", |root| {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            rt.block_on(async {
                let (hub, id) = reviewed(root).await;
                hub.plan_action(id, Action::Publish).await.unwrap();
                until(&hub, id).await;
                let mut s = forge(root);
                let (endpoint, sync) = (number(&s, "Endpoint"), number(&s, "Sync"));
                s["issues"][sync.to_string()]["labels"] = serde_json::json!(["agent:ready"]);
                std::fs::write(root.join("forge/state.json"), s.to_string()).unwrap();
                pass(&hub, 0).await;
                let s = forge(root);
                assert_eq!(s["issues"][sync.to_string()]["labels"], serde_json::json!([]));
                let body = s["issues"]["1"]["body"].as_str().unwrap();
                assert!(body.contains(&format!("#{sync} Sync · after #{endpoint} — waiting for #{endpoint}")), "{body}");
                let before = mutations(root);
                pass(&hub, 0).await;
                assert_eq!(mutations(root), before, "the list was right the first time");
            });
        });
    }

    #[test]
    fn a_hand_labelled_blocked_task_is_not_claimed() {
        isolated("schedule::tests::a_hand_labelled_blocked_task_is_not_claimed", |root| {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            rt.block_on(async {
                let (mut hub, id) = reviewed(root).await;
                std::sync::Arc::make_mut(&mut hub.cfg).automation.worktree_root = Some(root.join("worktrees"));
                hub.plan_action(id, Action::Publish).await.unwrap();
                until(&hub, id).await;
                let mut s = forge(root);
                let (records, endpoint, sync) = (number(&s, "Records"), number(&s, "Endpoint"), number(&s, "Sync"));
                // "Records" is already being worked on, so the poll has nothing of its own to start.
                s["issues"][records.to_string()]["labels"] = serde_json::json!(["agent:working"]);
                s["issues"][sync.to_string()]["labels"] = serde_json::json!(["agent:ready"]);
                std::fs::write(root.join("forge/state.json"), s.to_string()).unwrap();
                crate::issues::poll_once(&hub).await;
                let labels = |s: &serde_json::Value, n: u64| s["issues"][n.to_string()]["labels"].clone();
                assert_eq!(labels(&forge(root), sync), serde_json::json!([]), "the poll removed the label");
                // The label comes back just before the claim.
                let mut s = forge(root);
                s["issues"][sync.to_string()]["labels"] = serde_json::json!(["agent:ready"]);
                std::fs::write(root.join("forge/state.json"), s.to_string()).unwrap();
                let job = hub.submit(
                    format!("o/r#{sync} Sync"), String::new(),
                    crate::state::Source::Issue { repo_idx: 0, repo: "o/r".into(), number: sync, url: String::new() },
                    hub.cfg.repos[0].path.clone(), None, crate::state::Planning::Never,
                ).await.unwrap();
                let before = std::fs::read_to_string(root.join("forge/calls.log")).unwrap_or_default().lines().count();
                let err = crate::issues::prepare(&hub, job).await.unwrap_err();
                assert!(err.starts_with(team::TAKEN), "{err}");
                assert!(err.contains(&format!("#{sync} waits for ")) && err.contains(&format!("#{endpoint}")) && err.ends_with(" to merge"), "{err}");
                let s = forge(root);
                assert_eq!(labels(&s, sync), serde_json::json!([]));
                let calls = std::fs::read_to_string(root.join("forge/calls.log")).unwrap_or_default();
                assert!(calls.lines().skip(before).all(|l| !l.contains("working") && !l.contains("/comments")), "{calls}");
                assert!(s["issues"][sync.to_string()]["comments"].as_array().is_none_or(|c| c.is_empty()));
            });
        });
    }

    /// A forge call at the gate fails once, uncertainly: the claim must come back as "not started".
    fn gate_survives(test: &str, fail: fn(u64) -> String, said: fn(u64) -> String) {
        isolated(test, |root| {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            rt.block_on(async {
                let (mut hub, id) = reviewed(root).await;
                std::sync::Arc::make_mut(&mut hub.cfg).automation.worktree_root = Some(root.join("worktrees"));
                hub.plan_action(id, Action::Publish).await.unwrap();
                until(&hub, id).await;
                let mut s = forge(root);
                let sync = number(&s, "Sync");
                s["issues"][sync.to_string()]["labels"] = serde_json::json!(["agent:ready"]);
                std::fs::write(root.join("forge/state.json"), s.to_string()).unwrap();
                let job = hub.submit(
                    format!("o/r#{sync} Sync"), String::new(),
                    crate::state::Source::Issue { repo_idx: 0, repo: "o/r".into(), number: sync, url: String::new() },
                    hub.cfg.repos[0].path.clone(), None, crate::state::Planning::Never,
                ).await.unwrap();
                std::fs::write(root.join("forge/timeout_once"), fail(sync)).unwrap();
                let err = crate::issues::prepare(&hub, job).await.unwrap_err();
                assert!(err.starts_with(team::TAKEN), "{err}");
                assert!(err.contains(&said(sync)), "{err}");
            });
        });
    }

    #[test]
    fn a_forge_that_cannot_be_read_at_the_gate_is_not_started() {
        gate_survives("schedule::tests::a_forge_that_cannot_be_read_at_the_gate_is_not_started", |n| format!("GET repos/o/r/issues/{n}"), |n| format!("#{n} cannot start: its task list cannot be read"));
    }

    #[test]
    fn a_label_that_cannot_be_removed_at_the_gate_is_not_started() {
        gate_survives("schedule::tests::a_label_that_cannot_be_removed_at_the_gate_is_not_started", |n| format!("DELETE repos/o/r/issues/{n}/labels"), |n| format!("#{n} waits for "));
    }

    #[test]
    fn a_decision_labelled_ready_is_not_claimed() {
        isolated("schedule::tests::a_decision_labelled_ready_is_not_claimed", |root| {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            rt.block_on(async {
                let (mut hub, id) = reviewed(root).await;
                std::sync::Arc::make_mut(&mut hub.cfg).automation.worktree_root = Some(root.join("worktrees"));
                hub.plan_action(id, Action::Publish).await.unwrap();
                until(&hub, id).await;
                let mut s = forge(root);
                let d = number(&s, "Pick backend");
                s["issues"][d.to_string()]["labels"] = serde_json::json!(["agent:ready"]);
                std::fs::write(root.join("forge/state.json"), s.to_string()).unwrap();
                let job = hub.submit(
                    format!("o/r#{d} Pick backend"), String::new(),
                    crate::state::Source::Issue { repo_idx: 0, repo: "o/r".into(), number: d, url: String::new() },
                    hub.cfg.repos[0].path.clone(), None, crate::state::Planning::Never,
                ).await.unwrap();
                let err = crate::issues::prepare(&hub, job).await.unwrap_err();
                assert!(err.starts_with(team::TAKEN) && err.contains(&format!("#{d}")), "{err}");
                assert_eq!(forge(root)["issues"][d.to_string()]["labels"], serde_json::json!([]));
            });
        });
    }
}
