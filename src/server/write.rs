//! Browser write admission. Domain actions stay on Hub; request bodies contain only opaque
//! configured workspace IDs, text and agent choices, never execution configuration.
use super::{auth::Principal, http::App};
use crate::{orchestrator::Hub, state::{AgentKind, Choice, Job, JobStatus, Planning, Source}};
use axum::{body::{to_bytes, Body}, extract::State, http::{header, Request, StatusCode}, response::{IntoResponse, Response}, Json};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::{HashMap, VecDeque}, path::{Path, PathBuf}, time::Duration};

const LIMIT: usize = 32 * 1024;

#[derive(Default)]
pub(super) struct Writes {
    operations: HashMap<String, Operation>,
    audit: VecDeque<Value>,
    order: VecDeque<String>,
}
impl Writes {
    fn audit(&mut self, record: Value) {
        self.audit.push_back(record);
        while self.audit.len() > 200 { self.audit.pop_front(); }
    }
}
#[derive(Clone, Serialize, Deserialize)]
struct Operation { fingerprint: String, status: u16, body: Value }

pub(super) fn digest(value: &impl Serialize) -> String {
    format!("{:x}", Sha256::digest(serde_json::to_vec(value).unwrap()))
}

/// A revision of actionable state, independent of clocks and streaming output.
pub(crate) fn revision(job: &Job) -> String {
    job.action_revision()
}

