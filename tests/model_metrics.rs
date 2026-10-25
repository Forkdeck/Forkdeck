//! Real CLI boundary tests using local fake agents and a disposable repository.
use serde_json::{json, Value};
use std::os::unix::fs::PermissionsExt;
use std::{fs, path::PathBuf, process::Command};
struct Fixture {
    root: PathBuf,
    repo: PathBuf,
    config: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "forkdeck-metrics-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        let repo = root.join("repo");
        fs::create_dir_all(&repo).unwrap();
        let agent = root.join("agent");
        fs::write(&agent,r#"#!/bin/sh
[ "$1" = '--version' ] && { echo 'fixture-v1'; exit 0; }
model=baseline
while [ $# -gt 0 ]; do
  case "$1" in --model) model=$2; shift 2;; *) shift;; esac
done
prompt=$(cat)
case "$prompt" in *'Do not modify any file'*) text='VERDICT: PASS';; *) echo ok > result.txt; text='Done';; esac
printf '{"type":"result","model":"%s","is_error":false,"result":"%s","usage":{"input_tokens":10,"cache_read_input_tokens":20,"cache_creation_input_tokens":0,"output_tokens":5}}\n' "$model" "$text"
"#).unwrap();
        fs::set_permissions(&agent, fs::Permissions::from_mode(0o755)).unwrap();
        let config = root.join("config.toml");
        fs::write(
            &config,
            format!(
                r#"
[router]
kind="rules"
default_agent="claude"
[review]
mode="always"
reviewer="same"
checks=["test -f result.txt"]
[knowledge]
enabled=false
[agents.claude]
cmd={:?}
[agents.claude.models]
light="baseline"
standard="baseline"
heavy="baseline"
[agents.claude.efforts]
light="high"
standard="high"
heavy="high"
[agents.codex]
cmd="/nonexistent/forkdeck-fake-codex"
"#,
                agent.to_str().unwrap()
            ),
        )
        .unwrap();
        for args in [
            vec!["init", "-q"],
            vec!["config", "user.name", "Fixture"],
            vec!["config", "user.email", "fixture@example.invalid"],
            vec!["commit", "--allow-empty", "-qm", "base"],
        ] {
            assert!(Command::new("git").args(args).current_dir(&repo).status().unwrap().success());
        }
        Self { root, repo, config }
    }
    // Synthetic evidence is confined to this disposable state; never shipped as qualification.
    fn adaptive(&self) -> (String, String) {
    let evidence = self.root.join("evidence.json");
    let initial = fs::read_to_string(&self.config).unwrap();
    let settings = format!(
        r#"
[adaptive]
mode="shadow"
policy="test-policy"
evidence={:?}
candidates=[{{model="small",effort="high"}}]
repositories=[{:?}]
sources=["task"]
tiers=["standard"]
max_prompt_bytes=4096
min_tasks=30
min_coverage=0.95
quality_floor=0.9
noninferiority=0.05
max_p90_ms=100000
trial_probability=0.1
trial_tasks=10
trial_tokens=100000
reserve_tokens=1000
window_secs=3600
monitor_tasks=30
"#,
        evidence.to_str().unwrap(),
        self.repo.to_str().unwrap()
    );
    fs::write(&self.config, format!("{initial}{settings}")).unwrap();
    let status = self.json(&["adaptive", "--json"]);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    fs::write(&evidence,serde_json::to_vec(&json!({"schema":1,"version":"e1","expires":now+3600,"created":now,"fingerprint":status["fingerprint"],"cli":"claude","cli_version":"fixture-v1","reviewer_version":"fixture-v1","reviewer_actual_model":"baseline","baseline":{"model":"baseline","effort":"high"},"baseline_actual_model":"baseline","candidate":{"model":"small","effort":"high"},"candidate_actual_model":"small","repo":self.repo,"source":"task","tier":"standard","max_prompt_bytes":4096,"protocol":"controlled test fixture","observations":(0..120).map(|n|json!({"task":n.to_string(),"baseline_accepted":true,"candidate_accepted":true,"baseline_tokens":1000,"candidate_tokens":500,"candidate_ms":1000})).collect::<Vec<_>>()})).unwrap()).unwrap();
    assert!(self.run(&["adaptive", "--evaluate", evidence.to_str().unwrap()]).status.success());
        (initial, settings)
    }
    fn trials(&self) {
        let (initial, settings) = self.adaptive();
        assert!(self.run(&["run", "Document the local fixture"]).status.success());
        let path = self.root.join("state/forkdeck/adaptive-v1.json");
        let mut state: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        for count in state["validated"].as_object_mut().unwrap().values_mut() { *count = json!(30); }
        fs::write(path, serde_json::to_vec(&state).unwrap()).unwrap();
        fs::write(&self.config, format!("{initial}{}", settings.replace("mode=\"shadow\"", "mode=\"adaptive\""))).unwrap();
    }
    fn run(&self, args: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_forkdeck"))
            .arg("--config")
            .arg(&self.config)
            .args(args)
            .env("XDG_STATE_HOME", self.root.join("state"))
            .current_dir(&self.repo)
            .output()
            .unwrap()
    }
    fn json(&self, args: &[&str]) -> Value {
        let o = self.run(args);
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        serde_json::from_slice(&o.stdout).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
#[test]
fn attempts_reconcile_worker_review_and_legacy_totals() {
    let f = Fixture::new();
    let o = f.run(&["run", "--agent", "claude", "Document the local fixture"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let report = f.json(&["stats", "--by-model", "--json"]);
    let groups = report["groups"].as_array().unwrap();
    assert_eq!(groups.len(), 2);
    assert!(groups
        .iter()
        .all(|g| g["model"] == "baseline" && g["complete_usage"] == 1 && g["known_input"] == 30 && g["known_output"] == 5));
    let legacy = f.json(&["stats", "--json"]);
    assert_eq!(legacy["claude"]["runs"], 2);
    assert_eq!(legacy["claude"]["tokens_in"], 60);
    assert_eq!(legacy["tasks"]["jobs"], 1);
    assert!(legacy["repos"].as_array().unwrap().is_empty());
    let journal = fs::read_to_string(f.root.join("state/forkdeck/journal.jsonl")).unwrap();
    let entry: Value = serde_json::from_str(journal.lines().next().unwrap()).unwrap();
    assert_eq!(entry["repo"], "");
    let text = String::from_utf8(f.run(&["stats", "--jobs", "1"]).stdout).unwrap();
    assert!(text.lines().any(|line| line.split_whitespace().any(|word| word == "task")));
    assert!(!text.contains(f.repo.to_str().unwrap()));
    let legacy_filtered = f.run(&["stats", "--repo", f.repo.to_str().unwrap(), "--json"]);
    assert!(!legacy_filtered.status.success());
    assert!(String::from_utf8_lossy(&legacy_filtered.stderr).contains("--by-model"));
    let scoped = f.json(&["stats", "--by-model", "--repo", f.repo.to_str().unwrap(), "--json"]);
    assert_eq!(scoped["tasks"].as_array().unwrap().len(), 1);
    assert_eq!(scoped["tasks"][0]["repo"], f.repo.to_str().unwrap());
    assert_eq!(scoped["groups"].as_array().unwrap().len(), 2);
    let filtered = f.json(&["stats", "--by-model", "--role", "reviewer", "--json"]);
    assert_eq!(filtered["groups"].as_array().unwrap().len(), 1);
    let attempts = f.root.join("state/forkdeck/attempts-v1");
    let rows: Vec<Value> =
        fs::read_dir(attempts).unwrap().map(|e| serde_json::from_slice(&fs::read(e.unwrap().path()).unwrap()).unwrap()).collect();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|r| r["status"] == "completed" && r.get("prompt").is_none()));
    assert_eq!(rows[0]["job"], rows[1]["job"]);
}
#[test]
fn isolated_benchmark_cleans_clones_and_never_touches_production_journal() {
    let f = Fixture::new();
    let revision = Command::new("git").args(["rev-parse", "HEAD"]).current_dir(&f.repo).output().unwrap();
    let revision = String::from_utf8(revision.stdout).unwrap().trim().to_string();
    let manifest = f.root.join("benchmark.json");
    fs::write(&manifest,serde_json::to_vec(&json!({"version":"fixture-evidence","fixture":true,"repository":f.repo,"cohort_repo":"","source":"task","tier":"standard","cli":"claude","candidate":{"model":"small","effort":"high"},"repeats":2,"lifetime_secs":3600,"cache_state":"fixture has no cache","tasks":[{"id":"case-1","revision":revision,"prompt":"Document the local fixture"}]})).unwrap()).unwrap();
    let output = f.root.join("benchmark-results");
    let o = f.run(&["benchmark", manifest.to_str().unwrap(), "--output", output.to_str().unwrap()]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let path = PathBuf::from(String::from_utf8(o.stdout).unwrap().trim());
    let evidence: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(evidence["observations"].as_array().unwrap().len(), 2);
    assert_eq!(evidence["baseline_actual_model"], "baseline");
    assert_eq!(evidence["candidate_actual_model"], "small");
    assert_eq!(evidence["fixture"], true);
    for a in fs::read_dir(path.parent().unwrap().join("attempts")).unwrap() {
        let row: Value = serde_json::from_slice(&fs::read(a.unwrap().path()).unwrap()).unwrap();
        assert_eq!(row["cohort"], evidence["cohort"]);
    }
    let o = f.run(&["adaptive", "--evaluate", path.to_str().unwrap()]);
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("fixture"));

    assert!(evidence["observations"]
        .as_array()
        .unwrap()
        .iter()
        .all(|o| o["baseline_accepted"] == true && o["candidate_accepted"] == true && o["candidate_tokens"] == 70));
    assert!(!f.repo.join("result.txt").exists());
    assert!(!f.root.join("state/forkdeck/journal.jsonl").exists());
    assert!(fs::read_dir(path.parent().unwrap()).unwrap().all(|e| !e.unwrap().file_name().to_string_lossy().starts_with("task-")));
}

#[test]
fn benchmark_candidate_failure_retries_baseline_and_counts_both_attempts() {
    for (baseline_fails, rounds) in [(false, 0), (false, 1), (true, 1)] {
        let f = Fixture::new();
        let config = fs::read_to_string(&f.config).unwrap();
        fs::write(&f.config, config.replace("mode=\"always\"", &format!("mode=\"always\"\nmax_fix_rounds={rounds}"))).unwrap();
        let agent = f.root.join("agent");
        let original = fs::read_to_string(&agent).unwrap();
        let behavior = format!(r#"
if test "$model" = small; then
    echo preserved > candidate-work
    printf '{{"type":"result","model":"small","is_error":true,"result":"failure","usage":{{"input_tokens":10,"cache_read_input_tokens":0,"cache_creation_input_tokens":0,"output_tokens":5}}}}\n'
    exit 1
fi
if test -f candidate-work; then
    test "$(cat candidate-work)" = preserved || exit 9
    if test '{baseline_fails}' = true; then
        printf '{{"type":"result","model":"baseline","is_error":true,"result":"failure","usage":{{"input_tokens":10,"cache_read_input_tokens":0,"cache_creation_input_tokens":0,"output_tokens":5}}}}\n'
        exit 1
    fi
fi
"#);
        fs::write(&agent, original.replace("prompt=$(cat)", &format!("prompt=$(cat)\n{behavior}"))).unwrap();
        let revision = Command::new("git").args(["rev-parse", "HEAD"]).current_dir(&f.repo).output().unwrap();
        let revision = String::from_utf8(revision.stdout).unwrap().trim().to_string();
        let manifest = f.root.join("benchmark.json");
        fs::write(&manifest, serde_json::to_vec(&json!({"version":"failure-fixture","fixture":true,"repository":f.repo,"cohort_repo":"","source":"task","tier":"standard","cli":"claude","candidate":{"model":"small","effort":"high"},"repeats":1,"lifetime_secs":3600,"cache_state":"none","tasks":[{"id":"case-1","revision":revision,"prompt":"Document the local fixture"}]})).unwrap()).unwrap();
        let output = f.root.join("results");
        let o = f.run(&["benchmark", manifest.to_str().unwrap(), "--output", output.to_str().unwrap()]);
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        let path = PathBuf::from(String::from_utf8(o.stdout).unwrap().trim());
        let evidence: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        let row = &evidence["observations"][0];
        assert_eq!(row["baseline_accepted"], true);
        assert_eq!(row["candidate_accepted"], !baseline_fails);
        assert_eq!(row["candidate_tokens"], if baseline_fails { 30 } else { 85 });
        assert_eq!(evidence["candidate_actual_model"], "small");
    }
}

#[test]
fn manual_override_shadow_and_operator_disable_keep_baseline() {
    let f = Fixture::new();
    let (initial, settings) = f.adaptive();
    assert!(f.run(&["run", "--agent", "claude", "Document the local fixture"]).status.success());
    let s = f.json(&["adaptive", "--json"]);
    let d = s["state"]["decisions"].as_array().unwrap().last().unwrap();
    assert_eq!(d["reason"], "manual override");
    assert_eq!(d["executed"]["model"], "baseline");
    assert!(d["proposed"].is_null());
    assert!(f.run(&["run", "Document the local fixture"]).status.success());
    let s = f.json(&["adaptive", "--json"]);
    let d = s["state"]["decisions"].as_array().unwrap().last().unwrap();
    assert_eq!(d["proposed"]["model"], "small");
    assert_eq!(d["executed"]["model"], "baseline");
    assert_eq!(d["candidate"], false);
    assert_eq!(d["tokens"], 70);
    assert_eq!(s["state"]["validated"].as_object().unwrap().values().next().unwrap(), 1);
    let o = f.run(&["adaptive", "--disable"]);
    assert!(o.status.success());
    fs::write(&f.config, format!("{initial}{}", settings.replace("mode=\"shadow\"", "mode=\"adaptive\""))).unwrap();
    assert!(f.run(&["run", "Document the local fixture"]).status.success());
    let s = f.json(&["adaptive", "--json"]);
    let d = s["state"]["decisions"].as_array().unwrap().last().unwrap();
    assert_eq!(d["reason"], "disabled by operator");
    assert_eq!(d["candidate"], false);
    assert!(s["production"].as_array().unwrap().is_empty());
}

#[test]
fn worker_preflight_checks_budget_revisions_and_fixture_evidence() {
    let f = Fixture::new();
    let revision = Command::new("git").args(["rev-parse", "HEAD"]).current_dir(&f.repo).output().unwrap();
    let rev = String::from_utf8(revision.stdout).unwrap().trim().to_string();
    let manifest = f.root.join("preflight.json");
    let output = f.root.join("results");
    let mut m = json!({"version":"preflight","repository":f.repo,"cohort_repo":"","source":"task","tier":"standard","cli":"claude","candidate":{"model":"small","effort":"high"},"repeats":1,"lifetime_secs":3600,"cache_state":"fixture","fixture":true,"max_invocations":1,"tasks":[{"id":"case","revision":rev,"prompt":"Document behavior"}]});
    fs::write(&manifest, serde_json::to_vec(&m).unwrap()).unwrap();
    let args = ["benchmark", manifest.to_str().unwrap(), "--output", output.to_str().unwrap(), "--preflight"];
    let o = f.run(&args);
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("max_invocations"));
    m["max_invocations"] = json!(100);
    fs::write(&manifest, serde_json::to_vec(&m).unwrap()).unwrap();
    let o = f.run(&args);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(!output.exists());
    m["tasks"][0]["revision"] = json!("0".repeat(40));
    fs::write(&manifest, serde_json::to_vec(&m).unwrap()).unwrap();
    assert!(!f.run(&args).status.success());
    assert!(!output.exists());
}

#[test]
fn reviewer_benchmarks_report_missed_defects_without_routing_evidence() {
    let f = Fixture::new();
    let revision = Command::new("git").args(["rev-parse", "HEAD"]).current_dir(&f.repo).output().unwrap();
    let rev = String::from_utf8(revision.stdout).unwrap().trim().to_string();
    let manifest = f.root.join("reviewers.json");
    let output = f.root.join("reviews");
    fs::write(&manifest,serde_json::to_vec(&json!({"version":"review-fixture","repository":f.repo,"cli":"claude","baseline":{"model":"baseline","effort":"high"},"candidate":{"model":"small","effort":"high"},"repeats":1,"max_invocations":4,"cache_state":"fixture","fixture":true,"cases":[{"id":"clean","base":rev,"revision":rev,"task":"Document behavior","defect":null},{"id":"bug","base":rev,"revision":rev,"task":"Document behavior","defect":"labeled-bug"}]})).unwrap()).unwrap();
    let o = f.run(&["reviewer-benchmark", manifest.to_str().unwrap(), "--output", output.to_str().unwrap()]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let path = PathBuf::from(String::from_utf8(o.stdout).unwrap().trim());
    let r: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(r["baseline"]["missed_defects"], 1);
    assert_eq!(r["candidate"]["false_positives"], 0);
    assert_eq!(r["baseline"]["complete_usage"], 2);
    assert_eq!(r["baseline"]["known_tokens"], 70);
    assert!(!path.parent().unwrap().join("evidence.json").exists());
    assert!(!f.root.join("state/forkdeck/journal.jsonl").exists());
}

#[test]
fn advertised_invocation_budget_is_enforced_and_recorded() {
    let f = Fixture::new();
    let agent = f.root.join("agent");
    let original = fs::read_to_string(&agent).unwrap();
    fs::write(&agent,original.replace("model=baseline", "[ \"$1\" = '--help' ] && { echo '--max-budget-usd FLOAT'; exit 0; }\ncase \"$*\" in *'--max-budget-usd 0.25'*) ;; *) exit 7;; esac\nmodel=baseline")).unwrap();
    let probe_log = f.root.join("help-probes");
    let script = fs::read_to_string(&agent).unwrap();
    fs::write(&agent, script.replace("echo '--max-budget-usd FLOAT'", &format!("echo probe >> '{}'; echo '--max-budget-usd FLOAT'", probe_log.display()))).unwrap();
    let config = fs::read_to_string(&f.config).unwrap();
    fs::write(&f.config, format!("{config}\n[adaptive]\ninvocation_max_usd=0.25\n")).unwrap();
    let o = f.run(&["run", "--agent", "claude", "Document behavior"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(fs::read_to_string(probe_log).unwrap().lines().count(), 1, "worker and reviewer share the probe");
    for row in fs::read_dir(f.root.join("state/forkdeck/attempts-v1")).unwrap() {
        let r: Value = serde_json::from_slice(&fs::read(row.unwrap().path()).unwrap()).unwrap();
        assert!(r["invocation_limit"].as_str().unwrap().contains("--max-budget-usd=0.25"));
        assert_eq!(r["schema"], 2);
        assert!(r["first_output_ms"].is_number());
    }
}

#[test]
fn candidate_failures_escalate_once_with_retained_work_and_fixed_review() {
    for failure in ["worker", "worker_review", "worker_zero", "check", "review", "limit"] {
        let f = Fixture::new();
        let config = fs::read_to_string(&f.config).unwrap();
        fs::write(&f.config, config.replace("mode=\"always\"", "mode=\"always\"\nmax_fix_rounds=1")).unwrap();
        if failure == "worker_zero" {
            let config = fs::read_to_string(&f.config).unwrap();
            fs::write(&f.config, config.replace("max_fix_rounds=1", "max_fix_rounds=0")).unwrap();
        }
        f.trials();
        let agent = f.root.join("agent");
        let original = fs::read_to_string(&agent).unwrap();
        // Reviewers start in a scratch directory and are told where the checkout is.
        let repo = f.repo.display();
        let behavior = format!(r#"
case "$prompt" in
  *'Do not modify any file'*)
    test "$model" = baseline || exit 9
    cd '{repo}' || exit 7
    if test -f candidate-work && ! test -f repaired && test '{failure}' = review; then text='VERDICT: FAIL'; fi
    if test -f candidate-work && ! test -f reviewed && test '{failure}' = worker_review; then text='VERDICT: FAIL'; touch reviewed; fi
    if test -f candidate-work && test '{failure}' = limit; then text='VERDICT: FAIL'; fi;;
  *)
    if test "$model" = small; then
      echo preserved > candidate-work
      rm -f repaired
      if test '{failure}' = check; then rm -f result.txt; fi
      if test '{failure}' = worker || test '{failure}' = worker_review || test '{failure}' = worker_zero; then
        printf '{{"type":"result","model":"small","is_error":true,"result":"worker failure","usage":{{"input_tokens":10,"cache_read_input_tokens":20,"cache_creation_input_tokens":0,"output_tokens":5}}}}\n'
        exit 1
      fi
    elif test -f candidate-work; then
      test "$(cat candidate-work)" = preserved || exit 8
      echo ok >> repaired
    fi;;
esac
"#);
        let insertion = original.find("printf '{").unwrap();
        fs::write(&agent, format!("{}{}{}", &original[..insertion], behavior, &original[insertion..])).unwrap();
        let mut found = false;
        for _ in 0..200 {
            let o = f.run(&["run", "Document the local fixture"]);
            assert!(o.status.success() || failure == "limit", "{failure}: {}", String::from_utf8_lossy(&o.stderr));
            let status = f.json(&["adaptive", "--json"]);
            let d = status["state"]["decisions"].as_array().unwrap().last().unwrap();
            if d["candidate"] != true { continue; }
            assert_eq!(d["accepted"], failure != "limit", "{failure}: {d}");
            assert_eq!(d["probability"], 0.1);
            let rows: Vec<Value> = fs::read_dir(f.root.join("state/forkdeck/attempts-v1")).unwrap()
                .map(|e| serde_json::from_slice(&fs::read(e.unwrap().path()).unwrap()).unwrap())
                .filter(|a: &Value| a["decision"] == d["id"]).collect();
            let mut workers: Vec<_> = rows.iter().filter(|a| a["role"] == "worker").collect();
            workers.sort_by_key(|a| a["sequence"].as_u64().unwrap());
            assert_eq!(workers.len(), if failure == "worker_review" { 3 } else { 2 }, "{failure}");
            assert_eq!(workers[0]["actual_model"], "small");
            assert_eq!(workers[1]["actual_model"], "baseline");
            assert_eq!(workers[1]["retry"], if failure.starts_with("worker") { 0 } else { 1 });
            assert!(workers[1..].iter().all(|a| a["actual_model"] == "baseline"));
            assert!(rows.iter().filter(|a| a["role"] == "reviewer").all(|a| a["actual_model"] == "baseline"));
            assert_eq!(fs::read_to_string(f.repo.join("candidate-work")).unwrap().trim(), "preserved");
            assert!(f.repo.join("repaired").exists());
            assert_eq!(d["tokens"].as_u64().unwrap(), rows.len() as u64 * 35);
            found = true;
            break;
        }
        assert!(found, "no candidate in 200 independent 10% assignments");
    }
}

#[test]
fn killed_trial_keeps_reservation_and_suspension_across_process_restarts() {
    use std::os::unix::process::CommandExt;
    use std::time::{Duration, Instant};
    let f = Fixture::new();
    f.trials();
    let agent = f.root.join("agent");
    let original = fs::read_to_string(&agent).unwrap();
    fs::write(&agent, original.replace("prompt=$(cat)", "prompt=$(cat)\nif test \"$model\" = small; then touch trial-started; sleep 30; fi")).unwrap();
    let mut found = false;
    for _ in 0..200 {
        let mut child = Command::new(env!("CARGO_BIN_EXE_forkdeck"))
            .arg("--config").arg(&f.config).args(["run", "Document the local fixture"])
            .env("XDG_STATE_HOME", f.root.join("state")).current_dir(&f.repo)
            .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())
            .process_group(0).spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !f.repo.join("trial-started").exists() && child.try_wait().unwrap().is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        if !f.repo.join("trial-started").exists() {
            assert!(child.wait().unwrap().success());
            continue;
        }
        unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL); }
        child.wait().unwrap();
        found = true;
        break;
    }
    assert!(found);
    for _ in 0..2 {
        assert!(f.run(&["run", "Document the local fixture"]).status.success());
        let s = f.json(&["adaptive", "--json"]);
        let rows = s["state"]["decisions"].as_array().unwrap();
        let candidates: Vec<_> = rows.iter().filter(|d| d["candidate"] == true).collect();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0]["completed"], false);
        assert_eq!(candidates[0]["reserved"], 1000);
        assert!(candidates[0]["tokens"].is_null());
        assert_eq!(rows.last().unwrap()["candidate"], false);
        assert!(s["state"]["suspended"].as_object().unwrap().values()
            .any(|v| v.as_str().unwrap().contains("stopped process")));
    }
}

