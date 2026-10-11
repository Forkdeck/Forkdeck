mod agents;
mod config;
mod demo;
mod issues;
mod orchestrator;
mod proc;
mod router;
mod state;
mod ui;

use clap::{Parser, Subcommand};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use orchestrator::Hub;
use state::{AgentKind, JobStatus, Provider, Source};
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Parser)]
#[command(name = "forkdeck", version, about = "Agent tree for the claude and codex CLIs")]
struct Cli {
    /// Config file (default: ~/.config/forkdeck/config.toml)
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    /// Simulated agents, router and issues; nothing is executed
    #[arg(long, global = true)]
    demo: bool,
    /// Render one frame to a .html or .txt file and exit
    #[arg(long, hide = true)]
    snapshot: Option<PathBuf>,
    #[arg(long, hide = true, default_value = "112x46")]
    size: String,
    #[arg(long, hide = true, default_value_t = 6)]
    after: u64,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run one task without the interface and print progress
    Run {
        /// Force an agent: claude or codex
        #[arg(long)]
        agent: Option<String>,
        task: Vec<String>,
    },
    /// Show what the fork layer would decide for a task
    Route { task: Vec<String> },
    /// Poll the configured repositories once, work through what is labelled, and exit
    Once,
    /// Check the CLIs, the decision endpoint and the configured repositories
    Doctor,
    /// Write a commented config file
    Init {
        /// Where to write it (default: ~/.config/forkdeck/config.toml)
        #[arg(long)]
        path: Option<PathBuf>,
        #[arg(long)]
        force: bool,
    },
}

fn parse_agent(s: &str) -> anyhow::Result<AgentKind> {
    match s {
        "claude" => Ok(AgentKind::Claude),
        "codex" => Ok(AgentKind::Codex),
        other => anyhow::bail!("unknown agent {:?}: use claude or codex", other),
    }
}

/// Print without panicking when the terminal or pipe has gone away: a panic here would skip the shutdown.
macro_rules! say {
    ($($t:tt)*) => {{ let _ = writeln!(std::io::stdout(), $($t)*); }};
}
macro_rules! warn {
    ($($t:tt)*) => {{ let _ = writeln!(std::io::stderr(), $($t)*); }};
}

fn main() -> anyhow::Result<()> {
    // Whatever goes wrong, do not leave agents running with nobody watching them.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        proc::kill_all();
        default_hook(info);
    }));
    let cli = Cli::parse();
    if let Some(Cmd::Init { path, force }) = &cli.cmd {
        let p = path.clone().unwrap_or_else(config::user_path);
        if p.exists() && !force {
            anyhow::bail!("{} already exists (use --force to overwrite)", p.display());
        }
        if let Some(parent) = p.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&p, config::EXAMPLE)?;
        println!("wrote {}", p.display());
        if path.is_some() {
            println!("pass it with --config; only ~/.config/forkdeck/config.toml is read automatically");
        }
        return Ok(());
    }

    let (mut cfg, cfg_path) = config::load(cli.config.as_deref())?;
    let stray = cfg_path.is_none() && cli.config.is_none() && !cli.demo && std::path::Path::new("forkdeck.toml").exists();
    if stray && cli.cmd.is_some() {
        eprintln!("note: ./forkdeck.toml is not read. Move it to {} or pass --config.", config::user_path().display());
    }
    cfg.demo = cli.demo;
    if cfg.demo {
        cfg.automation.enabled = true;
        if cfg.repos.is_empty() {
            cfg.repos.push(config::RepoCfg {
                provider: Provider::Github,
                repo: "acme/payments".into(),
                path: PathBuf::from("."),
                base: "main".into(),
                label: "agent:ready".into(),
                working_label: "agent:working".into(),
                done_label: "agent:review".into(),
                failed_label: "agent:needs-human".into(),
                allowed_authors: vec![],
                checks: vec!["cargo test --quiet".into()],
                pr_label: "review:ready".into(),
                pr_working_label: "review:running".into(),
                pr_pass_label: "review:passed".into(),
                pr_fail_label: "review:changes".into(),
                pr_checks: true,
            });
        }
    }
    let rt = tokio::runtime::Runtime::new()?;
    let hub = Hub::new(cfg, rt.handle().clone());
    let cwd = std::env::current_dir()?;

    match cli.cmd {
        Some(Cmd::Route { task }) => {
            let task = task.join(" ");
            anyhow::ensure!(!task.trim().is_empty(), "give a task to route");
            let r = rt.block_on(router::route(&hub.cfg, &hub.http, &task, false));
            if let Some(e) = &r.fell_back {
                eprintln!("jev unavailable, rules used instead: {}", e);
            }
            println!("{}", serde_json::to_string_pretty(&r.decision)?);
            Ok(())
        }
        Some(Cmd::Once) => {
            anyhow::ensure!(!hub.cfg.repos.is_empty(), "no [[repos]] configured, nothing to poll");
            rt.block_on(hub.detect_agents());
            let code = rt.block_on(once(&hub));
            std::process::exit(code);
        }
        Some(Cmd::Doctor) => {
            let ok = rt.block_on(doctor(&hub, cfg_path));
            std::process::exit(if ok { 0 } else { 1 });
        }
        Some(Cmd::Run { agent, task }) => {
            let task = task.join(" ");
            anyhow::ensure!(!task.trim().is_empty(), "give a task to run");
            let forced = agent.as_deref().map(parse_agent).transpose()?;
            rt.block_on(hub.detect_agents());
            let code = rt.block_on(headless(&hub, task, cwd, forced));
            std::process::exit(code);
        }
        Some(Cmd::Init { .. }) => unreachable!(),
        None => {
            rt.block_on(hub.detect_agents());
            rt.spawn(issues::poll_loop(hub.clone()));
            if let Some(path) = cli.snapshot {
                return snapshot(&rt, &hub, cwd, &path, &cli.size, cli.after);
            }
            if let Some(p) = cfg_path {
                hub.log("config", format!("loaded {}", p.display()));
            } else if !hub.cfg.demo {
                hub.log("config", "no config file found, using defaults (forkdeck init writes one)");
                if stray {
                    hub.log("config", "ignoring ./forkdeck.toml: move it to ~/.config/forkdeck/config.toml or pass --config");
                }
            }
            tui(&rt, &hub, cwd)
        }
    }
}

