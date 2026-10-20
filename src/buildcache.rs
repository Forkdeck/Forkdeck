//! Builds shared between worktrees. Every job gets a fresh worktree, so without this each one
//! compiles the project and its dependencies from nothing, and inside Codex's sandbox the
//! toolchains' own caches in the home directory are read-only.

use std::path::{Path, PathBuf};

/// The environment and writable directories one job's builds use.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Plan {
    /// Set on agent processes and checks.
    pub env: Vec<(String, String)>,
    /// Directories outside the worktree that builds write to: made writable in Codex's sandbox.
    pub writable: Vec<PathBuf>,
}

/// One toolchain: the build files that show it is used, by name or extension, and where its
/// shared cache lives under the home directory. Add a toolchain by adding a line.
struct Toolchain {
    names: &'static [&'static str],
    extensions: &'static [&'static str],
    caches: &'static [&'static str],
    /// Its build and test commands, which a Claude worker may run (Bash(<command>:*)). Package
    /// runners that fetch and execute arbitrary code (npx, uvx) are deliberately absent.
    commands: &'static [&'static str],
}

const TOOLCHAINS: &[Toolchain] = &[
    // Rust's build output is redirected to one directory per repository, below.
    Toolchain { names: &["Cargo.toml"], extensions: &[], caches: &[".cargo/registry", ".cargo/git"], commands: &["cargo"] },
    Toolchain { names: &["go.mod", "go.work"], extensions: &[], caches: &[".cache/go-build", "go/pkg/mod"], commands: &["go"] },
    Toolchain { names: &["build.gradle", "build.gradle.kts", "settings.gradle", "settings.gradle.kts"], extensions: &[], caches: &[".gradle"], commands: &["gradle", "./gradlew"] },
    Toolchain { names: &["pom.xml"], extensions: &[], caches: &[".m2"], commands: &["mvn", "./mvnw"] },
    Toolchain { names: &["build.sbt"], extensions: &[], caches: &[".sbt", ".ivy2", ".cache/coursier"], commands: &["sbt"] },
    Toolchain { names: &["global.json", "Directory.Build.props"], extensions: &[".csproj", ".fsproj", ".vbproj", ".sln"], caches: &[".nuget/packages"], commands: &["dotnet"] },
    Toolchain { names: &["Package.swift"], extensions: &[], caches: &[".cache/org.swift.swiftpm", ".swiftpm"], commands: &["swift"] },
    Toolchain { names: &["stack.yaml"], extensions: &[], caches: &[".stack"], commands: &["stack"] },
    Toolchain { names: &["cabal.project"], extensions: &[".cabal"], caches: &[".cabal", ".cache/cabal", ".local/state/cabal"], commands: &["cabal"] },
    Toolchain { names: &["dune-project"], extensions: &[], caches: &[".cache/dune", ".opam"], commands: &["dune"] },
    Toolchain { names: &["build.zig", "build.zig.zon"], extensions: &[], caches: &[".cache/zig"], commands: &["zig"] },
    Toolchain { names: &["dub.json", "dub.sdl"], extensions: &[], caches: &[".dub"], commands: &["dub"] },
    Toolchain { names: &[], extensions: &[".nimble"], caches: &[".nimble", ".cache/nim"], commands: &["nimble", "nim"] },
    Toolchain { names: &["shard.yml"], extensions: &[], caches: &[".cache/crystal", ".cache/shards"], commands: &["crystal", "shards"] },
    Toolchain { names: &["mix.exs"], extensions: &[], caches: &[".mix", ".hex"], commands: &["mix"] },
    Toolchain { names: &["rebar.config"], extensions: &[], caches: &[".cache/rebar3"], commands: &["rebar3"] },
    Toolchain { names: &["pubspec.yaml"], extensions: &[], caches: &[".pub-cache"], commands: &["dart", "flutter"] },
    Toolchain { names: &["WORKSPACE", "WORKSPACE.bazel", "MODULE.bazel"], extensions: &[], caches: &[".cache/bazel"], commands: &["bazel", "bazelisk"] },
    Toolchain { names: &["package.json"], extensions: &[], caches: &[".npm", ".cache/yarn", ".local/share/pnpm"], commands: &["npm", "pnpm", "yarn"] },
    Toolchain { names: &["pyproject.toml", "setup.py", "setup.cfg", "requirements.txt", "Pipfile"], extensions: &[], caches: &[".cache/pip", ".cache/uv"], commands: &["pytest", "python -m pytest", "uv run pytest"] },
];

