mod adaptive;
mod agents;
mod buildcache;
mod catalog;
mod choices;
mod benchmark;
mod cicd;
mod config;
mod copilot;
mod demo;
mod diff;
mod history;
mod input;
mod issues;
mod journal;
mod knowledge;
mod mcp;
mod metrics;
mod orchestrator;
mod pick;
mod plan_ui;
mod planning;
mod proc;
mod remote;
mod routemem;
mod router;
mod schedule;
mod server;
mod state;
mod subissues;
mod taskgraph;
mod team;
mod tracker;
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
    /// Run the local web UI and orchestration daemon
    Serve,
    /// Run paired model benchmarks in disposable local clones (executes agents)
    Benchmark {
        manifest: PathBuf,
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        preflight: bool,
    },
    /// Compare reviewers against labeled defects and clean changes (never enables routing)
    ReviewerBenchmark {
        manifest: PathBuf,
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        preflight: bool,
    },
    /// Inspect adaptive decisions, suspensions and durable trial reservations
    Adaptive {
        /// Print full audit records as JSON
        #[arg(long)]
        json: bool,
        /// Evaluate a paired benchmark evidence file against configured gates
        #[arg(long)]
        evaluate: Option<PathBuf>,
        /// Disable new adaptive assignments immediately, across running instances
        #[arg(long, conflicts_with_all=["enable", "evaluate"])]
        disable: bool,
        /// Re-enable assignments after evidence qualification (suspensions remain binding)
        #[arg(long, conflicts_with_all=["disable", "evaluate"])]
        enable: bool,
    },
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
        /// Group invocation usage by actual model, effort, and role
        #[arg(long)]
        by_model: bool,
        #[arg(long, requires = "by_model")]
        role: Option<String>,
        #[arg(long, requires = "by_model")]
        repo: Option<String>,
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
    /// Control from a phone on the local network: pair one, list or revoke paired phones
    Remote {
        #[command(subcommand)]
        cmd: RemoteCmd,
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
enum RemoteCmd {
    /// Show a one-time QR code for the forkdeck app; the phone must be on this machine's subnet
    Pair {
        /// Network interface the phone will connect through (default: the first private IPv4 address)
        #[arg(long)]
        iface: Option<String>,
        /// Seconds the code works for
        #[arg(long, default_value_t = 120)]
        ttl: u64,
    },
    /// List paired phones
    Devices,
    /// Unpair a phone; a running forkdeck disconnects it
    Revoke { name: String },
    /// Run polling and the remote listener without the interface
    #[command(hide = true)]
    Serve,
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
    // Evidence is matched against the configuration as written, whatever is filled in below.
    cfg.adaptive.written = Some(adaptive::fingerprint(&cfg));
    if !cfg.demo {
        // Only where agents may run: asking the CLI takes a moment.
        let agents_run = matches!(cli.cmd, None | Some(Cmd::Once) | Some(Cmd::Run { .. }) | Some(Cmd::Resume) | Some(Cmd::Doctor) | Some(Cmd::Serve) | Some(Cmd::Remote { cmd: RemoteCmd::Serve }));
        let chosen = if agents_run && cfg.agents.discover_models.unwrap_or(true) { catalog::fill_codex(&mut cfg) } else { vec![] };
        if !chosen.is_empty() && matches!(cli.cmd, Some(Cmd::Doctor)) {
            println!("models    codex: {}", chosen.join(", "));
        }
        if let Some((r, automation)) = config::launch_repo(&std::env::current_dir()?, &cfg)? {
            cfg.repos.push(r);
            cfg.automation.enabled |= automation;
            cfg.validate()?;
        }
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
                agent_domains: vec![],
                provider: Provider::Github,
                repo: "acme/payments".into(),
                path: PathBuf::from("."),
                base: "main".into(),
                label: "agent:ready".into(),
                working_label: "agent:working".into(),
                done_label: "agent:review".into(),
                failed_label: "agent:needs-human".into(),
                plan_label: "forkdeck:plan".into(),
                human_label: "needs-human".into(),
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
    if cfg.demo && matches!(cli.cmd, Some(Cmd::Serve) | Some(Cmd::Remote { cmd: RemoteCmd::Serve })) { server::seed::configure(&mut cfg); }
    let rt = tokio::runtime::Runtime::new()?;
    let hub = Hub::new(cfg, rt.handle().clone()).with_history();
    *hub.config_path.lock().unwrap() = cfg_path.clone();
    let cwd = std::env::current_dir()?;

    match cli.cmd {
        Some(Cmd::Route { task }) => {
            let task = task.join(" ");
            anyhow::ensure!(!task.trim().is_empty(), "give a task to route");
            // As a task typed here would be routed: memory first, for this directory.
            let here = std::env::current_dir()?;
            let remembered = orchestrator::route_scope(&Source::Manual, &here).and_then(|(scope, kind)| orchestrator::recall(&hub, None, &scope, kind, &task));
            if let Some(d) = remembered {
                println!("{}", serde_json::to_string_pretty(&d)?);
                return Ok(());
            }
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
        Some(Cmd::ReviewerBenchmark { manifest, output, preflight }) => {
            rt.block_on(benchmark::reviewers(&hub, &manifest, &output, preflight)).map_err(anyhow::Error::msg)
        }
        Some(Cmd::Benchmark { manifest, output, preflight }) => {
            rt.block_on(benchmark::run(&hub, &manifest, &output, preflight)).map_err(anyhow::Error::msg)
        }
        Some(Cmd::Adaptive { json, evaluate, disable, enable }) => {
            if disable || enable {
                adaptive::set_disabled(&hub.cfg, disable).map_err(anyhow::Error::msg)?;
            }
            if let Some(path) = evaluate {
                let q = adaptive::evaluate(&hub.cfg, &path).map_err(anyhow::Error::msg)?;
                println!("{}", serde_json::to_string_pretty(&q)?);
            } else {
                let s = adaptive::status().map_err(anyhow::Error::msg)?;
                let production = adaptive::production_reports(&hub.cfg.adaptive, &s);
                if json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(
                            &serde_json::json!({"state":s,"production":production,"fingerprint":adaptive::fingerprint(&hub.cfg)})
                        )?
                    );
                } else {
                    println!("Mode: {:?}; configuration fingerprint: {}", hub.cfg.adaptive.mode, adaptive::fingerprint(&hub.cfg));
                    println!("Operator disable switch: {}", s.disabled);
                    println!(
                        "Objective: {:?}; invocation USD ceiling: {:?} (capability probed per CLI; no hard token ceiling)",
                        hub.cfg.adaptive.objective, hub.cfg.adaptive.invocation_max_usd
                    );
                    for p in production {
                        println!("Observed trial {} / {}: {} candidates, {} baseline, {} pending; savings {:?}, 95% lower {:?}; reserved {} tokens; gate {:?}",p.policy,p.cohort,p.candidate_tasks,p.baseline_tasks,p.pending,p.observed_savings,p.observed_savings_lower,p.reserved_tokens,p.gate_failure);
                    }
                    for (cohort, reason) in s.suspended {
                        println!("Suspended {cohort}: {reason}");
                    }
                    for d in s.decisions.iter().rev().take(30) {
                        println!(
                            "{} {:?} {} -> {} / {}: {}; probability {}; reserved {}; observed tokens {:?}; accepted {:?}",
                            d.id,
                            d.mode,
                            d.baseline.model,
                            d.executed.model,
                            d.executed.effort,
                            d.reason,
                            d.probability,
                            d.reserved,
                            d.tokens,
                            d.accepted
                        );
                        if let Some(q) = &d.qualification {
                            println!("  Predicted savings {:.1}% (95% interval {:.1}–{:.1}%); {} paired tasks; quality difference lower {:.3}; p90 {} ms",100.0*q.predicted_savings,100.0*q.savings_lower,100.0*q.savings_upper,q.tasks,q.quality_difference_lower,q.candidate_p90_ms);
                        }
                    }
                }
            }
            Ok(())
        }
        Some(Cmd::Stats { days, jobs, json, by_model, role, repo }) => {
            let days = days.clamp(1, 3650);
            let entries = journal::read(journal::now() - days as i64 * journal::DAY)
                .map_err(|e| anyhow::anyhow!("the journal cannot be read: {}", e))?;
            if by_model {
                let report = metrics::report(journal::now() - days as i64 * journal::DAY, role.as_deref(), repo.as_deref(), &entries)
                    .map_err(anyhow::Error::msg)?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&report)?);
                } else {
                    println!("{}", report.note);
                    println!(
                        "Damaged records: {}; historical jobs without attempts: {}",
                        report.damaged_records, report.historical_jobs_without_attempts
                    );
                    for t in &report.tasks {
                        println!(
                            "  Cohort {:?}; reported USD/accepted {:?}; cost coverage {}/{}",
                            t.cohort, t.reported_usd_per_accepted_task, t.complete_cost_tasks, t.tasks
                        );
                        println!("Task cohort {} / {} / {}: {} evaluated, {} accepted, {} unknown; {} first pass; tokens/accepted {:?} (partial: {}); {} known excluded tokens",t.repo,t.source,t.initial_model,t.evaluated,t.accepted,t.unknown_quality,t.first_pass,t.tokens_per_accepted_task_lower_bound,t.partial_usage,t.known_excluded_tokens);
                        println!("Queue wait p90 {:?} ms; measured {}/{} tasks (live admission to dispatch)", t.p90_queue_wait_ms, t.queue_coverage, t.tasks);
                    }
                    for g in report.groups {
                        println!("{} / {} / {} / {}: {} attempts, {} complete usage, {} incomplete, {} cancelled, {} errors; input {} output {} cache-read {} cache-write {}; median {:?} p90 {:?} tokens; p90 {:?} ms", g.cli,g.model,g.effort,g.role,g.attempts,g.complete_usage,g.incomplete,g.cancelled,g.errors,g.known_input,g.known_output,g.known_cache_read,g.known_cache_write,g.median_tokens,g.p90_tokens,g.p90_wall_ms);
                    }
                }
                return Ok(());
            }
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
        Some(Cmd::Remote { cmd }) => match cmd {
            RemoteCmd::Pair { iface, ttl } => remote::pair_cmd(&hub.cfg, iface.as_deref(), ttl),
            RemoteCmd::Devices => remote::devices_cmd(&hub.cfg),
            RemoteCmd::Revoke { name } => remote::revoke_cmd(&hub.cfg, &name),
            RemoteCmd::Serve => server::run(&rt, hub),
        },
        Some(Cmd::Serve) => server::run(&rt, hub),
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
            rt.block_on(hub.detect_agents_in_background());
            if hub.cfg.remote.enabled {
                let h = hub.clone();
                rt.spawn(async move {
                    if let Err(e) = remote::serve(h.clone()).await {
                        h.log("remote", format!("not listening: {}", e));
                    }
                });
            }
            if let Some(path) = cli.snapshot {
                rt.spawn(issues::poll_loop(hub.clone()));
                rt.spawn(team::watch_loop(hub.clone()));
                return snapshot(&rt, &hub, cwd, &path, &cli.size, cli.after, cli.team_pane);
            }
            // Earlier jobs come back; typed tasks that were cut off continue by themselves.
            hub.start_autosave();
            for id in hub.restore() {
                if let Err(e) = rt.block_on(hub.resume(id)) {
                    hub.log("queue", format!("#{} not resumed: {} (select it and press Enter to try again)", id, e));
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
            let result = tui(&rt, &hub, cwd);
            // Tokio's default drop waits indefinitely for blocking tasks (for example DNS).
            // The interface has already stopped its jobs and child processes.
            rt.shutdown_timeout(Duration::from_millis(200));
            result
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
    if agents::claude_sandboxed(&hub.cfg) {
        println!("sandbox   on       claude workers run any command in Claude Code's sandbox");
    } else if hub.cfg.agents.sandbox == Some(false) {
        println!("sandbox   off      claude workers run only the build commands found for each repository (agents.sandbox = false)");
    } else {
        println!("sandbox   off      claude workers run only the build commands found for each repository; install bubblewrap and socat to sandbox them");
    }
    if agents::claude_sandboxed(&hub.cfg) {
        let n = &hub.cfg.agents.network;
        let found = if n.detect_mirrors { buildcache::machine_mirrors() } else { vec![] };
        let mut open = vec![if n.public_registries { "the package registries of each repository's languages".to_string() } else { "no public registries".to_string() }];
        if !found.is_empty() { open.push(format!("mirrors your package managers use: {}", found.join(", "))); }
        if !n.mirrors.is_empty() { open.push(format!("configured mirrors: {}", n.mirrors.join(", "))); }
        println!("network   {}", open.join("; "));
    }
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
    let r = &hub.cfg.remote;
    if r.enabled {
        let phones = remote::devices::Registry::new(&r.dir).list().len();
        match std::net::TcpListener::bind(&r.listen) {
            Ok(_) => println!("remote    on       {} free, {} phone(s) paired", r.listen, phones),
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
                println!("remote    on       {} in use (forkdeck running?), {} phone(s) paired", r.listen, phones)
            }
            Err(e) => {
                ok = false;
                println!("remote    broken   cannot listen on {}: {}", r.listen, e);
            }
        }
    } else {
        println!("remote    off      [remote] enabled = false");
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

/// A click on an edit line opens its diff full screen; while it is open, keys and the wheel move
/// through it and Esc closes it. True when the event was used here.
fn diff_event(hub: &Hub, u: &mut ui::Ui, e: &Event, page: u16) -> bool {
    use crossterm::event::{MouseButton, MouseEventKind};
    if let Some(v) = &mut u.diff {
        match e {
            Event::Key(k) if k.kind != KeyEventKind::Release => match k.code {
                KeyCode::Esc | KeyCode::Char('q') => u.diff = None,
                KeyCode::Up => v.scroll_by(-1, page),
                KeyCode::Down => v.scroll_by(1, page),
                KeyCode::PageUp => v.scroll_by(-(page as i32), page),
                KeyCode::PageDown | KeyCode::Char(' ') => v.scroll_by(page as i32, page),
                KeyCode::Home => v.scroll = 0,
                KeyCode::End => v.scroll_by(i32::MAX / 2, page),
                _ => {}
            },
            Event::Mouse(m) if m.kind == MouseEventKind::ScrollUp => v.scroll_by(-3, page),
            Event::Mouse(m) if m.kind == MouseEventKind::ScrollDown => v.scroll_by(3, page),
            _ => {}
        }
        return true;
    }
    let Event::Mouse(m) = e else { return false };
    if m.kind != MouseEventKind::Down(MouseButton::Left) {
        return true;
    }
    let Some(spot) = crate::diff::spot_at(&u.spots.borrow(), m.column, m.row).cloned() else { return true };
    let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(j) = st.jobs.iter().find(|j| j.id == spot.job) {
        let base = j.base_sha.clone().unwrap_or_else(|| "HEAD".into());
        u.diff = Some(crate::diff::View::open(j.id, spot.files, j.workdir.clone(), base));
    }
    true
}

fn new_ui(hub: &Hub, cwd: PathBuf) -> ui::Ui {
    ui::Ui {
        input: String::new(),
        selected: None,
        follow: true,
        forced: None,
        cwd,
        demo: hub.cfg.demo,
        team: false,
        new_task: false,
        plan_next: false,
        factory: true,
        reply_to: None,
        choices: None,
        issue_panel: None,
        seen_choices: Default::default(),
        jobs_per_day: hub.cfg.limits.jobs_per_day,
        tokens_per_day: hub.cfg.limits.tokens_per_day,
        spots: Default::default(),
        diff: None,
        output_back: Default::default(),
        log_back: Default::default(),
        output_view: Default::default(),
        log_view: Default::default(),
    }
}

/// Ctrl with Shift or Caps Lock arrives as an uppercase letter; it is the same shortcut.
fn ctrl_lowercase(event: Event) -> Event {
    match event {
        Event::Key(mut k) if k.modifiers.contains(KeyModifiers::CONTROL) => {
            if let KeyCode::Char(c) = k.code { k.code = KeyCode::Char(c.to_ascii_lowercase()); }
            Event::Key(k)
        }
        e => e,
    }
}

/// Quit is global, including when a picker has focus. Some terminals report uppercase keys.
fn quit_key(key: event::KeyEvent) -> bool {
    key.kind != KeyEventKind::Release &&
        (key.code == KeyCode::Char('\u{3}') ||
         (key.modifiers.contains(KeyModifiers::CONTROL) &&
          matches!(key.code, KeyCode::Char('c' | 'C' | 'q' | 'Q'))))
}

/// What Enter with nothing typed asks of a finished task.
const CARRY_ON: &str = "Continue the previous task from where it stopped. Inspect the current working tree before making further changes.";

/// What the input line holds: the messages sent from it and an unsent draft. Kept per working
/// directory in the history database, so they are there when the interface opens again.
#[derive(serde::Serialize, serde::Deserialize, Default)]
struct Composer {
    history: Vec<String>,
    draft: String,
}

fn composer_key(cwd: &std::path::Path) -> String {
    std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf()).display().to_string()
}

fn load_composer(hub: &Hub, ui: &mut ui::Ui, history: &mut input::History) {
    let Some(h) = &hub.history else { return };
    let Some(c) = h.composer(&composer_key(&ui.cwd)).and_then(|s| serde_json::from_str::<Composer>(&s).ok()) else { return };
    for m in c.history {
        history.submitted(m);
    }
    ui.input = c.draft;
}

/// Saved when it changed. `last` is what was saved before.
fn save_composer(hub: &Hub, ui: &ui::Ui, history: &input::History, last: &mut String) {
    let Some(h) = &hub.history else { return };
    let draft = ui.issue_panel.as_ref().and_then(plan_ui::Panel::draft).unwrap_or(&ui.input).to_string();
    let Ok(json) = serde_json::to_string(&Composer { history: history.messages().to_vec(), draft }) else { return };
    if *last != json && h.save_composer(&composer_key(&ui.cwd), &json).is_ok() {
        *last = json;
    }
}

/// Enter: a new task, or what is typed for the task the input box names. With nothing typed, a
/// task cut off by a stop resumes and a finished one is asked to carry on. Refused (another
/// forkdeck holds the task, say), what was typed stays on the line.
fn submit_input(hub: &Hub, ui: &mut ui::Ui, history: &mut input::History) {
    let target = ui::reply_target(&hub.state.lock().unwrap_or_else(|e| e.into_inner()), ui);
    let typed = ui.input.trim().to_string();
    let sent = match (target, typed.is_empty()) {
        (None, true) => return,
        (Some(id), true) => match hub.job(id, |j| j.status) {
            Some(JobStatus::Interrupted) => hub.rt.block_on(hub.resume(id)).map(|_| id),
            Some(s) if s.finished() => hub.rt.block_on(hub.reply(id, CARRY_ON.into())).map(|_| id),
            _ => return,
        },
        (Some(id), false) => hub.rt.block_on(hub.reply(id, typed.clone())).map(|_| id),
        (None, false) => {
            let planning = if ui.plan_next { state::Planning::Always } else { state::Planning::IfRouted };
            hub.rt.block_on(hub.submit(state::clip(&typed, 80), typed.clone(), Source::Manual, ui.cwd.clone(), ui.forced.clone(), planning))
        }
    };
    match sent {
        Ok(id) => {
            if !typed.is_empty() {
                history.submitted(typed);
            }
            ui.input.clear();
            ui.selected = Some(id);
            ui.follow = false;
            ui.reply_to = Some(id);
            ui.new_task = false;
            ui.plan_next = false;
        }
        Err(why) => hub.log("queue", match target {
            Some(id) => format!("#{} not sent: {}", id, why),
            None => format!("not started: {}", why),
        }),
    }
}

/// A task whose choices can be picked from: waiting at its plan, or finished with an answer that asks.
fn choosable(j: &state::Job) -> bool {
    j.source.is_manual() && (j.status.finished() || j.status == JobStatus::Choosing)
}

fn open_choices(hub: &Hub, ui: &mut ui::Ui) {
    let picker = hub.state.lock().unwrap_or_else(|e| e.into_inner()).jobs.iter()
        .find(|j| Some(j.id) == ui.selected && choosable(j))
        .and_then(|j| {
            let options = choices::for_job(j);
            if options.is_empty() { None } else { Some(choices::Picker { job: j.id, turn: j.turns.len(), options, selected: 0 }) }
        });
    if let Some(picker) = picker {
        ui.seen_choices.insert((picker.job, picker.turn));
        ui.choices = Some(picker);
    } else {
        hub.log("choices", "no choices for this task; you can type a reply");
    }
}

/// Opens the picker by itself once for each answer that offers choices.
fn sync_choices(hub: &Hub, ui: &mut ui::Ui) {
    if ui.issue_panel.is_some() { return; }
    if let Some(picker) = &ui.choices {
        let valid = hub.job(picker.job, |j| j.turns.len() == picker.turn && choosable(j)).unwrap_or(false);
        if !valid { ui.choices = None; }
        return;
    }
    if ui.new_task || !ui.input.is_empty() { return; }
    let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
    let Some(job) = st.jobs.iter().find(|j| Some(j.id) == ui.selected && choosable(j)) else { return };
    // Implementation steps are not choices once a tracked plan has reached review.
    if job.issue_plan.as_ref().is_some_and(|p| !p.review.is_empty() || p.reviewed.is_some()) { return; }
    if !ui.seen_choices.insert((job.id, job.turns.len())) { return; }
    let options = choices::for_job(job);
    if !options.is_empty() {
        ui.choices = Some(choices::Picker { job: job.id, turn: job.turns.len(), options, selected: 0 });
    }
}

fn choice_key(hub: &Hub, ui: &mut ui::Ui, history: &mut input::History, key: event::KeyEvent) -> bool {
    let Some(picker) = &mut ui.choices else { return false };
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        if matches!(key.code, KeyCode::Char('t' | 'n' | 'c' | 'q')) { ui.choices = None; }
        return false;
    }
    match key.code {
        KeyCode::Up => picker.selected = picker.selected.saturating_sub(1),
        KeyCode::Down | KeyCode::Tab => picker.selected = (picker.selected + 1).min(picker.options.len()),
        KeyCode::Esc | KeyCode::F(5) => { ui.choices = None; }
        KeyCode::Char(c @ '1'..='9') => {
            picker.selected = ((c as u8 - b'1') as usize).min(picker.options.len());
        }
        KeyCode::Enter | KeyCode::Char('c') => {
            let picker = ui.choices.take().unwrap();
            let Some(waiting) = hub.job(picker.job, |j| (j.turns.len() == picker.turn && choosable(j)).then_some(j.status == JobStatus::Choosing)).flatten() else {
                hub.log("choices", "the task has changed; reopen its current choices with F5");
                return true;
            };
            ui.reply_to = Some(picker.job);
            ui.selected = Some(picker.job);
            ui.follow = false;
            ui.new_task = false;
            if key.code == KeyCode::Enter && picker.selected < picker.options.len() {
                if waiting {
                    // A plan waiting for its choice takes the option itself; anything typed goes along as a message.
                    // What is typed goes along as a message, queued before the choice wakes the
                    // worker. Sent as a reply, it would arrive while the task still waits for its
                    // choice and be taken as another choice.
                    let draft = ui.input.trim().to_string();
                    if !draft.is_empty() {
                        hub.job(picker.job, |j| { j.inbox.push_back(draft.clone()); j.worker.push(format!("› {} (queued)", state::clip(&draft, 300))); });
                    }
                    match hub.choose(picker.job, state::Choice::Option(picker.selected)) {
                        Ok(()) => {
                            if !draft.is_empty() { history.submitted(draft); ui.input.clear(); }
                        }
                        Err(why) => {
                            if !draft.is_empty() { hub.job(picker.job, |j| { j.inbox.pop_back(); }); }
                            hub.log("choices", format!("#{} not chosen: {}", picker.job, why));
                        }
                    }
                } else {
                    let option = &picker.options[picker.selected];
                    // Keep any draft as additional guidance, never discard it when selecting an option.
                    let draft = ui.input.trim();
                    ui.input = format!("For planning, I choose option {}.{}", option.label,
                        if draft.is_empty() { String::new() } else { format!("\n\n{}", draft) });
                    submit_input(hub, ui, history);
                }
            }
            // The custom entry leaves the input line ready for the user's own words.
        }
        _ => {}
    }
    true
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
    let mut history = input::History::default();
    load_composer(hub, &mut u, &mut history);
    let mut composer_saved = String::new();
    let mut composer_at = std::time::Instant::now();
    rt.spawn(issues::poll_loop(hub.clone()));
    rt.spawn(team::watch_loop(hub.clone()));
    if hub.cfg.demo {
        let _ = rt.block_on(hub.submit("Fix the failing retry test in the network client".into(), "Fix the failing retry test in the network client".into(), Source::Manual, cwd.clone(), None, state::Planning::IfRouted));
    }
    // A signal (terminal closed, kill, service stop) is handled entirely on the runtime: the main
    // thread may be stuck reading a terminal that no longer exists. A second one exits at once.
    let stopping = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    {
        let hub = hub.clone();
        let stopping = stopping.clone();
        rt.spawn(async move {
            stop_signal().await;
            let already = stopping.swap(true, std::sync::atomic::Ordering::SeqCst);
            let _ = crossterm::execute!(std::io::stdout(), event::DisableBracketedPaste, event::DisableMouseCapture);
            let _ = ratatui::try_restore();
            if already {
                proc::kill_all();
                std::process::exit(130);
            }
            warn!("stopping jobs and handing claimed issues back… (again to quit at once)");
            tokio::select! {
                _ = hub.shutdown(Duration::from_secs(45)) => {}
                _ = stop_signal() => {}
            }
            record_owed(&hub);
            proc::kill_all();
            std::process::exit(1);
        });
    }
    let mut term = ratatui::init();
    // Mouse capture: clicking an edit line opens its diff. Shift-drag still selects text.
    let _ = crossterm::execute!(std::io::stdout(), event::EnableBracketedPaste, event::EnableMouseCapture);
    let res = (|| -> anyhow::Result<()> {
        loop {
            if stopping.load(std::sync::atomic::Ordering::SeqCst) {
                // The signal task owns the shutdown and will exit the process; just stay out of its way.
                std::thread::sleep(Duration::from_millis(200));
                continue;
            }
            if composer_at.elapsed() >= Duration::from_secs(1) {
                save_composer(hub, &u, &history, &mut composer_saved);
                composer_at = std::time::Instant::now();
            }
            sync_selection(hub, &mut u);
            sync_choices(hub, &mut u);
            if let Some(v) = &mut u.diff { v.refresh(); }
            term.draw(|f| ui::draw(f, &hub.state.lock().unwrap_or_else(|e| e.into_inner()), &u))?;
            if !event::poll(Duration::from_millis(100))? {
                continue;
            }
            let page = term.size().map(|s| s.height.saturating_sub(2)).unwrap_or(20);
            let event = ctrl_lowercase(event::read()?);
            if diff_event(hub, &mut u, &event, page) {
                continue;
            }
            match event {
                Event::Paste(s) => {
                    if let Some(panel) = &mut u.issue_panel { panel.paste(&s); }
                    else { u.input.push_str(&s.replace(['\n', '\r'], " ")); }
                }
                Event::Key(k) if k.kind != KeyEventKind::Release => {
                    if quit_key(k) { return Ok(()); }
                    if k.code == KeyCode::F(6) { plan_ui::open(hub, &mut u); continue; }
                    if plan_ui::key(hub, &mut u, &mut history, k) { continue; }
                    if choice_key(hub, &mut u, &mut history, k) { continue; }
                    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
                    match k.code {
                        KeyCode::Char('a') if ctrl => {
                            let on = !hub.state.lock().unwrap_or_else(|e| e.into_inner()).automation;
                            hub.automation(on);
                            let mut st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
                            let none = st.repos.is_empty();
                            if on && none {
                                st.log("issues", "no [[repos]] configured, nothing to poll");
                            }
                        }
                        KeyCode::Char('x') if ctrl => {
                            if let Some(id) = u.selected {
                                hub.cancel(id);
                            }
                        }
                        KeyCode::F(2) => { u.factory = !u.factory; u.team = false; }
                        KeyCode::F(4) => u.team = !u.team,
                        KeyCode::F(5) => open_choices(hub, &mut u),
                        KeyCode::Char('t') | KeyCode::Char('n') if ctrl => { u.reply_to = None; u.new_task = true; }
                        // The next new task proposes approaches first.
                        // Shift-Tab as in Claude Code's plan mode.
                        KeyCode::Char('p') if ctrl => {
                            u.plan_next = !u.plan_next;
                            if u.plan_next { u.reply_to = None; u.new_task = true; }
                        }
                        KeyCode::BackTab => {
                            u.plan_next = !u.plan_next;
                            if u.plan_next { u.reply_to = None; u.new_task = true; }
                        }
                        // Pin replies to the selected typed task, whatever is selected later.
                        KeyCode::Char('r') if ctrl => {
                            let target = hub.state.lock().unwrap_or_else(|e| e.into_inner()).jobs.iter().find(|j| Some(j.id) == u.selected && j.source.is_manual()).map(|j| j.id);
                            if let Some(id) = target {
                                u.reply_to = Some(id);
                                u.new_task = false;
                            } else {
                                hub.log("queue", "select a typed task with Alt-Up/Down to reply; Enter starts a new task");
                            }
                        }
                        // Send what is typed to the task now, stopping its agent to take it.
                        KeyCode::Char('s') if ctrl => {
                            let text = u.input.trim().to_string();
                            let target = ui::reply_target(&hub.state.lock().unwrap_or_else(|e| e.into_inner()), &u);
                            if let (Some(id), false) = (target, text.is_empty()) {
                                match rt.block_on(hub.steer_now(id, text.clone())) {
                                    Ok(()) => { history.submitted(text); u.input.clear(); }
                                    Err(why) => hub.log("queue", format!("#{} not sent: {}", id, why)),
                                }
                            }
                        }
                        KeyCode::Char('u') if ctrl => { u.input.clear(); history.reset(); }
                        KeyCode::Char(c) if !ctrl => u.input.push(c),
                        KeyCode::Backspace => {
                            u.input.pop();
                        }
                        KeyCode::Esc => {
                            u.input.clear();
                            history.reset();
                            u.reply_to = None;
                            u.new_task = false;
                            u.plan_next = false;
                            u.follow = true;
                        }
                        // The session log with Shift, else the selected job's output panes.
                        KeyCode::PageUp | KeyCode::PageDown if k.modifiers.contains(KeyModifiers::SHIFT) => {
                            u.log_back.set(ui::page(u.log_back.get(), u.log_view.get(), k.code == KeyCode::PageUp));
                        }
                        KeyCode::PageUp | KeyCode::PageDown => {
                            if let Some(id) = u.selected {
                                u.output_back.set(Some((id, ui::page(u.back_for(id), u.output_view.get(), k.code == KeyCode::PageUp))));
                            }
                        }
                        KeyCode::End => {
                            u.output_back.set(None);
                            u.log_back.set(0);
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
                        KeyCode::Up | KeyCode::Down if k.modifiers.contains(KeyModifiers::ALT) => {
                            let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
                            let ids: Vec<u32> = st.jobs.iter().map(|j| j.id).collect();
                            if !ids.is_empty() {
                                if u.input.is_empty() { u.reply_to = None; u.new_task = false; }
                                else if u.reply_to.is_none() { u.reply_to = ui::reply_target(&st, &u); }
                                let pos = u.selected.and_then(|s| ids.iter().position(|i| *i == s)).unwrap_or(ids.len() - 1);
                                let next = if k.code == KeyCode::Up { pos.saturating_sub(1) } else { (pos + 1).min(ids.len() - 1) };
                                u.selected = Some(ids[next]);
                                u.follow = false;
                            }
                        }
                        KeyCode::Left | KeyCode::Right if k.modifiers.contains(KeyModifiers::ALT) => {
                            let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
                            if let Some(next) = ui::lane_step(&st, u.selected, k.code == KeyCode::Right) {
                                if u.input.is_empty() { u.reply_to = None; u.new_task = false; }
                                else if u.reply_to.is_none() { u.reply_to = ui::reply_target(&st, &u); }
                                u.selected = Some(next);
                                u.follow = false;
                            }
                        }
                        KeyCode::Up => history.older(&mut u.input),
                        KeyCode::Down => history.newer(&mut u.input),
                        KeyCode::Enter => {
                            submit_input(hub, &mut u, &mut history);
                            save_composer(hub, &u, &history, &mut composer_saved);
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    })();
    let _ = crossterm::execute!(std::io::stdout(), event::DisableBracketedPaste, event::DisableMouseCapture);
    let _ = ratatui::try_restore();
    save_composer(hub, &u, &history, &mut composer_saved);
    // Stop any agent still running, and give claimed issues time to be handed back with a comment.
    // A signal from here on (a second Ctrl-C) exits at once.
    stopping.store(true, std::sync::atomic::Ordering::SeqCst);
    let busy = hub.state.lock().unwrap_or_else(|e| e.into_inner()).jobs.iter().filter(|j| !j.status.finished()).count();
    if busy > 0 {
        warn!("stopping {} job(s) and handing claimed issues back… (Ctrl-C again to quit at once)", busy);
    }
    let left = rt.block_on(hub.shutdown(Duration::from_secs(45)));
    if left > 0 {
        warn!("{} job(s) did not finish in time; check their issues for a stale working label", left);
    }
    record_owed(hub);
    proc::kill_all();
    res
}

#[cfg(test)]
mod quit_tests {
    use super::*;

    #[test]
    fn quit_accepts_terminal_key_variants_but_not_plain_text() {
        for c in ['c', 'C', 'q', 'Q'] {
            assert!(quit_key(event::KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)));
            assert!(!quit_key(event::KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)));
        }
        assert!(quit_key(event::KeyEvent::new(KeyCode::Char('\u{3}'), KeyModifiers::NONE)));
        let mut release = event::KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        release.kind = KeyEventKind::Release;
        assert!(!quit_key(release));
    }

    #[test]
    fn ctrl_shortcuts_work_with_shift_or_caps_lock() {
        let shifted = Event::Key(event::KeyEvent::new(KeyCode::Char('T'), KeyModifiers::CONTROL | KeyModifiers::SHIFT));
        let Event::Key(k) = ctrl_lowercase(shifted) else { panic!() };
        assert_eq!(k.code, KeyCode::Char('t'));
        // Typed text keeps its case.
        let typed = Event::Key(event::KeyEvent::new(KeyCode::Char('T'), KeyModifiers::SHIFT));
        let Event::Key(k) = ctrl_lowercase(typed) else { panic!() };
        assert_eq!(k.code, KeyCode::Char('T'));
    }
}
