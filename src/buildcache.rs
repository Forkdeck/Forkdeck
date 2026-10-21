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
    /// The public package registries its dependencies come from, which a sandboxed worker may
    /// reach (agents.network.public_registries). Sources fetched from git hosts are not here.
    registries: &'static [&'static str],
}

const MAVEN: &[&str] = &["repo.maven.apache.org", "repo1.maven.org"];

const TOOLCHAINS: &[Toolchain] = &[
    // Rust's build output is redirected to one directory per repository, below.
    Toolchain { names: &["Cargo.toml"], extensions: &[], caches: &[".cargo/registry", ".cargo/git"], commands: &["cargo"], registries: &["crates.io", "index.crates.io", "static.crates.io"] },
    Toolchain { names: &["go.mod", "go.work"], extensions: &[], caches: &[".cache/go-build", "go/pkg/mod"], commands: &["go"], registries: &["proxy.golang.org", "sum.golang.org"] },
    Toolchain { names: &["build.gradle", "build.gradle.kts", "settings.gradle", "settings.gradle.kts"], extensions: &[], caches: &[".gradle"], commands: &["gradle", "./gradlew"], registries: &["repo.maven.apache.org", "repo1.maven.org", "plugins.gradle.org", "plugins-artifacts.gradle.org", "services.gradle.org", "downloads.gradle.org", "dl.google.com"] },
    Toolchain { names: &["pom.xml"], extensions: &[], caches: &[".m2"], commands: &["mvn", "./mvnw"], registries: MAVEN },
    Toolchain { names: &["build.sbt"], extensions: &[], caches: &[".sbt", ".ivy2", ".cache/coursier"], commands: &["sbt"], registries: &["repo.maven.apache.org", "repo1.maven.org", "repo.scala-sbt.org"] },
    Toolchain { names: &["deps.edn", "project.clj"], extensions: &[], caches: &[".m2", ".gitlibs", ".clojure", ".lein"], commands: &["clojure", "clj", "lein"], registries: &["repo.maven.apache.org", "repo1.maven.org", "repo.clojars.org", "clojars.org"] },
    Toolchain { names: &["global.json", "Directory.Build.props"], extensions: &[".csproj", ".fsproj", ".vbproj", ".sln"], caches: &[".nuget/packages"], commands: &["dotnet"], registries: &["api.nuget.org", "globalcdn.nuget.org"] },
    Toolchain { names: &["Package.swift"], extensions: &[], caches: &[".cache/org.swift.swiftpm", ".swiftpm"], commands: &["swift"], registries: &[] },
    Toolchain { names: &["stack.yaml"], extensions: &[], caches: &[".stack"], commands: &["stack"], registries: &["hackage.haskell.org", "www.stackage.org"] },
    Toolchain { names: &["cabal.project"], extensions: &[".cabal"], caches: &[".cabal", ".cache/cabal", ".local/state/cabal"], commands: &["cabal"], registries: &["hackage.haskell.org"] },
    Toolchain { names: &["dune-project"], extensions: &[".opam"], caches: &[".cache/dune", ".opam"], commands: &["dune", "opam"], registries: &["opam.ocaml.org"] },
    Toolchain { names: &["build.zig", "build.zig.zon"], extensions: &[], caches: &[".cache/zig"], commands: &["zig"], registries: &[] },
    Toolchain { names: &["dub.json", "dub.sdl"], extensions: &[], caches: &[".dub"], commands: &["dub"], registries: &["code.dlang.org", "dub.pm"] },
    Toolchain { names: &[], extensions: &[".nimble"], caches: &[".nimble", ".cache/nim"], commands: &["nimble", "nim"], registries: &[] },
    Toolchain { names: &["shard.yml"], extensions: &[], caches: &[".cache/crystal", ".cache/shards"], commands: &["crystal", "shards"], registries: &[] },
    Toolchain { names: &["mix.exs"], extensions: &[], caches: &[".mix", ".hex"], commands: &["mix"], registries: &["hex.pm", "repo.hex.pm", "builds.hex.pm"] },
    Toolchain { names: &["rebar.config"], extensions: &[], caches: &[".cache/rebar3"], commands: &["rebar3"], registries: &["hex.pm", "repo.hex.pm"] },
    Toolchain { names: &["pubspec.yaml"], extensions: &[], caches: &[".pub-cache"], commands: &["dart", "flutter"], registries: &["pub.dev", "pub.dartlang.org", "storage.googleapis.com"] },
    Toolchain { names: &["WORKSPACE", "WORKSPACE.bazel", "MODULE.bazel"], extensions: &[], caches: &[".cache/bazel"], commands: &["bazel", "bazelisk"], registries: &["bcr.bazel.build", "mirror.bazel.build"] },
    Toolchain { names: &["package.json"], extensions: &[], caches: &[".npm", ".cache/yarn", ".local/share/pnpm", ".bun/install/cache"], commands: &["npm", "pnpm", "yarn", "bun"], registries: &["registry.npmjs.org", "registry.yarnpkg.com"] },
    Toolchain { names: &["deno.json", "deno.jsonc"], extensions: &[], caches: &[".cache/deno"], commands: &["deno"], registries: &["deno.land", "jsr.io", "registry.npmjs.org"] },
    Toolchain { names: &["elm.json"], extensions: &[], caches: &[".elm"], commands: &["elm", "elm-test"], registries: &["package.elm-lang.org"] },
    Toolchain { names: &["pyproject.toml", "setup.py", "setup.cfg", "requirements.txt", "Pipfile"], extensions: &[], caches: &[".cache/pip", ".cache/uv"], commands: &["pytest", "python -m pytest", "uv run pytest"], registries: &["pypi.org", "files.pythonhosted.org"] },
    Toolchain { names: &["Gemfile", "Rakefile"], extensions: &[".gemspec"], caches: &[".gem", ".bundle", ".cache/bundle"], commands: &["bundle", "rake", "rspec"], registries: &["rubygems.org", "index.rubygems.org"] },
    Toolchain { names: &["composer.json"], extensions: &[], caches: &[".cache/composer", ".composer"], commands: &["composer", "phpunit", "./vendor/bin/phpunit"], registries: &["repo.packagist.org", "packagist.org"] },
    Toolchain { names: &["cpanfile", "Makefile.PL", "Build.PL"], extensions: &[], caches: &[".cpanm", "perl5"], commands: &["prove", "cpanm"], registries: &["cpan.metacpan.org", "fastapi.metacpan.org"] },
    Toolchain { names: &["DESCRIPTION", "renv.lock"], extensions: &[".Rproj"], caches: &["R", ".cache/R"], commands: &["Rscript", "R CMD"], registries: &["cloud.r-project.org", "cran.r-project.org"] },
    Toolchain { names: &["Project.toml", "JuliaProject.toml"], extensions: &[], caches: &[".julia"], commands: &["julia"], registries: &["pkg.julialang.org"] },
    Toolchain { names: &[], extensions: &[".rockspec"], caches: &[".luarocks", ".cache/luarocks"], commands: &["luarocks", "busted"], registries: &["luarocks.org"] },
];