/// Build files of C, C++, Fortran and other languages built by a native compiler, which ccache speeds up.
const NATIVE: &[&str] = &["CMakeLists.txt", "Makefile", "GNUmakefile", "meson.build", "configure.ac", "configure"];
const NATIVE_COMMANDS: &[&str] = &["make", "cmake", "ctest", "meson", "ninja"];
/// Read-only git, which every worker needs to see what it changed.
const GIT_COMMANDS: &[&str] = &["git status", "git diff", "git log", "git show"];

/// The build and test commands for the toolchains in a tree, plus read-only git.
pub fn commands(files: &[String]) -> Vec<&'static str> {
    let names: Vec<&str> = files.iter().map(|f| f.rsplit('/').next().unwrap_or(f)).collect();
    let mut out: Vec<&'static str> = TOOLCHAINS.iter()
        .filter(|t| names.iter().any(|n| t.names.contains(n) || t.extensions.iter().any(|e| n.ends_with(e))))
        .flat_map(|t| t.commands.iter().copied()).collect();
    if names.iter().any(|n| NATIVE.contains(n)) {
        out.extend(NATIVE_COMMANDS);
    }
    out.extend(GIT_COMMANDS);
    out.dedup();
    out
}

/// Claude's --allowedTools value for a worktree: its build and test commands and read-only git.
pub fn claude_allowed(dir: &Path) -> String {
    commands(&tracked(dir)).iter().map(|c| format!("Bash({}:*)", c)).collect::<Vec<_>>().join(" ")
}

fn tracked(dir: &Path) -> Vec<String> {
    std::process::Command::new("git").arg("-C").arg(dir).args(["ls-files"]).stderr(std::process::Stdio::null()).output().ok()
        .filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).lines().map(str::to_string).collect()).unwrap_or_default()
}

/// A directory name for a repository: readable, and the same every time.
pub fn key(repo: &str) -> String {
    repo.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '.' { c } else { '_' }).collect()
}

/// What builds for this tree should share. `files` are its tracked paths, `root` is where
/// forkdeck keeps build output (one directory per repository under it), `home` is the user's
/// home, and `on_path` says whether a program is installed.
pub fn plan(files: &[String], repo_key: &str, root: &Path, home: &Path, on_path: impl Fn(&str) -> bool) -> Plan {
    let names: Vec<&str> = files.iter().map(|f| f.rsplit('/').next().unwrap_or(f)).collect();
    let has = |t: &Toolchain| names.iter().any(|n| t.names.contains(n) || t.extensions.iter().any(|e| n.ends_with(e)));
    let mut p = Plan::default();
    for t in TOOLCHAINS.iter().filter(|t| has(t)) {
        p.writable.extend(t.caches.iter().map(|c| home.join(c)));
    }
    if names.contains(&"Cargo.toml") {
        // Cargo tracks the workspace's own crates by path, so worktrees never see each other's
        // changes; the dependencies, most of the work, build once for all of them.
        let target = root.join(repo_key).join("cargo-target");
        p.env.push(("CARGO_TARGET_DIR".into(), target.display().to_string()));
        p.writable.push(target);
        if on_path("sccache") {
            p.env.push(("RUSTC_WRAPPER".into(), "sccache".into()));
            p.writable.push(home.join(".cache/sccache"));
        }
    }
    if names.iter().any(|n| NATIVE.contains(n)) && on_path("ccache") {
        p.env.push(("CMAKE_C_COMPILER_LAUNCHER".into(), "ccache".into()));
        p.env.push(("CMAKE_CXX_COMPILER_LAUNCHER".into(), "ccache".into()));
        p.env.push(("CCACHE_DIR".into(), home.join(".cache/ccache").display().to_string()));
        p.writable.push(home.join(".cache/ccache"));
    }
    p.writable.sort();
    p.writable.dedup();
    p
}

