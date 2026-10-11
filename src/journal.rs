//! A record of every finished job, kept on this machine: one JSON line per job in
//! `~/.local/state/forkdeck/journal.jsonl`. It is what `forkdeck stats` reads, and what the
//! limits count from, so both survive a restart.
//!
//! The file never leaves the machine. It holds what a job was and how it went (item, agents,
//! durations, rounds, tokens, the one-line outcome), not prompts and not agent output.

use crate::state::{clip, AgentKind, Job, JobStatus, Source};
use serde::{Deserialize, Serialize};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;

/// What one CLI was used for within a job.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Usage {
    pub runs: u32,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub cost: f64,
}

impl Usage {
    pub fn tokens(&self) -> u64 {
        self.tokens_in + self.tokens_out
    }
    pub fn add(&mut self, o: &Usage) {
        self.runs += o.runs;
        self.tokens_in += o.tokens_in;
        self.tokens_out += o.tokens_out;
        self.cost += o.cost;
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Entry {
    /// When the job ended, in seconds since the epoch.
    pub ts: i64,
    /// task, issue, review or deploy
    pub kind: String,
    pub repo: String,
    /// `#7`, `!5`, a short commit, or empty for a typed task.
    pub item: String,
    pub title: String,
    pub agent: String,
    pub tier: String,
    pub reviewer: String,
    /// done, failed or cancelled
    pub status: String,
    pub secs: u64,
    /// Times the work was sent back to the worker.
    pub rounds: u32,
    pub verdict: Option<bool>,
    pub checks: Option<bool>,
    pub claude: Usage,
    pub codex: Usage,
    pub pr: String,
    pub rolled_back: bool,
    /// Rules added to the repository's notes from this job.
    pub learned: u32,
    pub outcome: String,
}

impl Entry {
    pub fn tokens(&self) -> u64 {
        self.claude.tokens() + self.codex.tokens()
    }
    /// Work forkdeck picked up by itself, as opposed to a typed task or a deployment.
    pub fn automated(&self) -> bool {
        self.kind == "issue" || self.kind == "review"
    }
}

pub fn path() -> PathBuf {
    crate::cicd::state_dir().join("journal.jsonl")
}

pub fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// What goes on record for a job that has just ended.
pub fn entry(j: &Job, status: JobStatus, outcome: &str, learned: u32) -> Entry {
    let (kind, repo, item) = match &j.source {
        Source::Manual => ("task", String::new(), String::new()),
        Source::Issue { repo, number, .. } => ("issue", repo.clone(), format!("#{}", number)),
        Source::Pr { repo, number, .. } => ("review", repo.clone(), format!("!{}", number)),
        Source::Deploy { repo, sha, .. } => ("deploy", repo.clone(), sha.chars().take(10).collect()),
    };
    Entry {
        ts: now(),
        kind: kind.into(),
        repo,
        item,
        // A typed task's title is the task as typed; that stays out of the record.
        title: if kind == "task" { String::new() } else { clip(&j.title, 120) },
        agent: j.worker.agent.map(|a| a.name().to_string()).unwrap_or_default(),
        tier: j.decision.as_ref().map(|d| d.tier.name().to_string()).unwrap_or_default(),
        reviewer: j.reviewer.agent.map(|a| a.name().to_string()).unwrap_or_default(),
        status: status.name().into(),
        secs: j.created.elapsed().as_secs(),
        rounds: j.rounds,
        verdict: j.verdict,
        checks: j.checks,
        claude: j.used[0],
        codex: j.used[1],
        pr: j.pr_url.clone().unwrap_or_default(),
        rolled_back: kind == "deploy" && outcome.contains("Rolled back to"),
        learned,
        outcome: if kind == "task" { status.name().to_string() } else { clip(&outcome.replace('\n', " "), 300) },
    }
}

pub fn slot(a: AgentKind) -> usize {
    (a == AgentKind::Codex) as usize
}

/// Add one line. Appends are single writes, so several forkdecks can share the file.
pub fn append(e: &Entry) -> Result<(), String> {
    let p = path();
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {}", d.display(), e))?;
    }
    let mut line = serde_json::to_string(e).map_err(|e| e.to_string())?;
    line.push('\n');
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&p).map_err(|e| format!("{}: {}", p.display(), e))?;
    f.write_all(line.as_bytes()).map_err(|e| format!("{}: {}", p.display(), e))
}