pub(super) fn workspace(hub: &Hub, id: &str) -> Option<std::path::PathBuf> {
    if id == "workspace" { std::env::current_dir().ok() }
    else { id.strip_prefix("repo-")?.parse::<usize>().ok().and_then(|i| hub.cfg.repos.get(i)).map(|r| r.path.clone()) }
}
pub(super) fn workspaces(hub: &Hub) -> Value {
    let mut choices = vec![json!({"id":"workspace","name":std::env::current_dir().ok().and_then(|p| p.file_name().map(|s| s.to_string_lossy().into_owned())).unwrap_or_default()})];
    choices.extend(hub.cfg.repos.iter().enumerate().map(|(i,r)| json!({"id":format!("repo-{i}"),"name":r.repo})));
    json!(choices)
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Command {
    Submit { text: String, workspace: String, agent: Option<AgentKind>, plan_first: bool },
    Reply { id: u32, text: String, expected_revision: Option<String> },
    Steer { id: u32, text: String, expected_revision: Option<String> },
    Cancel { id: u32 },
    Resume { id: u32 },
    Retry { id: u32 },
    Choose { id: u32, option: Option<usize>, text: Option<String>, expected_revision: String },
    Plan { id: u32, operation: String, text: Option<String>, expected_revision: String },
    LinkPlan { id: u32, workspace: String, issue: Option<String>, text: String, expected_revision: String },
    NewPlan { workspace: String, issue: String, text: String, agent: Option<AgentKind> },
    Automation { on: bool },
    AddRepo { picker: String, repo: String },
}

fn text(value: String) -> Result<String, String> {
    let value = value.trim().to_owned();
    if value.is_empty() { Err("Enter a message".into()) } else { Ok(value) }
}

async fn apply(app: &App, cookie: &str, command: Command) -> Result<Value, (StatusCode, String)> {
    let hub = &app.hub;
    let bad = |e: String| (if e == "stale revision" { StatusCode::CONFLICT } else { StatusCode::BAD_REQUEST }, e);
    let check = |id, expected: &str| -> Result<(), (StatusCode, String)> {
        match hub.job(id, |j| revision(j)) {
            Some(actual) if actual == expected => Ok(()),
            Some(_) => Err((StatusCode::CONFLICT, "This task changed; review the current revision".into())),
            None => Err((StatusCode::NOT_FOUND, "No such task".into())),
        }
    };
    // reply/steer can implicitly choose a planning option in the TUI. Browser callers must
    // still supply the displayed revision; a status transition during admission is also stale.
    let message_revision = |id, expected: Option<String>| -> Result<String, (StatusCode, String)> {
        let (choosing, current) = hub.job(id, |j| (j.status == JobStatus::Choosing, revision(j)))
            .ok_or_else(|| (StatusCode::NOT_FOUND, "No such task".into()))?;
        if choosing && expected.is_none() { return Err((StatusCode::CONFLICT, "A planning choice requires the displayed revision".into())); }
        Ok(expected.unwrap_or(current))
    };
    let id = match command {
        Command::Submit { text: prompt, workspace: key, agent, plan_first } => {
            let prompt = text(prompt).map_err(bad)?;
            let path = workspace(hub, &key).ok_or_else(|| bad("Select a configured workspace".into()))?;
            hub.submit(crate::state::clip(&prompt, 80), prompt, Source::Manual, path, agent.map(crate::state::AgentRef::Kind), if plan_first { Planning::Always } else { Planning::IfRouted }).await.map_err(bad)?
        }
        Command::NewPlan { workspace: key, issue, text: prompt, agent } => {
            if hub.cfg.demo { return Err(bad("Issue planning is disabled in demo mode".into())); }
            let index = key.strip_prefix("repo-").and_then(|s| s.parse::<usize>().ok()).ok_or_else(|| bad("Select a configured repository".into()))?;
            let repo = hub.cfg.repos.get(index).ok_or_else(|| bad("Select a configured repository".into()))?;
            let prompt = text(prompt).map_err(bad)?;
            let plan = crate::planning::Plan::new(repo, &issue).map_err(bad)?;
            hub.submit_with_plan(crate::state::clip(&prompt, 80), prompt, Source::Manual, repo.path.clone(), agent.map(crate::state::AgentRef::Kind), Planning::Never, Some(plan)).await.map_err(bad)?
        }
        Command::Reply { id, text: message, expected_revision } => {
            let status = hub.job(id, |j| j.status).ok_or_else(|| (StatusCode::NOT_FOUND, "No such task".into()))?;
            let message = if message.trim().is_empty() {
                if status == JobStatus::Interrupted {
                    if let Some(expected) = &expected_revision { check(id, expected)?; }
                    hub.resume(id).await.map_err(bad)?;
                    return Ok(json!({"id":id}));
                }
                if !status.finished() { return Err(bad("Enter a message".into())); }
                crate::CARRY_ON.into()
            } else { text(message).map_err(bad)? };
            let revision = message_revision(id, expected_revision)?;
            hub.reply_at(id, message, Some(&revision)).await.map_err(bad)?; id
        }
        Command::Steer { id, text: message, expected_revision } => {
            let revision = message_revision(id, expected_revision)?;
            hub.steer_now_at(id, text(message).map_err(bad)?, Some(&revision)).await.map_err(bad)?; id
        }
        Command::Cancel { id } => {
            if hub.job(id, |_| ()).is_none() { return Err((StatusCode::NOT_FOUND, "No such task".into())); }
            hub.cancel(id); id
        }
        Command::Resume { id } => { hub.resume(id).await.map_err(bad)?; id }
        Command::Retry { id } => {
            if !hub.job(id, |j| j.status.finished() && j.source.is_manual()).unwrap_or(false) { return Err(bad("Only a finished typed task can retry".into())); }
            hub.reply(id, crate::CARRY_ON.into()).await.map_err(bad)?; id
        }
        Command::Choose { id, option, text: own, expected_revision } => {
            check(id, &expected_revision)?;
            let (waiting, options) = hub.job(id, |j| (j.status == JobStatus::Choosing, crate::choices::for_job(j))).unwrap();
            let own = own.map(|s| s.trim().to_owned());
            let choice = match (option, &own) {
                (Some(i), _) if i < options.len() => {
                    if waiting { Choice::Option(i) } else { Choice::Own(format!("For planning, I choose option {}.{}", options[i].label, own.as_ref().filter(|s| !s.is_empty()).map(|s| format!("\n\n{s}")).unwrap_or_default())) }
                }
                (None, Some(s)) => Choice::Own(text(s.clone()).map_err(bad)?),
                _ => return Err(bad("Select an offered option or enter your own response".into())),
            };
            if waiting {
                if let Some(draft) = own.filter(|s| !s.is_empty() && matches!(&choice, Choice::Option(_))) {
                    // As in the TUI, queue the draft before the choice wakes the worker. A reply
                    // while still Choosing would be interpreted as another choice.
                    let (at, queued_revision) = hub.job(id, |j| {
                        j.check_revision(Some(&expected_revision))?;
                        let at = j.inbox.len();
                        j.inbox.push_back(draft.clone());
                        j.worker.push(format!("› {} (queued)", crate::state::clip(&draft, 300)));
                        Ok::<_, String>((at, revision(j)))
                    }).unwrap().map_err(bad)?;
                    if let Err(e) = hub.choose_at(id, choice, Some(&queued_revision)) {
                        hub.job(id, |j| { if j.inbox.get(at) == Some(&draft) { j.inbox.remove(at); } });
                        return Err(bad(e));
                    }
                } else { hub.choose_at(id, choice, Some(&expected_revision)).map_err(bad)?; }
            } else if let Choice::Own(message) = choice { hub.reply_at(id, message, Some(&expected_revision)).await.map_err(bad)?; }
            id
        }
        Command::Plan { id, operation, text: feedback, expected_revision } => {
            check(id, &expected_revision)?;
            if !hub.job(id, |j| j.issue_plan.is_some()).unwrap_or(false) { return Err(bad("Select an issue plan".into())); }
            match operation.as_str() {
                "accept" => hub.plan_action_at(id, crate::planning::Action::Publish, Some(&expected_revision)).await.map_err(bad)?,
                "ready" => hub.plan_action_at(id, crate::planning::Action::Ready, Some(&expected_revision)).await.map_err(bad)?,
                "refine" | "replan" => {
                    if hub.job(id, |j| j.issue_plan.as_ref().is_some_and(|p| p.graph.is_some() && p.creation.as_ref().is_some_and(|c| c.done))).unwrap_or(false) {
                        return Err(bad("Changes to a published task graph are not supported yet".into()));
                    }
                    hub.reply_at(id, text(feedback.unwrap_or_default()).map_err(bad)?, Some(&expected_revision)).await.map_err(bad)?;
                }
                _ => return Err(bad("Unknown plan action".into())),
            }
            id
        }
        Command::LinkPlan { id, workspace: key, issue, text: prompt, expected_revision } => {
            check(id, &expected_revision)?;
            if hub.cfg.demo { return Err(bad("Issue planning is disabled in demo mode".into())); }
            let index = key.strip_prefix("repo-").and_then(|s| s.parse::<usize>().ok()).ok_or_else(|| bad("Select a configured repository".into()))?;
            let repo = hub.cfg.repos.get(index).ok_or_else(|| bad("Select a configured repository".into()))?;
            let prompt = text(prompt).map_err(bad)?;
            let plan = crate::planning::Plan::new(repo, &issue.unwrap_or_default()).map_err(bad)?;
            hub.link_plan(id, plan, prompt).await.map_err(bad)?;
            id
        }
        Command::AddRepo { picker, repo } => return add_repo(app,&picker,cookie,&repo).await,
        Command::Automation { on } => {
            hub.automation(on);
            let mut st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
            if on && st.repos.is_empty() { st.log("issues", crate::NO_REPOS_TO_POLL); }
            return Ok(json!({"automation":on}));
        }
    };
    Ok(json!({"id":id}))
}

fn repo_error(error: impl std::fmt::Display) -> (StatusCode,String) {
    (StatusCode::CONFLICT,format!("Could not update repository configuration: {error}"))
}
fn disk_configured(path: &Path, entry: &crate::config::RepoEntry, host: &str) -> Result<bool,(StatusCode,String)> {
    let cfg = crate::config::load(Some(path)).map_err(repo_error)?.0;
    Ok(cfg.repos.iter().any(|r| r.provider == entry.provider && r.repo == entry.repo && super::forges::repo_host(r).eq_ignore_ascii_case(host)))
}
struct StagedClone(Option<PathBuf>);
impl StagedClone {
    fn path(&self) -> &Path { self.0.as_deref().unwrap() }
    fn installed(&mut self, path: &Path) { self.0 = Some(path.to_path_buf()); }
    fn keep(mut self) { self.0.take(); }
}
impl Drop for StagedClone {
    fn drop(&mut self) { if let Some(path) = &self.0 { let _ = std::fs::remove_dir_all(path); } }
}

async fn add_repo(app: &App, picker: &str, cookie: &str, repo: &str) -> Result<Value,(StatusCode,String)> {
    static ADDS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let missing = || (StatusCode::NOT_FOUND,"This repository is not in an active picker".into());
    let picked = super::forges::picked(app,picker,cookie,repo).ok_or_else(missing)?;
    let cfg = super::forges::config(app,picked.provider).ok_or_else(missing)?;
    if !super::forges::valid_repo(&picked.repo) || !cfg.host.eq_ignore_ascii_case(&picked.host) { return Err(missing()); }
    if app.hub.cfg.demo { return Err((StatusCode::CONFLICT,"Adding repositories is disabled in demo mode".into())); }
    let config_path = app.hub.config_path.lock().unwrap().clone()
        .ok_or_else(|| (StatusCode::CONFLICT,"Forkdeck was started without a config file".into()))?;
    let clone_dir = cfg.clone_dir.clone();
    let host = picked.host;
    let target = clone_dir.join(&host).join(&picked.repo);
    let entry = crate::config::RepoEntry { provider:picked.provider,repo:picked.repo,path:target,base:picked.default_branch };
    let already = super::forges::configured(app,entry.provider,&host,&entry.repo);
    let _serial = ADDS.lock().await;
    let duplicate = || (StatusCode::CONFLICT,"Repository is already configured".into());
    let exists = || (StatusCode::CONFLICT,"Clone path already exists".into());
    let mut clone = tokio::task::spawn_blocking({
        let entry = entry.clone();
        let config_path = config_path.clone();
        let host = host.clone();
        move || {
            if already || disk_configured(&config_path,&entry,&host)? { return Err(duplicate()); }
            if std::fs::symlink_metadata(&entry.path).is_ok() { return Err(exists()); }
            std::fs::create_dir_all(&clone_dir).map_err(repo_error)?;
            let parent = entry.path.parent().unwrap();
            let mut directory = clone_dir.clone();
            for component in parent.strip_prefix(&clone_dir).unwrap().components() {
                directory.push(component);
                match std::fs::symlink_metadata(&directory) {
                    Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => return Err((StatusCode::CONFLICT,"Clone parent must be a directory without symbolic links".into())),
                    Ok(_) => {},
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => std::fs::create_dir(&directory).map_err(repo_error)?,
                    Err(e) => return Err(repo_error(e)),
                }
            }
            let temporary = parent.join(format!(".forkdeck-{}",crate::metrics::id()));
            std::fs::create_dir(&temporary).map_err(repo_error)?;
            Ok(StagedClone(Some(temporary)))
        }
    }).await.map_err(|_| repo_error("Repository preparation interrupted"))??;
    let github = entry.provider == crate::state::Provider::Github;
    let mut command = crate::proc::command(if github { "gh" } else { "glab" });
    command.args(["repo","clone"]).arg(&entry.repo).arg(clone.path())
        .env(if github { "GH_HOST" } else { "GITLAB_HOST" },&host).env("GLAB_NO_PROMPT","1")
        .current_dir(clone.path().parent().unwrap());
    match crate::proc::output_code(command,Duration::from_secs(300)).await {
        Ok((Some(0),_,_)) => {},
        Ok((_,out,err)) => {
            let message = if err.trim().is_empty() { out.trim() } else { err.trim() };
            return Err((StatusCode::BAD_GATEWAY,if message.is_empty() { "Repository clone failed".into() } else { message.into() }));
        }
        Err(e) => return Err((StatusCode::BAD_GATEWAY,e)),
    }
    tokio::task::spawn_blocking(move || {
        if std::fs::symlink_metadata(&entry.path).is_ok() { return Err(exists()); }
        std::fs::rename(clone.path(),&entry.path).map_err(repo_error)?;
        clone.installed(&entry.path);
        if disk_configured(&config_path,&entry,&host)? { return Err(duplicate()); }
        crate::config::append_repo(&config_path,&entry).map_err(repo_error)?;
        clone.keep();
        Ok(json!({"repo":entry.repo,"path":entry.path,"restart_required":true}))
    }).await.map_err(|_| repo_error("Repository append interrupted"))?
}

fn response(status: u16, body: Value) -> Response {
    (StatusCode::from_u16(status).unwrap(), Json(body)).into_response()
}

impl App {
    async fn rejection(&self, writes: &mut Writes, actor: &str, status: u16, message: &str) -> Response {
        let audit = json!({"at":chrono::Utc::now(),"actor":actor,"action":"rejected","status":status});
        let operation = Operation { fingerprint: String::new(), status, body: json!({"error":message}) };
        if self.record(None, &operation, audit.clone()).await.is_err() { return response(503,json!({"error":"Audit storage unavailable"})); }
        writes.audit(audit);
        response(status, operation.body)
    }
    async fn record(&self, key: Option<String>, operation: &Operation, audit: Value) -> Result<(), String> {
        if let Some(history) = self.hub.history.clone() {
            let operation = serde_json::to_string(operation).unwrap();
            let audit = serde_json::to_string(&audit).unwrap();
            tokio::task::spawn_blocking(move || history.web_record(key.as_deref(), &operation, &audit)).await.map_err(|e| e.to_string())??;
        }
        self.hub.log("web", format!("{}: {} ({})", audit["actor"].as_str().unwrap_or("local"), audit["action"].as_str().unwrap_or("invalid"), audit["status"]));
        Ok(())
    }
    pub(super) async fn audit(&self) -> Result<Vec<Value>, String> {
        if let Some(history) = self.hub.history.clone() {
            tokio::task::spawn_blocking(move || history.web_audit()).await.map_err(|e| e.to_string())?
        } else { Ok(self.writes.lock().await.audit.iter().rev().cloned().collect()) }
    }
}

pub(super) async fn action(State(app): State<App>, req: Request<Body>) -> Response {
    let actor = req.extensions().get::<Principal>().map(|p| p.subject.clone()).unwrap_or_else(|| "local".into());
    let headers = req.headers();
    let forge_cookie = super::auth::cookie(headers,super::forges::COOKIE).unwrap_or_default();
    let origin = app.auth.as_ref().map(|auth| auth.origin())
        .or_else(|| headers.get(header::HOST).and_then(|h| h.to_str().ok()).map(|host| format!("http://{host}")));
    let denied = if !super::auth::can_write(req.extensions().get::<Principal>()) {
        Some((403, "Operator or admin role required"))
    } else if headers.get("sec-fetch-site").is_some_and(|h| h == "cross-site") || headers.get(header::ORIGIN).is_some_and(|h| h.to_str().ok() != origin.as_deref()) {
        Some((403, "Cross-origin actions are forbidden"))
    } else if headers.get(header::CONTENT_TYPE).and_then(|h| h.to_str().ok()).is_none_or(|s| !s.split(';').next().unwrap_or_default().trim().eq_ignore_ascii_case("application/json")) {
        Some((415, "Content-Type must be application/json"))
    } else { None };
    let key = headers.get("idempotency-key").and_then(|h| h.to_str().ok()).filter(|s| !s.is_empty() && s.len() <= 128).map(str::to_owned);
    let denied = denied.or_else(|| key.is_none().then_some((400, "Idempotency-Key is required (1–128 characters)")));
    let mut writes = app.writes.lock().await;
    if let Some((status, message)) = denied {
        let audit = json!({"at":chrono::Utc::now(),"actor":actor,"action":"rejected","status":status,"request_id":key});
        let op = Operation { fingerprint: String::new(), status, body: json!({"error":message}) };
        if app.record(None, &op, audit.clone()).await.is_err() { return response(503, json!({"error":"Audit storage unavailable"})); }
        writes.audit(audit);
        return response(status, op.body);
    }
    let key = digest(&json!([std::env::current_dir().ok(),actor,key.unwrap()]));
    let bytes = match to_bytes(req.into_body(), LIMIT).await {
        Ok(bytes) => bytes,
        Err(_) => return app.rejection(&mut writes, &actor, 413, "Request body exceeds 32 KiB").await,
    };
    let fingerprint = digest(&bytes.to_vec());
    let old = if let Some(history) = app.hub.history.clone() {
        let key = key.clone();
        match tokio::task::spawn_blocking(move || history.web_operation(&key)).await {
            Ok(Ok(Some(row))) => match serde_json::from_str::<Operation>(&row) {
                Ok(operation) => Some(operation),
                Err(_) => return response(503, json!({"error":"Invalid stored operation; action not started"})),
            },
            Ok(Ok(None)) => None,
            _ => return response(503, json!({"error":"Operation storage unavailable"})),
        }
    } else { writes.operations.get(&key).cloned() };
    if let Some(old) = old {
        if old.fingerprint != fingerprint { return app.rejection(&mut writes, &actor, 409, "Idempotency-Key was already used for a different request").await; }
        let audit = json!({"at":chrono::Utc::now(),"actor":actor,"action":"replay","request_id":key,"status":old.status});
        if app.record(None, &old, audit.clone()).await.is_err() { return response(503,json!({"error":"Audit storage unavailable"})); }
        writes.audit(audit);
        return response(old.status, old.body);
    }
    let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    let action = value["action"].as_str().filter(|action| matches!(*action,
        "submit" | "reply" | "steer" | "cancel" | "resume" | "retry" | "choose" | "plan" | "link_plan" | "new_plan" | "automation" | "add_repo"
    )).unwrap_or("invalid");
    let audit = |status: &str| json!({"at":chrono::Utc::now(),"actor":actor,"action":action,"target":value["id"],"revision":value["expected_revision"],"request_id":key,"status":status});
    let mut op = Operation { fingerprint, status: 409, body: json!({"error":"Operation admitted; outcome uncertain after interruption. Inspect the task before taking another action."}) };
    if app.record(Some(key.clone()), &op, audit("admitted")).await.is_err() { return response(503,json!({"error":"Could not persist admission; action not started"})); }
    writes.operations.insert(key.clone(), op.clone());
    writes.order.push_back(key.clone());
    let result = match serde_json::from_value(value.clone()) {
        Ok(command) => apply(&app, &forge_cookie, command).await,
        Err(_) => Err((StatusCode::BAD_REQUEST, "Invalid action fields".into())),
    };
    match result { Ok(body) => { op.status = 200; op.body = body; }, Err((status, error)) => { op.status = status.as_u16(); op.body = json!({"error":error}); } }
    if app.record(Some(key.clone()), &op, audit(&op.status.to_string())).await.is_err() { return response(503,json!({"error":"Action outcome could not be saved; inspect the task before retrying"})); }
    writes.operations.insert(key.clone(), op.clone());
    writes.audit(audit(&op.status.to_string()));
    // Bounded demo-only memory; normal servers retain keys for seven days in SQLite.
    while writes.order.len() > 4096 {
        if let Some(old) = writes.order.pop_front() { writes.operations.remove(&old); }
    }
    app.feed.tick(&app.hub);
    response(op.status, op.body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{history::{History, JobRecord}, remote::feed::Feed, state::{Plan, PlanOption, StageState}};
    use std::{sync::Arc, time::Duration};

    fn parity_job(hub: &Hub, id: u32, status: JobStatus) {
        let mut job = JobRecord { id, title:"native task".into(), workdir:std::env::current_dir().unwrap(), ..Default::default() }.job();
        job.status = status;
        if status == JobStatus::Choosing {
            job.plan = Some(Plan { text:"Options".into(), options:vec![PlanOption { title:"Patch".into(),summary:"Small change".into(),recommended:true }], chosen:None });
        }
        hub.state.lock().unwrap().jobs.push(job);
    }

    #[tokio::test]
    async fn parity_empty_replies_resume_carry_on_or_request_a_message() {
        let hub = Hub::new(crate::config::Config { demo:true, ..Default::default() }, tokio::runtime::Handle::current());
        let permit = hub.sem.clone().acquire_many_owned(hub.cfg.automation.max_parallel.max(1) as u32).await.unwrap();
        for (id, status) in [(901, JobStatus::Working), (902, JobStatus::Choosing), (903, JobStatus::Interrupted), (904, JobStatus::Done)] { parity_job(&hub, id, status); }
        for id in [901, 902] {
            let (status, body) = super::super::http::test_post(hub.clone(), json!({"action":"reply","id":id,"text":"  "}), &format!("empty-{id}")).await;
            assert_eq!((status, body["error"].as_str()), (StatusCode::BAD_REQUEST, Some("Enter a message")));
        }
        for id in [903, 904] {
            let (status, body) = super::super::http::test_post(hub.clone(), json!({"action":"reply","id":id,"text":""}), &format!("empty-{id}")).await;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_eq!(body, json!({"id":id}));
            assert_eq!(hub.job(id, |j| j.status).unwrap(), JobStatus::Queued);
        }
        assert_eq!(hub.job(904, |j| j.turns.last().unwrap().ask.clone()).unwrap(), crate::CARRY_ON);
        for id in [903, 904] { hub.cancel(id); }
        drop(permit);
    }

    #[tokio::test]
    async fn parity_choices_keep_the_draft_and_the_revision_guard() {
        let hub = Hub::new(crate::config::Config { demo:true, ..Default::default() }, tokio::runtime::Handle::current());
        let permit = hub.sem.clone().acquire_many_owned(hub.cfg.automation.max_parallel.max(1) as u32).await.unwrap();
        parity_job(&hub, 902, JobStatus::Choosing);
        let body = json!({"action":"choose","id":902,"option":0,"text":"  additional guidance  ","expected_revision":"old"});
        assert_eq!(super::super::http::test_post(hub.clone(), body.clone(), "stale-draft").await.0, StatusCode::CONFLICT);
        assert!(hub.job(902, |j| j.inbox.is_empty()).unwrap());
        let mut body = body;
        body["expected_revision"] = json!(hub.job(902, |j| revision(j)).unwrap());
        let (status, result) = super::super::http::test_post(hub.clone(), body, "choice-draft").await;
        assert_eq!(status, StatusCode::OK, "{result}");
        assert_eq!(hub.job(902, |j| j.plan.as_ref().unwrap().chosen.clone()).unwrap(), Some(Choice::Option(0)));
        assert_eq!(hub.job(902, |j| j.inbox.iter().cloned().collect::<Vec<_>>()).unwrap(), vec!["additional guidance"]);
        parity_job(&hub, 905, JobStatus::Done);
        hub.job(905, |j| j.turns = vec![crate::state::Turn { ask:"Plan it".into(), answer:"Choose an option:\n1. Patch it\n2. Rewrite it".into() }]);
        let label = hub.job(905, |j| crate::choices::for_job(j)[0].label.clone()).unwrap();
        let body = json!({"action":"choose","id":905,"option":0,"text":"extra context","expected_revision":hub.job(905, |j| revision(j)).unwrap()});
        let (status, result) = super::super::http::test_post(hub.clone(), body, "finished-choice-draft").await;
        assert_eq!(status, StatusCode::OK, "{result}");
        assert_eq!(hub.job(905, |j| j.turns.last().unwrap().ask.clone()).unwrap(), format!("For planning, I choose option {label}.\n\nextra context"));
        hub.cancel(905);
        drop(permit);
    }

    #[tokio::test]
    async fn parity_intake_explains_when_no_repositories_can_be_polled() {
        let hub = Hub::new(crate::config::Config { demo:true, ..Default::default() }, tokio::runtime::Handle::current());
        for on in [true, false] {
            assert_eq!(super::super::http::test_post(hub.clone(), json!({"action":"automation","on":on}), if on { "intake-on" } else { "intake-off" }).await.0, StatusCode::OK);
            assert_eq!(hub.state.lock().unwrap().log.iter().filter(|l| l.who == "issues" && l.msg == "no [[repos]] configured, nothing to poll").count(), 1);
        }
    }

    #[tokio::test]
    async fn parity_link_plan_uses_native_linking_revisions_roles_and_replay() {
        let cfg = toml::from_str("[[repos]]\nprovider = \"github\"\nrepo = \"owner/repo\"\npath = \".\"\nbase = \"main\"\n").unwrap();
        let hub = Hub::new(cfg, tokio::runtime::Handle::current());
        let permit = hub.sem.clone().acquire_many_owned(hub.cfg.automation.max_parallel.max(1) as u32).await.unwrap();
        parity_job(&hub, 904, JobStatus::Done);
        let app = App { hub:hub.clone(),feed:Arc::new(Feed::new()),loopback:true,auth:None,writes:Default::default(),forges:Default::default() };
        let (base, server) = add_server(app.clone(), false).await;
        let client = reqwest::Client::new();
        let body = json!({"action":"link_plan","id":904,"workspace":"repo-0","issue":"42","text":"Refine the existing conversation","expected_revision":"old"});
        let post = |base: String, body: Value, key: &'static str| {
            let client = client.clone();
            async move {
                let response = client.post(format!("{base}/api/v1/actions")).header("idempotency-key",key).json(&body).send().await.unwrap();
                (response.status(), response.json::<Value>().await.unwrap())
            }
        };
        assert_eq!(post(base.clone(), body.clone(), "stale-link").await.0, StatusCode::CONFLICT);
        assert!(hub.job(904, |j| j.issue_plan.is_none()).unwrap());
        let mut body = body;
        body["expected_revision"] = json!(hub.job(904, |j| revision(j)).unwrap());
        let (viewer, viewer_server) = add_server(app, true).await;
        assert_eq!(post(viewer, body.clone(), "viewer-link").await.0, StatusCode::FORBIDDEN);
        let (status, result) = post(base.clone(), body.clone(), "link").await;
        assert_eq!(status, StatusCode::OK, "{result}");
        assert_eq!(result, json!({"id":904}));
        assert_eq!(post(base, body, "link").await, (StatusCode::OK, result));
        assert_eq!(hub.job(904, |j| j.issue_plan.as_ref().unwrap().issue.as_ref().unwrap().key.clone()).unwrap(), "42");
        assert!(hub.job(904, |j| j.turns.last().unwrap().ask.starts_with("Refine the existing conversation\n\nRefine this into a complete issue implementation plan")).unwrap());
        assert_eq!(hub.job(904, |j| j.status).unwrap(), JobStatus::Queued);
        hub.cancel(904);
        server.abort();
        viewer_server.abort();
        drop(permit);
    }

    #[tokio::test]
    async fn every_task_action_uses_native_hub_state_and_durable_replay() {
        let cfg = crate::config::Config { demo: true, ..Default::default() };
        let mut hub = Hub::new(cfg, tokio::runtime::Handle::current());
        hub.history = Some(Arc::new(History::in_memory()));
        // Hold admission at the execution semaphore so assertions see real queue state without
        // racing the demo worker. The HTTP path still invokes the same unmodified Hub methods.
        let permit = hub.sem.clone().acquire_many_owned(hub.cfg.automation.max_parallel.max(1) as u32).await.unwrap();
        for (id, status) in [(901, JobStatus::Working), (902, JobStatus::Choosing), (903, JobStatus::Interrupted), (904, JobStatus::Done), (905, JobStatus::Choosing)] {
            let mut job = JobRecord { id, title:"native task".into(), workdir:std::env::current_dir().unwrap(), ..Default::default() }.job();
            job.status = status;
            job.worker.state = StageState::Running;
            if id == 902 || id == 905 { job.plan = Some(Plan { text:"Options".into(), options:vec![PlanOption { title:"Patch".into(),summary:"Small change".into(),recommended:true }], chosen:None }); }
            hub.state.lock().unwrap().jobs.push(job);
        }
        hub.state.lock().unwrap().next_id = 1000;
        let post = |body, key: &str| {
            let hub = hub.clone();
            let key = key.to_owned();
            async move { super::super::http::test_post(hub, body, &key).await }
        };
        let submit = json!({"action":"submit","text":"Build the real task","workspace":"workspace","agent":"codex","plan_first":true});
        let (status, result) = post(submit.clone(), "submit").await;
        assert_eq!(status, StatusCode::OK, "{result}");
        let id = result["id"].as_u64().unwrap() as u32;
        assert!(hub.job(id, |j| j.planning == Planning::Always && j.forced == Some(crate::state::AgentRef::Kind(AgentKind::Codex)) && j.workdir == std::env::current_dir().unwrap()).unwrap());
        assert_eq!(post(submit, "submit").await.1, result);
        assert_eq!(hub.state.lock().unwrap().jobs.iter().filter(|j| j.id == id).count(), 1);
        assert_eq!(post(json!({"action":"automation","on":true}), "submit").await.0, StatusCode::CONFLICT);
        let reply = json!({"action":"reply","id":901,"text":"after this turn"});
        assert_eq!(post(reply.clone(),"reply").await.0,StatusCode::OK);
        assert_eq!(post(reply,"reply").await.0,StatusCode::OK);
        assert_eq!(hub.job(901, |j| j.inbox.len()).unwrap(),1);
        assert_eq!(post(json!({"action":"steer","id":901,"text":"take this now"}),"steer").await.0,StatusCode::OK);
        assert!(hub.job(901, |j| j.steer.is_set() && j.inbox.front().unwrap() == "take this now").unwrap());
        assert_eq!(post(json!({"action":"cancel","id":901}),"cancel").await.0,StatusCode::OK);
        assert!(hub.job(901, |j| j.cancel.is_set()).unwrap());
        for action in ["reply", "steer"] {
            assert_eq!(post(json!({"action":action,"id":902,"text":"1"}), &format!("missing-choice-revision-{action}")).await.0,StatusCode::CONFLICT);
            assert_eq!(post(json!({"action":action,"id":902,"text":"1","expected_revision":"old"}), &format!("stale-choice-revision-{action}")).await.0,StatusCode::CONFLICT);
        }
        let revision = hub.job(902, |j| revision(j)).unwrap();
        assert_eq!(post(json!({"action":"choose","id":902,"option":0,"expected_revision":"old"}),"stale").await.0,StatusCode::CONFLICT);
        assert_eq!(post(json!({"action":"choose","id":902,"option":0,"expected_revision":revision}),"choose").await.0,StatusCode::OK);
        assert_eq!(hub.job(902, |j| j.plan.as_ref().unwrap().chosen.clone()).unwrap(),Some(Choice::Option(0)));
        let own_revision = hub.job(905, |j| j.action_revision()).unwrap();
        assert_eq!(post(json!({"action":"choose","id":905,"text":"My own approach","expected_revision":own_revision}),"own-choice").await.0,StatusCode::OK);
        assert_eq!(hub.job(905, |j| j.plan.as_ref().unwrap().chosen.clone()).unwrap(),Some(Choice::Own("My own approach".into())));
        assert_eq!(post(json!({"action":"resume","id":903}),"resume").await.0,StatusCode::OK);
        assert_eq!(hub.job(903, |j| j.status).unwrap(),JobStatus::Queued);
        assert_eq!(post(json!({"action":"retry","id":904}),"retry").await.0,StatusCode::OK);
        assert_eq!(hub.job(904, |j| j.turns.last().unwrap().ask.clone()).unwrap(),crate::CARRY_ON);
        assert_eq!(post(json!({"action":"automation","on":true}),"intake").await.0,StatusCode::OK);
        assert!(hub.state.lock().unwrap().automation);
        let audit = hub.history.as_ref().unwrap().web_audit().unwrap();
        assert!(audit.iter().any(|r| r["actor"] == "local" && r["action"] == "steer"));
        assert!(!serde_json::to_string(&audit).unwrap().contains("take this now"));
        // Explicitly release queued execution only after all assertions; cancellation stays Hub-owned.
        for id in [id,903,904] { hub.cancel(id); }
        drop(permit);
    }

    #[tokio::test]
    async fn post_admission_rejects_cross_origin_missing_keys_and_untrusted_execution_fields() {
        let hub = Hub::new(crate::config::Config { demo:true,..Default::default() },tokio::runtime::Handle::current());
        let feed = Arc::new(Feed::new());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}",listener.local_addr().unwrap());
        let app = super::super::http::test_router(hub.clone(),feed.clone());
        let task = tokio::spawn(async move { axum::serve(listener,app).await.unwrap(); });
        let client = reqwest::Client::new();
        let url = format!("{base}/api/v1/actions");
        let body = json!({"action":"automation","on":true});
        let request = || client.post(&url).header("idempotency-key","security").json(&body);
        assert_eq!(request().header(header::ORIGIN,"https://evil.example").send().await.unwrap().status(),StatusCode::FORBIDDEN);
        assert_eq!(request().header("sec-fetch-site","cross-site").send().await.unwrap().status(),StatusCode::FORBIDDEN);
        assert_eq!(client.post(&url).json(&body).send().await.unwrap().status(),StatusCode::BAD_REQUEST);
        assert_eq!(client.post(&url).header("idempotency-key","plain").body(body.to_string()).send().await.unwrap().status(),StatusCode::UNSUPPORTED_MEDIA_TYPE);
        assert_eq!(request().header(header::ORIGIN,&base).send().await.unwrap().status(),StatusCode::OK);
        assert_eq!(request().header(header::ORIGIN,&base).send().await.unwrap().status(),StatusCode::OK);
        assert!(hub.state.lock().unwrap().automation);
        assert_eq!(client.post(&url).header("idempotency-key","large").header(header::CONTENT_TYPE,"application/json").body("x".repeat(LIMIT+1)).send().await.unwrap().status(),StatusCode::PAYLOAD_TOO_LARGE);
        let dangerous = json!({"action":"submit","text":"task","workspace":"workspace","agent":null,"plan_first":false,"path":"/tmp","command":"sh","env":{}});
        assert_eq!(client.post(&url).header("idempotency-key","paths").json(&dangerous).send().await.unwrap().status(),StatusCode::BAD_REQUEST);
        assert!(hub.state.lock().unwrap().jobs.is_empty());
        let (_,mut rx) = feed.subscribe(0);
        hub.automation(false);
        feed.tick(&hub);
        let (_,event) = tokio::time::timeout(Duration::from_secs(1),rx.recv()).await.unwrap().unwrap();
        assert!(matches!(event,forkdeck_remote_proto::Event::Automation { on:false }));
        task.abort();
    }
    async fn add_server(app: App, viewer: bool) -> (String,tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}",listener.local_addr().unwrap());
        let mut router = super::super::http::router(app);
        if viewer { router = router.layer(axum::Extension(Principal { subject:"viewer".into(),name:"Viewer".into(),groups:vec![],role:"viewer".into() })); }
        let task = tokio::spawn(async move { axum::serve(listener,router).await.unwrap(); });
        (base,task)
    }
    async fn add_post(client: &reqwest::Client, base: &str, repo: &str, key: &str) -> (StatusCode,Value) {
        let response = client.post(format!("{base}/api/v1/actions")).header("idempotency-key",key).header(header::COOKIE,"forkdeck_forge=browser")
            .json(&json!({"action":"add_repo","picker":"picker","repo":repo})).send().await.unwrap();
        (response.status(),response.json().await.unwrap())
    }

    #[test]
    fn picked_repositories_clone_append_fail_cleanly_and_serialize() {
        crate::subissues::tests::isolated("server::write::tests::picked_repositories_clone_append_fail_cleanly_and_serialize", |root| {
            use std::os::unix::fs::PermissionsExt;
            let script = r#"#!/bin/sh
[ "$1" = repo ] && [ "$2" = clone ] || exit 9
repo=$3
dest=$4
case "${0##*/}" in gh) host=$GH_HOST;; *) host=$GITLAB_HOST;; esac
printf '%s %s\n' "$repo" "$host" >> "$FAKE_FORGE/clone-calls"
mkdir -p "$dest"
echo partial > "$dest/partial"
case "$repo" in
  */fail) echo 'Could not resolve to a Repository' >&2; exit 1;;
  */edited) printf '\nmanual = [' >> "$FORKDECK_ADD_CONFIG";;
