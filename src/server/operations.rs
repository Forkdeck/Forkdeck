//! Read-only operational projections. Missing observations are deliberately nullable.
use crate::{orchestrator::Hub, state::{Source, AgentKind}};
use serde_json::{json, Value};
use std::time::Instant;

fn unix(at: Instant) -> i64 { crate::journal::now() - at.elapsed().as_secs() as i64 }

pub fn repos(hub: &Hub) -> Value {
    let plans = super::view::plans(hub);
    let st = hub.state.lock().unwrap();
    json!(hub.cfg.repos.iter().enumerate().map(|(i, r)| {
        let observed = st.repos.get(i);
        json!({"repo":r.repo,"host":r.host,"provider":r.provider,"base":r.base,
            "intake_labels":[r.label,r.pr_label],"automation":st.automation,
            "last_poll":observed.and_then(|s| s.last_poll).map(unix),
            "last_error":observed.and_then(|s| s.last_error.as_ref()),
            "hold":observed.and_then(|s| s.hold.as_ref()),"allowed_authors":r.allowed_authors,
            "limits":{"repo_jobs_per_day":r.jobs_per_day,"jobs_per_day":hub.cfg.limits.jobs_per_day,
                "tokens_per_day":hub.cfg.limits.tokens_per_day,"parallel":hub.cfg.automation.max_parallel,"job_mins":hub.cfg.limits.job_mins,"pause_after_failures":hub.cfg.limits.pause_after_failures,"pause_mins":hub.cfg.limits.pause_mins},
            "claims":st.team.rows.iter().filter(|c| c.repo == r.repo).collect::<Vec<_>>(),
            "claims_observed":st.team.at.map(unix),"claims_error":st.team.error,
            "plans":plans.iter().filter(|p| p["plan"]["repo"] == r.repo && p["plan"]["host"].as_str().unwrap_or("") == r.host).collect::<Vec<_>>()})
    }).collect::<Vec<_>>())
}

pub fn deploys(hub: &Hub) -> Value {
    let saved = hub.read_deployments();
    let st = hub.state.lock().unwrap();
    json!({"error":saved.as_ref().err(),"repositories":hub.cfg.repos.iter().enumerate().filter_map(|(i,r)| {
        let d = r.deploy.as_ref()?;
        let state = saved.as_ref().ok().and_then(|s| s.get(&crate::cicd::saved_key(r,d)));
        let jobs = st.jobs.iter().filter(|j| matches!(&j.source, Source::Deploy { repo_idx,.. } if *repo_idx == i)).map(|j| {
            let stage = |s: &crate::state::Stage| json!({"step":s.label,"state":format!("{:?}",s.state),"progress":s.last,"lines":s.tail.iter().enumerate().map(|(i,line)|json!({"time":s.tail_times.get(i),"line":line})).collect::<Vec<_>>()});
            json!({"id":j.id,"revision":match &j.source { Source::Deploy{sha,..} => sha.as_str(), _ => "" },"status":format!("{:?}",j.status),"started":unix(j.created),"finished":j.finished.map(unix),"health":if j.worker.label == "deploy" && j.worker.state == crate::state::StageState::Ok && j.status == crate::state::JobStatus::Done {Some(true)} else {None},"outcome":j.outcome,"pipeline":stage(&j.worker),"rollout":stage(&j.reviewer),"steps":j.extra.iter().map(stage).collect::<Vec<_>>()})
        }).collect::<Vec<_>>();
        Some(json!({"repo":r.repo,"provider":r.provider,"host":r.host,"environment":d.environment,"current_revision":state.filter(|s|s.in_progress.is_none() && s.rolling_back.is_none() && s.seen == s.last_good).map(|s|&s.last_good).filter(|s|!s.is_empty()),"rollout_revision":state.and_then(|s|s.in_progress.as_ref()),"last_attempted_revision":state.map(|s|&s.seen).filter(|s|!s.is_empty()),"last_good_revision":state.map(|s|&s.last_good).filter(|s|!s.is_empty()),"saved":state,"jobs":jobs}))
    }).collect::<Vec<_>>()})
}

pub fn stats(hub: &Hub, days: u32) -> Value {
    let since = crate::journal::now() - i64::from(days) * crate::journal::DAY;
    let entries = hub.read_journal(since);
    let st = hub.state.lock().unwrap();
    let router = json!({"kind":st.router.kind,"calls":st.router.calls,"memory_hits":st.router.remembered,"fallbacks":st.router.fallbacks,"last_ms":st.router.last_ms,"healthy":st.router.healthy});
    drop(st);
    match entries {
        Err(e) => json!({"error":e,"days":days,"router":router}),
        Ok(entries) => {
            let summary = crate::journal::summarise(&entries,days);
            let metrics = hub.read_metrics(since,&entries);
            let prs: Vec<_> = entries.iter().filter(|e| e.kind == "issue" && !e.pr.is_empty()).collect();
            let mut times = prs.iter().map(|e|e.secs).collect::<Vec<_>>();
            json!({"days":days,"summary":summary,"metrics":metrics.as_ref().ok(),"metrics_error":metrics.as_ref().err(),"router":router,
                "pull_requests":{"count":prs.len(),"issue_jobs":summary.issues.jobs,"pr_rate":if summary.issues.jobs > 0 {Some(prs.len() as f64 / f64::from(summary.issues.jobs))} else {None},"first_pass_rate":if prs.is_empty() {None} else {Some(prs.iter().filter(|e|e.rounds == 0).count() as f64 / prs.len() as f64)},"first_pass":prs.iter().filter(|e|e.rounds == 0).count(),"median_secs":crate::metrics::percentile(&mut times,50),"p90_secs":crate::metrics::percentile(&mut times,90)}})
        }
    }
}