/// Entries that ended at or after `since`, oldest first. A line that cannot be read is skipped:
/// one damaged line must not hide the rest. `tail_bytes` bounds how much of a large file is read.
pub fn read(since: i64, tail_bytes: Option<u64>) -> Vec<Entry> {
    let Ok(mut f) = std::fs::File::open(path()) else { return Vec::new() };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let mut skip_first = false;
    if let Some(t) = tail_bytes {
        if len > t {
            skip_first = f.seek(SeekFrom::Start(len - t)).is_ok();
        }
    }
    let mut text = String::new();
    let mut raw = Vec::new();
    if f.read_to_end(&mut raw).is_ok() {
        text = String::from_utf8_lossy(&raw).to_string();
    }
    parse(&text, since, skip_first)
}

pub fn parse(text: &str, since: i64, skip_first: bool) -> Vec<Entry> {
    // After a seek into the middle of the file the first line is a fragment.
    text.lines().skip(skip_first as usize).filter_map(|l| serde_json::from_str::<Entry>(l).ok()).filter(|e| e.ts >= since).collect()
}

// ---------------------------------------------------------------------------------------------
// Limits

/// How much has been used in the last 24 hours.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Spent {
    pub jobs: u32,
    pub tokens: u64,
}

pub const DAY: i64 = 24 * 3600;

/// Jobs forkdeck picked up by itself and tokens of every kind, since a day ago.
pub fn spent(entries: &[Entry], repo: Option<&str>, now: i64) -> Spent {
    let mut s = Spent::default();
    for e in entries.iter().filter(|e| e.ts > now - DAY) {
        if repo.is_none_or(|r| e.repo == r) && e.automated() {
            s.jobs += 1;
        }
        if repo.is_none() {
            s.tokens += e.tokens();
        }
    }
    s
}

/// If the last `after` automated jobs on this repository all failed, and the latest of them less
/// than `pause_secs` ago: when the pause ends. A cancelled job says nothing either way.
pub fn paused_until(entries: &[Entry], repo: &str, after: u32, pause_secs: i64, now: i64) -> Option<i64> {
    if after == 0 {
        return None;
    }
    let recent: Vec<&Entry> = entries.iter().rev().filter(|e| e.repo == repo && e.automated() && e.status != "cancelled").take(after as usize).collect();
    if recent.len() < after as usize || recent.iter().any(|e| e.status != "failed") {
        return None;
    }
    let until = recent[0].ts + pause_secs;
    (until > now).then_some(until)
}

// ---------------------------------------------------------------------------------------------
// Statistics

#[derive(Serialize, Debug, Default, PartialEq)]
pub struct Group {
    pub jobs: u32,
    pub done: u32,
    pub failed: u32,
    pub cancelled: u32,
}

#[derive(Serialize, Debug, Default, PartialEq)]
pub struct Summary {
    pub days: u32,
    pub issues: Group,
    /// Issues that ended done without the work being sent back once.
    pub issues_first_pass: u32,
    pub issue_rounds: u32,
    pub issue_median_secs: u64,
    pub issue_p90_secs: u64,
    pub reviews: Group,
    pub reviews_passed: u32,
    pub reviews_changes: u32,
    pub deploys: Group,
    pub rollbacks: u32,
    pub tasks: Group,
    pub claude: Usage,
    pub codex: Usage,
    pub learned: u32,
    pub repos: Vec<(String, Group)>,
}

fn count(g: &mut Group, status: &str) {
    g.jobs += 1;
    match status {
        "done" => g.done += 1,
        "failed" => g.failed += 1,
        _ => g.cancelled += 1,
    }
}

fn percentile(sorted: &[u64], p: usize) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    sorted[((sorted.len() - 1) * p).div_ceil(100).min(sorted.len() - 1)]
}

pub fn summarise(entries: &[Entry], days: u32) -> Summary {
    let mut s = Summary { days, ..Summary::default() };
    let mut times: Vec<u64> = Vec::new();
    for e in entries {
        match e.kind.as_str() {
            "issue" => {
                count(&mut s.issues, &e.status);
                s.issue_rounds += e.rounds;
                if e.status == "done" {
                    times.push(e.secs);
                    if e.rounds == 0 {
                        s.issues_first_pass += 1;
                    }
                }
            }
            "review" => {
                count(&mut s.reviews, &e.status);
                match (e.status.as_str(), e.verdict) {
                    ("done", Some(true)) => s.reviews_passed += 1,
                    ("done", Some(false)) => s.reviews_changes += 1,
                    _ => {}
                }
            }
            "deploy" => {
                count(&mut s.deploys, &e.status);
                s.rollbacks += e.rolled_back as u32;
            }
            _ => count(&mut s.tasks, &e.status),
        }
        s.claude.add(&e.claude);
        s.codex.add(&e.codex);
        s.learned += e.learned;
        if !e.repo.is_empty() {
            match s.repos.iter_mut().find(|(r, _)| r == &e.repo) {
                Some((_, g)) => count(g, &e.status),
                None => {
                    let mut g = Group::default();
                    count(&mut g, &e.status);
                    s.repos.push((e.repo.clone(), g));
                }
            }
        }
    }
    times.sort_unstable();
    s.issue_median_secs = percentile(&times, 50);
    s.issue_p90_secs = percentile(&times, 90);
    s.repos.sort_by(|a, b| a.0.cmp(&b.0));
    s
}