/// Resolves on SIGINT, SIGTERM or SIGHUP.
async fn stop_signal() {
    use tokio::signal::unix::{signal, SignalKind};
    let mut term = signal(SignalKind::terminate()).expect("signal handler");
    let mut int = signal(SignalKind::interrupt()).expect("signal handler");
    let mut hup = signal(SignalKind::hangup()).expect("signal handler");
    tokio::select! {
        _ = term.recv() => {}
        _ = int.recv() => {}
        _ = hup.recv() => {}
    }
}

async fn headless(hub: &Hub, task: String, cwd: PathBuf, forced: Option<AgentKind>) -> i32 {
    let title = state::clip(&task, 80);
    let id = hub.submit(title, task, Source::Manual, cwd, forced);
    // The agent runs in its own process group, so a signal sent to us has to be passed on by hand.
    let h = hub.clone();
    tokio::spawn(async move {
        stop_signal().await;
        h.cancel(id);
    });
    loop {
        tokio::time::sleep(Duration::from_millis(150)).await;
        print_progress(hub);
        let st = hub.state.lock().unwrap();
        let Some(j) = st.jobs.iter().find(|j| j.id == id) else { return 1 };
        if j.status.finished() {
            let code = if j.status == JobStatus::Done { 0 } else { 1 };
            drop(st);
            print_progress(hub);
            return code;
        }
    }
}

/// Print and drain what has happened since the last call. Only for the modes without an interface.
fn print_progress(hub: &Hub) {
    let mut st = hub.state.lock().unwrap();
    let lines: Vec<_> = st.log.drain(..).collect();
    for l in &lines {
        say!("{} {:<7} {}", l.time, l.who, l.msg);
    }
    for j in st.jobs.iter_mut() {
        let id = j.id;
        for l in j.worker.tail.drain(..) {
            say!("         #{:<3} worker  {}", id, l);
        }
        for l in j.reviewer.tail.drain(..) {
            say!("         #{:<3} review  {}", id, l);
        }
    }
    let _ = std::io::stdout().flush();
}

