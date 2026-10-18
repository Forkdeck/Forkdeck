//! The full-screen diff of what an agent edited, opened by clicking an edit line in its output.

use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The files an output line says were edited: Codex's `edit a, b`, Claude's `Edit a`, `Write a`
/// or `MultiEdit a`. Anything else is not an edit line.
pub fn edited_files(line: &str) -> Option<Vec<String>> {
    let rest = line.strip_prefix("▸ ")?;
    let (verb, paths) = rest.split_once(' ')?;
    if !matches!(verb, "edit" | "Edit" | "Write" | "MultiEdit") {
        return None;
    }
    let files: Vec<String> = paths.split(", ").map(str::trim).filter(|p| !p.is_empty()).map(String::from).collect();
    (!files.is_empty()).then_some(files)
}

/// Rows of an output pane that hold an edit line, as drawn, with the files it names.
#[derive(Clone, Debug, PartialEq)]
pub struct Spot {
    pub area: Rect,
    pub job: u32,
    pub files: Vec<String>,
}

/// The edit line under a click, if any.
pub fn spot_at(spots: &[Spot], column: u16, row: u16) -> Option<&Spot> {
    spots.iter().find(|s| column >= s.area.x && column < s.area.x + s.area.width && row >= s.area.y && row < s.area.y + s.area.height)
}

/// What the files look like now against the commit the job started from. A file git does not
/// track yet is shown as wholly added. Paths may be absolute inside the worktree, or relative to it.
pub fn job_diff(workdir: &Path, base: &str, files: &[String]) -> String {
    let rel: Vec<String> = files.iter().map(|f| Path::new(f).strip_prefix(workdir).map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| f.clone())).collect();
    let git = |args: &[&str]| {
        std::process::Command::new("git").arg("-C").arg(workdir).args(args).stderr(std::process::Stdio::null()).output().ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default()
    };
    let mut out = String::new();
    for f in &rel {
        let tracked = !git(&["ls-files", "--", f]).trim().is_empty() || !git(&["ls-tree", "--name-only", base, "--", f]).trim().is_empty();
        let d = if tracked { git(&["diff", "--no-color", base, "--", f]) } else { git(&["diff", "--no-color", "--no-index", "/dev/null", f]) };
        out.push_str(if d.trim().is_empty() { "" } else { &d });
        if d.trim().is_empty() {
            out.push_str(&format!("{}: no change against the starting commit\n", f));
        }
    }
    out
}

/// The open view: which job and files, where it is scrolled to, and the latest diff, which a
/// background thread refreshes so git never runs on the interface's thread.
pub struct View {
    pub job: u32,
    pub files: Vec<String>,
    pub scroll: u16,
    text: Arc<Mutex<String>>,
    refreshing: Arc<std::sync::atomic::AtomicBool>,
    last: Instant,
    workdir: PathBuf,
    base: String,
}

impl View {
    pub fn open(job: u32, files: Vec<String>, workdir: PathBuf, base: String) -> View {
        let mut v = View { job, files, scroll: 0, text: Arc::new(Mutex::new("loading the diff...".into())), refreshing: Default::default(), last: Instant::now() - Duration::from_secs(60), workdir, base };
        v.refresh();
        v
    }

    /// Fetch the diff again in the background, at most every two seconds, while the agent edits.
    pub fn refresh(&mut self) {
        use std::sync::atomic::Ordering;
        if self.last.elapsed() < Duration::from_secs(2) || self.refreshing.swap(true, Ordering::SeqCst) {
            return;
        }
        self.last = Instant::now();
        let (text, busy, dir, base, files) = (self.text.clone(), self.refreshing.clone(), self.workdir.clone(), self.base.clone(), self.files.clone());
        std::thread::spawn(move || {
            let d = job_diff(&dir, &base, &files);
            if let Ok(mut t) = text.lock() { *t = d; }
            busy.store(false, Ordering::SeqCst);
        });
    }

    pub fn text(&self) -> String {
        self.text.lock().map(|t| t.clone()).unwrap_or_default()
    }

    pub fn scroll_by(&mut self, rows: i32, page: u16) {
        let max = self.text().lines().count().saturating_sub(page as usize) as i32;
        self.scroll = (self.scroll as i32 + rows).clamp(0, max.max(0)) as u16;
    }
}