/// The plan for one job's builds in `dir`, shared with every other job on the same repository.
/// Empty when switched off (resources.build_cache).
pub fn for_job(hub: &crate::orchestrator::Hub, job: u32, dir: &Path) -> Plan {
    use crate::state::Source;
    if !hub.cfg.resources.build_cache || hub.cfg.demo {
        return Plan::default();
    }
    let repo = hub.job(job, |j| match &j.source {
        Source::Issue { repo, .. } | Source::Pr { repo, .. } | Source::Deploy { repo, .. } => Some(repo.clone()),
        Source::Manual => None,
    }).flatten();
    // A typed task has no repository name; its directory stands in for one.
    let name = repo.unwrap_or_else(|| dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf()).display().to_string());
    for_dir(dir, &key(&name))
}

/// The plan for a worktree, from its tracked files.
pub fn for_dir(dir: &Path, repo_key: &str) -> Plan {
    let files = tracked(dir);
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()));
    let root = home.join(".cache/forkdeck/build");
    plan(&files, repo_key, &root, &home, |p| std::env::var_os("PATH").is_some_and(|path| std::env::split_paths(&path).any(|d| d.join(p).is_file())))
}

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|path| std::env::split_paths(&path).any(|d| d.join(program).is_file()))
}

/// Whether Claude Code's own sandbox can run here: built in on macOS; on Linux it needs
/// bubblewrap and socat, and bubblewrap able to start (in some containers it cannot). Asked once.
pub fn sandbox_available() -> bool {
    static AVAILABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        if cfg!(target_os = "macos") {
            return true;
        }
        cfg!(target_os = "linux") && on_path("bwrap") && on_path("socat")
            && std::process::Command::new("bwrap").args(["--ro-bind", "/", "/", "--dev", "/dev", "true"])
                .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().is_ok_and(|s| s.success())
    })
}

/// Claude Code settings that run every shell command a worker starts in its sandbox, approved
/// without asking, whatever the language: writes reach only the worktree, temp files and the
/// shared build caches, the network only `domains`, and a command that fails in there is not
/// retried outside. Given on the command line, a repository's own settings cannot loosen it.
pub fn claude_sandbox(p: &Plan, domains: &[String]) -> Vec<String> {
    for d in &p.writable {
        let _ = std::fs::create_dir_all(d);
    }
    let writable: Vec<String> = p.writable.iter().map(|d| d.display().to_string()).collect();
    let settings = serde_json::json!({ "sandbox": {
        "enabled": true,
        "autoAllowBashIfSandboxed": true,
        "allowUnsandboxedCommands": false,
        "failIfUnavailable": true,
        "filesystem": { "allowWrite": writable },
        "network": { "allowedDomains": domains, "strictAllowlist": true },
    }});
    vec!["--settings".into(), settings.to_string()]
}

/// Codex arguments that let its sandbox write where the builds do. Directories that do not exist
/// yet are created first, as the sandbox can only open up what is there.
pub fn codex_args(p: &Plan) -> Vec<String> {
    if p.writable.is_empty() {
        return vec![];
    }
    for d in &p.writable {
        let _ = std::fs::create_dir_all(d);
    }
    let list: Vec<String> = p.writable.iter().map(|d| format!("{:?}", d.display().to_string())).collect();
    vec!["-c".into(), format!("sandbox_workspace_write.writable_roots=[{}]", list.join(","))]
}