fn pct(part: u32, whole: u32) -> String {
    if whole == 0 {
        "-".into()
    } else {
        format!("{}%", (part as f64 * 100.0 / whole as f64).round())
    }
}

fn dur(secs: u64) -> String {
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        format!("{}m {:02}s", secs / 60, secs % 60)
    } else {
        format!("{}h {:02}m", secs / 3600, (secs % 3600) / 60)
    }
}

pub fn kilo(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1e6)
    } else if n >= 1000 {
        format!("{:.1}k", n as f64 / 1e3)
    } else {
        n.to_string()
    }
}

pub fn text(s: &Summary) -> Vec<String> {
    let total = s.issues.jobs + s.reviews.jobs + s.deploys.jobs + s.tasks.jobs;
    let mut out = vec![format!("last {} day(s): {} job(s)", s.days, total)];
    if total == 0 {
        out.push(format!("nothing on record in {}", path().display()));
        return out;
    }
    let line = |name: &str, g: &Group| format!("{:<9} {:>4}   done {:>4}   failed {:>4}   cancelled {:>3}", name, g.jobs, g.done, g.failed, g.cancelled);
    out.push(String::new());
    if s.issues.jobs > 0 {
        out.push(line("issues", &s.issues));
        out.push(format!(
            "          {} ended with a pull request · {} of those without being sent back · {} fix round(s) in all",
            pct(s.issues.done, s.issues.jobs - s.issues.cancelled),
            pct(s.issues_first_pass, s.issues.done),
            s.issue_rounds
        ));
        if s.issues.done > 0 {
            out.push(format!("          time to a pull request: median {}, 90% within {}", dur(s.issue_median_secs), dur(s.issue_p90_secs)));
        }
    }
    if s.reviews.jobs > 0 {
        out.push(line("reviews", &s.reviews));
        out.push(format!("          {} passed · {} asked for changes", s.reviews_passed, s.reviews_changes));
    }
    if s.deploys.jobs > 0 {
        out.push(line("deploys", &s.deploys));
        out.push(format!("          {} rolled back", s.rollbacks));
    }
    if s.tasks.jobs > 0 {
        out.push(line("tasks", &s.tasks));
    }
    out.push(String::new());
    for (name, u) in [("claude", &s.claude), ("codex", &s.codex)] {
        if u.runs > 0 {
            let cost = if u.cost > 0.0 { format!(" · ${:.2} as reported by the CLI", u.cost) } else { String::new() };
            out.push(format!("{:<9} {:>4} run(s) · {} tokens in · {} out{}", name, u.runs, kilo(u.tokens_in), kilo(u.tokens_out), cost));
        }
    }
    if s.learned > 0 {
        out.push(format!("notes     {} rule(s) learned", s.learned));
    }
    if s.repos.len() > 1 {
        out.push(String::new());
        for (r, g) in &s.repos {
            out.push(line(&clip(r, 28), g).replacen(&format!("{:<9}", clip(r, 28)), &format!("{:<28}", clip(r, 28)), 1));
        }
    }
    out
}

