//! Issue planning stays attached to the selected conversation.
use crate::{config::RepoCfg, input::History, orchestrator::Hub, planning::{Action, Plan}, state::*, ui::Ui};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{Frame, layout::{Constraint, Layout, Rect}, style::{Color, Style}, widgets::{Block, Clear, Paragraph, Wrap}};

pub enum Panel {
    Setup { repos: Vec<RepoCfg>, repo: usize, focus: u8, link: String, prompt: String, task: Option<u32>, error: String },
    Detail { id: u32, review: bool, scroll: u16, selected: usize, error: String },
}

impl Panel {
    pub fn draft(&self) -> Option<&str> {
        match self { Self::Setup { prompt, .. } => Some(prompt), Self::Detail { .. } => None }
    }
    pub fn paste(&mut self, text: &str) {
        if let Self::Setup { focus, link, prompt, .. } = self {
            if *focus == 1 { link.push_str(text.trim()); }
            else if *focus == 2 { prompt.push_str(text); }
        }
    }
}

pub fn open(hub: &Hub, ui: &mut Ui) {
    if let Some(panel) = ui.issue_panel.take() {
        if let Some(draft) = panel.draft() { ui.input = draft.into(); }
        return;
    }
    ui.choices = None;
    let target = crate::ui::reply_target(&hub.state.lock().unwrap(), ui);
    if let Some(id) = target {
        if hub.job(id, |j| j.issue_plan.is_some()).unwrap_or(false) {
            ui.issue_panel = Some(Panel::Detail { id, review: false, scroll: 0, selected: 0, error: String::new() });
            return;
        }
    }
    let repos = hub.cfg.repos.clone();
    let repo = repos.iter().enumerate().filter(|(_, r)| ui.cwd.starts_with(&r.path)).max_by_key(|(_, r)| r.path.components().count()).map(|(i, _)| i).unwrap_or(0);
    let prompt = if ui.input.trim().is_empty() { target.and_then(|id| hub.job(id, |j| j.prompt.clone())).unwrap_or_default() } else { ui.input.clone() };
    ui.issue_panel = Some(Panel::Setup { repos, repo, focus: 0, link: String::new(), prompt, task: target, error: String::new() });
}

pub fn key(hub: &Hub, ui: &mut Ui, history: &mut History, k: KeyEvent) -> bool {
    let Some(mut panel) = ui.issue_panel.take() else { return false };
    if let Some(draft) = panel.draft() { ui.input = draft.into(); }
    if k.code == KeyCode::Esc { return true; }
    if k.modifiers.contains(KeyModifiers::CONTROL) {
        if matches!(k.code, KeyCode::Char('t' | 'n')) { return false; }
        if matches!(k.code, KeyCode::Char('a' | 'x')) { ui.issue_panel = Some(panel); return false; }
        ui.issue_panel = Some(panel);
        return true;
    }
    let mut keep = true;
    match &mut panel {
        Panel::Setup { repos, repo, focus, link, prompt, task, error } => {
            match k.code {
                KeyCode::Tab => *focus = (*focus + 1) % 3,
                KeyCode::BackTab => *focus = (*focus + 2) % 3,
                KeyCode::Left | KeyCode::Up if *focus == 0 && !repos.is_empty() => *repo = (*repo + repos.len() - 1) % repos.len(),
                KeyCode::Right | KeyCode::Down if *focus == 0 && !repos.is_empty() => *repo = (*repo + 1) % repos.len(),
                KeyCode::Backspace => { if *focus == 1 { link.pop(); } else if *focus == 2 { prompt.pop(); } },
                KeyCode::Char(c) => { if *focus == 1 { link.push(c); } else if *focus == 2 { prompt.push(c); } },
                KeyCode::Enter => {
                    let result = (|| {
                        if hub.cfg.demo { return Err("Issue planning is disabled in demo mode.".into()); }
                        let r = repos.get(*repo).ok_or("Add a [[repos]] entry for GitHub or GitLab to configure issue planning.")?;
                        if prompt.trim().is_empty() { return Err("Enter the planning request in the third field.".into()); }
                        let p = Plan::new(r, link)?;
                        let id = if let Some(id) = task {
                            hub.rt.block_on(hub.link_plan(*id, p, prompt.clone()))?;
                            *id
                        } else {
                            hub.rt.block_on(hub.submit_with_plan(clip(prompt.trim().lines().next().unwrap_or("Implementation plan"), 100), prompt.trim().into(), Source::Manual, r.path.clone(), ui.forced.clone(), Planning::Never, Some(p)))?
                        };
                        history.submitted(prompt.trim().into());
                        ui.input.clear(); ui.selected = Some(id); ui.reply_to = Some(id); ui.new_task = false; ui.follow = false;
                        Ok(id)
                    })();
                    match result {
                        Ok(id) => { panel = Panel::Detail { id, review: false, scroll: 0, selected: 0, error: String::new() }; }
                        Err(e) => *error = e,
                    }
                }
                _ => {}
            }
        }
        Panel::Detail { id, review, scroll, selected, error } => {
            match k.code {
                KeyCode::Tab | KeyCode::BackTab => { *review = !*review; *scroll = 0; },
                KeyCode::Up => *scroll = scroll.saturating_sub(1),
                KeyCode::Down => *scroll = scroll.saturating_add(1),
                KeyCode::PageUp => *scroll = scroll.saturating_sub(10),
                KeyCode::PageDown => *scroll = scroll.saturating_add(10),
                KeyCode::Home => *scroll = 0,
                KeyCode::End => *scroll = u16::MAX,
                KeyCode::Left => *selected = (*selected + 2) % 3,
                KeyCode::Right => *selected = (*selected + 1) % 3,
                KeyCode::Char(c @ '1'..='3') => *selected = (c as u8 - b'1') as usize,
                KeyCode::Enter => {
                    if *selected == 1 {
                        ui.selected = Some(*id); ui.reply_to = Some(*id); ui.new_task = false; ui.follow = false; keep = false;
                    } else {
                        match hub.rt.block_on(hub.plan_action(*id, if *selected == 0 { Action::Publish } else { Action::Ready })) {
                            Ok(()) => error.clear(), Err(e) => *error = e,
                        }
                    }
                }
                _ => {}
            }
        }
    }
    if keep {
        if let Some(draft) = panel.draft() { ui.input = draft.into(); }
        ui.issue_panel = Some(panel);
    }
    true
}

