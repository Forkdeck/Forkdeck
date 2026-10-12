//! The agent-tree screen.

use crate::state::*;
use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::border;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Padding, Paragraph, Wrap};
use ratatui::Frame;
use std::path::PathBuf;

const BG: Color = Color::Rgb(13, 17, 23);
const DIM: Color = Color::Rgb(92, 101, 116);
const TEXT: Color = Color::Rgb(201, 209, 217);
const BRIGHT: Color = Color::Rgb(240, 246, 252);
const ORANGE: Color = Color::Rgb(240, 136, 62);
const GREEN: Color = Color::Rgb(63, 220, 150);
const PURPLE: Color = Color::Rgb(150, 142, 240);
const RED: Color = Color::Rgb(248, 81, 73);
const BLUE: Color = Color::Rgb(110, 160, 255);
const SEL: Color = Color::Rgb(38, 36, 72);

const DASHED: border::Set = border::Set {
    top_left: "┌",
    top_right: "┐",
    bottom_left: "└",
    bottom_right: "┘",
    vertical_left: "╎",
    vertical_right: "╎",
    horizontal_top: "╌",
    horizontal_bottom: "╌",
};

pub struct Ui {
    pub input: String,
    pub selected: Option<u32>,
    pub follow: bool,
    pub forced: Option<AgentRef>,
    pub cwd: PathBuf,
    pub demo: bool,
    /// Show what teammates are working on in place of the agent tree.
    pub team: bool,
    pub factory: bool,
    /// Explicit reply destination, pinned even when selection changes (Ctrl-R).
    pub reply_to: Option<u32>,
    pub new_task: bool,
    pub choices: Option<crate::choices::Picker>,
    pub issue_panel: Option<crate::plan_ui::Panel>,
    pub seen_choices: std::collections::HashSet<(u32, usize)>,
    /// The daily limits, to show next to what has been used. 0: none.
    pub jobs_per_day: u32,
    pub tokens_per_day: u64,
}

pub fn reply_target(st: &State, ui: &Ui) -> Option<u32> {
    if ui.new_task { return None; }
    ui.reply_to.or_else(|| st.jobs.iter().find(|j| Some(j.id) == ui.selected && j.source.is_manual()).map(|j| j.id))
}

fn fg(c: Color) -> Style {
    Style::new().fg(c)
}
fn bold(c: Color) -> Style {
    Style::new().fg(c).add_modifier(Modifier::BOLD)
}
fn agent_color(a: AgentKind) -> Color {
    match a {
        AgentKind::Claude => ORANGE,
        AgentKind::Codex => GREEN,
        AgentKind::Copilot => BLUE,
    }
}
fn status_color(s: JobStatus) -> Color {
    match s {
        JobStatus::Queued => DIM,
        JobStatus::Done => GREEN,
        JobStatus::Failed => RED,
        JobStatus::Cancelled => DIM,
        JobStatus::Routing => PURPLE,
        _ => ORANGE,
    }
}
fn spinner() -> &'static str {
    const F: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    F[((ms / 100) % 10) as usize]
}
fn boxed(title: &str, color: Color) -> Block<'static> {
    let b = Block::bordered().border_set(DASHED).border_style(fg(color));
    if title.is_empty() {
        b
    } else {
        b.title(Span::styled(format!(" {} ", title), bold(color)))
    }
}
fn centered(area: Rect, w: u16) -> Rect {
    let w = w.min(area.width);
    Rect { x: area.x + (area.width - w) / 2, y: area.y, width: w, height: area.height }
}
fn kilo(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1e6)
    } else if n >= 1000 {
        format!("{:.1}k", n as f64 / 1e3)
    } else {
        n.to_string()
    }
}
fn mmss(secs: u64) -> String {
    format!("{}:{:02}", secs / 60, secs % 60)
}

pub fn draw(f: &mut Frame, st: &State, ui: &Ui) {
    let area = f.area();
    f.render_widget(Block::new().style(Style::new().bg(BG).fg(TEXT)), area);
    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(2),
        Constraint::Min(8),
        Constraint::Length(5),
        Constraint::Length(3),
        Constraint::Length(1),
    ])
    .split(area);

    title_bar(f, rows[0], ui);
    header(f, rows[1], st);
    if ui.factory && !ui.team {
        factory(f, rows[2], st, ui);
    } else {
    let cols = Layout::horizontal([Constraint::Length(34), Constraint::Min(40)]).split(rows[2]);
    left_panel(f, cols[0], st, ui);
    let job = ui.selected.and_then(|id| st.jobs.iter().find(|j| j.id == id));
    match job {
        _ if ui.team => team_panel(f, cols[1], st),
        Some(j) => tree(f, cols[1], j),
        None => empty_tree(f, cols[1]),
    }
    }
    log_panel(f, rows[3], st);
    prompt(f, rows[4], ui, reply_target(st, ui).and_then(|id| st.jobs.iter().find(|j| j.id == id)));
    status(f, rows[5], st);
    if let Some(picker) = &ui.choices { choice_panel(f, area, picker); }
    if let Some(panel) = &ui.issue_panel { crate::plan_ui::draw(f, st, panel); }
}