/// The public package registries of the toolchains in a tree.
pub fn registries(files: &[String]) -> Vec<String> {
    let names: Vec<&str> = files.iter().map(|f| f.rsplit('/').next().unwrap_or(f)).collect();
    let mut out: Vec<String> = TOOLCHAINS.iter()
        .filter(|t| names.iter().any(|n| t.names.contains(n) || t.extensions.iter().any(|e| n.ends_with(e))))
        .flat_map(|t| t.registries.iter().map(|r| r.to_string())).collect();
    out.sort();
    out.dedup();
    out
}

/// The user's own package-manager settings that can name a mirror, under the home directory.
const MIRROR_FILES: &[&str] = &[
    ".npmrc", ".yarnrc", ".yarnrc.yml", ".config/pip/pip.conf", ".pip/pip.conf", ".config/uv/uv.toml",
    ".m2/settings.xml", ".gradle/init.gradle", ".gradle/init.gradle.kts", ".gradle/gradle.properties",
    ".cargo/config.toml", ".cargo/config", ".nuget/NuGet/NuGet.Config", ".gemrc", ".bundle/config",
    ".config/composer/config.json", ".composer/config.json", ".condarc", ".sbt/repositories", ".ivy2/ivysettings.xml",
    ".config/go/env", ".hex/hex.config", ".julia/config/startup.jl", ".Rprofile",
];
/// The same, system-wide.
const MIRROR_SYSTEM_FILES: &[&str] = &["/etc/pip.conf", "/etc/npmrc", "/etc/xdg/pip/pip.conf", "/etc/uv/uv.toml"];
/// Environment variables that point a package manager at a mirror.
const MIRROR_VARS: &[&str] = &[
    "GOPROXY", "GOSUMDB", "PIP_INDEX_URL", "PIP_EXTRA_INDEX_URL", "UV_INDEX_URL", "UV_DEFAULT_INDEX", "UV_INDEX", "UV_EXTRA_INDEX_URL",
    "npm_config_registry", "NPM_CONFIG_REGISTRY", "YARN_NPM_REGISTRY_SERVER", "BUNDLE_MIRROR__ALL", "HEX_MIRROR", "HEX_CDN",
    "COMPOSER_MIRROR_PATH_REPOS", "NUGET_SOURCE", "JULIA_PKG_SERVER", "CONDA_CHANNEL_ALIAS",
];

