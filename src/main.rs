mod agents;
mod cicd;
mod config;
mod copilot;
mod demo;
mod history;
mod issues;
mod journal;
mod knowledge;
mod mcp;
mod orchestrator;
mod pick;
mod proc;
mod router;
mod state;
mod team;
mod ui;

use clap::{Parser, Subcommand};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use orchestrator::Hub;
use state::{AgentKind, AgentRef, JobStatus, Provider, Source};
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
    /// With --snapshot: show the team pane
    #[arg(long, hide = true)]
    team_pane: bool,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run one task without the interface and print progress. Lines typed on stdin while it runs
    /// are sent to the task; a line starting with "!" stops the agent to take it at once
    Run {
        /// Force an agent: claude, codex, copilot or copilot:<profile>
        #[arg(long)]
        agent: Option<String>,
        /// Have the agent propose approaches first; pick one on stdin by its number, or type your own
        #[arg(long)]
        plan: bool,
        task: Vec<String>,
    },
    /// Continue the typed tasks that were cut off when forkdeck last stopped, without the interface.
    /// Lines typed on stdin go to them as in `run` ("#N text" to pick one)
    Resume,
    /// List the jobs kept from earlier runs, or clear them
    History {
        #[command(subcommand)]
        cmd: Option<HistoryCmd>,
    },
    /// Show what the fork layer would decide for a task
    Route { task: Vec<String> },
    /// Poll the configured repositories once, work through what is labelled, and exit
    Once,
    /// Look at a configured MCP server: list its tools, or call one by hand
    Mcp {
        #[command(subcommand)]
        cmd: McpCmd,
    },
    /// Show what everyone's forkdeck is working on in the configured repositories
    Team {
        /// Also list issues whose pull request is open and waiting
        #[arg(long)]
        all: bool,
        /// Print the rows as JSON
        #[arg(long)]
        json: bool,
        #[command(subcommand)]
        cmd: Option<TeamCmd>,
    },
    /// What forkdeck has done lately: jobs, pass rates, time to a pull request, tokens
    Stats {
        /// How many days back to look
        #[arg(long, default_value_t = 7)]
        days: u32,
        /// Also list the latest jobs, newest first
        #[arg(long, default_value_t = 0)]
        jobs: usize,
        /// Print the summary as JSON
        #[arg(long)]
        json: bool,
    },
    /// Show or add to the notes the worker is given for a repository
    Notes {
        /// owner/repo, as in the config. Without it, list the repositories and their notes files
        repo: Option<String>,
        #[command(subcommand)]
        cmd: Option<NotesCmd>,
    },
    /// Check the CLIs, the decision endpoint, the MCP servers and the configured repositories
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

#[derive(Subcommand)]
enum NotesCmd {
    /// Add a note of your own; it is kept above what forkdeck learns and never rewritten
    Add { text: Vec<String> },
}

#[derive(Subcommand)]
enum TeamCmd {
    /// Hand a claimed issue or review back to the queue, for instance after a machine went away
    Release {
        /// owner/repo#12 for an issue, owner/repo!5 for a pull request
        item: String,
        /// End a claim that is still being updated, and stop the job behind it
        #[arg(long)]
        force: bool,
        /// Move the label back although no forkdeck claim is on the item (claimed by hand, or with sharing off)
        #[arg(long)]
        unclaimed: bool,
    },
}

#[derive(Subcommand)]
enum HistoryCmd {
    /// Remove every saved job
    Clear,
}

#[derive(Subcommand)]
enum McpCmd {
    /// List the tools a server offers, to find the names for the config
    Tools { server: String },
    /// Call a tool and print its answer as forkdeck would read it
    Call {
        server: String,
        tool: String,
        /// Arguments as a JSON object
        #[arg(default_value = "{}")]
        args: String,
    },
}