#[test]
fn reports_keep_unknown_denominators_queue_coverage_and_savings_labels() {
    let f = Fixture::new();
    let (initial, settings) = f.adaptive();
    assert!(f.run(&["run", "Document the local fixture"]).status.success());
    let s = f.json(&["adaptive", "--json"]);
    assert!(s["production"].as_array().unwrap().is_empty());
    assert_eq!(s["state"]["decisions"][0]["qualification"]["predicted_savings"], 0.5);
    let text = String::from_utf8(f.run(&["adaptive"]).stdout).unwrap();
    assert!(text.contains("Predicted savings 50.0%"));
    assert!(!text.contains("Observed trial"));
    // A baseline holdout without executed candidates cannot show observed savings.
    fs::write(&f.config, format!("{initial}{}", settings.replace("mode=\"shadow\"", "mode=\"advisory\""))).unwrap();
    assert!(f.run(&["run", "Document the local fixture"]).status.success());
    let s = f.json(&["adaptive", "--json"]);
    assert_eq!(s["state"]["decisions"][1]["executed"]["model"], "baseline");
    assert!(s["production"].as_array().unwrap().is_empty());

    let report = f.json(&["stats", "--by-model", "--json"]);
    assert_eq!(report["tasks"][0]["queue_coverage"], 2);
    assert!(report["tasks"][0]["p90_queue_wait_ms"].is_number());
    for e in fs::read_dir(f.root.join("state/forkdeck/queue-v1")).unwrap() {
        let q: Value = serde_json::from_slice(&fs::read(e.unwrap().path()).unwrap()).unwrap();
        assert!(q["wait_ms"].is_number());
        assert!(q["dispatched_ms"].is_number());
        assert_eq!(q["provenance"], "process-monotonic-admission-v1");
    }
    let ledger = f.root.join("state/forkdeck/attempts-v1");
    for e in fs::read_dir(&ledger).unwrap() {
        let path = e.unwrap().path();
        let mut a: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        a["actual_model"] = Value::Null;
        a["terminal_usage"] = json!(false);
        a.as_object_mut().unwrap().remove("queue"); // Schema 2 before queue capture.
        fs::write(path, serde_json::to_vec(&a).unwrap()).unwrap();
    }
    let journal = f.root.join("state/forkdeck/journal.jsonl");
    let entries: Vec<Value> = fs::read_to_string(&journal).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    for (checks, evaluated, unknown) in [(Value::Null, 0, 2), (json!(false), 2, 0)] {
        let rows: Vec<String> = entries.iter().cloned().map(|mut e| {
            e["checks"] = checks.clone();
            serde_json::to_string(&e).unwrap()
        }).collect();
        fs::write(&journal, rows.join("\n") + "\n").unwrap();
        let r = f.json(&["stats", "--by-model", "--json"]);
        let t = &r["tasks"][0];
        assert_eq!(t["initial_model"], "unknown/default");
        assert_eq!(t["evaluated"], evaluated);
        assert_eq!(t["unknown_quality"], unknown);
        assert_eq!(t["accepted"], 0);
        assert_eq!(t["complete_usage"], 0);
        assert_eq!(t["queue_coverage"], 0);
        assert!(t["p90_queue_wait_ms"].is_null());
        assert!(t["reported_usd_per_accepted_task"].is_null());
        assert!(t["tokens_per_accepted_task_lower_bound"].is_null());
        assert_eq!(t["known_all_tokens"], 140);
        let text = String::from_utf8(f.run(&["stats", "--by-model"]).stdout).unwrap();
        assert!(text.contains(&format!("{evaluated} evaluated, 0 accepted, {unknown} unknown")));
        assert!(text.contains("Observational usage, not a model ranking"));
        assert!(text.contains("measured 0/2 tasks"));
    }
    let filtered = f.json(&["stats", "--by-model", "--repo", "absent/repository", "--json"]);
    assert!(filtered["tasks"].as_array().unwrap().is_empty());
    assert!(filtered["groups"].as_array().unwrap().is_empty());

    // Report-only fixture: measured production spend deliberately differs from prediction.
    let path = f.root.join("state/forkdeck/adaptive-v1.json");
    let mut state: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    for (i, d) in state["decisions"].as_array_mut().unwrap().iter_mut().enumerate() {
        d["mode"] = json!("adaptive");
        d["probability"] = json!(0.1);
        d["candidate"] = json!(i == 1);
        d["tokens"] = json!(if i == 1 { 56 } else { 70 });
    }
    fs::write(path, serde_json::to_vec(&state).unwrap()).unwrap();
    let r = f.json(&["adaptive", "--json"]);
    assert_eq!(r["state"]["decisions"][0]["qualification"]["predicted_savings"], 0.5);
    assert!((r["production"][0]["observed_savings"].as_f64().unwrap() - 0.2).abs() < 1e-10);
    let text = String::from_utf8(f.run(&["adaptive"]).stdout).unwrap();
    assert!(text.contains("Predicted savings 50.0%"));
    assert!(text.contains("Observed trial"));
    assert!(text.contains("1 candidates, 1 baseline, 0 pending; savings Some(0.19999999999999996)"));
}

