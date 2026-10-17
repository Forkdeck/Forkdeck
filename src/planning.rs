//! Durable issue-plan lifecycle. Remote writes run only after the job is saved to the history.
use crate::{config::RepoCfg, orchestrator::Hub, state::*, tracker};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum Action { #[default] Draft, Publish, Ready }

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Publication {
    pub revision: u64,
    pub token: String,
    pub section: String,
    pub expected: Option<String>,
    pub create_attempted: bool,
    pub comment_attempted: bool,
    pub summary: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    pub provider: Provider,
    pub repo: String,
    /// The forge host of the repository's clone: the same group/project may be on two GitLab hosts.
    #[serde(default)]
    pub host: String,
    pub issue: Option<tracker::IssueRef>,
    pub identity: String,
    pub revision: u64,
    pub draft: String,
    pub review: String,
    pub reviewed: Option<u64>,
    pub accepted: Option<u64>,
    pub published: Option<u64>,
    pub base: Option<String>,
    pub remote_context: String,
    pub pending: Option<Publication>,
    pub action: Action,
    pub ready: bool,
    pub build_job: Option<u32>,
}

impl Plan {
    pub fn new(r: &RepoCfg, link: &str) -> Result<Self, String> {
        let issue = if link.trim().is_empty() { None } else {
            Some(tracker::IssueRef { provider: r.provider, repo: r.repo.clone(), key: tracker::key(r, link)?, url: if link.contains("://") { link.trim().into() } else { String::new() } })
        };
        let identity = format!("{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|e| e.to_string())?.as_nanos());
        Ok(Self { provider: r.provider, repo: r.repo.clone(), host: r.host.clone(), issue, identity, revision: 0, draft: String::new(), review: String::new(), reviewed: None, accepted: None, published: None, base: None, remote_context: String::new(), pending: None, action: Action::Draft, ready: false, build_job: None })
    }

    pub fn invalidate(&mut self) { self.reviewed = None; }
    /// Planned for this configured repository: same forge, name and host.
    pub fn is_for(&self, r: &RepoCfg) -> bool {
        self.provider == r.provider && self.repo == r.repo && self.host == r.host
    }
    pub fn ready_for_intake(&self) -> bool {
        self.ready && self.pending.is_none() && self.reviewed == Some(self.revision) && self.accepted == Some(self.revision) && self.published == Some(self.revision)
    }
    pub fn accept(&mut self, queued: bool) -> Result<(), String> {
        if self.pending.is_some() { self.action = Action::Publish; return Ok(()); }
        if queued { return Err("Send queued feedback through review before accepting.".into()); }
        if self.reviewed != Some(self.revision) || self.draft.trim().is_empty() { return Err("This revision needs a passing plan review before acceptance.".into()); }
        if self.published == Some(self.revision) { return Err("This revision is already published. Choose Ready for build or Refine.".into()); }
        self.accepted = Some(self.revision);
        let token = format!("{}-{}", self.identity, self.revision);
        let plan = self.draft.trim_end().strip_suffix("PLAN: READY").unwrap_or(&self.draft).trim_end();
        self.pending = Some(Publication { revision: self.revision, section: tracker::managed(plan, &token, self.revision), token, expected: self.base.clone(), create_attempted: false, comment_attempted: false, summary: clip(plan.lines().find(|line| !line.trim().is_empty()).unwrap_or("Updated implementation plan"), 240) });
        self.action = Action::Publish;
        Ok(())
    }
    pub fn request_ready(&mut self, queued: bool) -> Result<(), String> {
        if queued || self.pending.is_some() || self.published != Some(self.revision) || self.accepted != Some(self.revision) || self.reviewed != Some(self.revision) {
            return Err("Ready requires the current reviewed, accepted and published revision, with no queued feedback.".into());
        }
        self.action = Action::Ready;
        Ok(())
    }
    pub fn status(&self, intake: bool) -> String {
        let target = self.issue.as_ref().map(|i| if i.url.is_empty() { format!("{}#{}", i.repo, i.key) } else { i.url.clone() }).unwrap_or_else(|| format!("{} · new issue (local draft)", self.repo));
        let state = if let Some(id) = self.build_job { format!("Build task #{id}") }
            else if self.pending.is_some() { "Publication pending — retry to reconcile".into() }
            else if self.ready { if intake { "Ready for build".into() } else { "Ready — intake paused".into() } }
            else if self.published == Some(self.revision) && self.reviewed == Some(self.revision) { "Published — choose Ready for build".into() }
            else if self.reviewed == Some(self.revision) { "Reviewed — accept and publish or refine".into() }
            else { "Planning — needs review".into() };
        format!("{target} · r{} · {state}", self.revision)
    }
}

pub fn repo(hub: &Hub, p: &Plan) -> Result<RepoCfg, String> {
    hub.cfg.repos.iter().find(|r| p.is_for(r)).cloned().ok_or_else(|| "The plan's repository is no longer configured.".into())
}
fn plan(hub: &Hub, id: u32) -> Result<Plan, String> { hub.job(id, |j| j.issue_plan.clone()).flatten().ok_or_else(|| "Issue plan missing".into()) }
fn edit(hub: &Hub, id: u32, f: impl FnOnce(&mut Plan)) { hub.job(id, |j| { if let Some(p) = &mut j.issue_plan { f(p); } }); }

fn durable(hub: &Hub, id: u32) -> Result<(), String> {
    let stopping = hub.state.lock().unwrap().stopping;
    if stopping || hub.job(id, |j| j.cancel.is_set()).unwrap_or(true) { return Err("Issue operation interrupted; retry from F6 to reconcile.".into()); }
    hub.persist(id)
}

async fn guard(hub: &Hub, r: &RepoCfg, remote: &tracker::RemoteIssue) -> Result<(), String> {
    if !remote.open { return Err("This issue is closed. Reopen it before refining or publishing.".into()); }
    let n = tracker::number(&remote.reference.key)?;
    if tracker::has_label(r, &remote.labels, &r.working_label) || tracker::has_label(r, &remote.labels, &r.done_label) || hub.state.lock().unwrap().jobs.iter().any(|j| !j.status.finished() && matches!(&j.source, Source::Issue { repo_idx, repo, number, .. } if repo == &r.repo && *number == n && hub.cfg.repos.get(*repo_idx).is_some_and(|configured| configured.provider == r.provider && configured.host == r.host))) {
        return Err("This issue is already claimed, building or awaiting delivery.".into());
    }
    let comments = tracker::history(r, &remote.reference.key).await?;
    if !crate::team::live(&crate::team::read_claims(&comments), crate::team::now(), (hub.cfg.team.stale_mins * 60) as i64).is_empty() {
        return Err("A teammate holds an active claim on this issue.".into());
    }
    Ok(())
}

/// Refresh before each planning turn and keep refinement out of issue intake.
pub async fn prepare(hub: &Hub, id: u32) -> Result<(), String> {
    let p = plan(hub, id)?;
    if p.pending.is_some() { return Err("Reconcile the pending publication with Accept / retry before refining.".into()); }
    let r = repo(hub, &p)?;
    // One plan per issue: intake waits while any plan for the issue is unready, so a second would hold
    // it up for good.
    if let Some(i) = &p.issue {
        if let Some(other) = hub.state.lock().unwrap().jobs.iter().find(|j| j.id != id && j.issue_plan.as_ref().is_some_and(|o| o.is_for(&r) && o.issue.as_ref().is_some_and(|oi| oi.key == i.key))).map(|j| j.id) {
            return Err(format!("Task #{other} already plans this issue; continue there."));
        }
    }
    if let Some(i) = &p.issue {
        let remote = tracker::read_ref(&r, i).await?;
        guard(hub, &r, &remote).await?;
        // The pending Draft action and link are durable before touching the intake label.
        durable(hub, id)?;
        if tracker::has_label(&r, &remote.labels, &r.label) { tracker::readiness(&r, &i.key, false).await?; }
        let remote = tracker::read(&r, &i.key).await?;
        guard(hub, &r, &remote).await?;
        let base = tracker::section(&remote.body)?;
        edit(hub, id, |p| { p.issue = Some(remote.reference); p.base = base; p.remote_context = format!("{}\n\n{}", remote.title, remote.body); p.ready = false; p.build_job = None; });
    }
    edit(hub, id, |p| { p.revision += 1; p.reviewed = None; p.review.clear(); });
    hub.persist(id)
}

pub async fn publish(hub: &Hub, id: u32) -> Result<String, String> {
    let p = plan(hub, id)?;
    let r = repo(hub, &p)?;
    let op = p.pending.clone().ok_or("No accepted publication to retry")?;
    durable(hub, id)?;
    let remote = match p.issue {
        Some(i) => tracker::read_ref(&r, &i).await?,
        None => {
            if op.create_attempted {
                tracker::find_created(&r, &format!("<!-- forkdeck:revision {} -->", op.token)).await?
                    .ok_or("The previous create may still be in flight. Retry reconciliation later; no second issue was created.")?
            } else {
                // The forge answers now, so a failure after this is one the create may have survived,
                // and one before it (not logged in, CLI missing) never sent anything.
                tracker::reachable(&r).await?;
                edit(hub, id, |p| p.pending.as_mut().unwrap().create_attempted = true);
                if let Err(e) = durable(hub, id) {
                    edit(hub, id, |p| p.pending.as_mut().unwrap().create_attempted = false);
                    return Err(e);
                }
                let title = hub.job(id, |j| j.title.clone()).unwrap_or_else(|| "Implementation plan".into());
                match tracker::create(&r, &title, &op.section).await {
                    Ok(issue) => issue,
                    // Refused outright: nothing was created, so the next retry may create it.
                    Err(e) if tracker::refused(&e) => {
                        edit(hub, id, |p| p.pending.as_mut().unwrap().create_attempted = false);
                        hub.persist(id)?;
                        return Err(e);
                    }
                    Err(e) => return Err(e),
                }
            }
        }
    };
    edit(hub, id, |p| p.issue = Some(remote.reference.clone()));
    durable(hub, id)?;
    guard(hub, &r, &remote).await?;
    if tracker::has_label(&r, &remote.labels, &r.label) { tracker::readiness(&r, &remote.reference.key, false).await?; }
    let remote = tracker::read(&r, &remote.reference.key).await?;
    guard(hub, &r, &remote).await?;
    let current = tracker::section(&remote.body)?;
    if current.as_ref() != Some(&op.section) {
        let body = match tracker::replace(&remote.body, &op.expected, &op.section) {
            Ok(body) => body,
            Err(e) => {
                // No create or comment can be duplicated by starting a new revision after this conflict.
                edit(hub, id, |p| { p.pending = None; p.reviewed = None; p.action = Action::Draft; p.ready = false; });
                hub.persist(id)?;
                return Err(e);
            }
        };
        durable(hub, id)?;
        tracker::update(&r, &remote.reference.key, &body).await?;
    }
    let marker = format!("<!-- forkdeck:history {} -->", op.token);
    let comments = tracker::history(&r, &remote.reference.key).await?;
    if !comments.iter().any(|c| c.body.contains(&marker)) {
        if op.comment_attempted { return Err("The previous history comment may still be in flight. Retry reconciliation later; no duplicate was posted.".into()); }
        tracker::reachable(&r).await?;
        edit(hub, id, |p| p.pending.as_mut().unwrap().comment_attempted = true);
        if let Err(e) = durable(hub, id) {
            edit(hub, id, |p| p.pending.as_mut().unwrap().comment_attempted = false);
            return Err(e);
        }
        if let Err(e) = tracker::comment(&r, &remote.reference.key, &format!("{marker}\nPlan revision {} accepted after agent review. {}\n\nThe full plan is in the issue description. Build readiness requires a separate action.", op.revision, op.summary)).await {
            // Refused outright: nothing was posted, so the next retry may post it.
            if tracker::refused(&e) {
                edit(hub, id, |p| p.pending.as_mut().unwrap().comment_attempted = false);
                hub.persist(id)?;
            }
            return Err(e);
        }
    }
    // A managed-section edit during publication cannot be mistaken for a successful sync.
    let latest = tracker::read(&r, &remote.reference.key).await?;
    if tracker::section(&latest.body)?.as_ref() != Some(&op.section) { return Err("Issue plan changed during publication. Retry to reconcile before refining.".into()); }
    edit(hub, id, |p| { p.published = Some(op.revision); p.base = Some(op.section); p.pending = None; p.action = Action::Draft; p.ready = false; });
    durable(hub, id)?;
    Ok("Plan published. F6: review the issue or choose Ready for build.".into())
}

pub async fn ready(hub: &Hub, id: u32) -> Result<String, String> {
    let p = plan(hub, id)?;
    let r = repo(hub, &p)?;
    let issue = p.issue.ok_or("Publish this plan first")?;
    let remote = tracker::read_ref(&r, &issue).await?;
    guard(hub, &r, &remote).await?;
    if tracker::section(&remote.body)? != p.base { return Err("Issue plan changed remotely. Refine and publish it again before Ready.".into()); }
    // Recheck after I/O: queued feedback invalidates a readiness request.
    hub.job(id, |j| j.issue_plan.as_mut().unwrap().request_ready(!j.inbox.is_empty())).ok_or("Task missing")??;
    durable(hub, id)?;
    if !tracker::has_label(&r, &remote.labels, &r.label) { tracker::readiness(&r, &issue.key, true).await?; }
    edit(hub, id, |p| { p.ready = true; p.action = Action::Draft; });
    hub.persist(id)?;
    Ok(if hub.state.lock().unwrap().automation { "Ready for build — waiting for issue intake." } else { "Ready — intake paused. Ctrl-A enables intake when you choose." }.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn acceptance_and_readiness_are_revision_bound() {
        let cfg: crate::config::Config = toml::from_str("[[repos]]\nprovider='github'\nrepo='a/b'\npath='/tmp'\n").unwrap();
        let mut p = Plan::new(&cfg.repos[0], "").unwrap();
        p.revision = 1; p.draft = "Do the work".into();
        assert!(p.accept(false).is_err());
        p.reviewed = Some(1);
        assert!(p.accept(true).is_err());
        p.accept(false).unwrap();
        assert!(p.request_ready(false).is_err());
        p.pending = None; p.published = Some(1);
        p.request_ready(false).unwrap();
        assert!(!p.ready_for_intake());
        p.ready = true;
        assert!(p.ready_for_intake());
        p.invalidate();
        assert!(!p.ready_for_intake());
        assert!(p.request_ready(false).is_err());
        p.revision += 1;
        assert!(p.accept(false).is_err());
    }

    #[test]
    fn pending_publication_can_reconcile_before_queued_feedback() {
        let cfg: crate::config::Config = toml::from_str("[[repos]]\nprovider='github'\nrepo='a/b'\npath='/tmp'\n").unwrap();
        let mut p = Plan::new(&cfg.repos[0], "").unwrap();
        p.revision = 1; p.draft = "A reviewed plan\nPLAN: READY".into(); p.reviewed = Some(1);
        p.accept(false).unwrap();
        let token = p.pending.as_ref().unwrap().token.clone();
        p.invalidate();
        p.accept(true).unwrap();
        assert_eq!(p.pending.as_ref().unwrap().token, token);
        assert!(!p.pending.as_ref().unwrap().section.contains("PLAN: READY"));
        assert!(p.request_ready(true).is_err());
    }

    #[test]
    fn linking_keeps_the_existing_conversation_and_agent() {
        let cfg: crate::config::Config = toml::from_str("[[repos]]\nprovider='gitlab'\nrepo='a/b'\npath='/tmp'\n").unwrap();
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let plan = Plan::new(&cfg.repos[0], "42").unwrap();
        let hub = Hub::new(cfg, rt.handle().clone());
        // Jobs stay queued: this is about the conversation, not about running agents.
        let _slots = rt.block_on(hub.sem.clone().acquire_many_owned(hub.cfg.automation.max_parallel as u32)).unwrap();
        let forced = Some(AgentRef::Kind(AgentKind::Codex));
        let id = rt.block_on(hub.submit("Existing task".into(), "Original request".into(), Source::Manual, ".".into(), forced.clone(), Planning::Never)).unwrap();
        hub.job(id, |j| { j.status = JobStatus::Done; j.turns[0].answer = "A local planning answer".into(); });
        rt.block_on(hub.link_plan(id, plan, "Keep the existing decisions".into())).unwrap();
        let st = hub.state.lock().unwrap();
        assert_eq!(st.jobs.len(), 1);
        let j = &st.jobs[0];
        assert_eq!(j.id, id);
        assert_eq!(j.forced, forced);
        assert_eq!(j.turns[0].ask, "Original request");
        assert_eq!(j.turns[0].answer, "A local planning answer");
        assert!(j.turns[1].ask.contains("Keep the existing decisions"));
        assert_eq!(j.workdir, std::path::PathBuf::from("/tmp"));
        assert_eq!(j.issue_plan.as_ref().unwrap().issue.as_ref().unwrap().key, "42");
    }
}