/// Never expose arbitrary tool arguments, environment, headers, or URL credentials in settings.
fn redact(v: &mut Value) {
    match v {
        Value::Object(m) => for (key, v) in m {
            let k = key.to_ascii_lowercase();
            if ["env","headers","args","write_args","review_args","command","cmd","token","access_token","refresh_token","checks","verify","url","base_url","endpoint","listen","agent_domains","mirrors"].contains(&k.as_str()) || ["secret","password","api_key","credential","authorization"].iter().any(|s| k.contains(s)) {
                if !v.is_null() { *v = json!("[redacted]"); }
            } else { redact(v); }
        },
        Value::Array(a) => for v in a { redact(v); },
        _ => {}
    }
}

pub fn settings(hub: &Hub) -> Value {
    let mut cfg = serde_json::to_value(hub.cfg.as_ref()).unwrap();
    redact(&mut cfg);
    cfg
}

fn safe_host(s: &str) -> Option<String> {
    reqwest::Url::parse(&format!("https://{}",s.trim_start_matches("https://").trim_start_matches("http://"))).ok().and_then(|u|u.host_str().map(str::to_owned))
}

pub async fn doctor(hub: &Hub) -> Value {
    let mut checks = vec![];
    let mut add = |name: &str,status: &str,detail: Value| checks.push(json!({"name":name,"status":status,"detail":detail}));
    let path = hub.config_path.lock().unwrap().clone();
    add("configuration","ok",json!({"defaults":path.is_none(),"path":path}));
    {
        let st = hub.state.lock().unwrap();
        for k in AgentKind::ALL {
            let cfg = hub.cfg.agents.get(k);
            add(&format!("{} models", k.name()),"ok",json!({"models":cfg.models,"efforts":cfg.efforts}));
            add(k.name(),if st.has(k) { "ok" } else { "failure" },json!({"version":st.cli_versions[crate::journal::slot(k)],"available":st.agent_ready(k)}));
        }
        add("copilot access",if st.copilot_usable() {"ok"} else {"warning"},json!({"seat":st.copilot.seat,"error":st.copilot.error}));
        add("decision endpoint",match st.router.healthy {Some(true)=>"ok",Some(false)=>"failure",None if hub.cfg.router.kind == "rules"=>"ok",None=>"warning"},json!({"kind":st.router.kind,"healthy":st.router.healthy,"configured":hub.cfg.router.kind == "jev" && !hub.cfg.router.jev.base_url.is_empty(),"origin":reqwest::Url::parse(&hub.cfg.router.jev.base_url).ok().map(|u|u.origin().ascii_serialization()),"last_ms":st.router.last_ms,"observation":"Latest daemon routing observation; no model call is made by this page"}));
    }
    add("sandbox",if crate::agents::claude_sandboxed(&hub.cfg) {"ok"} else {"warning"},json!({"enabled":crate::agents::claude_sandboxed(&hub.cfg)}));
    let n = &hub.cfg.agents.network;
    add("network","ok",json!({"public_registries":n.public_registries,"mirrors_configured":n.mirrors.iter().map(|s|safe_host(s)).collect::<Vec<_>>(),"detect_mirrors":n.detect_mirrors,"detected_mirrors":if n.detect_mirrors {crate::buildcache::machine_mirrors().iter().map(|s|safe_host(s)).collect::<Vec<_>>()} else {vec![]}}));
    add("remote","ok",json!({"enabled":hub.cfg.remote.enabled}));
    for r in hub.cfg.repos.iter().filter(|_| !hub.cfg.demo) {
        let git = crate::issues::sh("git", &["rev-parse","--git-dir"],Some(&r.path)).await.is_ok();
        let forge = crate::issues::sh(if r.provider == crate::state::Provider::Github {"gh"} else {"glab"}, &["--version"],None).await.is_ok();
        add(&r.repo,if git && forge {"ok"} else {"failure"},json!({"clone":git,"forge_cli":forge}));
    }
    if hub.cfg.repos.is_empty() { add("repositories","warning",json!("No configured repositories")); }
    for (name,cfg) in hub.cfg.mcp.iter().filter(|_| !hub.cfg.demo) {
        let result = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let mut session = crate::mcp::Session::open(name,cfg).await?;
            let result = session.tools().await;
            session.close().await;
            result.map(|t|t.len())
        }).await;
        add(&format!("MCP {name}"),if matches!(result,Ok(Ok(_))) {"ok"} else {"failure"},json!({"tools":result.ok().and_then(Result::ok)}));
    }
    if hub.cfg.demo {
        for r in &hub.cfg.repos { add(&r.repo,"warning",json!({"clone":null,"forge_cli":null,"note":"Not probed in demo mode"})); }
        for name in hub.cfg.mcp.keys() { add(&format!("MCP {name}"),"warning",json!({"tools":null,"note":"Not probed in demo mode"})); }
        add("demo","warning",json!("Seeded observations; external diagnostics were not executed"));
    }
    json!({"checks":checks,"settings":settings(hub)})
}

