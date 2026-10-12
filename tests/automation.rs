//! End-to-end tests of the unattended paths, run against the built binary with stand-ins for
//! `claude`, `gh` and an MCP server, and a local bare repository as the remote. They need `git` and `sh`.

use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const CLAUDE: &str = r#"#!/bin/sh
[ "$1" = "--version" ] && { echo stand-in; exit 0; }
[ -n "$FAKE_ARGS" ] && echo "$*" >> "$FAKE_ARGS"
prompt=$(cat)
[ -n "$FAKE_PROMPTS" ] && printf '%s\n-----\n' "$prompt" >> "$FAKE_PROMPTS"
case "$prompt" in
  *"<options>"*)
    printf '%s\n' '{"type":"result","is_error":false,"result":"Two ways.\n<options>{\"options\": [{\"title\": \"Patch it\", \"summary\": \"small\"}, {\"title\": \"Rewrite it\", \"summary\": \"clean\", \"recommended\": true}]}</options>"}'
    ;;
  *"Do not modify any file"*)
    [ -n "$FAKE_DURING_REVIEW" ] && PROMPT="$prompt" sh -c "$FAKE_DURING_REVIEW" >/dev/null 2>&1
    # Reviews are counted: FAKE_VERDICTS gives one verdict per review in turn (PASS after the
    # list), FAKE_REVIEW_CRASH=1 crashes the first review (=always: every one), and
    # FAKE_REVIEW_NOVERDICT=1 leaves the verdict out of the first.
    n=$(cat "$FAKE_GH/reviews" 2>/dev/null || echo 0); n=$((n + 1)); echo "$n" > "$FAKE_GH/reviews"
    [ "$FAKE_REVIEW_CRASH" = always ] && { echo "reviewer crashed" >&2; exit 1; }
    [ "$n" = 1 ] && [ -n "$FAKE_REVIEW_CRASH" ] && { echo "reviewer crashed" >&2; exit 1; }
    if [ "$n" = 1 ] && [ -n "$FAKE_REVIEW_NOVERDICT" ]; then
      printf '%s\n' '{"type":"result","is_error":false,"result":"Looked at it, it seems fine."}'
      exit 0
    fi
    v=${FAKE_VERDICT:-PASS}
    [ -n "$FAKE_VERDICTS" ] && { v=$(echo "$FAKE_VERDICTS" | cut -d' ' -f"$n"); [ -n "$v" ] || v=PASS; }
    printf '%s\n' "{\"type\":\"result\",\"is_error\":false,\"result\":\"Looked at it.\\nVERDICT: $v\"}"
    ;;
  *)
    sh -c "${FAKE_WORK:-echo change >> by_agent.txt}" >/dev/null 2>&1
    printf '%s\n' '{"type":"result","is_error":false,"result":"Did the work."}'
    ;;
esac
"#;

/// `copilot`, speaking its JSONL. `FAKE_COPILOT_REJECT` lists models to refuse the way GitHub does
/// for a model the organisation has switched off, and `FAKE_COPILOT_TRUNCATE` makes a review stop
/// before its closing line. Every run's arguments are appended to `$FAKE_GH/copilot.log`.
const COPILOT: &str = r#"#!/bin/sh
[ "$1" = "--version" ] && { echo "GitHub Copilot CLI stand-in"; exit 0; }
[ "$1" = "--headless" ] && exit 1
echo "$*" >> "$FAKE_GH/copilot.log"
model=""; usage=""
while [ $# -gt 0 ]; do
  case "$1" in --model) model=$2; shift 2 ;; --usage-output-file) usage=$2; shift 2 ;; *) shift ;; esac
done
prompt=$(cat)
for m in $FAKE_COPILOT_REJECT; do
  if [ "$m" = "$model" ]; then
    printf '%s\n' "{\"type\":\"model.call_failure\",\"data\":{\"model\":\"$model\",\"statusCode\":400,\"errorCode\":\"model_not_supported\"}}"
    printf '%s\n' '{"type":"session.error","data":{"errorType":"query","message":"400 The requested model is not supported."}}'
    printf '%s\n' '{"type":"result","exitCode":1,"usage":{"premiumRequests":0}}'
    exit 1
  fi
done
[ -n "$usage" ] && printf '{"modelMetrics":{"%s":{"usage":{"inputTokens":10,"outputTokens":2}}}}' "$model" > "$usage"
case "$prompt" in
  *"Do not modify any file"*)
    verdict=${FAKE_VERDICT:-PASS}
    [ -n "$model" ] && [ "$model" = "$FAKE_COPILOT_FAIL_MODEL" ] && verdict=FAIL
    printf '%s\n' "{\"type\":\"assistant.message\",\"data\":{\"content\":\"Looked at it.\\nVERDICT: $verdict\",\"model\":\"$model\"}}"
    [ "$FAKE_COPILOT_TRUNCATE" = usage ] && printf '%s\n' "{\"type\":\"assistant.usage\",\"data\":{\"model\":\"$model\",\"inputTokens\":10,\"outputTokens\":2}}"
    [ -n "$FAKE_COPILOT_TRUNCATE" ] && exit 0
    ;;
  *)
    printf '%s\n' '{"type":"tool.execution_start","data":{"toolName":"edit","arguments":{"path":"by_agent.txt"}}}'
    sh -c "${FAKE_WORK:-echo change >> by_agent.txt}" >/dev/null 2>&1
    printf '%s\n' "{\"type\":\"assistant.message\",\"data\":{\"content\":\"Did the work.\",\"model\":\"$model\"}}"
    ;;
esac
printf '%s\n' '{"type":"result","exitCode":0,"usage":{"premiumRequests":1}}'
"#;