pub fn draw(f: &mut Frame, st: &State, panel: &Panel) {
    let area = f.area();
    let area = Rect::new(area.x + 1.min(area.width), area.y + 1.min(area.height), area.width.saturating_sub(2), area.height.saturating_sub(2));
    f.render_widget(Clear, area);
    let block = Block::bordered().title(" Issue planning · F6 / Esc close ").style(Style::new().fg(Color::White).bg(Color::Rgb(13, 17, 23)));
    let inner = block.inner(area);
    f.render_widget(block, area);
    match panel {
        Panel::Setup { repos, repo, focus, link, prompt, task, error } => {
            let selected = |field| if *focus == field { "▶" } else { " " };
            let repository = repos.get(*repo).map(|r| format!("{} ({:?})", r.repo, r.provider)).unwrap_or_else(|| "No repositories configured; add [[repos]] first".into());
            let rows = Layout::vertical([Constraint::Length(3), Constraint::Length(3), Constraint::Min(2), Constraint::Length(4)]).split(inner);
            f.render_widget(Paragraph::new(format!("{} Repository: {}\n  ← / → choose repository · Tab next field", selected(0), repository)).wrap(Wrap { trim: false }), rows[0]);
            f.render_widget(Paragraph::new(format!("{} Existing issue URL / number: {}\n  Leave blank to create after review and acceptance", selected(1), link)).wrap(Wrap { trim: false }), rows[1]);
            f.render_widget(Paragraph::new(format!("{} Planning request{}:\n{}", selected(2), task.map(|id| format!(" · current task #{id}")).unwrap_or_default(), prompt)).wrap(Wrap { trim: false }), rows[2]);
            f.render_widget(Paragraph::new(format!("Enter: start read-only planning and review\nPublication and Ready for build each require your selection.\n{error}")).wrap(Wrap { trim: false }), rows[3]);
        }
        Panel::Detail { id, review, scroll, selected, error } => {
            let Some(j) = st.jobs.iter().find(|j| j.id == *id) else { return };
            let Some(p) = &j.issue_plan else { return };
            let rows = Layout::vertical([Constraint::Length(3), Constraint::Min(1), Constraint::Length(5)]).split(inner);
            f.render_widget(Paragraph::new(format!("{}\n{} · {} · Tab: plan / reviewer feedback", p.status(st.automation), j.status.name(), if *review { "Reviewer feedback" } else { "Full plan" })).wrap(Wrap { trim: false }), rows[0]);
            let text = if *review { &p.review } else { &p.draft };
            let text = if text.is_empty() { "Waiting for agent output…" } else { text };
            let paragraph = Paragraph::new(text).wrap(Wrap { trim: false });
            let max_scroll = paragraph.line_count(rows[1].width).saturating_sub(rows[1].height as usize).min(u16::MAX as usize) as u16;
            f.render_widget(paragraph.scroll(((*scroll).min(max_scroll), 0)), rows[1]);
            let actions = ["1 Accept / retry", "2 Refine", "3 Ready for build"];
            let actions = actions.iter().enumerate().map(|(i, label)| format!("{} {}", if i == *selected { "▶" } else { " " }, label)).collect::<Vec<_>>().join("  ");
            let outcome = if error.is_empty() { j.outcome.as_str() } else { error.as_str() };
            f.render_widget(Paragraph::new(format!("{actions}\n← / → select · Enter apply · ↑↓ / PgUp/PgDn scroll\n{outcome}")).wrap(Wrap { trim: false }), rows[2]);
        }
    }
}
