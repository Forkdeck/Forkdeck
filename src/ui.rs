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
    /// Enter starts a new task even though a typed job is selected (Ctrl-T or Ctrl-N).
    pub new_task: bool,
    /// The next new task proposes approaches first (Ctrl-P).
    pub plan_next: bool,
    /// The factory board in place of the agent tree (F2).
    pub factory: bool,
    /// Explicit reply destination, pinned even when selection changes (Ctrl-R).
    pub reply_to: Option<u32>,
    pub choices: Option<crate::choices::Picker>,
    pub issue_panel: Option<crate::plan_ui::Panel>,
    /// Answers already looked at for choices: (job, turns at the time).
    pub seen_choices: std::collections::HashSet<(u32, usize)>,
    /// The daily limits, to show next to what has been used. 0: none.
    pub jobs_per_day: u32,
    pub tokens_per_day: u64,
    /// Edit lines on screen, as last drawn, for a click to open their diff.
    pub spots: std::cell::RefCell<Vec<crate::diff::Spot>>,
    /// The full-screen diff opened by clicking an edit line.
    pub diff: Option<crate::diff::View>,
    /// How far the selected job's output panes are scrolled back (PageUp), and for which job;
    /// none, or another job, follows the end.
    /// Drawing moves it back further as output arrives, so what is on screen holds still.
    pub output_back: std::cell::Cell<Option<(u32, usize)>>,
    /// How far the session log is scrolled back (Shift-PageUp); 0 follows the end.
    pub log_back: std::cell::Cell<usize>,
    /// Rows of an output pane and how far it can scroll back, as last drawn.
    pub output_view: std::cell::Cell<Scroll>,
    /// The same for the session log.
    pub log_view: std::cell::Cell<Scroll>,
}

/// What a scrolled pane last showed: its height in rows, and the furthest it can go back.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Scroll {
    pub rows: usize,
    pub max_back: usize,
}

/// One page further back (`up`) or forward from `back` lines before the end, keeping a couple of
/// lines of context, never past the start and never below 0 (which follows the end again).
pub fn page(back: usize, view: Scroll, up: bool) -> usize {
    let step = view.rows.saturating_sub(2).max(1);
    if up { (back + step).min(view.max_back) } else { back.min(view.max_back).saturating_sub(step) }
}

impl Ui {
    /// Lines back from the end for the selected job's output panes.
    pub fn back_for(&self, job: u32) -> usize {
        self.output_back.get().filter(|(j, _)| *j == job).map(|(_, b)| b).unwrap_or(0)
    }

    /// The job's output panes as just drawn. While scrolled back, what arrived since the last
    /// draw adds to how far back, so the lines being read stay where they are.
    fn output_drawn(&self, job: u32, view: Scroll) {
        let before = self.output_view.replace(view);
        if let Some((j, back)) = self.output_back.get().filter(|(j, b)| *j == job && *b > 0) {
            self.output_back.set(Some((j, held(back, before, view))));
        }
    }
}

/// `back` for a pane that had `before` and now has `now` rows hidden at the top: held still while scrolled.
fn held(back: usize, before: Scroll, now: Scroll) -> usize {
    if back == 0 { 0 } else { (back + now.max_back.saturating_sub(before.max_back)).min(now.max_back) }
}

/// The typed task Enter sends to: the one pinned with Ctrl-R, else the selected one. The input
/// box's title says which, so this is never a surprise.
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
        JobStatus::Interrupted => Color::Rgb(230, 190, 60),
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
    ui.spots.borrow_mut().clear();
    if let Some(v) = &ui.diff {
        crate::diff::draw(f, f.area(), v);
        return;
    }
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
        Some(j) => tree(f, cols[1], j, ui),
        None => empty_tree(f, cols[1]),
    }
    }
    log_panel(f, rows[3], st, ui);
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
        JobStatus::Queued | JobStatus::Routing | JobStatus::Planning | JobStatus::Choosing => 0,
        JobStatus::Working | JobStatus::Fixing => 1,
        JobStatus::Reviewing => 2,
        JobStatus::Publishing | JobStatus::Ci | JobStatus::Deploying => 3,
        JobStatus::Done => 4,
        JobStatus::Failed | JobStatus::Cancelled | JobStatus::Interrupted | JobStatus::RollingBack => 5,
    }
}