fn choice_panel(f: &mut Frame, area: Rect, picker: &crate::choices::Picker) {
    let width = area.width.saturating_sub(2).min(100);
    let height = area.height.saturating_sub(2).min(picker.options.len() as u16 + 13);
    let area = Rect::new(area.x + (area.width - width) / 2, area.y + (area.height - height) / 2, width, height);
    let block = boxed(&format!("Planning options · task #{}", picker.job), PURPLE);
    let inner = block.inner(area);
    f.render_widget(Clear, area);
    f.render_widget(block.style(Style::new().bg(BG)), area);
    let rows = Layout::vertical([Constraint::Length(2), Constraint::Min(2), Constraint::Length(6), Constraint::Length(1)]).split(inner);
    f.render_widget(Paragraph::new(" Choose an option or write your own response."), rows[0]);
    let room = rows[1].height.max(1) as usize;
    let start = picker.selected.saturating_sub(room - 1);
    let labels = picker.options.iter().map(|o| o.label.as_str()).chain(std::iter::once("Other / custom response"));
    let lines: Vec<Line> = labels.enumerate().skip(start).take(room).map(|(index, label)| {
        let selected = picker.selected == index;
        Line::from(Span::styled(format!(" {} {}", if selected { "▸" } else { " " }, clip(label, width.saturating_sub(6) as usize)),
            if selected { bold(BRIGHT).bg(SEL) } else { fg(TEXT) }))
    }).collect();
    f.render_widget(Paragraph::new(lines), rows[1]);
    let detail = match picker.options.get(picker.selected) {
        Some(option) => format!("{}\n{}", option.label, option.detail),
        None => "Write your own answer in the message field, then press Enter. It will reply to this same task.".into(),
    };
    f.render_widget(Paragraph::new(detail).wrap(Wrap { trim: false }).block(Block::new().padding(Padding::horizontal(1))), rows[2]);
    f.render_widget(Paragraph::new(" ↑↓ choose · Enter send · c custom · Esc close").style(fg(DIM)), rows[3]);
}

fn lane(status: JobStatus) -> usize {
    match status {
        JobStatus::Queued | JobStatus::Routing | JobStatus::Planning => 0,
        JobStatus::Working | JobStatus::Fixing => 1,
        JobStatus::Reviewing => 2,
        JobStatus::Publishing | JobStatus::Ci | JobStatus::Deploying => 3,
        JobStatus::Done => 4,
        JobStatus::Failed | JobStatus::Cancelled | JobStatus::RollingBack => 5,
    }
}

fn factory(f: &mut Frame, area: Rect, st: &State, ui: &Ui) {
    let narrow = area.width < 120;
    let rows = Layout::vertical([Constraint::Length(if narrow { 14 } else { 9 }), Constraint::Min(4)]).split(area);
    let board_rows = Layout::vertical(vec![Constraint::Ratio(1, if narrow { 2 } else { 1 }); if narrow { 2 } else { 1 }]).split(rows[0]);
    let names = ["QUEUE", "BUILD", "REVIEW", "DELIVERY", "DONE", "ATTENTION"];
    let colors = [PURPLE, ORANGE, PURPLE, ORANGE, GREEN, RED];
    for (index, name) in names.iter().enumerate() {
        let count = if narrow { 3 } else { 6 };
        let columns = Layout::horizontal(vec![Constraint::Ratio(1, count as u32); count]).split(board_rows[index / count]);
        let cell = columns[index % count];
        let jobs: Vec<_> = st.jobs.iter().filter(|j| {
            let status = if j.status == JobStatus::Done && j.issue_plan.as_ref().is_some_and(|p| p.build_job.is_none()) { JobStatus::Planning } else { j.status };
            lane(status) == index
        }).collect();
        // Keep the selected card visible, even when browsing older completed jobs.
        let room = cell.height.saturating_sub(3) as usize / 2;
        let room = room.max(1);
        let start = jobs.iter().position(|j| Some(j.id) == ui.selected)
            .map(|pos| pos.saturating_sub(room - 1)).unwrap_or(jobs.len().saturating_sub(room));
        let mut lines = Vec::new();
        for job in jobs.iter().skip(start).take(room) {
            let selected = ui.selected == Some(job.id);
            let style = if selected { bold(BRIGHT).bg(SEL) } else { fg(TEXT) };
            lines.push(Line::from(Span::styled(clip(&format!("{}#{} {}", if selected { "◆" } else { " " }, job.id, job.title), cell.width.saturating_sub(2) as usize), style)));
            let agent = if job.status == JobStatus::Reviewing { job.reviewer.agent } else { job.worker.agent };
            lines.push(Line::from(Span::styled(clip(&format!(" {} · {}", job.status.name(), agent.map(|a| a.name()).unwrap_or("unassigned")), cell.width.saturating_sub(2) as usize), fg(colors[index]))));
        }
        if jobs.is_empty() { lines.push(Line::from(Span::styled(" —", fg(DIM)))); }
        if jobs.len() > room { lines.push(Line::from(Span::styled(format!(" {} more · Alt↑↓", jobs.len() - room), fg(DIM)))); }
        f.render_widget(Paragraph::new(lines).block(boxed(&format!("{} {}", name, jobs.len()), colors[index])), cell);
    }
    match ui.selected.and_then(|id| st.jobs.iter().find(|j| j.id == id)) {
        Some(job) => {
            let detail = Layout::vertical([Constraint::Length(4), Constraint::Min(0)]).split(rows[1]);
            let checks = match job.checks { Some(true) => "passed", Some(false) => "failed", None => "pending / not configured" };
            let review = match job.verdict { Some(true) => "passed", Some(false) => "changes requested", None => "pending / not requested" };
            let queued = job.queued_prompts.front().map(|next| format!("{} queued · next: {}", job.queued_prompts.len(), clip(next, 100)));
            let outcome = if let Some(ref queued) = queued { queued.as_str() } else if job.outcome.is_empty() { job.pr_url.as_deref().unwrap_or("Work is in progress") } else { &job.outcome };
            let summary = job.issue_plan.as_ref().map(|p| p.status(st.automation)).unwrap_or_else(|| format!("Checks: {} · Review: {} · {}", checks, review, job.workdir.display()));
            let lines = vec![
                Line::from(Span::styled(format!(" {summary}"), fg(TEXT))),
                Line::from(Span::styled(format!(" {}", outcome), fg(status_color(job.status)))),
            ];
            f.render_widget(Paragraph::new(lines).block(boxed(&format!("#{} {} · {}", job.id, job.title, job.status.name()), status_color(job.status))), detail[0]);
            let outputs = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).split(detail[1]);
            for (rect, role, stage) in [(outputs[0], "worker", &job.worker), (outputs[1], "reviewer", &job.reviewer)] {
                let agent = stage.agent.map(|a| a.name()).unwrap_or("waiting");
                output_panel(f, rect, &format!("{} · {} · {} tools", role, agent, stage.tools), stage, stage.agent.map(agent_color).unwrap_or(DIM));
            }
        }
        None => {
            let lines = vec![
                Line::from(Span::styled("Ready for work", bold(BRIGHT))),
                Line::from("Type a task and press Enter. Jobs move through the board as they run."),
                Line::from(format!("{} repositories configured · issue intake {} (Ctrl-A)", st.repos.len(), if st.automation { "on" } else { "paused" })),
                Line::from("Alt↑↓ selects a job · F2 opens the agent tree · F4 shows the team"),
            ];
            f.render_widget(Paragraph::new(lines).alignment(Alignment::Center).block(boxed("dispatch", PURPLE)), rows[1]);
        }
    }
}