esac
git init -q "$dest"
git -C "$dest" remote add origin "https://$host/$repo.git"
"#;
            std::fs::remove_file(root.join("bin/gh")).unwrap();
            for name in ["gh","glab"] {
                let path = root.join("bin").join(name);
                std::fs::write(&path,script).unwrap();
                std::fs::set_permissions(&path,std::fs::Permissions::from_mode(0o755)).unwrap();
            }
            std::env::set_var("FORKDECK_ADD_SECRET","fixture-secret");
            let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
            rt.block_on(async {
                for (i,provider,host) in [(0,"github","github.com"),(1,"github","github.example"),(2,"gitlab","gitlab.com"),(3,"gitlab","gitlab.example")] {
                    let clone_dir = root.join(format!("clones-{i}"));
                    let path = root.join(format!("config-{i}.toml"));
                    let initial = format!("# Preserve these exact bytes.\n[web.forges.{provider}]\nhost = {host:?}\nclient_id = \"fixture\"\nclient_secret_env = \"FORKDECK_ADD_SECRET\"\nredirect_url = \"https://forkdeck.example/forges/{provider}/callback\"\nclone_dir = {:?}\n",clone_dir.to_str().unwrap());
                    std::fs::write(&path,&initial).unwrap();
                    std::env::set_var("FORKDECK_ADD_CONFIG",&path);
                    let cfg = crate::config::load(Some(&path)).unwrap().0;
                    let hub = Hub::new(cfg,rt.handle().clone());
                    *hub.config_path.lock().unwrap() = Some(path.clone());
                    let app = App { hub:hub.clone(),feed:Arc::new(Feed::new()),loopback:true,auth:None,writes:Default::default(),forges:Default::default() };
                    let forge = if provider == "github" { crate::state::Provider::Github } else { crate::state::Provider::Gitlab };
                    let repo = if provider == "github" { "owner/new" } else { "group/sub/project" };
                    super::super::forges::test_picker(&app,"picker","browser",forge,host,&[repo,"owner/fail","owner/exists","owner/edited","owner/concurrent-a","owner/concurrent-b"]);
                    let (base,server) = add_server(app.clone(),false).await;
                    let client = reqwest::Client::new();
                    let (status,result) = add_post(&client,&base,repo,"success").await;
                    let clone = clone_dir.join(host).join(repo);
                    assert_eq!(status,StatusCode::OK,"{result}");
                    assert_eq!(result,json!({"repo":repo,"path":clone,"restart_required":true}));
                    assert!(clone.join(".git").is_dir());
                    let block = format!("\n[[repos]]\nprovider = {provider:?}\nrepo = {repo:?}\npath = {:?}\nbase = \"main\"\n",clone.to_str().unwrap());
                    let saved = std::fs::read_to_string(&path).unwrap();
                    assert_eq!(saved,format!("{initial}{block}"));
                    assert_eq!(add_post(&client,&base,repo,"success").await.1,result,"idempotent replay");
                    assert_eq!(add_post(&client,&base,repo,"duplicate").await.0,StatusCode::CONFLICT);
                    assert_eq!(std::fs::read_to_string(&path).unwrap(),saved);
                    let existing = clone_dir.join(host).join("owner/exists");
                    std::fs::create_dir_all(&existing).unwrap();
                    std::fs::write(existing.join("keep"),"untouched").unwrap();
                    assert_eq!(add_post(&client,&base,"owner/exists","exists").await.0,StatusCode::CONFLICT);
                    assert_eq!(std::fs::read_to_string(existing.join("keep")).unwrap(),"untouched");
                    let (status,result) = add_post(&client,&base,"owner/fail","failure").await;
                    assert_eq!(status,StatusCode::BAD_GATEWAY);
                    assert!(result["error"].as_str().unwrap().contains("Could not resolve to a Repository"));
                    assert_eq!(std::fs::read_to_string(&path).unwrap(),saved);
                    assert!(!clone_dir.join(host).join("owner/fail").exists());
                    assert!(std::fs::read_dir(clone_dir.join(host).join("owner")).unwrap().all(|e| !e.unwrap().file_name().to_string_lossy().contains(".forkdeck-")));
                    let mut other = app.clone();
                    other.writes = Default::default();
                    let (second,second_server) = add_server(other,false).await;
                    let (a,b) = tokio::join!(add_post(&client,&base,"owner/concurrent-a","a"),add_post(&client,&second,"owner/concurrent-b","b"));
                    assert_eq!(a.0,StatusCode::OK,"{}",a.1);
                    assert_eq!(b.0,StatusCode::OK,"{}",b.1);
                    let concurrent = std::fs::read_to_string(&path).unwrap();
                    assert!(concurrent.starts_with(&saved));
                    assert_eq!(concurrent.matches("[[repos]]").count(),3);
                    assert_eq!(crate::config::load(Some(&path)).unwrap().0.repos.len(),3);
                    let (status,_) = add_post(&client,&base,"owner/edited","edited").await;
                    assert!(!status.is_success());
                    assert_eq!(std::fs::read_to_string(&path).unwrap(),format!("{concurrent}\nmanual = ["));
                    assert!(!clone_dir.join(host).join("owner/edited").exists());
                    let calls = std::fs::read_to_string(root.join("forge/clone-calls")).unwrap();
                    assert_eq!(calls.lines().filter(|line| *line == format!("{repo} {host}")).count(),1);
                    assert!(app.audit().await.unwrap().iter().any(|a|a["action"] == "add_repo" && a["status"] == "200"));
                    server.abort();
                    second_server.abort();
                }
            });
        });
    }

    #[tokio::test]
    async fn add_repository_keeps_cookie_role_origin_and_body_guards() {
        let cfg = toml::from_str(r#"
[web.forges.github]
client_id = "fixture"
client_secret_env = "UNUSED_SECRET"
redirect_url = "https://forkdeck.example/forges/github/callback"
"#).unwrap();
        let hub = Hub::new(cfg,tokio::runtime::Handle::current());
        let app = App { hub,feed:Arc::new(Feed::new()),loopback:true,auth:None,writes:Default::default(),forges:Default::default() };
        super::super::forges::test_picker(&app,"picker","browser",crate::state::Provider::Github,"github.com",&["owner/repo"]);
        let (base,server) = add_server(app.clone(),false).await;
        let client = reqwest::Client::new();
        assert_eq!(add_post(&client,&base,"owner/not-listed","absent").await.0,StatusCode::NOT_FOUND);
        let (status,result) = add_post(&client,&base,"owner/repo","no-config").await;
        assert_eq!(status,StatusCode::CONFLICT);
        assert_eq!(result["error"],"Forkdeck was started without a config file");
        let body = json!({"action":"add_repo","picker":"picker","repo":"owner/repo"});
        let post = |key| client.post(format!("{base}/api/v1/actions")).header("idempotency-key",key).json(&body);
        assert_eq!(post("cookie").header(header::COOKIE,"forkdeck_forge=another-browser").send().await.unwrap().status(),StatusCode::NOT_FOUND);
        assert_eq!(post("origin").header(header::ORIGIN,"https://evil.example").send().await.unwrap().status(),StatusCode::FORBIDDEN);
        for field in ["path","url"] {
            let mut dangerous = body.clone();
            dangerous[field] = json!("/untrusted");
            assert_eq!(client.post(format!("{base}/api/v1/actions")).header("idempotency-key",field).json(&dangerous).send().await.unwrap().status(),StatusCode::BAD_REQUEST);
        }
        let (viewer,viewer_server) = add_server(app,true).await;
        assert_eq!(add_post(&client,&viewer,"owner/repo","viewer").await.0,StatusCode::FORBIDDEN);
        server.abort();
        viewer_server.abort();
    }

}