/// The board lane a job's card is in.
pub(crate) fn job_lane(j: &Job) -> usize {
    if j.status == JobStatus::Done && j.checks.is_none() && j.issue_plan.is_none() { return 5; }
    // A pull request still open is work a person has to review, not work that is done.
    if j.status == JobStatus::Done && j.pr_url.is_some() && j.pr_open != Some(false) {
        return 3;
    }
    let status = if j.status == JobStatus::Done && j.issue_plan.as_ref().is_some_and(|p| p.build_job.is_none()) { JobStatus::Planning } else { j.status };
    lane(status)
}

/// Alt+Right (`forward`) or Alt+Left: the newest job in the next lane that has any, from the
/// selected job's lane. At either end the selection stays where it is.
pub fn lane_step(st: &State, selected: Option<u32>, forward: bool) -> Option<u32> {
    let newest = |l: usize| st.jobs.iter().rev().find(|j| job_lane(j) == l).map(|j| j.id);
    let Some(from) = selected.and_then(|id| st.jobs.iter().find(|j| j.id == id)).map(job_lane) else {
        return (0..6).find_map(newest);
    };
    let next = if forward { (from + 1..6).find_map(newest) } else { (0..from).rev().find_map(newest) };
    next.or(selected)
}

fn factory(f: &mut Frame, area: Rect, st: &State, ui: &Ui) {
    let narrow = area.width < 120;
    let rows = Layout::vertical([Constraint::Length(if narrow { 14 } else { 9 }), Constraint::Min(4)]).split(area);
    let board_rows = Layout::vertical(vec![Constraint::Ratio(1, if narrow { 2 } else { 1 }); if narrow { 2 } else { 1 }]).split(rows[0]);
    let names = ["QUEUE", "BUILD", "REVIEW", "PR REVIEW", "DONE", "ATTENTION"];
    let colors = [PURPLE, ORANGE, PURPLE, ORANGE, GREEN, RED];
    for (index, name) in names.iter().enumerate() {
        let count = if narrow { 3 } else { 6 };
        let columns = Layout::horizontal(vec![Constraint::Ratio(1, count as u32); count]).split(board_rows[index / count]);
        let cell = columns[index % count];
        let jobs: Vec<_> = st.jobs.iter().filter(|j| job_lane(j) == index).collect();
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
            let queued = job.inbox.front().map(|next| format!("{} queued · next: {}", job.inbox.len(), clip(next, 100)));
            let outcome = if let Some(ref queued) = queued { queued.as_str() } else if job.outcome.is_empty() { job.pr_url.as_deref().unwrap_or("Work is in progress") } else { &job.outcome };
            let summary = job.issue_plan.as_ref().map(|p| p.status(st.automation)).unwrap_or_else(|| format!("Checks: {} · Review: {} · {}", checks, review, job.workdir.display()));
            let lines = vec![
                Line::from(Span::styled(format!(" {summary}"), fg(TEXT))),
                Line::from(Span::styled(format!(" {}", outcome), fg(status_color(job.status)))),
            ];
            f.render_widget(Paragraph::new(lines).block(boxed(&format!("#{} {} · {}", job.id, job.title, job.status.name()), status_color(job.status))), detail[0]);
            let outputs = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).split(detail[1]);
            let mut view = Scroll::default();
            for (rect, role, stage) in [(outputs[0], "worker", &job.worker), (outputs[1], "reviewer", &job.reviewer)] {
                let agent = stage.agent.map(|a| a.name()).unwrap_or("waiting");
                let (spots, v) = scrolled_panel(f, rect, &format!("{} · {} · {} tools", role, agent, stage.tools), stage, stage.agent.map(agent_color).unwrap_or(DIM), ui.back_for(job.id));
                view = Scroll { rows: v.rows, max_back: view.max_back.max(v.max_back) };
                ui.spots.borrow_mut().extend(spots.into_iter().map(|s| crate::diff::Spot { job: job.id, ..s }));
            }
            ui.output_drawn(job.id, view);
        }
        None => {
            let lines = vec![
                Line::from(Span::styled("Ready for work", bold(BRIGHT))),
                Line::from("Type a task and press Enter. Jobs move through the board as they run."),
                Line::from(format!("{} repositories configured · issue intake {} (Ctrl-A)", st.repos.len(), if st.automation { "on" } else { "paused" })),
                Line::from("Alt↑↓ selects a job · Alt←→ a lane · F2 opens the agent tree · F4 shows the team"),
            ];
            f.render_widget(Paragraph::new(lines).alignment(Alignment::Center).block(boxed("dispatch", PURPLE)), rows[1]);
        }
    }
}