fn title_bar(f: &mut Frame, area: Rect, ui: &Ui) {
    let dots = Line::from(vec![
        Span::styled(" ●", fg(RED)),
        Span::styled(" ●", fg(Color::Rgb(230, 190, 60))),
        Span::styled(" ●", fg(GREEN)),
    ]);
    f.render_widget(Paragraph::new(dots), area);
    let dir = ui.cwd.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let t = format!("~/{} — forkdeck{} — {}x{}", dir, if ui.demo { " (demo)" } else { "" }, area.width, f.area().height);
    f.render_widget(Paragraph::new(Span::styled(t, fg(DIM))).alignment(Alignment::Center), area);
}

fn header(f: &mut Frame, area: Rect, st: &State) {
    let chip = |k: AgentKind, own: Color| -> Span {
        let (word, color) = if st.has(k) && !st.agent_ready(k) {
            ("LIMITED", DIM)
        } else if st.has(k) && (k != AgentKind::Copilot || st.copilot_usable()) {
            ("ON CALL", own)
        } else if k == AgentKind::Copilot && st.has(k) {
            ("NO ACCESS", RED)
        } else if k != AgentKind::Copilot && st.stand_in && st.copilot_usable() {
            ("VIA COPILOT", BLUE)
        } else {
            ("MISSING", DIM)
        };
        Span::styled(format!("{} {}", k.name().to_uppercase(), word), bold(color))
    };
    let jev = match (st.router.kind.as_str(), st.router.healthy) {
        ("rules", _) => "RULES ROUTER".to_string(),
        (_, Some(false)) => "JEV DOWN · RULES".to_string(),
        (k, _) => k.to_uppercase(),
    };
    let top = Line::from(vec![
        Span::styled("  FORKDECK SOFTWARE FACTORY", bold(BRIGHT)),
        Span::styled("   ·   ", fg(DIM)),
        chip(AgentKind::Claude, ORANGE),
        Span::styled("   ·   ", fg(DIM)),
        chip(AgentKind::Codex, GREEN),
        Span::styled("   ·   ", fg(DIM)),
        chip(AgentKind::Copilot, BLUE),
        Span::styled("   ·   ", fg(DIM)),
        Span::styled(jev, bold(PURPLE)),
    ]);
    let legend = Line::from(vec![
        Span::styled(format!("  INTAKE {}", if st.automation { "ON" } else { "PAUSED" }), fg(if st.automation { GREEN } else { DIM })),
        Span::styled(format!(" · {} repos · {} active · {} queued · 24h: {} jobs / {} tokens",
            st.repos.len(),
            st.jobs.iter().filter(|j| !j.status.finished() && j.status != JobStatus::Queued).count(),
            st.jobs.iter().filter(|j| j.status == JobStatus::Queued).count(),
            st.spent.jobs, kilo(st.spent.tokens)), fg(DIM)),
    ]);
    f.render_widget(Paragraph::new(vec![top, legend]), area);
}