/// The latest jobs, newest first, one line each.
pub fn recent(entries: &[Entry], n: usize) -> Vec<String> {
    entries
        .iter()
        .rev()
        .take(n)
        .map(|e| {
            let when = chrono::DateTime::from_timestamp(e.ts, 0).map(|d| d.with_timezone(&chrono::Local).format("%m-%d %H:%M").to_string()).unwrap_or_default();
            let what = if e.repo.is_empty() { "task".to_string() } else { format!("{}{}{}", e.repo, if e.kind == "deploy" { "@" } else { "" }, e.item) };
            format!("{}  {:<9} {:<24} {:<7} {:>7}  {}", when, e.status, clip(&what, 24), clip(&e.agent, 7), dur(e.secs), clip(&e.outcome, 90))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(ts: i64, kind: &str, repo: &str, status: &str) -> Entry {
        Entry { ts, kind: kind.into(), repo: repo.into(), status: status.into(), ..Entry::default() }
    }

    #[test]
    fn a_damaged_line_does_not_hide_the_rest() {
        let a = serde_json::to_string(&e(10, "issue", "o/r", "done")).unwrap();
        let b = serde_json::to_string(&e(20, "review", "o/r", "failed")).unwrap();
        let text = format!("{}\n{{\"ts\": 15, \"kind\"\n\n{}\n", a, b);
        assert_eq!(parse(&text, 0, false).len(), 2);
        assert_eq!(parse(&text, 15, false).len(), 1);
        // A fragment left by seeking into the file, and fields from a newer version, are both tolerated.
        let cut = format!("\"status\":\"done\"}}\n{}\n", b.replace("\"ts\":20", "\"ts\":20,\"added_later\":1"));
        assert_eq!(parse(&cut, 0, true).len(), 1);
    }

    #[test]
    fn spending_counts_a_rolling_day() {
        let now = 1_000_000;
        let mut all = vec![e(now - DAY - 5, "issue", "o/r", "done"), e(now - 100, "issue", "o/r", "failed"), e(now - 50, "review", "x/y", "done"), e(now - 10, "task", "", "done"), e(now - 5, "deploy", "o/r", "done")];
        all[3].claude = Usage { runs: 1, tokens_in: 900, tokens_out: 100, cost: 0.0 };
        all[1].codex = Usage { runs: 2, tokens_in: 400, tokens_out: 100, cost: 0.0 };
        all[0].claude = Usage { runs: 1, tokens_in: 5_000_000, tokens_out: 0, cost: 0.0 };
        // Typed tasks and deployments are not jobs forkdeck took by itself, but their tokens were spent all the same.
        assert_eq!(spent(&all, None, now), Spent { jobs: 2, tokens: 1500 });
        assert_eq!(spent(&all, Some("o/r"), now).jobs, 1);
    }

    #[test]
    fn a_run_of_failures_pauses_a_repository_for_a_while() {
        let now = 10_000;
        let run = vec![e(100, "issue", "o/r", "done"), e(200, "issue", "o/r", "failed"), e(300, "review", "o/r", "failed"), e(350, "issue", "o/r", "cancelled"), e(400, "issue", "o/r", "failed"), e(450, "issue", "x/y", "done"), e(460, "deploy", "o/r", "done")];
        assert_eq!(paused_until(&run, "o/r", 3, 20_000, now), Some(20_400));
        assert_eq!(paused_until(&run, "o/r", 3, 600, now), None, "the pause is over");
        assert_eq!(paused_until(&run, "o/r", 4, 20_000, now), None, "the fourth one back succeeded");
        assert_eq!(paused_until(&run, "x/y", 1, 20_000, now), None);
        assert_eq!(paused_until(&run, "o/r", 0, 20_000, now), None, "switched off");
        assert_eq!(paused_until(&run[..2], "o/r", 3, 20_000, now), None, "not enough history");
    }

    #[test]
    fn the_summary_adds_up() {
        let mut all = vec![e(1, "issue", "o/r", "done"), e(2, "issue", "o/r", "done"), e(3, "issue", "o/r", "failed"), e(4, "review", "o/r", "done"), e(5, "review", "x/y", "done"), e(6, "deploy", "o/r", "failed"), e(7, "task", "", "cancelled")];
        all[0].secs = 100;
        all[1].secs = 300;
        all[1].rounds = 2;
        all[3].verdict = Some(true);
        all[4].verdict = Some(false);
        all[5].rolled_back = true;
        all[0].claude = Usage { runs: 1, tokens_in: 1000, tokens_out: 500, cost: 0.25 };
        let s = summarise(&all, 7);
        assert_eq!((s.issues.jobs, s.issues.done, s.issues.failed, s.issues_first_pass, s.issue_rounds), (3, 2, 1, 1, 2));
        assert_eq!((s.issue_median_secs, s.issue_p90_secs), (300, 300));
        assert_eq!((s.reviews_passed, s.reviews_changes, s.rollbacks, s.tasks.cancelled), (1, 1, 1, 1));
        assert_eq!(s.repos.len(), 2);
        let t = text(&s).join("\n");
        assert!(t.contains("67% ended with a pull request · 50% of those without being sent back · 2 fix round(s)"), "{}", t);
        assert!(t.contains("claude       1 run(s) · 1.0k tokens in · 500 out · $0.25"), "{}", t);
        assert!(text(&summarise(&[], 7))[1].contains("nothing on record"));
        assert_eq!(percentile(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10], 90), 10);
        assert_eq!(percentile(&[5], 50), 5);
    }
}