#[cfg(test)]
mod tests {
    #[test]
    fn claude_sandbox_settings_keep_writes_and_network_to_what_the_job_needs() {
        let dir = std::env::temp_dir().join(format!("forkdeck-sandbox-{}", std::process::id()));
        let p = super::Plan { env: vec![], writable: vec![dir.join("cargo-target")] };
        let args = super::claude_sandbox(&p, &["crates.io".to_string()]);
        assert_eq!(args[0], "--settings");
        let v: serde_json::Value = serde_json::from_str(&args[1]).unwrap();
        let s = &v["sandbox"];
        assert_eq!((s["enabled"].as_bool(), s["autoAllowBashIfSandboxed"].as_bool()), (Some(true), Some(true)));
        assert_eq!(s["allowUnsandboxedCommands"], false, "no retry outside the sandbox");
        assert_eq!(s["failIfUnavailable"], true, "never silently unsandboxed");
        assert_eq!(s["filesystem"]["allowWrite"][0], dir.join("cargo-target").display().to_string());
        assert_eq!((s["network"]["allowedDomains"][0].as_str(), s["network"]["strictAllowlist"].as_bool()), (Some("crates.io"), Some(true)));
        assert!(dir.join("cargo-target").is_dir(), "made first, so it can be opened up");
        std::fs::remove_dir_all(dir).unwrap();
    }

    use super::*;

    fn files(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn rust_shares_one_target_per_repository() {
        let p = plan(&files(&["Cargo.toml", "crates/a/Cargo.toml", "src/lib.rs"]), "o_r", Path::new("/c"), Path::new("/h"), |_| false);
        assert_eq!(p.env, [("CARGO_TARGET_DIR".to_string(), "/c/o_r/cargo-target".to_string())]);
        assert!(p.writable.contains(&PathBuf::from("/c/o_r/cargo-target")) && p.writable.contains(&PathBuf::from("/h/.cargo/registry")));
    }

    #[test]
    fn each_toolchain_keeps_its_own_cache_writable() {
        let tree = files(&["server/go.mod", "android/build.gradle.kts", "api/pom.xml", "web/package.json", "tools/pyproject.toml",
            "desktop/App.csproj", "ios/Package.swift", "hs/stack.yaml", "ml/dune-project", "zig/build.zig", "jvm/build.sbt",
            "ex/mix.exs", "app/pubspec.yaml", "MODULE.bazel", "d/dub.json", "lib/pkg.cabal"]);
        let p = plan(&tree, "k", Path::new("/c"), Path::new("/h"), |_| false);
        for d in [".cache/go-build", "go/pkg/mod", ".gradle", ".m2", ".npm", ".cache/pip", ".nuget/packages", ".cache/org.swift.swiftpm",
                  ".stack", ".cache/dune", ".cache/zig", ".ivy2", ".hex", ".pub-cache", ".cache/bazel", ".dub", ".cabal"] {
            assert!(p.writable.contains(&PathBuf::from("/h").join(d)), "{d}: {:?}", p.writable);
        }
        assert!(p.env.is_empty(), "their caches are already shared; nothing to redirect: {:?}", p.env);
    }

    #[test]
    fn compiler_caches_are_used_only_when_installed() {
        let tree = files(&["Cargo.toml", "engine/CMakeLists.txt"]);
        let without = plan(&tree, "k", Path::new("/c"), Path::new("/h"), |_| false);
        assert!(!without.env.iter().any(|(k, _)| k == "RUSTC_WRAPPER" || k.starts_with("CMAKE_")));
        let with = plan(&tree, "k", Path::new("/c"), Path::new("/h"), |p| p == "sccache" || p == "ccache");
        assert!(with.env.contains(&("RUSTC_WRAPPER".into(), "sccache".into())));
        assert!(with.env.contains(&("CMAKE_CXX_COMPILER_LAUNCHER".into(), "ccache".into())));
    }

    #[test]
    fn a_tree_without_build_files_changes_nothing() {
        let p = plan(&files(&["README.md", "docs/a.md"]), "k", Path::new("/c"), Path::new("/h"), |_| true);
        assert_eq!(p, Plan::default());
        assert!(codex_args(&p).is_empty());
    }

    #[test]
    fn repository_names_make_safe_directory_names() {
        assert_eq!(key("justinholmes/semantic-context"), "justinholmes_semantic-context");
        assert_eq!(key("group/sub/p"), "group_sub_p");
    }
}