fn left_panel(f: &mut Frame, area: Rect, st: &State, ui: &Ui) {
    let w = area.width.saturating_sub(4) as usize;
    let mut lines: Vec<Line> = Vec::new();
    let head = |s: &str| Line::from(Span::styled(s.to_string(), fg(DIM)));

    lines.push(head("jobs"));
    if st.jobs.is_empty() {
        lines.push(Line::from(Span::styled("  none yet", fg(DIM))));
    }
    let room = (area.height as usize).saturating_sub(16).max(3);
    let skip = st.jobs.len().saturating_sub(room);
    for j in st.jobs.iter().skip(skip) {
        let sel = ui.selected == Some(j.id);
        let icon = if j.status.finished() {
            match j.status {
                JobStatus::Done => "✓",
                JobStatus::Failed => "✗",
                _ => "–",
            }
        } else if j.status == JobStatus::Queued {
            "·"
        } else {
            spinner()
        };
        let text = format!("#{} {}", j.id, j.title);
        let mut l = Line::from(vec![
            Span::styled(if sel { "◆ " } else { "  " }, fg(PURPLE)),
            Span::styled(format!("{} ", icon), fg(status_color(j.status))),
            Span::styled(clip(&text, w.saturating_sub(4)), if sel { bold(BRIGHT) } else { fg(TEXT) }),
        ]);
        if sel {
            l = l.style(Style::new().bg(SEL));
        }
        lines.push(l);
    }
    lines.push(Line::default());

    lines.push(head("fork layer"));
    let r = &st.router;
    lines.push(Line::from(vec![Span::styled("  ", fg(DIM)), Span::styled(r.kind.clone(), fg(PURPLE))]));
    if r.kind != "rules" {
        lines.push(Line::from(Span::styled(format!("  {}", clip(&r.endpoint, w.saturating_sub(2))), fg(DIM))));
        let health = match r.healthy {
            None => ("not asked yet", DIM),
            Some(true) => ("answering", GREEN),
            Some(false) => ("down, rules in use", RED),
        };
        lines.push(Line::from(Span::styled(format!("  {}", health.0), fg(health.1))));
        lines.push(Line::from(vec![
            Span::styled("  calls ", fg(DIM)),
            Span::styled(r.calls.to_string(), fg(TEXT)),
            Span::styled("  fallbacks ", fg(DIM)),
            Span::styled(r.fallbacks.to_string(), fg(TEXT)),
            Span::styled("  last ", fg(DIM)),
            Span::styled(format!("{}ms", r.last_ms), fg(TEXT)),
        ]));
    }
    lines.push(Line::default());

    lines.push(Line::from(vec![
        Span::styled("issues  ", fg(DIM)),
        Span::styled(if st.automation { "polling" } else { "paused" }, fg(if st.automation { GREEN } else { DIM })),
    ]));
    if st.repos.is_empty() {
        lines.push(Line::from(Span::styled("  no repos configured", fg(DIM))));
    }
    for rp in &st.repos {
        lines.push(Line::from(Span::styled(format!("  {}", clip(&rp.name, w.saturating_sub(2))), fg(TEXT))));
        lines.push(Line::from(Span::styled(format!("  label {}", clip(&rp.label, w.saturating_sub(8))), fg(DIM))));
        let age = match rp.last_poll {
            Some(t) => format!("{}s ago", t.elapsed().as_secs()),
            None => "never".into(),
        };
        lines.push(Line::from(Span::styled(
            format!("  {}", clip(&format!("polled {} · {} taken", age, rp.picked), w.saturating_sub(2))),
            fg(DIM),
        )));
        if let Some(e) = &rp.last_error {
            lines.push(Line::from(Span::styled(format!("  {}", clip(e, w.saturating_sub(2))), fg(RED))));
        }
        if let Some(h) = &rp.hold {
            lines.push(Line::from(Span::styled(format!("  {}", clip(h, w.saturating_sub(2))), fg(ORANGE))));
        }
    }
    if !st.repos.is_empty() && !ui.demo {
        let of = |limit: u64| if limit == 0 { String::new() } else { format!("/{}", kilo(limit)) };
        let used = format!("24 h: {}{} jobs · {}{} tok", st.spent.jobs, of(ui.jobs_per_day as u64), kilo(st.spent.tokens), of(ui.tokens_per_day));
        lines.push(Line::from(Span::styled(format!("  {}", clip(&used, w.saturating_sub(2))), fg(DIM))));
    }
    f.render_widget(Paragraph::new(lines).block(boxed("queue", PURPLE)), area);
}

fn team_panel(f: &mut Frame, area: Rect, st: &State) {
    let w = area.width.saturating_sub(4) as usize;
    let mut lines: Vec<Line> = Vec::new();
    let age = match st.team.at {
        Some(t) => format!("read from the forge {}s ago", t.elapsed().as_secs()),
        None => "reading the forge…".to_string(),
    };
    lines.push(Line::from(Span::styled(clip(&age, w), fg(DIM))));
    if let Some(e) = &st.team.error {
        lines.push(Line::from(Span::styled(clip(e, w), fg(RED))));
    }
    lines.push(Line::default());
    if st.team.at.is_some() {
        for l in crate::team::text(&st.team.rows, w) {
            let style = if !l.starts_with(' ') {
                bold(if l.ends_with("(you)") { GREEN } else { PURPLE })
            } else if l.trim_start().starts_with("no update for") {
                fg(RED)
            } else if l.trim_start().starts_with("http") {
                fg(DIM)
            } else {
                fg(TEXT)
            };
            lines.push(Line::from(Span::styled(clip(&l, w), style)));
        }
    }
    let room = area.height.saturating_sub(2) as usize;
    if lines.len() > room && room > 1 {
        let more = lines.len() - (room - 1);
        lines.truncate(room - 1);
        lines.push(Line::from(Span::styled(format!("… {} more line(s): forkdeck team", more), fg(DIM))));
    }
    f.render_widget(Paragraph::new(lines).block(boxed("team", GREEN).padding(Padding::horizontal(1))), area);
}