fn title_bar(f: &mut Frame, area: Rect, ui: &Ui) {
    let dir = ui.cwd.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let t = format!("~/{} — forkdeck{} — {}x{}", dir, if ui.demo { " (demo)" } else { "" }, area.width, f.area().height);
    f.render_widget(Paragraph::new(Span::styled(t, fg(DIM))).alignment(Alignment::Center), area);
}

fn header(f: &mut Frame, area: Rect, st: &State) {
    let chip = |k: AgentKind, own: Color| -> Span {
        let (word, color) = if k == AgentKind::Copilot && st.copilot_checking {
            ("CHECKING", DIM)
        } else if st.has(k) && !st.agent_ready(k) {
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
        // Messages waiting for the agent show on the row.
        let text = if j.inbox.is_empty() { format!("#{} {}", j.id, j.title) } else { format!("#{} ✉{} {}", j.id, j.inbox.len(), j.title) };
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

fn tree(f: &mut Frame, area: Rect, j: &Job, ui: &Ui) {
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
            Span::styled(if j.inbox.is_empty() { String::new() } else { format!("  ·  ✉ {} queued", j.inbox.len()) }, fg(GREEN)),
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
        let (spots, view) = scrolled_panel(f, rows[7], &format!("{} output · {}", name, who), s, DIM, ui.back_for(j.id));
        ui.output_drawn(j.id, view);
        ui.spots.borrow_mut().extend(spots.into_iter().map(|s| crate::diff::Spot { job: j.id, ..s }));
    }
}

/// Follow the last screenful of rendered rows, including wrapped continuations. Returns where the
/// visible edit lines were drawn, for a click to open their diff (the job is filled in by the caller).
#[cfg(test)]
fn output_panel(f: &mut Frame, area: Rect, title: &str, stage: &Stage, color: Color) -> Vec<crate::diff::Spot> {
    scrolled_panel(f, area, title, stage, color, 0).0
}

/// `output_panel`, `back` rendered rows before the end (0 follows the end). Also says how tall the
/// pane is and how far back it can go.
fn scrolled_panel(f: &mut Frame, area: Rect, title: &str, stage: &Stage, color: Color, back: usize) -> (Vec<crate::diff::Spot>, Scroll) {
    let probe = boxed(title, color).inner(area);
    let raw: Vec<&str> = stage.tail.iter().flat_map(|entry| entry.split('\n')).collect();
    let total = Paragraph::new(raw.iter().map(|l| Line::from(*l)).collect::<Vec<_>>()).wrap(Wrap { trim: false }).line_count(probe.width);
    let max_back = total.saturating_sub(probe.height as usize);
    let back = back.min(max_back);
    let title = if back > 0 { format!("{title} · ↑ {back} lines back · PgDn/End") } else { title.to_string() };
    let block = boxed(&title, color);
    let inner = block.inner(area);
    let raw: Vec<&str> = stage.tail.iter().flat_map(|entry| entry.split('\n')).collect();
    let lines: Vec<Line> = raw.iter().map(|line| {
        let color = if line.starts_with('▸') { DIM } else if line.starts_with("error") { RED } else { TEXT };
        Line::from(Span::styled(*line, fg(color)))
    }).collect();
    let paragraph = Paragraph::new(lines.clone()).wrap(Wrap { trim: false });
    // Use the widget's own wrapping calculation so Unicode and word boundaries agree.
    let scroll = paragraph.line_count(inner.width).saturating_sub(inner.height as usize).saturating_sub(back).min(u16::MAX as usize) as u16;
    f.render_widget(paragraph.scroll((scroll, 0)).block(block), area);
    let (mut spots, mut top) = (Vec::new(), 0usize);
    for (raw, line) in raw.iter().zip(lines) {
        let rows = Paragraph::new(line).wrap(Wrap { trim: false }).line_count(inner.width);
        let (start, end) = (top.max(scroll as usize), (top + rows).min(scroll as usize + inner.height as usize));
        if let (Some(files), true) = (crate::diff::edited_files(raw), start < end) {
            let area = Rect::new(inner.x, inner.y + (start - scroll as usize) as u16, inner.width, (end - start) as u16);
            spots.push(crate::diff::Spot { area, job: 0, files });
        }
        top += rows;
    }
    (spots, Scroll { rows: inner.height as usize, max_back })
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

fn log_panel(f: &mut Frame, area: Rect, st: &State, ui: &Ui) {
    let h = area.height.saturating_sub(1) as usize;
    let w = area.width as usize;
    let max_back = st.log.len().saturating_sub(h);
    let view = Scroll { rows: h, max_back };
    let back = held(ui.log_back.get(), ui.log_view.replace(view), view);
    ui.log_back.set(back);
    let head = if back > 0 { format!("──── session log · ↑ {back} back · Shift-PgDn ") } else { "──── session log ".to_string() };
    let rule = w.saturating_sub(head.chars().count());
    let mut lines = vec![Line::from(Span::styled(format!("{head}{}", "─".repeat(rule)), fg(Color::Rgb(40, 46, 56))))];
    for l in st.log.iter().skip(max_back - back).take(h) {
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
        Some(j) if j.status == JobStatus::Choosing => format!("Choose for #{} · a number, or your own approach · F5 options · Ctrl-T new task", j.id),
        Some(j) if j.status == JobStatus::Interrupted => format!("Reply to #{} · Enter with nothing typed resumes it · Ctrl-T new task", j.id),
        Some(j) if !j.status.finished() => format!("Message #{} · {} queued · Enter to queue · Ctrl-S now · Ctrl-T new task", j.id, j.inbox.len()),
        Some(j) => format!("Reply to #{} · {} queued · Enter to send · Ctrl-T new task", j.id, j.inbox.len()),
        None => format!("{} · {} · Enter to start", if ui.plan_next { "New task, planned first" } else { "Quick task" }, ui.forced.as_ref().map(|a| a.name()).unwrap_or_else(|| "auto".into())),
    };
    let room = area.width.saturating_sub(5) as usize;
    let count = ui.input.chars().count();
    let shown: String = ui.input.chars().skip(count.saturating_sub(room)).collect();
    let content = if ui.input.is_empty() {
        Span::styled(if target.is_some() { "Type a follow-up…" } else { "Type a short task, e.g. fix the typo in README…" }, fg(DIM))
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
        "⏎ send · ↑↓ history · Alt↑↓ job · Alt←→ lane · PgUp/Dn output · F2 view · F5 choices · F6 issue plan · Tab agent · ^T new · ^P plan · ^R reply · ^S now · ^X stop · ^C quit "
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
        // Jobs stay queued: the task is busy for as long as the test needs.
        let _slots = rt.block_on(hub.sem.clone().acquire_many_owned(hub.cfg.automation.max_parallel as u32)).unwrap();
        let id = rt.block_on(hub.submit("task".into(), "original".into(), Source::Manual, ".".into(), None, Planning::Never)).unwrap();
        ui.selected = Some(id);
        ui.input = "Follow up while busy".into();
        crate::submit_input(&hub, &mut ui, &mut history);
        assert!(ui.input.is_empty());
        assert_eq!(history.messages(), &["Follow up while busy"]);
        assert_eq!(hub.state.lock().unwrap().jobs.len(), 1);
        // Empty Enter while busy must not queue a spurious resume request.
        crate::submit_input(&hub, &mut ui, &mut history);
        assert_eq!(hub.job(id, |j| j.inbox.len()), Some(1));
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(140, 40)).unwrap();
        term.draw(|f| draw(f, &hub.state.lock().unwrap(), &ui)).unwrap();
        let text = to_text(term.backend().buffer());
        assert!(text.contains("1 queued"), "{text}");
        assert!(text.contains("next: Follow up while busy"), "{text}");
        assert!(text.contains("Enter to queue"), "{text}");
    }

    #[test]
    fn edit_lines_are_clickable_where_they_are_drawn() {
        let mut stage = Stage::default();
        stage.push("first second third fourth fifth sixth seventh eighth");
        stage.push("▸ edit /w/crates/a/src/a_long_file_name.rs");
        stage.push("▸ shell cargo test");
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(22, 12)).unwrap();
        let mut spots = Vec::new();
        term.draw(|f| spots = output_panel(f, f.area(), "worker", &stage, GREEN)).unwrap();
        assert_eq!(spots.len(), 1, "{spots:?}");
        let buf = term.backend().buffer();
        let row = |y: u16| (0..22).map(|x| buf[(x, y)].symbol()).collect::<String>();
        let s = &spots[0];
        assert!(row(s.area.y).contains("▸ edit"), "{}", row(s.area.y));
        assert!(s.area.height >= 2, "the wrapped path is part of it: {s:?}");
        assert!(!row(s.area.y + s.area.height).contains("a_long"), "and nothing below it");
        assert_eq!(s.files, ["/w/crates/a/src/a_long_file_name.rs"]);
    }

    #[test]
    fn paging_keeps_context_and_stops_at_both_ends() {
        let view = Scroll { rows: 10, max_back: 25 };
        assert_eq!(page(0, view, true), 8, "a page less two lines of context");
        assert_eq!(page(8, view, true), 16);
        assert_eq!(page(24, view, true), 25, "never past the start");
        assert_eq!(page(25, view, false), 17);
        assert_eq!(page(5, view, false), 0, "back to following the end");
        assert_eq!(page(90, view, false), 17, "scrolled further than the text is now: from the start");
        assert_eq!(page(0, Scroll { rows: 1, max_back: 3 }, true), 1, "a tiny pane still moves");
    }

    #[test]
    fn a_scrolled_pane_holds_still_as_lines_arrive() {
        let (before, now) = (Scroll { rows: 6, max_back: 24 }, Scroll { rows: 6, max_back: 27 });
        assert_eq!(held(10, before, now), 13, "three lines arrived: three further back");
        assert_eq!(held(0, before, now), 0, "following the end keeps following");
        assert_eq!(held(26, before, Scroll { rows: 6, max_back: 20 }), 20, "never past the start");
    }

    #[test]
    fn an_output_pane_scrolls_back_and_says_so() {
        let mut stage = Stage::default();
        for n in 1..=30 {
            stage.push(format!("line {n}"));
        }
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(40, 8)).unwrap();
        let mut view = Scroll::default();
        term.draw(|f| { view = scrolled_panel(f, f.area(), "worker", &stage, GREEN, 0).1; }).unwrap();
        let text = to_text(term.backend().buffer());
        assert!(text.contains("line 30") && !text.contains("line 24") && !text.contains("back"), "{text}");
        assert_eq!(view, Scroll { rows: 6, max_back: 24 });
        term.draw(|f| { scrolled_panel(f, f.area(), "worker", &stage, GREEN, 10); }).unwrap();
        let text = to_text(term.backend().buffer());
        assert!(text.contains("line 15") && text.contains("line 20") && !text.contains("line 21"), "{text}");
        assert!(text.contains("10 lines back"), "{text}");
        // Further back than there is: the start.
        term.draw(|f| { scrolled_panel(f, f.area(), "worker", &stage, GREEN, 500); }).unwrap();
        let text = to_text(term.backend().buffer());
        assert!(text.contains("line 1 ") && text.contains("24 lines back"), "{text}");
    }

    #[test]
    fn output_wraps_and_follows_newest_rendered_rows_on_resize() {
        let mut stage = Stage::default();
        stage.push("first second third fourth fifth sixth seventh eighth");
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(22, 8)).unwrap();
        term.draw(|f| { output_panel(f, f.area(), "worker", &stage, GREEN); }).unwrap();
        let text = to_text(term.backend().buffer());
        assert!(text.contains("first second third"), "{text}");
        assert!(text.contains("fourth fifth sixth"), "{text}");
        assert!(text.contains("seventh eighth"), "{text}");
        stage.push("OLD\n日本語 日本語 日本語 日本語\nlast message wraps across several rows with FINAL at the end");
        for width in [18, 30, 60] {
            term.backend_mut().resize(width, 5);
            term.resize(Rect::new(0, 0, width, 5)).unwrap();
            term.draw(|f| { output_panel(f, f.area(), "reviewer", &stage, GREEN); }).unwrap();
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
                    term.draw(|f| { output_panel(f, f.area(), role, &stage, GREEN); }).unwrap();
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
        let _slots = rt.block_on(hub.sem.clone().acquire_many_owned(hub.cfg.automation.max_parallel as u32)).unwrap();
        for index in 0..15 {
            let id = rt.block_on(hub.submit(format!("TASK-{}", index), "task".into(), Source::Manual, PathBuf::from("."), None, Planning::Never)).unwrap();
            hub.job(id, |j| { j.status = JobStatus::Done; j.checks = Some(true); });
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

    #[test]
    fn a_finished_job_waits_in_pr_review_until_its_pull_request_closes() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let hub = crate::orchestrator::Hub::new(crate::config::Config::default(), rt.handle().clone());
        let _slots = rt.block_on(hub.sem.clone().acquire_many_owned(hub.cfg.automation.max_parallel as u32)).unwrap();
        let id = rt.block_on(hub.submit("t".into(), "t".into(), Source::Manual, PathBuf::from("."), None, Planning::Never)).unwrap();
        let lane_now = || hub.job(id, |j| job_lane(j)).unwrap();
        hub.job(id, |j| { j.status = JobStatus::Done; j.checks = Some(true); });
        assert_eq!(lane_now(), 4, "verified work without a pull request is done");
        hub.job(id, |j| j.checks = None);
        assert_eq!(lane_now(), 5, "finished work without checks needs attention");
        hub.job(id, |j| j.checks = Some(true));
        hub.job(id, |j| j.pr_url = Some("https://github.com/o/r/pull/9".into()));
        assert_eq!(lane_now(), 3, "a person still has to review it");
        hub.job(id, |j| j.pr_open = Some(true));
        assert_eq!(lane_now(), 3);
        hub.job(id, |j| j.pr_open = Some(false));
        assert_eq!(lane_now(), 4, "merged or closed");
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(180, 40)).unwrap();
        hub.job(id, |j| j.pr_open = None);
        let ui = crate::new_ui(&hub, PathBuf::from("."));
        term.draw(|f| draw(f, &hub.state.lock().unwrap(), &ui)).unwrap();
        assert!(to_text(term.backend().buffer()).contains("PR REVIEW 1"));
    }

    #[test]
    fn alt_left_and_right_move_between_lanes_that_have_jobs() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let hub = crate::orchestrator::Hub::new(crate::config::Config::default(), rt.handle().clone());
        let _slots = rt.block_on(hub.sem.clone().acquire_many_owned(hub.cfg.automation.max_parallel as u32)).unwrap();
        let mut ids = Vec::new();
        for status in [JobStatus::Queued, JobStatus::Done, JobStatus::Done, JobStatus::Failed] {
            let id = rt.block_on(hub.submit("t".into(), "t".into(), Source::Manual, PathBuf::from("."), None, Planning::Never)).unwrap();
            hub.job(id, |j| { j.status = status; j.checks = Some(true); });
            ids.push(id);
        }
        let st = hub.state.lock().unwrap();
        let (queued, done_old, done_new, failed) = (ids[0], ids[1], ids[2], ids[3]);
        assert_eq!(lane_step(&st, Some(queued), true), Some(done_new), "empty lanes are skipped; the newest job is taken");
        assert_eq!(lane_step(&st, Some(done_old), true), Some(failed));
        assert_eq!(lane_step(&st, Some(failed), true), Some(failed), "stays at the last lane");
        assert_eq!(lane_step(&st, Some(failed), false), Some(done_new));
        assert_eq!(lane_step(&st, Some(queued), false), Some(queued), "stays at the first lane");
        assert_eq!(lane_step(&st, None, true), Some(queued), "nothing selected: the first lane with jobs");
    }
}
