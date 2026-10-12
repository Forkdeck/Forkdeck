//! Turns what was asked for (claude, codex, copilot, copilot:<profile>) into what runs: which CLI,
//! which model, and whose model it is. Pure: everything it needs is passed in.

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Vendor {
    Anthropic,
    OpenAI,
    Other(String),
    Unknown,
}

pub fn vendor_of_model(id: &str) -> Vendor {
    let id = id.trim().to_ascii_lowercase();
    let mut c = id.chars();
    if id.starts_with("claude-") {
        Vendor::Anthropic
    } else if id.starts_with("gpt-") || (c.next() == Some('o') && c.next().is_some_and(|d| d.is_ascii_digit())) {
        Vendor::OpenAI
    } else {
        Vendor::Unknown
    }
}