#[cfg(test)]
mod tests {
    use super::*;
    fn hub() -> Hub {
        let mut cfg = crate::config::Config { demo: true, ..Default::default() };
        crate::server::seed::configure(&mut cfg);
        let hub = Hub::new(cfg,tokio::runtime::Handle::current());
        crate::server::seed::seed_demo(&hub).unwrap();
        hub
    }
    #[tokio::test]
    async fn repository_projection_preserves_poll_hold_claims_and_plans() {
        let hub = hub();
        let v = repos(&hub);
        assert_eq!(v[0]["base"],"main");
        assert!(v[0]["last_poll"].is_i64());
        assert!(v[0]["hold"].as_str().unwrap().contains("recovery probe"));
        assert_eq!(v[0]["claims"][0]["who"],"alex");
        assert_eq!(v[0]["plans"][0]["plan"]["revision"],3);
        let mut st = hub.state.lock().unwrap();
        st.repos[0].last_poll = None;
        st.repos[0].hold = None;
        st.automation = false;
        drop(st);
        let v = repos(&hub);
        assert!(v[0]["last_poll"].is_null());
        assert!(v[0]["hold"].is_null());
        assert_eq!(v[0]["automation"],false);
    }
    #[tokio::test]
    async fn deployment_projection_keeps_revisions_steps_and_failure_times() {
        let v = deploys(&hub());
        let d = &v["repositories"][0];
        assert_eq!(d["environment"],"staging");
        assert_eq!(d["last_good_revision"],"a06de31");
        assert!(d["current_revision"].is_null());
        assert_eq!(d["rollout_revision"],"c78f0a2");
        assert_eq!(d["jobs"][0]["status"],"Failed");
        assert!(d["jobs"][0]["finished"].is_i64());
        assert!(d["jobs"][1]["finished"].is_null());
        assert_eq!(d["jobs"][1]["rollout"]["state"],"Running");
        assert!(d["jobs"][0]["outcome"].as_str().unwrap().contains("rolled back"));
    }
    #[tokio::test]
    async fn stats_reuse_native_reports_and_keep_missing_usage_nullable() {
        let hub = hub();
        assert!(hub.state.lock().unwrap().unrecorded.is_empty(), "read-only fixtures must never enter the pending-write journal queue");
        let v = stats(&hub,7);
        assert_eq!(v["days"],7);
        assert!(v["summary"]["tasks"]["jobs"].as_u64().unwrap()>0);
        assert!(v["metrics"]["groups"][0]["attempts"].as_u64().unwrap()>0);
        assert_eq!(v["metrics"]["groups"][0]["cost_coverage"],0);
        assert!(v["metrics"]["tasks"][0]["reported_usd_per_accepted_task"].is_null());
        assert!(v["pull_requests"]["median_secs"].is_number());
        assert!(v["metrics"]["tasks"][0]["p90_queue_wait_ms"].is_number());
        hub.read_records.lock().unwrap().as_mut().unwrap().entries.clear();
        let v = stats(&hub,7);
        assert!(v["pull_requests"]["pr_rate"].is_null());
        assert!(v["pull_requests"]["first_pass_rate"].is_null());
    }
    #[tokio::test]
    async fn settings_redact_nested_secrets_and_doctor_is_structured() {
        let mut hub = hub();
        let cfg = std::sync::Arc::make_mut(&mut hub.cfg);
        cfg.router.jev.base_url = "https://person:secret@router.test/v1?token=secret".into();
        cfg.mcp.insert("tools".into(),crate::config::McpServerCfg { env: [("AUTH".into(),"secret".into())].into(),headers:[("Authorization".into(),"secret".into())].into(),args:vec!["secret".into()],..Default::default() });
        let v = settings(&hub);
        assert!(!v.to_string().contains("secret"));
        assert_eq!(v["mcp"]["tools"]["env"],"[redacted]");
        assert!(v["limits"]["tokens_per_day"].is_number());
        let d = doctor(&hub).await;
        assert!(d["checks"].as_array().unwrap().iter().all(|c|["ok","warning","failure"].contains(&c["status"].as_str().unwrap())));
        assert!(!d.to_string().contains("person:"));
        assert!(d["settings"].is_object());
    }
}
