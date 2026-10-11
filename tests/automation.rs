//! End-to-end tests of the unattended paths, run against the built binary with stand-ins for
//! `claude`, `gh` and an MCP server, and a local bare repository as the remote. They need `git` and `sh`.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const CLAUDE: &str = r#"#!/bin/sh
[ "$1" = "--version" ] && { echo stand-in; exit 0; }
prompt=$(cat)
[ -n "$FAKE_PROMPTS" ] && printf '%s\n-----\n' "$prompt" >> "$FAKE_PROMPTS"
case "$prompt" in
  *"Do not modify any file"*)
    [ -n "$FAKE_DURING_REVIEW" ] && PROMPT="$prompt" sh -c "$FAKE_DURING_REVIEW" >/dev/null 2>&1
    printf '%s\n' "{\"type\":\"result\",\"is_error\":false,\"result\":\"Looked at it.\\nVERDICT: ${FAKE_VERDICT:-PASS}\"}"
    ;;
  *)
    sh -c "${FAKE_WORK:-echo change >> by_agent.txt}" >/dev/null 2>&1
    printf '%s\n' '{"type":"result","is_error":false,"result":"Did the work."}'
    ;;
esac
"#;

const GH: &str = r#"#!/bin/sh
[ "$1" = "--version" ] && { echo stand-in; exit 0; }
case "$1 $2" in
  "issue list") cat "$FAKE_GH/issues.json" ;;
  "pr list") cat "$FAKE_GH/prs.json" ;;
  *)
    printf '%s ' "$@" | tr '\n' ' ' >> "$FAKE_GH/calls.log"
    echo >> "$FAKE_GH/calls.log"
    [ "$1 $2" = "pr create" ] && echo https://example.invalid/o/r/pull/12
    ;;
esac
exit 0
"#;

/// An MCP server over stdio. Each tool answers with the next line of `$FAKE_MCP/<tool>.txt`
/// (the last line repeats), and every call is appended to `$FAKE_MCP/calls.log`.
const MCP: &str = r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2025-11-25","capabilities":{"tools":{}},"serverInfo":{"name":"stand-in","version":"0"}}}\n' "$id" ;;
    *'"method":"tools/list"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"ci_status","description":"Pipeline state","inputSchema":{"type":"object"}},{"name":"deploy_status","description":"App health","inputSchema":{"type":"object"}}]}}\n' "$id" ;;
    *'"method":"tools/call"'*)
      tool=$(printf '%s' "$line" | sed -n 's/.*"name":"\([a-z_]*\)".*/\1/p')
      printf '%s\n' "$line" >> "$FAKE_MCP/calls.log"
      n=$(cat "$FAKE_MCP/$tool.n" 2>/dev/null || echo 0); n=$((n + 1)); echo "$n" > "$FAKE_MCP/$tool.n"
      total=$(wc -l < "$FAKE_MCP/$tool.txt" 2>/dev/null || echo 0)
      [ "$n" -gt "$total" ] && n=$total
      text=$(sed -n "${n}p" "$FAKE_MCP/$tool.txt" 2>/dev/null)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"%s"}],"isError":false}}\n' "$id" "$text" ;;
  esac
done
"#;