fn parse_agent(s: &str) -> anyhow::Result<AgentRef> {
    AgentRef::parse(s).map_err(anyhow::Error::msg)
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
    let main_thread = std::thread::current().id();
    std::panic::set_hook(Box::new(move |info| {
        if std::thread::current().id() == main_thread { proc::kill_all(); }
        default_hook(info);
    }));
    let cli = Cli::parse();
    anyhow::ensure!(!cli.demo || !matches!(cli.cmd, Some(Cmd::Team { cmd: Some(_), .. } | Cmd::Mcp { .. } | Cmd::Init { .. })), "this command is not supported in demo mode; nothing was executed");
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
    if !cfg.demo {
        // Files saved under a repository's name from before its host was part of it.
        knowledge::migrate_names(&cfg.repos);
        if let Err(e) = cicd::migrate_keys(&cfg.repos) {
            eprintln!("note: the deployment state was not moved to its new names: {}", e);
        }
    }
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
                jobs_per_day: 0,
                ci: None,
                deploy: None,
                host: String::new(),
            });
        }
    }
    let rt = tokio::runtime::Runtime::new()?;
    let hub = Hub::new(cfg, rt.handle().clone()).with_history();
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
            hub.start_autosave();
            let code = rt.block_on(once(&hub));
            std::process::exit(code);
        }
        Some(Cmd::Mcp { cmd }) => {
            let code = rt.block_on(mcp_cmd(&hub, cmd));
            std::process::exit(code);
        }
        Some(Cmd::Team { all, json, cmd }) => {
            anyhow::ensure!(hub.cfg.demo || !hub.cfg.repos.is_empty(), "no [[repos]] configured, nothing to look at");
            let code = rt.block_on(team_cmd(&hub, all, json, cmd));
            std::process::exit(code);
        }
        Some(Cmd::Stats { days, jobs, json }) => {
            let days = days.clamp(1, 3650);
            let entries = journal::read(journal::now() - days as i64 * journal::DAY).map_err(|e| anyhow::anyhow!("the journal cannot be read: {}", e))?;
            let summary = journal::summarise(&entries, days);
            if json {
                println!("{}", serde_json::to_string_pretty(&summary)?);
            } else {
                for l in journal::text(&summary) {
                    println!("{}", l);
                }
                if jobs > 0 && !entries.is_empty() {
                    println!();
                    for l in journal::recent(&entries, jobs) {
                        println!("{}", l);
                    }
                }
            }
            Ok(())
        }
        Some(Cmd::Notes { repo, cmd }) => {
            let Some(name) = repo else {
                anyhow::ensure!(!hub.cfg.repos.is_empty(), "no [[repos]] configured");
                for r in &hub.cfg.repos {
                    let p = knowledge::path(r);
                    let n = knowledge::load(r).lines().filter(|l| l.trim_start().starts_with("- ")).count();
                    println!("{:<30} {} note(s)   {}", r.repo, n, p.display());
                }
                return Ok(());
            };
            let r = hub.cfg.repos.iter().find(|r| r.repo.eq_ignore_ascii_case(&name)).ok_or_else(|| anyhow::anyhow!("{} is not one of the configured [[repos]]", name))?;
            match cmd {
                Some(NotesCmd::Add { text }) => {
                    knowledge::add_own(r, &text.join(" ")).map_err(anyhow::Error::msg)?;
                    println!("added to {}", knowledge::path(r).display());
                }
                None => {
                    let _ = rt.block_on(issues::sh("git", &["fetch", "-q", "origin", &format!("+refs/heads/{b}:refs/remotes/origin/{b}", b = r.base)], Some(&r.path)));
                    let team = rt.block_on(knowledge::shared(r));
                    println!("the team's notes: {} on {}", knowledge::SHARED, r.base);
                    if team.trim().is_empty() {
                        println!("  none yet. Add the file through a pull request, or let forkdeck propose rules ([knowledge] learn = true)");
                    } else {
                        println!();
                        println!("{}", team.trim_end());
                    }
                    let waiting = knowledge::pending(r).len();
                    if waiting > 0 {
                        println!("\n{} learned rule(s) are waiting to be proposed (the pull request could not be made yet); no job is given them", waiting);
                    }
                    let notes = knowledge::load(r);
                    println!("\nthis machine's own: {}", knowledge::path(r).display());
                    if notes.trim().is_empty() {
                        println!("  none yet. Add one with: forkdeck notes {} add \"...\", or edit the file", r.repo);
                    } else {
                        println!();
                        print!("{}", notes);
                    }
                    if !hub.cfg.knowledge.enabled {
                        println!("\nnote: [knowledge] enabled = false, so these are not given to the worker");
                    }
                }
            }
            Ok(())
        }
        Some(Cmd::Doctor) => {
            let ok = rt.block_on(doctor(&hub, cfg_path));
            std::process::exit(if ok { 0 } else { 1 });
        }
        Some(Cmd::Run { agent, plan, task }) => {
            let task = task.join(" ");
            anyhow::ensure!(!task.trim().is_empty(), "give a task to run");
            let forced = agent.as_deref().map(parse_agent).transpose()?;
            rt.block_on(hub.detect_agents());
            hub.start_autosave();
            let code = rt.block_on(headless(&hub, task, cwd, forced, plan));
            std::process::exit(code);
        }
        Some(Cmd::Resume) => {
            rt.block_on(hub.detect_agents());
            hub.start_autosave();
            let code = rt.block_on(resume_cmd(&hub));
            std::process::exit(code);
        }
        Some(Cmd::History { cmd }) => {
            let Some(h) = &hub.history else { anyhow::bail!("no history: {}", history::path().display()) };
            match cmd {
                Some(HistoryCmd::Clear) => println!("removed {} job(s)", h.clear().map_err(anyhow::Error::msg)?),
                None => {
                    for r in h.load(history::KEEP).map_err(anyhow::Error::msg)? {
                        println!("{}", history::summary(&r));
                    }
                }
            }
            Ok(())
        }
        Some(Cmd::Init { .. }) => unreachable!(),
        None => {
            rt.block_on(hub.detect_agents());
            rt.spawn(issues::poll_loop(hub.clone()));
            rt.spawn(team::watch_loop(hub.clone()));
            if let Some(path) = cli.snapshot {
                return snapshot(&rt, &hub, cwd, &path, &cli.size, cli.after, cli.team_pane);
            }
            // Earlier jobs come back; typed tasks that were cut off continue by themselves.
            hub.start_autosave();
            for id in hub.restore() {
                if let Err(e) = rt.block_on(hub.resume(id)) {
                    hub.log("queue", format!("#{} not resumed: {} (Ctrl-R tries again)", id, e));
                }
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

async fn headless(hub: &Hub, task: String, cwd: PathBuf, forced: Option<AgentRef>, plan: bool) -> i32 {
    let title = state::clip(&task, 80);
    // Without --plan nobody may be there to choose, so a plan is never waited for.
    let id = match hub.submit(title, task, Source::Manual, cwd, forced, if plan { state::Planning::Always } else { state::Planning::Never }).await { Ok(id) => id, Err(e) => { hub.log("queue", e); return 1; } };
    read_messages(hub, vec![id]);
    wait_for(hub, &[id]).await
}

/// Continue every typed task that was cut off, and wait for them.
/// Lines typed while jobs run go to them: queued for the agent, or at once with a leading "!". With
/// several jobs, "#N text" picks one; otherwise a job waiting for a choice of plan gets it, else the
/// first one still running.
fn read_messages(hub: &Hub, ids: Vec<u32>) {
    let h = hub.clone();
    std::thread::spawn(move || {
        for line in std::io::stdin().lines() {
            let Ok(line) = line else { break };
            let mut text = line.trim();
            let picked = text.strip_prefix('#').and_then(|r| r.split_once(' ')).and_then(|(n, rest)| n.parse::<u32>().ok().filter(|n| ids.contains(n)).map(|n| (n, rest.trim())));
            let id = match picked {
                Some((n, rest)) => {
                    text = rest;
                    n
                }
                None => {
                    let st = h.state.lock().unwrap_or_else(|e| e.into_inner());
                    let mine = || st.jobs.iter().filter(|j| ids.contains(&j.id));
                    let Some(id) = mine().find(|j| j.status == JobStatus::Choosing).or_else(|| mine().find(|j| !j.status.finished())).map(|j| j.id) else { continue };
                    id
                }
            };
            let sent = match text.strip_prefix('!') {
                Some(now) if !now.trim().is_empty() => h.rt.block_on(h.steer_now(id, now.trim().to_string())),
                _ if !text.is_empty() => h.rt.block_on(h.reply(id, text.to_string())),
                _ => continue,
            };
            if let Err(why) = sent {
                h.log("queue", format!("#{} not sent: {}", id, why));
            }
        }
        h.input_closed.store(true, std::sync::atomic::Ordering::SeqCst);
    });
}

async fn resume_cmd(hub: &Hub) -> i32 {
    let mut ids = Vec::new();
    for id in hub.restore() {
        match hub.resume(id).await {
            Ok(()) => ids.push(id),
            Err(e) => hub.log("queue", format!("#{} not resumed: {}", id, e)),
        }
    }
    if ids.is_empty() {
        print_progress(hub);
        say!("nothing to resume");
        return 0;
    }
    // A task stopped while planning or choosing asks again, and the answer is typed here.
    read_messages(hub, ids.clone());
    wait_for(hub, &ids).await
}

/// Print progress until the jobs are finished: 0 when all are done.
async fn wait_for(hub: &Hub, ids: &[u32]) -> i32 {
    let code = tokio::select! { result = wait_for_inner(hub, ids) => result, _ = stop_and_wait(hub) => 1 };
    record_owed(hub);
    code
}

/// Before exiting: a job that ended while the journal was busy or unwritable is written now, or the
/// limits would not count it next time.
fn record_owed(hub: &Hub) {
    if let Err(e) = hub.record_owed() {
        warn!("{} finished job(s) could not be written to the journal: {}", hub.state.lock().unwrap_or_else(|e| e.into_inner()).unrecorded.len(), e);
    }
}

async fn stop_and_wait(hub: &Hub) {
    stop_signal().await;
    hub.shutdown(Duration::from_secs(45)).await;
    print_progress(hub);
    proc::kill_all();
}

async fn wait_for_inner(hub: &Hub, ids: &[u32]) -> i32 {
    // The agents run in their own process groups, so a signal sent to us has to be passed on by
    // hand. Stopping this way leaves typed tasks interrupted, to continue with `forkdeck resume`.
    loop {
        tokio::time::sleep(Duration::from_millis(150)).await;
        print_progress(hub);
        let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
        let mine: Vec<_> = st.jobs.iter().filter(|j| ids.contains(&j.id)).collect();
        if mine.len() < ids.len() {
            return 1;
        }
        if mine.iter().all(|j| j.status.finished()) && hub.settled() {
            let code = if mine.iter().all(|j| j.status == JobStatus::Done) { 0 } else { 1 };
            drop(st);
            print_progress(hub);
            return code;
        }
    }
}

/// Print and drain what has happened since the last call. Only for the modes without an interface.
fn print_progress(hub: &Hub) {
    let mut st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
    let lines: Vec<_> = st.log.drain(..).collect();
    for l in &lines {
        say!("{} {:<7} {}", l.time, l.who, l.msg);
    }
    for j in st.jobs.iter_mut() {
        let id = j.id;
        for l in j.worker.tail.iter().skip(j.worker.tail.len().saturating_sub(j.worker.unprinted)) {
            say!("         #{:<3} worker  {}", id, l);
        }
        for l in j.reviewer.tail.iter().skip(j.reviewer.tail.len().saturating_sub(j.reviewer.unprinted)) {
            say!("         #{:<3} review  {}", id, l);
        }
        j.worker.unprinted = 0;
        j.reviewer.unprinted = 0;
        for (i, e) in j.extra.iter_mut().enumerate() {
            for l in e.tail.iter().skip(e.tail.len().saturating_sub(e.unprinted)) {
                say!("         #{:<3} review{} {}", id, i + 3, l);
            }
            e.unprinted = 0;
        }
    }
    let _ = std::io::stdout().flush();
}

/// One pass over the repositories, for cron and for tests: take what is labelled now, finish it, exit.
/// Exits 1 if any job failed.
async fn once(hub: &Hub) -> i32 {
    let code = tokio::select! { result = once_inner(hub) => result, _ = stop_and_wait(hub) => 1 };
    record_owed(hub);
    code
}

async fn once_inner(hub: &Hub) -> i32 {
    issues::poll_once(hub).await;
    loop {
        print_progress(hub);
        let (open, failed) = {
            let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
            (
                st.jobs.iter().filter(|j| !j.status.finished()).count(),
                st.jobs.iter().filter(|j| matches!(j.status, JobStatus::Failed | JobStatus::Cancelled)).count(),
            )
        };
        if open == 0 && hub.settled() {
            print_progress(hub);
            return if failed == 0 { 0 } else { 1 };
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
}

async fn mcp_cmd(hub: &Hub, cmd: McpCmd) -> i32 {
    let (server, run): (&str, _) = match &cmd {
        McpCmd::Tools { server } => (server, None),
        McpCmd::Call { server, tool, args } => (server, Some((tool, args))),
    };
    let Some(sc) = hub.cfg.mcp.get(server) else {
        let known: Vec<&str> = hub.cfg.mcp.keys().map(|k| k.as_str()).collect();
        warn!("no [mcp.{}] in the config. Configured servers: {}", server, if known.is_empty() { "none".to_string() } else { known.join(", ") });
        return 2;
    };
    let mut s = match mcp::Session::open(server, sc).await {
        Ok(s) => s,
        Err(e) => {
            warn!("{}", e);
            return 1;
        }
    };
    let code = match run {
        None => match s.tools().await {
            Ok(tools) => {
                for t in &tools {
                    say!("{:<34} {}", t.name, state::clip(&t.description.replace('\n', " "), 100));
                }
                say!("{} tool(s)", tools.len());
                0
            }
            Err(e) => {
                warn!("{}", e);
                1
            }
        },
        Some((tool, args)) => match serde_json::from_str::<serde_json::Value>(args) {
            Err(e) => {
                warn!("the arguments are not JSON: {}", e);
                2
            }
            Ok(a) => match s.call(tool, a).await {
                Ok(r) => {
                    say!("{}", r.shown);
                    if r.is_error {
                        warn!("the tool reported an error");
                        1
                    } else {
                        0
                    }
                }
                Err(e) => {
                    warn!("{}", e);
                    1
                }
            },
        },
    };
    s.close().await;
    code
}

async fn team_cmd(hub: &Hub, all: bool, json: bool, cmd: Option<TeamCmd>) -> i32 {
    if let Some(TeamCmd::Release { item, force, unclaimed }) = cmd {
        return match team::release(hub, &item, force, unclaimed).await {
            Ok(said) => {
                say!("{}", said);
                0
            }
            Err(e) => {
                warn!("{}", e);
                1
            }
        };
    }
    let (rows, errors) = team::gather(hub, all).await;
    if json {
        say!("{}", serde_json::to_string_pretty(&rows).unwrap_or_default());
    } else {
        use std::io::IsTerminal;
        let width = if std::io::stdout().is_terminal() { crossterm::terminal::size().map(|(w, _)| w as usize).unwrap_or(110).clamp(70, 200) } else { 110 };
        for l in team::text(&rows, width) {
            say!("{}", l);
        }
        if !hub.cfg.team.enabled && !hub.cfg.demo {
            say!("\nThis machine does not share its own work: set enabled = true under [team] to claim with a status comment.");
        }
    }
    for e in &errors {
        warn!("{}", e);
    }
    if errors.is_empty() { 0 } else { 1 }
}

async fn doctor(hub: &Hub, cfg_path: Option<PathBuf>) -> bool {
    let mut ok = true;
    match cfg_path {
        Some(p) => println!("config    {}", p.display()),
        None => println!("config    none found, defaults in use"),
    }
    hub.detect_agents().await;
    for k in AgentKind::ALL {
        let cmd = hub.cfg.agents.get(k).cmd.unwrap_or_default();
        match issues::sh(&cmd, &["--version"], None).await {
            Ok(v) => println!("{:<9} ok       {}", k.name(), v.lines().next().unwrap_or("")),
            Err(_) => println!("{:<9} missing  `{}` is not on PATH", k.name(), cmd),
        }
    }
    {
        let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
        if st.has(AgentKind::Copilot) {
            let p = &st.copilot;
            let who = p.login.clone().unwrap_or_else(|| "this account".into());
            match (&p.error, p.seat, &p.enabled) {
                (Some(e), _, _) => println!("copilot   unknown  could not list models: {}", e),
                (None, Some(false), _) => println!("copilot   no access  {} cannot use Copilot from the CLI (see github.com/settings/copilot)", who),
                (None, _, Some(m)) => println!("copilot   ok       logged in as {}, {} models enabled, {} switched off", who, m.len(), p.disabled.len()),
                _ => {}
            }
        }
        let a = orchestrator::avail_of(&hub.cfg, &st);
        let mut names: Vec<AgentRef> = vec![AgentRef::Kind(AgentKind::Claude), AgentRef::Kind(AgentKind::Codex)];
        let mut profiles: Vec<&String> = hub.cfg.agents.copilot.profiles.keys().filter(|n| *n != "claude" && *n != "codex").collect();
        profiles.sort();
        names.extend(profiles.into_iter().map(|n| AgentRef::Profile(n.clone())));
        let mut workers = 0;
        for r in &names {
            let e = pick::resolve(&hub.cfg, &a.borrow(), r, state::Tier::Standard);
            if matches!(r, AgentRef::Kind(_)) && e.is_some() {
                workers += 1;
            }
            match e {
                Some(e) if e.via.is_some() => println!("{:<9} via      {} (standard tier: {})", r.name(), e.label(), if e.model.is_empty() { "default model" } else { &e.model }),
                Some(_) => {}
                None if st.copilot_usable() => println!("{:<9} none     no model of this profile is enabled for the account", r.name()),
                None => {}
            }
        }
        if workers == 0 {
            ok = false;
            println!("          at least one of claude, codex or a usable copilot is required");
        } else if let Some(w) = pick::worker_for(&hub.cfg, &a.borrow(), &AgentRef::Kind(hub.cfg.router.default_agent), state::Tier::Standard) {
            if pick::reviewer_for(&hub.cfg, &a.borrow(), &w, state::Tier::Heavy).1 {
                println!("          work and review will use the same vendor's models");
            }
        }
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
    if hub.cfg.team.enabled {
        for r in &hub.cfg.repos {
            match team::me(hub, r).await {
                Ok(name) => {
                    let public = team::is_public(hub, r).await;
                    let host = team::shown_host(hub, r).await;
                    let shown = match (host.is_empty(), public, hub.cfg.team.host.is_some()) {
                        (true, true, false) => "no machine name (public repository)".to_string(),
                        (true, ..) => "no machine name".to_string(),
                        (false, true, _) => format!("machine name {:?} from the config, on a public repository", host),
                        (false, false, _) => format!("machine name {:?}", host),
                    };
                    println!("team      ok       {} claims as {}, {}", r.repo, name, shown);
                }
                Err(e) => {
                    ok = false;
                    println!("team      problem  {}: cannot tell who is logged in: {}", r.repo, e);
                }
            }
        }
    } else if !hub.cfg.repos.is_empty() {
        println!("team      off      work is not shared with teammates ([team] enabled = true)");
    }
    if !hub.cfg.repos.is_empty() {
        let l = &hub.cfg.limits;
        let part = |n: u64, what: &str| if n == 0 { format!("no limit on {}", what) } else { format!("{} {}", n, what) };
        println!(
            "limits    {:<8} {} · {} · {} · {}",
            if l.jobs_per_day == 0 && l.tokens_per_day == 0 { "open" } else { "set" },
            part(l.jobs_per_day as u64, "jobs a day"),
            part(l.tokens_per_day, "tokens a day"),
            if l.job_mins == 0 { "no time limit per job".to_string() } else { format!("{} min per job", l.job_mins) },
            if l.pause_after_failures == 0 { "no pause on failures".to_string() } else { format!("pause {} min after {} failures in a row", l.pause_mins, l.pause_after_failures) }
        );
        let k = &hub.cfg.knowledge;
        println!("notes     {:<8} {}", if k.enabled { "on" } else { "off" }, if k.learn { "rules are learned from review and check failures" } else { "nothing is learned automatically ([knowledge] learn = true)" });
    }
    // Every tool a stage names has to exist on its server; a typo would otherwise show up as a stuck pipeline.
    let mut offered: std::collections::HashMap<String, Vec<String>> = Default::default();
    let mut names: Vec<&String> = hub.cfg.mcp.keys().collect();
    names.sort();
    for name in names {
        let listed = match mcp::Session::open(name, &hub.cfg.mcp[name]).await {
            Ok(mut s) => {
                let t = s.tools().await;
                s.close().await;
                t
            }
            Err(e) => Err(e),
        };
        match listed {
            Ok(tools) => {
                println!("mcp       ok       {} offers {} tool(s)", name, tools.len());
                offered.insert(name.clone(), tools.into_iter().map(|t| t.name).collect());
            }
            Err(e) => {
                ok = false;
                println!("mcp       problem  {}", e);
            }
        }
    }
    for r in &hub.cfg.repos {
        let mut calls: Vec<(&str, &str, &config::CallCfg)> = Vec::new();
        if let Some(ci) = &r.ci {
            calls.push(("ci.status", &ci.server, &ci.status));
            calls.extend(ci.logs.iter().map(|c| ("ci.logs", ci.server.as_str(), c)));
        }
        if let Some(d) = &r.deploy {
            calls.push(("deploy.status", &d.server, &d.status));
            for (n, c) in [("deploy.trigger", &d.trigger), ("deploy.rollback", &d.rollback), ("deploy.notify", &d.notify)] {
                calls.extend(c.iter().map(|c| (n, d.server.as_str(), c)));
            }
            if d.rollback.is_none() {
                println!("stage     note     {} deploys to {} with no rollback configured", r.repo, d.environment);
            }
        }
        for (what, stage_server, c) in calls {
            let server = c.server.as_deref().unwrap_or(stage_server);
            if let Some(tools) = offered.get(server) {
                if !tools.contains(&c.tool) {
                    ok = false;
                    println!("stage     problem  {} {}: {} has no tool named {:?} (forkdeck mcp tools {})", r.repo, what, server, c.tool, server);
                }
            }
        }
    }
    if hub.cfg.repos.is_empty() {
        println!("repos     none     add [[repos]] entries to pick up labelled issues");
    }
    ok
}

fn new_ui(hub: &Hub, cwd: PathBuf) -> ui::Ui {
    ui::Ui { input: String::new(), selected: None, follow: true, forced: None, cwd, demo: hub.cfg.demo, team: false, new_task: false, plan_next: false, jobs_per_day: hub.cfg.limits.jobs_per_day, tokens_per_day: hub.cfg.limits.tokens_per_day }
}

fn sync_selection(hub: &Hub, u: &mut ui::Ui) {
    let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
    if u.follow || u.selected.is_none() {
        // Follow the newest job that is still moving, else the newest.
        u.selected = st.jobs.iter().rev().find(|j| !j.status.finished()).or(st.jobs.last()).map(|j| j.id);
    }
}

fn snapshot(rt: &tokio::runtime::Runtime, hub: &Hub, cwd: PathBuf, path: &std::path::Path, size: &str, after: u64, team_pane: bool) -> anyhow::Result<()> {
    let (w, h) = size.split_once('x').ok_or_else(|| anyhow::anyhow!("--size wants WIDTHxHEIGHT"))?;
    let (w, h): (u16, u16) = (w.parse()?, h.parse()?);
    let mut u = new_ui(hub, cwd.clone());
    u.team = team_pane;
    if hub.cfg.demo {
        let _ = rt.block_on(hub.submit("Fix the failing retry test in the network client".into(), "Fix the failing retry test in the network client".into(), Source::Manual, cwd, None, state::Planning::IfRouted));
    }
    rt.block_on(async { tokio::time::sleep(Duration::from_secs(after)).await });
    sync_selection(hub, &mut u);
    let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h))?;
    term.draw(|f| ui::draw(f, &hub.state.lock().unwrap_or_else(|e| e.into_inner()), &u))?;
    let buf = term.backend().buffer();
    let out = if path.extension().map(|e| e == "html").unwrap_or(false) { ui::to_html(buf) } else { ui::to_text(buf) };
    std::fs::write(path, out)?;
    Ok(())
}

fn tui(rt: &tokio::runtime::Runtime, hub: &Hub, cwd: PathBuf) -> anyhow::Result<()> {
    let mut u = new_ui(hub, cwd.clone());
    if hub.cfg.demo {
        let _ = rt.block_on(hub.submit("Fix the failing retry test in the network client".into(), "Fix the failing retry test in the network client".into(), Source::Manual, cwd.clone(), None, state::Planning::IfRouted));
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
    // What was typed and then refused (a reply to a job another forkdeck holds, say) comes back to
    // the input line rather than being lost.
    let bounced: std::sync::Arc<std::sync::Mutex<Vec<String>>> = Default::default();
    let bounce = |text: String| {
        let b = bounced.clone();
        move || b.lock().unwrap_or_else(|e| e.into_inner()).push(text)
    };
    let res = (|| -> anyhow::Result<()> {
        loop {
            if stopping.load(std::sync::atomic::Ordering::SeqCst) {
                // The signal task owns the shutdown and will exit the process; just stay out of its way.
                std::thread::sleep(Duration::from_millis(200));
                continue;
            }
            for text in std::mem::take(&mut *bounced.lock().unwrap_or_else(|e| e.into_inner())) {
                // In front of anything typed since, which stays as it is.
                u.input = if u.input.trim().is_empty() { text } else { format!("{} {}", text, u.input) };
            }
            sync_selection(hub, &mut u);
            term.draw(|f| ui::draw(f, &hub.state.lock().unwrap_or_else(|e| e.into_inner()), &u))?;
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
                            let mut st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
                            st.automation = !st.automation;
                            let on = st.automation;
                            let none = st.repos.is_empty();
                            st.log("issues", if on { "polling on" } else { "polling paused" });
                            if on && none {
                                st.log("issues", "no [[repos]] configured, nothing to poll");
                            }
                        }
                        KeyCode::Char('r') if ctrl => {
                            if let Some(id) = u.selected {
                                let h = hub.clone();
                                rt.spawn(async move { if let Err(why) = h.resume(id).await { h.log("queue", format!("#{} not resumed: {}", id, why)); } });
                            }
                        }
                        KeyCode::Char('x') if ctrl => {
                            if let Some(id) = u.selected {
                                hub.cancel(id);
                            }
                        }
                        KeyCode::Char('t') if ctrl => u.team = !u.team,
                        KeyCode::Char('n') if ctrl => u.new_task = !u.new_task,
                        KeyCode::Char('p') if ctrl => u.plan_next = !u.plan_next,
                        // Send what is typed to the selected task now, stopping its agent to take it.
                        KeyCode::Char('s') if ctrl => {
                            let text = u.input.trim().to_string();
                            let target = ui::reply_target(&hub.state.lock().unwrap_or_else(|e| e.into_inner()), &u);
                            if let (Some(id), false) = (target, text.is_empty()) {
                                u.input.clear();
                                let (h, back) = (hub.clone(), bounce(text.clone()));
                                rt.spawn(async move { if let Err(why) = h.steer_now(id, text).await { h.log("queue", format!("#{} not sent: {}", id, why)); back(); } });
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
                            // Profiles are for --agent and the config; Tab steps through the CLIs.
                            u.forced = match u.forced.as_ref().map(|r| r.kind()) {
                                None => Some(AgentRef::Kind(AgentKind::Claude)),
                                Some(AgentKind::Claude) => Some(AgentRef::Kind(AgentKind::Codex)),
                                Some(AgentKind::Codex) => Some(AgentRef::Kind(AgentKind::Copilot)),
                                Some(AgentKind::Copilot) => None,
                            }
                        }
                        KeyCode::Up | KeyCode::Down => {
                            let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
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
                                let target = ui::reply_target(&hub.state.lock().unwrap_or_else(|e| e.into_inner()), &u);
                                match target {
                                    // A typed job is selected: carry on its conversation.
                                    Some(id) => {
                                        u.input.clear();
                                        let (h, back) = (hub.clone(), bounce(task.clone()));
                                        rt.spawn(async move { if let Err(why) = h.reply(id, task).await { h.log("queue", format!("#{} not sent: {}", id, why)); back(); } });
                                    }
                                    None => {
                                        u.input.clear();
                                        let planning = if u.plan_next { state::Planning::Always } else { state::Planning::IfRouted };
                                        let (h, dir, forced, back) = (hub.clone(), cwd.clone(), u.forced.clone(), bounce(task.clone()));
                                        rt.spawn(async move { if let Err(e) = h.submit(state::clip(&task, 80), task, Source::Manual, dir, forced, planning).await { h.log("queue", e); back(); } });
                                        u.plan_next = false;
                                        u.selected = None;
                                        u.follow = true;
                                        u.new_task = false;
                                    }
                                }
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
    let busy = hub.state.lock().unwrap_or_else(|e| e.into_inner()).jobs.iter().filter(|j| !j.status.finished()).count();
    if busy > 0 {
        warn!("stopping {} job(s) and handing claimed issues back…", busy);
    }
    let left = rt.block_on(hub.shutdown(Duration::from_secs(45)));
    if left > 0 {
        warn!("{} job(s) did not finish in time; check their issues for a stale working label", left);
    }
    record_owed(hub);
    proc::kill_all();
    res
}
