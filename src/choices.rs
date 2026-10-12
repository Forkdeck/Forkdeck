//! Planning choices in worker answers. Code blocks and ordinary step lists are not menus.
use crate::state::Job;
use regex::Regex;
use std::sync::LazyLock;

#[derive(Clone, Debug, PartialEq)]
pub struct Choice {
    pub label: String,
    pub detail: String,
}

pub struct Picker {
    pub job: u32,
    pub turn: usize,
    pub options: Vec<Choice>,
    /// The entry after the options is always a custom response.
    pub selected: usize,
}

pub fn for_job(job: &Job) -> Vec<Choice> {
    if !job.source.is_manual() { return Vec::new(); }
    job.turns.last().map(|turn| {
        // An interrupted Build can still have its completed plan as the latest answer.
        if !job.planning && job.plan.as_deref() == Some(turn.answer.as_str()) {
            Vec::new()
        } else {
            parse(&turn.answer)
        }
    }).unwrap_or_default()
}

pub fn parse(text: &str) -> Vec<Choice> {
    static CUE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(options|choices|alternatives|choose|prefer)\b").unwrap());
    static NUMBER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(?:[-*]\s+)?(?:\*\*)?([1-9][0-9]?|[A-Z])[.)]\s+(.+)$").unwrap());
    let mut options: Vec<Choice> = Vec::new();
    let mut fenced = false;
    let mut table = false;
    let mut cue = false;
    for raw in text.lines() {
        let line = raw.trim();
        if line.starts_with("```") || line.starts_with("~~~") { fenced = !fenced; continue; }
        if fenced { continue; }
        cue |= CUE.is_match(line);
        if line.starts_with('|') {
            let cells: Vec<_> = line.trim_matches('|').split('|').map(str::trim).collect();
            if cells.first().is_some_and(|s| matches!(s.trim_matches('*').to_lowercase().as_str(), "option" | "choice" | "approach")) {
                table = true;
                continue;
            }
            if table && cells.len() >= 2 && NUMBER.is_match(cells[0]) {
                options.push(Choice { label: cells[0].trim_matches('*').to_string(), detail: cells[1..].join(" · ") });
            }
        } else {
            table = false;
            if cue && NUMBER.is_match(line) {
                options.push(Choice { label: line.trim_matches('*').to_string(), detail: String::new() });
            } else if raw.starts_with("  ") && !line.is_empty() {
                if let Some(last) = options.last_mut() {
                    if !last.detail.is_empty() { last.detail.push(' '); }
                    last.detail.push_str(line);
                }
            }
        }
        if options.len() > 16 { return Vec::new(); }
    }
    if options.len() < 2 { return Vec::new(); }
    options
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbered_and_table_choices_keep_descriptions() {
        let choices = parse("Which approach do you prefer?\n1. Minimal\n   Keeps the current layout.\n2. Full redesign");
        assert_eq!(choices.len(), 2);
        assert_eq!(choices[0].detail, "Keeps the current layout.");
        let choices = parse("Planning options:\n| Option | What you get | Tradeoff |\n|---|---|---|\n| **1. Measurement MVP** | Token tracking | Limited evidence |\n| **2. Comparisons — recommended** | Benchmarks | More work |");
        assert_eq!(choices[1].label, "2. Comparisons — recommended");
        assert_eq!(choices[1].detail, "Benchmarks · More work");
    }

    #[test]
    fn steps_and_quoted_code_are_not_choices() {
        assert!(parse("Completed:\n1. Read the files\n2. Fixed the bug").is_empty());
        assert!(parse("Options:\n```\n1. example\n2. example\n```\n> 1. quoted\n> 2. quoted").is_empty());
        assert!(parse("Options:\n1. Only one").is_empty());
    }

    #[test]
    fn selecting_and_custom_answers_reply_to_the_original_task() {
        use crate::{config::Config, input::History, orchestrator::Hub, state::*};
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let hub = Hub::new(Config::default(), rt.handle().clone());
        let mut ui = crate::new_ui(&hub, ".".into());
        let mut history = History::default();
        let id = hub.submit("plan".into(), "give me planning options".into(), Source::Manual, ".".into(), None);
        hub.job(id, |j| { j.status = JobStatus::Done; j.turns[0].answer = "Options:\n1. Minimal\n2. Full".into(); });
        ui.selected = Some(id);
        crate::sync_choices(&hub, &mut ui);
        assert_eq!(ui.choices.as_ref().unwrap().options.len(), 2);
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        crate::choice_key(&hub, &mut ui, &mut history, key(KeyCode::Down));
        crate::choice_key(&hub, &mut ui, &mut history, key(KeyCode::Enter));
        assert_eq!(hub.state.lock().unwrap().jobs.len(), 1);
        assert!(hub.job(id, |j| j.turns[1].ask.contains("2. Full")).unwrap());
        assert!(ui.choices.is_none());
        assert_eq!(hub.job(id, |j| j.planning), Some(true));

        hub.job(id, |j| { j.status = JobStatus::Done; j.turns[1].answer = "Choose:\n1. Fast\n2. Thorough".into(); });
        crate::sync_choices(&hub, &mut ui);
        crate::choice_key(&hub, &mut ui, &mut history, key(KeyCode::Down));
        crate::choice_key(&hub, &mut ui, &mut history, key(KeyCode::Down));
        crate::choice_key(&hub, &mut ui, &mut history, key(KeyCode::Enter));
        assert!(ui.choices.is_none());
        assert_eq!(ui.reply_to, Some(id));
        assert_eq!(hub.job(id, |j| j.turns.len()), Some(2), "custom entry must not submit a canned answer");
        ui.input = "Combine both, but keep it small".into();
        crate::submit_input(&hub, &mut ui, &mut history);
        assert_eq!(hub.state.lock().unwrap().jobs.len(), 1);
        assert_eq!(hub.job(id, |j| j.turns[2].ask.clone()).unwrap(), "Combine both, but keep it small");
        assert_eq!(hub.job(id, |j| j.planning), Some(true));

        hub.job(id, |j| {
            j.status = JobStatus::Cancelled;
            j.planning = false;
            let plan = "Selected options:\n1. Implement it\n2. Test it\nPLAN: READY";
            j.plan = Some(plan.into());
            j.turns[2].answer = plan.into();
            assert!(for_job(j).is_empty(), "resuming Build must not reopen completed planning choices");
        });
        hub.reply(id, "continue".into()).unwrap();
        assert_eq!(hub.job(id, |j| j.planning), Some(false));
    }
}