/// The hosts of the URLs in some text, without credentials or ports.
fn hosts_in(text: &str) -> Vec<String> {
    static URL: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| regex::Regex::new(r#"(?i)\b(?:sparse\+)?https?://([^/\s"'<>,|;]+)"#).unwrap());
    URL.captures_iter(text)
        .filter_map(|c| {
            let authority = c.get(1)?.as_str();
            let host = authority.rsplit_once('@').map(|(_, h)| h).unwrap_or(authority);
            let host = host.split(':').next()?.trim_end_matches('.').to_ascii_lowercase();
            (!host.is_empty() && host.contains(|ch: char| ch.is_ascii_alphanumeric())).then_some(host)
        })
        .collect()
}

/// The mirrors your package managers already use, read from your own settings and environment
/// (never from a repository, whose files the work may have changed): their hosts.
pub fn detected_mirrors(home: &Path, system: &[&str], var: impl Fn(&str) -> Option<String>) -> Vec<String> {
    let mut out = Vec::new();
    for f in MIRROR_FILES.iter().map(|f| home.join(f)).chain(system.iter().map(PathBuf::from)) {
        if let Ok(text) = std::fs::read_to_string(&f) {
            out.extend(hosts_in(&text));
        }
    }
    for v in MIRROR_VARS {
        if let Some(value) = var(v) {
            out.extend(hosts_in(&value));
        }
    }
    out.sort();
    out.dedup();
    out
}

/// `detected_mirrors` for this machine.
pub fn machine_mirrors() -> Vec<String> {
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()));
    detected_mirrors(&home, MIRROR_SYSTEM_FILES, |v| std::env::var(v).ok())
}

/// The hosts a sandboxed worker in `dir` may reach: the public registries of its languages
/// (unless switched off), the mirrors your package managers use, the configured mirrors, and
/// what its repository adds.
pub fn domains(dir: &Path, network: &crate::config::NetworkCfg, repo: &[String]) -> Vec<String> {
    let mut out = if network.public_registries { registries(&tracked(dir)) } else { Vec::new() };
    if network.detect_mirrors {
        out.extend(machine_mirrors());
    }
    out.extend(network.mirrors.iter().cloned());
    out.extend(repo.iter().cloned());
    out.sort();
    out.dedup();
    out
}

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
    let mut p = plan(&files, repo_key, &root, &home, |p| std::env::var_os("PATH").is_some_and(|path| std::env::split_paths(&path).any(|d| d.join(p).is_file())));
    if let Some(path) = std::env::var_os("PATH").and_then(|path| unsnapped_path(&path, &commands(&files), Path::new("/snap"))) {
        p.env.push(("PATH".into(), path));
    }
    p
}