struct Rig {
    root: PathBuf,
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .expect("git runs");
    assert!(out.status.success(), "git {:?} failed: {}", args, String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

impl Rig {
    /// A remote with one commit on main, a clone of it, the stand-ins, and a config with one repo.
    fn new(name: &str, extra_config: &str, repo_config: &str) -> Rig {
        let root = std::env::temp_dir().join(format!("forkdeck-test-{}-{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for d in ["bin", "gh", "mcp", "home/.config/forkdeck"] {
            fs::create_dir_all(root.join(d)).unwrap();
        }
        for (file, body) in [("claude", CLAUDE), ("gh", GH), ("mcp-stand-in", MCP)] {
            let p = root.join("bin").join(file);
            fs::write(&p, body).unwrap();
            fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
        }
        git(&root, &["init", "-q", "--bare", "-b", "main", "origin.git"]);
        git(&root, &["clone", "-q", "origin.git", "clone"]);
        let clone = root.join("clone");
        git(&clone, &["config", "user.name", "Test Person"]);
        git(&clone, &["config", "user.email", "test@example.com"]);
        fs::write(clone.join("README"), "base\n").unwrap();
        git(&clone, &["add", "."]);
        git(&clone, &["commit", "-qm", "init"]);
        git(&clone, &["push", "-q", "origin", "HEAD:main"]);
        fs::write(root.join("gh/issues.json"), "[]").unwrap();
        fs::write(root.join("gh/prs.json"), "[]").unwrap();
        fs::write(
            root.join("home/.config/forkdeck/config.toml"),
            format!(
                "{}\n[automation]\nworktree_root = \"{}\"\n[[repos]]\nprovider = \"github\"\nrepo = \"o/r\"\npath = \"{}\"\n{}\n",
                extra_config,
                root.join("wt").display(),
                clone.display(),
                repo_config
            )
            .replace("{root}", &root.display().to_string()),
        )
        .unwrap();
        Rig { root }
    }

    fn clone(&self) -> PathBuf {
        self.root.join("clone")
    }
    fn origin(&self) -> PathBuf {
        self.root.join("origin.git")
    }

    fn issue(&self) {
        let j = r#"[{"number":7,"title":"Add a change file","body":"Please add it.","url":"https://example.invalid/o/r/issues/7","author":{"login":"someone"}}]"#;
        fs::write(self.root.join("gh/issues.json"), j).unwrap();
    }

    /// Push a branch with one new file and expose it the way GitHub exposes pull request N.
    fn pull_request(&self, n: u64, branch: &str) -> String {
        let c = self.clone();
        git(&c, &["checkout", "-q", "-b", branch, "main"]);
        fs::write(c.join(format!("{}.txt", branch)), "x\n").unwrap();
        git(&c, &["add", "."]);
        git(&c, &["commit", "-qm", branch]);
        git(&c, &["push", "-q", "origin", branch]);
        let sha = git(&c, &["rev-parse", "HEAD"]);
        git(&c, &["checkout", "-q", "main"]);
        git(&self.origin(), &["update-ref", &format!("refs/pull/{}/head", n), &sha]);
        sha
    }

    fn list_pull_request(&self, n: u64, branch: &str) {
        let j = format!(
            r#"[{{"number":{},"title":"A change","body":"Adds a file.","url":"https://example.invalid/o/r/pull/{}","author":{{"login":"someone"}},"headRefName":"{}","baseRefName":"main"}}]"#,
            n, n, branch
        );
        fs::write(self.root.join("gh/prs.json"), j).unwrap();
    }

    /// Run `forkdeck once` and return (exit ok, output).
    fn once(&self, env: &[(&str, &str)]) -> (bool, String) {
        let mut c = Command::new(env!("CARGO_BIN_EXE_forkdeck"));
        c.arg("once")
            .current_dir(self.clone())
            .env_clear()
            .env("PATH", format!("{}:/usr/local/bin:/usr/bin:/bin", self.root.join("bin").display()))
            .env("HOME", self.root.join("home"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("FAKE_GH", self.root.join("gh"));
        for (k, v) in env {
            c.env(k, v);
        }
        let out = c.output().expect("forkdeck runs");
        (out.status.success(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
    }

    /// Script the answers of one tool on the stand-in MCP server and start it from the first again.
    fn mcp_answers(&self, tool: &str, lines: &[&str]) {
        fs::write(self.root.join(format!("mcp/{}.txt", tool)), format!("{}\n", lines.join("\n"))).unwrap();
        let _ = fs::remove_file(self.root.join(format!("mcp/{}.n", tool)));
    }

    fn mcp_calls(&self) -> String {
        fs::read_to_string(self.root.join("mcp/calls.log")).unwrap_or_default()
    }

    /// Add a commit to the remote's main branch, as a merge would.
    fn merge_to_main(&self, file: &str) -> String {
        let c = self.clone();
        fs::write(c.join(file), "merged\n").unwrap();
        git(&c, &["add", "."]);
        git(&c, &["commit", "-qm", file]);
        git(&c, &["push", "-q", "origin", "HEAD:main"]);
        git(&c, &["rev-parse", "HEAD"])
    }

    fn calls(&self) -> String {
        fs::read_to_string(self.root.join("gh/calls.log")).unwrap_or_default()
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

#[test]
fn passing_issue_becomes_one_commit_and_a_draft_pull_request() {
    let rig = Rig::new("pass", "", "");
    rig.issue();
    let (ok, out) = rig.once(&[]);
    assert!(ok, "{}", out);
    let calls = rig.calls();
    assert!(calls.contains("pr create") && calls.contains("--draft"), "{}", calls);
    assert!(calls.contains("--add-label agent:pr-open"), "{}", calls);
    assert_eq!(git(&rig.origin(), &["rev-list", "--count", "main..forkdeck/issue-7"]), "1");
    assert_eq!(git(&rig.origin(), &["show", "--name-only", "--format=", "forkdeck/issue-7"]), "by_agent.txt");
}

#[test]
fn a_failed_attempt_survives_a_retry() {
    let rig = Rig::new("retry", "[review]\nmax_fix_rounds = 0", "");
    rig.issue();
    let (ok, out) = rig.once(&[("FAKE_VERDICT", "FAIL")]);
    assert!(!ok, "a failed review must fail the run:\n{}", out);
    assert!(rig.calls().contains("--add-label agent:needs-human"));
    assert!(!rig.calls().contains("pr create"), "nothing is published after a failed review");
    let first = git(&rig.clone(), &["rev-parse", "forkdeck/issue-7"]);
    assert_ne!(first, git(&rig.clone(), &["rev-parse", "origin/main"]), "the attempt was committed locally");

    // The issue is labelled again; this attempt fails as well.
    let (_, out) = rig.once(&[("FAKE_VERDICT", "FAIL"), ("FAKE_WORK", "echo second >> other.txt")]);
    let holding = git(&rig.clone(), &["branch", "--contains", &first, "--format=%(refname:short)"]);
    assert!(holding.contains("forkdeck/attempts/issue-7-"), "first attempt must stay on a branch, got {:?}\n{}", holding, out);
    let second = git(&rig.clone(), &["rev-parse", "forkdeck/issue-7"]);
    assert_ne!(first, second);
    assert_eq!(git(&rig.clone(), &["show", "--name-only", "--format=", &second]), "other.txt", "the retry started from the base");

    // Work left uncommitted in the worktree (a crash) is kept too.
    let wt = rig.root.join("wt/github/o_r/issue-7");
    fs::write(wt.join("half-done.txt"), "wip\n").unwrap();
    rig.once(&[("FAKE_VERDICT", "FAIL")]);
    let kept = git(&rig.clone(), &["log", "--all", "--format=%s", "--", "half-done.txt"]);
    assert!(kept.contains("Unfinished attempt at #7"), "{:?}", kept);
    assert!(git(&rig.clone(), &["branch", "--contains", &second]).contains("forkdeck/attempts/"));
}

#[test]
fn a_pull_request_whose_head_moves_during_review_is_not_passed() {
    let rig = Rig::new("stale", "", "");
    let reviewed = rig.pull_request(5, "feat");
    let later = rig.pull_request(6, "later");
    rig.list_pull_request(5, "feat");
    // While the reviewer is reading, the author pushes: pull request 5 now points at another commit.
    let push = format!("git -C {} update-ref refs/pull/5/head {}", rig.origin().display(), later);
    let (ok, out) = rig.once(&[("FAKE_DURING_REVIEW", &push)]);
    assert!(ok, "{}", out);
    let calls = rig.calls();
    assert!(!calls.contains("review:passed"), "an unreviewed head must never get the pass label:\n{}", calls);
    assert!(calls.contains("forkdeck review: stale"), "{}", calls);
    assert!(calls.contains(&reviewed[..12]) && calls.contains(&later[..12]), "the comment names both commits:\n{}", calls);
    assert!(calls.contains("--remove-label review:running --add-label review:ready"), "{}", calls);
}

#[test]
fn a_pull_request_that_stays_put_is_passed_with_its_commit_named() {
    let rig = Rig::new("fresh", "", "");
    let reviewed = rig.pull_request(5, "feat");
    rig.list_pull_request(5, "feat");
    let (ok, out) = rig.once(&[]);
    assert!(ok, "{}", out);
    let calls = rig.calls();
    assert!(calls.contains("--add-label review:passed"), "{}", calls);
    assert!(calls.contains(&format!("applies to commit `{}` only", &reviewed[..12])), "{}", calls);
}

#[test]
fn an_incomplete_review_goes_back_to_the_request_label() {
    let rig = Rig::new("incomplete", "", "");
    rig.pull_request(5, "feat");
    rig.list_pull_request(5, "feat");
    let (_, out) = rig.once(&[("FAKE_VERDICT", "MAYBE")]);
    let calls = rig.calls();
    assert!(!calls.contains("review:passed") && !calls.contains("review:changes"), "{}\n{}", calls, out);
    assert!(calls.contains("--add-label review:ready"), "{}", calls);
}

#[test]
fn a_check_cannot_add_to_the_commit_through_the_index() {
    let rig = Rig::new("index", "", "checks = [\"echo generated > leftover.txt && git add leftover.txt && echo more >> README\"]");
    rig.issue();
    let (ok, out) = rig.once(&[]);
    assert!(ok, "{}", out);
    assert_eq!(
        git(&rig.origin(), &["show", "--name-only", "--format=", "forkdeck/issue-7"]),
        "by_agent.txt",
        "only the agent's file may be committed"
    );
}

#[test]
fn a_check_that_moves_head_stops_the_job() {
    let rig = Rig::new("head", "", "checks = [\"git -c user.name=x -c user.email=x@y commit -q --allow-empty -m sneaky\"]");
    rig.issue();
    let (ok, out) = rig.once(&[]);
    assert!(!ok, "{}", out);
    assert!(out.contains("moved HEAD"), "{}", out);
    assert!(!rig.calls().contains("pr create"));
    assert!(rig.calls().contains("--add-label agent:needs-human"));
}

#[test]
fn a_commit_made_by_the_agent_is_reviewed_and_squashed() {
    let rig = Rig::new("agentcommit", "", "");
    rig.issue();
    let seen = rig.root.join("reviewer-saw.txt");
    // The reviewer follows its prompt: diff from the commit the work started at.
    let look = format!(
        "b=$(printf '%s' \"$PROMPT\" | sed -n 's/.*started from commit \\([0-9a-f]*\\)\\..*/\\1/p' | head -1); git diff --name-only \"$b\" > {}",
        seen.display()
    );
    let work = "echo c > committed.txt && git add . && git -c user.name=agent -c user.email=a@b commit -qm 'agent commit' && echo u > uncommitted.txt";
    let (ok, out) = rig.once(&[("FAKE_WORK", work), ("FAKE_DURING_REVIEW", &look)]);
    assert!(ok, "{}", out);
    let saw = fs::read_to_string(&seen).unwrap_or_default();
    assert!(saw.contains("committed.txt") && saw.contains("uncommitted.txt"), "the reviewer must see committed work too, saw {:?}", saw);
    assert_eq!(git(&rig.origin(), &["rev-list", "--count", "main..forkdeck/issue-7"]), "1", "one commit, not the agent's own");
    assert_eq!(git(&rig.origin(), &["log", "-1", "--format=%an|%s", "forkdeck/issue-7"]), "Test Person|Add a change file (#7)");
    let mut files: Vec<String> =
        git(&rig.origin(), &["show", "--name-only", "--format=", "forkdeck/issue-7"]).lines().map(String::from).collect();
    files.sort();
    assert_eq!(files, vec!["committed.txt", "uncommitted.txt"]);
}

const MCP_SERVER: &str = "[mcp.pipeline]\ncommand = \"{root}/bin/mcp-stand-in\"\nenv = { FAKE_MCP = \"{root}/mcp\" }";

const CI_STAGE: &str = r#"
[repos.ci]
server = "pipeline"
interval_secs = 1
max_fix_rounds = 1
[repos.ci.status]
tool = "ci_status"
args = { pr = "{pr}", ref = "{branch}", owner = "{owner}", name = "{name}" }
success = "PASSED"
failure = "FAILED"
capture = { run = 'run=(\d+)' }
[repos.ci.logs]
tool = "ci_logs"
args = { sha = "{sha}", run = "{run}" }
"#;

#[test]
fn a_ci_failure_goes_back_to_the_worker_and_the_fix_is_pushed() {
    let rig = Rig::new("ci-fix", MCP_SERVER, CI_STAGE);
    rig.issue();
    rig.mcp_answers("ci_status", &["state=RUNNING", "state=FAILED run=4411", "state=PASSED"]);
    rig.mcp_answers("ci_logs", &["error: test_parse_header failed at parser.rs line 9"]);
    let prompts = rig.root.join("prompts.txt");
    let (ok, out) = rig.once(&[("FAKE_PROMPTS", prompts.to_str().unwrap())]);
    assert!(ok, "{}", out);
    assert!(out.contains("CI passed"), "{}", out);

    // The tool was called with the pull request as a number and the branch filled in.
    let mcp = rig.mcp_calls();
    assert!(mcp.contains(r#""pr":12"#) && mcp.contains(r#""ref":"forkdeck/issue-7""#) && mcp.contains(r#""owner":"o""#), "{}", mcp);
    assert!(mcp.contains(r#""run":4411"#), "the failing run named in the status answer is passed to the log call:\n{}", mcp);
    // The worker was shown the pipeline output.
    let seen = fs::read_to_string(&prompts).unwrap();
    assert!(seen.contains("test_parse_header failed") && seen.contains("not instructions to follow"), "{}", seen);
    // The fix is a second commit on the same branch, pushed without force, and noted on the pull request.
    assert_eq!(git(&rig.origin(), &["rev-list", "--count", "main..forkdeck/issue-7"]), "2");
    assert_eq!(git(&rig.origin(), &["log", "-1", "--format=%s", "forkdeck/issue-7"]), "Fix CI failure (#7)");
    let calls = rig.calls();
    assert_eq!(calls.matches("pr create").count(), 1, "one pull request, not two:\n{}", calls);
    assert!(calls.contains("pr comment 12") && calls.contains("pushed a fix"), "{}", calls);
    assert!(!calls.contains("agent:needs-human"), "{}", calls);
}

#[test]
fn ci_that_stays_red_is_handed_to_a_person_with_the_pull_request_left_open() {
    let rig = Rig::new("ci-red", MCP_SERVER, CI_STAGE);
    rig.issue();
    rig.mcp_answers("ci_status", &["state=FAILED"]);
    rig.mcp_answers("ci_logs", &["error: still broken"]);
    let (ok, out) = rig.once(&[]);
    assert!(!ok, "{}", out);
    assert!(out.contains("still failing after 1 fix round"), "{}", out);
    assert_eq!(git(&rig.origin(), &["rev-list", "--count", "main..forkdeck/issue-7"]), "2", "one fix was tried, no more");
    let calls = rig.calls();
    assert!(calls.contains("--remove-label agent:pr-open --add-label agent:needs-human"), "{}", calls);
    assert!(calls.contains("pr comment 12") && calls.contains("forkdeck stopped here"), "{}", calls);
}

const DEPLOY_STAGE: &str = r#"
[repos.deploy]
server = "pipeline"
environment = "staging"
interval_secs = 1
verify = ["test -f README"]
[repos.deploy.trigger]
tool = "deploy"
args = { revision = "{sha}", target = "{env}" }
[repos.deploy.status]
tool = "deploy_status"
args = { app = "{name}" }
success = "Healthy"
failure = "Degraded"
revision = "rev=([0-9a-f]+)"
[repos.deploy.rollback]
tool = "rollback"
args = { to = "{last_good}" }
[repos.deploy.notify]
tool = "notify"
args = { text = "{message}" }
"#;

#[test]
fn a_merge_is_deployed_to_staging_and_a_bad_one_is_rolled_back() {
    let rig = Rig::new("deploy", MCP_SERVER, DEPLOY_STAGE);
    let start = git(&rig.clone(), &["rev-parse", "HEAD"]);
    let state = rig.root.join("home/.local/state/forkdeck/deploy.json");

    // First sight: note where the branch is, deploy nothing.
    let (ok, out) = rig.once(&[]);
    assert!(ok, "{}", out);
    assert_eq!(rig.mcp_calls(), "", "nothing may be deployed on first sight");
    assert!(fs::read_to_string(&state).unwrap().contains(&start));

    // A merge: deployed, and only reported healthy once the system is on the new commit.
    let a = rig.merge_to_main("a.txt");
    rig.mcp_answers("deploy_status", &[&format!("rev={} Healthy", &start[..12]), &format!("rev={} Progressing", &a[..12]), &format!("rev={} Healthy", &a[..12])]);
    let (ok, out) = rig.once(&[]);
    assert!(ok, "{}", out);
    let mcp = rig.mcp_calls();
    assert!(mcp.contains(&format!(r#""revision":"{}""#, a)) && mcp.contains(r#""target":"staging""#), "{}", mcp);
    assert_eq!(mcp.matches(r#""name":"deploy_status""#).count(), 3, "waited through the old commit and the rollout:\n{}", mcp);
    assert!(!mcp.contains(r#""name":"rollback""#));
    assert!(mcp.contains("is deployed to staging"), "the notification carries the result:\n{}", mcp);

    // Nothing new: nothing happens.
    let before = rig.mcp_calls().len();
    rig.once(&[]);
    assert_eq!(rig.mcp_calls().len(), before, "an unchanged branch must not be deployed again");

    // A bad merge: rolled back to the last commit that deployed cleanly.
    let b = rig.merge_to_main("b.txt");
    rig.mcp_answers("deploy_status", &[&format!("rev={} Degraded", &b[..12]), &format!("rev={} Healthy", &a[..12])]);
    let (ok, out) = rig.once(&[]);
    assert!(!ok, "a failed deployment fails the run:\n{}", out);
    let mcp = rig.mcp_calls();
    assert!(mcp.contains(&format!(r#""to":"{}""#, a)), "rollback targets the last good commit:\n{}", mcp);
    assert!(out.contains(&format!("Rolled back to {}", &a[..10])), "{}", out);
    let saved = fs::read_to_string(&state).unwrap();
    assert!(saved.contains(&format!(r#""last_good": "{}""#, a)) && saved.contains(&format!(r#""seen": "{}""#, b)), "{}", saved);

    // And the bad commit is not retried on the next pass.
    let before = rig.mcp_calls().len();
    rig.once(&[]);
    assert_eq!(rig.mcp_calls().len(), before);
}

#[test]
fn a_failed_smoke_test_rolls_the_deployment_back() {
    let rig = Rig::new("verify", MCP_SERVER, &DEPLOY_STAGE.replace("test -f README", "test -f smoke-ok"));
    let start = git(&rig.clone(), &["rev-parse", "HEAD"]);
    rig.once(&[]);
    let a = rig.merge_to_main("a.txt");
    rig.mcp_answers("deploy_status", &[&format!("rev={} Healthy", &a[..12]), &format!("rev={} Healthy", &start[..12])]);
    let (ok, out) = rig.once(&[]);
    assert!(!ok, "{}", out);
    assert!(out.contains("verification failed"), "{}", out);
    assert!(rig.mcp_calls().contains(&format!(r#""to":"{}""#, start)), "{}", rig.mcp_calls());
}