fn empty_tree(f: &mut Frame, area: Rect) {
    let lines = vec![
        Line::default(),
        Line::default(),
        Line::from(Span::styled("No job selected", bold(BRIGHT))),
        Line::default(),
        Line::from(Span::styled("Type a task below and press Enter.", fg(TEXT))),
        Line::from(Span::styled("The fork layer picks claude or codex and an effort tier,", fg(DIM))),
        Line::from(Span::styled("the other CLI reviews the result.", fg(DIM))),
        Line::default(),
        Line::from(Span::styled("Ctrl-A starts picking up labelled issues.", fg(DIM))),
    ];
    f.render_widget(Paragraph::new(lines).alignment(Alignment::Center), area);
}

fn connector(f: &mut Frame, area: Rect, color: Color) {
    f.render_widget(Paragraph::new(Span::styled("┊", fg(color))).alignment(Alignment::Center), area);
}

fn tree(f: &mut Frame, area: Rect, j: &Job) {
    let rows = Layout::vertical([
        Constraint::Length(5),
        Constraint::Length(1),
        Constraint::Length(6),
        Constraint::Length(1),
        Constraint::Length(7),
        Constraint::Length(1),
        Constraint::Length(4),
        Constraint::Min(0),
    ])
    .split(area);
    let inner = Rect { x: area.x + 2, y: area.y, width: area.width.saturating_sub(4), height: area.height };

    // Task
    let r = Rect { y: rows[0].y, height: rows[0].height, ..centered(inner, 64) };
    let w = r.width.saturating_sub(4) as usize;
    let src = match &j.source {
        Source::Manual => "typed task".to_string(),
        Source::Issue { repo, number, url, .. } => {
            if url.is_empty() { format!("issue {}#{}", repo, number) } else { url.clone() }
        }
        Source::Deploy { repo, sha, .. } => format!("{} at {}", repo, &sha[..sha.len().min(10)]),
        Source::Pr { repo, number, url, .. } => {
            if url.is_empty() { format!("pull request {}!{}", repo, number) } else { url.clone() }
        }
    };
    let secs = j.finished.unwrap_or_else(std::time::Instant::now).duration_since(j.created).as_secs();
    let lines = vec![
        Line::from(Span::styled(clip(&j.title, w), bold(BRIGHT))),
        Line::from(Span::styled(clip(&src, w), fg(DIM))),
        Line::from(vec![
            Span::styled(j.status.name(), bold(status_color(j.status))),
            Span::styled(format!("  ·  {}", mmss(secs)), fg(DIM)),
            Span::styled(j.branch.as_ref().map(|b| format!("  ·  {}", b)).unwrap_or_default(), fg(DIM)),
        ]),
    ];
    f.render_widget(Paragraph::new(lines).alignment(Alignment::Center).block(boxed(&format!("job #{}", j.id), ORANGE)), r);
    connector(f, rows[1], DIM);

    // Fork layer
    let r = Rect { y: rows[2].y, height: rows[2].height, ..centered(inner, 76) };
    let mut b = boxed("fork layer", PURPLE);
    let mut lines: Vec<Line> = Vec::new();
    match &j.decision {
        None => {
            let msg = if matches!(j.source, Source::Deploy { .. }) {
                "not used: a deployment runs no agent".to_string()
            } else if j.status == JobStatus::Queued {
                "waiting for a free slot".to_string()
            } else if j.status.finished() {
                "never reached".to_string()
            } else {
                format!("{} asking", spinner())
            };
            lines.push(Line::default());
            lines.push(Line::from(Span::styled(msg, fg(DIM))).alignment(Alignment::Center));
        }
        Some(d) => {
            b = b.title(Line::from(Span::styled(format!(" {} · {} ms ", d.source, d.ms), fg(DIM))).alignment(Alignment::Right));
            let bar_w = (r.width as usize).saturating_sub(50).clamp(8, 28);
            for g in &d.gates {
                let c = if g.sharp { GREEN } else { ORANGE };
                let filled = ((g.value.clamp(0.0, 1.0)) * bar_w as f64).round() as usize;
                let solid = filled.saturating_sub(3);
                lines.push(Line::from(vec![
                    Span::styled(format!("  {:<14}", g.name), fg(BRIGHT)),
                    Span::styled("█".repeat(solid), fg(c)),
                    Span::styled("▒".repeat(filled - solid), fg(c)),
                    Span::styled(" ".repeat(bar_w - filled), fg(DIM)),
                    Span::styled(format!("  {:.2} {:<6}", g.value, if g.sharp { "sharp" } else { "split" }), fg(c)),
                    Span::styled(g.answer.clone(), bold(TEXT)),
                ]));
            }
            let note = if d.note.is_empty() { "sharp → runs as answered · split → cautious branch".to_string() } else { d.note.clone() };
            lines.push(Line::from(Span::styled(format!("  {}", clip(&note, r.width.saturating_sub(6) as usize)), fg(DIM))));
        }
    }
    f.render_widget(Paragraph::new(lines).block(b), r);
    connector(f, rows[3], DIM);

    // Worker and reviewer
    let pair = Rect { y: rows[4].y, height: rows[4].height, ..centered(inner, 84) };
    let stages: Vec<&Stage> = [&j.worker, &j.reviewer].into_iter().chain(j.extra.iter()).collect();
    let cols = Layout::horizontal(vec![Constraint::Ratio(1, stages.len() as u32); stages.len()]).spacing(2).split(pair);
    let names: Vec<String> = match j.source {
        Source::Pr { .. } if stages.len() > 2 => (1..=stages.len()).map(|n| format!("review {}", n)).collect(),
        Source::Pr { .. } => vec!["first review".into(), "second review".into()],
        Source::Deploy { .. } => vec!["deploy".into(), if j.reviewer.label.is_empty() { "verify".into() } else { j.reviewer.label.clone() }],
        _ => vec!["worker".into(), "reviewer".into()],
    };
    for ((r, name), s) in cols.iter().zip(&names).zip(&stages) {
        stage_box(f, *r, name, s);
    }
    connector(f, rows[5], DIM);

    // Outcome
    let r = Rect { y: rows[6].y, height: rows[6].height, ..centered(inner, 76) };
    let w = r.width.saturating_sub(4) as usize;
    let (label, color) = match (j.status, j.verdict) {
        (JobStatus::Done, _) => ("done", GREEN),
        (JobStatus::Failed, _) => ("failed", RED),
        (JobStatus::Cancelled, _) => ("cancelled", DIM),
        (_, Some(true)) => ("review passed", GREEN),
        (_, Some(false)) => ("review failed · fixing", ORANGE),
        _ => ("in progress", DIM),
    };
    let text = if j.outcome.is_empty() { "waiting on the stages above".to_string() } else { j.outcome.clone() };
    let label = match j.checks {
        Some(true) => format!("{} · checks passed", label),
        Some(false) => format!("{} · checks failed", label),
        None => label.to_string(),
    };
    let _ = w;
    f.render_widget(
        Paragraph::new(Span::styled(text, fg(TEXT))).wrap(Wrap { trim: true }).block(boxed(&format!("outcome · {}", label), color).padding(Padding::horizontal(1))),
        r,
    );

    // Live output of whichever stage is active
    if rows[7].height >= 4 {
        let extra = j.extra.iter().enumerate().find(|(_, e)| matches!(e.state, StageState::Running) && !matches!(j.reviewer.state, StageState::Running));
        let (name, s) = if let Some((i, e)) = extra {
            (format!("review {}", i + 3), e)
        } else if matches!(j.reviewer.state, StageState::Running) || (j.status.finished() && !j.reviewer.tail.is_empty() && j.verdict == Some(false)) {
            ("reviewer".to_string(), &j.reviewer)
        } else {
            ("worker".to_string(), &j.worker)
        };
        let who = s.agent.map(|a| if s.via.is_empty() { a.name().to_string() } else { format!("{} via {}", a.name(), s.via) }).unwrap_or_else(|| "idle".into());
        output_panel(f, rows[7], &format!("{} output · {}", name, who), s, DIM);
    }
}