/// A PATH on which the tree's toolchain commands that resolve to snap shims resolve to the
/// binaries behind them instead. A shim in /snap/bin cannot start inside a sandbox (snap
/// confinement needs what the sandbox takes away), but the binary in /snap/<snap>/current/bin
/// can: Rust installed as the rustup snap is the common case. None when nothing needs changing.
pub fn unsnapped_path(path: &std::ffi::OsStr, commands: &[&str], snap_root: &Path) -> Option<String> {
    let dirs: Vec<PathBuf> = std::env::split_paths(path).collect();
    let shims = snap_root.join("bin");
    let mut found: Vec<PathBuf> = Vec::new();
    for c in commands.iter().filter_map(|c| c.split_whitespace().next()).filter(|c| !c.contains('/')) {
        let Some(at) = dirs.iter().find(|d| d.join(c).symlink_metadata().is_ok()) else { continue };
        if *at != shims { continue; }
        // `cargo -> rustup.cargo` for an app of a snap; `hugo -> /usr/bin/snap` for a snap's own command.
        let Ok(target) = std::fs::read_link(at.join(c)) else { continue };
        let target = target.to_string_lossy().to_string();
        let (snap, app) = match target.split_once('.') { Some((s, a)) if !target.contains('/') => (s.to_string(), a.to_string()), _ => (c.to_string(), c.to_string()) };
        let bin = snap_root.join(snap).join("current/bin");
        if bin.join(&app).is_file() && !found.contains(&bin) { found.push(bin); }
    }
    if found.is_empty() { return None; }
    let at = dirs.iter().position(|d| *d == shims)?;
    let mut out = dirs.clone();
    out.splice(at..at, found);
    std::env::join_paths(out).ok().map(|p| p.to_string_lossy().to_string())
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
    fn each_language_brings_its_own_registries() {
        let r = super::registries(&files(&["Cargo.toml", "web/package.json", "src/main.rs"]));
        assert!(r.contains(&"crates.io".to_string()) && r.contains(&"registry.npmjs.org".to_string()), "{r:?}");
        assert!(!r.contains(&"pypi.org".to_string()), "only the languages the tree uses: {r:?}");
        for (file, host, command) in [("Gemfile", "rubygems.org", "bundle"), ("composer.json", "repo.packagist.org", "composer"), ("pyproject.toml", "files.pythonhosted.org", "pytest"),
            ("go.mod", "proxy.golang.org", "go"), ("pom.xml", "repo1.maven.org", "mvn"), ("app.csproj", "api.nuget.org", "dotnet"), ("mix.exs", "repo.hex.pm", "mix"),
            ("Project.toml", "pkg.julialang.org", "julia"), ("deps.edn", "repo.clojars.org", "clojure"), ("x.rockspec", "luarocks.org", "luarocks")] {
            assert!(super::registries(&files(&[file])).contains(&host.to_string()), "{file}");
            assert!(super::commands(&files(&[file])).contains(&command), "{file}");
        }
        assert!(super::registries(&files(&["README.md"])).is_empty());
    }

    #[test]
    fn mirrors_are_found_in_the_users_own_settings() {
        let home = std::env::temp_dir().join(format!("forkdeck-mirrors-{}", std::process::id()));
        let w = |f: &str, text: &str| {
            let p = home.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        };
        w(".npmrc", "registry=https://nexus.corp.example/repository/npm/\n//nexus.corp.example/:_authToken=secret\n");
        w(".config/pip/pip.conf", "[global]\nindex-url = https://user:pw@artifactory.example:8443/api/pypi/pypi/simple\n");
        w(".cargo/config.toml", "[source.crates-io]\nreplace-with = \"corp\"\n[source.corp]\nregistry = \"sparse+https://cargo.corp.example/index/\"\n");
        w(".m2/settings.xml", "<settings><mirrors><mirror><url>https://maven.corp.example/repository/central</url></mirror></mirrors></settings>");
        let env = |v: &str| match v {
            "GOPROXY" => Some("https://goproxy.corp.example,direct".to_string()),
            "PIP_EXTRA_INDEX_URL" => Some("https://extra.example/simple https://pypi.corp.example/simple".to_string()),
            _ => None,
        };
        let found = super::detected_mirrors(&home, &[], env);
        assert_eq!(found, ["artifactory.example", "cargo.corp.example", "extra.example", "goproxy.corp.example", "maven.corp.example", "nexus.corp.example", "pypi.corp.example"]);
        assert!(super::detected_mirrors(&home.join("nowhere"), &[], |_| None).is_empty());
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn mirrors_replace_or_join_the_public_registries() {
        let dir = std::env::temp_dir().join(format!("forkdeck-registries-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let git = |a: &[&str]| assert!(std::process::Command::new("git").arg("-C").arg(&dir).args(a).status().unwrap().success());
        git(&["init", "-q"]);
        std::fs::write(dir.join("package.json"), "{}").unwrap();
        git(&["add", "."]);
        let mut net = crate::config::NetworkCfg { public_registries: true, mirrors: vec!["nexus.corp".into()], detect_mirrors: false };
        let repo = vec!["api.example.com".to_string()];
        assert_eq!(super::domains(&dir, &net, &repo), ["api.example.com", "nexus.corp", "registry.npmjs.org", "registry.yarnpkg.com"]);
        net.public_registries = false;
        assert_eq!(super::domains(&dir, &net, &repo), ["api.example.com", "nexus.corp"], "only the mirror");
        std::fs::remove_dir_all(dir).unwrap();
    }

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
    fn snap_shims_are_bypassed_for_the_toolchains_a_tree_uses() {
        let root = std::env::temp_dir().join(format!("forkdeck-snap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("bin")).unwrap();
        std::fs::create_dir_all(root.join("rustup/current/bin")).unwrap();
        std::fs::write(root.join("rustup/current/bin/cargo"), "").unwrap();
        std::os::unix::fs::symlink("rustup.cargo", root.join("bin/cargo")).unwrap();
        std::os::unix::fs::symlink("/usr/bin/snap", root.join("bin/hugo")).unwrap();
        let bin = root.join("bin").display().to_string();
        let path = std::ffi::OsString::from(format!("/nonexistent:{bin}"));
        assert_eq!(unsnapped_path(&path, &["cargo", "git status"], &root), Some(format!("/nonexistent:{}:{bin}", root.join("rustup/current/bin").display())));
        assert_eq!(unsnapped_path(&path, &["go"], &root), None, "not installed at all");
        assert_eq!(unsnapped_path(&path, &["hugo"], &root), None, "a snap whose binary is not there is left alone");
        // A real cargo earlier on PATH wins already: nothing to change.
        std::fs::create_dir_all(root.join("real")).unwrap();
        std::fs::write(root.join("real/cargo"), "").unwrap();
        assert_eq!(unsnapped_path(&std::ffi::OsString::from(format!("{}:{bin}", root.join("real").display())), &["cargo"], &root), None);
        std::fs::remove_dir_all(root).unwrap();
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
