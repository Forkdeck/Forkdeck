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
/// (the last line repeats), and every call is appended to `$FAKE_MCP/calls.log`. In an answer,
/// `@SHA@` becomes the commit the call asked about and `@FIRST@` the first commit ever asked about.
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
      if [ -f "$FAKE_MCP/on_$tool.sh" ]; then mv "$FAKE_MCP/on_$tool.sh" "$FAKE_MCP/ran_$tool.sh"; sh "$FAKE_MCP/ran_$tool.sh" >/dev/null 2>&1; fi
      n=$(cat "$FAKE_MCP/$tool.n" 2>/dev/null || echo 0); n=$((n + 1)); echo "$n" > "$FAKE_MCP/$tool.n"
      total=$(wc -l < "$FAKE_MCP/$tool.txt" 2>/dev/null || echo 0)
      [ "$n" -gt "$total" ] && n=$total
      text=$(sed -n "${n}p" "$FAKE_MCP/$tool.txt" 2>/dev/null)
      sha=$(printf '%s' "$line" | sed -n 's/.*"sha":"\([0-9a-f]*\)".*/\1/p')
      [ -n "$sha" ] && [ ! -f "$FAKE_MCP/first_sha" ] && printf '%s' "$sha" > "$FAKE_MCP/first_sha"
      first=$(cat "$FAKE_MCP/first_sha" 2>/dev/null)
      text=$(printf '%s' "$text" | sed "s/@SHA@/$sha/g; s/@FIRST@/$first/g")
      err=false; case "$text" in ERR:*) err=true ;; esac
      printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"%s"}],"isError":%s}}\n' "$id" "$text" "$err" ;;
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

    fn command(&self, env: &[(&str, &str)]) -> Command {
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
        c
    }

    /// Run `forkdeck once` and return (exit ok, output).
    fn once(&self, env: &[(&str, &str)]) -> (bool, String) {
        let out = self.command(env).output().expect("forkdeck runs");
        (out.status.success(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
    }

    /// Start `forkdeck once`, wait until `ready` holds (or ten seconds), send it SIGTERM as a
    /// service manager or a closing terminal would, and return its output.
    fn once_then_stop(&self, ready: impl Fn(&Rig) -> bool, while_running: impl FnOnce(&Rig)) -> String {
        let child = self.command(&[]).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).spawn().expect("forkdeck starts");
        for _ in 0..100 {
            if ready(self) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        assert!(ready(self), "forkdeck never reached the point the test stops it at");
        while_running(self);
        Command::new("kill").args(["-TERM", &child.id().to_string()]).status().unwrap();
        let out = child.wait_with_output().unwrap();
        format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))
    }

    fn state_file(&self) -> PathBuf {
        self.root.join("home/.local/state/forkdeck/deploy.json")
    }

    /// Script the answers of one tool on the stand-in MCP server and start it from the first again.
    fn mcp_answers(&self, tool: &str, lines: &[&str]) {
        fs::write(self.root.join(format!("mcp/{}.txt", tool)), format!("{}\n", lines.join("\n"))).unwrap();
        let _ = fs::remove_file(self.root.join(format!("mcp/{}.n", tool)));
    }

    /// First start with a deploy stage: the deployment system reports the branch head as running and
    /// healthy, so forkdeck takes it as the commit to roll back to. Returns that commit.
    fn adopt(&self) -> String {
        let head = git(&self.clone(), &["rev-parse", "HEAD"]);
        self.mcp_answers("deploy_status", &[&format!("rev={} Healthy", &head[..12])]);
        let (ok, out) = self.once(&[]);
        assert!(ok && out.contains("reports healthy"), "{}", out);
        let _ = fs::remove_file(self.root.join("mcp/calls.log"));
        head
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
settle_secs = 0
[repos.ci.status]
tool = "ci_status"
args = { pr = "{pr}", ref = "{branch}", owner = "{owner}", name = "{name}", sha = "{sha}" }
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

    // First sight: note where the branch is, ask what is running, deploy nothing.
    rig.mcp_answers("deploy_status", &[&format!("rev={} Healthy", &start[..12])]);
    let (ok, out) = rig.once(&[]);
    assert!(ok, "{}", out);
    assert!(!rig.mcp_calls().contains(r#""name":"deploy""#), "nothing may be deployed on first sight:\n{}", rig.mcp_calls());
    assert!(fs::read_to_string(&state).unwrap().contains(&format!(r#""last_good": "{}""#, start)), "confirmed healthy, so it is a rollback target");
    fs::remove_file(rig.root.join("mcp/calls.log")).unwrap();

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
    rig.adopt();
    let a = rig.merge_to_main("a.txt");
    rig.mcp_answers("deploy_status", &[&format!("rev={} Healthy", &a[..12]), &format!("rev={} Healthy", &start[..12])]);
    let (ok, out) = rig.once(&[]);
    assert!(!ok, "{}", out);
    assert!(out.contains("verification failed"), "{}", out);
    assert!(rig.mcp_calls().contains(&format!(r#""to":"{}""#, start)), "{}", rig.mcp_calls());
}

#[test]
fn one_tool_error_is_not_a_failed_deployment_or_a_red_pipeline() {
    // Deploy: a rate limit in the middle of a rollout must not roll anything back.
    let rig = Rig::new("flaky-deploy", MCP_SERVER, DEPLOY_STAGE);
    rig.adopt();
    let a = rig.merge_to_main("a.txt");
    rig.mcp_answers("deploy_status", &[&format!("rev={} Progressing", &a[..12]), "ERR: rate limited, try later", &format!("rev={} Healthy", &a[..12])]);
    let (ok, out) = rig.once(&[]);
    assert!(ok, "{}", out);
    assert!(!rig.mcp_calls().contains(r#""name":"rollback""#), "{}", rig.mcp_calls());
    assert!(fs::read_to_string(rig.state_file()).unwrap().contains(&format!(r#""last_good": "{}""#, a)));

    // CI: a bad gateway on the status call must not send the worker off to "fix" anything.
    let rig = Rig::new("flaky-ci", MCP_SERVER, CI_STAGE);
    rig.issue();
    rig.mcp_answers("ci_status", &["ERR: 502 Bad Gateway", "state=PASSED"]);
    let (ok, out) = rig.once(&[]);
    assert!(ok, "{}", out);
    assert_eq!(git(&rig.origin(), &["rev-list", "--count", "main..forkdeck/issue-7"]), "1", "no fix commit for a pipeline that never failed");
}

#[test]
fn an_unreadable_status_rolls_nothing_back() {
    let rig = Rig::new("unknown", MCP_SERVER, DEPLOY_STAGE);
    rig.adopt();
    let a = rig.merge_to_main("a.txt");
    rig.mcp_answers("deploy_status", &["ERR: upstream unavailable"]);
    let (ok, out) = rig.once(&[]);
    assert!(!ok, "not knowing is still a failed run:\n{}", out);
    assert!(out.contains("UNKNOWN") && out.contains("Nothing was rolled back"), "{}", out);
    assert!(!rig.mcp_calls().contains(r#""name":"rollback""#), "no evidence of failure, no rollback:\n{}", rig.mcp_calls());
    let saved = fs::read_to_string(rig.state_file()).unwrap();
    assert!(!saved.contains(&format!(r#""last_good": "{}""#, a)), "an unverified commit never becomes a rollback target:\n{}", saved);
}

#[test]
fn a_newer_merge_takes_over_instead_of_triggering_a_rollback() {
    let rig = Rig::new("superseded", MCP_SERVER, DEPLOY_STAGE);
    let start = git(&rig.clone(), &["rev-parse", "HEAD"]);
    rig.adopt();
    let a = rig.merge_to_main("a.txt");
    // While A is rolling out, B is merged, and from then on the system only ever reports B.
    let c = rig.clone();
    fs::write(
        rig.root.join("mcp/on_deploy.sh"),
        format!("cd {} && echo b > b.txt && git add . && git commit -qm b && git push -q origin HEAD:main", c.display()),
    )
    .unwrap();
    rig.mcp_answers("deploy_status", &[&format!("rev={} Healthy", &start[..12])]);
    let (ok, out) = rig.once(&[]);
    assert!(ok, "{}", out);
    assert!(out.contains("now points at"), "{}", out);
    assert!(!rig.mcp_calls().contains(r#""name":"rollback""#), "a healthy environment must not be rolled back:\n{}", rig.mcp_calls());

    // The next pass deploys B.
    let b = git(&rig.clone(), &["rev-parse", "HEAD"]);
    assert_ne!(a, b);
    rig.mcp_answers("deploy_status", &[&format!("rev={} Healthy", &b[..12])]);
    let (ok, out) = rig.once(&[]);
    assert!(ok, "{}", out);
    assert!(rig.mcp_calls().contains(&format!(r#""revision":"{}""#, b)), "{}", rig.mcp_calls());
    assert!(fs::read_to_string(rig.state_file()).unwrap().contains(&format!(r#""last_good": "{}""#, b)));
}

#[test]
fn stopping_during_verification_rolls_nothing_back_and_the_next_start_picks_it_up() {
    let rig = Rig::new("stop-verify", MCP_SERVER, &DEPLOY_STAGE.replace("test -f README", "test -f {root}/fast || sleep 60"));
    rig.adopt();
    let a = rig.merge_to_main("a.txt");
    rig.mcp_answers("deploy_status", &[&format!("rev={} Healthy", &a[..12])]);
    let out = rig.once_then_stop(
        |r| r.mcp_calls().contains(r#""name":"deploy_status""#),
        |r| {
            // A second forkdeck started meanwhile must stay out of deployments altogether.
            let before = r.mcp_calls().len();
            let (_, other) = r.once(&[]);
            assert!(other.contains("another forkdeck is running deployments"), "{}", other);
            assert_eq!(r.mcp_calls().len(), before, "the second process made tool calls");
        },
    );
    assert!(out.contains("cancelled while verifying"), "{}", out);
    assert!(out.contains("nothing was rolled back"), "{}", out);
    let mcp = rig.mcp_calls();
    assert!(!mcp.contains(r#""name":"rollback""#), "quitting is not a failed smoke test:\n{}", mcp);
    assert!(!mcp.contains(r#""name":"notify""#), "{}", mcp);
    let saved = fs::read_to_string(rig.state_file()).unwrap();
    assert!(saved.contains(&format!(r#""in_progress": "{}""#, a)), "the interrupted deployment is remembered:\n{}", saved);

    // Started again: it says what happened and finishes the job.
    fs::write(rig.root.join("fast"), "").unwrap();
    let (ok, out) = rig.once(&[]);
    assert!(ok, "{}", out);
    assert!(out.contains("was started and never finished"), "{}", out);
    let saved = fs::read_to_string(rig.state_file()).unwrap();
    assert!(saved.contains(&format!(r#""last_good": "{}""#, a)) && saved.contains(r#""in_progress": null"#), "{}", saved);
}

#[test]
fn a_damaged_state_file_stops_deployments_instead_of_resetting_them() {
    let rig = Rig::new("damaged", MCP_SERVER, DEPLOY_STAGE);
    rig.adopt();
    rig.merge_to_main("a.txt");
    fs::write(rig.state_file(), "{ \"github:o/r:staging\": { \"seen\": ").unwrap();
    let (_, out) = rig.once(&[]);
    assert!(out.contains("is damaged"), "{}", out);
    assert_eq!(rig.mcp_calls(), "", "nothing may be deployed from a state that cannot be read");
    assert!(fs::read_to_string(rig.state_file()).unwrap().starts_with("{ \"github:o/r:staging\": { \"seen\": "), "the file is left for a person to repair");
}

#[test]
fn a_failure_left_over_from_the_previous_run_does_not_use_up_a_fix_round() {
    // After the fix is pushed, the status tool still shows the old failure for one more poll.
    let rig = Rig::new("stale-ci", MCP_SERVER, &CI_STAGE.replace("settle_secs = 0", "settle_secs = 4"));
    rig.issue();
    rig.mcp_answers("ci_status", &["state=RUNNING", "state=FAILED run=1", "state=FAILED run=1", "state=PASSED"]);
    rig.mcp_answers("ci_logs", &["error: flaky"]);
    let (ok, out) = rig.once(&[]);
    assert!(ok, "the stale failure was believed:\n{}", out);
    assert_eq!(git(&rig.origin(), &["rev-list", "--count", "main..forkdeck/issue-7"]), "2", "exactly one fix");
    assert!(!rig.calls().contains("agent:needs-human"));
}

#[test]
fn stopping_while_ci_runs_leaves_the_open_pull_request_alone() {
    let rig = Rig::new("stop-ci", MCP_SERVER, CI_STAGE);
    rig.issue();
    rig.mcp_answers("ci_status", &["state=RUNNING"]);
    let out = rig.once_then_stop(|r| r.mcp_calls().contains(r#""name":"ci_status""#), |_| {});
    let calls = rig.calls();
    assert!(calls.contains("pr create") && calls.contains("--add-label agent:pr-open"), "{}\n{}", calls, out);
    assert!(!calls.contains("agent:needs-human"), "the work did not fail, forkdeck was stopped:\n{}", calls);
    assert!(!calls.contains("could not finish"), "{}", calls);
}

#[test]
fn a_branch_forced_back_is_deployed_again_rather_than_assumed() {
    let rig = Rig::new("forced-back", MCP_SERVER, DEPLOY_STAGE);
    let start = git(&rig.clone(), &["rev-parse", "HEAD"]);
    rig.adopt();
    let b = rig.merge_to_main("b.txt");
    // B is triggered, then main is forced back to where it was.
    fs::write(rig.root.join("mcp/on_deploy.sh"), format!("git -C {} push -q --force origin {}:refs/heads/main", rig.clone().display(), start)).unwrap();
    rig.mcp_answers("deploy_status", &[&format!("rev={} Progressing", &b[..12])]);
    let (ok, out) = rig.once(&[]);
    assert!(ok, "{}", out);
    assert!(out.contains("now points at"), "{}", out);
    assert!(!rig.mcp_calls().contains(r#""name":"rollback""#));

    // B was triggered, so the environment is not known to be on the old commit any more: deploy it again.
    rig.mcp_answers("deploy_status", &[&format!("rev={} Healthy", &start[..12])]);
    let (ok, out) = rig.once(&[]);
    assert!(ok, "{}", out);
    assert!(rig.mcp_calls().contains(&format!(r#""revision":"{}""#, start)), "the commit the branch was forced back to is deployed:\n{}", rig.mcp_calls());
    assert!(out.contains(&format!("{} is deployed to staging", &start[..10])), "{}", out);
}

#[test]
fn a_failed_smoke_test_does_not_roll_back_once_a_newer_commit_is_on_the_branch() {
    let rig = Rig::new("verify-moved", MCP_SERVER, &DEPLOY_STAGE.replace("test -f README", "test -f smoke-ok"));
    rig.adopt();
    let a = rig.merge_to_main("a.txt");
    // A reports healthy; before its smoke test has run, C is merged, and the smoke test then fails.
    fs::write(
        rig.root.join("mcp/on_deploy_status.sh"),
        format!("cd {} && echo c > c.txt && git add . && git commit -qm c && git push -q origin HEAD:main", rig.clone().display()),
    )
    .unwrap();
    rig.mcp_answers("deploy_status", &[&format!("rev={} Healthy", &a[..12])]);
    let (ok, out) = rig.once(&[]);
    assert!(!ok, "{}", out);
    assert!(out.contains("verification failed") && out.contains("Nothing was rolled back, because main already points at"), "{}", out);
    assert!(!rig.mcp_calls().contains(r#""name":"rollback""#), "rolling back now would undo the newer commit:\n{}", rig.mcp_calls());

    // The newer commit is deployed on the next pass.
    let c = git(&rig.clone(), &["rev-parse", "HEAD"]);
    rig.mcp_answers("deploy_status", &[&format!("rev={} Healthy", &c[..12])]);
    fs::write(rig.clone().join("smoke-ok"), "").unwrap();
    let (ok, out) = rig.once(&[]);
    assert!(ok, "{}", out);
    assert!(rig.mcp_calls().contains(&format!(r#""revision":"{}""#, c)), "{}", rig.mcp_calls());
}

/// A deployment system that rolls out in steps and decides for itself whether to go on.
fn progressive_stage() -> String {
    DEPLOY_STAGE.replace(
        "failure = \"Degraded\"",
        "failure = \"Degraded\"\nreverted = \"Aborted\"\npaused = \"Paused\"\nprogress = 'step=(\\S+)'",
    )
}

#[test]
fn a_canary_rollout_is_followed_through_its_steps() {
    let rig = Rig::new("canary", MCP_SERVER, &progressive_stage());
    rig.adopt();
    let a = rig.merge_to_main("a.txt");
    let r = &a[..12];
    rig.mcp_answers(
        "deploy_status",
        &[
            &format!("rev={} step=canary-20% Progressing", r),
            &format!("rev={} step=canary-50% Paused", r),
            &format!("rev={} step=eu-west-then-us-east Progressing", r),
            &format!("rev={} step=all-regions Healthy", r),
        ],
    );
    let (ok, out) = rig.once(&[]);
    assert!(ok, "{}", out);
    // forkdeck shows where the system says it is; it sent nothing about strategy.
    assert!(out.contains("pending · canary-20%") && out.contains("paused, waiting for a person · canary-50%") && out.contains("eu-west-then-us-east"), "{}", out);
    let mcp = rig.mcp_calls();
    assert_eq!(mcp.matches(r#""name":"deploy""#).count(), 1, "one trigger, however many steps:\n{}", mcp);
    assert!(!mcp.contains("canary") && !mcp.contains("percent"), "the strategy is not forkdeck's to send:\n{}", mcp);
    assert!(!mcp.contains(r#""name":"rollback""#));
    assert!(fs::read_to_string(rig.state_file()).unwrap().contains(&format!(r#""last_good": "{}""#, a)));
}

#[test]
fn a_rollout_the_system_aborts_is_a_failure_without_a_second_rollback() {
    let rig = Rig::new("aborted", MCP_SERVER, &progressive_stage());
    let start = git(&rig.clone(), &["rev-parse", "HEAD"]);
    rig.adopt();
    let a = rig.merge_to_main("a.txt");
    // The canary fails its analysis; the system puts the old version back and reports that.
    rig.mcp_answers("deploy_status", &[&format!("rev={} step=canary-20% Progressing", &a[..12]), &format!("rev={} Aborted", &start[..12])]);
    let (ok, out) = rig.once(&[]);
    assert!(!ok, "{}", out);
    assert!(out.contains("restored the previous version itself") && out.contains("forkdeck called no rollback"), "{}", out);
    let mcp = rig.mcp_calls();
    assert!(!mcp.contains(r#""name":"rollback""#), "the system already reverted; a rollback on top would fight it:\n{}", mcp);
    assert!(mcp.contains(r#""name":"notify""#) && mcp.contains("restored the previous version"), "{}", mcp);
    let saved = fs::read_to_string(rig.state_file()).unwrap();
    assert!(saved.contains(&format!(r#""last_good": "{}""#, start)) && saved.contains(&format!(r#""seen": "{}""#, a)), "{}", saved);
}

#[test]
fn an_abort_left_over_from_an_earlier_rollout_is_not_taken_for_this_one() {
    let rig = Rig::new("stale-abort", MCP_SERVER, &progressive_stage());
    let start = git(&rig.clone(), &["rev-parse", "HEAD"]);
    rig.adopt();
    let a = rig.merge_to_main("a.txt");
    rig.mcp_answers("deploy_status", &[&format!("rev={} Aborted", &start[..12]), &format!("rev={} step=canary-20% Progressing", &a[..12]), &format!("rev={} Healthy", &a[..12])]);
    let (ok, out) = rig.once(&[]);
    assert!(ok, "{}", out);
    assert!(out.contains(&format!("{} is deployed to staging", &a[..10])), "{}", out);
}

#[test]
fn nothing_is_rolled_back_when_the_branch_cannot_be_read() {
    // The deployment is bad for certain (the smoke test says no), but at that moment the remote is
    // unreachable, so forkdeck cannot rule out that a newer commit is already rolling out.
    let rig = Rig::new("blind", MCP_SERVER, &DEPLOY_STAGE.replace("test -f README", "mv {root}/origin.git {root}/origin.gone; exit 1"));
    rig.adopt();
    let a = rig.merge_to_main("a.txt");
    rig.mcp_answers("deploy_status", &[&format!("rev={} Healthy", &a[..12])]);
    let (ok, out) = rig.once(&[]);
    assert!(!ok, "{}", out);
    assert!(out.contains("verification failed") && out.contains("could not be read to make sure no newer commit is on its way"), "{}", out);
    assert!(!rig.mcp_calls().contains(r#""name":"rollback""#), "a rollback is not something to do on a guess:\n{}", rig.mcp_calls());
}

#[test]
fn a_smoke_test_that_gives_no_answer_is_not_a_failed_deployment() {
    // The check is killed before it can say anything. That is a broken check, not a bad deployment.
    let rig = Rig::new("verify-killed", MCP_SERVER, &DEPLOY_STAGE.replace("test -f README", "kill -9 $$"));
    rig.adopt();
    let a = rig.merge_to_main("a.txt");
    rig.mcp_answers("deploy_status", &[&format!("rev={} Healthy", &a[..12])]);
    let (ok, out) = rig.once(&[]);
    assert!(!ok, "{}", out);
    assert!(out.contains("UNKNOWN") && out.contains("verification gave no result") && out.contains("Nothing was rolled back"), "{}", out);
    assert!(!rig.mcp_calls().contains(r#""name":"rollback""#), "{}", rig.mcp_calls());
    let saved = fs::read_to_string(rig.state_file()).unwrap();
    assert!(!saved.contains(&format!(r#""last_good": "{}""#, a)), "an unverified commit is not a rollback target:\n{}", saved);
}

#[test]
fn a_baseline_that_is_not_confirmed_healthy_is_never_rolled_back_to() {
    let rig = Rig::new("baseline", MCP_SERVER, &DEPLOY_STAGE.replace("test -f README", "test -f smoke-ok"));
    let start = git(&rig.clone(), &["rev-parse", "HEAD"]);
    // On first start the environment turns out to be running some other commit.
    rig.mcp_answers("deploy_status", &["rev=0123456789ab Healthy"]);
    let (ok, out) = rig.once(&[]);
    assert!(ok, "{}", out);
    assert!(out.contains("it is running a different commit") && out.contains("nothing to roll back to"), "{}", out);
    let saved = fs::read_to_string(rig.state_file()).unwrap();
    assert!(saved.contains(r#""last_good": """#) && !saved.contains(&format!(r#""last_good": "{}""#, start)), "{}", saved);

    // The next deployment fails its smoke test. There is no known-good commit, so no rollback.
    let a = rig.merge_to_main("a.txt");
    rig.mcp_answers("deploy_status", &[&format!("rev={} Healthy", &a[..12])]);
    let (ok, out) = rig.once(&[]);
    assert!(!ok, "{}", out);
    assert!(out.contains("No earlier good commit is known"), "{}", out);
    assert!(!rig.mcp_calls().contains(r#""name":"rollback""#), "{}", rig.mcp_calls());

    // Once a deployment has succeeded, that commit is the target.
    fs::write(rig.clone().join("smoke-ok"), "").unwrap();
    let b = rig.merge_to_main("b.txt");
    rig.mcp_answers("deploy_status", &[&format!("rev={} Healthy", &b[..12])]);
    let (ok, out) = rig.once(&[]);
    assert!(ok, "{}", out);
    assert!(fs::read_to_string(rig.state_file()).unwrap().contains(&format!(r#""last_good": "{}""#, b)));
}

#[test]
fn a_deployment_that_cannot_be_recorded_is_not_reported_as_a_clean_success() {
    let rig = Rig::new("unrecorded", MCP_SERVER, DEPLOY_STAGE);
    let start = rig.adopt();
    let a = rig.merge_to_main("a.txt");
    // While the deployment runs, the state file becomes unwritable (here: replaced by a directory).
    let state = rig.state_file();
    fs::write(rig.root.join("mcp/on_deploy.sh"), format!("rm -f {p} && mkdir {p}", p = state.display())).unwrap();
    rig.mcp_answers("deploy_status", &[&format!("rev={} Healthy", &a[..12])]);
    let (ok, out) = rig.once(&[]);
    assert!(!ok, "the run must not exit clean when its record is wrong:\n{}", out);
    assert!(out.contains(&format!("{} is deployed to staging", &a[..10])) && out.contains("COULD NOT BE RECORDED"), "{}", out);
    assert!(rig.mcp_calls().contains("COULD NOT BE RECORDED"), "the notification says so too:\n{}", rig.mcp_calls());
    let _ = start;
}

#[test]
fn a_ci_pass_that_belongs_to_the_previous_push_is_not_taken_for_the_fix() {
    let stage = CI_STAGE.replace("failure = \"FAILED\"", "failure = \"FAILED\"\nrevision = 'rev=([0-9a-f]+)'");
    let rig = Rig::new("stale-pass", MCP_SERVER, &stage);
    rig.issue();
    // First push fails. After the fix is pushed, the tool still answers about the first push for a
    // while, and what it says about it is "passed" (a re-run, say). Only then does it get to the fix.
    rig.mcp_answers("ci_status", &["rev=@SHA@ FAILED run=1", "rev=@FIRST@ PASSED", "rev=@FIRST@ PASSED", "rev=@SHA@ RUNNING", "rev=@SHA@ PASSED"]);
    rig.mcp_answers("ci_logs", &["error: broken"]);
    let (ok, out) = rig.once(&[]);
    assert!(ok, "{}", out);
    assert_eq!(git(&rig.origin(), &["rev-list", "--count", "main..forkdeck/issue-7"]), "2");
    assert_eq!(rig.mcp_calls().matches(r#""name":"ci_status""#).count(), 5, "it kept waiting for the fix's own result:\n{}", rig.mcp_calls());
}