/// `gh`. Lists come from files, everything else is appended to `calls.log`. `gh api` keeps
/// comments as files under `$FAKE_GH/c/<path>/<id>.json` (what was posted), `.who` (the author),
/// `.at` (when it last changed) and optionally `.assoc` (how the forge says the author relates to
/// the repository; a member unless stated), which is all the team feature reads and writes.
const GH: &str = r#"#!/bin/sh
[ "$1" = "--version" ] && { echo stand-in; exit 0; }
if [ "$1" = "api" ]; then
  shift; method=GET; path=""; input=""
  while [ $# -gt 0 ]; do
    case "$1" in
      -X) method=$2; shift 2 ;;
      --input) input=$(cat); shift 2 ;;
      --jq) shift 2 ;;
      --paginate) shift ;;
      *) path=$1; shift ;;
    esac
  done
  echo "api $method $path" >> "$FAKE_GH/calls.log"
  if [ -f "$FAKE_GH/fail_once" ]; then
    pat=$(cat "$FAKE_GH/fail_once")
    case "$method $path" in *$pat*) rm "$FAKE_GH/fail_once"; echo "HTTP 502: bad gateway" >&2; exit 1 ;; esac
  fi
  if [ -n "$FAKE_GH_API_FAIL" ]; then case "$method $path" in *$FAKE_GH_API_FAIL*) echo "HTTP 502: bad gateway" >&2; exit 1 ;; esac; fi
  bare=${path%%\?*}
  key=$(printf '%s' "$bare" | tr '/' '_')
  case "$method $path" in
    "GET user") echo "${FAKE_LOGIN:-alice}" ;;
    "GET repos/o/r") cat "$FAKE_GH/visibility" 2>/dev/null || echo private ;;
    GET\ repos/*/commits\?*) cat "$FAKE_GH/commits.json" 2>/dev/null || echo "[]" ;;
    GET\ *)
      d="$FAKE_GH/c/$key"; printf '['; sep=""
      for id in $(ls "$d" 2>/dev/null | sed -n 's/\.json$//p' | sort -n); do
        printf '%s{"id":%s,"user":{"login":"%s"},"author_association":"%s","updated_at":"%s",%s' "$sep" "$id" "$(cat "$d/$id.who")" "$(cat "$d/$id.assoc" 2>/dev/null || echo MEMBER)" "$(cat "$d/$id.at")" "$(sed 's/^{//' "$d/$id.json")"
        sep=","
      done
      printf ']\n' ;;
    POST\ *)
      if [ -f "$FAKE_GH/on_post.sh" ]; then mv "$FAKE_GH/on_post.sh" "$FAKE_GH/ran_post.sh"; sh "$FAKE_GH/ran_post.sh" >/dev/null 2>&1; fi
      d="$FAKE_GH/c/$key"; mkdir -p "$d"
      n=$(cat "$FAKE_GH/next_id" 2>/dev/null || echo 100); echo $((n + 1)) > "$FAKE_GH/next_id"
      printf '%s' "$input" > "$d/$n.json"; printf '%s' "${FAKE_LOGIN:-alice}" > "$d/$n.who"; date -u +%Y-%m-%dT%H:%M:%SZ > "$d/$n.at"
      echo "{\"id\":$n}" ;;
    PATCH\ *)
      id=${bare##*/}
      for f in "$FAKE_GH"/c/*/"$id.json"; do
        [ -f "$f" ] || { echo "HTTP 404" >&2; exit 1; }
        printf '%s' "$input" > "$f"; date -u +%Y-%m-%dT%H:%M:%SZ > "${f%.json}.at"
      done
      echo "{\"id\":$id}" ;;
  esac
  exit 0
fi
case "$1 $2" in
  "issue view"|"pr view") cat "$FAKE_GH/view-$3.json" 2>/dev/null || echo '{"labels":[]}' ;;
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
        Rig::with_agents(name, extra_config, repo_config, &[("claude", CLAUDE)])
    }

    /// Like `new`, with these agent stand-ins on PATH instead of `claude`.
    fn with_agents(name: &str, extra_config: &str, repo_config: &str, agents: &[(&str, &str)]) -> Rig {
        let root = std::env::temp_dir().join(format!("forkdeck-test-{}-{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for d in ["bin", "gh", "mcp", "home/.config/forkdeck", "home/.local/state/forkdeck"] {
            fs::create_dir_all(root.join(d)).unwrap();
        }
        fs::write(root.join("home/.local/state/forkdeck/machine-id"), "0123456789abcdef\n").unwrap();
        for (file, body) in agents.iter().copied().chain([("gh", GH), ("mcp-stand-in", MCP)]) {
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
        let mut c = self.bare(env);
        c.arg("once");
        c
    }

    /// Run forkdeck with other arguments and return (exit ok, output).
    fn run(&self, args: &[&str], env: &[(&str, &str)]) -> (bool, String) {
        let out = self.bare(env).args(args).output().expect("forkdeck runs");
        (out.status.success(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
    }

    fn bare(&self, env: &[(&str, &str)]) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_forkdeck"));
        c.current_dir(self.clone())
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

// ---------------------------------------------------------------------------------------------
// Team: claims and status comments on the forge

const TEAM: &str = "[team]\nenabled = true";
/// The made-up machine id every rig starts with.
const MACHINE: &str = "0123456789abcdef";

/// Seconds since the epoch as the forge would print them.
fn forge_time(ago_secs: i64) -> String {
    let out = Command::new("date").args(["-u", "-d", &format!("@{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64 - ago_secs), "+%Y-%m-%dT%H:%M:%SZ"]).output().unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

impl Rig {
    fn comment_dir(&self, what: &str) -> PathBuf {
        self.root.join("gh/c").join(format!("repos_o_r_{}_comments", what))
    }

    /// Put a claim comment on the forge as somebody's forkdeck would have left it.
    fn seed_claim(&self, what: &str, id: u64, author: &str, marker: serde_json::Value, ago_secs: i64) {
        let d = self.comment_dir(what);
        fs::create_dir_all(&d).unwrap();
        let body = format!("<!-- forkdeck:claim {} -->\n**forkdeck** · somebody is on this", marker);
        fs::write(d.join(format!("{}.json", id)), serde_json::json!({ "body": body }).to_string()).unwrap();
        fs::write(d.join(format!("{}.who", id)), author).unwrap();
        fs::write(d.join(format!("{}.at", id)), forge_time(ago_secs)).unwrap();
    }

    /// How the forge describes the author of a comment: MEMBER, COLLABORATOR, CONTRIBUTOR, NONE…
    fn assoc(&self, what: &str, id: u64, assoc: &str) {
        fs::write(self.comment_dir(what).join(format!("{}.assoc", id)), assoc).unwrap();
    }

    /// The labels `gh issue view` / `gh pr view` reports for an item.
    fn labelled(&self, n: u64, label: &str) {
        fs::write(self.root.join(format!("gh/view-{}.json", n)), format!(r#"{{"labels":[{{"name":"{}"}}]}}"#, label)).unwrap();
    }

    /// A script the stand-in agent can run mid-job: `$1` posts a release naming the running job's
    /// claim on issue 7, and the forge describes that account as `$2`.
    fn release_script(&self) -> String {
        let d = self.comment_dir("issues_7");
        let p = self.root.join("bin/post-release");
        let body = r#"#!/bin/sh
d=@DIR@
id=$(sed -n 's/.*\\"id\\":\\"\([0-9a-f-]*\)\\".*/\1/p' "$d/100.json")
printf '{"body":"<!-- forkdeck:claim {\\"v\\":1,\\"id\\":\\"rel\\",\\"by\\":\\"%s\\",\\"state\\":\\"released\\",\\"ends\\":[\\"%s\\"]} -->\\nreleased"}' "$1" "$id" > "$d/150.json"
printf '%s' "$1" > "$d/150.who"; printf '%s' "$2" > "$d/150.assoc"; date -u +%Y-%m-%dT%H:%M:%SZ > "$d/150.at"
"#
        .replace("@DIR@", &d.display().to_string());
        fs::write(&p, body).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
        p.display().to_string()
    }

    /// The comments on an item, oldest first, as the text that was posted.
    fn comments(&self, what: &str) -> Vec<String> {
        let d = self.comment_dir(what);
        let mut ids: Vec<u64> = fs::read_dir(&d).map(|r| r.filter_map(|e| e.ok()?.file_name().to_str()?.strip_suffix(".json")?.parse().ok()).collect()).unwrap_or_default();
        ids.sort();
        ids.iter().map(|i| serde_json::from_str::<serde_json::Value>(&fs::read_to_string(d.join(format!("{}.json", i))).unwrap()).unwrap()["body"].as_str().unwrap().to_string()).collect()
    }
}

fn claim(id: &str, by: &str, host: &str, pid: u32) -> serde_json::Value {
    serde_json::json!({ "v": 1, "id": id, "by": by, "host": host, "pid": pid, "agent": "claude", "state": "active", "stage": "working", "started": 1, "beat": 1 })
}

