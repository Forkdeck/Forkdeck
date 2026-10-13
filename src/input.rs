//! Session-local submitted messages, with an unfinished draft preserved during recall.
#[derive(Default)]
pub struct History {
    messages: Vec<String>,
    cursor: Option<usize>,
    draft: String,
}

impl History {
    pub fn messages(&self) -> &[String] { &self.messages }

    pub fn submitted(&mut self, message: String) {
        self.messages.push(message);
        if self.messages.len() > 100 {
            self.messages.remove(0);
        }
        self.reset();
    }

    pub fn reset(&mut self) {
        self.cursor = None;
        self.draft.clear();
    }

    pub fn older(&mut self, input: &mut String) {
        if self.messages.is_empty() { return; }
        let index = match self.cursor {
            Some(index) => index.saturating_sub(1),
            None => {
                self.draft = input.clone();
                self.messages.len() - 1
            }
        };
        self.cursor = Some(index);
        *input = self.messages[index].clone();
    }

    pub fn newer(&mut self, input: &mut String) {
        if let Some(index) = self.cursor {
            if index + 1 < self.messages.len() {
                self.cursor = Some(index + 1);
                *input = self.messages[index + 1].clone();
            } else {
                self.cursor = None;
                *input = std::mem::take(&mut self.draft);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recall_preserves_draft_and_original_after_editing() {
        let mut history = History::default();
        let mut input = "draft 日本語".to_string();
        history.older(&mut input);
        assert_eq!(input, "draft 日本語");
        history.submitted("first".into());
        history.submitted("second".into());
        history.older(&mut input);
        assert_eq!(input, "second");
        input.push_str(" edited");
        history.older(&mut input);
        history.older(&mut input);
        assert_eq!(input, "first");
        history.newer(&mut input);
        assert_eq!(input, "second");
        history.newer(&mut input);
        history.newer(&mut input);
        assert_eq!(input, "draft 日本語");
    }

    #[test]
    fn resubmission_and_clearing_reset_navigation() {
        let mut history = History::default();
        history.submitted("first".into());
        let mut input = String::new();
        history.older(&mut input);
        input.push_str(" edited");
        history.submitted(input.clone());
        input.clear();
        history.older(&mut input);
        assert_eq!(input, "first edited");
        history.reset();
        input.clear();
        history.newer(&mut input);
        assert!(input.is_empty());
    }
}