fn styled(line: &str) -> Line<'_> {
    let color = if line.starts_with("+++") || line.starts_with("---") || line.starts_with("diff ") || line.starts_with("index ") {
        Color::Rgb(150, 150, 170)
    } else if line.starts_with('+') {
        Color::Rgb(110, 200, 120)
    } else if line.starts_with('-') {
        Color::Rgb(230, 100, 100)
    } else if line.starts_with("@@") {
        Color::Rgb(120, 160, 230)
    } else {
        Color::Rgb(210, 210, 215)
    };
    Line::from(Span::styled(line, Style::new().fg(color)))
}

/// The whole screen: the diff, coloured, scrolled to where the user left it.
pub fn draw(f: &mut Frame, area: Rect, v: &View) {
    let text = v.text();
    let lines: Vec<Line> = text.lines().map(styled).collect();
    let title = format!(" #{} · {} · Esc closes · ↑↓ PgUp PgDn scroll ", v.job, v.files.join(", "));
    f.render_widget(Paragraph::new(lines).scroll((v.scroll, 0)).block(Block::default().borders(Borders::ALL).title(title)), area);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edit_lines_from_either_cli_name_their_files() {
        assert_eq!(edited_files("▸ edit /w/a.rs, /w/b.rs"), Some(vec!["/w/a.rs".to_string(), "/w/b.rs".to_string()]));
        assert_eq!(edited_files("▸ Edit src/a.rs"), Some(vec!["src/a.rs".to_string()]));
        assert_eq!(edited_files("▸ Write docs/new.md"), Some(vec!["docs/new.md".to_string()]));
        assert_eq!(edited_files("▸ shell /bin/bash -lc 'edit me'"), None);
        assert_eq!(edited_files("edit a.rs"), None, "only tool lines");
    }

    #[test]
    fn a_click_finds_the_edit_line_under_it() {
        let spots = vec![Spot { area: Rect::new(2, 5, 40, 2), job: 3, files: vec!["a.rs".into()] }];
        assert_eq!(spot_at(&spots, 10, 6).map(|s| s.job), Some(3), "the wrapped second row counts");
        assert!(spot_at(&spots, 10, 7).is_none());
        assert!(spot_at(&spots, 1, 5).is_none());
    }

    #[test]
    fn the_diff_shows_changed_and_new_files_against_the_starting_commit() {
        let dir = std::env::temp_dir().join(format!("forkdeck-diff-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let git = |args: &[&str]| assert!(std::process::Command::new("git").arg("-C").arg(&dir).args(args).output().unwrap().status.success(), "git {:?}", args);
        git(&["init", "-q", "-b", "main"]);
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
        git(&["add", "."]);
        git(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "c"]);
        let base = String::from_utf8(std::process::Command::new("git").arg("-C").arg(&dir).args(["rev-parse", "HEAD"]).output().unwrap().stdout).unwrap().trim().to_string();
        std::fs::write(dir.join("a.txt"), "one\ntwo\n").unwrap();
        std::fs::write(dir.join("new.txt"), "fresh\n").unwrap();
        let abs = dir.join("a.txt").to_string_lossy().into_owned();
        let d = job_diff(&dir, &base, &[abs, "new.txt".into()]);
        assert!(d.contains("+two") && d.contains("+fresh"), "{d}");
        assert!(!d.contains("-one"), "{d}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_view_fills_the_screen_and_colours_the_diff() {
        let v = View { job: 9, files: vec!["a.rs".into()], scroll: 0, text: Arc::new(Mutex::new("@@ -1 +1 @@\n-old\n+new\n".into())), refreshing: Default::default(), last: Instant::now(), workdir: ".".into(), base: "HEAD".into() };
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(60, 10)).unwrap();
        term.draw(|f| draw(f, f.area(), &v)).unwrap();
        let text = crate::ui::to_text(term.backend().buffer());
        assert!(text.contains("#9 · a.rs") && text.contains("+new") && text.contains("-old"), "{text}");
        let buf = term.backend().buffer();
        let y = (0..10).find(|y| (0..60).map(|x| buf[(x, *y)].symbol()).collect::<String>().contains("+new")).unwrap();
        let x = (0..60).find(|x| buf[(*x, y)].symbol() == "+").unwrap();
        assert_eq!(buf[(x, y)].fg, Color::Rgb(110, 200, 120));
    }
}