/// Follow the last screenful of rendered rows, including wrapped continuations.
fn output_panel(f: &mut Frame, area: Rect, title: &str, stage: &Stage, color: Color) {
    let block = boxed(title, color);
    let inner = block.inner(area);
    let lines: Vec<Line> = stage.tail.iter().flat_map(|entry| {
        entry.split('\n').map(|line| {
            let color = if line.starts_with('▸') { DIM } else if line.starts_with("error") { RED } else { TEXT };
            Line::from(Span::styled(line, fg(color)))
        })
    }).collect();
    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    // Use the widget's own wrapping calculation so Unicode and word boundaries agree.
    let scroll = paragraph.line_count(inner.width).saturating_sub(inner.height as usize).min(u16::MAX as usize) as u16;
    f.render_widget(paragraph.scroll((scroll, 0)).block(block), area);
}

fn stage_box(f: &mut Frame, area: Rect, role: &str, s: &Stage) {
    let color = match (s.state, s.agent) {
        (StageState::Failed, _) => RED,
        (StageState::Idle, _) | (StageState::Skipped, _) => DIM,
        (_, Some(a)) => agent_color(a),
        _ if !s.label.is_empty() => PURPLE,
        _ => DIM,
    };
    let w = area.width.saturating_sub(4) as usize;
    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(Span::styled(role.to_string(), bold(BRIGHT))));
    match s.agent {
        Some(a) => {
            let mut bits = vec![if s.via.is_empty() { a.name().to_string() } else { format!("{} via {}", a.name(), s.via) }];
            if !s.model.is_empty() {
                bits.push(s.model.clone());
            }
            if !s.effort.is_empty() {
                bits.push(s.effort.clone());
            }
            lines.push(Line::from(Span::styled(clip(&bits.join(" · "), w), fg(agent_color(a)))));
        }
        None if !s.label.is_empty() => lines.push(Line::from(Span::styled("via MCP", fg(PURPLE)))),
        None => lines.push(Line::from(Span::styled(if s.state == StageState::Skipped { "not needed" } else { "waiting" }, fg(DIM)))),
    }
    let st = match s.state {
        StageState::Idle => "[ idle ]".to_string(),
        StageState::Skipped => "[ skipped ]".to_string(),
        StageState::Running => format!("[{} running {}]", spinner(), mmss(s.secs())),
        StageState::Ok => format!("[✓ done {}]", mmss(s.secs())),
        StageState::Failed => format!("[✗ failed {}]", mmss(s.secs())),
    };
    lines.push(Line::from(Span::styled(st, fg(color))));
    if s.agent.is_none() && !s.label.is_empty() {
        lines.push(Line::from(Span::styled(clip(&s.last, w), fg(DIM))));
    }
    if s.agent.is_some() {
        let mut usage = format!("{} tools · {} in · {} out", s.tools, kilo(s.tokens_in), kilo(s.tokens_out));
        if s.cost > 0.0 {
            usage.push_str(&format!(" · ${:.2}", s.cost));
        }
        lines.push(Line::from(Span::styled(clip(&usage, w), fg(DIM))));
        lines.push(Line::from(Span::styled(clip(&s.last, w), fg(DIM))));
    }
    f.render_widget(Paragraph::new(lines).alignment(Alignment::Center).block(boxed("", color)), area);
}