/// One pass over the repositories, for cron and for tests: take what is labelled now, finish it, exit.
/// Exits 1 if any job failed.
async fn once(hub: &Hub) -> i32 {
    let h = hub.clone();
    tokio::spawn(async move {
        stop_signal().await;
        h.shutdown(Duration::from_secs(45)).await;
    });
    issues::poll_once(hub).await;
    loop {
        print_progress(hub);
        let (open, failed) = {
            let st = hub.state.lock().unwrap();
            (
                st.jobs.iter().filter(|j| !j.status.finished()).count(),
                st.jobs.iter().filter(|j| matches!(j.status, JobStatus::Failed | JobStatus::Cancelled)).count(),
            )
        };
        if open == 0 {
            print_progress(hub);
            return if failed == 0 { 0 } else { 1 };
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
}

async fn doctor(hub: &Hub, cfg_path: Option<PathBuf>) -> bool {
    let mut ok = true;
    match cfg_path {
        Some(p) => println!("config    {}", p.display()),
        None => println!("config    none found, defaults in use"),
    }
    let mut agents_found = 0;
    for k in [AgentKind::Claude, AgentKind::Codex] {
        let cmd = hub.cfg.agents.get(k).cmd.unwrap_or_default();
        match issues::sh(&cmd, &["--version"], None).await {
            Ok(v) => {
                agents_found += 1;
                println!("{:<9} ok       {}", k.name(), v.lines().next().unwrap_or(""));
            }
            Err(_) => println!("{:<9} missing  `{}` is not on PATH", k.name(), cmd),
        }
    }
    if agents_found == 0 {
        ok = false;
        println!("          at least one of the two CLIs is required");
    } else if agents_found == 1 {
        println!("          one CLI only: it will also review its own work");
    }
    if hub.cfg.router.kind == "jev" {
        let r = router::route(&hub.cfg, &hub.http, "Fix a typo in the README heading", false).await;
        match r.fell_back {
            None => println!(
                "router    ok       {} answered in {} ms: {} / {}",
                hub.cfg.router.jev.base_url,
                r.decision.ms,
                r.decision.agent.name(),
                r.decision.tier.name()
            ),
            Some(e) => {
                ok = false;
                println!("router    down     {} (rules will be used until it answers)", e);
            }
        }
    } else {
        println!("router    rules    keyword rules, no endpoint configured");
    }
    for r in &hub.cfg.repos {
        let tool = if r.provider == Provider::Github { "gh" } else { "glab" };
        let tool_ok = issues::sh(tool, &["--version"], None).await.is_ok();
        let git_ok = issues::sh("git", &["rev-parse", "--git-dir"], Some(&r.path)).await.is_ok();
        println!(
            "repo      {:<8} {} · {} {} · clone at {} {}",
            if tool_ok && git_ok { "ok" } else { "problem" },
            r.repo,
            tool,
            if tool_ok { "found" } else { "missing" },
            r.path.display(),
            if git_ok { "found" } else { "is not a git repository" }
        );
        ok &= tool_ok && git_ok;
    }
    if hub.cfg.repos.is_empty() {
        println!("repos     none     add [[repos]] entries to pick up labelled issues");
    }
    ok
}

fn new_ui(hub: &Hub, cwd: PathBuf) -> ui::Ui {
    ui::Ui { input: String::new(), selected: None, follow: true, forced: None, cwd, demo: hub.cfg.demo }
}

fn sync_selection(hub: &Hub, u: &mut ui::Ui) {
    let st = hub.state.lock().unwrap();
    if u.follow || u.selected.is_none() {
        // Follow the newest job that is still moving, else the newest.
        u.selected = st.jobs.iter().rev().find(|j| !j.status.finished()).or(st.jobs.last()).map(|j| j.id);
    }
}

fn snapshot(rt: &tokio::runtime::Runtime, hub: &Hub, cwd: PathBuf, path: &std::path::Path, size: &str, after: u64) -> anyhow::Result<()> {
    let (w, h) = size.split_once('x').ok_or_else(|| anyhow::anyhow!("--size wants WIDTHxHEIGHT"))?;
    let (w, h): (u16, u16) = (w.parse()?, h.parse()?);
    let mut u = new_ui(hub, cwd.clone());
    if hub.cfg.demo {
        hub.submit("Fix the failing retry test in the network client".into(), "Fix the failing retry test in the network client".into(), Source::Manual, cwd, None);
    }
    rt.block_on(async { tokio::time::sleep(Duration::from_secs(after)).await });
    sync_selection(hub, &mut u);
    let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h))?;
    term.draw(|f| ui::draw(f, &hub.state.lock().unwrap(), &u))?;
    let buf = term.backend().buffer();
    let out = if path.extension().map(|e| e == "html").unwrap_or(false) { ui::to_html(buf) } else { ui::to_text(buf) };
    std::fs::write(path, out)?;
    Ok(())
}

