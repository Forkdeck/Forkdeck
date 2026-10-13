//! Planning choices in worker answers. Code blocks and ordinary step lists are not menus.
use crate::state::{Job, JobStatus};
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
    // A plan waiting for its choice offers its own options.
    if job.status == JobStatus::Choosing {
        return job.plan.as_ref().filter(|p| p.chosen.is_none()).map(|p| p.options.iter().enumerate().map(|(i, o)| Choice {
            label: format!("{}. {}{}", i + 1, o.title, if o.recommended { " (recommended)" } else { "" }),
            detail: o.summary.clone(),
        }).collect()).unwrap_or_default();
    }
    if !job.status.finished() { return Vec::new(); }
    job.turns.last().map(|turn| {
        // The plan's own answer, after a choice was made, is not a question any more.
        if job.plan.as_ref().is_some_and(|p| p.text == turn.answer) { Vec::new() } else { parse(&turn.answer) }
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
    fn a_waiting_plan_offers_its_options_and_a_chosen_one_does_not() {
        use crate::state::*;
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let hub = crate::orchestrator::Hub::new(crate::config::Config::default(), rt.handle().clone());
        hub.state.lock().unwrap().stopping = true;
        let id = rt.block_on(hub.submit("plan".into(), "plan it".into(), Source::Manual, ".".into(), None, Planning::Never)).unwrap();
        let option = |t: &str, r: bool| PlanOption { title: t.into(), summary: format!("about {t}"), recommended: r };
        hub.job(id, |j| {
            j.status = JobStatus::Choosing;
            j.plan = Some(Plan { options: vec![option("Patch", false), option("Rewrite", true)], text: "the plan".into(), chosen: None });
            let got = for_job(j);
            assert_eq!(got.iter().map(|c| c.label.as_str()).collect::<Vec<_>>(), ["1. Patch", "2. Rewrite (recommended)"]);
            assert_eq!(got[0].detail, "about Patch");
            j.status = JobStatus::Interrupted;
            j.turns[0].answer = "the plan".into();
            assert!(for_job(j).is_empty(), "the plan's own text is not asked again once it was chosen from");
            j.turns[0].answer = "Which do you prefer?\n1. Small\n2. Large".into();
            assert_eq!(for_job(j).len(), 2, "a finished answer that asks is offered");
        });
    }
}