#[test]
fn a_claimed_issue_carries_one_status_comment_and_only_metadata() {
    let rig = Rig::new("team-claim", TEAM, "");
    rig.issue();
    let (ok, out) = rig.once(&[]);
    assert!(ok, "{}", out);
    let comments = rig.comments("issues_7");
    assert_eq!(comments.len(), 1, "one comment, edited in place: {:?}", comments);
    let c = &comments[0];
    assert!(c.contains(r#""by":"alice""#) && c.contains(&format!(r#""machine":"{}""#, MACHINE)) && c.contains(r#""state":"done""#), "{}", c);
    assert!(c.contains("https://example.invalid/o/r/pull/12"), "the pull request is linked: {}", c);
    assert!(!c.contains("Did the work") && !c.contains("Please add it") && !c.contains("Looked at it"), "no prompt and no agent output may be shared: {}", c);
    let calls = rig.calls();
    let said = calls.find("api POST repos/o/r/issues/7/comments").expect("the claim is posted");
    let labelled = calls.find("--add-label agent:working").expect("the label moves");
    assert!(said < labelled, "the claim is on the forge before the label moves:\n{}", calls);
    assert!(calls.contains("issue edit 7 -R o/r --add-assignee @me"), "{}", calls);
    assert!(calls.contains("pr create"), "{}", calls);
}

#[test]
fn sharing_is_off_unless_asked_for() {
    let rig = Rig::new("team-off", "", "");
    rig.issue();
    let (ok, out) = rig.once(&[]);
    assert!(ok, "{}", out);
    assert!(!rig.calls().contains("api "), "nothing extra is said on the forge:\n{}", rig.calls());
    assert!(!rig.calls().contains("--add-assignee"));
}

#[test]
fn an_issue_a_teammate_holds_is_left_alone() {
    let rig = Rig::new("team-held", TEAM, "");
    rig.issue();
    rig.seed_claim("issues_7", 50, "bob", claim("b1", "bob", "bobs-box", 1), 60);
    let (ok, out) = rig.once(&[]);
    assert!(ok, "leaving a teammate's issue alone is not a failure:\n{}", out);
    assert!(out.contains("not started: bob on bobs-box is already on it"), "{}", out);
    let calls = rig.calls();
    assert!(!calls.contains("label") && !calls.contains("pr create") && !calls.contains("POST") && !calls.contains("issue comment"), "nothing on the forge may change:\n{}", calls);
    assert_eq!(git(&rig.origin(), &["branch", "--list", "forkdeck/*"]), "");
}

#[test]
fn a_claim_that_went_quiet_or_was_forged_does_not_block() {
    let rig = Rig::new("team-stale", TEAM, "");
    rig.issue();
    // Bob's machine stopped updating two hours ago; Mallory typed a marker naming Carol.
    rig.seed_claim("issues_7", 50, "bob", claim("b1", "bob", "bobs-box", 1), 7200);
    rig.seed_claim("issues_7", 51, "mallory", claim("c1", "carol", "x", 1), 5);
    // Eve signs her own name, but she only has permission to comment.
    rig.seed_claim("issues_7", 52, "eve", claim("e1", "eve", "x", 1), 5);
    rig.assoc("issues_7", 52, "NONE");
    let (ok, out) = rig.once(&[]);
    assert!(ok, "{}", out);
    assert!(rig.calls().contains("pr create"), "{}", rig.calls());
}

#[test]
fn when_two_machines_claim_at_once_the_older_comment_wins() {
    let rig = Rig::new("team-race", TEAM, "");
    rig.issue();
    // Bob's claim lands after we looked and before ours is stored: neither saw the other.
    let d = rig.comment_dir("issues_7");
    let body = serde_json::json!({ "body": format!("<!-- forkdeck:claim {} -->\nx", claim("b1", "bob", "bobs-box", 1)) }).to_string();
    fs::write(
        rig.root.join("gh/on_post.sh"),
        format!("mkdir -p {d}; cat > {d}/50.json <<'EOF'\n{body}\nEOF\nprintf bob > {d}/50.who; date -u +%Y-%m-%dT%H:%M:%SZ > {d}/50.at\n", d = d.display(), body = body),
    )
    .unwrap();
    let (ok, out) = rig.once(&[]);
    assert!(ok, "{}", out);
    assert!(out.contains("not started: bob on bobs-box is already on it"), "{}", out);
    let comments = rig.comments("issues_7");
    assert!(comments[1].contains(r#""state":"yielded""#), "our claim steps back: {:?}", comments);
    let calls = rig.calls();
    assert!(!calls.contains("pr create") && !calls.contains("agent:needs-human") && !calls.contains("issue comment") && !calls.contains("--add-assignee"), "the issue is Bob's; nothing more is done to it:\n{}", calls);
    assert_eq!(git(&rig.origin(), &["branch", "--list", "forkdeck/*"]), "");
}

#[test]
fn a_claim_left_by_a_run_that_died_on_this_machine_is_replaced() {
    let rig = Rig::new("team-own", TEAM, "");
    rig.issue();
    // Same account, same machine, and no such process any more.
    let mut left = claim("old", "alice", "", 4_000_000);
    left["machine"] = MACHINE.into();
    rig.seed_claim("issues_7", 50, "alice", left, 30);
    let (ok, out) = rig.once(&[]);
    assert!(ok, "{}", out);
    let comments = rig.comments("issues_7");
    assert!(comments[1].contains(r#""ends":["old"]"#) && comments[1].contains(r#""state":"done""#), "{:?}", comments);
    assert!(rig.calls().contains("pr create"));

    // From another machine the same account is somebody else.
    let rig = Rig::new("team-other-host", TEAM, "");
    rig.issue();
    let mut other = claim("old", "alice", "laptop", 4_000_000);
    other["machine"] = "ffffffffffffffff".into();
    rig.seed_claim("issues_7", 50, "alice", other, 30);
    let (_, out) = rig.once(&[]);
    assert!(out.contains("not started: alice on laptop is already on it"), "{}", out);
}

#[test]
fn nothing_is_claimed_when_the_claims_cannot_be_read() {
    let rig = Rig::new("team-unreadable", TEAM, "");
    rig.issue();
    let (ok, out) = rig.once(&[("FAKE_GH_API_FAIL", "GET repos/o/r/issues/7/comments")]);
    assert!(!ok, "{}", out);
    assert!(out.contains("could not prepare issue: team:"), "{}", out);
    let calls = rig.calls();
    assert!(!calls.contains("label") && !calls.contains("POST"), "a teammate may hold it; it must stay untouched:\n{}", calls);
}

#[test]
fn a_teammates_release_stops_the_job_before_anything_is_pushed() {
    let rig = Rig::new("team-release", TEAM, "");
    rig.issue();
    rig.labelled(7, "agent:working");
    // While Alice's agent is working, Bob hands the issue back to the queue from his machine.
    let release = format!("echo change >> by_agent.txt; FAKE_LOGIN=bob {} team release o/r#7 --force > {}/release.out 2>&1", env!("CARGO_BIN_EXE_forkdeck"), rig.root.display());
    let (ok, out) = rig.once(&[("FAKE_WORK", &release)]);
    let said = fs::read_to_string(rig.root.join("release.out")).unwrap_or_default();
    assert!(said.contains("ended the claim of alice") && said.contains("label back to agent:ready"), "{}", said);
    assert!(!ok && out.contains("bob has taken this over"), "{}", out);
    let calls = rig.calls();
    assert!(!calls.contains("pr create") && !calls.contains("agent:needs-human") && !calls.contains("issue comment"), "the issue is no longer ours to report on:\n{}", calls);
    assert_eq!(git(&rig.origin(), &["branch", "--list", "forkdeck/*"]), "", "nothing was pushed");
    let comments = rig.comments("issues_7");
    assert!(comments[0].contains(r#""state":"released""#) && comments[0].contains(r#""by":"alice""#), "{:?}", comments);
    assert!(comments[1].contains(r#""by":"bob""#) && comments[1].contains(r#""ends":["#), "{:?}", comments);
}

#[test]
fn a_fresh_claim_is_only_released_with_force_and_a_stale_one_without() {
    let rig = Rig::new("team-force", TEAM, "");
    rig.labelled(7, "agent:working");
    rig.labelled(9, "agent:working");
    rig.seed_claim("issues_7", 50, "bob", claim("b1", "bob", "bobs-box", 1), 60);
    let (ok, out) = rig.run(&["team", "release", "o/r#7"], &[]);
    assert!(!ok && out.contains("bob on bobs-box is already on it") && out.contains("--force"), "{}", out);
    assert!(!rig.calls().contains("label") && !rig.calls().contains("POST"), "{}", rig.calls());

    rig.seed_claim("issues_9", 60, "bob", claim("b2", "bob", "bobs-box", 1), 7200);
    let (ok, out) = rig.run(&["team", "release", "o/r#9"], &[]);
    assert!(ok && out.contains("ended the claim of bob"), "{}", out);
    assert!(rig.calls().contains("issue edit 9 -R o/r --remove-label agent:working --add-label agent:ready"), "{}", rig.calls());
    assert!(rig.comments("issues_9")[1].contains(r#""ends":["b2"]"#));

    // Claimed by hand, or by a forkdeck with sharing off: only moved when that is said.
    rig.labelled(5, "review:running");
    let before = rig.calls();
    let (ok, out) = rig.run(&["team", "release", "o/r!5"], &[]);
    assert!(!ok && out.contains("no forkdeck claim is open") && out.contains("--unclaimed"), "{}", out);
    assert_eq!(rig.calls().matches("label").count(), before.matches("label").count(), "nothing may change");
    let (ok, out) = rig.run(&["team", "release", "o/r!5", "--unclaimed"], &[]);
    assert!(ok && out.contains("no forkdeck claim was open") && out.contains("label back to review:ready"), "{}", out);
    let (ok, out) = rig.run(&["team", "release", "other/repo#1"], &[]);
    assert!(!ok && out.contains("not one of the configured"), "{}", out);
}

#[test]
fn a_reviewed_pull_request_carries_a_status_comment_too() {
    let rig = Rig::new("team-pr", TEAM, "");
    rig.pull_request(5, "feat");
    rig.list_pull_request(5, "feat");
    let (ok, out) = rig.once(&[]);
    assert!(ok, "{}", out);
    let comments = rig.comments("issues_5");
    assert_eq!(comments.len(), 1, "{:?}", comments);
    assert!(comments[0].contains(r#""state":"done""#) && comments[0].contains("reviewing this pull request"), "{:?}", comments);
    assert!(rig.calls().contains("--add-label review:passed"));
    assert!(!rig.calls().contains("--add-assignee"), "reviewing is not owning");

    // A teammate is already reviewing: leave it.
    let rig = Rig::new("team-pr-held", TEAM, "");
    rig.pull_request(5, "feat");
    rig.list_pull_request(5, "feat");
    rig.seed_claim("issues_5", 50, "bob", claim("b1", "bob", "bobs-box", 1), 60);
    let (ok, out) = rig.once(&[]);
    assert!(ok && out.contains("not started: bob"), "{}", out);
    assert!(!rig.calls().contains("label") && !rig.calls().contains("pr comment"), "{}", rig.calls());
}

#[test]
fn a_deployment_is_announced_on_its_commit() {
    let rig = Rig::new("team-deploy", &format!("{}\n{}", TEAM, MCP_SERVER), DEPLOY_STAGE);
    rig.adopt();
    let a = rig.merge_to_main("a.txt");
    rig.mcp_answers("deploy_status", &[&format!("rev={} Healthy", &a[..12])]);
    let (ok, out) = rig.once(&[]);
    assert!(ok, "{}", out);
    let comments = rig.comments(&format!("commits_{}", a));
    assert_eq!(comments.len(), 1, "{:?}\n{}", comments, out);
    assert!(comments[0].contains(r#""state":"done""#) && comments[0].contains("deploying this commit"), "{:?}", comments);
}

#[test]
fn the_overview_lists_who_holds_what() {
    let rig = Rig::new("team-list", TEAM, "");
    rig.issue();
    rig.seed_claim("issues_7", 50, "bob", claim("b1", "bob", "bobs-box", 1), 7200);
    let (ok, out) = rig.run(&["team"], &[]);
    assert!(ok, "{}", out);
    assert!(out.contains("bob on bobs-box") && out.contains("r#7") && out.contains("working") && out.contains("Add a change file"), "{}", out);
    assert!(out.contains("no update for 2h 0m · forkdeck team release o/r#7"), "{}", out);
    assert!(!rig.calls().contains("POST") && !rig.calls().contains("label"), "looking changes nothing:\n{}", rig.calls());

    let (ok, out) = rig.run(&["team", "--json"], &[("FAKE_LOGIN", "bob")]);
    assert!(ok, "{}", out);
    let rows: serde_json::Value = serde_json::from_str(&out).expect("json");
    assert_eq!(rows[0]["who"], "bob");
    assert_eq!(rows[0]["mine"], true);
    assert_eq!(rows[0]["stale"], true);
}

#[test]
fn an_issue_with_an_open_pull_request_is_not_handed_back() {
    let rig = Rig::new("team-pr-open", TEAM, "");
    rig.labelled(7, "agent:working");
    let mut m = claim("b1", "bob", "bobs-box", 1);
    m["pr"] = "https://example.invalid/o/r/pull/12".into();
    m["stage"] = "watching ci".into();
    rig.seed_claim("issues_7", 50, "bob", m, 7200);
    let (ok, out) = rig.run(&["team", "release", "o/r#7", "--force"], &[]);
    assert!(!ok && out.contains("already opened https://example.invalid/o/r/pull/12"), "{}", out);
    assert!(!rig.calls().contains("label") && !rig.calls().contains("POST"), "{}", rig.calls());
}

fn this_host() -> String {
    String::from_utf8_lossy(&Command::new("hostname").output().expect("hostname runs").stdout).trim().to_string()
}

#[test]
fn no_hostname_is_posted_on_a_public_repository() {
    let host = this_host();
    assert!(!host.is_empty());
    let named = format!(r#""host":"{}""#, host);

    // Private: teammates see which machine it is.
    let rig = Rig::new("team-private", TEAM, "");
    rig.issue();
    let (ok, out) = rig.once(&[]);
    assert!(ok, "{}", out);
    assert!(rig.comments("issues_7")[0].contains(&named), "{:?}", rig.comments("issues_7"));

    // Public, and a forge that gives no usable answer: nothing about the machine but its made-up id.
    for (name, visibility) in [("team-public", "public"), ("team-unknown", "{\"message\":\"Not Found\"}")] {
        let rig = Rig::new(name, TEAM, "");
        fs::write(rig.root.join("gh/visibility"), visibility).unwrap();
        rig.issue();
        // A claim an earlier run on this machine left behind is still recognised, by the id alone.
        let mut left = claim("old", "alice", "", 4_000_000);
        left["machine"] = MACHINE.into();
        rig.seed_claim("issues_7", 50, "alice", left, 30);
        let (ok, out) = rig.once(&[]);
        assert!(ok, "{}: {}", name, out);
        let c = rig.comments("issues_7")[1].clone();
        assert!(c.contains(r#""host":"""#) && !c.contains(&host), "{}: the hostname must not be posted: {}", name, c);
        assert!(c.contains(r#""ends":["old"]"#) && c.contains("`alice` finished"), "{}: {}", name, c);
        assert!(!out.contains(&format!("on {}", host)), "{}", out);
    }

    // A name somebody chose for the purpose is a label, not a hostname, and is used as written.
    let rig = Rig::new("team-public-named", "[team]\nenabled = true\nhost = \"runner-a\"", "");
    fs::write(rig.root.join("gh/visibility"), "public").unwrap();
    rig.issue();
    let (ok, out) = rig.once(&[]);
    assert!(ok, "{}", out);
    assert!(rig.comments("issues_7")[0].contains(r#""host":"runner-a""#));

    // And the release comment a person posts follows the same rule.
    let rig = Rig::new("team-public-release", TEAM, "");
    fs::write(rig.root.join("gh/visibility"), "public").unwrap();
    rig.labelled(7, "agent:working");
    rig.seed_claim("issues_7", 50, "bob", claim("b1", "bob", "", 1), 7200);
    let (ok, out) = rig.run(&["team", "release", "o/r#7"], &[]);
    assert!(ok, "{}", out);
    assert!(!rig.comments("issues_7")[1].contains(&host), "{:?}", rig.comments("issues_7"));
}

#[test]
fn a_release_typed_by_someone_without_write_access_is_ignored() {
    let rig = Rig::new("team-outsider", TEAM, "");
    rig.issue();
    let work = format!("echo change >> by_agent.txt; {} eve NONE", rig.release_script());
    let (ok, out) = rig.once(&[("FAKE_WORK", &work)]);
    assert!(rig.comments("issues_7").len() == 2, "the forged release was posted: {}", out);
    assert!(ok, "a passer-by's comment must not stop the job:\n{}", out);
    assert!(rig.calls().contains("pr create"), "{}", rig.calls());
    assert!(rig.comments("issues_7")[0].contains(r#""state":"done""#));

    // The very same comment from a teammate does stop it: what differs is what the forge says about the author.
    let rig = Rig::new("team-insider", TEAM, "");
    rig.issue();
    let work = format!("echo change >> by_agent.txt; {} bob COLLABORATOR", rig.release_script());
    let (ok, out) = rig.once(&[("FAKE_WORK", &work)]);
    assert!(!ok && out.contains("bob has taken this over"), "{}", out);
    assert!(!rig.calls().contains("pr create"), "{}", rig.calls());
}

#[test]
fn a_claim_that_expired_while_the_agent_ran_publishes_nothing() {
    // The machine slept: its claim was last refreshed two hours ago when the agent finishes.
    let rig = Rig::new("team-expired", TEAM, "");
    rig.issue();
    let d = rig.comment_dir("issues_7");
    let work = format!("echo change >> by_agent.txt; echo {} > {}/100.at", forge_time(7200), d.display());
    let (ok, out) = rig.once(&[("FAKE_WORK", &work)]);
    assert!(!ok && out.contains("has expired"), "{}", out);
    let calls = rig.calls();
    assert!(!calls.contains("pr create") && !calls.contains("agent:needs-human") && !calls.contains("issue comment"), "an expired claim writes nothing:\n{}", calls);
    assert_eq!(git(&rig.origin(), &["branch", "--list", "forkdeck/*"]), "");

    // Same, and meanwhile the issue was put back by hand and Bob's machine took it without a release.
    let rig = Rig::new("team-expired-taken", "[team]\nenabled = true\n[review]\nmax_fix_rounds = 0", "");
    rig.issue();
    let d = rig.comment_dir("issues_7");
    rig.seed_claim("staging", 150, "bob", claim("b1", "bob", "bobs-box", 1), 30);
    let staged = rig.comment_dir("staging");
    let work = format!("echo change >> by_agent.txt; echo {} > {d}/100.at; cp {s}/150.* {d}/", forge_time(7200), d = d.display(), s = staged.display());
    // The work also fails its review, so the failure path runs: it must stay off Bob's issue too.
    let (ok, out) = rig.once(&[("FAKE_WORK", &work), ("FAKE_VERDICT", "FAIL")]);
    assert!(!ok && out.contains("bob has taken this over"), "{}", out);
    let calls = rig.calls();
    assert!(!calls.contains("pr create") && !calls.contains("agent:needs-human") && !calls.contains("issue comment"), "the issue is Bob's:\n{}", calls);
    assert_eq!(git(&rig.origin(), &["branch", "--list", "forkdeck/*"]), "");
}

#[test]
fn a_job_that_fails_after_it_was_released_leaves_the_labels_to_the_new_holder() {
    let rig = Rig::new("team-fail-released", "[team]\nenabled = true\n[review]\nmax_fix_rounds = 0", "");
    rig.issue();
    rig.labelled(7, "agent:working");
    let release = format!("echo change >> by_agent.txt; FAKE_LOGIN=bob {} team release o/r#7 --force > {}/release.out 2>&1", env!("CARGO_BIN_EXE_forkdeck"), rig.root.display());
    let (ok, out) = rig.once(&[("FAKE_WORK", &release), ("FAKE_VERDICT", "FAIL")]);
    assert!(!ok, "{}", out);
    assert!(out.contains("not reported on the forge: bob has taken this over"), "{}", out);
    let calls = rig.calls();
    assert!(!calls.contains("agent:needs-human") && !calls.contains("issue comment"), "the failure label would overwrite the next holder's:\n{}", calls);
    assert_eq!(calls.matches("--remove-label").count(), 2, "one label move to claim, one by the release, none after:\n{}", calls);
}

#[test]
fn the_final_status_is_written_even_if_the_forge_fails_once() {
    let rig = Rig::new("team-final", TEAM, "");
    rig.pull_request(5, "feat");
    rig.list_pull_request(5, "feat");
    fs::write(rig.root.join("gh/fail_once"), "PATCH").unwrap();
    let (ok, out) = rig.once(&[]);
    assert!(ok, "{}", out);
    assert!(!rig.root.join("gh/fail_once").exists(), "an update did fail");
    assert!(rig.comments("issues_5")[0].contains(r#""state":"done""#), "a comment left active would block teammates: {:?}", rig.comments("issues_5"));
}

#[test]
fn a_finished_item_cannot_be_put_back_in_the_queue_by_a_release() {
    let rig = Rig::new("team-finished", TEAM, "");
    // Reviewed and passed; no claim is open on it and it does not carry the working label.
    rig.labelled(5, "review:passed");
    for extra in [&[][..], &["--unclaimed"][..], &["--force"][..]] {
        let mut args = vec!["team", "release", "o/r!5"];
        args.extend_from_slice(extra);
        let (ok, out) = rig.run(&args, &[]);
        assert!(!ok && out.contains("does not carry the label \"review:running\""), "{:?}: {}", extra, out);
    }
    assert!(!rig.calls().contains("label") && !rig.calls().contains("POST"), "{}", rig.calls());
}

/// Real `claude` or `codex` installed on the machine must not take part in the copilot tests.
const NO_REAL_CLIS: &str = "[agents.claude]\ncmd = \"absent-claude\"\n[agents.codex]\ncmd = \"absent-codex\"\n";

fn copilot_runs(rig: &Rig) -> Vec<String> {
    fs::read_to_string(rig.root.join("gh/copilot.log")).unwrap_or_default().lines().map(String::from).collect()
}

fn model_of(run: &str) -> &str {
    run.split("--model ").nth(1).and_then(|r| r.split(' ').next()).unwrap_or("")
}

#[test]
fn with_only_copilot_work_and_review_come_from_different_vendors() {
    let rig = Rig::with_agents("copilot-only", NO_REAL_CLIS, "", &[("copilot", COPILOT)]);
    rig.issue();
    let (ok, out) = rig.once(&[]);
    assert!(ok, "{}", out);
    let runs = copilot_runs(&rig);
    assert_eq!(runs.len(), 2, "{:?}", runs);
    // Whichever part the router gave the work, the review comes from the other vendor.
    let (work, review) = (model_of(&runs[0]), model_of(&runs[1]));
    assert!(runs[0].contains("--allow-tool write") && runs[1].contains("--deny-tool write"), "{:?}", runs);
    assert!((work.starts_with("claude-") && review.starts_with("gpt-")) || (work.starts_with("gpt-") && review.starts_with("claude-")), "{:?}", runs);
    assert!(runs.iter().all(|r| !r.contains("allow-all") && !r.contains("--yolo")), "{:?}", runs);
    assert!(rig.calls().contains("pr create"), "{}", rig.calls());
    assert_eq!(git(&rig.origin(), &["show", "--name-only", "--format=", "forkdeck/issue-7"]), "by_agent.txt");
}

#[test]
fn a_model_the_organisation_switched_off_is_skipped() {
    let rig = Rig::with_agents("copilot-refused", NO_REAL_CLIS, "", &[("copilot", COPILOT)]);
    rig.issue();
    let (ok, out) = rig.once(&[("FAKE_COPILOT_REJECT", "claude-sonnet-5.5 claude-sonnet-5 gpt-5.5 gpt-5.4")]);
    assert!(ok, "{}", out);
    let runs = copilot_runs(&rig);
    let workers: Vec<&str> = runs.iter().filter(|r| r.contains("--allow-tool write")).map(|r| model_of(r)).collect();
    let claude = ["claude-sonnet-5.5", "claude-sonnet-5", "claude-sonnet-4.6"];
    let codex = ["gpt-5.5", "gpt-5.4", "gpt-5.3-codex"];
    assert!(workers == claude || workers == codex, "each refused model is tried once, then the next: {:?}", runs);
    assert!(out.contains("is not available to this copilot account"), "{}", out);
}

#[test]
fn with_codex_missing_copilot_plays_codex() {
    let rig = Rig::with_agents("claude-and-copilot", "[agents.codex]\ncmd = \"absent-codex\"\n", "", &[("claude", CLAUDE), ("copilot", COPILOT)]);
    rig.issue();
    let (ok, out) = rig.once(&[]);
    assert!(ok, "{}", out);
    let runs = copilot_runs(&rig);
    assert_eq!(runs.len(), 1, "claude does its own part, copilot only codex's: {:?}", runs);
    assert!(model_of(&runs[0]).starts_with("gpt-"), "{}", runs[0]);
}

#[test]
fn a_copilot_profile_reviews_pull_requests_next_to_claude() {
    let extra = "[agents.codex]\ncmd = \"absent-codex\"\n[agents.copilot.profiles.second]\nvendor = \"xai\"\nmodels = { light = \"grok-4.7\", standard = \"grok-4.7\", heavy = \"grok-4.7\" }\n[review]\npr_reviewers = [\"claude\", \"copilot:second\"]";
    let rig = Rig::with_agents("copilot-profile", extra, "", &[("claude", CLAUDE), ("copilot", COPILOT)]);
    rig.pull_request(5, "feat");
    rig.list_pull_request(5, "feat");
    let (ok, out) = rig.once(&[]);
    assert!(ok, "{}", out);
    let runs = copilot_runs(&rig);
    assert_eq!(runs.len(), 1, "{:?}", runs);
    assert_eq!(model_of(&runs[0]), "grok-4.7");
    let calls = rig.calls();
    assert!(calls.contains("copilot:second") && calls.contains("--add-label review:passed"), "{}", calls);
}

#[test]
fn a_copilot_review_that_stops_early_is_not_a_pass() {
    let rig = Rig::with_agents("copilot-truncated", NO_REAL_CLIS, "", &[("copilot", COPILOT)]);
    rig.pull_request(5, "feat");
    rig.list_pull_request(5, "feat");
    let (_, out) = rig.once(&[("FAKE_COPILOT_TRUNCATE", "1")]);
    let calls = rig.calls();
    assert!(!calls.contains("review:passed"), "{}\n{}", calls, out);
    assert!(calls.contains("--add-label review:ready"), "{}\n{}", calls, out);
    assert!(out.contains("exited before reporting a result"), "it failed for the missing closing line: {}", out);
}

fn arg_after<'a>(line: &'a str, flag: &str) -> Option<&'a str> {
    let mut it = line.split(' ');
    it.find(|a| *a == flag)?;
    it.next()
}

#[test]
fn a_fix_round_continues_the_workers_own_conversation() {
    let rig = Rig::new("session", "", "");
    rig.issue();
    let log = rig.root.join("args.log");
    let (_, out) = rig.once(&[("FAKE_VERDICTS", "FAIL PASS"), ("FAKE_ARGS", log.to_str().unwrap())]);
    let runs: Vec<String> = fs::read_to_string(&log).unwrap().lines().map(String::from).collect();
    let workers: Vec<&String> = runs.iter().filter(|r| r.contains("acceptEdits")).collect();
    let reviews: Vec<&String> = runs.iter().filter(|r| r.contains("--permission-mode plan")).collect();
    assert_eq!((workers.len(), reviews.len()), (2, 2), "work, review, fix, review: {:?}\n{}", runs, out);
    let first = arg_after(workers[0], "--session-id").expect("the first run names its conversation");
    assert_eq!(arg_after(workers[1], "--resume"), Some(first), "the fix continues it: {:?}", workers);
    assert!(reviews.iter().all(|r| arg_after(r, "--resume").is_none()), "reviews start fresh: {:?}", reviews);
}


#[test]
fn a_typed_task_cut_off_by_a_stop_continues_its_own_conversation_later() {
    let rig = Rig::new("resume", "", "");
    let log = rig.root.join("args.log");
    let logged = log.to_str().unwrap().to_string();
    // The first attempt works slowly, and forkdeck is stopped while it does.
    let child = rig
        .bare(&[("FAKE_ARGS", &logged), ("FAKE_WORK", "sleep 30")])
        .args(["run", "add a change file"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("forkdeck starts");
    for _ in 0..100 {
        if fs::read_to_string(&log).map(|l| l.contains("acceptEdits")).unwrap_or(false) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    Command::new("kill").args(["-TERM", &child.id().to_string()]).status().unwrap();
    let out = child.wait_with_output().unwrap();
    let said = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(!out.status.success() && said.contains("interrupted"), "{}", said);
    assert!(!rig.clone().join("by_agent.txt").exists(), "the first attempt never finished");

    let (_, listed) = rig.run(&["history"], &[]);
    assert!(listed.contains("interrupted") && listed.contains("add a change file"), "{}", listed);

    // Next time, the task carries on in the same conversation and finishes.
    let (ok, out) = rig.run(&["resume"], &[("FAKE_ARGS", &logged)]);
    assert!(ok, "{}", out);
    let runs = fs::read_to_string(&log).unwrap();
    let workers: Vec<&str> = runs.lines().filter(|r| r.contains("acceptEdits")).collect();
    assert_eq!(workers.len(), 2, "{:?}", workers);
    let first = arg_after(workers[0], "--session-id").expect("the first attempt named its conversation");
    assert_eq!(arg_after(workers[1], "--resume"), Some(first), "{:?}", workers);
    assert!(rig.clone().join("by_agent.txt").exists());
    let (_, listed) = rig.run(&["history"], &[]);
    assert!(listed.contains("done") && !listed.contains("interrupted"), "{}", listed);
    let (ok, again) = rig.run(&["resume"], &[]);
    assert!(ok && again.contains("nothing to resume"), "{}", again);
}

#[test]
fn a_copilot_review_that_reports_usage_but_no_result_is_not_a_pass() {
    let rig = Rig::with_agents("copilot-usage-eof", NO_REAL_CLIS, "", &[("copilot", COPILOT)]);
    rig.pull_request(5, "feat");
    rig.list_pull_request(5, "feat");
    let (_, out) = rig.once(&[("FAKE_COPILOT_TRUNCATE", "usage")]);
    let calls = rig.calls();
    assert!(!calls.contains("review:passed"), "{}\n{}", calls, out);
    assert!(calls.contains("--add-label review:ready"), "{}\n{}", calls, out);
    assert!(out.contains("exited before reporting a result"), "it failed for the missing closing line: {}", out);
}

#[test]
fn every_configured_pull_request_reviewer_runs_and_a_third_fail_counts() {
    let extra = "[agents.codex]\ncmd = \"absent-codex\"\n[agents.copilot.profiles.second]\nvendor = \"xai\"\nmodels = { light = \"grok-4.7\", standard = \"grok-4.7\", heavy = \"grok-4.7\" }\n[agents.copilot.profiles.third]\nvendor = \"google\"\nmodels = { light = \"gemini-3.8-pro\", standard = \"gemini-3.8-pro\", heavy = \"gemini-3.8-pro\" }\n[review]\npr_reviewers = [\"claude\", \"copilot:second\", \"copilot:third\"]";
    let rig = Rig::with_agents("three-reviewers", extra, "", &[("claude", CLAUDE), ("copilot", COPILOT)]);
    rig.pull_request(5, "feat");
    rig.list_pull_request(5, "feat");
    let (ok, out) = rig.once(&[("FAKE_COPILOT_FAIL_MODEL", "gemini-3.8-pro")]);
    assert!(ok, "{}", out);
    let mut models: Vec<String> = copilot_runs(&rig).iter().map(|r| model_of(r).to_string()).collect();
    models.sort();
    assert_eq!(models, ["gemini-3.8-pro", "grok-4.7"], "both copilot profiles ran");
    let calls = rig.calls();
    assert!(calls.contains("copilot:third") && calls.contains("--add-label review:changes"), "{}", calls);
    assert!(!calls.contains("review:passed"), "{}", calls);
}

#[test]
fn a_pinned_copilot_model_the_account_refuses_falls_back_to_the_cli_choice() {
    let extra = format!("{}[agents.copilot]\nmodels = {{ light = \"gpt-5.5\", standard = \"gpt-5.5\", heavy = \"gpt-5.5\" }}\n[review]\npr_reviewers = [\"copilot\"]", NO_REAL_CLIS);
    let rig = Rig::with_agents("copilot-pinned-refused", &extra, "", &[("copilot", COPILOT)]);
    rig.pull_request(5, "feat");
    rig.list_pull_request(5, "feat");
    let (ok, out) = rig.once(&[("FAKE_COPILOT_REJECT", "gpt-5.5")]);
    assert!(ok, "{}", out);
    let runs = copilot_runs(&rig);
    let models: Vec<&str> = runs.iter().map(|r| model_of(r)).collect();
    assert_eq!(models, ["gpt-5.5", ""], "the refused pin is tried once, then the CLI picks: {:?}", runs);
    assert!(rig.calls().contains("--add-label review:passed"), "{}", rig.calls());
}

/// Collect a child's output as it comes, so a test can wait for a line before doing the next thing.
fn tee(child: &mut std::process::Child) -> std::sync::Arc<std::sync::Mutex<String>> {
    let seen = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    for stream in [child.stdout.take().map(|s| Box::new(s) as Box<dyn std::io::Read + Send>), child.stderr.take().map(|s| Box::new(s) as Box<dyn std::io::Read + Send>)].into_iter().flatten() {
        let seen = seen.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            let mut stream = stream;
            while let Ok(n) = stream.read(&mut buf) {
                if n == 0 {
                    break;
                }
                seen.lock().unwrap().push_str(&String::from_utf8_lossy(&buf[..n]));
            }
        });
    }
    seen
}

fn wait_until_said(seen: &std::sync::Arc<std::sync::Mutex<String>>, needle: &str) {
    for _ in 0..600 {
        if seen.lock().unwrap().contains(needle) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    panic!("never said {:?}:\n{}", needle, seen.lock().unwrap());
}

/// Start `forkdeck run` with a worker that waits on its first attempt (until the test says so)
/// and works at once on later ones. Returns once the first attempt is running.
fn run_with_slow_first_attempt(rig: &Rig, first_takes: &str) -> std::process::Child {
    let marker = rig.root.join("first-attempt");
    let work = format!("if [ -f {m} ]; then echo change >> by_agent.txt; else touch {m}; {first_takes}; fi", m = marker.display());
    let (args, prompts) = (rig.root.join("args.log"), rig.root.join("prompts.log"));
    let child = rig
        .bare(&[("FAKE_ARGS", args.to_str().unwrap()), ("FAKE_PROMPTS", prompts.to_str().unwrap()), ("FAKE_WORK", &work)])
        .args(["run", "add a change file"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("forkdeck starts");
    for _ in 0..600 {
        if marker.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(marker.exists(), "the first attempt never started");
    child
}

fn workers(rig: &Rig) -> Vec<String> {
    fs::read_to_string(rig.root.join("args.log")).unwrap_or_default().lines().filter(|l| l.contains("acceptEdits")).map(String::from).collect()
}

#[test]
fn a_message_sent_while_the_agent_works_is_taken_in_the_same_conversation() {
    let rig = Rig::new("message-queued", "", "");
    // The first attempt finishes only once the message is known to be queued.
    let go = rig.root.join("go");
    let mut child = run_with_slow_first_attempt(&rig, &format!("while [ ! -f {} ]; do sleep 0.1; done", go.display()));
    let seen = tee(&mut child);
    child.stdin.as_mut().unwrap().write_all(b"also mention it in the README\n").unwrap();
    wait_until_said(&seen, "message queued");
    fs::write(&go, "").unwrap();
    let status = child.wait().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(200));
    let said = seen.lock().unwrap().clone();
    assert!(status.success(), "{}", said);
    let w = workers(&rig);
    assert_eq!(w.len(), 2, "the first attempt finished, then the message was taken: {:?}\n{}", w, said);
    assert_eq!(arg_after(&w[1], "--resume"), arg_after(&w[0], "--session-id"), "{:?}", w);
    let prompts = fs::read_to_string(rig.root.join("prompts.log")).unwrap();
    assert!(prompts.contains("While you were working, the user said:\nalso mention it in the README"), "{}", prompts);
    assert!(prompts.contains("Later, the user also asked (oldest first):\n- also mention it in the README"), "the reviewer knows: {}", prompts);
}

#[test]
fn a_message_sent_with_a_bang_stops_the_agent_and_is_taken_at_once() {
    let rig = Rig::new("message-now", "", "");
    // The first attempt would never finish on its own: only stopping it lets the job go on.
    let mut child = run_with_slow_first_attempt(&rig, "sleep 600");
    let seen = tee(&mut child);
    child.stdin.as_mut().unwrap().write_all(b"!use a different file name\n").unwrap();
    let status = child.wait().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(200));
    let said = seen.lock().unwrap().clone();
    assert!(status.success(), "{}", said);
    let w = workers(&rig);
    assert_eq!(w.len(), 2, "{:?}", w);
    assert_eq!(arg_after(&w[1], "--resume"), arg_after(&w[0], "--session-id"));
    let prompts = fs::read_to_string(rig.root.join("prompts.log")).unwrap();
    assert!(prompts.contains("the user said:\nuse a different file name"), "{}", prompts);
}

fn plan_runs(rig: &Rig) -> (Vec<String>, String) {
    let args = fs::read_to_string(rig.root.join("args.log")).unwrap_or_default().lines().map(String::from).collect();
    (args, fs::read_to_string(rig.root.join("prompts.log")).unwrap_or_default())
}

fn run_planned(rig: &Rig, typed: &str) -> (bool, String) {
    let (args, prompts) = (rig.root.join("args.log"), rig.root.join("prompts.log"));
    let mut child = rig
        .bare(&[("FAKE_ARGS", args.to_str().unwrap()), ("FAKE_PROMPTS", prompts.to_str().unwrap())])
        .args(["run", "--plan", "add a change file"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("forkdeck starts");
    // Typed straight away, before the plan is even there: it is kept for the choice.
    child.stdin.as_mut().unwrap().write_all(typed.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    (out.status.success(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
}

#[test]
fn a_planned_task_goes_ahead_with_the_option_picked_in_the_plans_conversation() {
    let rig = Rig::new("plan-pick", "", "");
    let (ok, out) = run_planned(&rig, "1\n");
    assert!(ok, "{}", out);
    let (args, prompts) = plan_runs(&rig);
    let plan = args.iter().find(|a| a.contains("--permission-mode plan")).expect("planned read-only");
    let work: Vec<&String> = args.iter().filter(|a| a.contains("acceptEdits")).collect();
    assert_eq!(work.len(), 1, "{:?}", args);
    assert_eq!(arg_after(work[0], "--resume"), arg_after(plan, "--session-id"), "the work continues the plan's conversation: {:?}", args);
    assert!(prompts.contains("Go ahead with this: Option 1: Patch it. small"), "{}", prompts);
    assert!(out.contains("option 1: Patch it"), "{}", out);
    assert!(rig.clone().join("by_agent.txt").exists());
}

#[test]
fn a_planned_task_can_go_its_own_way() {
    let rig = Rig::new("plan-own", "", "");
    let (ok, out) = run_planned(&rig, "keep it to one line\n");
    assert!(ok, "{}", out);
    let (_, prompts) = plan_runs(&rig);
    assert!(prompts.contains("None of the proposed options; the user wants this instead: keep it to one line"), "{}", prompts);
}

/// A decision server that asks for a plan first, whatever the task.
fn jev_wanting_a_plan() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut buf = [0u8; 65536];
            let _ = std::io::Read::read(&mut stream, &mut buf);
            let body = r#"{"answers":{"agent":{"choice":"claude","confidence":0.9},"effort":{"score":1,"confidence":0.9},"review":{"noul":0.9},"plan":{"noul":0.97}}}"#;
            let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body);
        }
    });
    url
}

#[test]
fn an_issue_the_router_wants_planned_takes_the_recommended_option_and_says_so() {
    let jev = jev_wanting_a_plan();
    let rig = Rig::new("plan-issue", &format!("[router]\nkind = \"jev\"\n[router.jev]\nbase_url = \"{}\"", jev), "");
    rig.issue();
    let log = rig.root.join("prompts.log");
    let (ok, out) = rig.once(&[("FAKE_PROMPTS", log.to_str().unwrap())]);
    assert!(ok, "{}", out);
    assert!(out.contains("took option 2: Rewrite it"), "{}", out);
    let prompts = fs::read_to_string(&log).unwrap();
    assert!(prompts.contains("Go ahead with this: Option 2: Rewrite it. clean"), "{}", prompts);
    let calls = rig.calls();
    assert!(calls.contains("Planned first: option 2: Rewrite it. Also proposed: Patch it."), "{}", calls);
}

fn reviews_run(rig: &Rig) -> u32 {
    fs::read_to_string(rig.root.join("gh/reviews")).map(|n| n.trim().parse().unwrap_or(0)).unwrap_or(0)
}

#[test]
fn a_failed_review_goes_back_to_the_worker_until_it_passes() {
    let rig = Rig::new("fix-until-pass", "", "");
    rig.issue();
    let log = rig.root.join("args.log");
    let (ok, out) = rig.once(&[("FAKE_VERDICTS", "FAIL FAIL PASS"), ("FAKE_ARGS", log.to_str().unwrap())]);
    assert!(ok, "{}", out);
    let workers = fs::read_to_string(&log).unwrap().lines().filter(|l| l.contains("acceptEdits")).count();
    assert_eq!((workers, reviews_run(&rig)), (3, 3), "work, then a fix for each failed review: {}", out);
    assert!(out.contains("fix round 2 of 5"), "{}", out);
    assert!(rig.calls().contains("pr create"), "{}", rig.calls());
}

#[test]
fn fixing_stops_at_the_cap_and_says_so() {
    let rig = Rig::new("fix-cap", "[review]\nmax_fix_rounds = 2", "");
    rig.issue();
    let (ok, out) = rig.once(&[("FAKE_VERDICT", "FAIL")]);
    assert!(!ok, "{}", out);
    assert_eq!(reviews_run(&rig), 3);
    assert!(out.contains("review still failing after 2 fix rounds"), "{}", out);
}

#[test]
fn a_fix_that_changes_nothing_ends_the_rounds() {
    let rig = Rig::new("fix-stalled", "", "");
    rig.issue();
    // Every attempt writes the same file with the same content: after the first, nothing changes.
    let (ok, out) = rig.once(&[("FAKE_VERDICT", "FAIL"), ("FAKE_WORK", "echo same > by_agent.txt")]);
    assert!(!ok, "{}", out);
    assert_eq!(reviews_run(&rig), 1, "no second review of an unchanged tree: {}", out);
    assert!(out.contains("the last fix changed nothing"), "{}", out);
}

#[test]
fn a_reviewer_that_crashes_is_asked_again() {
    let rig = Rig::new("review-crash-once", "", "");
    rig.issue();
    let (ok, out) = rig.once(&[("FAKE_REVIEW_CRASH", "1")]);
    assert!(ok, "{}", out);
    assert!(out.contains("gave no verdict") && out.contains("asking again"), "{}", out);
    assert_eq!(reviews_run(&rig), 2);

    let rig = Rig::new("review-crash-always", "", "");
    rig.issue();
    let (ok, out) = rig.once(&[("FAKE_REVIEW_CRASH", "always")]);
    assert!(!ok && out.contains("reviewer failed"), "{}", out);
    assert_eq!(reviews_run(&rig), 2, "asked once more, not forever");
}

#[test]
fn a_review_without_a_verdict_is_asked_for_one() {
    let rig = Rig::new("review-noverdict", "", "");
    rig.issue();
    let log = rig.root.join("prompts.log");
    let (ok, out) = rig.once(&[("FAKE_REVIEW_NOVERDICT", "1"), ("FAKE_PROMPTS", log.to_str().unwrap())]);
    assert!(ok, "{}", out);
    assert!(fs::read_to_string(&log).unwrap().contains("Your previous review ended without a verdict."));
}