fn log_panel(f: &mut Frame, area: Rect, st: &State) {
    let h = area.height.saturating_sub(1) as usize;
    let w = area.width as usize;
    let mut lines = vec![Line::from(Span::styled(format!("──── session log {}", "─".repeat(w.saturating_sub(17))), fg(Color::Rgb(40, 46, 56))))];
    for l in st.log.iter().skip(st.log.len().saturating_sub(h)) {
        let c = match l.who.as_str() {
            "router" => PURPLE,
            "review" => GREEN,
            "issues" => ORANGE,
            _ => TEXT,
        };
        lines.push(Line::from(vec![
            Span::styled(format!("  {}  ", l.time), fg(DIM)),
            Span::styled(format!("{:<8}", l.who), fg(c)),
            Span::styled(clip(&l.msg, w.saturating_sub(22)), fg(TEXT)),
        ]));
    }
    f.render_widget(Paragraph::new(lines), area);
}

fn prompt(f: &mut Frame, area: Rect, ui: &Ui, target: Option<&Job>) {
    let title = match target {
        Some(j) if !j.status.finished() => format!("Reply to #{} · {} queued · Enter to queue · Ctrl-T new task", j.id, j.queued_prompts.len()),
        Some(j) => format!("Reply to #{} · {} queued · Enter to send / resume · Ctrl-T new task", j.id, j.queued_prompts.len()),
        None => format!("Quick task · {} · Enter to queue", ui.forced.as_ref().map(|a| a.name()).unwrap_or_else(|| "auto".into())),
    };
    let room = area.width.saturating_sub(5) as usize;
    let count = ui.input.chars().count();
    let shown: String = ui.input.chars().skip(count.saturating_sub(room)).collect();
    let content = if ui.input.is_empty() {
        Span::styled(if target.is_some() { "Type a follow-up, or press Enter to resume…" } else { "Type a short task, e.g. fix the typo in README…" }, fg(DIM))
    } else {
        Span::styled(shown, fg(BRIGHT))
    };
    let line = Line::from(vec![Span::styled(" › ", fg(GREEN)), content, Span::styled("█", fg(PURPLE))]);
    f.render_widget(Paragraph::new(line).block(boxed(&title, if target.is_some() { GREEN } else { PURPLE })), area);
}

fn status(f: &mut Frame, area: Rect, st: &State) {
    let running = st.jobs.iter().filter(|j| !j.status.finished() && j.status != JobStatus::Queued).count();
    let queued = st.jobs.iter().filter(|j| j.status == JobStatus::Queued).count();
    let left = Line::from(vec![
        Span::styled(" running: ", fg(DIM)),
        Span::styled(format!("[{}]", running), fg(ORANGE)),
        Span::styled("  queued: ", fg(DIM)),
        Span::styled(format!("[{}]", queued), fg(TEXT)),
        Span::styled("  issues: ", fg(DIM)),
        Span::styled(if st.automation { "[on]" } else { "[off]" }, fg(if st.automation { GREEN } else { DIM })),
    ]);
    if area.width > 155 { f.render_widget(Paragraph::new(left), area); }
    let keys = if area.width < 110 {
        "⏎ send · ↑↓ history · ^T new · F5 choices · F6 issue plan · ^C quit"
    } else {
        "⏎ send · ↑↓ history · Alt↑↓ job · F2 view · F5 choices · F6 issue plan · Tab agent · ^T new · ^R reply · ^X stop · ^C quit "
    };
    f.render_widget(Paragraph::new(Span::styled(keys, fg(DIM))).alignment(Alignment::Right), area);
}

fn css(c: Color, default: &str) -> String {
    match c {
        Color::Rgb(r, g, b) => format!("rgb({},{},{})", r, g, b),
        _ => default.to_string(),
    }
}

/// Render a buffer as a standalone HTML page. Used for screenshots and tests.
pub fn to_html(buf: &Buffer) -> String {
    let mut out = String::from(
        "<!doctype html><meta charset=utf-8><style>body{margin:0;background:rgb(13,17,23)}\
         pre{margin:0;padding:14px 18px;font:14px/1.25 'DejaVu Sans Mono','Menlo',monospace;color:rgb(201,209,217);display:inline-block}\
         b{font-weight:bold}</style><pre>",
    );
    let a = buf.area;
    for y in a.y..a.y + a.height {
        for x in a.x..a.x + a.width {
            let cell = &buf[(x, y)];
            let sym = cell.symbol().replace('&', "&amp;").replace('<', "&lt;");
            let mut style = format!("color:{}", css(cell.fg, "inherit"));
            if let Color::Rgb(..) = cell.bg {
                if cell.bg != BG {
                    style.push_str(&format!(";background:{}", css(cell.bg, "none")));
                }
            }
            if cell.modifier.contains(Modifier::BOLD) {
                style.push_str(";font-weight:bold");
            }
            out.push_str(&format!("<span style=\"{}\">{}</span>", style, sym));
        }
        out.push('\n');
    }
    out.push_str("</pre>");
    out
}