#[test]
fn missing_policy_and_unavailable_telemetry_keep_baseline_and_require_requalification() {
    let f = Fixture::new();
    f.trials();
    let evidence = f.root.join("evidence.json");
    let saved = fs::read(&evidence).unwrap();
    fs::remove_file(&evidence).unwrap();
    assert!(!f.run(&["adaptive", "--enable"]).status.success());
    assert!(f.run(&["run", "Document the local fixture"]).status.success());
    let s = f.json(&["adaptive", "--json"]);
    let d = s["state"]["decisions"].as_array().unwrap().last().unwrap();
    assert_eq!(d["executed"]["model"], "baseline");
    assert!(d["proposed"].is_null());
    fs::write(&evidence, "broken json").unwrap();
    assert!(f.run(&["run", "Document the local fixture"]).status.success());
    assert!(!f.run(&["adaptive", "--enable"]).status.success());
    fs::write(&evidence, saved).unwrap();
    assert!(f.run(&["adaptive", "--enable"]).status.success());
    let ledger = f.root.join("state/forkdeck/attempts-v1");
    fs::rename(&ledger, ledger.with_extension("saved")).unwrap();
    fs::write(&ledger, "unavailable storage").unwrap();
    assert!(f.run(&["run", "Document the local fixture"]).status.success());
    let s = f.json(&["adaptive", "--json"]);
    let d = s["state"]["decisions"].as_array().unwrap().last().unwrap();
    assert_eq!(d["executed"]["model"], "baseline");
    assert!(d["tokens"].is_null());
    assert!(!s["state"]["suspended"].as_object().unwrap().is_empty());
    assert!(fs::read_dir(ledger.with_extension("saved")).unwrap().count() >= 2);
}