fn tui(rt: &tokio::runtime::Runtime, hub: &Hub, cwd: PathBuf) -> anyhow::Result<()> {
    let mut u = new_ui(hub, cwd.clone());
    if hub.cfg.demo {
        hub.submit("Fix the failing retry test in the network client".into(), "Fix the failing retry test in the network client".into(), Source::Manual, cwd.clone(), None);
    }
    // A signal (terminal closed, kill, service stop) is handled entirely on the runtime: the main
    // thread may be stuck reading a terminal that no longer exists.
    let stopping = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    {
        let hub = hub.clone();
        let stopping = stopping.clone();
        rt.spawn(async move {
            stop_signal().await;
            stopping.store(true, std::sync::atomic::Ordering::SeqCst);
            let _ = ratatui::try_restore();
            warn!("stopping jobs and handing claimed issues back…");
            hub.shutdown(Duration::from_secs(45)).await;
            proc::kill_all();
            std::process::exit(1);
        });
    }
    let mut term = ratatui::init();
    let _ = crossterm::execute!(std::io::stdout(), event::EnableBracketedPaste);
    let res = (|| -> anyhow::Result<()> {
        loop {
            if stopping.load(std::sync::atomic::Ordering::SeqCst) {
                // The signal task owns the shutdown and will exit the process; just stay out of its way.
                std::thread::sleep(Duration::from_millis(200));
                continue;
            }
            sync_selection(hub, &mut u);
            term.draw(|f| ui::draw(f, &hub.state.lock().unwrap(), &u))?;
            if !event::poll(Duration::from_millis(100))? {
                continue;
            }
            match event::read()? {
                Event::Paste(s) => u.input.push_str(&s.replace(['\n', '\r'], " ")),
                Event::Key(k) if k.kind != KeyEventKind::Release => {
                    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
                    match k.code {
                        KeyCode::Char('c') | KeyCode::Char('q') if ctrl => return Ok(()),
                        KeyCode::Char('a') if ctrl => {
                            let mut st = hub.state.lock().unwrap();
                            st.automation = !st.automation;
                            let on = st.automation;
                            let none = st.repos.is_empty();
                            st.log("issues", if on { "polling on" } else { "polling paused" });
                            if on && none {
                                st.log("issues", "no [[repos]] configured, nothing to poll");
                            }
                        }
                        KeyCode::Char('x') if ctrl => {
                            if let Some(id) = u.selected {
                                hub.cancel(id);
                            }
                        }
                        KeyCode::Char('u') if ctrl => u.input.clear(),
                        KeyCode::Char(c) if !ctrl => u.input.push(c),
                        KeyCode::Backspace => {
                            u.input.pop();
                        }
                        KeyCode::Esc => {
                            u.input.clear();
                            u.follow = true;
                        }
                        KeyCode::Tab => {
                            u.forced = match u.forced {
                                None => Some(AgentKind::Claude),
                                Some(AgentKind::Claude) => Some(AgentKind::Codex),
                                Some(AgentKind::Codex) => None,
                            }
                        }
                        KeyCode::Up | KeyCode::Down => {
                            let st = hub.state.lock().unwrap();
                            let ids: Vec<u32> = st.jobs.iter().map(|j| j.id).collect();
                            if !ids.is_empty() {
                                let pos = u.selected.and_then(|s| ids.iter().position(|i| *i == s)).unwrap_or(ids.len() - 1);
                                let next = if k.code == KeyCode::Up { pos.saturating_sub(1) } else { (pos + 1).min(ids.len() - 1) };
                                u.selected = Some(ids[next]);
                                u.follow = false;
                            }
                        }
                        KeyCode::Enter => {
                            let task = u.input.trim().to_string();
                            if !task.is_empty() {
                                u.input.clear();
                                let id = hub.submit(state::clip(&task, 80), task, Source::Manual, cwd.clone(), u.forced);
                                u.selected = Some(id);
                                u.follow = false;
                            }
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    })();
    let _ = crossterm::execute!(std::io::stdout(), event::DisableBracketedPaste);
    let _ = ratatui::try_restore();
    // Stop any agent still running, and give claimed issues time to be handed back with a comment.
    let busy = hub.state.lock().unwrap().jobs.iter().filter(|j| !j.status.finished()).count();
    if busy > 0 {
        warn!("stopping {} job(s) and handing claimed issues back…", busy);
    }
    let left = rt.block_on(hub.shutdown(Duration::from_secs(45)));
    if left > 0 {
        warn!("{} job(s) did not finish in time; check their issues for a stale working label", left);
    }
    proc::kill_all();
    res
}
