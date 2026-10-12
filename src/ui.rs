//! The agent-tree screen.

use crate::state::*;
use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::border;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Padding, Paragraph, Wrap};
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
    /// Enter starts a new task even though a typed job is selected (Ctrl-N).
    pub new_task: bool,
    /// The next new task proposes approaches first (Ctrl-P).
    pub plan_next: bool,
}

/// The selected job, when Enter would send what is typed to it as a reply.
pub fn reply_target(st: &State, ui: &Ui) -> Option<u32> {
    if ui.new_task {
        return None;
    }
    let id = ui.selected?;
    st.jobs.iter().find(|j| j.id == id && j.source.is_manual()).map(|j| j.id)
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
    let area = f.area();
    f.render_widget(Block::new().style(Style::new().bg(BG).fg(TEXT)), area);
    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(2),
        Constraint::Min(8),
        Constraint::Length(7),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(area);

    title_bar(f, rows[0], ui);
    header(f, rows[1], st);
    let cols = Layout::horizontal([Constraint::Length(34), Constraint::Min(40)]).split(rows[2]);
    left_panel(f, cols[0], st, ui);
    let job = ui.selected.and_then(|id| st.jobs.iter().find(|j| j.id == id));
    match job {
        _ if ui.team => team_panel(f, cols[1], st),
        Some(j) => tree(f, cols[1], j),
        None => empty_tree(f, cols[1]),
    }
    log_panel(f, rows[3], st);
    let target = reply_target(st, ui).map(|id| (id, st.jobs.iter().any(|j| j.id == id && !j.status.finished())));
    prompt(f, rows[4], ui, target);
    status(f, rows[5], st);
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
        let (word, color) = if st.has(k) && (k != AgentKind::Copilot || st.copilot_usable()) {
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
        Span::styled("  FORKDECK AGENT TREE", bold(BRIGHT)),
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
        Span::styled("      ■ ", fg(ORANGE)),
        Span::styled("claude", fg(DIM)),
        Span::styled("    ■ ", fg(GREEN)),
        Span::styled("codex", fg(DIM)),
        Span::styled("    ■ ", fg(BLUE)),
        Span::styled("copilot", fg(DIM)),
        Span::styled("    ■ ", fg(PURPLE)),
        Span::styled("fork layer", fg(DIM)),
        Span::styled("    ■ ", fg(RED)),
        Span::styled("failed", fg(DIM)),
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
        let r = rows[7];
        let h = r.height.saturating_sub(2) as usize;
        let w = r.width.saturating_sub(4) as usize;
        let lines: Vec<Line> = s
            .tail
            .iter()
            .skip(s.tail.len().saturating_sub(h))
            .map(|l| {
                let c = if l.starts_with('▸') { DIM } else if l.starts_with("error") { RED } else { TEXT };
                Line::from(Span::styled(format!(" {}", clip(l, w)), fg(c)))
            })
            .collect();
        let who = s.agent.map(|a| if s.via.is_empty() { a.name().to_string() } else { format!("{} via {}", a.name(), s.via) }).unwrap_or_else(|| "idle".into());
        f.render_widget(Paragraph::new(lines).block(boxed(&format!("{} output · {}", name, who), DIM)), r);
    }
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

/// `reply`: the job Enter would send to, and whether it is still running.
fn prompt(f: &mut Frame, area: Rect, ui: &Ui, reply: Option<(u32, bool)>) {
    let dir = ui.cwd.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let forced = match (reply, &ui.forced) {
        (Some((id, true)), _) => Span::styled(format!("[message #{}] ", id), fg(GREEN)),
        (Some((id, false)), _) => Span::styled(format!("[reply to #{}] ", id), fg(GREEN)),
        (None, None) => Span::styled("[agent: auto] ", fg(DIM)),
        (None, Some(a)) => Span::styled(format!("[agent: {}] ", a.name()), fg(agent_color(a.kind()))),
    };
    let room = (area.width as usize).saturating_sub(dir.len() + 28);
    let shown: String = {
        let n = ui.input.chars().count();
        ui.input.chars().skip(n.saturating_sub(room)).collect()
    };
    let line = Line::from(vec![
        Span::styled(format!(" ~/{} ", dir), fg(GREEN)),
        Span::styled("$ ", fg(BRIGHT)),
        Span::styled(if ui.plan_next && reply.is_none() { "[plan first] " } else { "" }, fg(PURPLE)),
        forced,
        Span::styled(shown, fg(BRIGHT)),
        Span::styled("█", fg(PURPLE)),
    ]);
    f.render_widget(Paragraph::new(line).wrap(Wrap { trim: false }), area);
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
    f.render_widget(Paragraph::new(left), area);
    let keys = "⏎ run · Tab agent · ↑↓ job · ^A issues · ^T team · ^X stop · ^C quit ";
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