pub fn to_text(buf: &Buffer) -> String {
    let a = buf.area;
    let mut out = String::new();
    for y in a.y..a.y + a.height {
        let mut line = String::new();
        for x in a.x..a.x + a.width {
            line.push_str(buf[(x, y)].symbol());
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sending_to_a_busy_task_clears_the_draft_and_displays_its_queue() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let hub = crate::orchestrator::Hub::new(crate::config::Config::default(), rt.handle().clone());
        let mut ui = crate::new_ui(&hub, ".".into());
        let mut history = crate::input::History::default();
        let id = hub.submit("task".into(), "original".into(), Source::Manual, ".".into(), None);
        ui.selected = Some(id);
        ui.input = "Follow up while busy".into();
        crate::submit_input(&hub, &mut ui, &mut history);
        assert!(ui.input.is_empty());
        assert_eq!(history.messages(), &["Follow up while busy"]);
        assert_eq!(hub.state.lock().unwrap().jobs.len(), 1);
        // Empty Enter while busy must not queue a spurious resume request.
        crate::submit_input(&hub, &mut ui, &mut history);
        assert_eq!(hub.job(id, |j| j.queued_prompts.len()), Some(1));
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(140, 40)).unwrap();
        term.draw(|f| draw(f, &hub.state.lock().unwrap(), &ui)).unwrap();
        let text = to_text(term.backend().buffer());
        assert!(text.contains("1 queued"), "{text}");
        assert!(text.contains("next: Follow up while busy"), "{text}");
        assert!(text.contains("Enter to queue"), "{text}");
    }

    #[test]
    fn output_wraps_and_follows_newest_rendered_rows_on_resize() {
        let mut stage = Stage::default();
        stage.push("first second third fourth fifth sixth seventh eighth");
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(22, 8)).unwrap();
        term.draw(|f| output_panel(f, f.area(), "worker", &stage, GREEN)).unwrap();
        let text = to_text(term.backend().buffer());
        assert!(text.contains("first second third"), "{text}");
        assert!(text.contains("fourth fifth sixth"), "{text}");
        assert!(text.contains("seventh eighth"), "{text}");
        stage.push("OLD\n日本語 日本語 日本語 日本語\nlast message wraps across several rows with FINAL at the end");
        for width in [18, 30, 60] {
            term.backend_mut().resize(width, 5);
            term.resize(Rect::new(0, 0, width, 5)).unwrap();
            term.draw(|f| output_panel(f, f.area(), "reviewer", &stage, GREEN)).unwrap();
            let text = to_text(term.backend().buffer());
            assert!(text.contains("FINAL"), "{text}");
            assert!(!text.contains("first second"), "{text}");
            assert!(!text.contains('…'), "{text}");
        }
    }

    #[test]
    fn agent_output_survives_ingestion_and_wrapping() {
        for role in ["worker", "reviewer"] {
            for kind in 0..3 {
                let text = format!("{} END_SENTINEL", "日本語 paragraph ".repeat(60));
                let event = match kind {
                    0 => crate::agents::Parsed::Text(text.clone()),
                    1 => crate::agents::Parsed::Note(text.clone()),
                    _ => crate::agents::Parsed::Tool(text.clone()),
                };
                let mut stage = Stage::default();
                crate::agents::record_event(&mut stage, &event);
                let expected = if kind == 2 { format!("▸ {text}") } else { text };
                assert_eq!(stage.tail.back(), Some(&expected));
                for width in [22, 40, 80] {
                    let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, 6)).unwrap();
                    term.draw(|f| output_panel(f, f.area(), role, &stage, GREEN)).unwrap();
                    let visible = to_text(term.backend().buffer());
                    assert!(visible.contains("END_SENTINEL"), "{visible}");
                    assert!(!visible.contains('…'), "{visible}");
                }
            }
        }
    }

    #[test]
    fn board_keeps_selected_older_job_visible_and_renders_small_terminals() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let hub = crate::orchestrator::Hub::new(crate::config::Config::default(), rt.handle().clone());
        let mut ui = crate::new_ui(&hub, PathBuf::from("."));
        for index in 0..15 {
            let id = hub.submit(format!("TASK-{}", index), "task".into(), Source::Manual, PathBuf::from("."), None);
            hub.job(id, |j| j.status = JobStatus::Done);
            if index == 0 { ui.selected = Some(id); }
        }
        for (width, height) in [(180, 50), (120, 40), (80, 30), (40, 12)] {
            let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
            term.draw(|f| draw(f, &hub.state.lock().unwrap(), &ui)).unwrap();
            if width >= 80 {
                let text = to_text(term.backend().buffer());
                assert!(text.contains("◆#1 TASK-0"), "{text}");
                assert!(text.contains("DONE 15"), "{text}");
            }
        }
    }
}
